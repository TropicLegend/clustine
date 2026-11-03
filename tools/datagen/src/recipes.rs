//! Compiles the game's recipes into `recipes.rs`, which `clustine-data`'s `recipes`
//! reads (`docs/groundwork/v-plan.md`, the V3 step): every recipe of the fourteen
//! crafting serialisers (`RecipeSerializers` @11-121), the four cooking kinds, the
//! stonecutter's and the smithing table's, and the brewing recipes' inputs and reagents,
//! from `data/minecraft/recipe/**.json`; and the
//! dye colour of each item that has `minecraft:dye` by default, from the generator's
//! report of the items' components.
//!
//! The recipes are a datapack registry, and the order a crafting grid tries them in is
//! the registry's (`RecipeMap.create` over `HolderLookup.listElements`, then `findFirst`
//! in `RecipeMap.getRecipesFor`). That order is believed to be the resources' sorted by
//! id (`listResources` gives a `TreeMap`) and is not proven: the tables are sorted by
//! key, and the grids that two recipes can both match are listed by `clustine-sim`'s
//! tests for the comparison with the official server.
//!
//! An ingredient is the set of items its `HolderSet` holds, a tag resolved here, sorted
//! by id; the tag's name is kept beside it, since the wire sends a tag by name. The
//! ingredients are shared: each distinct one is emitted once. Anything the reader does
//! not understand (a type, a key, a result component, an ingredient of another shape)
//! fails the run, so that a jar that changes one is noticed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::loot::{kind, only, tables_under};
use crate::model::Tag;

/// `DyeColor`'s names in the order of its ids (n7 `DyeColor.<clinit>`), white 0 to
/// black 15: what `minecraft:dye` names.
const DYE_COLOURS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// `CraftingBookCategory`, by the name its codec reads.
const CATEGORIES: [(&str, &str); 4] = [
    ("building", "Building"),
    ("redstone", "Redstone"),
    ("equipment", "Equipment"),
    ("misc", "Misc"),
];

/// A set of items: the items of a tag or a list, by id, sorted; and the tag's name
/// where it is one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ingredient {
    pub tag: Option<String>,
    pub items: Vec<usize>,
}

/// An `ItemStackTemplate`: the item, the count, and the only component a recipe's
/// result gives in 26.3, `minecraft:suspicious_stew_effects`, each an effect by name and
/// its duration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub item: usize,
    pub count: u8,
    pub stew_effects: Vec<(String, i32)>,
}

/// One crafting recipe's kind and what its serialiser reads. Ingredients are indices
/// into [`Compiled::ingredients`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Crafting {
    /// `crafting_shaped`, its pattern shrunk as `ShapedRecipePattern.shrink` does: the
    /// cells row by row, each an ingredient's index plus one, or 0 for a space.
    Shaped {
        width: u8,
        height: u8,
        cells: Vec<usize>,
        result: Output,
    },
    Shapeless {
        ingredients: Vec<usize>,
        result: Output,
    },
    /// `crafting_transmute`: the material count's bounds (`MinMaxBounds$Ints`, 1 to 1
    /// by default and within 1 to 8), and a result item that is the input's own where
    /// it is not named.
    Transmute {
        input: usize,
        material: usize,
        min: u8,
        max: u8,
        add_material_count: bool,
        item: Option<usize>,
        count: u8,
    },
    Dye {
        target: usize,
        dye: usize,
        result: Output,
    },
    Imbue {
        source: usize,
        material: usize,
        result: Output,
    },
    DecoratedPot {
        back: usize,
        left: usize,
        right: usize,
        front: usize,
        result: Output,
    },
    BookCloning {
        source: usize,
        material: usize,
        result: Output,
    },
    MapExtending {
        map: usize,
        material: usize,
    },
    FireworkRocket {
        shell: usize,
        fuel: usize,
        star: usize,
        result: Output,
    },
    FireworkStar {
        fuel: usize,
        dye: usize,
        trail: usize,
        twinkle: usize,
        /// `FireworkExplosion$Shape` by its name, and the ingredient that gives it.
        shapes: Vec<(String, usize)>,
        result: Output,
    },
    FireworkStarFade {
        target: usize,
        dye: usize,
        result: Output,
    },
    BannerDuplicate {
        banner: usize,
        result: Output,
    },
    ShieldDecoration {
        banner: usize,
        target: usize,
        result: Output,
    },
    RepairItem,
}

/// A crafting recipe: its key without `minecraft:`, its group and book category, and
/// whether learning it shows a toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingRecipe {
    pub key: String,
    pub group: String,
    pub category: &'static str,
    pub show_notification: bool,
    pub kind: Crafting,
}

/// A recipe of one input: a cooking kind's or the stonecutter's. Its key is kept here
/// for the order but not emitted: nothing reads it yet, and the budget of generated
/// output (ADR-0019, section 6) is nearly spent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Single {
    pub key: String,
    /// The variant of `SingleKind`.
    pub kind: &'static str,
    pub input: usize,
    pub result: Output,
}

/// A smithing recipe: the transform's or the trim's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Smithing {
    pub key: String,
    pub trim: bool,
    pub template: Option<usize>,
    pub base: usize,
    pub addition: Option<usize>,
}

/// The recipes, each list in the registry's order (by key), and the dye colours.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compiled {
    pub ingredients: Vec<Ingredient>,
    pub crafting: Vec<CraftingRecipe>,
    pub single: Vec<Single>,
    pub smithing: Vec<Smithing>,
    /// Each brewing recipe's input and reagent, by ingredient.
    pub brewing: Vec<(usize, usize)>,
    /// Each item with `minecraft:dye` by default, by id, and its colour's id.
    pub dyes: Vec<(usize, u8)>,
}

/// The names the recipes may hold: items by id in order, and the item tags.
pub struct Names<'a> {
    pub items: &'a [String],
    pub item_tags: &'a [Tag],
}

/// Reads the recipes of `recipes` (`data/minecraft/recipe`) and the dye colours of the
/// items from `item_reports` (`reports/minecraft/components/item`).
pub fn load(recipes: &Path, item_reports: &Path, names: &Names) -> Result<Compiled> {
    let mut files = Vec::new();
    for stem in tables_under(recipes)? {
        let file = recipes.join(format!("{stem}.json"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        files.push((stem, text));
    }
    let mut reports = Vec::new();
    for name in names.items {
        let local = name.strip_prefix("minecraft:").unwrap_or(name);
        let file = item_reports.join(format!("{local}.json"));
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        reports.push(text);
    }
    compile(&files, &reports, names)
}

/// Compiles the recipes, each a file's stem and text, and the items' component reports,
/// one an item in the order of `names.items`.
pub fn compile(
    recipes: &[(String, String)],
    reports: &[String],
    names: &Names,
) -> Result<Compiled> {
    ensure!(
        reports.len() == names.items.len(),
        "a component report for each item"
    );
    let mut reader = Reader {
        names,
        ingredients: Vec::new(),
        index: BTreeMap::new(),
    };
    let mut sorted: Vec<&(String, String)> = recipes.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut compiled = Compiled::default();
    for (key, text) in sorted {
        let json = Json::parse(text).with_context(|| format!("reading the recipe {key}"))?;
        reader
            .recipe(key, &json, &mut compiled)
            .with_context(|| format!("reading the recipe {key}"))?;
    }
    compiled.ingredients = reader.ingredients;
    for (id, text) in reports.iter().enumerate() {
        let json = Json::parse(text).with_context(|| format!("reading {}", names.items[id]))?;
        if let Some(dye) = json.member("components")?.get("minecraft:dye") {
            let name = dye.string()?;
            let colour = DYE_COLOURS
                .iter()
                .position(|known| *known == name)
                .with_context(|| format!("{} is dyed {name}, no colour known", names.items[id]))?;
            compiled.dyes.push((id, u8::try_from(colour)?));
        }
    }
    ensure!(compiled.dyes.len() == 16, "a dye item of each colour");
    Ok(compiled)
}

struct Reader<'a> {
    names: &'a Names<'a>,
    ingredients: Vec<Ingredient>,
    index: BTreeMap<Ingredient, usize>,
}

impl Reader<'_> {
    fn recipe(&mut self, key: &str, json: &Json, compiled: &mut Compiled) -> Result<()> {
        let kind = kind(json)?;
        let crafting = |kind: Crafting, group: String, category, show| CraftingRecipe {
            key: key.to_owned(),
            group,
            category,
            show_notification: show,
            kind,
        };
        let group = || -> Result<String> {
            Ok(json
                .get("group")
                .map(Json::string)
                .transpose()?
                .unwrap_or_default()
                .to_owned())
        };
        let category = || -> Result<&'static str> {
            let name = json
                .get("category")
                .map(Json::string)
                .transpose()?
                .unwrap_or("misc");
            CATEGORIES
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, variant)| *variant)
                .with_context(|| format!("the crafting category {name} is not known"))
        };
        let recipe = match kind {
            "crafting_shaped" => {
                only(
                    json,
                    &[
                        "type",
                        "group",
                        "category",
                        "key",
                        "pattern",
                        "result",
                        "show_notification",
                    ],
                )?;
                let show = json
                    .get("show_notification")
                    .map(Json::boolean)
                    .transpose()?
                    .unwrap_or(true);
                let shaped = self.shaped(json)?;
                crafting(shaped, group()?, category()?, show)
            }
            "crafting_shapeless" => {
                only(
                    json,
                    &["type", "group", "category", "ingredients", "result"],
                )?;
                let ingredients = json
                    .member("ingredients")?
                    .array()?
                    .iter()
                    .map(|ingredient| self.ingredient(ingredient))
                    .collect::<Result<Vec<_>>>()?;
                ensure!(
                    (1..=9).contains(&ingredients.len()),
                    "a shapeless recipe of {} ingredients",
                    ingredients.len()
                );
                let result = self.output(json.member("result")?)?;
                crafting(
                    Crafting::Shapeless {
                        ingredients,
                        result,
                    },
                    group()?,
                    category()?,
                    true,
                )
            }
            "crafting_transmute" => {
                only(
                    json,
                    &[
                        "type",
                        "group",
                        "category",
                        "input",
                        "material",
                        "material_count",
                        "result",
                        "add_material_count_to_result",
                    ],
                )?;
                // `TransmuteRecipe.DEFAULT_MATERIAL_COUNT` is exactly 1, and the bounds
                // have to lie within 1 to 8 (`FULL_RANGE_MATERIAL_COUNT`).
                let (min, max) = match json.get("material_count") {
                    None => (1, 1),
                    Some(Json::Number(_)) => {
                        let exactly = json.member("material_count")?.integer()?;
                        (exactly, exactly)
                    }
                    Some(bounds) => {
                        only(bounds, &["min", "max"])?;
                        (
                            bounds
                                .get("min")
                                .map(Json::integer)
                                .transpose()?
                                .unwrap_or(1),
                            bounds
                                .get("max")
                                .map(Json::integer)
                                .transpose()?
                                .unwrap_or(8),
                        )
                    }
                };
                ensure!(
                    1 <= min && min <= max && max <= 8,
                    "material counts {min} to {max} are outside 1 to 8"
                );
                let result = json.member("result")?;
                only(result, &["id", "count"])?;
                let item = result
                    .get("id")
                    .map(|id| -> Result<usize> { self.item(id.string()?) })
                    .transpose()?;
                let count = result
                    .get("count")
                    .map(Json::integer)
                    .transpose()?
                    .unwrap_or(1);
                let transmute = Crafting::Transmute {
                    input: self.ingredient(json.member("input")?)?,
                    material: self.ingredient(json.member("material")?)?,
                    min: u8::try_from(min)?,
                    max: u8::try_from(max)?,
                    add_material_count: json
                        .get("add_material_count_to_result")
                        .map(Json::boolean)
                        .transpose()?
                        .unwrap_or(false),
                    item,
                    count: u8::try_from(count)?,
                };
                crafting(transmute, group()?, category()?, true)
            }
            "crafting_dye" => {
                only(
                    json,
                    &["type", "group", "category", "target", "dye", "result"],
                )?;
                let dye = Crafting::Dye {
                    target: self.ingredient(json.member("target")?)?,
                    dye: self.ingredient(json.member("dye")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(dye, group()?, category()?, true)
            }
            "crafting_imbue" => {
                only(
                    json,
                    &["type", "group", "category", "source", "material", "result"],
                )?;
                let imbue = Crafting::Imbue {
                    source: self.ingredient(json.member("source")?)?,
                    material: self.ingredient(json.member("material")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(imbue, group()?, category()?, true)
            }
            "crafting_decorated_pot" => {
                only(
                    json,
                    &[
                        "type", "group", "category", "back", "left", "right", "front", "result",
                    ],
                )?;
                let pot = Crafting::DecoratedPot {
                    back: self.ingredient(json.member("back")?)?,
                    left: self.ingredient(json.member("left")?)?,
                    right: self.ingredient(json.member("right")?)?,
                    front: self.ingredient(json.member("front")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(pot, group()?, category()?, true)
            }
            "crafting_special_bookcloning" => {
                only(
                    json,
                    &["type", "group", "category", "source", "material", "result"],
                )?;
                let cloning = Crafting::BookCloning {
                    source: self.ingredient(json.member("source")?)?,
                    material: self.ingredient(json.member("material")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(cloning, group()?, category()?, true)
            }
            "crafting_special_mapextending" => {
                only(
                    json,
                    &["type", "group", "category", "map", "material", "result"],
                )?;
                let extending = Crafting::MapExtending {
                    map: self.ingredient(json.member("map")?)?,
                    material: self.ingredient(json.member("material")?)?,
                };
                crafting(extending, group()?, category()?, true)
            }
            "crafting_special_firework_rocket" => {
                only(
                    json,
                    &[
                        "type", "group", "category", "shell", "fuel", "star", "result",
                    ],
                )?;
                let rocket = Crafting::FireworkRocket {
                    shell: self.ingredient(json.member("shell")?)?,
                    fuel: self.ingredient(json.member("fuel")?)?,
                    star: self.ingredient(json.member("star")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(rocket, group()?, category()?, true)
            }
            "crafting_special_firework_star" => {
                only(
                    json,
                    &[
                        "type", "group", "category", "fuel", "dye", "trail", "twinkle", "shapes",
                        "result",
                    ],
                )?;
                let mut shapes = Vec::new();
                for (shape, ingredient) in json.member("shapes")?.members()? {
                    ensure!(
                        ["large_ball", "star", "creeper", "burst"].contains(&shape.as_str()),
                        "the firework shape {shape} is not known"
                    );
                    shapes.push((shape.clone(), self.ingredient(ingredient)?));
                }
                let star = Crafting::FireworkStar {
                    fuel: self.ingredient(json.member("fuel")?)?,
                    dye: self.ingredient(json.member("dye")?)?,
                    trail: self.ingredient(json.member("trail")?)?,
                    twinkle: self.ingredient(json.member("twinkle")?)?,
                    shapes,
                    result: self.output(json.member("result")?)?,
                };
                crafting(star, group()?, category()?, true)
            }
            "crafting_special_firework_star_fade" => {
                only(
                    json,
                    &["type", "group", "category", "target", "dye", "result"],
                )?;
                let fade = Crafting::FireworkStarFade {
                    target: self.ingredient(json.member("target")?)?,
                    dye: self.ingredient(json.member("dye")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(fade, group()?, category()?, true)
            }
            "crafting_special_bannerduplicate" => {
                only(json, &["type", "group", "category", "banner", "result"])?;
                let duplicate = Crafting::BannerDuplicate {
                    banner: self.ingredient(json.member("banner")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(duplicate, group()?, category()?, true)
            }
            "crafting_special_shielddecoration" => {
                only(
                    json,
                    &["type", "group", "category", "banner", "target", "result"],
                )?;
                let decoration = Crafting::ShieldDecoration {
                    banner: self.ingredient(json.member("banner")?)?,
                    target: self.ingredient(json.member("target")?)?,
                    result: self.output(json.member("result")?)?,
                };
                crafting(decoration, group()?, category()?, true)
            }
            "crafting_special_repairitem" => {
                only(json, &["type", "group", "category"])?;
                crafting(Crafting::RepairItem, group()?, category()?, true)
            }
            "smelting" | "blasting" | "smoking" | "campfire_cooking" | "stonecutting" => {
                only(
                    json,
                    &[
                        "type",
                        "group",
                        "category",
                        "ingredient",
                        "result",
                        "cookingtime",
                        "experience",
                    ],
                )?;
                let kind = match kind {
                    "smelting" => "Smelting",
                    "blasting" => "Blasting",
                    "smoking" => "Smoking",
                    "campfire_cooking" => "Campfire",
                    _ => "Stonecutting",
                };
                let result = self.output(json.member("result")?)?;
                ensure!(
                    result.stew_effects.is_empty(),
                    "a recipe of one input gives a component"
                );
                compiled.single.push(Single {
                    key: key.to_owned(),
                    kind,
                    input: self.ingredient(json.member("ingredient")?)?,
                    result,
                });
                return Ok(());
            }
            "smithing_transform" | "smithing_trim" => {
                only(
                    json,
                    &["type", "template", "base", "addition", "result", "pattern"],
                )?;
                let mut optional = |name: &str| -> Result<Option<usize>> {
                    json.get(name)
                        .map(|value| self.ingredient(value))
                        .transpose()
                };
                let template = optional("template")?;
                let addition = optional("addition")?;
                compiled.smithing.push(Smithing {
                    key: key.to_owned(),
                    trim: kind == "smithing_trim",
                    template,
                    base: self.ingredient(json.member("base")?)?,
                    addition,
                });
                return Ok(());
            }
            "brewing" => {
                // Only what the property sets read (`RecipeManager`'s `BREWING_INPUTS` and
                // `BREWING_REAGENTS`): the input's and the reagent's items. The potions they
                // ask for and give are the brewing stand's, which is not built yet.
                only(json, &["type", "input", "reagent", "output"])?;
                let input = json.member("input")?;
                only(input, &["item", "potion_contents"])?;
                let reagent = json.member("reagent")?;
                only(reagent, &["item", "potion_contents"])?;
                compiled.brewing.push((
                    self.ingredient(input.member("item")?)?,
                    self.ingredient(reagent.member("item")?)?,
                ));
                return Ok(());
            }
            other => bail!("the recipe type {other} is not known"),
        };
        compiled.crafting.push(recipe);
        Ok(())
    }

    /// `ShapedRecipePattern.unpack`: the pattern shrunk to its used box, every key used
    /// and every symbol a key or a space.
    fn shaped(&mut self, json: &Json) -> Result<Crafting> {
        let rows: Vec<&str> = json
            .member("pattern")?
            .array()?
            .iter()
            .map(Json::string)
            .collect::<Result<_>>()?;
        ensure!(
            (1..=3).contains(&rows.len()),
            "a pattern of {} rows",
            rows.len()
        );
        let width = rows[0].chars().count();
        ensure!(
            (1..=3).contains(&width) && rows.iter().all(|row| row.chars().count() == width),
            "the rows of the pattern are not all one width of 1 to 3"
        );
        let shrunk = shrink(&rows);
        ensure!(!shrunk.is_empty(), "the pattern is empty");
        let mut keys = BTreeMap::new();
        for (symbol, ingredient) in json.member("key")?.members()? {
            let mut chars = symbol.chars();
            let (Some(symbol), None) = (chars.next(), chars.next()) else {
                bail!("the key {symbol:?} is not one symbol");
            };
            ensure!(symbol != ' ', "a space is a key");
            keys.insert(symbol, (self.ingredient(ingredient)?, false));
        }
        let mut cells = Vec::new();
        for row in &shrunk {
            for symbol in row.chars() {
                if symbol == ' ' {
                    cells.push(0);
                    continue;
                }
                let (ingredient, used) = keys
                    .get_mut(&symbol)
                    .with_context(|| format!("the pattern's {symbol:?} is not a key"))?;
                *used = true;
                cells.push(*ingredient + 1);
            }
        }
        ensure!(
            keys.values().all(|(_, used)| *used),
            "a key is not used in the pattern"
        );
        Ok(Crafting::Shaped {
            width: u8::try_from(shrunk[0].chars().count())?,
            height: u8::try_from(shrunk.len())?,
            cells,
            result: self.output(json.member("result")?)?,
        })
    }

    /// The index of `json`'s ingredient, added where it is new: an item, a `#tag`, or a
    /// list of items.
    fn ingredient(&mut self, json: &Json) -> Result<usize> {
        let ingredient = match json {
            Json::String(name) => match name.strip_prefix('#') {
                Some(tag) => {
                    let found = self
                        .names
                        .item_tags
                        .iter()
                        .find(|known| known.name == tag)
                        .with_context(|| format!("the item tag {tag} does not exist"))?;
                    let mut items = found.entries.clone();
                    items.sort_unstable();
                    items.dedup();
                    Ingredient {
                        tag: Some(tag.to_owned()),
                        items,
                    }
                }
                None => Ingredient {
                    tag: None,
                    items: vec![self.item(name)?],
                },
            },
            Json::Array(listed) => {
                let mut items = listed
                    .iter()
                    .map(|name| self.item(name.string()?))
                    .collect::<Result<Vec<_>>>()?;
                items.sort_unstable();
                items.dedup();
                Ingredient { tag: None, items }
            }
            other => bail!("the ingredient {other:?} is not understood"),
        };
        // `Ingredient`'s codec refuses an empty set.
        ensure!(!ingredient.items.is_empty(), "an ingredient holds no item");
        ensure!(
            ingredient
                .items
                .iter()
                .all(|item| *item < usize::from(u16::MAX)),
            "an item id beyond 16 bits"
        );
        if let Some(index) = self.index.get(&ingredient) {
            return Ok(*index);
        }
        let index = self.ingredients.len();
        self.index.insert(ingredient.clone(), index);
        self.ingredients.push(ingredient);
        Ok(index)
    }

    fn item(&self, name: &str) -> Result<usize> {
        self.names
            .items
            .iter()
            .position(|known| known == name)
            .with_context(|| format!("the item {name} does not exist"))
    }

    /// An `ItemStackTemplate`: an id, a count of 1 to 99 (1 by default) and the
    /// components it sets.
    fn output(&self, json: &Json) -> Result<Output> {
        only(json, &["id", "count", "components"])?;
        let item = self.item(json.member("id")?.string()?)?;
        let count = json
            .get("count")
            .map(Json::integer)
            .transpose()?
            .unwrap_or(1);
        ensure!((1..=99).contains(&count), "a count of {count}");
        let mut stew_effects = Vec::new();
        if let Some(components) = json.get("components") {
            for (name, value) in components.members()? {
                ensure!(
                    name == "minecraft:suspicious_stew_effects",
                    "the result component {name} is not understood"
                );
                for effect in value.array()? {
                    only(effect, &["id", "duration"])?;
                    stew_effects.push((
                        effect.member("id")?.string()?.to_owned(),
                        effect.member("duration")?.integer()?,
                    ));
                }
            }
        }
        ensure!(item < usize::from(u16::MAX), "an item id beyond 16 bits");
        Ok(Output {
            item,
            count: u8::try_from(count)?,
            stew_effects,
        })
    }
}

/// `ShapedRecipePattern.shrink`: the rows without the leading and trailing rows and
/// columns of spaces.
fn shrink(rows: &[&str]) -> Vec<String> {
    let mut first = usize::MAX;
    let mut last = 0;
    let mut leading = 0;
    let mut trailing = 0;
    for (at, row) in rows.iter().enumerate() {
        let chars: Vec<char> = row.chars().collect();
        first = first.min(chars.iter().take_while(|c| **c == ' ').count());
        match chars.iter().rposition(|c| *c != ' ') {
            Some(end) => {
                last = last.max(end);
                trailing = 0;
            }
            None => {
                if leading == at {
                    leading += 1;
                }
                trailing += 1;
            }
        }
    }
    if trailing == rows.len() {
        return Vec::new();
    }
    rows[leading..rows.len() - trailing]
        .iter()
        .map(|row| row.chars().skip(first).take(last + 1 - first).collect())
        .collect()
}

/// `recipes.rs`.
pub fn emit(compiled: &Compiled) -> Result<String> {
    let mut body = String::new();
    writeln!(
        body,
        "/// The ingredients the recipes name, each once: an item set by id, sorted, and the \
         tag it\n/// is where it is one."
    )?;
    writeln!(
        body,
        "pub static INGREDIENTS: [Ingredient; {}] = [",
        compiled.ingredients.len()
    )?;
    for ingredient in &compiled.ingredients {
        let items = list(&ingredient.items);
        match &ingredient.tag {
            Some(tag) => writeln!(body, "    tagged({tag:?}, &[{items}]),")?,
            None => writeln!(body, "    items(&[{items}]),")?,
        }
    }
    writeln!(body, "];\n")?;

    writeln!(
        body,
        "/// The crafting recipes in the registry's order, believed to be by key; keys and \
         groups\n/// are without `minecraft:`."
    )?;
    writeln!(
        body,
        "pub static CRAFTING: [CraftingRecipe; {}] = [",
        compiled.crafting.len()
    )?;
    for recipe in &compiled.crafting {
        let text = crafting_text(recipe);
        if recipe.show_notification {
            writeln!(body, "    {text},")?;
        } else {
            writeln!(body, "    quiet({text}),")?;
        }
    }
    writeln!(body, "];\n")?;

    writeln!(
        body,
        "/// The recipes of one input, the cooking kinds' and the stonecutter's, in the \
         registry's\n/// order (by key, which is not emitted)."
    )?;
    writeln!(
        body,
        "pub static SINGLE_INPUT: [SingleInput; {}] = [",
        compiled.single.len()
    )?;
    for single in &compiled.single {
        writeln!(
            body,
            "    single({}, {}, {}, {}),",
            single.kind, single.input, single.result.item, single.result.count
        )?;
    }
    writeln!(body, "];\n")?;

    writeln!(
        body,
        "/// The smithing recipes, the transforms and the trims, in the registry's order."
    )?;
    writeln!(
        body,
        "pub static SMITHING: [Smithing; {}] = [",
        compiled.smithing.len()
    )?;
    for smithing in &compiled.smithing {
        writeln!(
            body,
            "    Smithing {{ key: {:?}, trim: {}, template: {:?}, base: {}, addition: {:?} }},",
            smithing.key, smithing.trim, smithing.template, smithing.base, smithing.addition
        )?;
    }
    writeln!(body, "];\n")?;

    writeln!(
        body,
        "/// Each brewing recipe's input and reagent ingredients, in the registry's order: what \
         the\n/// property sets read of them."
    )?;
    writeln!(
        body,
        "pub static BREWING: [(u16, u16); {}] = [",
        compiled.brewing.len()
    )?;
    for (input, reagent) in &compiled.brewing {
        writeln!(body, "    ({input}, {reagent}),")?;
    }
    writeln!(body, "];\n")?;

    writeln!(
        body,
        "/// Each item with `minecraft:dye` by default, by id, and its `DyeColor`'s id."
    )?;
    writeln!(
        body,
        "pub static DYES: [(u16, u8); {}] = [",
        compiled.dyes.len()
    )?;
    for (item, colour) in &compiled.dyes {
        writeln!(body, "    ({item}, {colour}),")?;
    }
    writeln!(body, "];")?;

    let categories: Vec<&str> = CATEGORIES
        .iter()
        .map(|(_, variant)| *variant)
        .filter(|variant| body.contains(&format!(", {variant}, ")))
        .collect();
    let mut uses = Vec::new();
    if !categories.is_empty() {
        let mut categories = categories;
        categories.sort_unstable();
        uses.push(format!(
            "use crate::recipes::Category::{{{}}};",
            categories.join(", ")
        ));
    }
    let mut kinds: Vec<&str> = compiled.single.iter().map(|single| single.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    if !kinds.is_empty() {
        uses.push(format!(
            "use crate::recipes::SingleKind::{{{}}};",
            kinds.join(", ")
        ));
    }
    uses.push(
        "use crate::recipes::{\n    Crafting, CraftingRecipe, Ingredient, SingleInput, Smithing, \
         items, out, quiet, shaped,\n    shapeless, single, special, stew, tagged,\n};"
            .to_owned(),
    );
    Ok(format!("{}\n\n{body}", uses.join("\n")))
}

fn list(values: &[usize]) -> String {
    values
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn output_text(output: &Output) -> String {
    if output.stew_effects.is_empty() {
        return format!("out({}, {})", output.item, output.count);
    }
    let effects: Vec<String> = output
        .stew_effects
        .iter()
        .map(|(effect, duration)| format!("({effect:?}, {duration})"))
        .collect();
    format!(
        "stew({}, {}, &[{}])",
        output.item,
        output.count,
        effects.join(", ")
    )
}

/// A crafting recipe as its constructor: `shaped` and `shapeless` for the thousand of
/// those without components, `special` with the kind written out for the rest.
fn crafting_text(recipe: &CraftingRecipe) -> String {
    let head = format!("{:?}, {:?}, {}", recipe.key, recipe.group, recipe.category);
    let kind = match &recipe.kind {
        Crafting::Shaped {
            width,
            height,
            cells,
            result,
        } => {
            if result.stew_effects.is_empty() {
                return format!(
                    "shaped({head}, {width}, {height}, &[{}], {}, {})",
                    list(cells),
                    result.item,
                    result.count
                );
            }
            format!(
                "Shaped {{ width: {width}, height: {height}, cells: &[{}], result: {} }}",
                list(cells),
                output_text(result)
            )
        }
        Crafting::Shapeless {
            ingredients,
            result,
        } => {
            if result.stew_effects.is_empty() {
                return format!(
                    "shapeless({head}, &[{}], {}, {})",
                    list(ingredients),
                    result.item,
                    result.count
                );
            }
            format!(
                "Shapeless {{ ingredients: &[{}], result: {} }}",
                list(ingredients),
                output_text(result)
            )
        }
        Crafting::Transmute {
            input,
            material,
            min,
            max,
            add_material_count,
            item,
            count,
        } => format!(
            "Transmute {{ input: {input}, material: {material}, min: {min}, max: {max}, \
             add_material_count: {add_material_count}, item: {item:?}, count: {count} }}"
        ),
        Crafting::Dye {
            target,
            dye,
            result,
        } => format!(
            "Dye {{ target: {target}, dye: {dye}, result: {} }}",
            output_text(result)
        ),
        Crafting::Imbue {
            source,
            material,
            result,
        } => format!(
            "Imbue {{ source: {source}, material: {material}, result: {} }}",
            output_text(result)
        ),
        Crafting::DecoratedPot {
            back,
            left,
            right,
            front,
            result,
        } => format!(
            "DecoratedPot {{ back: {back}, left: {left}, right: {right}, front: {front}, \
             result: {} }}",
            output_text(result)
        ),
        Crafting::BookCloning {
            source,
            material,
            result,
        } => format!(
            "BookCloning {{ source: {source}, material: {material}, result: {} }}",
            output_text(result)
        ),
        Crafting::MapExtending { map, material } => {
            format!("MapExtending {{ map: {map}, material: {material} }}")
        }
        Crafting::FireworkRocket {
            shell,
            fuel,
            star,
            result,
        } => format!(
            "FireworkRocket {{ shell: {shell}, fuel: {fuel}, star: {star}, result: {} }}",
            output_text(result)
        ),
        Crafting::FireworkStar {
            fuel,
            dye,
            trail,
            twinkle,
            shapes,
            result,
        } => {
            let shapes: Vec<String> = shapes
                .iter()
                .map(|(shape, ingredient)| format!("({shape:?}, {ingredient})"))
                .collect();
            format!(
                "FireworkStar {{ fuel: {fuel}, dye: {dye}, trail: {trail}, twinkle: {twinkle}, \
                 shapes: &[{}], result: {} }}",
                shapes.join(", "),
                output_text(result)
            )
        }
        Crafting::FireworkStarFade {
            target,
            dye,
            result,
        } => format!(
            "FireworkStarFade {{ target: {target}, dye: {dye}, result: {} }}",
            output_text(result)
        ),
        Crafting::BannerDuplicate { banner, result } => format!(
            "BannerDuplicate {{ banner: {banner}, result: {} }}",
            output_text(result)
        ),
        Crafting::ShieldDecoration {
            banner,
            target,
            result,
        } => format!(
            "ShieldDecoration {{ banner: {banner}, target: {target}, result: {} }}",
            output_text(result)
        ),
        Crafting::RepairItem => "RepairItem".to_owned(),
    };
    format!("special({head}, Crafting::{kind})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> (Vec<String>, Vec<Tag>) {
        let items = [
            "minecraft:air",
            "minecraft:oak_planks",
            "minecraft:birch_planks",
            "minecraft:stick",
            "minecraft:wooden_pickaxe",
            "minecraft:red_dye",
        ]
        .map(str::to_owned)
        .to_vec();
        let tags = vec![Tag {
            name: "minecraft:planks".to_owned(),
            entries: vec![2, 1],
        }];
        (items, tags)
    }

    fn report(dye: Option<&str>) -> String {
        match dye {
            Some(colour) => format!("{{\"components\": {{\"minecraft:dye\": \"{colour}\"}}}}"),
            None => "{\"components\": {}}".to_owned(),
        }
    }

    /// The reports of the six items, with sixteen dyes made up by giving red to the
    /// one dye and checking only that the count is refused otherwise.
    fn compile_with(recipes: &[(&str, &str)]) -> Result<Compiled> {
        let (items, tags) = names();
        let names = Names {
            items: &items,
            item_tags: &tags,
        };
        let reports: Vec<String> = (0..items.len())
            .map(|id| report((id == 5).then_some("red")))
            .collect();
        let recipes: Vec<(String, String)> = recipes
            .iter()
            .map(|(key, text)| ((*key).to_owned(), (*text).to_owned()))
            .collect();
        compile(&recipes, &reports, &names)
    }

    #[test]
    fn a_pattern_is_shrunk_to_its_used_box_as_the_game_shrinks_it() {
        assert_eq!(shrink(&["   ", " # ", "   "]), vec!["#"]);
        assert_eq!(shrink(&["# ", "# "]), vec!["#", "#"]);
        assert_eq!(shrink(&[" ##", "  #"]), vec!["##", " #"]);
        assert!(shrink(&["  ", "  "]).is_empty());
    }

    #[test]
    fn fewer_than_sixteen_dyes_fail_the_run() {
        // The test's items hold one dye; a jar with fewer than sixteen is not the game's.
        let error = compile_with(&[]).expect_err("one dye is too few");
        assert!(format!("{error:#}").contains("a dye item of each colour"));
    }

    #[test]
    fn a_tag_resolves_to_its_items_sorted_and_is_shared_between_recipes() {
        let (items, tags) = names();
        let names = Names {
            items: &items,
            item_tags: &tags,
        };
        let mut reader = Reader {
            names: &names,
            ingredients: Vec::new(),
            index: BTreeMap::new(),
        };
        let planks = Json::parse("\"#minecraft:planks\"").unwrap();
        let first = reader.ingredient(&planks).unwrap();
        let again = reader.ingredient(&planks).unwrap();
        assert_eq!(first, again);
        assert_eq!(
            reader.ingredients[first],
            Ingredient {
                tag: Some("minecraft:planks".to_owned()),
                items: vec![1, 2],
            }
        );
        let stick = reader
            .ingredient(&Json::parse("\"minecraft:stick\"").unwrap())
            .unwrap();
        assert_eq!(reader.ingredients[stick].items, vec![3]);
        assert!(
            reader
                .ingredient(&Json::parse("\"#minecraft:nothing\"").unwrap())
                .is_err()
        );
    }

    #[test]
    fn a_shaped_recipe_keeps_its_cells_row_by_row_with_spaces_as_nought() {
        let (items, tags) = names();
        let names = Names {
            items: &items,
            item_tags: &tags,
        };
        let mut reader = Reader {
            names: &names,
            ingredients: Vec::new(),
            index: BTreeMap::new(),
        };
        let json = Json::parse(
            r##"{"type": "minecraft:crafting_shaped", "key": {"#": "minecraft:stick",
            "X": "#minecraft:planks"}, "pattern": ["XXX", " # ", " # "],
            "result": {"id": "minecraft:wooden_pickaxe"}}"##,
        )
        .unwrap();
        let Crafting::Shaped {
            width,
            height,
            cells,
            result,
        } = reader.shaped(&json).unwrap()
        else {
            panic!("a shaped recipe");
        };
        assert_eq!((width, height), (3, 3));
        let stick = 1 + reader
            .ingredients
            .iter()
            .position(|i| i.items == [3])
            .unwrap();
        let planks = 1 + reader
            .ingredients
            .iter()
            .position(|i| i.items == [1, 2])
            .unwrap();
        assert_eq!(
            cells,
            vec![planks, planks, planks, 0, stick, 0, 0, stick, 0]
        );
        assert_eq!(result.item, 4);
        assert_eq!(result.count, 1);
    }

    #[test]
    fn an_unknown_recipe_type_or_result_component_fails_the_run() {
        let (items, tags) = names();
        let names = Names {
            items: &items,
            item_tags: &tags,
        };
        let mut reader = Reader {
            names: &names,
            ingredients: Vec::new(),
            index: BTreeMap::new(),
        };
        let mut compiled = Compiled::default();
        let unknown = Json::parse(r#"{"type": "minecraft:crafting_special_mapcloning"}"#).unwrap();
        assert!(
            reader
                .recipe("map_cloning", &unknown, &mut compiled)
                .is_err()
        );
        let named = Json::parse(
            r#"{"type": "minecraft:crafting_shapeless", "ingredients": ["minecraft:stick"],
            "result": {"id": "minecraft:stick", "components": {"minecraft:custom_name": "x"}}}"#,
        )
        .unwrap();
        assert!(reader.recipe("named_stick", &named, &mut compiled).is_err());
    }
}
