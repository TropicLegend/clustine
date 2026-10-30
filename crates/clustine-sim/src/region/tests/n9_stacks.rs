//! Item stacks with their components in a region, tested from the plan
//! (`docs/groundwork/n9-plan.md`, sections 3.1, 3.2 and 14) by someone who did not
//! write it: a stack with every component of the set kept whole through a hand-over, a
//! chest's NBT (the game's own form, which a chunk the game wrote is in), a hopper and
//! an item entity's merge; and stacks that differ only in their components never put
//! together, as the game's `ItemStack.isSameItemSameComponents` keeps them apart.
//!
//! The writers of the store's format and the wire are tested beside the end-to-end
//! tests (`bin/clustine/tests/n9_components.rs`).

use clustine_data::{entity_types, items};
use clustine_world::components::{
    BannerLayer, Component, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, Kind,
    PotionContents, Rarity, TooltipDisplay, UseCooldown,
};
use clustine_world::{Entity, EntityChange, EntityData, Float};

use super::n9_world::*;
use super::redstone::{state, ticking_with};
use super::*;
use crate::region::containers::{container_data, read_container};

/// A stack with one of each component of the set, its values as the creative tab's and
/// the loot's might be: what every writer has to keep.
pub(super) fn with_every_component(item: i32) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    let components = [
        Component::PotionContents(PotionContents {
            potion: Some("minecraft:strong_swiftness".to_owned()),
            custom_colour: Some(0x0012_3456),
            custom_effects: vec![EffectInstance {
                effect: "minecraft:poison".to_owned(),
                amplifier: 1,
                duration: 200,
                ambient: true,
                visible: false,
                show_icon: true,
            }],
            custom_name: Some("brew".to_owned()),
        }),
        Component::PotionDurationScale(Float(0.5)),
        Component::ChargedProjectiles(vec![
            ItemStack::new(items::ARROW, 1),
            ItemStack::new(items::FIREWORK_ROCKET, 1),
        ]),
        Component::Enchantments(vec![("minecraft:multishot".to_owned(), 1)]),
        Component::UseCooldown(UseCooldown {
            seconds: Float(2.5),
            group: Some("minecraft:test_group".to_owned()),
        }),
        Component::Fireworks(Fireworks {
            flight_duration: 2,
            explosions: vec![FireworkExplosion {
                shape: FireworkShape::Star,
                colours: vec![0x00FF_0000, 0x0000_00FF],
                fade_colours: vec![0x0000_FF00],
                trail: true,
                twinkle: false,
            }],
        }),
        Component::Damage(7),
        Component::IntangibleProjectile,
        Component::BannerPatterns(vec![BannerLayer {
            pattern: "minecraft:stripe_bottom".to_owned(),
            colour: "red".to_owned(),
        }]),
        Component::ItemName("\"Named\"".to_owned()),
        Component::Rarity(Rarity::Epic),
        Component::TooltipDisplay(TooltipDisplay {
            hide_tooltip: false,
            hidden_components: vec!["minecraft:damage".to_owned()],
        }),
        Component::SuspiciousStewEffects(vec![("minecraft:night_vision".to_owned(), 100)]),
        Component::OminousBottleAmplifier(3),
        Component::ChickenVariant("minecraft:cold".to_owned()),
        Component::StoredEnchantments(vec![("minecraft:mending".to_owned(), 1)]),
        Component::DyedColor(0x00B0_2E26),
    ];
    assert_eq!(components.len(), Kind::ALL.len(), "one of each kind");
    for component in components {
        stack.components.set(component);
    }
    stack
}

/// A crossbow whose patch removes its default charge: a removal to keep as well.
pub(super) fn with_a_removal() -> ItemStack {
    let mut stack = ItemStack::new(items::CROSSBOW, 1);
    stack.components.remove(Kind::ChargedProjectiles);
    stack
}

/// Arrows named `name`, `count` of them: a stack of 64 with a component.
fn named_arrows(name: &str, count: i32) -> ItemStack {
    let mut stack = ItemStack::new(items::ARROW, count);
    stack
        .components
        .set(Component::ItemName(format!("\"{name}\"")));
    stack
}

/// Section 3.1 and 5.4: the whole inventory goes with a player at a hand-over, every
/// stack with its components and its removals as they were.
#[test]
fn a_hand_over_carries_every_component_of_every_stack() {
    let mut west = west(10, clock_at(0));
    let mut east = east(10, clock_at(0));
    let mut inventory = Inventory::default();
    inventory.set(0, Some(with_every_component(items::POTION)));
    inventory.set(5, Some(with_a_removal()));
    inventory.set(20, Some(named_arrows("a", 17)));
    let place = Place {
        inventory: inventory.clone(),
        ..entering_place(Vec3::new(14.5, -60.0, 0.5), Vec::new(), 0)
    };
    enter_with(&mut west, 1, place);
    let output = acting(
        &mut west,
        1,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(17.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    let [(_, transfer)] = &departures(&output)[..] else {
        panic!("one departure: {:?}", output.durable);
    };
    assert_eq!(transfer.inventory, inventory);
    east.tick(&changes(vec![arrive(1, transfer)]));
    assert_eq!(east.players[&player(1)].inventory, inventory);
}

/// Section 3.2: a container's stacks are written in its block entity's NBT as the game's
/// `ItemStackWithSlot.CODEC` writes them, each component in its persistent form, and read
/// back as they were: a potion in a chest keeps its contents at a save.
#[test]
fn a_chest_s_nbt_keeps_every_component_and_removal_of_its_stacks() {
    let chest = state("minecraft:chest[facing=north,type=single,waterlogged=false]");
    let mut slots = vec![None; 27];
    slots[0] = Some(with_every_component(items::POTION));
    slots[1] = Some(with_a_removal());
    slots[26] = Some(named_arrows("b", 64));
    let container = crate::region::containers::Container {
        slots: slots.clone(),
        openers: 0,
        loot: false,
    };
    let data = container_data(chest, &container, &[]);
    assert_eq!(read_container(27, &data).slots, slots);
}

/// Section 3.1: two stacks are the same kind where item and components are equal
/// (`isSameItemSameComponents`), and `ItemEnchantments` is a map, equal whatever order
/// its entries came in; the plan keeps them "by the registry's order" so that equal is
/// equal field for field. A crossbow with two enchantments saved in a chest and read back
/// is the same kind as before.
#[test]
fn a_stack_with_two_enchantments_saved_in_a_chest_is_the_same_kind_read_back() {
    let chest = state("minecraft:chest[facing=north,type=single,waterlogged=false]");
    let mut crossbow = ItemStack::new(items::CROSSBOW, 1);
    crossbow.components.set(Component::Enchantments(vec![
        ("minecraft:multishot".to_owned(), 1),
        ("minecraft:unbreaking".to_owned(), 3),
    ]));
    let mut slots = vec![None; 27];
    slots[0] = Some(crossbow.clone());
    let container = crate::region::containers::Container {
        slots,
        openers: 0,
        loot: false,
    };
    let data = container_data(chest, &container, &[]);
    let read = read_container(27, &data).slots[0]
        .clone()
        .expect("the crossbow");
    assert!(read.same_kind(&crossbow), "{read:?}");
}

/// An NBT string: its length as an unsigned short and its bytes.
fn nbt_string(text: &str) -> Vec<u8> {
    let mut out = u16::try_from(text.len())
        .expect("short")
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(text.as_bytes());
    out
}

/// A named tag's head: its type and its name.
fn named(tag: u8, name: &str) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(nbt_string(name));
    out
}

/// Section 3.2: "a chunk written by the game reads". A chest's data as the game writes it
/// with `NbtOps` (`ItemStack.CODEC`: `id`, `count` and `components`; `PotionContents`'
/// full codec, a compound with its `potion`; `Enchantments` a compound of levels by
/// name), read into stacks with those components.
#[test]
fn a_chest_the_game_wrote_reads_with_its_stacks_components() {
    const COMPOUND: u8 = 10;
    const LIST: u8 = 9;
    const STRING: u8 = 8;
    const INT: u8 = 3;
    const BYTE: u8 = 1;
    const END: u8 = 0;
    let mut data = vec![COMPOUND];
    data.extend(named(LIST, "Items"));
    data.push(COMPOUND);
    data.extend(2_i32.to_be_bytes());
    // { Slot: 0b, id: "minecraft:potion", count: 1,
    //   components: { "minecraft:potion_contents": { potion: "minecraft:swiftness" } } }
    data.extend(named(BYTE, "Slot"));
    data.push(0);
    data.extend(named(STRING, "id"));
    data.extend(nbt_string("minecraft:potion"));
    data.extend(named(INT, "count"));
    data.extend(1_i32.to_be_bytes());
    data.extend(named(COMPOUND, "components"));
    data.extend(named(COMPOUND, "minecraft:potion_contents"));
    data.extend(named(STRING, "potion"));
    data.extend(nbt_string("minecraft:swiftness"));
    data.push(END);
    data.push(END);
    data.push(END);
    // { Slot: 4b, id: "minecraft:crossbow", count: 1,
    //   components: { "minecraft:enchantments": { "minecraft:multishot": 1 },
    //                 "minecraft:damage": 9 } }
    data.extend(named(BYTE, "Slot"));
    data.push(4);
    data.extend(named(STRING, "id"));
    data.extend(nbt_string("minecraft:crossbow"));
    data.extend(named(INT, "count"));
    data.extend(1_i32.to_be_bytes());
    data.extend(named(COMPOUND, "components"));
    data.extend(named(COMPOUND, "minecraft:enchantments"));
    data.extend(named(INT, "minecraft:multishot"));
    data.extend(1_i32.to_be_bytes());
    data.push(END);
    data.extend(named(INT, "minecraft:damage"));
    data.extend(9_i32.to_be_bytes());
    data.push(END);
    data.push(END);
    data.push(END);

    let read = read_container(27, &data).slots;
    let mut swiftness = ItemStack::new(items::POTION, 1);
    swiftness
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:swiftness".to_owned()),
            ..PotionContents::default()
        }));
    let mut crossbow = ItemStack::new(items::CROSSBOW, 1);
    crossbow.components.set(Component::Enchantments(vec![(
        "minecraft:multishot".to_owned(),
        1,
    )]));
    crossbow.components.set(Component::Damage(9));
    assert_eq!(read[0], Some(swiftness));
    assert_eq!(read[4], Some(crossbow));
    assert!(
        read.iter()
            .enumerate()
            .all(|(slot, stack)| { stack.is_none() || slot == 0 || slot == 4 })
    );
}

const HOPPER: BlockPos = BlockPos::new(5, -60, 5);
const ABOVE: BlockPos = BlockPos::new(5, -59, 5);
const FRONT: BlockPos = BlockPos::new(6, -60, 5);

/// A chest above a hopper that faces a chest east of it, `above` in the upper chest and
/// `in_hopper` in the hopper's first slot.
fn hopper_between_chests(above: ItemStack, in_hopper: Option<ItemStack>) -> Region {
    let chest = state("minecraft:chest[facing=north,type=single,waterlogged=false]");
    let hopper = state("minecraft:hopper[enabled=true,facing=east]");
    let mut region = ticking_with(&[(HOPPER, hopper), (ABOVE, chest), (FRONT, chest)]);
    let mut upper = region.container_at(ABOVE).expect("a chest");
    upper.slots[0] = Some(above);
    region.set_container(ABOVE, upper, &mut TickOutput::default());
    let mut own = region.container_at(HOPPER).expect("a hopper");
    own.slots[0] = in_hopper;
    region.set_container(HOPPER, own, &mut TickOutput::default());
    region
}

fn slots_of(region: &Region, at: BlockPos) -> Vec<Option<ItemStack>> {
    region.container_at(at).expect("a container").slots
}

/// Section 3.1: hoppers move stacks together only where item and components are equal.
/// A hopper holding plain arrows takes a named arrow into a slot of its own; one holding
/// arrows of that name tops them up; and what it pushes on keeps its components.
#[test]
fn a_hopper_moves_a_stack_with_its_components_and_puts_it_only_with_its_equal() {
    let mut region =
        hopper_between_chests(named_arrows("n", 10), Some(ItemStack::new(items::ARROW, 5)));
    // Its first move pushes one of what it holds on, then takes one from above.
    region.tick(&TickInputs::default());
    let held = slots_of(&region, HOPPER);
    assert_eq!(held[0], Some(ItemStack::new(items::ARROW, 4)));
    assert_eq!(held[1], Some(named_arrows("n", 1)), "a slot of its own");

    let mut region = hopper_between_chests(named_arrows("n", 10), Some(named_arrows("n", 5)));
    region.tick(&TickInputs::default());
    assert_eq!(slots_of(&region, HOPPER)[0], Some(named_arrows("n", 5)));
    assert_eq!(slots_of(&region, HOPPER)[1], None, "topped up");
    assert_eq!(slots_of(&region, FRONT)[0], Some(named_arrows("n", 1)));

    // On into the chest in front, whole.
    let mut region = hopper_between_chests(with_every_component(items::POTION), None);
    for _ in 0..10 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(
        slots_of(&region, FRONT)[0],
        Some(with_every_component(items::POTION))
    );
}

/// An item entity of `stack` lying still at `feet`, ready to merge and to be picked up.
fn lying(region: &mut Region, feet: [f64; 3], stack: ItemStack) -> EntityId {
    let chunk = ChunkPos::containing(feet[0], feet[2]);
    let key = region.take_entity_key(chunk).expect("a key");
    let entity = Entity {
        portal: Default::default(),
        key,
        kind: u16::try_from(entity_types::ITEM).expect("a type"),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 3),
        position: feet,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    };
    region
        .chunks
        .get_mut(&chunk)
        .expect("loaded")
        .apply_entity(&EntityChange::Whole(entity));
    region.entity_chunks.insert(key, chunk);
    key
}

/// The stacks of the item entities the region has.
fn lying_stacks(region: &Region) -> Vec<ItemStack> {
    let mut stacks: Vec<ItemStack> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, .. } => Some(stack.clone()),
            _ => None,
        })
        .collect();
    stacks.sort_by_key(|stack| stack.count);
    stacks
}

/// Section 3.1: item entities merge (`ItemEntity.tryToMerge`) only where item and
/// components are equal, and the merged stack keeps them.
#[test]
fn item_entities_merge_only_with_equal_components_and_keep_them() {
    let mut region = alone(0, clock_at(0));
    lying(&mut region, [5.3, -60.0, 5.3], named_arrows("m", 3));
    lying(&mut region, [5.6, -60.0, 5.6], named_arrows("m", 4));
    for _ in 0..60 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(lying_stacks(&region), [named_arrows("m", 7)]);

    let mut region = alone(0, clock_at(0));
    lying(&mut region, [5.3, -60.0, 5.3], named_arrows("m", 3));
    lying(&mut region, [5.6, -60.0, 5.6], named_arrows("other", 4));
    lying(
        &mut region,
        [5.5, -60.0, 5.4],
        ItemStack::new(items::ARROW, 5),
    );
    for _ in 0..60 {
        region.tick(&TickInputs::default());
    }
    assert_eq!(
        lying_stacks(&region),
        [
            named_arrows("m", 3),
            named_arrows("other", 4),
            ItemStack::new(items::ARROW, 5)
        ]
    );
}

/// Section 3.1: a player picks an item up into a slot with an equal stack, components
/// and all (`Inventory.add`), and into a slot of its own where the components differ.
#[test]
fn a_player_picks_up_into_a_stack_only_of_equal_components() {
    let mut region = alone(0, clock_at(0));
    let mut inventory = Inventory::default();
    inventory.set(0, Some(named_arrows("p", 5)));
    let place = Place {
        inventory,
        ..entering_place(Vec3::new(0.5, -60.0, 0.5), Vec::new(), 0)
    };
    enter_with(&mut region, 1, place);
    lying(&mut region, [0.5, -60.0, 0.5], named_arrows("p", 3));
    lying(&mut region, [0.6, -60.0, 0.6], named_arrows("q", 2));
    for _ in 0..5 {
        region.tick(&TickInputs::default());
    }
    let carried = &region.players[&player(1)].inventory.0;
    assert_eq!(carried[0], Some(named_arrows("p", 8)));
    assert_eq!(carried[1], Some(named_arrows("q", 2)));
    assert!(lying_stacks(&region).is_empty());
}
