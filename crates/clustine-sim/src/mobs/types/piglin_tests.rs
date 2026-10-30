//! The piglin's and the brute's own rules from the game's code (`Piglin`, `PiglinAi`,
//! `PiglinBrute`, the barter table's file; family E5 of `docs/groundwork/p-plan.md`,
//! section 18): the barter table's outcomes, an egg's draws and what it makes, the
//! bastion's piglins and brutes from their templates, what a piglin wants of an item
//! and which items it keeps in its hands, its spawn rule and its shown data.

use std::collections::VecDeque;

use clustine_data::{blocks, entity_types, items};
use clustine_world::components::{Component, Kind};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::piglin::{self, PIGLIN, PIGLIN_BRUTE};
use super::piglin_barter::{self, ENTRIES, Function};
use super::template::build;
use super::{MobType, SpawnReason, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::brain::{self, MemoryKind, Value};
use crate::region::Random;
use crate::shown::DataShown;

/// One call of the level's random.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// The level's random of a test: every call recorded, each float answered from its
/// script where one is given, else a half; an int is its least.
#[derive(Default)]
struct Scripted {
    calls: Vec<Call>,
    floats: VecDeque<f32>,
}

impl clustine_world::generation::Draws for Scripted {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        0
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
        panic!("no booleans are drawn");
    }
}

/// `Monster.finalizeSpawn`'s tail: the follow range's triangle and the hand.
const TAIL: [Call; 3] = [Double, Double, Float];

/// A mob of `kind` made for `reason` with `level`, its template's `tag` read first where
/// given, as a structure's maker reads one.
fn made(kind: u16, level: &mut Scripted, reason: SpawnReason, tag: Option<&[u8]>) -> Mob {
    let mut own = Random::of_entity(7, EntityId(40), 0);
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    if let Some(tag) = tag {
        super::from_template(kind, &mut mob, tag);
    }
    let mut group = None;
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome: Some("minecraft:nether_wastes"),
            at: BlockPos::new(3, 40, 3),
            day: 0,
            game_time: 0,
            reason,
            group: &mut group,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    mob
}

fn main_hand(mob: &Mob) -> Option<i32> {
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
        .as_ref()
        .map(|stack| stack.item)
}

// ------------------------------------------------------------------------------------
// The barter table.

#[test]
fn the_barter_table_weighs_its_nineteen_entries_to_469() {
    assert_eq!(ENTRIES.len(), 19);
    assert_eq!(piglin_barter::total_weight(), 469);
    // The first and last of each entry's share of the roll.
    let mut from = 0;
    for entry in &ENTRIES {
        assert_eq!(piglin_barter::chosen(from), entry);
        assert_eq!(piglin_barter::chosen(from + entry.weight - 1), entry);
        from += entry.weight;
    }
    assert_eq!(from, 469);
}

#[test]
fn every_entry_of_the_barter_table_gives_its_item_within_its_counts() {
    for (index, entry) in ENTRIES.iter().enumerate() {
        for seed in 0..64 {
            let mut draws = Random::of_entity(seed, EntityId(index as u64), 0);
            let stack = piglin_barter::make(entry, &mut draws);
            match entry.function {
                Function::One => assert_eq!((stack.item, stack.count), (entry.item, 1)),
                Function::Count { min, max } => {
                    assert_eq!(stack.item, entry.item);
                    assert!((min..=max).contains(&stack.count), "{stack:?}");
                }
                Function::Potion(potion) => {
                    assert_eq!((stack.item, stack.count), (entry.item, 1));
                    let Some(Component::PotionContents(contents)) =
                        stack.components.get(Kind::PotionContents)
                    else {
                        panic!("a potion's contents: {stack:?}");
                    };
                    assert_eq!(contents.potion.as_deref(), Some(potion));
                }
                Function::SoulSpeed => {
                    let (item, kind) = if entry.item == items::BOOK {
                        (items::ENCHANTED_BOOK, Kind::StoredEnchantments)
                    } else {
                        (items::IRON_BOOTS, Kind::Enchantments)
                    };
                    assert_eq!(stack.item, item);
                    let list = match stack.components.get(kind) {
                        Some(
                            Component::StoredEnchantments(list) | Component::Enchantments(list),
                        ) => list.clone(),
                        other => panic!("{other:?}"),
                    };
                    assert_eq!(list.len(), 1);
                    assert_eq!(list[0].0, "minecraft:soul_speed");
                    assert!((1..=3).contains(&list[0].1), "{list:?}");
                }
            }
        }
    }
}

#[test]
fn a_barter_gives_one_stack_and_over_many_rolls_each_entry_comes() {
    let mut seen = [0_u32; 19];
    for seed in 0..4_000 {
        let mut draws = Random::of_entity(seed, EntityId(1), 0);
        let given = piglin_barter::barter(&mut draws);
        assert_eq!(given.len(), 1);
        let item = given[0].item;
        let potion = match given[0].components.get(Kind::PotionContents) {
            Some(Component::PotionContents(contents)) => contents.potion.clone(),
            _ => None,
        };
        let at = ENTRIES
            .iter()
            .position(|entry| match entry.function {
                Function::SoulSpeed if entry.item == items::BOOK => item == items::ENCHANTED_BOOK,
                Function::Potion(named) => entry.item == item && potion.as_deref() == Some(named),
                _ => entry.item == item,
            })
            .expect("of the table");
        seen[at] += 1;
    }
    assert!(seen.iter().all(|&count| count > 0), "{seen:?}");
    // Obsidian (40 of 469) far more often than an enchanted book (5).
    let obsidian = ENTRIES
        .iter()
        .position(|entry| entry.item == items::OBSIDIAN)
        .expect("obsidian");
    assert!(seen[obsidian] > 4 * seen[0], "{seen:?}");
}

// ------------------------------------------------------------------------------------
// Making.

/// Piglin.txt 161–208: an egg's adult (the level's float at a half is no baby) holds its
/// weapon of its own random; its hunt's memory is drawn (30 to 120 seconds, an int of
/// 1801); each golden armour piece one time in ten (four floats), the hand's
/// enchantment's float, then `Mob`'s tail.
#[test]
fn an_egg_s_adult_piglin_draws_a_baby_its_hunt_its_armour_and_its_weapon_s_enchantment() {
    let mut level = Scripted::default();
    let mob = made(PIGLIN, &mut level, SpawnReason::SpawnEgg, None);
    assert!(!piglin::is_baby(&mob));
    assert!(matches!(
        main_hand(&mob),
        Some(items::CROSSBOW | items::GOLDEN_SWORD | items::GOLDEN_SPEAR)
    ));
    let mut expected = vec![Float, Int(1801), Float, Float, Float, Float, Float];
    expected.extend(TAIL);
    assert_eq!(level.calls, expected);
    // Hunted recently, for the 600 ticks the least of the draw gives.
    let memories = brain::memories_of(&super::piglin_ai::PROVIDER, &mob, 0);
    assert!(memories.check(MemoryKind::HuntedRecently, brain::Status::Present));
    assert_eq!(memories.expires_in(MemoryKind::HuntedRecently), 600);
}

/// A float under a fifth makes a baby, which holds no weapon and wears no armour; its
/// speed is a fifth more and its size `0.49 × 0.98`.
#[test]
fn an_egg_s_baby_piglin_holds_nothing_and_is_small_and_quick() {
    let mut level = Scripted {
        floats: VecDeque::from([0.1]),
        ..Scripted::default()
    };
    let mob = made(PIGLIN, &mut level, SpawnReason::SpawnEgg, None);
    assert!(piglin::is_baby(&mob));
    assert_eq!(main_hand(&mob), None);
    let mut expected = vec![Float, Int(1801)];
    expected.extend(TAIL);
    assert_eq!(level.calls, expected);
    let size = piglin::Piglin.dimensions(&mob).expect("a baby's size");
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.49, 0.98, 0.78)
    );
    let speed = mob.body.attributes.value(
        PIGLIN,
        crate::mobs::body::attribute(clustine_data::attributes::MOVEMENT_SPEED),
    );
    assert!((speed - 0.35 * 1.2).abs() < 1e-6, "{speed}");
}

/// Under a tenth, a golden piece is worn: the four floats are the head's, the chest's,
/// the legs' and the feet's.
#[test]
fn golden_armour_is_worn_where_its_float_is_under_a_tenth() {
    let mut level = Scripted {
        floats: VecDeque::from([0.5, 0.05, 0.5, 0.05, 0.5]),
        ..Scripted::default()
    };
    let mob = made(PIGLIN, &mut level, SpawnReason::SpawnEgg, None);
    let worn = |slot: u8| {
        mob.body.equipment.slots[usize::from(slot)]
            .as_ref()
            .map(|stack| stack.item)
    };
    assert_eq!(worn(slots::HEAD), Some(items::GOLDEN_HELMET));
    assert_eq!(worn(slots::CHEST), None);
    assert_eq!(worn(slots::LEGS), Some(items::GOLDEN_LEGGINGS));
    assert_eq!(worn(slots::FEET), None);
}

/// The bastion's crossbow piglin (`bastion/mobs/crossbow_piglin`): its template's
/// crossbow and `CannotHunt`; for a structure, neither a baby nor a weapon is drawn.
#[test]
fn a_bastion_s_piglin_keeps_its_template_s_crossbow_and_cannot_hunt() {
    let tag = build::compound(&[
        build::byte("CannotHunt", 1),
        build::byte("PersistenceRequired", 1),
        build::float("Health", 16.0),
        build::int("TimeInOverworld", 0),
        build::nested(
            "equipment",
            &[build::nested(
                "mainhand",
                &[
                    build::int("count", 1),
                    build::string("id", "minecraft:crossbow"),
                ],
            )],
        ),
    ]);
    let mut level = Scripted::default();
    let mob = made(PIGLIN, &mut level, SpawnReason::Structure, Some(&tag));
    assert!(super::is_built(PIGLIN) && super::is_built(PIGLIN_BRUTE));
    assert_eq!(main_hand(&mob), Some(items::CROSSBOW));
    assert!(piglin::read(&mob).cannot_hunt);
    assert!(mob.body.flags.persistent);
    let mut expected = vec![Int(1801), Float, Float, Float, Float, Float];
    expected.extend(TAIL);
    assert_eq!(level.calls, expected);
}

/// The bastion's brute (`bastion/mobs/melee_piglin`): a golden axe, and its home where
/// it is made.
#[test]
fn a_brute_holds_a_golden_axe_and_remembers_its_home() {
    let tag = build::compound(&[
        build::byte("PersistenceRequired", 1),
        build::float("Health", 50.0),
    ]);
    let mut level = Scripted::default();
    let mob = made(PIGLIN_BRUTE, &mut level, SpawnReason::Structure, Some(&tag));
    assert_eq!(main_hand(&mob), Some(items::GOLDEN_AXE));
    let memories = brain::memories_of(&super::piglin_brute::PROVIDER, &mob, 0);
    assert_eq!(
        memories.get(MemoryKind::Home),
        Some(Value::Block(BlockPos::new(3, 40, 3)))
    );
    assert_eq!(level.calls, TAIL.to_vec());
}

// ------------------------------------------------------------------------------------
// What it wants and keeps.

fn adult() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.flags.picks_up_loot = true;
    mob
}

fn stack(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

fn wants(mob: &Mob, memories: &brain::Memories, item: i32) -> bool {
    super::piglin_ai::wants_to_pick_up(PIGLIN, mob, memories, &stack(item))
}

/// PiglinAi 812–875: a gold ingot while it holds no loved item in its off hand; nuggets,
/// food (not lately eaten) and loved items where its inventory has room; never a
/// repellent; a baby none of `#ignored_by_piglin_babies`; else what it may hold instead
/// of what it holds.
#[test]
fn a_piglin_wants_gold_food_and_loved_items_but_no_repellent() {
    let mut mob = adult();
    let mut memories = brain::memories_of(&super::piglin_ai::PROVIDER, &mob, 0);
    assert!(wants(&mob, &memories, items::GOLD_INGOT));
    assert!(wants(&mob, &memories, items::GOLD_NUGGET));
    assert!(wants(&mob, &memories, items::PORKCHOP));
    assert!(wants(&mob, &memories, items::GOLDEN_HELMET));
    assert!(!wants(&mob, &memories, items::SOUL_TORCH), "a repellent");
    memories.set_with_expiry(MemoryKind::AteRecently, Value::Bool(true), 200);
    assert!(!wants(&mob, &memories, items::PORKCHOP), "lately eaten");
    piglin::hold_in_off_hand(&mut mob, stack(items::GOLD_INGOT));
    assert!(!wants(&mob, &memories, items::GOLD_INGOT), "it admires one");
    assert!(
        !wants(&mob, &memories, items::GOLD_BLOCK),
        "a loved item in its off hand"
    );
    // A baby ignores `#ignored_by_piglin_babies` (leather), which an adult with an empty
    // hand would hold.
    let mut baby = adult();
    piglin::set_baby(&mut baby, true);
    assert!(!wants(&baby, &memories, items::LEATHER));
    assert!(wants(&adult(), &memories, items::LEATHER));
    // Admiring disabled with a target: nothing.
    let mut fighting = brain::memories_of(&super::piglin_ai::PROVIDER, &adult(), 0);
    fighting.set_with_expiry(MemoryKind::AdmiringDisabled, Value::Bool(true), 400);
    fighting.set(MemoryKind::AttackTarget, Value::Entity(EntityId(9)));
    assert!(!wants(&adult(), &fighting, items::GOLD_INGOT));
    // Nobody picking up loot wants anything.
    let mut idle = adult();
    idle.body.flags.picks_up_loot = false;
    assert!(!wants(&idle, &memories, items::GOLD_INGOT));
}

/// Piglin.txt 474–502: a golden sword (loved) is kept against an iron sword (neither
/// loved nor preferred), and a crossbow (preferred) taken over a stone sword; a baby
/// prefers no weapon.
#[test]
fn a_piglin_keeps_a_loved_or_preferred_weapon_over_one_that_is_neither() {
    let mut mob = adult();
    let hand = Some(&ItemStack::new(items::GOLDEN_SWORD, 1));
    assert!(!piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::IRON_SWORD),
        hand,
        slots::MAIN_HAND
    ));
    let stone = Some(&ItemStack::new(items::STONE_SWORD, 1));
    assert!(piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::CROSSBOW),
        stone,
        slots::MAIN_HAND
    ));
    // An empty hand takes anything.
    assert!(piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::STICK),
        None,
        slots::MAIN_HAND
    ));
    // Two weapons that are neither: the greater damage.
    assert!(piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::IRON_SWORD),
        stone,
        slots::MAIN_HAND
    ));
    assert!(!piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::STONE_SWORD),
        Some(&stack(items::IRON_SWORD)),
        slots::MAIN_HAND
    ));
    piglin::set_baby(&mut mob, true);
    assert!(!piglin::can_replace(
        PIGLIN,
        &mob,
        &stack(items::CROSSBOW),
        stone,
        slots::MAIN_HAND
    ));
}

/// `equipItemIfPossible`: armour better than what is worn replaces it, what was worn
/// falling where `max(nextFloat() − 0.1, 0)` is under its chance; worn for certain after,
/// and the piglin kept.
#[test]
fn better_armour_is_worn_and_the_piece_it_replaces_falls_by_its_chance() {
    let mut mob = adult();
    mob.body.equipment.slots[usize::from(slots::HEAD)] = Some(stack(items::LEATHER_HELMET));
    mob.body.equipment.drop_chances = Some([2.0; 8]);
    let mut own = Random::of_entity(1, EntityId(1), 0);
    let equipped =
        piglin::equip_if_possible(PIGLIN, &mut mob, &stack(items::IRON_HELMET), &mut own);
    assert_eq!(
        equipped.worn.map(|stack| stack.item),
        Some(items::IRON_HELMET)
    );
    assert_eq!(
        equipped.dropped.map(|stack| stack.item),
        Some(items::LEATHER_HELMET)
    );
    assert!(mob.body.flags.persistent);
    assert_eq!(
        mob.body.equipment.drop_chance(usize::from(slots::HEAD)),
        2.0
    );
    // Worse armour is not worn, nor held in a hand that holds something.
    mob.body.equipment.slots[0] = Some(stack(items::GOLDEN_SWORD));
    let refused =
        piglin::equip_if_possible(PIGLIN, &mut mob, &stack(items::LEATHER_HELMET), &mut own);
    assert!(refused.worn.is_none() && refused.dropped.is_none());
}

/// `SimpleContainer.addItem`: onto a stack of its kind first, then an empty slot of
/// eight; what does not fit is given back.
#[test]
fn the_inventory_of_eight_stacks_like_items_and_gives_back_what_does_not_fit() {
    let mut own = piglin::PiglinState::default();
    assert_eq!(
        piglin::add_to_inventory(&mut own, ItemStack::new(items::GOLD_NUGGET, 60)),
        None
    );
    assert_eq!(
        piglin::add_to_inventory(&mut own, ItemStack::new(items::GOLD_NUGGET, 10)),
        None
    );
    assert_eq!(
        own.inventory,
        vec![
            ItemStack::new(items::GOLD_NUGGET, 64),
            ItemStack::new(items::GOLD_NUGGET, 6)
        ]
    );
    for _ in 0..6 {
        assert_eq!(
            piglin::add_to_inventory(&mut own, stack(items::LEATHER_HELMET)),
            None
        );
    }
    assert!(!piglin::can_add_to_inventory(
        &own,
        &stack(items::IRON_SWORD)
    ));
    assert!(piglin::can_add_to_inventory(
        &own,
        &stack(items::GOLD_NUGGET)
    ));
    assert_eq!(
        piglin::add_to_inventory(&mut own, ItemStack::new(items::GOLD_NUGGET, 70)),
        Some(ItemStack::new(items::GOLD_NUGGET, 12))
    );
}

// ------------------------------------------------------------------------------------
// Its spawn and its data.

/// A world of one block below the spawn's place.
struct Below(clustine_data::BlockState);

impl crate::spawning::rules::SpawnWorld for Below {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        Some(if pos.y == 39 { self.0 } else { blocks::AIR })
    }

    fn light(&self, _: BlockPos) -> Option<crate::spawning::rules::Light> {
        None
    }

    fn sky_darken(&self) -> i32 {
        0
    }

    fn biome(&self, _: BlockPos) -> Option<clustine_world::Biome> {
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

/// Piglin.txt 150–160: anywhere but above a nether wart block.
#[test]
fn a_piglin_spawns_anywhere_but_above_a_nether_wart_block() {
    let at = BlockPos::new(0, 40, 0);
    let mut draws = Scripted::default();
    assert!(piglin::check_spawn_rules(
        &Below(blocks::NETHERRACK),
        at,
        &mut draws
    ));
    assert!(piglin::check_spawn_rules(
        &Below(blocks::CRIMSON_NYLIUM),
        at,
        &mut draws
    ));
    assert!(!piglin::check_spawn_rules(
        &Below(blocks::NETHER_WART_BLOCK),
        at,
        &mut draws
    ));
    assert!(draws.calls.is_empty());
}

/// Its synced data: `IsImmuneToZombification` at 16, the baby at 17, the charging
/// crossbow at 18, the dance at 19; a brute's only the first.
#[test]
fn a_piglin_shows_its_immunity_baby_charge_and_dance_and_a_brute_its_immunity() {
    let mut mob = Mob::fresh();
    piglin::update(&mut mob, |own| {
        own.immune = true;
        own.charging = true;
    });
    assert_eq!(
        piglin::Piglin.shown(&mob),
        vec![
            (16, DataShown::Boolean(true), false),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Boolean(true), false),
            (19, DataShown::Boolean(false), true),
        ]
    );
    assert_eq!(
        super::piglin_brute::PiglinBrute.shown(&mob),
        vec![(16, DataShown::Boolean(true), false)]
    );
    let _ = entity_types::PIGLIN;
}
