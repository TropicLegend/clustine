//! The pufferfish (`Pufferfish` and `Pufferfish$PufferfishPuffGoal`;
//! `docs/groundwork/n7-brief.md`, section 6.6.7): a fish that puffs up, half at once and
//! whole 40 ticks later, while something that scares it is within two blocks, and
//! deflates again, half 61 ticks after and flat 40 later; puffed, it stings the mobs it
//! touches.
//!
//! Its box grows with its puff in the tick it puffs, as `refreshDimensions` makes it,
//! without the game's nudge of a box grown into a wall.
//!
//! Named differences: its sting poisons nothing (Clustine has no mob effects until N9:
//! the plan's 7.5.3), and stings no mob that only a copy shows (a hit across a line is
//! a player's alone yet); a player is never stung, every player being a creative one,
//! whom the sting's hurt refuses.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_world::EntityData;

use super::fish::{self, Species};
use super::{Dimensions, Interaction, MobType, Used, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::{self, Conditions, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::shown::DataShown;

/// The pufferfish.
pub(crate) struct Pufferfish;

fn kind() -> u16 {
    entity_types::PUFFERFISH as u16
}

/// `getScale(puffState)`.
pub(crate) fn scale(puff: i32) -> f32 {
    match puff {
        1 => 0.7,
        2 => 1.0,
        _ => 0.5,
    }
}

/// Its size at the puff `puff`: the type's scaled.
pub(crate) fn size(puff: i32) -> Dimensions {
    let info = EntityTypeInfo::of(entity_types::PUFFERFISH);
    water::scaled(info.width, info.height, info.eye_height, scale(puff))
}

/// `SCARY_MOB`: anything but a creative player and the creatures of
/// `#not_scary_for_pufferfish`.
fn scary(looked: &Looked) -> bool {
    if looked.is_player() {
        return !looked.creative;
    }
    !looked
        .kind
        .is_some_and(|kind| entity_in_tag(i32::from(kind), "minecraft:not_scary_for_pufferfish"))
}

/// `TARGETING_CONDITIONS`: not a combat test, through walls and whatever it sees.
fn conditions() -> Conditions {
    Conditions::non_combat()
        .ignore_invisibility()
        .ignore_line_of_sight()
}

/// The living things whose boxes meet `bounds` (`getEntitiesOfClass(LivingEntity)`):
/// the view's mobs and armour stands, then its players.
fn living_in(ai: &Ai<'_, '_>, bounds: Aabb) -> Vec<Looked> {
    let stand = entity_types::ARMOR_STAND as u16;
    let mut found: Vec<Looked> = ai
        .tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| {
            matches!(seen.entity.data, EntityData::Mob { .. }) || seen.entity.kind == stand
        })
        .map(Looked::of_seen)
        .collect();
    found.extend(
        sensing::players(ai)
            .into_iter()
            .filter(|player| player.bounds.intersects(bounds)),
    );
    found
}

/// `PufferfishPuffGoal`: while something scary is within two blocks of its box, it
/// counts up its puffing. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PuffGoal;

impl Goal for PuffGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([2.0; 3]);
        let conditions = conditions();
        living_in(ai, bounds)
            .iter()
            .any(|looked| conditions.test(ai, looked, &scary))
    }

    /// `inflateCounter = 1; deflateTimer = 0`: the next tick's step reads the counter
    /// as 1.
    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let next = ai.tick.lived.saturating_add(1);
        fish::update(kind(), &mut ai.tick.mob, |fish| {
            fish.inflate_from = Some(next);
            fish.deflate_from = None;
        });
    }

    /// `inflateCounter = 0`: the deflate timer counts from 0 at the next tick's step.
    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let next = ai.tick.lived.saturating_add(1);
        fish::update(kind(), &mut ai.tick.mob, |fish| {
            fish.inflate_from = None;
            fish.deflate_from = Some(next);
        });
    }
}

/// `setPuffState(puff)`, with the box it gives at once.
fn set_puff(tick: &mut MobTick<'_>, puff: i32) {
    fish::update(kind(), &mut tick.mob, |fish| fish.puff = puff);
    let size = size(puff);
    tick.living.body.width = size.width;
    tick.living.body.height = size.height;
    tick.living.eye_height = size.eye_height;
}

/// `Pufferfish.tick()` before the fish's: puffing while its counter counts, half at once
/// and whole once it is past 40; deflating once it stops, half past 60 and flat past
/// 100; each change with its sound (two draws).
fn puff(tick: &mut MobTick<'_>) {
    if !water::alive(&tick.mob) || !tick.effective_ai() {
        return;
    }
    let now = tick.lived;
    let fish = fish::read(kind(), &tick.mob.state);
    if let Some(from) = fish.inflate_from {
        let counter = (i64::from(now) - i64::from(from) + 1).max(1);
        if fish.puff == 0 {
            water::make_sound(tick, sounds::ENTITY_PUFFER_FISH_BLOW_UP, 1.0, false);
            set_puff(tick, 1);
        } else if counter > 40 && fish.puff == 1 {
            water::make_sound(tick, sounds::ENTITY_PUFFER_FISH_BLOW_UP, 1.0, false);
            set_puff(tick, 2);
        }
    } else if fish.puff != 0 {
        let Some(from) = fish.deflate_from else {
            // Puffed with no timer (as loaded): it counts from 0 now.
            fish::update(kind(), &mut tick.mob, |fish| fish.deflate_from = Some(now));
            return;
        };
        let timer = (i64::from(now) - i64::from(from)).max(0);
        if timer > 60 && fish.puff == 2 {
            water::make_sound(tick, sounds::ENTITY_PUFFER_FISH_BLOW_OUT, 1.0, false);
            set_puff(tick, 1);
        } else if timer > 100 && fish.puff == 1 {
            water::make_sound(tick, sounds::ENTITY_PUFFER_FISH_BLOW_OUT, 1.0, false);
            set_puff(tick, 0);
        }
    }
}

/// `Pufferfish.aiStep()` after the fish's: puffed, it stings every living mob within 0.3
/// of its box that scares it, for one more than its puff (`touch`), as a hit of a mob
/// for the region to deal, with the sting's sound where it took.
fn sting(tick: &mut MobTick<'_>) {
    let fish = fish::read(kind(), &tick.mob.state);
    if !water::alive(&tick.mob) || fish.puff <= 0 {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([0.3; 3]);
    let from = body.position;
    let stung: Vec<_> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied && seen.entity.key != tick.key)
        .filter(|seen| matches!(seen.entity.data, EntityData::Mob { .. }))
        .map(Looked::of_seen)
        .filter(|looked| looked.seen_by_anyone() && scary(looked))
        .map(|looked| looked.key)
        .collect();
    for key in stung {
        tick.out.local.push(Local::Hurt {
            key,
            kind: kind(),
            from,
            amount: (1 + fish.puff) as f32,
            sound: Some(sounds::ENTITY_PUFFER_FISH_STING),
        });
    }
}

impl MobType for Pufferfish {
    fn register_goals(&self, setup: &mut Setup) {
        fish::register_goals(setup, Species::Pufferfish);
        setup.add_goal(1, PuffGoal);
    }

    fn before_tick(&self, tick: &mut MobTick<'_>) {
        puff(tick);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        fish::flop(tick, sounds::ENTITY_PUFFER_FISH_FLOP);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        sting(tick);
    }

    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        fish::travel(tick)
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        fish::tick_move_control(ai);
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        fish::mob_interact(used)
    }

    fn remove_when_far_away(&self, kind: u16, mob: &Mob, _: u32, _: f64) -> bool {
        fish::remove_when_far_away(kind, mob)
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        Some(size(fish::read(kind(), &mob.state).puff))
    }

    /// None: a pufferfish makes no sound of its own.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        None
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PUFFER_FISH_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PUFFER_FISH_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        fish::shown(kind(), mob, true)
    }
}
