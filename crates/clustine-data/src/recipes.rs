//! The game's recipes, compiled by `cargo datagen` from `data/minecraft/recipe/*.json`
//! (`docs/groundwork/v-plan.md`, the V3 step): every recipe of the fourteen crafting
//! serialisers (`RecipeSerializers` @11-121), the cooking kinds', the stonecutter's and
//! the smithing table's; the dye colour of each item with `minecraft:dye`; and what
//! `ClientboundUpdateRecipesPacket` carries of them (`RecipeManager.finalizeRecipeLoading`):
//! the property sets and the stonecutter's selectable recipes.
//!
//! Matching a crafting grid is `clustine-sim`'s (`crafting.rs`), since it reads and
//! writes item components. The order of [`crafting`] is the registry's, which a grid
//! tries its recipes in (`RecipeMap.getRecipesFor`'s `findFirst`); it is believed to be
//! by key and is not proven (`tools/datagen/src/recipes.rs`).

use crate::generated::recipes::{BREWING, CRAFTING, DYES, INGREDIENTS, SINGLE_INPUT, SMITHING};

/// An `Ingredient`: the items its `HolderSet` holds, by id, sorted, and the tag it
/// names where it is one (the wire sends a tag by name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ingredient {
    pub tag: Option<&'static str>,
    pub items: &'static [u16],
}

impl Ingredient {
    /// `Ingredient.test`: whether the item is in the set. Components do not count.
    pub fn test(&self, item: i32) -> bool {
        u16::try_from(item).is_ok_and(|item| self.items.binary_search(&item).is_ok())
    }
}

/// `CraftingBookCategory`: the recipe book's tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Building,
    Redstone,
    Equipment,
    Misc,
}

/// A recipe's `ItemStackTemplate`: the item, the count, and the one component a result
/// sets in 26.3, `minecraft:suspicious_stew_effects` (each an effect by name and its
/// duration in ticks), which the generator checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub item: u16,
    pub count: u8,
    pub stew_effects: &'static [(&'static str, i32)],
}

/// An index into the shared ingredients, [`ingredient`].
pub type IngredientId = u16;

/// What one crafting serialiser reads, its ingredients by [`IngredientId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crafting {
    /// `crafting_shaped`, its pattern shrunk to its used box (`ShapedRecipePattern.
    /// shrink`). `cells` are row by row, each an [`IngredientId`] plus one, or 0 for a
    /// cell that has to be empty ([`Crafting::cell`]).
    Shaped {
        width: u8,
        height: u8,
        cells: &'static [u16],
        result: Output,
    },
    Shapeless {
        ingredients: &'static [IngredientId],
        result: Output,
    },
    /// `crafting_transmute`: the input and from `min` to `max` materials, the result
    /// the input's own item where `item` is none, its count `count` (and the materials'
    /// count where `add_material_count` holds).
    Transmute {
        input: IngredientId,
        material: IngredientId,
        min: u8,
        max: u8,
        add_material_count: bool,
        item: Option<u16>,
        count: u8,
    },
    /// `crafting_dye`: leather and wolf armour dyed.
    Dye {
        target: IngredientId,
        dye: IngredientId,
        result: Output,
    },
    /// `crafting_imbue`: tipped arrows.
    Imbue {
        source: IngredientId,
        material: IngredientId,
        result: Output,
    },
    DecoratedPot {
        back: IngredientId,
        left: IngredientId,
        right: IngredientId,
        front: IngredientId,
        result: Output,
    },
    BookCloning {
        source: IngredientId,
        material: IngredientId,
        result: Output,
    },
    MapExtending {
        map: IngredientId,
        material: IngredientId,
    },
    FireworkRocket {
        shell: IngredientId,
        fuel: IngredientId,
        star: IngredientId,
        result: Output,
    },
    /// `crafting_special_firework_star`, `shapes` each a `FireworkExplosion$Shape` by
    /// name and the ingredient that gives it.
    FireworkStar {
        fuel: IngredientId,
        dye: IngredientId,
        trail: IngredientId,
        twinkle: IngredientId,
        shapes: &'static [(&'static str, IngredientId)],
        result: Output,
    },
    FireworkStarFade {
        target: IngredientId,
        dye: IngredientId,
        result: Output,
    },
    BannerDuplicate {
        banner: IngredientId,
        result: Output,
    },
    ShieldDecoration {
        banner: IngredientId,
        target: IngredientId,
        result: Output,
    },
    RepairItem,
}

impl Crafting {
    /// The ingredient of a shaped recipe's cell, row by row; `None` for one that has to
    /// be empty, or for a recipe of another kind.
    pub fn cell(&self, at: usize) -> Option<&'static Ingredient> {
        match self {
            Crafting::Shaped { cells, .. } => match cells.get(at)? {
                0 => None,
                id => Some(ingredient(id - 1)),
            },
            _ => None,
        }
    }
}

/// A crafting recipe: its key and group without `minecraft:`, its book category,
/// whether learning it shows a toast (`show_notification`), and its kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CraftingRecipe {
    pub key: &'static str,
    pub group: &'static str,
    pub category: Category,
    pub show_notification: bool,
    pub kind: Crafting,
}

/// The kind of a recipe of one input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingleKind {
    Smelting,
    Blasting,
    Smoking,
    Campfire,
    Stonecutting,
}

/// A recipe of one input (`SingleItemRecipe`): a cooking kind's or the stonecutter's.
/// Its key, cooking time and experience are not compiled yet, to stay within the budget
/// of generated output (ADR-0019, section 6); the furnaces will need the last two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingleInput {
    pub kind: SingleKind,
    pub input: IngredientId,
    pub result: Output,
}

/// A smithing recipe, a transform's or a trim's: its three ingredients, the template and
/// the addition optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Smithing {
    pub key: &'static str,
    pub trim: bool,
    pub template: Option<IngredientId>,
    pub base: IngredientId,
    pub addition: Option<IngredientId>,
}

/// The shared ingredient `id`.
pub fn ingredient(id: IngredientId) -> &'static Ingredient {
    &INGREDIENTS[usize::from(id)]
}

/// The crafting recipes in the registry's order: a recipe's index is its place.
pub fn crafting() -> &'static [CraftingRecipe] {
    &CRAFTING
}

/// The cooking kinds' and the stonecutter's recipes, in the registry's order.
pub fn single_input() -> &'static [SingleInput] {
    &SINGLE_INPUT
}

/// The smithing recipes, in the registry's order.
pub fn smithing() -> &'static [Smithing] {
    &SMITHING
}

/// The colour `minecraft:dye` gives the item by default, as `DyeColor`'s id (white 0 to
/// black 15): the sixteen dyes.
pub fn dye_colour(item: i32) -> Option<u8> {
    let item = u16::try_from(item).ok()?;
    DYES.iter()
        .find(|(dye, _)| *dye == item)
        .map(|(_, colour)| *colour)
}

/// The property sets `ClientboundUpdateRecipesPacket` carries
/// (`RecipeManager.finalizeRecipeLoading` with its `RECIPE_PROPERTY_SETS`), each its key
/// and its items by id: the furnace's, blast furnace's, smoker's and campfire's inputs
/// (each `SingleItemRecipe.input` of its kind), the smithing table's templates, bases
/// and additions, and the brewing stand's inputs and reagents (the items of each
/// `BrewingRecipe`'s input and reagent `PotionIngredient`).
///
/// The game keeps the sets in an immutable map and each set's items in a `Set`, whose
/// orders are its hash's; here they are in `RECIPE_PROPERTY_SETS`'s order and by id, which
/// the client, reading them into a map and sets, cannot tell apart. Every ingredient is
/// taken as enabled (`filterDisabled`), as the game's own features are.
pub fn property_sets() -> Vec<(&'static str, Vec<i32>)> {
    let cooking = |kind: SingleKind| -> Vec<i32> {
        gathered(
            SINGLE_INPUT
                .iter()
                .filter(|recipe| recipe.kind == kind)
                .map(|recipe| recipe.input),
        )
    };
    vec![
        (
            "minecraft:smithing_addition",
            gathered(SMITHING.iter().filter_map(|recipe| recipe.addition)),
        ),
        (
            "minecraft:smithing_base",
            gathered(SMITHING.iter().map(|recipe| recipe.base)),
        ),
        (
            "minecraft:smithing_template",
            gathered(SMITHING.iter().filter_map(|recipe| recipe.template)),
        ),
        ("minecraft:furnace_input", cooking(SingleKind::Smelting)),
        (
            "minecraft:blast_furnace_input",
            cooking(SingleKind::Blasting),
        ),
        ("minecraft:smoker_input", cooking(SingleKind::Smoking)),
        ("minecraft:campfire_input", cooking(SingleKind::Campfire)),
        (
            "minecraft:brewing_input",
            gathered(BREWING.iter().map(|(input, _)| *input)),
        ),
        (
            "minecraft:brewing_reagent",
            gathered(BREWING.iter().map(|(_, reagent)| *reagent)),
        ),
    ]
}

/// The items of `ingredients` together, each once, by id (`RecipePropertySet.create`).
fn gathered(ingredients: impl Iterator<Item = IngredientId>) -> Vec<i32> {
    let mut items: Vec<i32> = ingredients
        .flat_map(|id| ingredient(id).items.iter().map(|item| i32::from(*item)))
        .collect();
    items.sort_unstable();
    items.dedup();
    items
}

/// One of the stonecutter's selectable recipes (`SelectableRecipe$SingleInputEntry`):
/// its input and the stack its button shows (`StonecutterRecipe.resultDisplay`, an
/// `item_stack` slot display of the result).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StonecutterEntry {
    pub input: &'static Ingredient,
    pub item: u16,
    pub count: u8,
}

/// The stonecutter's selectable recipes `ClientboundUpdateRecipesPacket` carries, in the
/// registry's order (`finalizeRecipeLoading` @61-141: each `StonecutterRecipe` whose
/// input and result are enabled, which every one is).
pub fn stonecutter_entries() -> Vec<StonecutterEntry> {
    SINGLE_INPUT
        .iter()
        .filter(|recipe| recipe.kind == SingleKind::Stonecutting)
        .map(|recipe| StonecutterEntry {
            input: ingredient(recipe.input),
            item: recipe.result.item,
            count: recipe.result.count,
        })
        .collect()
}

// The constructors the generated table is written with, so that a thousand recipes take
// a line each.

/// An ingredient of listed items.
pub const fn items(items: &'static [u16]) -> Ingredient {
    Ingredient { tag: None, items }
}

/// An ingredient that is the item tag `tag`, its items resolved.
pub const fn tagged(tag: &'static str, items: &'static [u16]) -> Ingredient {
    Ingredient {
        tag: Some(tag),
        items,
    }
}

/// A result without components.
pub const fn out(item: u16, count: u8) -> Output {
    Output {
        item,
        count,
        stew_effects: &[],
    }
}

/// A result with `minecraft:suspicious_stew_effects`.
pub const fn stew(item: u16, count: u8, effects: &'static [(&'static str, i32)]) -> Output {
    Output {
        item,
        count,
        stew_effects: effects,
    }
}

/// A shaped recipe whose result has no components and that shows a toast when learnt.
#[expect(
    clippy::too_many_arguments,
    reason = "a line of the generated table, which is read as the recipe's fields in order"
)]
pub const fn shaped(
    key: &'static str,
    group: &'static str,
    category: Category,
    width: u8,
    height: u8,
    cells: &'static [u16],
    item: u16,
    count: u8,
) -> CraftingRecipe {
    special(
        key,
        group,
        category,
        Crafting::Shaped {
            width,
            height,
            cells,
            result: out(item, count),
        },
    )
}

/// A shapeless recipe whose result has no components and that shows a toast when
/// learnt.
pub const fn shapeless(
    key: &'static str,
    group: &'static str,
    category: Category,
    ingredients: &'static [IngredientId],
    item: u16,
    count: u8,
) -> CraftingRecipe {
    special(
        key,
        group,
        category,
        Crafting::Shapeless {
            ingredients,
            result: out(item, count),
        },
    )
}

/// A recipe of any kind that shows a toast when learnt.
pub const fn special(
    key: &'static str,
    group: &'static str,
    category: Category,
    kind: Crafting,
) -> CraftingRecipe {
    CraftingRecipe {
        key,
        group,
        category,
        show_notification: true,
        kind,
    }
}

/// `recipe` without its toast (`show_notification: false`).
pub const fn quiet(recipe: CraftingRecipe) -> CraftingRecipe {
    CraftingRecipe {
        show_notification: false,
        ..recipe
    }
}

/// A recipe of one input, its result without components.
pub const fn single(kind: SingleKind, input: IngredientId, item: u16, count: u8) -> SingleInput {
    SingleInput {
        kind,
        input,
        result: out(item, count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items;

    fn count(kind: impl Fn(&Crafting) -> bool) -> usize {
        crafting()
            .iter()
            .filter(|recipe| kind(&recipe.kind))
            .count()
    }

    /// The counts of each type in the jar's `data/minecraft/recipe` (26.3).
    #[test]
    fn every_recipe_of_the_jar_is_compiled_by_its_kind() {
        assert_eq!(count(|k| matches!(k, Crafting::Shaped { .. })), 827);
        assert_eq!(count(|k| matches!(k, Crafting::Shapeless { .. })), 375);
        assert_eq!(count(|k| matches!(k, Crafting::Transmute { .. })), 33);
        assert_eq!(count(|k| matches!(k, Crafting::Dye { .. })), 6);
        assert_eq!(count(|k| matches!(k, Crafting::Imbue { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::DecoratedPot { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::BookCloning { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::MapExtending { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::FireworkRocket { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::FireworkStar { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::FireworkStarFade { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::BannerDuplicate { .. })), 16);
        assert_eq!(count(|k| matches!(k, Crafting::ShieldDecoration { .. })), 1);
        assert_eq!(count(|k| matches!(k, Crafting::RepairItem)), 1);
        assert_eq!(crafting().len(), 1266);
        let single = |kind: SingleKind| single_input().iter().filter(|r| r.kind == kind).count();
        assert_eq!(single(SingleKind::Smelting), 73);
        assert_eq!(single(SingleKind::Blasting), 25);
        assert_eq!(single(SingleKind::Smoking), 9);
        assert_eq!(single(SingleKind::Campfire), 9);
        assert_eq!(single(SingleKind::Stonecutting), 351);
        assert_eq!(smithing().iter().filter(|r| !r.trim).count(), 12);
        assert_eq!(smithing().iter().filter(|r| r.trim).count(), 18);
    }

    #[test]
    fn the_recipes_are_in_the_order_of_their_keys() {
        assert!(crafting().is_sorted_by(|a, b| a.key < b.key));
        assert!(smithing().is_sorted_by(|a, b| a.key < b.key));
    }

    #[test]
    fn a_shaped_recipe_names_its_tag_and_its_empty_cells() {
        let pickaxe = crafting()
            .iter()
            .find(|recipe| recipe.key == "wooden_pickaxe")
            .expect("the wooden pickaxe is crafted");
        let Crafting::Shaped {
            width,
            height,
            result,
            ..
        } = pickaxe.kind
        else {
            panic!("a shaped recipe");
        };
        assert_eq!((width, height), (3, 3));
        assert_eq!(i32::from(result.item), items::WOODEN_PICKAXE);
        assert_eq!(pickaxe.category, Category::Equipment);
        let head = pickaxe.kind.cell(0).expect("the head is a material");
        assert_eq!(head.tag, Some("minecraft:wooden_tool_materials"));
        assert!(head.test(items::OAK_PLANKS));
        assert!(pickaxe.kind.cell(3).is_none());
        assert!(pickaxe.kind.cell(4).expect("a stick").test(items::STICK));
    }

    #[test]
    fn only_the_crafting_table_learns_without_a_toast() {
        let quiet: Vec<&str> = crafting()
            .iter()
            .filter(|recipe| !recipe.show_notification)
            .map(|recipe| recipe.key)
            .collect();
        assert_eq!(quiet, ["crafting_table"]);
    }

    #[test]
    fn the_sixteen_dyes_have_their_colours() {
        assert_eq!(dye_colour(items::WHITE_DYE), Some(0));
        assert_eq!(dye_colour(items::LIME_DYE), Some(5));
        assert_eq!(dye_colour(items::BLACK_DYE), Some(15));
        assert_eq!(dye_colour(items::BONE_MEAL), None);
        assert_eq!(DYES.len(), 16);
    }

    #[test]
    fn the_property_sets_gather_each_kind_s_inputs() {
        let sets = property_sets();
        let set = |key: &str| {
            sets.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, items)| items.clone())
                .expect("the set is sent")
        };
        assert_eq!(sets.len(), 9);
        assert!(set("minecraft:furnace_input").contains(&items::IRON_ORE));
        assert!(set("minecraft:furnace_input").contains(&items::IRON_PICKAXE));
        assert!(!set("minecraft:smoker_input").contains(&items::IRON_ORE));
        assert!(set("minecraft:smoker_input").contains(&items::BEEF));
        assert!(set("minecraft:campfire_input").contains(&items::POTATO));
        assert!(set("minecraft:blast_furnace_input").contains(&items::RAW_GOLD));
        assert!(
            set("minecraft:smithing_template")
                .contains(&items::NETHERITE_UPGRADE_SMITHING_TEMPLATE)
        );
        assert!(set("minecraft:smithing_base").contains(&items::DIAMOND_SWORD));
        assert!(set("minecraft:smithing_addition").contains(&items::NETHERITE_INGOT));
        assert_eq!(
            set("minecraft:brewing_input"),
            [items::POTION, items::SPLASH_POTION, items::LINGERING_POTION]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert!(set("minecraft:brewing_reagent").contains(&items::NETHER_WART));
        assert_eq!(BREWING.len(), 279);
        for (_, items) in &sets {
            assert!(items.is_sorted_by(|a, b| a < b));
        }
    }

    #[test]
    fn the_stonecutter_offers_every_stonecutting_recipe_with_its_result() {
        let entries = stonecutter_entries();
        assert_eq!(entries.len(), 351);
        let slab = entries
            .iter()
            .find(|entry| {
                entry.input.test(items::ANDESITE) && i32::from(entry.item) == items::ANDESITE_SLAB
            })
            .expect("andesite is cut into slabs");
        assert_eq!(slab.count, 2);
        assert_eq!(slab.input.tag, None);
    }
}
