//! Crafting: which recipe a grid makes and what it leaves behind
//! (`docs/groundwork/v-plan.md`, the V3 step, V3b). A pure function of the grid, over
//! `clustine-data`'s generated recipes, which the inventory screen's 2×2 grid and the
//! crafting table call after every change of their grid.
//!
//! As the game, every lookup tries the recipes of the crafting type in the registry's
//! order and takes the first that matches (`RecipeManager.getRecipeFor`, `RecipeMap.
//! getRecipesFor`'s `findFirst`): a crafting grid has no cached check (`InventoryMenu.
//! slotsChanged` @34 and `CraftingMenu` pass no recipe to `slotChangedCraftingGrid`).
//! Each of the fourteen crafting serialisers is ported from its class (`v3-javap`).
//!
//! Where Clustine's item stacks cannot hold a component a recipe makes, the recipe makes
//! nothing rather than a wrong item (a grid it is the first match of gives no result):
//! - `crafting_special_firework_star` and `crafting_special_firework_star_fade` set
//!   `minecraft:firework_explosion` on the star, which Clustine does not hold;
//! - `crafting_special_shielddecoration` sets `minecraft:base_color` on the shield;
//! - `crafting_decorated_pot` sets `minecraft:pot_decorations`; only four plain bricks,
//!   whose decorations are the pot's default (`PotDecorations.allBrick`, `Items.<clinit>`
//!   lambda 89), make a pot.
//!
//! And where a recipe reads a component Clustine does not hold, it reads it as absent,
//! as the game reads a stack that lacks it:
//! - `crafting_special_bookcloning` matches only a written book with
//!   `minecraft:written_book_content`, which no Clustine book has; it matches nothing;
//! - `crafting_special_mapextending` matches only a map with saved data, and Clustine has
//!   no maps; it matches nothing. (Map cloning is a transmute and clones what a filled map
//!   holds, which in Clustine is no map id.)
//! - a firework rocket's stars give their `minecraft:firework_explosion`, which Clustine's
//!   stars never have: a rocket flies, and does not burst.
//!
//! `isItemEnabled` (`slotChangedCraftingGrid`) is taken to hold for every item: Clustine
//! enables no experimental feature.

use clustine_data::recipes::{self, Crafting, CraftingRecipe, Ingredient, IngredientId, Output};
use clustine_data::{ITEMS, ItemClass, ItemComponents, TAGS, items};
use clustine_world::components::{Fireworks, Kind, PotionContents, in_registry_order};
use clustine_world::{Component, ItemStack};

use crate::items::enchantments::damage;
use crate::trading::evaluate::mixed_colour;

/// A recipe of the crafting type: its index in the generated table, which is the
/// registry's order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct RecipeId(pub u16);

impl RecipeId {
    /// The recipe.
    pub fn recipe(self) -> &'static CraftingRecipe {
        &recipes::crafting()[usize::from(self.0)]
    }
}

/// `RecipeManager.getRecipeFor(CRAFTING, CraftingInput.of(width, height, grid))`: the
/// first recipe in the registry's order that matches, and what it assembles. `grid` is
/// `width * height` slots, row by row; it is trimmed to its used box as the game does.
///
/// `None` where no recipe matches, or where the first that matches assembles nothing
/// (`slotChangedCraftingGrid` then leaves the result empty) or makes what Clustine
/// cannot hold (the module's notes).
pub fn craft(
    width: usize,
    height: usize,
    grid: &[Option<ItemStack>],
) -> Option<(RecipeId, ItemStack)> {
    let input = Input::of(width, height, grid);
    let id = first_match(&input)?;
    let result = assemble(id.recipe(), &input)?;
    Some((id, result))
}

/// `ResultSlot.getRemainingItems` for the same grid: for each of the `width * height`
/// slots (untrimmed, in the grid's own positions) what is left in it after one craft
/// besides the ingredient taken: the recipe's remainders, and with no matching recipe
/// copies of the inputs (`copyAllInputItems`).
///
/// The game's list then holds the grid's own stacks, not copies, which matters only to
/// how `ResultSlot.onTake` applies it; here they are clones, whole.
pub fn remainders(
    width: usize,
    height: usize,
    grid: &[Option<ItemStack>],
) -> Vec<Option<ItemStack>> {
    let input = Input::of(width, height, grid);
    let list = match first_match(&input) {
        Some(id) => remaining_items(id.recipe(), &input),
        None => input.items.iter().map(|stack| stack.cloned()).collect(),
    };
    // `ResultSlot.onTake` @45-265: the list is the trimmed box's, placed at its corner.
    let mut out = vec![None; width * height];
    for y in 0..input.height {
        for x in 0..input.width {
            out[input.left + x + (input.top + y) * width] = list[x + y * input.width].clone();
        }
    }
    out
}

/// `CraftingInput.Positioned`: the grid trimmed to the box of its non-empty slots, and
/// where that box's corner is in the grid.
#[derive(Debug)]
pub(crate) struct Input<'a> {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Row by row; `None` for an empty slot.
    pub(crate) items: Vec<Option<&'a ItemStack>>,
    left: usize,
    top: usize,
    /// The non-empty slots.
    pub(crate) ingredient_count: usize,
}

impl<'a> Input<'a> {
    /// `CraftingInput.ofPositioned` @0-294; `EMPTY` (no slots, at 0, 0) for a grid with
    /// nothing in it.
    pub(crate) fn of(width: usize, height: usize, grid: &'a [Option<ItemStack>]) -> Input<'a> {
        let at = |x: usize, y: usize| -> Option<&'a ItemStack> {
            grid.get(x + y * width)
                .and_then(Option::as_ref)
                .filter(|stack| !is_empty(stack))
        };
        let (mut left, mut right, mut top, mut bottom) = (usize::MAX, 0, usize::MAX, 0);
        for y in 0..height {
            for x in 0..width {
                if at(x, y).is_some() {
                    left = left.min(x);
                    right = right.max(x);
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
            }
        }
        if left == usize::MAX {
            return Input {
                width: 0,
                height: 0,
                items: Vec::new(),
                left: 0,
                top: 0,
                ingredient_count: 0,
            };
        }
        let (box_width, box_height) = (right - left + 1, bottom - top + 1);
        let mut items = Vec::with_capacity(box_width * box_height);
        for y in top..=bottom {
            for x in left..=right {
                items.push(at(x, y));
            }
        }
        let ingredient_count = items.iter().flatten().count();
        Input {
            width: box_width,
            height: box_height,
            items,
            left,
            top,
            ingredient_count,
        }
    }

    /// `getItem(x, y)`.
    fn at(&self, x: usize, y: usize) -> Option<&'a ItemStack> {
        self.items[x + y * self.width]
    }

    /// The non-empty stacks, row by row.
    fn stacks(&self) -> impl Iterator<Item = &'a ItemStack> + '_ {
        self.items.iter().flatten().copied()
    }
}

/// `ItemStack.isEmpty`: air, or a count below one.
fn is_empty(stack: &ItemStack) -> bool {
    stack.item == items::AIR || stack.count <= 0
}

/// `RecipeMap.getRecipesFor(CRAFTING, input).findFirst()`: nothing for an empty input
/// (`RecipeInput.isEmpty`), else the first recipe in the registry's order that matches.
pub(crate) fn first_match(input: &Input) -> Option<RecipeId> {
    if input.ingredient_count == 0 {
        return None;
    }
    recipes::crafting()
        .iter()
        .position(|recipe| matches(&recipe.kind, input))
        .map(|at| RecipeId(u16::try_from(at).expect("fewer than 65,536 recipes")))
}

fn ingredient(id: IngredientId) -> &'static Ingredient {
    recipes::ingredient(id)
}

/// `Ingredient.test` of a stack.
fn test(id: IngredientId, stack: &ItemStack) -> bool {
    ingredient(id).test(stack.item)
}

/// `Ingredient.testOptionalIngredient`: an empty cell takes only an empty slot.
fn test_cell(cell: Option<&Ingredient>, stack: Option<&ItemStack>) -> bool {
    match (cell, stack) {
        (None, stack) => stack.is_none(),
        (Some(ingredient), Some(stack)) => ingredient.test(stack.item),
        (Some(_), None) => false,
    }
}

/// `has(DataComponents.DYE)`: the dyes have it by default, and no patch Clustine holds
/// adds or removes it.
fn carries_dye(stack: &ItemStack) -> bool {
    recipes::dye_colour(stack.item).is_some()
}

/// `BannerItem.getColor`, for an item of `BannerItem`'s class: its name's colour.
fn banner_colour(item: i32) -> Option<&'static str> {
    let info = ITEMS.get(usize::try_from(item).ok()?)?;
    if info.class != ItemClass::BannerItem {
        return None;
    }
    info.name
        .strip_prefix("minecraft:")?
        .strip_suffix("_banner")
}

/// `getOrDefault(BANNER_PATTERNS, EMPTY).layers().size()`: the patch's layers; none for a
/// patch that removes them, or by default (a banner's and a shield's default is empty).
fn banner_layers(stack: &ItemStack) -> usize {
    match stack.components.get(Kind::BannerPatterns) {
        Some(Component::BannerPatterns(layers)) => layers.len(),
        _ => 0,
    }
}

/// Whether `recipe` matches `input`, as the game's `matches` of its class.
pub(crate) fn matches(recipe: &Crafting, input: &Input) -> bool {
    match *recipe {
        Crafting::Shaped { width, height, .. } => {
            shaped(recipe, usize::from(width), usize::from(height), input)
        }
        Crafting::Shapeless { ingredients, .. } => shapeless(ingredients, input),
        Crafting::Transmute {
            input: wanted,
            material,
            min,
            max,
            ..
        } => transmute_input(wanted, material, min, max, input)
            .is_some_and(|(stack, materials)| transmute_matches(recipe, stack, materials)),
        Crafting::Dye { target, dye, .. } => dye_matches(target, dye, input),
        Crafting::Imbue {
            source, material, ..
        } => imbue_matches(source, material, input),
        Crafting::DecoratedPot {
            back,
            left,
            right,
            front,
            ..
        } => pot_matches([back, left, right, front], input),
        // `BookCloningRecipe.matches` @0-139 asks for a source with
        // `minecraft:written_book_content` (@58-86), which no Clustine book has, and ends
        // there as the game ends for a book without it; without a source, never.
        Crafting::BookCloning { .. } => false,
        // `MapExtendingRecipe.matches` asks for the centre map's saved data (its scale
        // below 4, not an explorer's map), and Clustine has no maps.
        Crafting::MapExtending { .. } => false,
        Crafting::FireworkRocket {
            shell, fuel, star, ..
        } => rocket_matches(shell, fuel, star, input),
        Crafting::FireworkStar {
            fuel,
            dye,
            trail,
            twinkle,
            shapes,
            ..
        } => star_matches(fuel, dye, trail, twinkle, shapes, input),
        Crafting::FireworkStarFade { target, dye, .. } => fade_matches(target, dye, input),
        Crafting::BannerDuplicate { banner, .. } => {
            banner_duplicate_source(banner, input).is_some()
        }
        Crafting::ShieldDecoration { banner, target, .. } => shield_matches(banner, target, input),
        Crafting::RepairItem => repair_pair(input).is_some(),
    }
}

/// `ShapedRecipePattern.matches` @0-65: the same count of ingredients, the same box, and
/// the cells matched as they are or mirrored. The game skips the mirror of a symmetrical
/// pattern (`Util.isSymmetrical`), which can only give the same answer.
fn shaped(recipe: &Crafting, width: usize, height: usize, input: &Input) -> bool {
    let cells = width * height;
    let count = (0..cells).filter(|at| recipe.cell(*at).is_some()).count();
    if input.ingredient_count != count || input.width != width || input.height != height {
        return false;
    }
    // `matches(input, mirrored)` @0-116.
    let oriented = |mirrored: bool| {
        (0..height).all(|y| {
            (0..width).all(|x| {
                let column = if mirrored { width - x - 1 } else { x };
                test_cell(recipe.cell(column + y * width), input.at(x, y))
            })
        })
    };
    oriented(true) || oriented(false)
}

/// `ShapelessRecipe.matches` @0-71: as many stacks as ingredients, and one ingredient for
/// each stack: a 1×1 input against its one ingredient, else `StackedItemContents.
/// canCraft`, which finds a matching of the stacks (each counted once, `CraftingInput.<init>`'s
/// `accountStack(stack, 1)`) to the ingredients whatever their order. Its own search
/// (`StackedContents`, not dumped) is taken to find one wherever one exists, as a
/// matching by augmenting paths does.
fn shapeless(ingredients: &[IngredientId], input: &Input) -> bool {
    if input.ingredient_count != ingredients.len() {
        return false;
    }
    if input.items.len() == 1 && ingredients.len() == 1 {
        return input.items[0].is_some_and(|stack| test(ingredients[0], stack));
    }
    let stacks: Vec<&ItemStack> = input.stacks().collect();
    perfect_matching(ingredients.len(), stacks.len(), |ingredient, stack| {
        test(ingredients[ingredient], stacks[stack])
    })
}

/// Whether each of `left` things can be paired with its own one of `right` things, where
/// `fits(l, r)` says which pairs may be made (Kuhn's augmenting paths, in index order).
pub(crate) fn perfect_matching(
    left: usize,
    right: usize,
    fits: impl Fn(usize, usize) -> bool,
) -> bool {
    if left != right {
        return false;
    }
    fn augment(
        at: usize,
        right: usize,
        fits: &dyn Fn(usize, usize) -> bool,
        seen: &mut [bool],
        owner: &mut [Option<usize>],
    ) -> bool {
        for to in 0..right {
            if seen[to] || !fits(at, to) {
                continue;
            }
            seen[to] = true;
            if owner[to].is_none_or(|other| augment(other, right, fits, seen, owner)) {
                owner[to] = Some(at);
                return true;
            }
        }
        false
    }
    let mut owner = vec![None; right];
    (0..left).all(|at| augment(at, right, &fits, &mut vec![false; right], &mut owner))
}

/// `TransmuteRecipe.matches` @0-129: from `min + 1` to `max + 1` stacks, one of them the
/// input and every other a material, at most `max` of them. The input's stack and the
/// materials' count; `None` where it does not match so far.
fn transmute_input<'a>(
    wanted: IngredientId,
    material: IngredientId,
    min: u8,
    max: u8,
    input: &Input<'a>,
) -> Option<(&'a ItemStack, usize)> {
    let (min, max) = (usize::from(min), usize::from(max));
    if input.ingredient_count < min + 1 || input.ingredient_count > max + 1 {
        return None;
    }
    let mut found = None;
    let mut materials = 0;
    for stack in input.stacks() {
        if test(wanted, stack) {
            if found.is_some() {
                return None;
            }
            found = Some(stack);
        } else if test(material, stack) {
            materials += 1;
            if materials > max {
                return None;
            }
        } else {
            return None;
        }
    }
    // @129-154: the material count within its bounds.
    let found = found?;
    (min..=max)
        .contains(&materials)
        .then_some((found, materials))
}

/// `TransmuteRecipe.matches` @156-206: a result of one has to differ from the input
/// (`isSameItemSameComponents`), so that a box is not dyed the colour it is.
fn transmute_matches(recipe: &Crafting, stack: &ItemStack, materials: usize) -> bool {
    let Crafting::Transmute {
        add_material_count,
        count,
        ..
    } = *recipe
    else {
        return false;
    };
    let size = transmute_size(add_material_count, count, materials);
    if size != 1 {
        return true;
    }
    let result = transmute_result(recipe, stack, 0);
    !is_empty(&result) && !result.same_kind(stack)
}

/// `computeResultSize`.
fn transmute_size(add_material_count: bool, count: u8, materials: usize) -> i32 {
    let materials = i32::try_from(materials).expect("at most eight materials");
    if add_material_count {
        materials + i32::from(count)
    } else {
        i32::from(count)
    }
}

/// `computeResult(input, extra)`: `createWithOriginalComponents` of the result, its item
/// the input's where the recipe names none (`TransmuteResult.resolve`), its count the
/// result's and `extra`, with the input's components.
fn transmute_result(recipe: &Crafting, stack: &ItemStack, extra: i32) -> ItemStack {
    let Crafting::Transmute { item, count, .. } = *recipe else {
        unreachable!("only a transmute recipe has a transmute result");
    };
    ItemStack {
        item: item.map_or(stack.item, i32::from),
        count: i32::from(count) + extra,
        components: stack.components.clone(),
    }
}

/// `DyeRecipe.matches` @0-120: at least two stacks, one the target and the rest dyes.
fn dye_matches(target: IngredientId, dye: IngredientId, input: &Input) -> bool {
    if input.ingredient_count < 2 {
        return false;
    }
    let (mut has_target, mut has_dye) = (false, false);
    for stack in input.stacks() {
        if test(target, stack) {
            if has_target {
                return false;
            }
            has_target = true;
        } else if test(dye, stack) && carries_dye(stack) {
            has_dye = true;
        } else {
            return false;
        }
    }
    has_target && has_dye
}

/// `ImbueRecipe.matches` @0-117: a full 3×3, the source in the centre and the material
/// round it.
fn imbue_matches(source: IngredientId, material: IngredientId, input: &Input) -> bool {
    if input.width != 3 || input.height != 3 || input.ingredient_count != 9 {
        return false;
    }
    (0..3).all(|y| {
        (0..3).all(|x| {
            let wanted = if (x, y) == (1, 1) { source } else { material };
            input.at(x, y).is_some_and(|stack| test(wanted, stack))
        })
    })
}

/// `DecoratedPotRecipe.matches`: four stacks in a 3×3 box, the back at the top's middle,
/// the left and right at the middle row's ends, the front at the bottom's middle.
fn pot_matches(sides: [IngredientId; 4], input: &Input) -> bool {
    if input.width != 3 || input.height != 3 || input.ingredient_count != 4 {
        return false;
    }
    pot_sides(input)
        .iter()
        .zip(sides)
        .all(|(stack, side)| stack.is_some_and(|stack| test(side, stack)))
}

/// The back, left, right and front stacks of a pot's grid (`back`, `left`, `right`,
/// `front`: `getItem(1, 0)`, `(0, 1)`, `(2, 1)`, `(1, 2)`).
fn pot_sides<'a>(input: &Input<'a>) -> [Option<&'a ItemStack>; 4] {
    [
        input.at(1, 0),
        input.at(0, 1),
        input.at(2, 1),
        input.at(1, 2),
    ]
}

/// `FireworkRocketRecipe.matches` @0-127: one shell, one to three fuels, and any number of
/// stars.
fn rocket_matches(
    shell: IngredientId,
    fuel: IngredientId,
    star: IngredientId,
    input: &Input,
) -> bool {
    if input.ingredient_count < 2 {
        return false;
    }
    let (mut has_shell, mut fuels) = (false, 0);
    for stack in input.stacks() {
        if test(shell, stack) {
            if has_shell {
                return false;
            }
            has_shell = true;
        } else if test(fuel, stack) {
            fuels += 1;
            if fuels > 3 {
                return false;
            }
        } else if !test(star, stack) {
            return false;
        }
    }
    has_shell && fuels >= 1
}

/// `FireworkStarRecipe.matches` @0-205: one fuel and at least one dye, with at most one
/// each of twinkle, trail and shape.
fn star_matches(
    fuel: IngredientId,
    dye: IngredientId,
    trail: IngredientId,
    twinkle: IngredientId,
    shapes: &[(&str, IngredientId)],
    input: &Input,
) -> bool {
    if input.ingredient_count < 2 {
        return false;
    }
    let (mut has_fuel, mut has_dye, mut has_shape, mut has_trail, mut has_twinkle) =
        (false, false, false, false, false);
    for stack in input.stacks() {
        let once = |seen: &mut bool| {
            let first = !*seen;
            *seen = true;
            first
        };
        let fine = if test(twinkle, stack) {
            once(&mut has_twinkle)
        } else if test(trail, stack) {
            once(&mut has_trail)
        } else if test(fuel, stack) {
            once(&mut has_fuel)
        } else if test(dye, stack) && carries_dye(stack) {
            has_dye = true;
            true
        } else if shapes.iter().any(|(_, shape)| test(*shape, stack)) {
            // `findShape`.
            once(&mut has_shape)
        } else {
            false
        };
        if !fine {
            return false;
        }
    }
    has_fuel && has_dye
}

/// `FireworkStarFadeRecipe.matches`: one star and at least one dye.
fn fade_matches(target: IngredientId, dye: IngredientId, input: &Input) -> bool {
    if input.ingredient_count < 2 {
        return false;
    }
    let (mut has_target, mut has_dye) = (false, false);
    for stack in input.stacks() {
        if test(dye, stack) && carries_dye(stack) {
            has_dye = true;
        } else if test(target, stack) {
            if has_target {
                return false;
            }
            has_target = true;
        } else {
            return false;
        }
    }
    has_target && has_dye
}

/// `BannerDuplicateRecipe.matches` @0-191: two banners of one colour, one with one to six
/// layers and one with none. The one with layers, which `assemble` copies (@0-79).
fn banner_duplicate_source<'a>(banner: IngredientId, input: &Input<'a>) -> Option<&'a ItemStack> {
    if input.ingredient_count != 2 {
        return None;
    }
    let mut colour = None;
    let (mut source, mut has_target) = (None, false);
    for stack in input.stacks() {
        if !test(banner, stack) {
            return None;
        }
        let this = banner_colour(stack.item)?;
        if *colour.get_or_insert(this) != this {
            return None;
        }
        let layers = banner_layers(stack);
        if layers > 6 {
            return None;
        }
        if layers > 0 {
            if source.is_some() {
                return None;
            }
            source = Some(stack);
        } else {
            if has_target {
                return None;
            }
            has_target = true;
        }
    }
    source.filter(|_| has_target)
}

/// `ShieldDecorationRecipe.matches`: a banner and a shield without layers.
fn shield_matches(banner: IngredientId, target: IngredientId, input: &Input) -> bool {
    if input.ingredient_count != 2 {
        return false;
    }
    let (mut has_banner, mut has_target) = (false, false);
    for stack in input.stacks() {
        if test(banner, stack) && banner_colour(stack.item).is_some() {
            if has_banner {
                return false;
            }
            has_banner = true;
        } else if test(target, stack) {
            if has_target || banner_layers(stack) > 0 {
                return false;
            }
            has_target = true;
        } else {
            return false;
        }
    }
    has_banner && has_target
}

/// `has(MAX_DAMAGE)`: the item's default, which no Clustine patch changes.
fn has_max_damage(stack: &ItemStack) -> bool {
    ItemComponents::of(stack.item).max_damage.is_some()
}

/// `has(DAMAGE)`: the patch's, else the item's default, unless the patch removes it.
fn has_damage(stack: &ItemStack) -> bool {
    !stack.components.removes(Kind::Damage)
        && (stack.components.get(Kind::Damage).is_some()
            || ItemComponents::of(stack.item).damage.is_some())
}

/// `RepairItemRecipe.getItemsToCombine` @0-72: two stacks, the second combinable with the
/// first (`canCombine`: the same item, one each, both damageable and with damage).
fn repair_pair<'a>(input: &Input<'a>) -> Option<(&'a ItemStack, &'a ItemStack)> {
    if input.ingredient_count != 2 {
        return None;
    }
    let mut stacks = input.stacks();
    let (first, second) = (stacks.next()?, stacks.next()?);
    let combinable = second.item == first.item
        && first.count == 1
        && second.count == 1
        && has_max_damage(first)
        && has_max_damage(second)
        && has_damage(first)
        && has_damage(second);
    combinable.then_some((first, second))
}

/// `ItemStackTemplate.create`: the result, with its `minecraft:suspicious_stew_effects`
/// where it sets them.
fn create(output: &Output) -> ItemStack {
    let mut stack = ItemStack::new(i32::from(output.item), i32::from(output.count));
    if !output.stew_effects.is_empty() {
        stack.components.set(Component::SuspiciousStewEffects(
            output
                .stew_effects
                .iter()
                .map(|(effect, duration)| ((*effect).to_owned(), *duration))
                .collect(),
        ));
    }
    stack
}

/// `assemble` of `recipe`'s class, which matches `input`; `None` for an empty result, or
/// for one Clustine cannot hold (the module's notes).
pub(crate) fn assemble(recipe: &CraftingRecipe, input: &Input) -> Option<ItemStack> {
    let result = match recipe.kind {
        Crafting::Shaped { result, .. } | Crafting::Shapeless { result, .. } => create(&result),
        Crafting::Transmute {
            input: wanted,
            material,
            min,
            max,
            add_material_count,
            ..
        } => {
            // `assemble` @0-140: the input's stack, and the materials counted where the
            // result adds them.
            let (stack, materials) = transmute_input(wanted, material, min, max, input)?;
            let extra = if add_material_count {
                i32::try_from(materials).expect("at most eight materials")
            } else {
                0
            };
            transmute_result(&recipe.kind, stack, extra)
        }
        Crafting::Dye {
            target,
            dye,
            result,
        } => dye_result(target, dye, &result, input)?,
        Crafting::Imbue { result, .. } => {
            // `assemble` @0-34: the result with the centre's `potion_contents`.
            let source = input.at(1, 1)?;
            let mut arrows = create(&result);
            match source.components.get(Kind::PotionContents) {
                Some(Component::PotionContents(contents)) => {
                    set_or_default(&mut arrows, Component::PotionContents(contents.clone()))
                }
                _ if source.components.removes(Kind::PotionContents) => {
                    arrows.components.remove(Kind::PotionContents)
                }
                // The lingering potion's default is empty, and so is the arrow's.
                _ => {}
            }
            arrows
        }
        Crafting::DecoratedPot { result, .. } => {
            // `assemble` sets `pot_decorations` of the four sides' templates; four plain
            // bricks are the pot's default, and anything else Clustine cannot hold.
            let plain_bricks = pot_sides(input).iter().all(|side| {
                side.is_some_and(|stack| stack.item == items::BRICK && stack.components.is_empty())
            });
            if !plain_bricks {
                return None;
            }
            create(&result)
        }
        Crafting::BookCloning { .. } | Crafting::MapExtending { .. } => return None,
        Crafting::FireworkRocket { fuel, result, .. } => {
            // `assemble` @0-136: the fuels make the flight, and the stars' explosions
            // (none in Clustine) the bursts.
            let fuels = input.stacks().filter(|stack| test(fuel, stack)).count();
            let mut rocket = create(&result);
            set_or_default(
                &mut rocket,
                Component::Fireworks(Fireworks {
                    flight_duration: i32::try_from(fuels).expect("at most three fuels"),
                    explosions: Vec::new(),
                }),
            );
            rocket
        }
        Crafting::FireworkStar { .. }
        | Crafting::FireworkStarFade { .. }
        | Crafting::ShieldDecoration { .. } => return None,
        Crafting::BannerDuplicate { banner, result } => {
            // `assemble` @0-79: the result with the patterned banner's components.
            let source = banner_duplicate_source(banner, input)?;
            ItemStack {
                item: i32::from(result.item),
                count: i32::from(result.count),
                components: source.components.clone(),
            }
        }
        Crafting::RepairItem => repaired(input)?,
    };
    (!is_empty(&result)).then_some(result)
}

/// `DyeRecipe.assemble` @0-185: the target with its `dyed_color` mixed with the dyes'
/// (`DyedItemColor.applyDyes`), as `createWithOriginalComponents` of the result.
fn dye_result(
    target: IngredientId,
    dye: IngredientId,
    result: &Output,
    input: &Input,
) -> Option<ItemStack> {
    let mut found = None;
    let mut dyes = Vec::new();
    for stack in input.stacks() {
        if test(target, stack) {
            if found.is_some() {
                return None;
            }
            found = Some(stack);
        } else if test(dye, stack) {
            // `getOrDefault(DYE, WHITE)`.
            let colour = recipes::dye_colour(stack.item).unwrap_or(0);
            dyes.push(TEXTURE_COLOURS[usize::from(colour)]);
        } else {
            return None;
        }
    }
    let found = found?;
    if dyes.is_empty() {
        return None;
    }
    let old = match found.components.get(Kind::DyedColor) {
        Some(Component::DyedColor(rgb)) => Some(*rgb),
        _ => None,
    };
    let mut dyed = ItemStack {
        item: i32::from(result.item),
        count: i32::from(result.count),
        components: found.components.clone(),
    };
    // No dyeable item has a `dyed_color` by default, so the colour is always the patch's.
    dyed.components
        .set(Component::DyedColor(mixed_colour(old, &dyes)));
    Some(dyed)
}

/// `RepairItemRecipe.assemble` @0-152: a new stack of the item, worn by what the two
/// lack together less a twentieth of its most, with the curses of both at the higher of
/// their levels.
fn repaired(input: &Input) -> Option<ItemStack> {
    let (first, second) = repair_pair(input)?;
    let most =
        |stack: &ItemStack| i32::from(ItemComponents::of(stack.item).max_damage.unwrap_or(0));
    let worn = |stack: &ItemStack| damage(stack).clamp(0, most(stack));
    let max = most(first).max(most(second));
    let left = most(first) - worn(first) + most(second) - worn(second) + max * 5 / 100;
    let mut repaired = ItemStack::new(first.item, 1);
    // `set(MAX_DAMAGE, max)` is the item's default, which no patch changes; and
    // `setDamageValue` clamps, and 0 is the default.
    let wear = (max - left).max(0).clamp(0, most(first));
    if wear != 0 {
        repaired.components.set(Component::Damage(wear));
    }
    let mut curses: Vec<(String, i32)> = Vec::new();
    for stack in [first, second] {
        for (name, level) in crafting_enchantments(stack) {
            if !is_curse(&name) {
                continue;
            }
            match curses.iter_mut().find(|(known, _)| *known == name) {
                Some((_, held)) => *held = (*held).max(level),
                None => curses.push((name, level)),
            }
        }
    }
    if !curses.is_empty() {
        repaired
            .components
            .set(Component::Enchantments(in_registry_order(curses)));
    }
    Some(repaired)
}

/// `EnchantmentHelper.getEnchantmentsForCrafting`: an enchanted book's stored
/// enchantments, anything else's own.
fn crafting_enchantments(stack: &ItemStack) -> Vec<(String, i32)> {
    let wanted = if stack.item == items::ENCHANTED_BOOK {
        Kind::StoredEnchantments
    } else {
        Kind::Enchantments
    };
    match stack.components.get(wanted) {
        Some(Component::Enchantments(levels) | Component::StoredEnchantments(levels)) => {
            levels.clone()
        }
        _ => Vec::new(),
    }
}

/// Whether the enchantment `name` is in `#minecraft:curse`.
fn is_curse(name: &str) -> bool {
    let Some(id) = clustine_data::synced_registry("minecraft:enchantment")
        .and_then(|registry| registry.entries.iter().position(|entry| *entry == name))
        .and_then(|id| i32::try_from(id).ok())
    else {
        return false;
    };
    TAGS.iter()
        .find(|tags| tags.registry == "minecraft:enchantment")
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == "minecraft:curse"))
        .is_some_and(|tag| tag.entries.contains(&id))
}

/// `ItemStack.set(type, value)`, which leaves the patch without the type where the value
/// is the item's default (`PatchedDataComponentMap.set`).
fn set_or_default(stack: &mut ItemStack, component: Component) {
    let kind = component.kind();
    let defaults = ItemComponents::of(stack.item);
    let is_default = match &component {
        Component::Fireworks(fireworks) => {
            fireworks.explosions.is_empty()
                && defaults.fireworks_flight_duration == Some(fireworks.flight_duration)
        }
        // An empty default reads as none (`ItemComponents::potion_contents`).
        Component::PotionContents(contents) => {
            *contents == PotionContents::default() && defaults.potion_contents.is_none()
        }
        _ => false,
    };
    if is_default {
        stack.components.reset(kind);
    } else {
        stack.components.set(component);
    }
}

/// `Item.getCraftingRemainder`, set by `Items.<clinit>`'s `craftRemainder` (@15588,
/// @15619, @15714, @20223, @22839): the three full buckets give a bucket, dragon's breath
/// and a honey bottle a glass bottle. No other item has one.
fn crafting_remainder(item: i32) -> Option<ItemStack> {
    match item {
        items::WATER_BUCKET | items::LAVA_BUCKET | items::MILK_BUCKET => {
            Some(ItemStack::new(items::BUCKET, 1))
        }
        items::DRAGON_BREATH | items::HONEY_BOTTLE => Some(ItemStack::new(items::GLASS_BOTTLE, 1)),
        _ => None,
    }
}

/// `getRemainingItems` of `recipe`'s class for `input`, the trimmed box's slots row by
/// row.
fn remaining_items(recipe: &CraftingRecipe, input: &Input) -> Vec<Option<ItemStack>> {
    let remainder =
        |stack: Option<&ItemStack>| stack.and_then(|stack| crafting_remainder(stack.item));
    match recipe.kind {
        // `BookCloningRecipe.getRemainingItems` @0-90 gives the remainders up to the first
        // book with content, which stays (one of it), and none after it. No Clustine book
        // has content, so it gives every remainder, as the default; and the recipe never
        // matches.
        // `BannerDuplicateRecipe.getRemainingItems` @0-109: the remainders, and the banner
        // with layers stays (one of it).
        Crafting::BannerDuplicate { .. } => input
            .items
            .iter()
            .map(|stack| {
                let stack = (*stack)?;
                crafting_remainder(stack.item)
                    .or_else(|| (banner_layers(stack) > 0).then(|| stack.with_count(1)))
            })
            .collect(),
        // `CraftingRecipe.defaultCraftingReminder` @0-65.
        _ => input.items.iter().map(|stack| remainder(*stack)).collect(),
    }
}

/// `DyeColor`'s texture colours (`getTextureDiffuseColor`, n7 `DyeColor.<clinit>`), white
/// 0 to black 15, without the alpha that dyeing does not read.
const TEXTURE_COLOURS: [i32; 16] = [
    16_383_998, 16_351_261, 13_061_821, 3_847_130, 16_701_501, 8_439_583, 15_961_002, 4_673_362,
    10_329_495, 1_481_884, 8_991_416, 3_949_738, 8_606_770, 6_192_150, 11_546_150, 1_908_001,
];

#[cfg(test)]
mod tests;
