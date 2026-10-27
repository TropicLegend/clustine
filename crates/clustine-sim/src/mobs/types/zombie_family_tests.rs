//! The zombie family's rules on their own, from the brief's sections 3.4, 5, 11.1, 11.5,
//! 11.8 and 11.12 (`docs/groundwork/n7k-brief.md`): each type's egg's draws at easy in
//! the game's order (the brief's **Z** list), a baby's and a chicken jockey's, the
//! husk's second loot draw, the drowned's equipment, a zombie villager's profession at
//! its construction, the drowning's count, what clients are shown and what a template
//! gives.

use std::collections::VecDeque;

use clustine_data::villagers::{professions, types};
use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::template::build::{self as nbt};
use super::zombie_family::{self as family, Tracker};
use super::{Companion, CompanionSeat, Companions, Interaction, SpawnReason, Spawning, of};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::making;
use crate::physics::aabb::Aabb;
use crate::region::Random;
use crate::shown::DataShown;

/// A draw of the level's random, as the counting stream records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// The level's random of a test: it records every call, and answers each from its
/// script where one is given, else with a middling value (an int of 0, a float or a
/// double of a half).
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    ints: VecDeque<u32>,
    floats: VecDeque<f32>,
}

impl Counting {
    fn floats(floats: &[f32]) -> Self {
        Self {
            floats: floats.iter().copied().collect(),
            ..Self::default()
        }
    }
}

impl clustine_world::generation::Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        self.ints
            .pop_front()
            .unwrap_or(0)
            .min(bound.saturating_sub(1))
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        self.floats.pop_front().unwrap_or(0.5)
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        0.5
    }

    fn next_bool(&mut self) -> bool {
        unreachable!("no zombie draws a boolean")
    }
}

/// A maker of companions that keeps what it is given and offers the entities `near`
/// (their keys, types and whether they carry or ride anything) to a search.
#[derive(Default)]
struct Kept {
    made: Vec<Companion>,
    near: Vec<(EntityId, u16, bool)>,
    searched: Option<Aabb>,
    rides: Option<EntityId>,
}

impl Companions for Kept {
    fn next(&mut self) -> Option<(EntityId, Random)> {
        let key = EntityId(100 + self.made.len() as u64);
        Some((key, Random::of_entity(7, key, 0)))
    }

    fn add(&mut self, companion: Companion) {
        self.made.push(companion);
    }

    fn nearby(&self, bounds: &Aabb) -> Vec<(EntityId, u16, bool)> {
        assert!(self.searched.is_none() || self.searched == Some(*bounds));
        self.near.clone()
    }

    fn ride(&mut self, vehicle: EntityId) {
        self.rides = Some(vehicle);
    }

    fn position(&self) -> Option<crate::physics::Vec3> {
        Some([0.5, 64.0, 0.5])
    }
}

/// The Mob tail: the follow range's triangle, then the hand.
const TAIL: [Call; 3] = [Double, Double, Float];

/// A mob of `kind` as an egg makes it, finalised from `level` with `reason`, in
/// `biome`, made beside by `kept`: the mob and its own stream after its making, with
/// the stream as its construction left it.
fn made(
    kind: i32,
    reason: SpawnReason,
    biome: Option<&'static str>,
    level: &mut Counting,
    kept: Option<&mut Kept>,
) -> (Mob, Random, Random) {
    let kind = kind as u16;
    let mut own = Random::of_entity(3, EntityId(9), 0);
    let (_, mut mob) = making::construct(kind, &mut own, 0.0);
    let constructed = own.clone();
    making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome,
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: kept.map(|kept| kept as &mut dyn Companions),
        },
    );
    (mob, own, constructed)
}

/// `handleAttributes` at special 0: the reinforcements' chance, the knockback bonus,
/// the follow range's draw and the leader's, all of the mob's own stream.
fn after_own_four(mut stream: Random) -> Random {
    stream.next_double();
    stream.next_double();
    stream.next_double();
    stream.next_float();
    stream
}

fn held(mob: &Mob, slot: u8) -> Option<i32> {
    mob.body.equipment.slots[usize::from(slot)]
        .as_ref()
        .map(|stack| stack.item)
}

#[test]
fn an_egg_s_zombie_draws_the_tail_loot_baby_doors_armour_and_weapon_then_four_of_its_own() {
    for kind in [entity_types::ZOMBIE, entity_types::ZOMBIE_VILLAGER] {
        let mut level = Counting::default();
        let (mob, own, constructed) = made(kind, SpawnReason::SpawnEgg, None, &mut level, None);
        // Tail, loot, baby, doors, armour (at special 0 it stops there), the weapon's
        // chance; no weapon, so no enchantment's draw.
        let mut expected = TAIL.to_vec();
        expected.extend([Float, Float, Float, Float, Float]);
        assert_eq!(level.calls, expected, "{kind}");
        assert_eq!(own, after_own_four(constructed), "{kind}");
        assert!(!family::is_baby(&mob) && !mob.body.flags.picks_up_loot);
        assert!(!family::read(&mob).can_break_doors);
        assert!(mob.body.equipment.slots.iter().all(Option::is_none));
        // The knockback bonus of `random_spawn_bonus`, a twentieth at most.
        let knockback = mob
            .body
            .attributes
            .made(attribute(clustine_data::attributes::KNOCKBACK_RESISTANCE))
            .expect("made by its bonus");
        assert!(knockback.has_modifier("minecraft:random_spawn_bonus"));
        let reinforcements = mob
            .body
            .attributes
            .made(attribute(clustine_data::attributes::SPAWN_REINFORCEMENTS))
            .expect("made by its chance");
        assert!((0.0..0.1).contains(&reinforcements.base));
    }
}

#[test]
fn a_zombie_s_weapon_one_time_in_a_hundred_is_a_sword_spear_or_shovel_and_its_enchantment_drawn() {
    for (int, item) in [
        (0, items::IRON_SWORD),
        (1, items::IRON_SPEAR),
        (2, items::IRON_SHOVEL),
        (5, items::IRON_SHOVEL),
    ] {
        let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.5, 0.009]);
        level.ints.push_back(int);
        let (mob, _, _) = made(
            entity_types::ZOMBIE,
            SpawnReason::SpawnEgg,
            None,
            &mut level,
            None,
        );
        let mut expected = TAIL.to_vec();
        expected.extend([Float, Float, Float, Float, Float, Int(6), Float]);
        assert_eq!(level.calls, expected);
        assert_eq!(held(&mob, slots::MAIN_HAND), Some(item));
    }
    // At 0.01 the chance is missed.
    let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.5, 0.01]);
    let (mob, _, _) = made(
        entity_types::ZOMBIE,
        SpawnReason::SpawnEgg,
        None,
        &mut level,
        None,
    );
    assert_eq!(held(&mob, slots::MAIN_HAND), None);
}

#[test]
fn a_baby_zombie_is_small_and_fast_and_draws_for_a_chicken_twice() {
    // Baby at 0.0; neither chicken's chance at 0.05 (not below it).
    let mut level = Counting::floats(&[0.5, 0.5, 0.0, 0.05, 0.05]);
    let mut kept = Kept::default();
    let (mob, _, _) = made(
        entity_types::ZOMBIE,
        SpawnReason::SpawnEgg,
        None,
        &mut level,
        Some(&mut kept),
    );
    let mut expected = TAIL.to_vec();
    // Loot, baby, the chicken near, the new chicken, doors, armour, weapon.
    expected.extend([Float, Float, Float, Float, Float, Float, Float]);
    assert_eq!(level.calls, expected);
    assert!(family::is_baby(&mob));
    assert!(kept.made.is_empty() && kept.rides.is_none());
    let kind = of(entity_types::ZOMBIE as u16);
    let size = kind.dimensions(&mob).expect("a baby's own size");
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.49, 0.98, 0.775)
    );
    let speed = mob
        .body
        .attributes
        .made(attribute(clustine_data::attributes::MOVEMENT_SPEED))
        .expect("made by the baby's speed");
    assert!(speed.has_modifier("minecraft:baby"));
    assert!((speed.value() - 0.23 * 1.5).abs() < 1e-6);
    // Its synched baby, shown.
    assert!(
        kind.shown(&mob)
            .contains(&(16, DataShown::Boolean(true), false))
    );
    // A grown one's size is the type's.
    assert!(kind.dimensions(&Mob::fresh()).is_none());
}

#[test]
fn a_baby_rides_the_first_chicken_near_that_carries_nobody_and_draws_no_second_chance() {
    let chicken = entity_types::CHICKEN as u16;
    let mut level = Counting::floats(&[0.5, 0.5, 0.0, 0.04]);
    let mut kept = Kept {
        near: vec![
            (EntityId(1), entity_types::COW as u16, false),
            (EntityId(2), chicken, true),
            (EntityId(3), chicken, false),
            (EntityId(4), chicken, false),
        ],
        searched: Some(Aabb::of_entity([0.5, 64.0, 0.5], 0.49, 0.98).inflate([5.0, 3.0, 5.0])),
        ..Kept::default()
    };
    made(
        entity_types::ZOMBIE,
        SpawnReason::SpawnEgg,
        None,
        &mut level,
        Some(&mut kept),
    );
    assert_eq!(kept.rides, Some(EntityId(3)));
    assert!(kept.made.is_empty());
    let mut expected = TAIL.to_vec();
    // Loot, baby, the chicken near; then doors, armour, weapon.
    expected.extend([Float, Float, Float, Float, Float, Float]);
    assert_eq!(level.calls, expected);
}

#[test]
fn a_baby_s_new_chicken_is_made_a_jockey_under_it_finalised_from_the_level_s_draws() {
    let mut level = Counting::floats(&[0.5, 0.5, 0.0, 0.5, 0.04]);
    let mut kept = Kept::default();
    made(
        entity_types::ZOMBIE,
        SpawnReason::SpawnEgg,
        Some("minecraft:plains"),
        &mut level,
        Some(&mut kept),
    );
    assert_eq!(kept.made.len(), 1);
    let chicken = &kept.made[0];
    assert_eq!(chicken.seat, CompanionSeat::Vehicle);
    assert_eq!(i32::from(chicken.entity.kind), entity_types::CHICKEN);
    assert_eq!(chicken.entity.position, [0.5, 64.0, 0.5]);
    let decoded = Mob::decode(&chicken.entity.data).unwrap();
    let (_, own) = super::animal::read::<super::chicken::ChickenState>(&decoded.state);
    assert!(own.jockey);
    // The zombie's tail, loot, baby and two chances; then the chicken's own making from
    // the level's random (its tail among them); then the zombie's doors, armour and
    // weapon.
    assert_eq!(
        level.calls[..7],
        [Double, Double, Float, Float, Float, Float, Float]
    );
    assert!(level.calls.len() > 7 + 3 + 3);
    assert_eq!(
        &level.calls[level.calls.len() - 3..],
        &[Float, Float, Float]
    );
}

#[test]
fn a_husk_draws_its_loot_s_chance_again_after_the_zombie_s() {
    let mut level = Counting::default();
    let (_, own, constructed) = made(
        entity_types::HUSK,
        SpawnReason::SpawnEgg,
        None,
        &mut level,
        None,
    );
    let mut expected = TAIL.to_vec();
    expected.extend([Float, Float, Float, Float, Float, Float]);
    assert_eq!(level.calls, expected);
    assert_eq!(own, after_own_four(constructed));
}

#[test]
fn a_drowned_holds_a_trident_or_rod_by_its_own_draws_and_carries_a_shell_one_time_in_33() {
    // The drowned's equipment in place of armour and weapon: above 0.9 a draw of 16,
    // under 10 a trident; its enchantment's draw with it; then the shell's.
    for (int, item) in [
        (0, items::TRIDENT),
        (9, items::TRIDENT),
        (10, items::FISHING_ROD),
    ] {
        let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.95, 0.5, 0.029]);
        level.ints.push_back(int);
        let (mob, _, _) = made(
            entity_types::DROWNED,
            SpawnReason::SpawnEgg,
            None,
            &mut level,
            None,
        );
        let mut expected = TAIL.to_vec();
        expected.extend([Float, Float, Float, Float, Int(16), Float, Float]);
        assert_eq!(level.calls, expected);
        assert_eq!(held(&mob, slots::MAIN_HAND), Some(item));
        assert_eq!(held(&mob, slots::OFF_HAND), Some(items::NAUTILUS_SHELL));
        assert_eq!(
            mob.body.equipment.drop_chance(usize::from(slots::OFF_HAND)),
            2.0
        );
    }
    // 0.9 itself is not above it (as a double, 0.9f is a little below 0.9); no weapon,
    // no enchantment's draw, and the shell missed at 0.03.
    let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.9, 0.03]);
    let (mob, _, _) = made(
        entity_types::DROWNED,
        SpawnReason::SpawnEgg,
        None,
        &mut level,
        None,
    );
    let mut expected = TAIL.to_vec();
    expected.extend([Float, Float, Float, Float, Float]);
    assert_eq!(level.calls, expected);
    assert!(mob.body.equipment.slots.iter().all(Option::is_none));
}

#[test]
fn one_drowned_in_16_holds_a_trident_by_its_draws() {
    let mut tridents = 0;
    let mut rods = 0;
    let mut shells = 0;
    let eggs = 16_000;
    for seed in 0..eggs {
        let kind = entity_types::DROWNED as u16;
        let mut own = Random::of_entity(3, EntityId(seed), 0);
        let mut level = Random::of_entity(5, EntityId(seed), 0);
        let (_, mut mob) = making::construct(kind, &mut own, 0.0);
        making::finalize(
            kind,
            &mut Spawning {
                mob: &mut mob,
                level: &mut level,
                own: &mut own,
                biome: None,
                at: BlockPos::new(0, 64, 0),
                day: 0,
                game_time: 0,
                reason: SpawnReason::SpawnEgg,
                group: &mut None,
                structure: None,
                moon_brightness: 1.0,
                difficulty: (0.0, 0.0),
                companions: None,
            },
        );
        match held(&mob, slots::MAIN_HAND) {
            Some(items::TRIDENT) => tridents += 1,
            Some(items::FISHING_ROD) => rods += 1,
            _ => {}
        }
        shells += usize::from(held(&mob, slots::OFF_HAND).is_some());
    }
    // 1000 tridents, 600 rods and 480 shells expected; within four deviations.
    assert!((880..1120).contains(&tridents), "{tridents}");
    assert!((500..700).contains(&rods), "{rods}");
    assert!((390..570).contains(&shells), "{shells}");
}

#[test]
fn a_structure_s_drowned_with_a_trident_rides_a_persistent_zombie_nautilus_one_time_in_two() {
    // Trident; the shell missed; the nautilus's half.
    let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.95, 0.5, 0.5, 0.49]);
    let mut kept = Kept::default();
    made(
        entity_types::DROWNED,
        SpawnReason::Structure,
        Some("minecraft:warm_ocean"),
        &mut level,
        Some(&mut kept),
    );
    assert_eq!(kept.made.len(), 1);
    let nautilus = &kept.made[0];
    assert_eq!(
        i32::from(nautilus.entity.kind),
        entity_types::ZOMBIE_NAUTILUS
    );
    assert_eq!(nautilus.seat, CompanionSeat::Vehicle);
    assert!(
        Mob::decode(&nautilus.entity.data)
            .unwrap()
            .body
            .flags
            .persistent
    );
    // In a river it draws the half all the same, and makes none.
    let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.95, 0.5, 0.5, 0.0]);
    let mut kept = Kept::default();
    made(
        entity_types::DROWNED,
        SpawnReason::Structure,
        Some("minecraft:river"),
        &mut level,
        Some(&mut kept),
    );
    assert!(kept.made.is_empty());
    assert_eq!(level.calls.last(), Some(&Float));
    // An egg's draws no half.
    let mut level = Counting::floats(&[0.5, 0.5, 0.5, 0.5, 0.95, 0.5, 0.5, 0.0]);
    let mut kept = Kept::default();
    made(
        entity_types::DROWNED,
        SpawnReason::SpawnEgg,
        Some("minecraft:warm_ocean"),
        &mut level,
        Some(&mut kept),
    );
    assert!(kept.made.is_empty());
    let mut expected = TAIL.to_vec();
    expected.extend([Float, Float, Float, Float, Int(16), Float, Float]);
    assert_eq!(level.calls, expected);
}

#[test]
fn a_zombie_villager_draws_its_profession_after_its_uuid_and_before_its_yaw() {
    let kind = entity_types::ZOMBIE_VILLAGER as u16;
    let mut own = Random::of_entity(3, EntityId(9), 0);
    let mut replay = own.clone();
    let (_, mob) = making::construct(kind, &mut own, 0.0);
    replay.next_u64();
    replay.next_u64();
    let profession = replay.next_int(15) as i32;
    replay.next_float();
    assert_eq!(own, replay, "nothing else is drawn");
    let villager = super::zombie_villager::villager(&mob);
    assert_eq!(villager.profession, profession);
    assert_eq!((villager.kind, villager.level), (types::PLAINS, 1));
    assert!(!villager.finalised);
    // Its data at the default its construction made is not sent to a client paired.
    let shown = of(kind).shown(&mob);
    assert!(shown.contains(&(
        20,
        DataShown::VillagerData {
            kind: types::PLAINS,
            profession,
            level: 1
        },
        true
    )));
    assert!(shown.contains(&(19, DataShown::Boolean(false), true)));
    assert!(shown.contains(&(21, DataShown::Boolean(false), true)));
}

#[test]
fn a_zombie_villager_s_type_is_its_biome_s_and_settled_once_made() {
    let mut level = Counting::default();
    let (mob, _, _) = made(
        entity_types::ZOMBIE_VILLAGER,
        SpawnReason::SpawnEgg,
        Some("minecraft:desert"),
        &mut level,
        None,
    );
    let villager = super::zombie_villager::villager(&mob);
    assert_eq!(villager.kind, types::DESERT);
    assert!(villager.finalised);
    let shown = of(entity_types::ZOMBIE_VILLAGER as u16).shown(&mob);
    assert!(shown.contains(&(21, DataShown::Boolean(true), false)));
    assert!(shown.iter().any(|(index, value, default)| *index == 20
        && matches!(value, DataShown::VillagerData { kind, .. } if *kind == types::DESERT)
        && !default));
}

#[test]
fn a_template_s_zombie_villager_keeps_its_villager_data_and_is_finalised() {
    let kind = entity_types::ZOMBIE_VILLAGER as u16;
    let tag = nbt::compound(&[
        nbt::byte("IsBaby", 1),
        nbt::int("ConversionTime", -1),
        nbt::nested(
            "VillagerData",
            &[
                nbt::string("profession", "minecraft:cleric"),
                nbt::int("level", 1),
                nbt::string("type", "minecraft:snow"),
            ],
        ),
    ]);
    let (_, mut mob) = making::construct(kind, &mut Random::of_entity(3, EntityId(9), 0), 0.0);
    super::from_template(kind, &mut mob, &tag);
    let villager = super::zombie_villager::villager(&mob);
    assert_eq!(
        (villager.kind, villager.profession, villager.level),
        (types::SNOW, professions::CLERIC, 1)
    );
    assert!(villager.finalised && family::is_baby(&mob));
    assert_eq!(villager.conversion, -1);
    // Its making then keeps its type.
    let mut level = Counting::default();
    let mut own = Random::of_entity(3, EntityId(9), 1);
    making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome: Some("minecraft:desert"),
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason: SpawnReason::Structure,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    assert_eq!(super::zombie_villager::villager(&mob).kind, types::SNOW);
}

#[test]
fn a_golden_apple_does_nothing_to_a_zombie_villager_that_is_not_weak() {
    let kind = entity_types::ZOMBIE_VILLAGER as u16;
    let mut mob = Mob::fresh();
    let mut draws = Random::of_entity(1, EntityId(2), 0);
    let mut used = super::Used {
        key: EntityId(2),
        kind,
        mob: &mut mob,
        player: EntityId(1),
        player_uuid: None,
        game_time: 0,
        held: Some(ItemStack::new(items::GOLDEN_APPLE, 1)),
        creative: true,
        sneaking: false,
        carries: false,
        passengers: 0,
        on_ground: true,
        from: [0.0; 3],
        position: [0.0; 3],
        moment: 0,
        draws: &mut draws,
        did: Vec::new(),
    };
    assert_eq!(of(kind).mob_interact(&mut used), Interaction::Consume);
    assert!(used.did.is_empty());
    used.held = Some(ItemStack::new(items::APPLE, 1));
    assert_eq!(of(kind).mob_interact(&mut used), Interaction::Pass);
}

#[test]
fn a_baby_zombie_villager_s_voice_is_about_two_and_others_about_one_and_a_half() {
    let mut baby = Mob::fresh();
    family::set_baby(entity_types::ZOMBIE_VILLAGER as u16, &mut baby, true);
    for (kind, base) in [
        (entity_types::ZOMBIE_VILLAGER, 2.0),
        (entity_types::ZOMBIE, 1.5),
        (entity_types::HUSK, 1.5),
    ] {
        let mob_type = of(kind as u16);
        let mut draws = Random::of_entity(1, EntityId(2), 0);
        let mut replay = draws.clone();
        let pitch = crate::mobs::body::hurt::mob_voice_pitch(mob_type, &baby, &mut draws);
        let spread = (replay.next_float() - replay.next_float()) * 0.2;
        assert_eq!(pitch, spread + base, "{kind}");
    }
}

#[test]
fn the_drowning_converts_on_the_901st_tick_under_water_and_601_once_it_has_left() {
    let mut tracker = Tracker::default();
    for tick in 1..=900 {
        assert!(!tracker.tick(true), "{tick}");
        assert_eq!(tracker.converting, tick >= 600, "{tick}");
    }
    assert!(tracker.tick(true));
    // Out of water it starts again, from -1.
    let mut tracker = Tracker::default();
    for _ in 0..700 {
        tracker.tick(true);
    }
    assert!(tracker.converting);
    tracker.tick(false);
    assert_eq!(tracker.affliction, -1);
    assert!(!tracker.converting);
    for tick in 1..=601 {
        assert!(!tracker.tick(true), "{tick}");
        assert_eq!(tracker.converting, tick >= 601, "{tick}");
    }
    for _ in 0..300 {
        assert!(!tracker.tick(true));
    }
    assert!(tracker.tick(true));
}

#[test]
fn a_zombie_shows_its_baby_special_type_and_conversion_at_16_to_18() {
    let kind = of(entity_types::HUSK as u16);
    let mut mob = Mob::fresh();
    assert_eq!(
        kind.shown(&mob),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Int(0), true),
            (18, DataShown::Boolean(false), true),
        ]
    );
    family::update(&mut mob, |own| own.drowning.converting = true);
    assert!(
        kind.shown(&mob)
            .contains(&(18, DataShown::Boolean(true), false))
    );
}

#[test]
fn a_zombie_from_a_template_reads_its_baby_doors_and_drowning() {
    let kind = entity_types::ZOMBIE as u16;
    let tag = nbt::compound(&[
        nbt::byte("IsBaby", 1),
        nbt::byte("CanBreakDoors", 1),
        nbt::int("InWaterTime", 120),
        nbt::int("DrownedConversionTime", 40),
    ]);
    let mut mob = Mob::fresh();
    super::from_template(kind, &mut mob, &tag);
    let own = family::read(&mob);
    assert!(own.baby && own.can_break_doors);
    assert_eq!(
        own.drowning,
        Tracker {
            affliction: 120,
            conversion: 40,
            converting: true,
        }
    );
}

#[test]
fn a_baby_zombie_is_worth_twelve_and_a_grown_one_five_and_more_for_what_it_holds() {
    let kind = of(entity_types::ZOMBIE as u16);
    let mut draws = Random::of_entity(1, EntityId(2), 0);
    assert_eq!(kind.experience_reward(0, &Mob::fresh(), &mut draws), 5);
    let mut baby = Mob::fresh();
    family::set_baby(entity_types::ZOMBIE as u16, &mut baby, true);
    assert_eq!(kind.experience_reward(0, &baby, &mut draws), 12);
    let mut armed = Mob::fresh();
    armed.body.equipment.slots[0] = Some(ItemStack::new(items::IRON_SWORD, 1));
    let mut replay = draws.clone();
    let reward = kind.experience_reward(0, &armed, &mut draws);
    assert_eq!(reward, 5 + 1 + replay.next_int(3) as i32);
}

#[test]
fn every_zombie_of_the_family_is_built_and_is_one() {
    for kind in [
        entity_types::ZOMBIE,
        entity_types::HUSK,
        entity_types::DROWNED,
        entity_types::ZOMBIE_VILLAGER,
    ] {
        assert!(super::is_built(kind as u16) && family::is_zombie(kind as u16));
    }
    assert!(!family::is_zombie(entity_types::SKELETON as u16));
}
