//! The evaluator against the 26.3 tables, with results worked out by hand from the
//! tables' files (`generated/data/minecraft/loot_table/blocks/`).

use std::collections::VecDeque;

use clustine_data::loot::{IntProvider, LootEntry, LootModifier, LootPool, LootTable};
use clustine_data::{BLOCK_STATE_COUNT, BlockState, items};
use clustine_world::components::Component;
use clustine_world::{BlockPos, ItemStack};

use super::{Broken, LootDraws, block_drops, block_drops_as, table_drops};

/// A draw the test gives, and the kind the evaluator has to ask for there.
#[derive(Clone, Copy, Debug)]
enum Draw {
    Float(f32),
    /// The answer, and the bound the evaluator has to ask with.
    Int {
        bound: i32,
        value: i32,
    },
}

/// Draws given in order, which fails a test where the evaluator asks for another
/// kind, another bound or more than were given.
struct Scripted(VecDeque<Draw>);

impl Scripted {
    fn new(draws: &[Draw]) -> Self {
        Scripted(draws.iter().copied().collect())
    }

    fn assert_used_up(&self) {
        assert!(self.0.is_empty(), "draws left over: {:?}", self.0);
    }
}

impl LootDraws for Scripted {
    fn next_float(&mut self) -> f32 {
        match self.0.pop_front() {
            Some(Draw::Float(value)) => value,
            other => panic!("asked for a float, given {other:?}"),
        }
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        match self.0.pop_front() {
            Some(Draw::Int {
                bound: wanted,
                value,
            }) => {
                assert_eq!(bound, wanted, "the bound of an int draw");
                value
            }
            other => panic!("asked for an int below {bound}, given {other:?}"),
        }
    }
}

/// `java.util.Random`, for draws in bulk.
struct JavaRandom(i64);

impl JavaRandom {
    fn new(seed: i64) -> Self {
        JavaRandom((seed ^ 0x5DEE_CE66D) & ((1 << 48) - 1))
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(0x5DEE_CE66D).wrapping_add(0xB)) & ((1 << 48) - 1);
        (self.0 >> (48 - bits)) as i32
    }
}

impl LootDraws for JavaRandom {
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1 << 24) as f32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        assert!(bound > 0);
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }
}

const AT: BlockPos = BlockPos::new(10, 64, -3);

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("no state {text}"))
}

fn air(_: BlockPos) -> BlockState {
    BlockState(0)
}

/// What `text` drops with these draws, all of which it has to take.
fn drops(text: &str, draws: &[Draw]) -> Vec<ItemStack> {
    let mut draws = Scripted::new(draws);
    let dropped = block_drops(state(text), AT, &mut draws, &mut air);
    draws.assert_used_up();
    dropped
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

#[test]
fn stone_drops_cobblestone_without_a_draw() {
    assert_eq!(
        drops("minecraft:stone", &[]),
        [stack(items::COBBLESTONE, 1)]
    );
}

#[test]
fn a_torch_and_a_wall_torch_drop_a_torch() {
    assert_eq!(drops("minecraft:torch", &[]), [stack(items::TORCH, 1)]);
    assert_eq!(
        drops("minecraft:wall_torch[facing=east]", &[]),
        [stack(items::TORCH, 1)]
    );
}

/// The three pools in order: a sapling below 0.05, sticks below 0.02 (then one or two
/// by `nextInt(2) + 1`), an apple below 0.005; shears and silk touch never apply.
#[test]
fn oak_leaves_broken_by_a_rule_drop_by_the_draws_given() {
    let leaves = "minecraft:oak_leaves[distance=7,persistent=false]";
    assert_eq!(
        drops(
            leaves,
            &[
                Draw::Float(0.049),
                Draw::Float(0.019),
                Draw::Int { bound: 2, value: 1 },
                Draw::Float(0.0049),
            ]
        ),
        [
            stack(items::OAK_SAPLING, 1),
            stack(items::STICK, 2),
            stack(items::APPLE, 1)
        ]
    );
    // The chances are strict: a draw at the chance fails, and a failed stick takes no
    // draw for its count.
    assert_eq!(
        drops(
            leaves,
            &[Draw::Float(0.05), Draw::Float(0.02), Draw::Float(0.005)]
        ),
        []
    );
    assert_eq!(
        drops(
            leaves,
            &[
                Draw::Float(0.9),
                Draw::Float(0.0),
                Draw::Int { bound: 2, value: 0 },
                Draw::Float(0.9),
            ]
        ),
        [stack(items::STICK, 1)]
    );
}

#[test]
fn oak_leaves_drop_at_the_tables_rates_over_many_breaks() {
    const BREAKS: u32 = 200_000;
    let leaves = state("minecraft:oak_leaves");
    let mut random = JavaRandom::new(4711);
    let (mut saplings, mut stick_drops, mut sticks, mut apples) = (0u32, 0u32, 0u32, 0u32);
    for _ in 0..BREAKS {
        for dropped in block_drops(leaves, AT, &mut random, &mut air) {
            match dropped.item {
                items::OAK_SAPLING => saplings += 1,
                items::STICK => {
                    assert!((1..=2).contains(&dropped.count));
                    stick_drops += 1;
                    sticks += u32::try_from(dropped.count).unwrap();
                }
                items::APPLE => apples += 1,
                other => panic!("oak leaves dropped item {other}"),
            }
        }
    }
    // Within five standard deviations of the binomial's mean.
    let within = |count: u32, p: f64| {
        let mean = f64::from(BREAKS) * p;
        let deviation = (mean * (1.0 - p)).sqrt();
        (f64::from(count) - mean).abs() < 5.0 * deviation
    };
    assert!(within(saplings, 0.05), "{saplings} saplings");
    assert!(within(stick_drops, 0.02), "{stick_drops} drops of sticks");
    assert!(within(apples, 0.005), "{apples} apples");
    // One or two sticks evenly: one and a half a drop.
    let per_drop = f64::from(sticks) / f64::from(stick_drops);
    assert!((per_drop - 1.5).abs() < 0.05, "{per_drop} sticks a drop");
}

#[test]
fn a_double_slab_drops_two_and_a_single_one_one() {
    assert_eq!(
        drops("minecraft:oak_slab[type=double]", &[]),
        [stack(items::OAK_SLAB, 2)]
    );
    assert_eq!(
        drops("minecraft:oak_slab[type=top]", &[]),
        [stack(items::OAK_SLAB, 1)]
    );
}

#[test]
fn a_beds_foot_drops_nothing_and_its_head_a_bed() {
    assert_eq!(drops("minecraft:red_bed[part=foot]", &[]), []);
    assert_eq!(
        drops("minecraft:red_bed[part=head]", &[]),
        [stack(items::RED_BED, 1)]
    );
}

#[test]
fn gravel_drops_flint_below_a_tenth_and_else_itself() {
    assert_eq!(
        drops("minecraft:gravel", &[Draw::Float(0.099)]),
        [stack(items::FLINT, 1)]
    );
    assert_eq!(
        drops("minecraft:gravel", &[Draw::Float(0.1)]),
        [stack(items::GRAVEL, 1)]
    );
}

#[test]
fn glass_drops_nothing_without_silk_touch() {
    assert_eq!(drops("minecraft:glass", &[]), []);
}

#[test]
fn a_block_without_a_table_or_without_pools_drops_nothing() {
    for block in [
        "minecraft:air",
        "minecraft:water",
        "minecraft:fire",
        "minecraft:frosted_ice",
    ] {
        assert_eq!(drops(block, &[]), [], "{block}");
    }
    // An id that is no block state.
    let mut draws = Scripted::new(&[]);
    let beyond = BlockState(u16::try_from(BLOCK_STATE_COUNT).unwrap());
    assert_eq!(block_drops(beyond, AT, &mut draws, &mut air), []);
}

/// Ripe wheat: the wheat, then seeds by `binomial_with_bonus_count` (extra 3, p
/// 0.5714286) at level 0, three draws on a count of one; unripe, one seed and no
/// draw.
#[test]
fn ripe_wheat_draws_three_times_for_its_seeds() {
    assert_eq!(
        drops(
            "minecraft:wheat[age=7]",
            &[Draw::Float(0.5), Draw::Float(0.6), Draw::Float(0.1)]
        ),
        [stack(items::WHEAT, 1), stack(items::WHEAT_SEEDS, 3)]
    );
    assert_eq!(
        drops("minecraft:wheat[age=3]", &[]),
        [stack(items::WHEAT_SEEDS, 1)]
    );
}

/// The lower half of tall grass drops seeds by a chance of 0.125 only with its upper
/// half above it (a `location_check`); the upper half drops nothing.
#[test]
fn tall_grass_reads_the_half_above_it() {
    let lower = state("minecraft:tall_grass[half=lower]");
    let upper = state("minecraft:tall_grass[half=upper]");
    let mut above = |at: BlockPos| {
        if at == BlockPos::new(AT.x, AT.y + 1, AT.z) {
            upper
        } else {
            BlockState(0)
        }
    };
    let mut draws = Scripted::new(&[Draw::Float(0.12)]);
    assert_eq!(
        block_drops(lower, AT, &mut draws, &mut above),
        [stack(items::WHEAT_SEEDS, 1)]
    );
    draws.assert_used_up();

    let mut draws = Scripted::new(&[Draw::Float(0.125)]);
    assert_eq!(block_drops(lower, AT, &mut draws, &mut above), []);
    draws.assert_used_up();

    // Nothing above: the pool's condition fails before its chance is drawn.
    let mut draws = Scripted::new(&[]);
    assert_eq!(block_drops(lower, AT, &mut draws, &mut air), []);
    assert_eq!(block_drops(upper, AT, &mut draws, &mut above), []);
}

/// A brown mushroom block: `uniform` from -6 to 2 (`nextInt(9) - 6`), limited to at
/// least nought. Nought is an empty stack, which stays in the list, as the game's
/// `popResource` draws its position before it finds it empty.
#[test]
fn a_mushroom_block_keeps_an_empty_stack_in_its_drops() {
    let block = "minecraft:brown_mushroom_block";
    assert_eq!(
        drops(block, &[Draw::Int { bound: 9, value: 0 }]),
        [stack(items::BROWN_MUSHROOM, 0)]
    );
    assert_eq!(
        drops(block, &[Draw::Int { bound: 9, value: 8 }]),
        [stack(items::BROWN_MUSHROOM, 2)]
    );
}

#[test]
fn a_chorus_flower_broken_by_a_rule_drops_nothing() {
    assert_eq!(drops("minecraft:chorus_flower", &[]), []);
    assert_eq!(drops("minecraft:snow[layers=3]", &[]), []);
}

/// What needs a block entity or components drops the plain item.
#[test]
fn containers_and_hives_drop_the_plain_item() {
    assert_eq!(
        drops("minecraft:shulker_box", &[]),
        [stack(items::SHULKER_BOX, 1)]
    );
    assert_eq!(
        drops("minecraft:beehive[honey_level=5]", &[]),
        [stack(items::BEEHIVE, 1)]
    );
    assert_eq!(
        drops("minecraft:decorated_pot", &[]),
        [stack(items::DECORATED_POT, 1)]
    );
    // A cracked pot drops its sherds, which only its block entity knows.
    assert_eq!(drops("minecraft:decorated_pot[cracked=true]", &[]), []);
}

/// The stack splitter: a stack at its item's size or above goes out in full stacks
/// and the rest. A bed stacks to one, so a set count of four gives four beds.
#[test]
fn a_stack_above_its_size_is_split_into_full_ones() {
    let four = (0u16..)
        .find(|&number| {
            clustine_data::loot::modifier(number)
                == &LootModifier::SetCount {
                    count: IntProvider::Constant(4),
                    add: false,
                    condition: None,
                }
        })
        .expect("some table sets a count of four");
    let modifiers: &'static [u16] = Box::leak(Box::new([four]));
    let entries: &'static [LootEntry] = Box::leak(Box::new([LootEntry::Item {
        item: u16::try_from(items::RED_BED).unwrap(),
        weight: 1,
        condition: None,
        modifiers,
    }]));
    let pools: &'static [LootPool] = Box::leak(Box::new([LootPool {
        rolls: IntProvider::Constant(1),
        condition: None,
        entries,
        modifiers: &[],
    }]));
    let table: &'static LootTable = Box::leak(Box::new(LootTable {
        name: "test:four_beds",
        pools,
        modifiers: &[],
    }));
    let mut draws = Scripted::new(&[]);
    assert_eq!(
        table_drops(table, BlockState(0), AT, &mut draws, &mut air),
        vec![stack(items::RED_BED, 1); 4]
    );
}

/// Every state of every block, with a draw source that answers anything: no table
/// asks for what the evaluator cannot answer, and no count comes out negative.
#[test]
fn every_block_state_evaluates() {
    let mut random = JavaRandom::new(1);
    let mut grass_above = |_: BlockPos| state("minecraft:tall_grass[half=upper]");
    for id in 0..BLOCK_STATE_COUNT {
        let block = BlockState(u16::try_from(id).unwrap());
        for dropped in block_drops(block, AT, &mut random, &mut grass_above) {
            assert!(dropped.count >= 0, "{block:?}");
            assert!(usize::try_from(dropped.item).unwrap() < clustine_data::ITEMS.len());
        }
    }
}

/// What `text` drops broken as `broken` says, with these draws, all of which it has to
/// take.
fn drops_as(text: &str, broken: Broken<'_>, draws: &[Draw]) -> Vec<ItemStack> {
    let mut draws = Scripted::new(draws);
    let dropped = block_drops_as(state(text), AT, broken, &mut draws, &mut air);
    draws.assert_used_up();
    dropped
}

/// TNT's blast: the TNT as `this_entity`, and no radius, as `tnt_explosion_drop_decay`
/// is false (`docs/groundwork/n6-brief.md`, finding 2.0 item 4).
const BY_TNT: Broken<'static> = Broken::by_explosion(true, None);

#[test]
fn a_chorus_flower_drops_to_an_explosion_and_not_to_a_rule() {
    let flower = ItemStack::new(items::CHORUS_FLOWER, 1);
    assert_eq!(drops("minecraft:chorus_flower[age=0]", &[]), []);
    assert_eq!(
        drops_as("minecraft:chorus_flower[age=0]", BY_TNT, &[]),
        [flower]
    );
}

#[test]
fn an_explosion_that_decays_keeps_a_block_one_time_in_its_radius() {
    let decaying = Broken::by_explosion(true, Some(4.0));
    let cobblestone = ItemStack::new(items::COBBLESTONE, 1);
    // `survives_explosion` holds for a draw at or below a quarter.
    assert_eq!(
        drops_as("minecraft:stone", decaying, &[Draw::Float(0.25)]),
        std::slice::from_ref(&cobblestone)
    );
    assert_eq!(
        drops_as("minecraft:stone", decaying, &[Draw::Float(0.26)]),
        []
    );
    // TNT's own blast keeps everything, without a draw.
    assert_eq!(drops_as("minecraft:stone", BY_TNT, &[]), [cobblestone]);
}

// A player's break: the tables read the tool in the main hand (`Block.getDrops(state,
// level, pos, blockEntity, entity, tool)`, `docs/groundwork/v-plan.md`, section 4.5).
// Whether the tool is the correct one for the block's drops at all is the caller's
// (`Player.hasCorrectToolForDrops`), so these tests evaluate the tables only.

/// `item` enchanted with `levels` in its `minecraft:enchantments`.
fn enchanted(item: i32, levels: &[(&str, i32)]) -> ItemStack {
    let mut tool = ItemStack::new(item, 1);
    tool.components.set(Component::Enchantments(
        levels
            .iter()
            .map(|&(name, level)| (name.to_owned(), level))
            .collect(),
    ));
    tool
}

/// What `text` drops broken by a player holding `tool`, with these draws, all of which
/// it has to take.
fn drops_with(text: &str, tool: &ItemStack, draws: &[Draw]) -> Vec<ItemStack> {
    drops_as(text, Broken::by_player(tool), draws)
}

const SILK_TOUCH: &str = "minecraft:silk_touch";
const FORTUNE: &str = "minecraft:fortune";

fn fortune(item: i32, level: i32) -> ItemStack {
    enchanted(item, &[(FORTUNE, level)])
}

#[test]
fn stone_drops_cobblestone_to_a_pickaxe_and_itself_to_silk_touch() {
    let wooden = ItemStack::new(items::WOODEN_PICKAXE, 1);
    assert_eq!(
        drops_with("minecraft:stone", &wooden, &[]),
        [stack(items::COBBLESTONE, 1)]
    );
    let silky = enchanted(items::DIAMOND_PICKAXE, &[(SILK_TOUCH, 1)]);
    assert_eq!(
        drops_with("minecraft:stone", &silky, &[]),
        [stack(items::STONE, 1)]
    );
}

/// `short_grass`: itself to shears without a draw; else seeds where a draw is below an
/// eighth, then `uniform_bonus_count` (multiplier 2) draws `nextInt(2·level + 1)`.
#[test]
fn grass_drops_itself_to_shears_and_seeds_by_chance_without_them() {
    let shears = ItemStack::new(items::SHEARS, 1);
    assert_eq!(
        drops_with("minecraft:short_grass", &shears, &[]),
        [stack(items::SHORT_GRASS, 1)]
    );
    let hand = ItemStack::new(items::AIR, 0);
    assert_eq!(
        drops_with("minecraft:short_grass", &hand, &[Draw::Float(0.125)]),
        []
    );
    assert_eq!(
        drops_with(
            "minecraft:short_grass",
            &hand,
            &[Draw::Float(0.124), Draw::Int { bound: 1, value: 0 }]
        ),
        [stack(items::WHEAT_SEEDS, 1)]
    );
    // Fortune II: `nextInt(5)`, added to the one.
    assert_eq!(
        drops_with(
            "minecraft:short_grass",
            &fortune(items::IRON_HOE, 2),
            &[Draw::Float(0.0), Draw::Int { bound: 5, value: 4 }]
        ),
        [stack(items::WHEAT_SEEDS, 5)]
    );
}

#[test]
fn glass_drops_itself_to_silk_touch_and_nothing_without() {
    let silky = enchanted(items::IRON_PICKAXE, &[(SILK_TOUCH, 1)]);
    assert_eq!(
        drops_with("minecraft:glass", &silky, &[]),
        [stack(items::GLASS, 1)]
    );
    let plain = ItemStack::new(items::IRON_PICKAXE, 1);
    assert_eq!(drops_with("minecraft:glass", &plain, &[]), []);
}

/// `can_silk_touch` reads `minecraft:enchantments` (`ItemEnchantmentsPredicate.
/// Enchantments`); a book's `stored_enchantments` are not the tool's.
#[test]
fn an_enchanted_book_holding_silk_touch_does_not_count_as_silk_touch() {
    let mut book = ItemStack::new(items::ENCHANTED_BOOK, 1);
    book.components.set(Component::StoredEnchantments(vec![(
        SILK_TOUCH.to_owned(),
        1,
    )]));
    assert_eq!(drops_with("minecraft:glass", &book, &[]), []);
    assert_eq!(
        drops_with("minecraft:stone", &book, &[]),
        [stack(items::COBBLESTONE, 1)]
    );
}

/// An empty stack given as the tool is the game's `ItemStack.EMPTY`, whatever it holds.
#[test]
fn an_empty_stack_with_silk_touch_is_no_tool() {
    let mut empty = enchanted(items::DIAMOND_PICKAXE, &[(SILK_TOUCH, 1)]);
    empty.count = 0;
    assert_eq!(drops_with("minecraft:glass", &empty, &[]), []);
}

/// `ore_drops` at level L draws `nextInt(L + 2)` and multiplies the count by
/// `max(draw − 1, 0) + 1`; at level 0 it draws nothing.
fn ore_with_fortune(ore: &str, ore_item: i32, dropped: i32) {
    let silky = enchanted(items::DIAMOND_PICKAXE, &[(SILK_TOUCH, 1)]);
    assert_eq!(drops_with(ore, &silky, &[]), [stack(ore_item, 1)]);
    let plain = ItemStack::new(items::IRON_PICKAXE, 1);
    assert_eq!(drops_with(ore, &plain, &[]), [stack(dropped, 1)]);
    let fortune_three = fortune(items::DIAMOND_PICKAXE, 3);
    for (value, count) in [(0, 1), (1, 1), (2, 2), (3, 3), (4, 4)] {
        assert_eq!(
            drops_with(ore, &fortune_three, &[Draw::Int { bound: 5, value }]),
            [stack(dropped, count)],
            "a draw of {value}"
        );
    }
    let fortune_one = fortune(items::DIAMOND_PICKAXE, 1);
    assert_eq!(
        drops_with(ore, &fortune_one, &[Draw::Int { bound: 3, value: 2 }]),
        [stack(dropped, 2)]
    );
}

#[test]
fn diamond_ore_drops_itself_to_silk_touch_and_diamonds_by_fortunes_ore_formula() {
    ore_with_fortune("minecraft:diamond_ore", items::DIAMOND_ORE, items::DIAMOND);
}

#[test]
fn coal_ore_drops_itself_to_silk_touch_and_coal_by_fortunes_ore_formula() {
    ore_with_fortune("minecraft:coal_ore", items::COAL_ORE, items::COAL);
}

/// Silk Touch and Fortune together: the alternatives' first child, the ore itself,
/// wins without a draw.
#[test]
fn silk_touch_beside_fortune_drops_the_ore() {
    let both = enchanted(items::DIAMOND_PICKAXE, &[(FORTUNE, 3), (SILK_TOUCH, 1)]);
    assert_eq!(
        drops_with("minecraft:diamond_ore", &both, &[]),
        [stack(items::DIAMOND_ORE, 1)]
    );
}

/// `ore_drops` at Fortune III over many breaks: two draws in five (0 and 1) give one,
/// the others two, three and four, an average of 2.2 diamonds a break.
#[test]
fn fortune_three_on_diamond_ore_averages_two_and_a_fifth_over_many_breaks() {
    let tool = fortune(items::DIAMOND_PICKAXE, 3);
    let broken = Broken::by_player(&tool);
    let mut random = JavaRandom::new(7);
    let mut counts = [0_u32; 5];
    let breaks = 50_000_u32;
    for _ in 0..breaks {
        let dropped = block_drops_as(
            state("minecraft:diamond_ore"),
            AT,
            broken,
            &mut random,
            &mut air,
        );
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].item, items::DIAMOND);
        counts[usize::try_from(dropped[0].count).unwrap()] += 1;
    }
    assert_eq!(counts[0], 0);
    let total: u32 = (1..5_u32).map(|count| counts[count as usize] * count).sum();
    let average = f64::from(total) / f64::from(breaks);
    assert!((average - 2.2).abs() < 0.03, "an average of {average}");
    let ones = f64::from(counts[1]) / f64::from(breaks);
    assert!((ones - 0.4).abs() < 0.01, "ones {ones}");
}

/// Gravel's flint: `table_bonus` with chances 0.1, 1/7, 0.25 and 1 at Fortune 0 to 3,
/// the last above them.
#[test]
fn gravels_flint_takes_the_chance_of_fortunes_level_and_the_last_above_the_table() {
    let shovel = ItemStack::new(items::IRON_SHOVEL, 1);
    let chances = [
        (shovel, 0.1_f32),
        (fortune(items::IRON_SHOVEL, 1), 0.142_857_15),
        (fortune(items::IRON_SHOVEL, 2), 0.25),
        (fortune(items::IRON_SHOVEL, 3), 1.0),
        (fortune(items::IRON_SHOVEL, 5), 1.0),
    ];
    for (tool, chance) in &chances {
        let below = chance - 0.001;
        assert_eq!(
            drops_with("minecraft:gravel", tool, &[Draw::Float(below)]),
            [stack(items::FLINT, 1)],
            "{chance}"
        );
        if *chance < 1.0 {
            assert_eq!(
                drops_with("minecraft:gravel", tool, &[Draw::Float(*chance)]),
                [stack(items::GRAVEL, 1)],
                "{chance}"
            );
        }
    }
    let silky = enchanted(items::IRON_SHOVEL, &[(SILK_TOUCH, 1)]);
    assert_eq!(
        drops_with("minecraft:gravel", &silky, &[]),
        [stack(items::GRAVEL, 1)]
    );
}

/// Leaves to shears or Silk Touch: the leaves, and the sticks' and the apple's pools,
/// whose conditions are the inverse, are skipped without a draw.
#[test]
fn leaves_drop_themselves_to_shears_and_to_silk_touch_without_a_draw() {
    let shears = ItemStack::new(items::SHEARS, 1);
    assert_eq!(
        drops_with("minecraft:oak_leaves", &shears, &[]),
        [stack(items::OAK_LEAVES, 1)]
    );
    let silky = enchanted(items::DIAMOND_HOE, &[(SILK_TOUCH, 1)]);
    assert_eq!(
        drops_with("minecraft:oak_leaves", &silky, &[]),
        [stack(items::OAK_LEAVES, 1)]
    );
}

/// Leaves to Fortune III: a sapling below 0.1, sticks below 1/30 (then one or two), an
/// apple below 1/120.
#[test]
fn leaves_to_fortune_three_take_each_pools_third_chance() {
    let hoe = fortune(items::DIAMOND_HOE, 3);
    assert_eq!(
        drops_with(
            "minecraft:oak_leaves",
            &hoe,
            &[
                Draw::Float(0.099),
                Draw::Float(0.033),
                Draw::Int { bound: 2, value: 1 },
                Draw::Float(0.0083),
            ]
        ),
        [
            stack(items::OAK_SAPLING, 1),
            stack(items::STICK, 2),
            stack(items::APPLE, 1)
        ]
    );
    assert_eq!(
        drops_with(
            "minecraft:oak_leaves",
            &hoe,
            &[Draw::Float(0.1), Draw::Float(0.034), Draw::Float(0.0084)]
        ),
        []
    );
}

/// The amethyst cluster's `match_tool` names the item tag
/// `#minecraft:cluster_max_harvestables` (the pickaxes): four shards, Fortune's
/// `ore_drops` on them; any other tool two.
#[test]
fn an_amethyst_cluster_drops_four_shards_to_a_tool_in_the_tag_and_two_else() {
    let pickaxe = ItemStack::new(items::IRON_PICKAXE, 1);
    let cluster = "minecraft:amethyst_cluster[facing=up,waterlogged=false]";
    assert_eq!(
        drops_with(cluster, &pickaxe, &[]),
        [stack(items::AMETHYST_SHARD, 4)]
    );
    assert_eq!(
        drops_with(
            cluster,
            &fortune(items::IRON_PICKAXE, 2),
            &[Draw::Int { bound: 4, value: 3 }]
        ),
        [stack(items::AMETHYST_SHARD, 12)]
    );
    let axe = ItemStack::new(items::WOODEN_AXE, 1);
    assert_eq!(
        drops_with(cluster, &axe, &[]),
        [stack(items::AMETHYST_SHARD, 2)]
    );
    assert_eq!(drops(cluster, &[]), [stack(items::AMETHYST_SHARD, 2)]);
}

/// A player is the table's `this_entity`, so the chorus flower's `entity_properties`
/// holds, as it does not for a rule's break.
#[test]
fn a_chorus_flower_broken_by_a_player_drops_itself() {
    let hand = ItemStack::new(items::AIR, 0);
    assert_eq!(
        drops_with("minecraft:chorus_flower[age=0]", &hand, &[]),
        [stack(items::CHORUS_FLOWER, 1)]
    );
}

/// Every state of every block broken by a player with Fortune III, with shears and with
/// Silk Touch: no table asks for what the evaluator cannot answer with a tool, and no
/// count comes out negative.
#[test]
fn every_block_state_evaluates_with_each_kind_of_tool() {
    let tools = [
        fortune(items::DIAMOND_PICKAXE, 3),
        ItemStack::new(items::SHEARS, 1),
        enchanted(items::DIAMOND_AXE, &[(SILK_TOUCH, 1)]),
    ];
    let mut random = JavaRandom::new(2);
    let mut grass_above = |_: BlockPos| state("minecraft:tall_grass[half=upper]");
    for tool in &tools {
        for id in 0..BLOCK_STATE_COUNT {
            let block = BlockState(u16::try_from(id).unwrap());
            let broken = Broken::by_player(tool);
            for dropped in block_drops_as(block, AT, broken, &mut random, &mut grass_above) {
                assert!(dropped.count >= 0, "{block:?}");
                assert!(usize::try_from(dropped.item).unwrap() < clustine_data::ITEMS.len());
            }
        }
    }
}
