//! Pandas (`Panda` and its nested classes; `docs/groundwork/n7-brief.md`, section 6.2):
//! an animal of bamboo whose two genes give its mood (lazy, worried, playful, weak,
//! aggressive, brown or normal): it sits to eat bamboo and cake it picks up or is fed,
//! rolls, lies on its back, sneezes as a cub, and breeds only with bamboo near.
//!
//! Its three counters (unhappy, sneezing, eating) are synced data that change every tick
//! while they run, as the game's do (each such tick is in the commit).
//!
//! Named differences: the weather is clear, so no worried panda is ever scared and no
//! storm sits it down; fire, lava and the like of `#panic_environmental_causes` are not
//! seen, so it never panics; a hurt panda's alert of the aggressive pandas near it
//! (`alertOthers`) writes their targets, which is not built; the eating particles' and
//! the sneeze's draws are made and the particles not sent; the sneeze's gift
//! (`gameplay/panda_sneeze`: a slime ball, weight 1 against 699 of nothing) draws from
//! the panda's own stream where the game uses the table's random sequence; the pandas a
//! sneeze makes jump are pushed up to a jump's height rather than jumping
//! (`jumpFromGround` of another mob); the amounts of sitting, lying and rolling are the
//! client's looks alone; its pick-ups take only its own region's items; a panda fed in
//! water is taken as out of it (a use reads no blocks).

use clustine_blocks::data::{has_water, is_block};
use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, cow};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Attacking, AvoidEntityGoal, Avoiding, FloatGoal, HurtByTargetGoal, Items, LookingAt,
    MeleeAttackGoal, RandomLookAroundGoal, RandomStrollGoal, Targeting, TemptGoal, ai_bounds,
    melee_hit,
};
use crate::mobs::ai::maths::{rotate_towards, table_cos, table_sin};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::{Ai, Setup, controls, navigation};
use crate::mobs::body::Event;
use crate::mobs::body::attribute;
use crate::mobs::body::hurt::{DamageSource, GameEvent};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::{self, Vec3};
use crate::shown::DataShown;

/// `#panda_food`: bamboo.
const FOOD: Items = Items::Tag("minecraft:panda_food");

/// `#panda_eats_from_ground`: bamboo and cake.
const EATS_FROM_GROUND: Items = Items::Tag("minecraft:panda_eats_from_ground");

/// A cub's size: the type's (1.3 by 1.25) scaled by a half, its eyes at 0.28125.
const BABY: Dimensions = Dimensions {
    width: 1.3 * 0.5,
    height: 1.25 * 0.5,
    eye_height: 0.281_25,
};

/// The synced data's indices.
const UNHAPPY_INDEX: u8 = 18;
const SNEEZE_INDEX: u8 = 19;
const EAT_INDEX: u8 = 20;
const MAIN_GENE_INDEX: u8 = 21;
const HIDDEN_GENE_INDEX: u8 = 22;
const FLAGS_INDEX: u8 = 23;

/// The bits of `DATA_ID_FLAGS`.
pub(crate) mod flag_bits {
    pub(crate) const SNEEZING: u8 = 2;
    pub(crate) const ROLLING: u8 = 4;
    pub(crate) const SITTING: u8 = 8;
    pub(crate) const ON_BACK: u8 = 16;
}

/// `Panda$Gene`'s ids.
pub(crate) mod gene {
    pub(crate) const NORMAL: u8 = 0;
    pub(crate) const LAZY: u8 = 1;
    pub(crate) const WORRIED: u8 = 2;
    pub(crate) const PLAYFUL: u8 = 3;
    pub(crate) const BROWN: u8 = 4;
    pub(crate) const WEAK: u8 = 5;
    pub(crate) const AGGRESSIVE: u8 = 6;

    /// `isRecessive()`: brown and weak.
    pub(crate) fn recessive(gene: u8) -> bool {
        gene == BROWN || gene == WEAK
    }
}

/// `Gene.getRandom(random)`: one of sixteen, a weak gene five times, normal five, brown
/// twice, the others once.
pub(crate) fn random_gene(random: &mut dyn clustine_world::generation::Draws) -> u8 {
    match random.next_int(16) {
        0 => gene::LAZY,
        1 => gene::WORRIED,
        2 => gene::PLAYFUL,
        4 => gene::AGGRESSIVE,
        drawn if drawn < 9 => gene::WEAK,
        drawn if drawn < 11 => gene::BROWN,
        _ => gene::NORMAL,
    }
}

/// `getVariantFromGenes(main, hidden)`: a recessive main gene shows only when the hidden
/// one is the same.
pub(crate) fn variant_of(main: u8, hidden: u8) -> u8 {
    if gene::recessive(main) {
        if main == hidden { main } else { gene::NORMAL }
    } else {
        main
    }
}

/// What a panda keeps beyond an animal: its genes; its three counters and flags; its
/// roll's ticks and push; whether it was given bamboo or bit while it had a target; the
/// player its look goal was set on (by its breeding's disappointment); its goals'
/// cooldowns that count by its `tickCount`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PandaState {
    pub(crate) main_gene: u8,
    pub(crate) hidden_gene: u8,
    pub(crate) unhappy: i32,
    pub(crate) sneeze: i32,
    pub(crate) eat: i32,
    pub(crate) flags: u8,
    pub(crate) roll_counter: i32,
    pub(crate) roll_delta: Vec3,
    pub(crate) got_bamboo: bool,
    pub(crate) did_bite: bool,
    pub(crate) look_at: Option<EntityId>,
}

impl Own for PandaState {}

impl PandaState {
    pub(crate) fn has(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }

    pub(crate) fn set(&mut self, bit: u8, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }

    pub(crate) fn variant(&self) -> u8 {
        variant_of(self.main_gene, self.hidden_gene)
    }

    fn is_eating(&self) -> bool {
        self.eat > 0
    }

    /// `eat(eating)`: the counter at 1, or nothing.
    fn eat(&mut self, eating: bool) {
        self.eat = i32::from(eating);
    }

    /// `sneeze(sneezing)`: the flag, and the counter back to nothing when it stops.
    fn sneeze(&mut self, sneezing: bool) {
        self.set(flag_bits::SNEEZING, sneezing);
        if !sneezing {
            self.sneeze = 0;
        }
    }

    /// `isScared()`: a worried panda in a storm (never: the weather is clear).
    fn is_scared(&self) -> bool {
        false
    }

    /// `canPerformAction()`: not on its back, scared, eating, rolling or sitting.
    fn can_perform_action(&self) -> bool {
        use flag_bits::*;
        !self.has(ON_BACK)
            && !self.is_scared()
            && !self.is_eating()
            && !self.has(ROLLING)
            && !self.has(SITTING)
    }
}

/// The panda.
pub(crate) struct Panda;

pub(crate) fn state_of(mob: &Mob) -> PandaState {
    animal::read::<PandaState>(&mob.state).1
}

fn put_state(mob: &mut Mob, own: &PandaState) {
    let animal = animal::animal_of(&mob.state);
    mob.state = animal::write(&animal, own);
}

fn update(mob: &mut Mob, change: impl FnOnce(&mut PandaState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

fn holds_something(mob: &Mob) -> bool {
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_some_and(|held| held.count > 0)
}

/// `setAttributes()`: a weak panda's health at most 10, a lazy one's speed 0.07.
fn set_attributes(mob: &mut Mob, kind: u16) {
    let variant = state_of(mob).variant();
    if variant == gene::WEAK
        && let Some(instance) = mob
            .body
            .attributes
            .instance(kind, attribute(clustine_data::attributes::MAX_HEALTH))
    {
        instance.base = 10.0;
    }
    if variant == gene::LAZY
        && let Some(instance) = mob
            .body
            .attributes
            .instance(kind, attribute(clustine_data::attributes::MOVEMENT_SPEED))
    {
        instance.base = f64::from(0.07_f32);
    }
}

/// `tryToSit()`: out of water it stops and sits.
fn try_to_sit(ai: &mut Ai<'_, '_>) {
    if ai.in_water() {
        return;
    }
    ai.tick.mob.body.inputs.zza = 0.0;
    navigation::stop(ai);
    update(&mut ai.tick.mob, |own| own.set(flag_bits::SITTING, true));
}

/// What a panda takes up from the ground (`canPickUpAndEat`): bamboo or cake, without a
/// pick-up delay, of its own region.
fn eatable_items(ai: &Ai<'_, '_>, bounds: crate::physics::aabb::Aabb) -> Vec<(EntityId, Vec3)> {
    ai.tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                stack,
                pick_up_delay: 0,
                ..
            } if EATS_FROM_GROUND.test(Some(stack)) => {
                Some((seen.entity.key, seen.entity.position))
            }
            _ => None,
        })
        .collect()
}

/// `PandaBreedGoal(panda, 1.0)`'s state: a breeding's, and its cooldown of
/// disappointment by `tickCount`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PandaCourting {
    pub(crate) courting: animal::Courting,
    pub(crate) unhappy_cooldown: u32,
}

/// `PandaBreedGoal(panda, 1.0)`: a breeding that needs bamboo within seven blocks; a
/// panda in love without it is unhappy for a while and looks at the player near.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaBreedGoal;

impl PandaBreedGoal {
    const BREED: animal::BreedGoal = animal::BreedGoal { speed: 1.0 };

    /// `canFindBamboo()`: bamboo at the panda's feet or up to two above, in rings of up
    /// to seven around it.
    fn can_find_bamboo(ai: &Ai<'_, '_>) -> bool {
        let at = ai.block_position();
        let next = |d: i32| if d > 0 { -d } else { 1 - d };
        for dy in 0..3 {
            for r in 0..8 {
                let mut dx = 0;
                while dx <= r {
                    let mut dz = if dx < r && dx > -r { r } else { 0 };
                    while dz <= r {
                        let pos = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
                        if is_block(ai.tick.world.state(pos), blocks::BAMBOO) {
                            return true;
                        }
                        dz = next(dz);
                    }
                    dx = next(dx);
                }
            }
        }
        false
    }
}

impl Goal for PandaBreedGoal {
    type State = PandaCourting;

    fn flags(&self) -> u8 {
        Self::BREED.flags()
    }

    fn can_use(&self, state: &mut PandaCourting, ai: &mut Ai<'_, '_>) -> bool {
        if !Self::BREED.can_use(&mut state.courting, ai) || state_of(&ai.tick.mob).unhappy != 0 {
            return false;
        }
        if Self::can_find_bamboo(ai) {
            return true;
        }
        let tick_count = ai.tick_count();
        if state.unhappy_cooldown <= tick_count {
            state.unhappy_cooldown = tick_count + 600;
            let conditions = Conditions::non_combat().range(8.0);
            let [x, _, z] = ai.position();
            let eyes = [x, ai.eye_y(), z];
            let player = sensing::nearest_player(ai, &conditions, &|_| true, eyes);
            update(&mut ai.tick.mob, |own| {
                own.unhappy = 32;
                own.look_at = player.map(|player| player.key);
            });
        }
        false
    }

    fn can_continue(&self, state: &mut PandaCourting, ai: &mut Ai<'_, '_>) -> bool {
        Self::BREED.can_continue(&mut state.courting, ai)
    }

    fn start(&self, state: &mut PandaCourting, ai: &mut Ai<'_, '_>) {
        Self::BREED.start(&mut state.courting, ai);
    }

    fn stop(&self, state: &mut PandaCourting, ai: &mut Ai<'_, '_>) {
        Self::BREED.stop(&mut state.courting, ai);
    }

    fn tick(&self, state: &mut PandaCourting, ai: &mut Ai<'_, '_>) {
        Self::BREED.tick(&mut state.courting, ai);
    }
}

/// `PandaAttackGoal(panda, 1.2f, true)`: a melee attack, only while it may act; a bite
/// by a panda without the aggressive gene ends its anger.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaAttackGoal;

impl PandaAttackGoal {
    const MELEE: MeleeAttackGoal = MeleeAttackGoal {
        speed: 1.200_000_047_683_715_8,
        following_even_if_not_seen: true,
    };
}

impl Goal for PandaAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        Self::MELEE.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        state_of(&ai.tick.mob).can_perform_action() && Self::MELEE.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_continue(state, ai)
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.start(state, ai);
    }

    fn stop(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.stop(state, ai);
    }

    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = Self::MELEE.chase(state, ai) else {
            return;
        };
        if MeleeAttackGoal::can_perform_attack(state, ai, &target) {
            Self::MELEE.reset_attack_cooldown(state);
            // `Panda.doHurtTarget`.
            update(&mut ai.tick.mob, |own| {
                if own.variant() != gene::AGGRESSIVE {
                    own.did_bite = true;
                }
            });
            melee_hit(ai, &target);
        }
    }
}

/// `PandaAvoidGoal<T>(panda, class, distance, 2.0, 2.0)`: a worried panda free to act
/// runs from players within 8 and monsters within 4 (never a creative player).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaAvoidGoal {
    pub(crate) avoid: AvoidEntityGoal,
}

impl Goal for PandaAvoidGoal {
    type State = Avoiding;

    fn flags(&self) -> u8 {
        self.avoid.flags()
    }

    fn can_use(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        own.variant() == gene::WORRIED && own.can_perform_action() && self.avoid.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        self.avoid.can_continue(state, ai)
    }

    fn start(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        self.avoid.start(state, ai);
    }

    fn stop(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        self.avoid.stop(state, ai);
    }

    fn tick(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        self.avoid.tick(state, ai);
    }
}

/// `PandaSitGoal`'s cooldown, by `tickCount`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sitting {
    pub(crate) cooldown: u32,
}

/// `PandaSitGoal`: a grown panda at ease with something to eat in hand or near goes to
/// it and sits down to eat; it gets up now and then, dropping what it holds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaSitGoal;

/// `canContinueToUse` of the sit and lie goals: out of water; one time in 300 for a
/// panda not lazy, then one in 1000, it stops.
fn sits_on(ai: &mut Ai<'_, '_>) -> bool {
    if ai.in_water() {
        return false;
    }
    if state_of(&ai.tick.mob).variant() != gene::LAZY && ai.tick.draws.next_int(300) == 1 {
        return false;
    }
    ai.tick.draws.next_int(1000) != 1
}

impl Goal for PandaSitGoal {
    type State = Sitting;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        if state.cooldown > ai.tick_count()
            || animal::is_baby(&ai.tick.mob)
            || ai.in_water()
            || !own.can_perform_action()
            || own.unhappy > 0
        {
            return false;
        }
        if holds_something(&ai.tick.mob) {
            return true;
        }
        let bounds = ai_bounds(ai).inflate([6.0, 6.0, 6.0]);
        !eatable_items(ai, bounds).is_empty()
    }

    fn can_continue(&self, _: &mut Sitting, ai: &mut Ai<'_, '_>) -> bool {
        sits_on(ai)
    }

    fn start(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) {
        if holds_something(&ai.tick.mob) {
            try_to_sit(ai);
        } else {
            let bounds = ai_bounds(ai).inflate([8.0, 8.0, 8.0]);
            if let Some((_, at)) = eatable_items(ai, bounds).first() {
                navigation::move_to_entity(ai, physics::containing(*at), 1.200_000_047_683_715_8);
            }
        }
        state.cooldown = 0;
    }

    fn stop(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) {
        let hand = usize::from(slots::MAIN_HAND);
        if let Some(held) = ai.tick.mob.body.equipment.slots[hand].filter(|held| held.count > 0) {
            let at = ai.position();
            ai.tick.out.local.push(Local::Drop {
                stack: held,
                at,
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            });
            ai.tick.mob.body.equipment.slots[hand] = None;
            let lazy = state_of(&ai.tick.mob).variant() == gene::LAZY;
            let d = if lazy {
                ai.tick.draws.next_int(50) + 10
            } else {
                ai.tick.draws.next_int(150) + 10
            };
            state.cooldown = ai.tick_count() + d * 20;
        }
        update(&mut ai.tick.mob, |own| own.set(flag_bits::SITTING, false));
    }

    fn tick(&self, _: &mut Sitting, ai: &mut Ai<'_, '_>) {
        if !state_of(&ai.tick.mob).has(flag_bits::SITTING) && holds_something(&ai.tick.mob) {
            try_to_sit(ai);
        }
    }
}

/// `PandaLieOnBackGoal`: a lazy panda free to act lies on its back now and then.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaLieOnBackGoal;

impl Goal for PandaLieOnBackGoal {
    type State = Sitting;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        state.cooldown < ai.tick_count()
            && own.variant() == gene::LAZY
            && own.can_perform_action()
            && ai.tick.draws.next_int(200) == 1
    }

    fn can_continue(&self, _: &mut Sitting, ai: &mut Ai<'_, '_>) -> bool {
        sits_on(ai)
    }

    fn start(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.set(flag_bits::ON_BACK, true));
        state.cooldown = 0;
    }

    fn stop(&self, state: &mut Sitting, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.set(flag_bits::ON_BACK, false));
        state.cooldown = ai.tick_count() + 200;
    }
}

/// `PandaSneezeGoal`: a cub free to act starts a sneeze now and then, a weak one more
/// often.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaSneezeGoal;

impl Goal for PandaSneezeGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        if !animal::is_baby(&ai.tick.mob) || !own.can_perform_action() {
            return false;
        }
        if own.variant() == gene::WEAK && ai.tick.draws.next_int(250) == 1 {
            return true;
        }
        ai.tick.draws.next_int(3000) == 1
    }

    fn can_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.sneeze(true));
    }
}

/// `PandaLookAtPlayerGoal(panda, Player, 6)`: a look at the nearest player within six,
/// or at the one its breeding's disappointment set, while it may act.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaLookAtPlayerGoal;

impl Goal for PandaLookAtPlayerGoal {
    type State = LookingAt;

    fn flags(&self) -> u8 {
        flags::LOOK
    }

    fn can_use(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.draws.next_float() >= 0.02 {
            return false;
        }
        let mut look_at = state_of(&ai.tick.mob).look_at;
        if look_at.is_none() {
            let conditions = Conditions::non_combat().range(6.0);
            let [x, _, z] = ai.position();
            let eyes = [x, ai.eye_y(), z];
            let me = ai.tick.key;
            let not_riding = move |looked: &Looked| looked.vehicle != Some(me);
            look_at =
                sensing::nearest_player(ai, &conditions, &not_riding, eyes).map(|found| found.key);
            update(&mut ai.tick.mob, |own| own.look_at = look_at);
        }
        state.look_at = look_at;
        state_of(&ai.tick.mob).can_perform_action() && look_at.is_some()
    }

    fn can_continue(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        let Some(looked) = state_of(&ai.tick.mob)
            .look_at
            .and_then(|key| sensing::find(ai, key))
        else {
            return false;
        };
        looked.alive && looked.distance_sqr(ai.position()) <= 36.0 && state.look_time > 0
    }

    fn start(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        let draw = ai.tick.draws.next_int(40) as i32;
        state.look_time = self.adjusted_tick_delay(40 + draw);
    }

    fn stop(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        state.look_at = None;
        update(&mut ai.tick.mob, |own| own.look_at = None);
    }

    fn tick(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        let Some(looked) = state_of(&ai.tick.mob)
            .look_at
            .and_then(|key| sensing::find(ai, key))
        else {
            return;
        };
        if looked.alive {
            controls::set_look_at_default(
                ai,
                [looked.position[0], looked.eye_y, looked.position[2]],
            );
            state.look_time -= 1;
        }
    }
}

/// `PandaRollGoal`: a cub or a playful panda on the ground and free to act rolls over a
/// ledge ahead, or now and then.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaRollGoal;

impl Goal for PandaRollGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK | flags::JUMP
    }

    fn interruptable(&self, _: &()) -> bool {
        false
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        let playful = own.variant() == gene::PLAYFUL;
        if !(animal::is_baby(&ai.tick.mob) || playful) || !ai.on_ground() {
            return false;
        }
        if !own.can_perform_action() {
            return false;
        }
        let f = ai.tick.yaw * 0.017_453_292;
        let sx = -table_sin(f);
        let cz = table_cos(f);
        let ix = if sx.abs() > 0.5 {
            sx.signum() as i32
        } else {
            0
        };
        let iz = if cz.abs() > 0.5 {
            cz.signum() as i32
        } else {
            0
        };
        let at = ai.block_position();
        let ahead = BlockPos::new(at.x + ix, at.y - 1, at.z + iz);
        if ai.tick.world.state(ahead).props().is_air() {
            return true;
        }
        if playful && ai.tick.draws.next_int(30) == 1 {
            return true;
        }
        ai.tick.draws.next_int(250) == 1
    }

    fn can_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.set(flag_bits::ROLLING, true));
    }
}

/// `PandaHurtByTargetGoal`: who hurt it, given up once it has bitten (if it is not
/// aggressive) or been given bamboo.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaHurtByTargetGoal;

impl PandaHurtByTargetGoal {
    const HURT_BY: HurtByTargetGoal = HurtByTargetGoal { ignore: &[] };
}

impl Goal for PandaHurtByTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        Self::HURT_BY.flags()
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::HURT_BY.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        if own.got_bamboo || own.did_bite {
            sensing::set_target(ai, None);
            return false;
        }
        Self::HURT_BY.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::HURT_BY.stop(state, ai);
    }
}

/// `Mob.aiStep`'s pick-up for a panda: an empty-handed panda takes up a whole stack of
/// what it eats from the ground, for sure to drop.
fn pick_up(tick: &mut MobTick<'_>) {
    if !tick.mob.body.flags.picks_up_loot || tick.mob.body.vitals.dying() {
        return;
    }
    let bounds = crate::physics::aabb::Aabb::of_entity(
        tick.living.body.position,
        tick.living.body.width,
        tick.living.body.height,
    )
    .inflate([1.0, 0.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack)),
            _ => None,
        })
        .collect();
    let hand = usize::from(slots::MAIN_HAND);
    for (item, stack) in found {
        if holds_something(&tick.mob) || !EATS_FROM_GROUND.test(Some(stack)) {
            continue;
        }
        tick.mob.body.equipment.slots[hand] = Some(stack);
        let mut chances = tick.mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
        chances[hand] = 2.0;
        tick.mob.body.equipment.drop_chances = Some(chances);
        tick.out.local.push(Local::TakeItem {
            item,
            count: stack.count,
        });
    }
}

/// `Mob.lookAt(entity, 90, 90)` of the mob in a tick: its yaw and pitch turned at once
/// towards the entity's eyes.
fn look_at_now(tick: &mut MobTick<'_>, target: &Looked) {
    let [x, y, z] = tick.living.body.position;
    let dx = target.position[0] - x;
    let dz = target.position[2] - z;
    let dy = target.eye_y - (y + f64::from(tick.living.eye_height));
    let horizontal = (dx * dx + dz * dz).sqrt();
    let yaw = crate::mobs::ai::maths::yaw_towards(dx, dz);
    let pitch = crate::mobs::ai::maths::pitch_towards(dy, horizontal);
    tick.pitch = rotate_towards(tick.pitch, pitch, 90.0);
    tick.yaw = rotate_towards(tick.yaw, yaw, 90.0);
}

/// `addEatingParticles()`, every fifth tick of eating: its eating's sound, then (holding
/// something) six particles of six draws each, drawn and not sent.
fn eating_particles(tick: &mut MobTick<'_>, own: &PandaState) {
    if own.eat % 5 != 0 {
        return;
    }
    let volume = 0.5 + 0.5 * tick.draws.next_int(2) as f32;
    let pitch = (tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0;
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_PANDA_EAT,
        volume,
        pitch,
    });
    if holds_something(&tick.mob) {
        for _ in 0..36 {
            tick.draws.next_float();
        }
    }
}

/// `handleEating()`: a sitting, unscared panda holding food starts eating one time in
/// 80; past 80 ticks of it, one time in 20 it stops, having eaten what it held past 100.
fn handle_eating(tick: &mut MobTick<'_>, own: &mut PandaState) {
    let holding = holds_something(&tick.mob);
    if !own.is_eating()
        && own.has(flag_bits::SITTING)
        && !own.is_scared()
        && holding
        && tick.draws.next_int(80) == 1
    {
        own.eat(true);
    } else if !holding || !own.has(flag_bits::SITTING) {
        own.eat(false);
    }
    if !own.is_eating() {
        return;
    }
    eating_particles(tick, own);
    if own.eat > 80 && tick.draws.next_int(20) == 1 {
        let hand = usize::from(slots::MAIN_HAND);
        if own.eat > 100 && EATS_FROM_GROUND.test(tick.mob.body.equipment.slots[hand]) {
            tick.mob.body.equipment.slots[hand] = None;
            tick.out.events.push(Event::GameEvent(GameEvent::Eat));
            own.set(flag_bits::SITTING, false);
        }
        own.eat(false);
        return;
    }
    own.eat += 1;
}

/// `handleRoll()`: 32 ticks of rolling, pushed forward at the first and bounced up at
/// the seventh, fifteenth and twenty-third.
fn handle_roll(tick: &mut MobTick<'_>, own: &mut PandaState) {
    own.roll_counter += 1;
    if own.roll_counter > 32 {
        own.set(flag_bits::ROLLING, false);
        return;
    }
    let v = tick.living.body.motion;
    if own.roll_counter == 1 {
        let f = tick.yaw * 0.017_453_292;
        let s = if animal::is_baby(&tick.mob) { 0.1 } else { 0.2 };
        own.roll_delta = [
            v[0] + f64::from(-table_sin(f) * s),
            0.0,
            v[2] + f64::from(table_cos(f) * s),
        ];
        tick.living.body.motion = physics::add(own.roll_delta, [0.0, 0.27, 0.0]);
    } else if matches!(own.roll_counter, 7 | 15 | 23) {
        let y = if tick.living.body.on_ground {
            0.27
        } else {
            v[1]
        };
        tick.living.body.motion = [0.0, y, 0.0];
    } else {
        tick.living.body.motion = [own.roll_delta[0], v[1], own.roll_delta[2]];
    }
    tick.out.needs_sync = true;
}

/// `afterSneeze()`: its sneeze's sound; the grown pandas near on the ground and free to
/// act jump; one time in 700 a slime ball (`gameplay/panda_sneeze`).
fn after_sneeze(tick: &mut MobTick<'_>) {
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_PANDA_SNEEZE,
        volume: 1.0,
        pitch: 1.0,
    });
    let bounds = crate::physics::aabb::Aabb::of_entity(
        tick.living.body.position,
        tick.living.body.width,
        tick.living.body.height,
    )
    .inflate([10.0, 10.0, 10.0]);
    let pandas: Vec<(EntityId, f64)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied && i32::from(seen.entity.kind) == entity_types::PANDA)
        .filter_map(|seen| {
            let EntityData::Mob { state, .. } = &seen.entity.data else {
                return None;
            };
            let (shared, own) = animal::read::<PandaState>(state);
            let water = has_water(tick.world.state(physics::containing(seen.entity.position)));
            (!shared.is_baby() && seen.entity.on_ground && !water && own.can_perform_action())
                .then_some((seen.entity.key, seen.entity.motion[1]))
        })
        .collect();
    for (key, y) in pandas {
        // `jumpFromGround()`: up at a jump's power (`0.42f`, ordinary ground).
        let up = f64::from(0.42_f32).max(y);
        tick.out.local.push(Local::Push {
            key,
            by: [0.0, up - y, 0.0],
        });
    }
    if tick.draws.next_int(700) == 0 {
        tick.out.local.push(Local::Drop {
            stack: ItemStack {
                item: items::SLIME_BALL,
                count: 1,
            },
            at: tick.living.body.position,
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
        });
    }
}

impl MobType for Panda {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(2, PandaPanicGoal);
        setup.add_goal(2, PandaBreedGoal);
        setup.add_goal(3, PandaAttackGoal);
        setup.add_goal(4, TemptGoal::new(1.0, FOOD, false));
        for (class, distance) in [(Class::Player, 8.0), (Class::Monster, 4.0)] {
            setup.add_goal(
                6,
                PandaAvoidGoal {
                    avoid: AvoidEntityGoal {
                        class,
                        max_distance: distance,
                        walk_speed: 2.0,
                        sprint_speed: 2.0,
                    },
                },
            );
        }
        setup.add_goal(7, PandaSitGoal);
        setup.add_goal(8, PandaLieOnBackGoal);
        setup.add_goal(8, PandaSneezeGoal);
        setup.add_goal(9, PandaLookAtPlayerGoal);
        setup.add_goal(10, RandomLookAroundGoal);
        setup.add_goal(12, PandaRollGoal);
        setup.add_goal(13, animal::FollowParentGoal { speed: 1.25 });
        setup.add_goal(14, RandomStrollGoal::avoiding_water(1.0));
        setup.add_target(1, PandaHurtByTargetGoal);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK)
    }

    /// The constructor: it picks up loot (no panda is a cub as it is made).
    fn construct(&self, mob: &mut Mob, _: &mut crate::region::Random) {
        mob.body.flags.picks_up_loot = true;
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `PandaMoveControl`: it moves only while it may act.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        if state_of(&ai.tick.mob).can_perform_action() {
            controls::tick_move(ai);
        }
    }

    /// `Mob.aiStep`'s pick-up and `Animal.aiStep`, then `Panda.tick`'s own after the
    /// living tick: a worried panda stands when it does not eat; without a target it
    /// forgets bamboo and bites; its unhappiness, sneeze and roll run on; sitting it looks
    /// level; it eats. A weak panda's health is brought within its most at the end of
    /// its first tick (finding F12).
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        animal::after_ai_step(tick);
        let mut own = state_of(&tick.mob);
        if own.variant() == gene::WORRIED && !own.is_eating() {
            own.set(flag_bits::SITTING, false);
        }
        let target = tick
            .mob
            .body
            .combat
            .target
            .and_then(|key| super::tamable::find(tick.world, key))
            .filter(sensing::valid_target);
        if target.is_none() {
            own.got_bamboo = false;
            own.did_bite = false;
        }
        if own.unhappy > 0 {
            if let Some(target) = &target {
                look_at_now(tick, target);
            }
            if own.unhappy == 29 || own.unhappy == 14 {
                tick.out.events.push(Event::Sound {
                    sound: sounds::ENTITY_PANDA_CANT_BREED,
                    volume: 1.0,
                    pitch: 1.0,
                });
            }
            own.unhappy -= 1;
        }
        if own.has(flag_bits::SNEEZING) {
            own.sneeze += 1;
            if own.sneeze > 20 {
                own.sneeze(false);
                after_sneeze(tick);
            } else if own.sneeze == 1 {
                tick.out.events.push(Event::Sound {
                    sound: sounds::ENTITY_PANDA_PRE_SNEEZE,
                    volume: 1.0,
                    pitch: 1.0,
                });
            }
        }
        if own.has(flag_bits::ROLLING) {
            handle_roll(tick, &mut own);
        } else {
            own.roll_counter = 0;
        }
        if own.has(flag_bits::SITTING) {
            tick.pitch = 0.0;
        }
        handle_eating(tick, &mut own);
        put_state(&mut tick.mob, &own);
        // `refreshDirtyAttributes`: health above its most is cut down to it.
        let most = tick
            .mob
            .body
            .attributes
            .value(tick.kind, attribute(clustine_data::attributes::MAX_HEALTH))
            as f32;
        if tick.mob.body.vitals.health > most {
            tick.mob.body.vitals.health = most;
        }
    }

    /// `Panda.hurtServer`: it stands up first (named: the game's does so even for a hurt
    /// it is spared, which no panda's ever is but by its type).
    fn before_hurt(&self, mob: &mut Mob) {
        update(mob, |own| own.set(flag_bits::SITTING, false));
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let mut own = state_of(used.mob);
        if own.is_scared() {
            return Interaction::Pass;
        }
        if own.has(flag_bits::ON_BACK) {
            own.set(flag_bits::ON_BACK, false);
            put_state(used.mob, &own);
            return Interaction::Success;
        }
        if FOOD.test(used.held) {
            if used.mob.body.combat.target.is_some() {
                own.got_bamboo = true;
            }
            let now = used.moment;
            let mut shared = animal::animal_of(&used.mob.state);
            if shared.can_age_up() {
                let age = shared.age(now);
                shared.age_up(animal::speed_up_seconds(age.wrapping_neg()), true, now);
                used.mob.state = animal::write(&shared, &own);
                return Interaction::SuccessServer;
            }
            if shared.is_baby() {
                put_state(used.mob, &own);
                return Interaction::Pass;
            }
            if shared.age(now) == 0 && shared.love(now) <= 0 {
                shared.set_in_love(now);
                used.mob.state = animal::write(&shared, &own);
                used.did.push(Did::Event(18));
                return Interaction::SuccessServer;
            }
            // A use reads no blocks: the panda is taken as out of water.
            if !own.has(flag_bits::SITTING) {
                // `tryToSit()` and `eat(true)`.
                used.mob.body.inputs.zza = 0.0;
                own.set(flag_bits::SITTING, true);
                own.eat(true);
                let hand = usize::from(slots::MAIN_HAND);
                if let Some(held) =
                    used.mob.body.equipment.slots[hand].filter(|held| held.count > 0)
                    && !used.creative
                {
                    used.did.push(Did::Drop {
                        stack: held,
                        at: used.position,
                        pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        push: None,
                    });
                }
                let item = used.held.map_or(items::BAMBOO, |held| held.item);
                used.mob.body.equipment.slots[hand] = Some(ItemStack { item, count: 1 });
                put_state(used.mob, &own);
                used.mob.body.path.followed = None;
                return Interaction::SuccessServer;
            }
            put_state(used.mob, &own);
            return Interaction::Pass;
        }
        if animal::is_baby(used.mob) {
            return animal::mob_interact(used, Items::Of(&[]), None);
        }
        Interaction::Pass
    }

    /// `finalizeSpawn`: two genes of the level's draws, and the weak and lazy genes'
    /// attributes, then a group whose cubs come one in five (`AgeableMobGroupData(0.2f)`)
    /// where it has none. An egg's panda is the first of its group: never a cub.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let main = random_gene(spawn.level);
        let hidden = random_gene(spawn.level);
        update(spawn.mob, |own| {
            own.main_gene = main;
            own.hidden_gene = hidden;
        });
        set_attributes(spawn.mob, entity_types::PANDA as u16);
        let group = super::SpawnGroup::Ageable(super::AgeableGroup::with_chance(0.2));
        animal::finalize_ageable(spawn, group);
    }

    /// `getBreedOffspring`: a cub of a gene from each parent (each parent's own draw of
    /// which), the order drawn by the cub, each gene one time in 32 a new one, and its
    /// genes' attributes.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this = state_of(breeding.this);
        let partner = state_of(breeding.partner);
        let draws = &mut *breeding.draws;
        let one_of = |parent: &PandaState, draws: &mut crate::region::Random| {
            if draws.next_bool() {
                parent.main_gene
            } else {
                parent.hidden_gene
            }
        };
        let (mut main, mut hidden) = if draws.next_bool() {
            let main = one_of(&this, draws);
            (main, one_of(&partner, draws))
        } else {
            let main = one_of(&partner, draws);
            (main, one_of(&this, draws))
        };
        if draws.next_int(32) == 0 {
            main = random_gene(draws);
        }
        if draws.next_int(32) == 0 {
            hidden = random_gene(draws);
        }
        let mut child = Mob::fresh();
        let own = PandaState {
            main_gene: main,
            hidden_gene: hidden,
            ..PandaState::default()
        };
        child.state = animal::write(&animal::Animal::default(), &own);
        set_attributes(&mut child, entity_types::PANDA as u16);
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    /// `canBeLeashed()`: no panda is.
    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(match state_of(mob).variant() {
            gene::AGGRESSIVE => sounds::ENTITY_PANDA_AGGRESSIVE_AMBIENT,
            gene::WORRIED => sounds::ENTITY_PANDA_WORRIED_AMBIENT,
            _ => sounds::ENTITY_PANDA_AMBIENT,
        })
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PANDA_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PANDA_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((UNHAPPY_INDEX, DataShown::Int(own.unhappy), own.unhappy == 0));
        values.push((SNEEZE_INDEX, DataShown::Int(own.sneeze), own.sneeze == 0));
        values.push((EAT_INDEX, DataShown::Int(own.eat), own.eat == 0));
        values.push((
            MAIN_GENE_INDEX,
            DataShown::Byte(own.main_gene),
            own.main_gene == gene::NORMAL,
        ));
        values.push((
            HIDDEN_GENE_INDEX,
            DataShown::Byte(own.hidden_gene),
            own.hidden_gene == gene::NORMAL,
        ));
        values.push((FLAGS_INDEX, DataShown::Byte(own.flags), own.flags == 0));
        values
    }
}

/// `PandaPanicGoal(panda, 2.0, #panic_environmental_causes)`: what it panics at is not
/// seen, so it never starts; it is registered so that the goals after it keep their
/// places.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PandaPanicGoal;

impl Goal for PandaPanicGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}
