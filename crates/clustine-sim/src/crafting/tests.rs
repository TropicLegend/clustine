//! The matcher against the game's recipes, each kind with a grid of its own, and the
//! pairs of recipes one grid can match, listed for the comparison with the official
//! server.

use std::collections::BTreeSet;

use clustine_data::recipes::{self, Crafting, IngredientId};
use clustine_data::{ItemComponents, items};
use clustine_world::components::{BannerLayer, Fireworks, Kind, PotionContents};
use clustine_world::{Component, ItemStack};

use super::*;

fn stack(item: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, 1))
}

/// A 3×3 grid from rows of item ids, 0 for an empty slot.
fn grid(rows: [[i32; 3]; 3]) -> Vec<Option<ItemStack>> {
    rows.iter()
        .flatten()
        .map(|item| (*item != 0).then(|| ItemStack::new(*item, 1)))
        .collect()
}

fn key_of(id: RecipeId) -> &'static str {
    id.recipe().key
}

/// The grid's result and its recipe's key.
fn result_of(
    width: usize,
    height: usize,
    grid: &[Option<ItemStack>],
) -> Option<(&'static str, ItemStack)> {
    craft(width, height, grid).map(|(id, stack)| (key_of(id), stack))
}

#[test]
fn nothing_in_the_grid_makes_nothing_and_leaves_nothing() {
    let empty = vec![None; 9];
    assert_eq!(craft(3, 3, &empty), None);
    assert_eq!(remainders(3, 3, &empty), vec![None; 9]);
    assert_eq!(craft(2, 2, &[None, None, None, None]), None);
}

#[test]
fn a_log_of_any_kind_makes_planks_of_its_kind_from_its_tag_wherever_it_lies() {
    for (log, planks) in [
        (items::OAK_LOG, items::OAK_PLANKS),
        (items::STRIPPED_OAK_WOOD, items::OAK_PLANKS),
        (items::ACACIA_LOG, items::ACACIA_PLANKS),
    ] {
        for at in 0..4 {
            let mut two_by_two = vec![None; 4];
            two_by_two[at] = stack(log);
            let (key, made) = result_of(2, 2, &two_by_two).expect("a log makes planks");
            assert!(key.ends_with("_planks"), "{key}");
            assert_eq!(made, ItemStack::new(planks, 4));
        }
    }
}

#[test]
fn a_pickaxe_takes_planks_of_any_wood_and_two_sticks() {
    let (oak, birch, stick) = (items::OAK_PLANKS, items::BIRCH_PLANKS, items::STICK);
    let made = result_of(
        3,
        3,
        &grid([[oak, birch, oak], [0, stick, 0], [0, stick, 0]]),
    );
    assert_eq!(
        made,
        Some(("wooden_pickaxe", ItemStack::new(items::WOODEN_PICKAXE, 1)))
    );
    // One stick short, or one out of place, is no pickaxe.
    assert_eq!(
        craft(3, 3, &grid([[oak, oak, oak], [0, stick, 0], [0, 0, 0]])),
        None
    );
    assert_eq!(
        craft(3, 3, &grid([[oak, oak, oak], [stick, 0, 0], [0, stick, 0]])),
        None
    );
}

#[test]
fn an_axe_is_made_facing_either_way() {
    let (oak, stick) = (items::OAK_PLANKS, items::STICK);
    let left = grid([[oak, oak, 0], [oak, stick, 0], [0, stick, 0]]);
    let right = grid([[oak, oak, 0], [stick, oak, 0], [stick, 0, 0]]);
    for facing in [left, right] {
        assert_eq!(
            result_of(3, 3, &facing),
            Some(("wooden_axe", ItemStack::new(items::WOODEN_AXE, 1)))
        );
    }
}

#[test]
fn a_pattern_is_found_wherever_it_lies_in_the_grid() {
    let planks = items::OAK_PLANKS;
    for column in 0..3 {
        for top in 0..2 {
            let mut rows = [[0; 3]; 3];
            rows[top][column] = planks;
            rows[top + 1][column] = planks;
            assert_eq!(
                result_of(3, 3, &grid(rows)),
                Some(("stick", ItemStack::new(items::STICK, 4))),
                "column {column}, top {top}"
            );
        }
    }
    // Apart, they are not the pattern.
    assert_eq!(
        craft(3, 3, &grid([[planks, 0, 0], [0, 0, 0], [planks, 0, 0]])),
        None
    );
}

#[test]
fn a_shapeless_dye_is_mixed_in_any_order_and_place() {
    let (red, blue) = (items::RED_DYE, items::BLUE_DYE);
    for rows in [
        [[red, blue, 0], [0, 0, 0], [0, 0, 0]],
        [[0, 0, blue], [0, 0, 0], [red, 0, 0]],
    ] {
        assert_eq!(
            result_of(3, 3, &grid(rows)),
            Some(("purple_dye", ItemStack::new(items::PURPLE_DYE, 2)))
        );
    }
}

/// An ingredient's index in the shared table.
fn ingredient_id(tag: Option<&str>, items: &[i32]) -> IngredientId {
    (0..u16::MAX)
        .find(|id| {
            let ingredient = recipes::ingredient(*id);
            ingredient.tag == tag
                && ingredient
                    .items
                    .iter()
                    .map(|item| i32::from(*item))
                    .eq(items.iter().copied())
        })
        .expect("the ingredient is in the table")
}

#[test]
fn a_shapeless_recipe_pairs_its_ingredients_with_the_stacks_where_first_fit_would_not() {
    // No shapeless recipe of 26.3 has two ingredients that share an item, so two of the
    // table's are put together: any planks, and oak planks.
    let any_planks = (0..u16::MAX)
        .find(|id| recipes::ingredient(*id).tag == Some("minecraft:planks"))
        .expect("the planks tag is an ingredient");
    let oak = ingredient_id(None, &[items::OAK_PLANKS]);
    let grid = [stack(items::OAK_PLANKS), stack(items::BIRCH_PLANKS)];
    let input = Input::of(2, 1, &grid);
    // First fit gives the oak planks to "any planks", and the birch planks are no oak.
    assert!(shapeless(&[any_planks, oak], &input));
    assert!(shapeless(&[oak, any_planks], &input));
    let two_birch = [stack(items::BIRCH_PLANKS), stack(items::BIRCH_PLANKS)];
    assert!(!shapeless(&[any_planks, oak], &Input::of(2, 1, &two_birch)));
}

#[test]
fn a_shulker_box_dyed_keeps_its_components_and_is_not_dyed_its_own_colour() {
    let mut boxed = ItemStack::new(items::SHULKER_BOX, 1);
    boxed.components.set(Component::Enchantments(vec![(
        "minecraft:unbreaking".to_owned(),
        2,
    )]));
    let grid = [Some(boxed.clone()), stack(items::RED_DYE)];
    let (key, dyed) = result_of(2, 1, &grid).expect("a box is dyed");
    assert_eq!(key, "red_shulker_box");
    assert_eq!(dyed.item, items::RED_SHULKER_BOX);
    assert_eq!(dyed.count, 1);
    assert_eq!(dyed.components, boxed.components);
    // A red box dyed red is the same box: no recipe.
    let again = [Some(dyed), stack(items::RED_DYE)];
    assert_eq!(craft(2, 1, &again), None);
    // Two dyes are one too many.
    let two = [
        stack(items::SHULKER_BOX),
        stack(items::RED_DYE),
        stack(items::RED_DYE),
    ];
    assert_eq!(craft(3, 1, &two), None);
}

#[test]
fn leather_armour_is_dyed_and_dyed_again_as_the_game_mixes() {
    let helmet = [stack(items::LEATHER_HELMET), stack(items::RED_DYE)];
    let (key, red) = result_of(2, 1, &helmet).expect("a helmet is dyed");
    assert_eq!(key, "leather_helmet_dyed");
    // Red's texture colour alone, `0xB02E26`.
    assert_eq!(
        red.components.get(Kind::DyedColor),
        Some(&Component::DyedColor(0x00B0_2E26))
    );
    let again = [Some(red), stack(items::YELLOW_DYE)];
    let (_, orange) = result_of(2, 1, &again).expect("a red helmet is dyed");
    // (176+254)/2, (46+216)/2, (38+61)/2 = 215, 131, 49; scaled by 215 / 215.
    assert_eq!(
        orange.components.get(Kind::DyedColor),
        Some(&Component::DyedColor((215 << 16) | (131 << 8) | 49))
    );
    // A helmet alone, or with two helmets, is dyed nothing.
    assert_eq!(craft(1, 1, &[stack(items::LEATHER_HELMET)]), None);
    let two = [
        stack(items::LEATHER_HELMET),
        stack(items::LEATHER_HELMET),
        stack(items::RED_DYE),
    ];
    assert_eq!(craft(3, 1, &two), None);
}

#[test]
fn tipped_arrows_take_the_lingering_potion_s_contents() {
    let mut potion = ItemStack::new(items::LINGERING_POTION, 1);
    let poison = PotionContents {
        potion: Some("minecraft:poison".to_owned()),
        ..PotionContents::default()
    };
    potion
        .components
        .set(Component::PotionContents(poison.clone()));
    let arrow = items::ARROW;
    let mut cells = grid([
        [arrow, arrow, arrow],
        [arrow, 0, arrow],
        [arrow, arrow, arrow],
    ]);
    cells[4] = Some(potion);
    let (key, arrows) = result_of(3, 3, &cells).expect("arrows are tipped");
    assert_eq!(key, "tipped_arrow");
    assert_eq!(arrows.item, items::TIPPED_ARROW);
    assert_eq!(arrows.count, 8);
    assert_eq!(
        arrows.components.get(Kind::PotionContents),
        Some(&Component::PotionContents(poison))
    );
    // Seven arrows are not enough.
    cells[0] = None;
    assert_eq!(craft(3, 3, &cells), None);
}

#[test]
fn four_bricks_make_a_plain_pot_and_a_sherd_makes_nothing_clustine_could_hold() {
    let brick = items::BRICK;
    let bricks = grid([[0, brick, 0], [brick, 0, brick], [0, brick, 0]]);
    assert_eq!(
        result_of(3, 3, &bricks),
        Some(("decorated_pot", ItemStack::new(items::DECORATED_POT, 1)))
    );
    let sherd = grid([
        [0, items::ANGLER_POTTERY_SHERD, 0],
        [brick, 0, brick],
        [0, brick, 0],
    ]);
    let input = Input::of(3, 3, &sherd);
    assert_eq!(first_match(&input).map(key_of), Some("decorated_pot"));
    assert_eq!(craft(3, 3, &sherd), None);
}

#[test]
fn a_written_book_is_not_cloned_since_no_clustine_book_has_content() {
    let grid = [stack(items::WRITTEN_BOOK), stack(items::WRITABLE_BOOK)];
    assert_eq!(craft(2, 1, &grid), None);
    // With no recipe, what is left is what was there.
    assert_eq!(remainders(2, 1, &grid), grid.to_vec());
}

fn patterned(banner: i32) -> ItemStack {
    let mut stack = ItemStack::new(banner, 1);
    stack
        .components
        .set(Component::BannerPatterns(vec![BannerLayer {
            pattern: "minecraft:stripe_top".to_owned(),
            colour: "black".to_owned(),
        }]));
    stack
}

#[test]
fn a_banner_is_copied_onto_a_plain_one_and_the_original_stays() {
    let original = patterned(items::WHITE_BANNER);
    let grid = [Some(original.clone()), None, stack(items::WHITE_BANNER)];
    let (key, copy) = result_of(3, 1, &grid).expect("the banner is copied");
    assert_eq!(key, "white_banner_duplicate");
    assert_eq!(copy, original);
    assert_eq!(remainders(3, 1, &grid), vec![Some(original), None, None]);
    // Banners of two colours, or two plain ones, are no copy.
    assert_eq!(
        craft(
            2,
            1,
            &[
                Some(patterned(items::WHITE_BANNER)),
                stack(items::RED_BANNER)
            ]
        ),
        None
    );
    assert_eq!(
        craft(
            2,
            1,
            &[stack(items::WHITE_BANNER), stack(items::WHITE_BANNER)]
        ),
        None
    );
}

#[test]
fn a_star_is_matched_but_not_made_and_a_rocket_flies_as_long_as_its_gunpowder() {
    let star = [stack(items::GUNPOWDER), stack(items::RED_DYE)];
    let input = Input::of(2, 1, &star);
    assert_eq!(first_match(&input).map(key_of), Some("firework_star"));
    assert_eq!(craft(2, 1, &star), None);

    let (paper, powder) = (items::PAPER, items::GUNPOWDER);
    let (key, one) = result_of(2, 1, &[stack(paper), stack(powder)]).expect("a rocket");
    assert_eq!(key, "firework_rocket");
    // A flight of 1 is the rocket's default.
    assert_eq!(one, ItemStack::new(items::FIREWORK_ROCKET, 3));
    let with_star = grid([
        [paper, powder, powder],
        [items::FIREWORK_STAR, 0, 0],
        [0, 0, 0],
    ]);
    let (_, two) = result_of(3, 3, &with_star).expect("a rocket with a star");
    assert_eq!(two.count, 3);
    assert_eq!(
        two.components.get(Kind::Fireworks),
        Some(&Component::Fireworks(Fireworks {
            flight_duration: 2,
            explosions: Vec::new(),
        }))
    );
    let four = grid([[paper, powder, powder], [powder, powder, 0], [0, 0, 0]]);
    assert_eq!(craft(3, 3, &four), None);
}

fn worn(item: i32, damage: i32) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack.components.set(Component::Damage(damage));
    stack
}

#[test]
fn two_worn_pickaxes_are_mended_into_one_with_a_twentieth_more_and_only_their_curses() {
    let most = i32::from(
        ItemComponents::of(items::IRON_PICKAXE)
            .max_damage
            .expect("a pickaxe wears"),
    );
    assert_eq!(most, 250);
    let mut first = worn(items::IRON_PICKAXE, 200);
    first.components.set(Component::Enchantments(vec![
        ("minecraft:efficiency".to_owned(), 3),
        ("minecraft:vanishing_curse".to_owned(), 1),
    ]));
    let second = worn(items::IRON_PICKAXE, 180);
    let (key, mended) = result_of(2, 1, &[Some(first), Some(second)]).expect("mended");
    assert_eq!(key, "repair_item");
    // Left: 50 + 70 + 250 · 5 / 100 = 132, so worn by 118.
    assert_eq!(mended.count, 1);
    assert_eq!(
        mended.components.get(Kind::Damage),
        Some(&Component::Damage(118))
    );
    assert_eq!(
        mended.components.get(Kind::Enchantments),
        Some(&Component::Enchantments(vec![(
            "minecraft:vanishing_curse".to_owned(),
            1
        )]))
    );
    // Hardly worn, they mend whole: no wear in the patch.
    let (_, whole) = result_of(
        2,
        1,
        &[
            Some(worn(items::IRON_PICKAXE, 5)),
            Some(worn(items::IRON_PICKAXE, 5)),
        ],
    )
    .expect("mended");
    assert_eq!(whole, ItemStack::new(items::IRON_PICKAXE, 1));
    // Two kinds, or two of one in a stack, are not mended.
    assert_eq!(
        craft(
            2,
            1,
            &[stack(items::IRON_PICKAXE), stack(items::DIAMOND_PICKAXE)]
        ),
        None
    );
    assert_eq!(
        craft(
            2,
            1,
            &[
                Some(ItemStack::new(items::IRON_PICKAXE, 2)),
                stack(items::IRON_PICKAXE)
            ]
        ),
        None
    );
}

#[test]
fn a_shield_is_matched_with_a_banner_but_not_decorated() {
    let grid = [stack(items::WHITE_BANNER), stack(items::SHIELD)];
    let input = Input::of(2, 1, &grid);
    assert_eq!(first_match(&input).map(key_of), Some("shield_decoration"));
    assert_eq!(craft(2, 1, &grid), None);
}

#[test]
fn a_cake_leaves_three_buckets_where_the_milk_was() {
    let (milk, sugar, wheat) = (items::MILK_BUCKET, items::SUGAR, items::WHEAT);
    let cake = grid([
        [milk, milk, milk],
        [sugar, items::EGG, sugar],
        [wheat, wheat, wheat],
    ]);
    assert_eq!(
        result_of(3, 3, &cake),
        Some(("cake", ItemStack::new(items::CAKE, 1)))
    );
    let mut expected = vec![None; 9];
    for slot in &mut expected[0..3] {
        *slot = stack(items::BUCKET);
    }
    assert_eq!(remainders(3, 3, &cake), expected);
}

#[test]
fn a_honey_bottle_leaves_a_glass_bottle_in_its_own_slot() {
    let mut cells = vec![None; 9];
    cells[8] = stack(items::HONEY_BOTTLE);
    assert_eq!(
        result_of(3, 3, &cells),
        Some(("sugar_from_honey_bottle", ItemStack::new(items::SUGAR, 3)))
    );
    let mut expected = vec![None; 9];
    expected[8] = stack(items::GLASS_BOTTLE);
    assert_eq!(remainders(3, 3, &cells), expected);
}

#[test]
fn a_grid_without_a_recipe_leaves_its_stacks_as_they_are() {
    let cells = [
        Some(ItemStack::new(items::DIRT, 5)),
        None,
        None,
        stack(items::STICK),
    ];
    assert_eq!(craft(2, 2, &cells), None);
    assert_eq!(remainders(2, 2, &cells), cells.to_vec());
}

// The pairs of recipes one grid can match. The game takes the first in the registry's
// order, believed to be by key; each pair is a grid to try on the official server.

/// The items two ingredients share.
fn shared(a: IngredientId, b: IngredientId) -> Vec<u16> {
    let other: BTreeSet<u16> = recipes::ingredient(b).items.iter().copied().collect();
    recipes::ingredient(a)
        .items
        .iter()
        .copied()
        .filter(|item| other.contains(item))
        .collect()
}

/// A pattern recipe's slots as ingredients: a shaped one's cells (`None` empty), or a
/// shapeless one's ingredients.
fn slots(kind: &Crafting) -> Option<(bool, usize, usize, Vec<Option<IngredientId>>)> {
    match *kind {
        Crafting::Shaped {
            width,
            height,
            cells,
            ..
        } => Some((
            true,
            usize::from(width),
            usize::from(height),
            cells.iter().map(|cell| cell.checked_sub(1)).collect(),
        )),
        Crafting::Shapeless { ingredients, .. } => Some((
            false,
            ingredients.len(),
            1,
            ingredients.iter().map(|id| Some(*id)).collect(),
        )),
        _ => None,
    }
}

/// Whether one grid can match both pattern recipes: two shaped ones of one box whose
/// cells share an item each, as they are or one mirrored; or, with a shapeless one, as
/// many stacks as ingredients, paired so that each pair shares an item.
fn patterns_overlap(a: &Crafting, b: &Crafting) -> bool {
    let (Some((a_shaped, aw, ah, a_cells)), Some((b_shaped, bw, bh, b_cells))) =
        (slots(a), slots(b))
    else {
        return false;
    };
    let compatible = |x: Option<IngredientId>, y: Option<IngredientId>| match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => !shared(x, y).is_empty(),
        _ => false,
    };
    if a_shaped && b_shaped {
        if (aw, ah) != (bw, bh) {
            return false;
        }
        return [false, true].into_iter().any(|mirrored| {
            (0..ah).all(|y| {
                (0..aw).all(|x| {
                    let bx = if mirrored { aw - x - 1 } else { x };
                    compatible(a_cells[x + y * aw], b_cells[bx + y * bw])
                })
            })
        });
    }
    let a_used: Vec<IngredientId> = a_cells.iter().flatten().copied().collect();
    let b_used: Vec<IngredientId> = b_cells.iter().flatten().copied().collect();
    perfect_matching(a_used.len(), b_used.len(), |x, y| {
        !shared(a_used[x], b_used[y]).is_empty()
    })
}

/// Every item a recipe of another kind names: what a grid it matches is made of.
fn universe(kind: &Crafting) -> BTreeSet<u16> {
    let ids: Vec<IngredientId> = match *kind {
        Crafting::Transmute {
            input, material, ..
        } => vec![input, material],
        Crafting::Dye { target, dye, .. } => vec![target, dye],
        Crafting::Imbue {
            source, material, ..
        } => vec![source, material],
        Crafting::DecoratedPot {
            back,
            left,
            right,
            front,
            ..
        } => vec![back, left, right, front],
        Crafting::BookCloning {
            source, material, ..
        } => vec![source, material],
        Crafting::MapExtending { map, material } => vec![map, material],
        Crafting::FireworkRocket {
            shell, fuel, star, ..
        } => vec![shell, fuel, star],
        Crafting::FireworkStar {
            fuel,
            dye,
            trail,
            twinkle,
            shapes,
            ..
        } => [fuel, dye, trail, twinkle]
            .into_iter()
            .chain(shapes.iter().map(|(_, id)| *id))
            .collect(),
        Crafting::FireworkStarFade { target, dye, .. } => vec![target, dye],
        Crafting::BannerDuplicate { banner, .. } => vec![banner],
        Crafting::ShieldDecoration { banner, target, .. } => vec![banner, target],
        Crafting::RepairItem => {
            return (0..clustine_data::ITEMS.len())
                .filter(|item| ItemComponents::of(*item as i32).max_damage.is_some())
                .map(|item| u16::try_from(item).expect("an item id fits 16 bits"))
                .collect();
        }
        Crafting::Shaped { .. } | Crafting::Shapeless { .. } => Vec::new(),
    };
    ids.iter()
        .flat_map(|id| recipes::ingredient(*id).items.iter().copied())
        .collect()
}

/// A grid a pattern recipe matches, of items `within` holds where it can: each
/// ingredient its first such item. `None` where an ingredient has none.
fn pattern_grid(
    kind: &Crafting,
    within: &BTreeSet<u16>,
) -> Option<(usize, usize, Vec<Option<ItemStack>>)> {
    let (_, width, height, cells) = slots(kind)?;
    let mut grid = Vec::new();
    for cell in cells {
        grid.push(match cell {
            None => None,
            Some(id) => {
                let item = recipes::ingredient(id)
                    .items
                    .iter()
                    .find(|item| within.contains(item))?;
                Some(ItemStack::new(i32::from(*item), 1))
            }
        });
    }
    Some((width, height, grid))
}

/// A grid each recipe of another kind matches, made of each ingredient's first item;
/// none for the two that match nothing in Clustine.
fn example_grid(kind: &Crafting) -> Option<(usize, usize, Vec<Option<ItemStack>>)> {
    let first = |id: IngredientId| {
        Some(ItemStack::new(
            i32::from(recipes::ingredient(id).items[0]),
            1,
        ))
    };
    let row = |stacks: Vec<Option<ItemStack>>| Some((stacks.len(), 1, stacks));
    match *kind {
        Crafting::Transmute {
            input,
            material,
            min,
            ..
        } => row(std::iter::once(first(input))
            .chain((0..min).map(|_| first(material)))
            .collect()),
        Crafting::Dye { target, dye, .. } => row(vec![first(target), first(dye)]),
        Crafting::Imbue {
            source, material, ..
        } => {
            let mut cells: Vec<Option<ItemStack>> = (0..9).map(|_| first(material)).collect();
            cells[4] = first(source);
            Some((3, 3, cells))
        }
        Crafting::DecoratedPot {
            back,
            left,
            right,
            front,
            ..
        } => {
            let mut cells = vec![None; 9];
            cells[1] = first(back);
            cells[3] = first(left);
            cells[5] = first(right);
            cells[7] = first(front);
            Some((3, 3, cells))
        }
        Crafting::FireworkRocket { shell, fuel, .. } => row(vec![first(shell), first(fuel)]),
        Crafting::FireworkStar { fuel, dye, .. } => row(vec![first(fuel), first(dye)]),
        Crafting::FireworkStarFade { target, dye, .. } => row(vec![first(target), first(dye)]),
        Crafting::BannerDuplicate { banner, .. } => {
            let item = i32::from(recipes::ingredient(banner).items[0]);
            row(vec![Some(patterned(item)), stack(item)])
        }
        Crafting::ShieldDecoration { banner, target, .. } => {
            row(vec![first(banner), first(target)])
        }
        Crafting::RepairItem => row(vec![
            Some(worn(items::IRON_PICKAXE, 10)),
            Some(worn(items::IRON_PICKAXE, 10)),
        ]),
        Crafting::BookCloning { .. }
        | Crafting::MapExtending { .. }
        | Crafting::Shaped { .. }
        | Crafting::Shapeless { .. } => None,
    }
}

/// The pairs of crafting recipes one grid can match, each by key in the table's order.
/// Two pattern recipes are compared exactly, by their ingredients' items; a recipe of
/// another kind is tried on a grid of each pattern recipe made of its own items, and every
/// recipe on a grid of each recipe of another kind. That second part tries one grid each
/// and can miss a pair.
fn overlapping_pairs() -> BTreeSet<(&'static str, &'static str)> {
    let all = recipes::crafting();
    let mut pairs = BTreeSet::new();
    let mut add = |a: usize, b: usize| {
        let (first, second) = (a.min(b), a.max(b));
        pairs.insert((all[first].key, all[second].key));
    };
    for (a, first) in all.iter().enumerate() {
        for (b, second) in all.iter().enumerate().skip(a + 1) {
            if patterns_overlap(&first.kind, &second.kind) {
                add(a, b);
            }
        }
    }
    for (special_at, special) in all.iter().enumerate() {
        if slots(&special.kind).is_some() {
            continue;
        }
        let within = universe(&special.kind);
        for (at, pattern) in all.iter().enumerate() {
            let Some((width, height, grid)) = pattern_grid(&pattern.kind, &within) else {
                continue;
            };
            if matches(&special.kind, &Input::of(width, height, &grid)) {
                add(special_at, at);
            }
        }
        if let Some((width, height, grid)) = example_grid(&special.kind) {
            let input = Input::of(width, height, &grid);
            assert!(
                matches(&special.kind, &input),
                "{}'s example grid matches it",
                special.key
            );
            for (at, other) in all.iter().enumerate() {
                if at != special_at && matches(&other.kind, &input) {
                    add(special_at, at);
                }
            }
        }
    }
    pairs
}

/// Prints the pairs (`cargo test -p clustine-sim recipes_one_grid -- --nocapture`), which
/// the comparison with the official server is to try, and holds them, so that a change of
/// the data is noticed. In 26.3 there are two, and in both the special recipe comes
/// first by key and makes what its plain twin would: four bricks a plain pot (the
/// shaped `decorated_pot_simple`), and paper with one gunpowder three rockets of flight 1
/// (the shapeless `firework_rocket_simple`). So the registry's order, if it is not the
/// keys', changes no result of the game's data.
#[test]
fn the_recipes_one_grid_can_match_are_listed_for_the_comparison() {
    // The comparison of two patterns finds what it should: a recipe overlaps itself and
    // its mirror, and not a recipe of another box.
    let kind = |name: &str| {
        &recipes::crafting()
            .iter()
            .find(|recipe| recipe.key == name)
            .expect("the recipe is in the table")
            .kind
    };
    assert!(patterns_overlap(kind("wooden_axe"), kind("wooden_axe")));
    assert!(patterns_overlap(kind("purple_dye"), kind("purple_dye")));
    assert!(!patterns_overlap(kind("stick"), kind("crafting_table")));
    assert!(!patterns_overlap(kind("wooden_axe"), kind("wooden_hoe")));

    let pairs = overlapping_pairs();
    for (first, second) in &pairs {
        println!("{first} before {second}");
    }
    println!("{} pairs", pairs.len());
    assert_eq!(
        pairs.into_iter().collect::<Vec<_>>(),
        [
            ("decorated_pot", "decorated_pot_simple"),
            ("firework_rocket", "firework_rocket_simple"),
        ]
    );
    // Both make the same as their shaped twin.
    let brick = items::BRICK;
    let bricks = grid([[0, brick, 0], [brick, 0, brick], [0, brick, 0]]);
    let simple = |name: &str| {
        recipes::crafting()
            .iter()
            .position(|recipe| recipe.key == name)
            .map(|at| RecipeId(u16::try_from(at).expect("a recipe's place fits 16 bits")))
            .expect("the recipe is in the table")
    };
    let input = Input::of(3, 3, &bricks);
    assert_eq!(
        assemble(simple("decorated_pot_simple").recipe(), &input),
        assemble(simple("decorated_pot").recipe(), &input)
    );
    let rocket = [stack(items::PAPER), stack(items::GUNPOWDER)];
    let input = Input::of(2, 1, &rocket);
    assert!(matches(
        &simple("firework_rocket_simple").recipe().kind,
        &input
    ));
    assert_eq!(
        assemble(simple("firework_rocket_simple").recipe(), &input),
        assemble(simple("firework_rocket").recipe(), &input)
    );
}
