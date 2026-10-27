//! The tropical fish (`TropicalFish` with `$Pattern`, `$Base` and `$Variant`;
//! `docs/groundwork/n7-brief.md`, section 6.6.8): a schooling fish of a pattern and two
//! colours packed in one number, nine times in ten one of 22 common kinds, else any
//! pattern of any two colours, drawn from the level's random after `Mob`'s own draws.
//!
//! An egg or a bucket makes each fish the first of its group, so it draws its variant;
//! the variant a group shares (`TropicalFishGroupData`) is natural spawning's, N9's.

use clustine_data::{entity_types, sounds};

use super::fish::{self, Species};
use super::{AgeableGroup, Interaction, MobType, SpawnGroup, Spawning, Used, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The patterns in their declaration order (`Pattern.values()`), each its packed id:
/// its base's id (small 0, large 1) and its index among its base's above it.
pub(crate) const PATTERNS: [i32; 12] = [
    0x000, // kob
    0x100, // sunstreak
    0x200, // snooper
    0x300, // dasher
    0x400, // brinely
    0x500, // spotty
    0x001, // flopper
    0x101, // stripey
    0x201, // glitter
    0x301, // blockfish
    0x401, // betty
    0x501, // clayfish
];

/// The dye colours by id (`DyeColor.values()`).
mod colour {
    pub(super) const WHITE: i32 = 0;
    pub(super) const ORANGE: i32 = 1;
    pub(super) const LIGHT_BLUE: i32 = 3;
    pub(super) const YELLOW: i32 = 4;
    pub(super) const LIME: i32 = 5;
    pub(super) const PINK: i32 = 6;
    pub(super) const GRAY: i32 = 7;
    pub(super) const CYAN: i32 = 9;
    pub(super) const PURPLE: i32 = 10;
    pub(super) const BLUE: i32 = 11;
    pub(super) const RED: i32 = 14;
}

/// The patterns by name, as the common variants name them.
mod pattern {
    pub(super) const KOB: i32 = 0x000;
    pub(super) const SUNSTREAK: i32 = 0x100;
    pub(super) const SNOOPER: i32 = 0x200;
    pub(super) const DASHER: i32 = 0x300;
    pub(super) const BRINELY: i32 = 0x400;
    pub(super) const SPOTTY: i32 = 0x500;
    pub(super) const FLOPPER: i32 = 0x001;
    pub(super) const STRIPEY: i32 = 0x101;
    pub(super) const GLITTER: i32 = 0x201;
    pub(super) const BLOCKFISH: i32 = 0x301;
    pub(super) const BETTY: i32 = 0x401;
    pub(super) const CLAYFISH: i32 = 0x501;
}

/// `COMMON_VARIANTS`, in the list's order: pattern, base colour, pattern colour.
pub(crate) const COMMON: [(i32, i32, i32); 22] = {
    use colour::*;
    use pattern::*;
    [
        (STRIPEY, ORANGE, GRAY),
        (FLOPPER, GRAY, GRAY),
        (FLOPPER, GRAY, BLUE),
        (CLAYFISH, WHITE, GRAY),
        (SUNSTREAK, BLUE, GRAY),
        (KOB, ORANGE, WHITE),
        (SPOTTY, PINK, LIGHT_BLUE),
        (BLOCKFISH, PURPLE, YELLOW),
        (CLAYFISH, WHITE, RED),
        (SPOTTY, WHITE, YELLOW),
        (GLITTER, WHITE, GRAY),
        (CLAYFISH, WHITE, ORANGE),
        (DASHER, CYAN, PINK),
        (BRINELY, LIME, LIGHT_BLUE),
        (BETTY, RED, WHITE),
        (SNOOPER, GRAY, RED),
        (BLOCKFISH, RED, WHITE),
        (FLOPPER, WHITE, YELLOW),
        (KOB, RED, WHITE),
        (SUNSTREAK, GRAY, WHITE),
        (DASHER, CYAN, YELLOW),
        (FLOPPER, YELLOW, YELLOW),
    ]
};

/// `packVariant(pattern, base, patternColour)`.
pub(crate) fn pack(pattern: i32, base: i32, pattern_colour: i32) -> i32 {
    (pattern & 0xFFFF) | ((base & 255) << 16) | ((pattern_colour & 255) << 24)
}

/// The variant [`drawn_variant`] draws, as the tests read it.
#[cfg(test)]
pub(crate) fn random_variant(level: &mut dyn clustine_world::generation::Draws) -> i32 {
    drawn_variant(level).0
}

/// `finalizeSpawn`'s variant, of the level's random after the parent's draws: nine times
/// in ten (`nextFloat() < 0.9`, compared in double) one of the common ones
/// (`nextInt(22)`), else a pattern (`nextInt(12)`) and two colours (`nextInt(16)` each);
/// and whether it was a common one, which makes a school (`TropicalFishGroupData`): one
/// drawn whole leaves the fish no school's (`isSchool` false).
fn drawn_variant(level: &mut dyn clustine_world::generation::Draws) -> (i32, bool) {
    if f64::from(level.next_float()) < 0.9 {
        let (pattern, base, colour) = COMMON[level.next_int(22) as usize];
        return (pack(pattern, base, colour), true);
    }
    let pattern = PATTERNS[level.next_int(12) as usize];
    let base = level.next_int(16) as i32;
    let colour = level.next_int(16) as i32;
    (pack(pattern, base, colour), false)
}

/// The tropical fish.
pub(crate) struct TropicalFish;

fn kind() -> u16 {
    entity_types::TROPICAL_FISH as u16
}

impl MobType for TropicalFish {
    fn register_goals(&self, setup: &mut Setup) {
        fish::register_goals(setup, Species::TropicalFish);
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        fish::construct(mob, Species::TropicalFish, own);
    }

    /// `TropicalFish.finalizeSpawn` after its parents': the school's variant where the
    /// group has one (no draw); else drawn, a common one making the group a school whose
    /// later members take it.
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = match *spawn.group {
            Some(SpawnGroup::TropicalFish { variant, .. }) => variant,
            _ => {
                let (variant, school) = drawn_variant(spawn.level);
                if school {
                    *spawn.group = Some(SpawnGroup::TropicalFish {
                        ageable: AgeableGroup::new(false),
                        variant,
                    });
                }
                variant
            }
        };
        fish::update(kind(), spawn.mob, |fish| fish.variant = variant);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        fish::flop(tick, sounds::ENTITY_TROPICAL_FISH_FLOP);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        fish::census(tick);
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

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_TROPICAL_FISH_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_TROPICAL_FISH_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_TROPICAL_FISH_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        fish::shown(kind(), mob, true)
    }
}
