//! The spellcasters' goals: the evoker's casting, summoning, fangs and wololo
//! (`SpellcasterIllager$SpellcasterCastingSpellGoal`, `$SpellcasterUseSpellGoal`,
//! `Evoker$EvokerCastingSpellGoal`, `$EvokerSummonSpellGoal`, `$EvokerAttackSpellGoal`,
//! `$EvokerWololoSpellGoal`, n7; `docs/groundwork/n9-plan.md`, section 8.4): part S8's.
//!
//! A spell's goal (`UseSpellGoal`, taking no flag) starts at its interval: at a living
//! target for the summoning and the fangs, with no target for the wololo. Its start
//! sets the evoker casting for the spell's casting time (`spellCastingTickCount`, which
//! `customServerAiStep` counts down every tick), shows the spell (data 17), plays its
//! preparing sound and sets when it may start again; its warmup, `adjustedTickDelay` of
//! 20 (40 for the wololo), is counted down in the goal's ticks, which are every other
//! tick, and at nothing the spell is cast with the cast sound. While the evoker casts,
//! the casting goal (MOVE and LOOK) holds it still, watching its target or the sheep, and
//! shows no spell once the casting time ran out.
//!
//! | Spell | Warmup | Casting | Interval | Draws (the evoker's own) |
//! |---|---|---|---|---|
//! | Summon | 20 | 100 | 340 | at an asked tick past the target's tests, `nextInt(8)`; at the cast, three times: `nextInt(5)`, `nextInt(5)`, the vex's `finalizeSpawn` (the sword's `nextFloat`, then `Mob`'s two `nextDouble` and `nextFloat`), `nextInt(90)` |
//! | Fangs | 20 | 40 | 100 | none |
//! | Wololo | 40 | 60 | 140 | at an asked tick past its tests, `nextInt(sheep)` |
//!
//! When a spell may start again is reckoned from the mob's `lived`, where the game's
//! goal reckons from `tickCount`, which a load starts again from nothing: the goal's
//! state is the record's in Clustine (the brief's 0.10), and `lived` goes on with it
//! across a load and a line (named: after a load the game's evoker may cast at once,
//! Clustine's waits out the rest of its interval).
//!
//! Named differences: a vex is made by section 4.1's rule (in a chunk the region does
//! not tick, sent; where it reads nothing, not made, its draws made all the same), as
//! are the fangs; a sheep the wololo turns in a copy keeps its colour; no mob has a
//! team, so a vex joins none.

use clustine_data::{Face, SupportType, entity_types, sounds};
use clustine_world::{BlockPos, Difficulty, EntityData, EntityId};
use serde::{Deserialize, Serialize};

use super::goals::{ai_bounds, look_at_thing};
use super::maths;
use super::pathfinder::shape_top;
use super::selector::{Goal, flags};
use super::sensing::{self, Class, Conditions, Looked};
use super::{Ai, navigation};
use crate::api::TickOutput;
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::tick::Local;
use crate::mobs::types::evoker::EvokerState;
use crate::mobs::types::sheep::SheepState;
use crate::mobs::types::{SpawnReason, Spawning, animal, illagers, vex};
use crate::physics::{self, Vec3};
use crate::region::{Random, Region};

/// `IllagerSpell.NONE`'s id.
pub(crate) const NO_SPELL: u8 = 0;

/// `DyeColor.BLUE` and `DyeColor.RED`, by id.
pub(crate) const BLUE: u8 = 11;
pub(crate) const RED: u8 = 14;

/// The evoker's spells (`IllagerSpell`), each with its goal's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spell {
    /// `SUMMON_VEX` (`EvokerSummonSpellGoal`).
    Summon,
    /// `FANGS` (`EvokerAttackSpellGoal`).
    Fangs,
    /// `WOLOLO` (`EvokerWololoSpellGoal`).
    Wololo,
}

impl Spell {
    /// `IllagerSpell.id`, as data 17 shows it.
    pub(crate) const fn id(self) -> u8 {
        match self {
            Spell::Summon => 1,
            Spell::Fangs => 2,
            Spell::Wololo => 3,
        }
    }

    /// `getCastWarmupTime()`.
    pub(crate) const fn warmup(self) -> i32 {
        match self {
            Spell::Summon | Spell::Fangs => 20,
            Spell::Wololo => 40,
        }
    }

    /// `getCastingTime()`.
    pub(crate) const fn casting(self) -> i32 {
        match self {
            Spell::Summon => 100,
            Spell::Fangs => 40,
            Spell::Wololo => 60,
        }
    }

    /// `getCastingInterval()`.
    pub(crate) const fn interval(self) -> i32 {
        match self {
            Spell::Summon => 340,
            Spell::Fangs => 100,
            Spell::Wololo => 140,
        }
    }

    /// `getSpellPrepareSound()`.
    pub(crate) const fn prepare_sound(self) -> i32 {
        match self {
            Spell::Summon => sounds::ENTITY_EVOKER_PREPARE_SUMMON,
            Spell::Fangs => sounds::ENTITY_EVOKER_PREPARE_ATTACK,
            Spell::Wololo => sounds::ENTITY_EVOKER_PREPARE_WOLOLO,
        }
    }
}

/// The evoker's own state.
fn own(ai: &Ai<'_, '_>) -> EvokerState {
    illagers::own_of::<EvokerState>(&ai.tick.mob)
}

fn update(ai: &mut Ai<'_, '_>, change: impl FnOnce(&mut EvokerState)) {
    illagers::update_own(&mut ai.tick.mob, change);
}

/// `isCastingSpell()` on the server: casting ticks left.
fn casting(ai: &Ai<'_, '_>) -> bool {
    own(ai).spell_ticks > 0
}

/// `playSound(sound, 1, 1)` at the evoker.
fn play(ai: &mut Ai<'_, '_>, sound: i32) {
    ai.tick.out.events.push(Event::Sound {
        sound,
        volume: 1.0,
        pitch: 1.0,
    });
}

/// `Evoker$EvokerCastingSpellGoal` (MOVE and LOOK): while the evoker casts, it stops
/// its path, and looks at its target, else at its wololo's sheep; at its end the spell
/// shown is none.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CastingSpellGoal;

impl Goal for CastingSpellGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    /// `getSpellCastingTime() > 0`, which `canContinueToUse` asks too.
    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        casting(ai)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(ai, |own| own.spell = NO_SPELL);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let head = ai.head();
        let (yaw, pitch) = (head.max_y_rot as f32, head.max_x_rot as f32);
        if let Some(target) = sensing::target(ai) {
            look_at_thing(ai, &target, yaw, pitch);
        } else if let Some(sheep) = own(ai).wololo_target.and_then(|key| sensing::find(ai, key)) {
            look_at_thing(ai, &sheep, yaw, pitch);
        }
    }
}

/// A spell goal's state (`SpellcasterUseSpellGoal`'s fields).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsingSpell {
    /// `attackWarmupDelay`, in the goal's ticks.
    pub(crate) warmup: i32,
    /// `nextAttackTickCount`, as the mob's `lived` (the module's note).
    pub(crate) next: i64,
}

/// `SpellcasterIllager$SpellcasterUseSpellGoal` of one of the evoker's spells (no
/// flags).
#[derive(Debug, Clone, Copy)]
pub(crate) struct UseSpellGoal {
    pub(crate) spell: Spell,
}

impl UseSpellGoal {
    /// `SpellcasterUseSpellGoal.canUse`: a living target, not casting, its interval
    /// over.
    fn parent_can_use(state: &UsingSpell, ai: &Ai<'_, '_>) -> bool {
        if !sensing::target(ai).is_some_and(|target| target.alive) {
            return false;
        }
        !casting(ai) && ai.now() >= state.next
    }

    /// `EvokerSummonSpellGoal.canUse` past its parent's: the vexes within 16 of its box
    /// (`forNonCombat().range(16).ignoreLineOfSight().ignoreInvisibilityTesting()`)
    /// counted, then its own `nextInt(8) + 1 > count`.
    fn summon_can_use(ai: &mut Ai<'_, '_>) -> bool {
        let conditions = Conditions::non_combat()
            .range(16.0)
            .ignore_line_of_sight()
            .ignore_invisibility();
        let bounds = ai_bounds(ai).inflate([16.0, 16.0, 16.0]);
        let count = sensing::nearby(ai, Class::Kinds(&[vex::VEX]), &conditions, bounds).len();
        ai.tick.draws.next_int(8) as usize + 1 > count
    }

    /// `EvokerWololoSpellGoal.canUse`: no target, not casting, its interval over,
    /// `mob_griefing` (on, as everywhere), then the blue sheep of its box inflated by 16
    /// across and 4 up and down that a non-combat test within 16 passes (seen, by its
    /// line of sight), one of them its own `nextInt(size)`.
    fn wololo_can_use(state: &UsingSpell, ai: &mut Ai<'_, '_>) -> bool {
        if sensing::target(ai).is_some() || casting(ai) || ai.now() < state.next {
            return false;
        }
        let bounds = ai_bounds(ai).inflate([16.0, 4.0, 16.0]);
        let candidates: Vec<(Looked, bool)> = ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| i32::from(seen.entity.kind) == entity_types::SHEEP)
            .map(|seen| (Looked::of_seen(seen), colour_of(&seen.entity.data) == BLUE))
            .collect();
        let conditions = Conditions::non_combat().range(16.0);
        let mut sheep = Vec::new();
        for (looked, blue) in candidates {
            if conditions.test(ai, &looked, &|_| blue) {
                sheep.push(looked.key);
            }
        }
        if sheep.is_empty() {
            return false;
        }
        let chosen = sheep[ai.tick.draws.next_int(sheep.len() as u32) as usize];
        update(ai, |own| own.wololo_target = Some(chosen));
        true
    }

    /// `performSpellCasting()`.
    fn perform(self, ai: &mut Ai<'_, '_>) {
        match self.spell {
            Spell::Summon => summon(ai),
            Spell::Fangs => fangs(ai),
            Spell::Wololo => wololo(ai),
        }
    }
}

impl Goal for UseSpellGoal {
    type State = UsingSpell;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut UsingSpell, ai: &mut Ai<'_, '_>) -> bool {
        match self.spell {
            Spell::Wololo => Self::wololo_can_use(state, ai),
            Spell::Summon => Self::parent_can_use(state, ai) && Self::summon_can_use(ai),
            Spell::Fangs => Self::parent_can_use(state, ai),
        }
    }

    fn can_continue(&self, state: &mut UsingSpell, ai: &mut Ai<'_, '_>) -> bool {
        let held = match self.spell {
            Spell::Wololo => own(ai).wololo_target.is_some(),
            Spell::Summon | Spell::Fangs => sensing::target(ai).is_some_and(|target| target.alive),
        };
        held && state.warmup > 0
    }

    fn start(&self, state: &mut UsingSpell, ai: &mut Ai<'_, '_>) {
        state.warmup = self.adjusted_tick_delay(self.spell.warmup());
        state.next = ai.now() + i64::from(self.spell.interval());
        let (casting, id) = (self.spell.casting(), self.spell.id());
        update(ai, |own| own.spell_ticks = casting);
        play(ai, self.spell.prepare_sound());
        update(ai, |own| own.spell = id);
    }

    fn stop(&self, _: &mut UsingSpell, ai: &mut Ai<'_, '_>) {
        if self.spell == Spell::Wololo {
            update(ai, |own| own.wololo_target = None);
        }
    }

    fn tick(&self, state: &mut UsingSpell, ai: &mut Ai<'_, '_>) {
        state.warmup -= 1;
        if state.warmup == 0 {
            self.perform(ai);
            play(ai, sounds::ENTITY_EVOKER_CAST_SPELL);
        }
    }
}

/// `EvokerSummonSpellGoal.performSpellCasting`: three vexes, each at a block 2 or less
/// across from the evoker's and one up (its own two `nextInt(5)`), made for
/// `MOB_SUMMONED` and finalised there with the difficulty at that block (the level's
/// draws, the evoker's on its behalf), owned by the evoker, bound to that block, with a
/// life of `20 · (30 + nextInt(90))` ticks; each made, then `entity_place` at its block.
fn summon(ai: &mut Ai<'_, '_>) {
    let at = ai.block_position();
    let day = ai.tick.world.day();
    for _ in 0..3 {
        let dx = -2 + ai.tick.draws.next_int(5) as i32;
        let dz = -2 + ai.tick.draws.next_int(5) as i32;
        let pos = BlockPos::new(at.x + dx, at.y + 1, at.z + dz);
        let mut made = Mob::fresh();
        // The vex's own draws (its UUID, its constructor's) are the region's to make
        // with its key; its `finalizeSpawn` draws none of them.
        let mut own = Random::of_entity(0, ai.tick.key, 0);
        crate::mobs::making::finalize(
            vex::VEX,
            &mut Spawning {
                mob: &mut made,
                level: &mut *ai.tick.draws,
                own: &mut own,
                biome: ai.tick.world.biome(pos),
                at: pos,
                day,
                game_time: ai.tick.world.game_time(),
                reason: SpawnReason::MobSummoned,
                group: &mut None,
                structure: None,
                moon_brightness: crate::mobs::making::moon_brightness(day),
                difficulty: difficulty(day),
                companions: None,
            },
        );
        let life = 20 * (30 + ai.tick.draws.next_int(90) as i32);
        let owner = ai.tick.key;
        vex::update(&mut made, |own| {
            own.owner = Some(owner);
            own.bound_origin = Some(pos);
            own.life = Some(life);
        });
        ai.tick.out.local.push(Local::MakeMob {
            kind: vex::VEX,
            mob: Box::new(made),
            at: [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y),
                f64::from(pos.z) + 0.5,
            ],
        });
        ai.tick.out.local.push(Local::GameEvent {
            name: "minecraft:entity_place",
            pos,
        });
    }
}

/// `getCurrentDifficultyAt(pos)` as `Region::difficulty_at` reckons it from the clock's
/// day (the inhabited time is not kept), and its special multiplier: nothing below 2,
/// all above 4, between them its share. At easy it is always nothing.
fn difficulty(day: u64) -> (f32, f32) {
    let base = Difficulty::WORLD;
    if base == Difficulty::Peaceful {
        return (0.0, 0.0);
    }
    let level_time = day as f32;
    let t = ((level_time - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
    let mut c = (crate::mobs::making::moon_brightness(day) * 0.25).clamp(0.0, t);
    if base == Difficulty::Easy {
        c *= 0.5;
    }
    let effective = base.id() as f32 * (0.75 + t + c);
    let special = if effective < 2.0 {
        0.0
    } else if effective > 4.0 {
        1.0
    } else {
        (effective - 2.0) / 2.0
    };
    (effective, special)
}

/// `EvokerAttackSpellGoal.performSpellCasting`: towards the target, between the lower
/// of the two's feet and a block above the higher; within 3 of it, five fangs on a
/// ring of 1.5 at once and eight on a ring of 2.5 after 3; else sixteen in a line, 1.25
/// apart, each one tick after the last. No draws.
fn fangs(ai: &mut Ai<'_, '_>) {
    let Some(target) = sensing::target(ai) else {
        return;
    };
    let [x, y, z] = ai.position();
    let [tx, ty, tz] = target.position;
    let min_y = ty.min(y);
    let max_y = ty.max(y) + 1.0;
    let yaw = maths::atan2(tz - z, tx - x) as f32;
    let pi = std::f32::consts::PI;
    if target.distance_sqr([x, y, z]) < 9.0 {
        for i in 0..5 {
            let angle = yaw + i as f32 * pi * 0.4;
            let (cx, cz) = (x + cos(angle) * 1.5, z + sin(angle) * 1.5);
            spell_entity(ai, cx, cz, min_y, max_y, angle, 0);
        }
        for i in 0..8 {
            let angle = yaw + i as f32 * pi * 2.0 / 8.0 + 1.256_637_1;
            let (cx, cz) = (x + cos(angle) * 2.5, z + sin(angle) * 2.5);
            spell_entity(ai, cx, cz, min_y, max_y, angle, 3);
        }
    } else {
        for i in 0..16 {
            let distance = 1.25 * f64::from(i + 1);
            let (cx, cz) = (x + cos(yaw) * distance, z + sin(yaw) * distance);
            spell_entity(ai, cx, cz, min_y, max_y, yaw, i);
        }
    }
}

/// `Mth.cos` of a float angle, widened.
fn cos(angle: f32) -> f64 {
    f64::from(clustine_noise::trig::cos(f64::from(angle)))
}

/// `Mth.sin` of a float angle, widened.
fn sin(angle: f32) -> f64 {
    f64::from(clustine_noise::trig::sin(f64::from(angle)))
}

/// `EvokerAttackSpellGoal.createSpellEntity(x, z, minY, maxY, yaw, warmup)`: down from
/// the block at `max_y` to the one a block below `min_y`, the first whose block below is
/// sturdy on top; the fangs stand in it, raised by its own collision shape's top where
/// it is not air, turned to `yaw` (radians) and owned by the evoker, with
/// `entity_place` there. Where no block has such a floor, none.
pub(crate) fn spell_entity(
    ai: &mut Ai<'_, '_>,
    x: f64,
    z: f64,
    min_y: f64,
    max_y: f64,
    yaw: f32,
    warmup: i32,
) {
    let Some(at) = floor(ai, x, z, min_y, max_y) else {
        return;
    };
    ai.tick.out.local.push(Local::Fangs {
        at,
        yaw: yaw * 57.295_776,
        warmup,
    });
    ai.tick.out.local.push(Local::GameEventAt {
        name: "minecraft:entity_place",
        at,
        affected: None,
    });
}

/// Where `createSpellEntity`'s search finds its floor, if it does.
pub(crate) fn floor(ai: &Ai<'_, '_>, x: f64, z: f64, min_y: f64, max_y: f64) -> Option<Vec3> {
    let world = ai.tick.world;
    let mut pos = physics::containing([x, max_y, z]);
    let lowest = physics::floor(min_y) - 1;
    loop {
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        if world
            .state(below)
            .props()
            .is_face_sturdy(Face::Up, SupportType::Full)
        {
            let state = world.state(pos);
            let raised = if state.props().is_air() {
                0.0
            } else {
                shape_top(state, pos).unwrap_or(0.0)
            };
            return Some([x, f64::from(pos.y) + raised, z]);
        }
        pos = below;
        if pos.y < lowest {
            return None;
        }
    }
}

/// `EvokerWololoSpellGoal.performSpellCasting`: its sheep, if still alive, made red.
fn wololo(ai: &mut Ai<'_, '_>) {
    let Some(sheep) = own(ai).wololo_target.and_then(|key| sensing::find(ai, key)) else {
        return;
    };
    if sheep.alive {
        ai.tick.out.local.push(Local::SheepColour {
            key: sheep.key,
            colour: RED,
        });
    }
}

/// The colour of a sheep's record (`getColor()`).
pub(crate) fn colour_of(data: &EntityData) -> u8 {
    let EntityData::Mob { state, .. } = data else {
        return 0;
    };
    animal::read::<SheepState>(state).1.colour()
}

impl Region {
    /// `Sheep.setColor(colour)` of the region's sheep `key` (an evoker's wololo,
    /// [`Local::SheepColour`]): its wool's colour, shown to clients. A sheep the region
    /// does not hold keeps its colour (named).
    pub(crate) fn set_sheep_colour(&mut self, key: EntityId, colour: u8, output: &mut TickOutput) {
        let Some((chunk, record)) = self
            .find_entity(key)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return;
        };
        if i32::from(record.kind) != entity_types::SHEEP {
            return;
        }
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let (shared, mut own) = animal::read::<SheepState>(&mob.state);
        own.set_colour(colour);
        mob.state = animal::write(&shared, &own);
        self.commit_mob(&record, chunk, &mob, output);
    }
}
