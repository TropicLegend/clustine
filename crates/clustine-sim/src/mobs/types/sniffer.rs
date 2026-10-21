//! The sniffer (`Sniffer`, `Sniffer$State`; `docs/groundwork/n7-brief.md`, section 7.9):
//! a big, slow animal whose whole life is a state machine in its synced data (idle,
//! happy, scenting, sniffing, searching, digging, rising), which its brain's seven
//! behaviours drive. Digging, it drops a torchflower seed or a pitcher pod 120 ticks in;
//! bred with torchflower seeds it lays a sniffer egg (an item, never a baby), whose block
//! (`region/sniffer_eggs.rs`) hatches a snifflet that grows for 48000 ticks.
//!
//! Everything here but the brain's activities is built (`docs/groundwork/n7j-plan.md`,
//! section 4.2): the body, the state and its transitions with their sounds and the
//! entity event, the seed's tick, the dig's game event, the box made lower while it digs,
//! feeding, mating's rule, the egg its breeding lays, the quad leash and its sounds. Until
//! the brain's provider comes, a sniffer has no goals and no brain, and so stays idle,
//! walking nowhere; [`brain_seams`] are what its behaviours will call.
//!
//! Named differences: the seed's draw comes from the sniffer's own stream, not from the
//! world's random sequence of `gameplay/sniffer_digging` (which is not extracted yet);
//! the water's malus is not lifted while it is in water or burning
//! (`onPathfindingStart`), which only its brain's paths read.

use clustine_blocks::data::in_tag;
use clustine_data::{PathType, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, Laid, Mate, MobType, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::maths::{self, table_cos, table_sin};
use crate::mobs::ai::{Ai, HeadLimits, Setup, navigation, random_pos};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};
use crate::region::{DEFAULT_PICK_UP_DELAY, Random};
use crate::shown::{DataShown, Serialiser};

/// `#sniffer_food`: torchflower seeds.
pub(crate) const FOOD: Items = Items::Tag("minecraft:sniffer_food");

/// The synced data's indices: the state (`SNIFFER_STATE`) and the seed's tick.
pub(crate) const STATE_INDEX: u8 = 18;
pub(crate) const DROP_SEED_INDEX: u8 = 19;

/// `SNIFFER_BABY_START_AGE`: a snifflet grows for twice an animal's time.
pub(crate) const BABY_START_AGE: i32 = -48_000;

/// `DIGGING_DROP_SEED_OFFSET_TICKS`.
pub(crate) const DROP_SEED_AFTER: i32 = 120;

/// `DIGGING_BB_HEIGHT_OFFSET` and the digging box's eye height.
const DIGGING_LOWER_BY: f32 = 0.4;
const DIGGING_EYE_HEIGHT: f32 = 0.81;

/// How far ahead of its feet its nose is (`getHeadPosition`).
const HEAD_REACH: f64 = 2.25;

/// How many explored positions it keeps beyond the newest (`storeExploredPosition`).
const EXPLORED_KEPT: usize = 20;

/// `Sniffer$State`, by its ordinal, which is its id on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum State {
    #[default]
    Idling,
    FeelingHappy,
    Scenting,
    Sniffing,
    Searching,
    Digging,
    Rising,
}

impl State {
    /// Its id (`Sniffer$State.id`, its ordinal).
    pub(crate) fn id(self) -> i32 {
        self as i32
    }
}

/// What a sniffer keeps beyond an animal: its state (`DATA_STATE`) and the tick its seed
/// drops at (`DATA_DROP_SEED_AT_TICK`, in the game's `tickCount`). Neither is saved by the
/// game, which loads a sniffer idle with no seed tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SnifferState {
    pub(crate) state: State,
    pub(crate) drop_seed_at: i32,
}

impl Own for SnifferState {}

/// A sniffer's own state.
pub(crate) fn read(mob: &Mob) -> SnifferState {
    animal::read::<SnifferState>(&mob.state).1
}

/// Changes a sniffer's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut SnifferState)) {
    let (shared, mut own) = animal::read::<SnifferState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// The game's `tickCount` within the mob's tick, counted at its start.
fn game_tick_count(tick: &MobTick<'_>) -> i32 {
    tick.tick_count.wrapping_add(1) as i32
}

/// `transitionTo(state)`: the state set, with what each brings: the happy sound, the
/// scenting sound (higher for a baby), the sniffing sound, the seed's tick and entity
/// event 63 as digging starts, the sound of digging's end as it rises.
#[allow(
    dead_code,
    reason = "what the sniffer's brain's behaviours call once its provider is wired"
)]
pub(crate) fn transition_to(tick: &mut MobTick<'_>, to: State) {
    let sound = |sound: i32, pitch: f32| Event::Sound {
        sound,
        volume: 1.0,
        pitch,
    };
    match to {
        State::FeelingHappy => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_HAPPY, 1.0)),
        State::Sniffing => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_SNIFFING, 1.0)),
        State::Rising => tick
            .out
            .events
            .push(sound(sounds::ENTITY_SNIFFER_DIGGING_STOP, 1.0)),
        _ => {}
    }
    update(&mut tick.mob, |own| own.state = to);
    match to {
        State::Scenting => {
            let pitch = if animal::is_baby(&tick.mob) { 1.3 } else { 1.0 };
            tick.out
                .events
                .push(sound(sounds::ENTITY_SNIFFER_SCENTING, pitch));
        }
        State::Digging => {
            let at = game_tick_count(tick).wrapping_add(DROP_SEED_AFTER);
            update(&mut tick.mob, |own| own.drop_seed_at = at);
            tick.out.events.push(Event::Entity(63));
        }
        _ => {}
    }
}

/// `getHeadPosition()`: its feet moved 2.25 along its look (`getForward`, the game's
/// table of sines).
pub(crate) fn head_position(position: Vec3, yaw: f32, pitch: f32) -> Vec3 {
    let f = pitch * maths::RADIANS;
    let g = -yaw * maths::RADIANS;
    let (h, i) = (table_cos(g), table_sin(g));
    let (j, k) = (table_cos(f), table_sin(f));
    let forward = [f64::from(i * j), f64::from(-k), f64::from(h * j)];
    physics::add(position, physics::scale(forward, HEAD_REACH))
}

/// `getHeadBlock()`: the block of its nose, at its feet's height and a fifth.
pub(crate) fn head_block(position: Vec3, yaw: f32, pitch: f32) -> BlockPos {
    let head = head_position(position, yaw, pitch);
    physics::containing([head[0], position[1] + 0.2, head[2]])
}

/// `#sniffer_diggable_block`: dirt, mud, moss, grass and podzol.
pub(crate) fn diggable(state: clustine_data::BlockState) -> bool {
    in_tag(state, "minecraft:sniffer_diggable_block")
}

/// `storeExploredPosition(pos)`: the newest first, the twenty before it kept.
pub(crate) fn store_explored(explored: &[BlockPos], pos: BlockPos) -> Vec<BlockPos> {
    let mut kept = Vec::with_capacity(EXPLORED_KEPT + 1);
    kept.push(pos);
    kept.extend(explored.iter().take(EXPLORED_KEPT).copied());
    kept
}

/// The seams the brain's behaviours call (`SnifferAi`'s seven, B 7.9.3 and 7.9.4), which
/// part A's machinery wires to its provider: the predicates the behaviours read and the
/// search for a place to dig. Its memories (`sniffer_explored_positions`, the digging and
/// sniffing marks, `sniff_cooldown`) are the brain's; these take what they read of them.
#[allow(
    dead_code,
    reason = "what the sniffer's brain calls once the machinery's provider is wired"
)]
pub(crate) mod brain_seams {
    use super::*;

    /// What the brain's memories say of the sniffer, as its predicates read them.
    #[derive(Debug, Clone, Copy, Default)]
    pub(crate) struct Marks {
        /// `TEMPTING_PLAYER` present (`isTempted`).
        pub(crate) tempted: bool,
        /// `IS_PANICKING` present (`isPanicking`).
        pub(crate) panicking: bool,
    }

    /// `canSniff()`: not tempted, not panicking, not in water, not in love, on the
    /// ground, riding nothing and on no lead.
    pub(crate) fn can_sniff(ai: &Ai<'_, '_>, marks: Marks) -> bool {
        let now = ai.tick.lived;
        !marks.tempted
            && !marks.panicking
            && !ai.in_water()
            && !animal::animal_of(&ai.tick.mob.state).in_love(now)
            && ai.on_ground()
            && ai.tick.vehicle.is_none()
            && !ai.tick.leashed
    }

    /// `canDig(pos)`: a diggable block not among the explored, to which a path of reach
    /// one reaches (one path search each time, as the game's).
    pub(crate) fn can_dig_at(ai: &mut Ai<'_, '_>, pos: BlockPos, explored: &[BlockPos]) -> bool {
        if !diggable(ai.tick.world.state(pos)) || explored.contains(&pos) {
            return false;
        }
        navigation::create_path(ai, pos, 1)
            .is_some_and(|made| made.path(ai).is_some_and(|path| path.reached))
    }

    /// `canDig()`: not panicking, not tempted, grown, not in water, on the ground,
    /// riding nothing, and the block below its nose diggable and reached.
    pub(crate) fn can_dig(ai: &mut Ai<'_, '_>, marks: Marks, explored: &[BlockPos]) -> bool {
        if marks.panicking
            || marks.tempted
            || animal::is_baby(&ai.tick.mob)
            || ai.in_water()
            || !ai.on_ground()
            || ai.tick.vehicle.is_some()
        {
            return false;
        }
        let head = head_block(ai.position(), ai.tick.yaw, ai.tick.pitch);
        can_dig_at(ai, head.offset(0, -1, 0), explored)
    }

    /// `calculateDigPosition()`: up to five random places on land, 10, 12, 14, 16 and 18
    /// across and 3 up or down, the first whose ground it can dig (within the world's
    /// border, which is not kept: every place is within it).
    pub(crate) fn calculate_dig_position(
        ai: &mut Ai<'_, '_>,
        explored: &[BlockPos],
    ) -> Option<BlockPos> {
        for i in 0..5 {
            let Some(found) = random_pos::land_pos(ai, 10 + 2 * i, 3) else {
                continue;
            };
            let ground = physics::containing(found).offset(0, -1, 0);
            if can_dig_at(ai, ground, explored) {
                return Some(ground);
            }
        }
        None
    }

    /// `onDiggingComplete(timedOut)`: a dig run its whole length explores the block it
    /// stood on (`getOnPos`), as the explored list's newest.
    pub(crate) fn on_digging_complete(
        explored: &[BlockPos],
        on_pos: BlockPos,
        timed_out: bool,
    ) -> Option<Vec<BlockPos>> {
        timed_out.then(|| store_explored(explored, on_pos))
    }

    /// `canMate`'s state test of one sniffer: idle, scenting or happy.
    pub(crate) fn may_mate_in(state: State) -> bool {
        matches!(state, State::Idling | State::Scenting | State::FeelingHappy)
    }
}

/// `dropSeed()`: at its tick, a seed (a torchflower's or a pitcher pod, one draw of two)
/// at the corner of its nose's block, with its sound.
fn drop_seed(tick: &mut MobTick<'_>) {
    let own = read(&tick.mob);
    if own.drop_seed_at != game_tick_count(tick) {
        return;
    }
    let head = head_block(tick.living.body.position, tick.yaw, tick.pitch);
    tick.out.local.push(Local::Drop {
        stack: seed(tick.draws),
        at: [f64::from(head.x), f64::from(head.y), f64::from(head.z)],
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
    });
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_SNIFFER_DROP_SEED,
        volume: 1.0,
        pitch: 1.0,
    });
}

/// `gameplay/sniffer_digging`: one roll of two entries of one weight each.
pub(crate) fn seed(draws: &mut Random) -> ItemStack {
    let item = if draws.next_int(2) == 0 {
        items::TORCHFLOWER_SEEDS
    } else {
        items::PITCHER_POD
    };
    ItemStack { item, count: 1 }
}

/// `emitDiggingParticles`' part the server's world hears: every tenth tick while it
/// digs, `ENTITY_ACTION` at its nose's block, in the window of the particles or not (the
/// particles and the hit sounds are the client's).
fn dig_game_event(tick: &mut MobTick<'_>) {
    if game_tick_count(tick) % 10 != 0 {
        return;
    }
    let head = head_block(tick.living.body.position, tick.yaw, tick.pitch);
    tick.out.local.push(Local::GameEvent {
        name: "minecraft:entity_action",
        pos: head,
    });
}

/// The sniffer.
pub(crate) struct Sniffer;

impl MobType for Sniffer {
    /// The constructor's navigation and maluses (and `Animal`'s): no goals, as its mind is
    /// its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.config.navigation.set_can_float(true);
        setup.set_malus(PathType::Water, -1.0);
        setup.set_malus(PathType::OnTopOfPowderSnow, -1.0);
        setup.set_malus(PathType::DamageCautious, -1.0);
    }

    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 50,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    /// `Sniffer.customServerAiStep`: the brain's tick and its activity's update (part A's,
    /// not wired yet), then the animal's.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `Sniffer.tick()` before the animal's: digging, its game event and its seed.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        if read(&tick.mob).state == State::Digging {
            dig_game_event(tick);
            drop_seed(tick);
        }
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Sniffer.jumpFromGround()` after the living one: a sniffer stuck at a step while
    /// it means to move is pushed a tenth ahead.
    fn jumped_from_ground(&self, tick: &mut MobTick<'_>) {
        let motion = tick.living.body.motion;
        if tick.mob.body.controls.movement.speed > 0.0
            && motion[0] * motion[0] + motion[2] * motion[2] < 0.01
        {
            let push = crate::physics::minecart::input_vector([0.0, 0.0, 1.0], 0.1, tick.yaw);
            tick.living.body.motion = physics::add(motion, push);
        }
    }

    /// `Sniffer.mobInteract`: the animal's feeding with torchflower seeds, after which
    /// it eats once more (`playEatingSound` is called by `Animal.mobInteract` and again
    /// by the sniffer's own: two sounds, each a draw of the level's random).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let food = FOOD.test(used.held);
        let result = animal::mob_interact(used, FOOD, None);
        if food && result != Interaction::Pass {
            for _ in 0..2 {
                // `Mth.randomBetween(random, 0.8, 1.2)`.
                let pitch = used.draws.next_float() * (1.2 - 0.8) + 0.8;
                used.did.push(Did::Sound {
                    sound: sounds::ENTITY_SNIFFER_EAT,
                    source: SOUND_SOURCE_NEUTRAL,
                    at: used.position,
                    volume: 1.0,
                    pitch,
                    but_the_player: false,
                });
            }
        }
        result
    }

    /// `getBreedOffspring`: a sniffer (what an egg used on one makes), a baby of its
    /// own start age.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(hatchling())
    }

    /// `spawnChildFromBreeding`: an egg laid at its feet, and no child.
    fn laid_from_breeding(&self) -> Option<Laid> {
        Some(Laid {
            stack: ItemStack {
                item: items::SNIFFER_EGG,
                count: 1,
            },
            sound: sounds::BLOCK_SNIFFER_EGG_PLOP,
            pitch_base: 0.5,
        })
    }

    /// `Sniffer.canMate`: both sniffers, each idle, scenting or happy.
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        this.entity.kind == partner.entity.kind
            && brain_seams::may_mate_in(read(this.mob).state)
            && brain_seams::may_mate_in(read(partner.mob).state)
    }

    /// `Sniffer.die`: idle again before it dies.
    fn before_die(&self, mob: &mut Mob) {
        update(mob, |own| own.state = State::Idling);
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `getDefaultDimensions`: lower by 0.4 while it digs (scaled as a baby is), else
    /// the animal's.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = clustine_data::EntityTypeInfo::of(clustine_data::entity_types::SNIFFER);
        let scale = if animal::is_baby(mob) { 0.5 } else { 1.0 };
        if read(mob).state == State::Digging {
            return Some(Dimensions {
                width: info.width * scale,
                height: (info.height - DIGGING_LOWER_BY) * scale,
                eye_height: DIGGING_EYE_HEIGHT * scale,
            });
        }
        animal::dimensions(
            mob,
            super::water::scaled(info.width, info.height, info.eye_height, 0.5),
        )
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `isFood` and `supportQuadLeash`: a lead holds it by four.
    fn quad_leash(&self, _: u16) -> bool {
        true
    }

    /// `getAmbientSound`: none while it digs or searches.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        match read(mob).state {
            State::Digging | State::Searching => None,
            _ => Some(sounds::ENTITY_SNIFFER_IDLE),
        }
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_STEP)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNIFFER_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        values.push((
            STATE_INDEX,
            DataShown::Serialised(Serialiser::SnifferState, own.state.id()),
            own.state == State::Idling,
        ));
        values.push((
            DROP_SEED_INDEX,
            DataShown::Int(own.drop_seed_at),
            own.drop_seed_at == 0,
        ));
        values
    }
}

/// A snifflet hatched from an egg (`SnifferEggBlock`'s hatching): `EntityType.create(level,
/// BREEDING)` with no `finalizeSpawn`, `setBaby(true)` of a sniffer's own start age.
pub(crate) fn hatchling() -> Mob {
    let mut mob = Mob::fresh();
    animal::update(&mut mob, |shared| shared.set_age(BABY_START_AGE, 0));
    mob
}
