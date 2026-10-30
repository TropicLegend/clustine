//! The hoglin's and the zoglin's rules apart from a region (family E6 of P12): the spawn
//! rule, the egg, a bastion stable's hoglin from its template, what is hunted, what is
//! shown and what is heard.

use std::collections::VecDeque;

use clustine_data::{BlockState, blocks, entity_types, sounds};
use clustine_world::generation::{Draws, Makings, Placed};
use clustine_world::{Biome, BlockPos, ChunkPos};

use super::template::build as nbt;
use super::{animal, hoglin, hoglin_ai, zoglin};
use crate::mobs::Mob;
use crate::mobs::brain::Activity;
use crate::shown::DataShown;
use crate::spawning::rules::{Light, SpawnWorld, check_spawn_rules};

const HOGLIN: u16 = entity_types::HOGLIN as u16;
const ZOGLIN: u16 = entity_types::ZOGLIN as u16;

/// Air everywhere but `under` below the spawn's block.
struct Ground {
    under: BlockState,
}

impl SpawnWorld for Ground {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(if pos.y == 63 { self.under } else { blocks::AIR })
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        None
    }

    fn sky_darken(&self) -> i32 {
        0
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        None
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        None
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        0
    }
}

/// A random that must not be drawn from.
struct NoDraws;

impl Draws for NoDraws {
    fn next_int(&mut self, _: u32) -> u32 {
        unreachable!("the rule draws nothing")
    }

    fn next_float(&mut self) -> f32 {
        unreachable!("the rule draws nothing")
    }

    fn next_double(&mut self) -> f64 {
        unreachable!("the rule draws nothing")
    }

    fn next_bool(&mut self) -> bool {
        unreachable!("the rule draws nothing")
    }
}

#[test]
fn a_hoglin_spawns_anywhere_but_above_a_nether_wart_block_drawing_nothing() {
    let at = BlockPos::new(0, 64, 0);
    for (under, spawns) in [
        (blocks::CRIMSON_NYLIUM, true),
        (blocks::NETHERRACK, true),
        (blocks::NETHER_WART_BLOCK, false),
    ] {
        assert_eq!(
            check_spawn_rules(HOGLIN, &Ground { under }, at, &mut NoDraws),
            spawns,
            "{under:?}"
        );
    }
}

#[test]
fn eggs_and_structures_make_hoglins_and_zoglins() {
    assert!(super::is_built(HOGLIN));
    assert!(super::is_built(ZOGLIN));
}

/// The level's random of a template's making: it notes each call and answers a half.
#[derive(Default)]
struct Noted {
    calls: VecDeque<&'static str>,
}

impl Draws for Noted {
    fn next_int(&mut self, _: u32) -> u32 {
        self.calls.push_back("int");
        0
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push_back("float");
        0.5
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push_back("double");
        0.5
    }

    fn next_bool(&mut self) -> bool {
        unreachable!("no hoglin's making draws a boolean")
    }
}

#[test]
fn a_bastion_stable_s_hoglin_is_made_from_its_template_kept_and_not_to_be_hunted() {
    // `bastion/mobs/hoglin`'s entity as its template keeps it (n7-templates), with the
    // follow range's bonus a placed mob carries.
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:hoglin"),
        nbt::byte("PersistenceRequired", 1),
        nbt::byte("CannotBeHunted", 1),
        nbt::int("TimeInOverworld", 0),
        nbt::int("Age", 0),
        nbt::compounds(
            "attributes",
            &[nbt::attribute(
                "minecraft:follow_range",
                16.0,
                &[(
                    crate::mobs::body::attributes::RANDOM_SPAWN_BONUS,
                    0.02,
                    "add_multiplied_base",
                )],
            )],
        ),
    ]);
    let mut makings = Makings::new(0, ChunkPos::new(0, 0));
    let mut level = Noted::default();
    let made = crate::generation::template_entity(
        5,
        &tag,
        true,
        Placed {
            position: [8.5, 40.0, 8.5],
            yaw: 0.0,
            pitch: 0.0,
            structure: Some("minecraft:bastion_remnant"),
            biome: Biome(0),
            makings: &mut makings,
            level: &mut level,
        },
    );
    assert_eq!(made.len(), 1, "a hoglin is made");
    assert_eq!(made[0].kind, HOGLIN);
    let mob = Mob::decode(&made[0].data).unwrap();
    assert!(mob.body.flags.persistent);
    assert!(hoglin::state_of(&mob).cannot_be_hunted);
    assert!(hoglin::cannot_be_hunted(&mob));
    // Its baby's draw (a half is no baby), then `Mob`'s hand: its bonus draws no
    // triangle.
    assert_eq!(Vec::from(level.calls), ["float", "float"]);
    assert!(!animal::is_baby(&mob));
}

#[test]
fn a_baby_cannot_be_hunted_and_an_adult_can_unless_its_stable_keeps_it() {
    let adult = Mob::fresh();
    assert!(!hoglin::cannot_be_hunted(&adult));
    let mut baby = Mob::fresh();
    super::of(HOGLIN).set_baby(&mut baby, true);
    assert!(hoglin::cannot_be_hunted(&baby));
    let mut kept = Mob::fresh();
    hoglin::update(&mut kept, |own| own.cannot_be_hunted = true);
    assert!(hoglin::cannot_be_hunted(&kept));
}

#[test]
fn a_hoglin_sounds_its_activity_its_retreat_while_converting_or_near_a_repellent() {
    use hoglin_ai::sound_for_activity as sound;
    assert_eq!(
        sound(Activity::Idle, false, false),
        sounds::ENTITY_HOGLIN_AMBIENT
    );
    assert_eq!(
        sound(Activity::Fight, false, false),
        sounds::ENTITY_HOGLIN_ANGRY
    );
    assert_eq!(
        sound(Activity::Avoid, false, false),
        sounds::ENTITY_HOGLIN_RETREAT
    );
    assert_eq!(
        sound(Activity::Fight, true, false),
        sounds::ENTITY_HOGLIN_RETREAT
    );
    assert_eq!(
        sound(Activity::Idle, false, true),
        sounds::ENTITY_HOGLIN_RETREAT
    );
    assert_eq!(
        sound(Activity::Fight, false, true),
        sounds::ENTITY_HOGLIN_ANGRY
    );
}

#[test]
fn a_hoglin_shows_its_immunity_at_18_and_a_zoglin_its_babyhood_at_16() {
    let mut immune = Mob::fresh();
    hoglin::update(&mut immune, |own| own.immune_to_zombification = true);
    let shown = super::of(HOGLIN).shown(&immune);
    assert!(
        shown.contains(&(18, DataShown::Boolean(true), false)),
        "{shown:?}"
    );
    assert!(super::of(HOGLIN).shown(&Mob::fresh()).contains(&(
        18,
        DataShown::Boolean(false),
        true
    )));
    let mut baby = Mob::fresh();
    super::of(ZOGLIN).set_baby(&mut baby, true);
    assert_eq!(
        super::of(ZOGLIN).shown(&baby),
        [(16, DataShown::Boolean(true), false)]
    );
    assert!(zoglin::state_of(&baby).baby);
    let attack = crate::mobs::body::attribute(clustine_data::attributes::ATTACK_DAMAGE);
    assert_eq!(
        baby.body.attributes.value(ZOGLIN, attack),
        0.5,
        "a baby's hit"
    );
}

#[test]
fn a_hoglin_is_an_enemy_heard_as_a_monster_is() {
    use crate::mobs::body::ladder;
    assert!(ladder::is_enemy(HOGLIN));
    assert_eq!(ladder::sound_source(HOGLIN), 5, "hostile");
    assert!(ladder::is_enemy(ZOGLIN));
    assert!(super::of(HOGLIN).can_be_leashed(HOGLIN, &Mob::fresh()));
    assert!(super::of(ZOGLIN).can_be_leashed(ZOGLIN, &Mob::fresh()));
}
