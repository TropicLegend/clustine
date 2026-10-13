// Written for Clustine from the game's data (`worldgen/material_rule/**`,
// `worldgen/material_condition/**`, `worldgen/biome/**`) and the codecs of the pinned
// jar's classes that read it (`net.minecraft.world.level.levelgen.material`). It uses
// the text helpers of the router's emitter (src/router.rs), which is adapted from
// SteelMC; nothing else here is.

//! Emits the material rules of `clustine-worldgen-data` (ADR-0019, section 1, row 6)
//! and what they read of a biome (part of row 9).
//!
//! A material rule is a tree: a block, a sequence of rules of which the first that
//! gives a block counts, a rule under a condition, the bands of the badlands, an ore
//! vein. Rules and conditions are entries of two registries and name one another.
//!
//! How the emitted Rust is shaped:
//!
//! - A rule of the registry is a function `r_…` over a context that answers the
//!   conditions (`clustine_worldgen_data::material::Context`), and gives the state id
//!   of a block or `None`. Its body is the tree written as statements, in the data's
//!   order: a sequence is one statement after the other, a condition an `if`, a block
//!   a `return`. What the data has after a rule that always gives a block is never
//!   reached and is not written.
//! - A condition of the registry is a function `q_…` that gives a truth value.
//! - A part of a tree that stands deep or is long is a function of its own, `h_…`,
//!   which keeps every line and every function within ADR-0019's budgets.
//! - Biomes are constants of their numbers in the registry; the noises, the names of
//!   random numbers and the density functions the rule asks are numbered in the order
//!   they are met, and listed in the constants of the rule's type.
//!
//! Only what the dimension's own rule reaches is written, so the other presets' rules
//! (`overworld_caves`, `overworld_floating_islands`) are not.

use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::model::Block;
use crate::router::{LINE_WIDTH, check_layout, double, float, identifier};
use crate::worldgen::{Worldgen, block_state};

/// A part of a tree that stands this many `if`s deep becomes a function of its own,
/// unless it is small.
const DEEPEST: usize = 5;

/// A part of a tree with more nodes than this becomes a function of its own.
const LARGEST: usize = 48;

/// A part with fewer nodes than this stays where it is, however deep.
const SMALL: usize = 4;

/// A height against the terrain's (`VerticalAnchor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

impl Anchor {
    fn parse(json: &Json) -> Result<Self> {
        let [(kind, value)] = json.members()? else {
            bail!("a height has one member");
        };
        let value = value.integer()?;
        Ok(match kind.as_str() {
            "absolute" => Self::Absolute(value),
            "above_bottom" => Self::AboveBottom(value),
            "below_top" => Self::BelowTop(value),
            other => bail!("a height that is {other:?} is not known to the emitter"),
        })
    }

    fn text(self) -> String {
        match self {
            Self::Absolute(value) => format!("Anchor::Absolute({value})"),
            Self::AboveBottom(value) => format!("Anchor::AboveBottom({value})"),
            Self::BelowTop(value) => format!("Anchor::BelowTop({value})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// An entry of `worldgen/material_condition`.
    Reference(String),
    StoneDepth {
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    },
    Not(Box<Condition>),
    /// The biomes' names.
    Biome(Vec<String>),
    YAbove {
        anchor: Anchor,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Water {
        offset: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    Hole,
    Steep,
    Temperature,
    AbovePreliminarySurface,
    NoiseThreshold {
        noise: String,
        min: f64,
        max: f64,
        is_3d: bool,
    },
    VerticalGradient {
        random_name: String,
        true_at_and_below: Anchor,
        false_at_and_above: Anchor,
    },
}

/// The members a kind of rule or condition may have; one that is not among them
/// fails, so that nothing the data says is passed over.
fn only(json: &Json, what: &str, known: &[&str]) -> Result<()> {
    for (key, _) in json.members()? {
        ensure!(
            key == "type" || known.contains(&key.as_str()),
            "the member {key:?} of {what} is not known to the emitter"
        );
    }
    Ok(())
}

impl Condition {
    pub fn parse(json: &Json) -> Result<Self> {
        if let Json::String(name) = json {
            return Ok(Self::Reference(name.clone()));
        }
        let kind = json.member("type")?.string()?;
        let surface_type = |json: &Json| -> Result<bool> {
            Ok(match json.member("surface_type")?.string()? {
                "floor" => false,
                "ceiling" => true,
                other => bail!("a surface that is {other:?} is not known to the emitter"),
            })
        };
        Ok(match kind {
            "minecraft:stone_depth" => {
                only(
                    json,
                    kind,
                    &[
                        "offset",
                        "add_surface_depth",
                        "secondary_depth_range",
                        "surface_type",
                    ],
                )?;
                Self::StoneDepth {
                    offset: json.member("offset")?.integer()?,
                    add_surface_depth: json.member("add_surface_depth")?.boolean()?,
                    secondary_depth_range: json.member("secondary_depth_range")?.integer()?,
                    ceiling: surface_type(json)?,
                }
            }
            "minecraft:not" => {
                only(json, kind, &["invert"])?;
                Self::Not(Box::new(Self::parse(json.member("invert")?)?))
            }
            "minecraft:biome" => {
                only(json, kind, &["biome_is"])?;
                let names = match json.member("biome_is")? {
                    Json::String(name) => vec![name.clone()],
                    list => list
                        .array()?
                        .iter()
                        .map(|name| name.string().map(str::to_owned))
                        .collect::<Result<Vec<String>>>()?,
                };
                for name in &names {
                    ensure!(
                        !name.starts_with('#'),
                        "a rule asks for the biomes of the tag {name}, which the emitter does \
                         not resolve"
                    );
                }
                ensure!(!names.is_empty(), "a rule asks for no biome at all");
                Self::Biome(names)
            }
            "minecraft:y_above" => {
                only(
                    json,
                    kind,
                    &["anchor", "surface_depth_multiplier", "add_stone_depth"],
                )?;
                Self::YAbove {
                    anchor: Anchor::parse(json.member("anchor")?)?,
                    surface_depth_multiplier: json.member("surface_depth_multiplier")?.integer()?,
                    add_stone_depth: json.member("add_stone_depth")?.boolean()?,
                }
            }
            "minecraft:water" => {
                only(
                    json,
                    kind,
                    &["offset", "surface_depth_multiplier", "add_stone_depth"],
                )?;
                Self::Water {
                    offset: json.member("offset")?.integer()?,
                    surface_depth_multiplier: json.member("surface_depth_multiplier")?.integer()?,
                    add_stone_depth: json.member("add_stone_depth")?.boolean()?,
                }
            }
            "minecraft:hole" => {
                only(json, kind, &[])?;
                Self::Hole
            }
            "minecraft:steep" => {
                only(json, kind, &[])?;
                Self::Steep
            }
            "minecraft:temperature" => {
                only(json, kind, &[])?;
                Self::Temperature
            }
            "minecraft:above_preliminary_surface" => {
                only(json, kind, &[])?;
                Self::AbovePreliminarySurface
            }
            "minecraft:noise_threshold" => {
                only(
                    json,
                    kind,
                    &["noise", "min_threshold", "max_threshold", "is_3d"],
                )?;
                Self::NoiseThreshold {
                    noise: json.member("noise")?.string()?.to_owned(),
                    min: json.member("min_threshold")?.double()?,
                    max: json.member("max_threshold")?.double()?,
                    // The one member the game's codec lets the data leave out.
                    is_3d: match json.get("is_3d") {
                        Some(flat) => flat.boolean()?,
                        None => false,
                    },
                }
            }
            "minecraft:vertical_gradient" => {
                only(
                    json,
                    kind,
                    &["random_name", "true_at_and_below", "false_at_and_above"],
                )?;
                Self::VerticalGradient {
                    random_name: json.member("random_name")?.string()?.to_owned(),
                    true_at_and_below: Anchor::parse(json.member("true_at_and_below")?)?,
                    false_at_and_above: Anchor::parse(json.member("false_at_and_above")?)?,
                }
            }
            other => bail!("a condition of the kind {other:?} is not known to the emitter"),
        })
    }
}

/// An ore vein as the data has it: blocks by state id and name, density functions by
/// name.
#[derive(Clone, Debug, PartialEq)]
pub struct Vein {
    pub ore: (u64, String),
    pub raw_ore: (u64, String),
    pub filler: (u64, String),
    pub raw_ore_chance: f32,
    pub density: String,
    pub richness: String,
    pub filler_gap: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    /// An entry of `worldgen/material_rule`.
    Reference(String),
    /// A state id, and the block's name for a comment.
    Block(u64, String),
    Sequence(Vec<Rule>),
    Condition(Condition, Box<Rule>),
    Bandlands,
    OreVein(Box<Vein>),
}

/// The name a block state of the data is written with, for a comment.
fn state_name(json: &Json) -> Result<String> {
    Ok(match json {
        Json::String(name) => name.clone(),
        other => other.member("id")?.string()?.to_owned(),
    })
}

impl Rule {
    pub fn parse(json: &Json, blocks: &[Block]) -> Result<Self> {
        if let Json::String(name) = json {
            return Ok(Self::Reference(name.clone()));
        }
        let kind = json.member("type")?.string()?;
        let state = |member: &str| -> Result<(u64, String)> {
            let json = json.member(member)?;
            Ok((block_state(json, blocks)?, state_name(json)?))
        };
        let function = |member: &str| -> Result<String> {
            match json.member(member)? {
                Json::String(name) => Ok(name.clone()),
                _ => bail!(
                    "the {member} of an ore vein is a density function written out where it \
                     stands; the emitter knows only one that is named"
                ),
            }
        };
        Ok(match kind {
            "minecraft:block" => {
                only(json, kind, &["result_state"])?;
                let (id, name) = state("result_state")?;
                Self::Block(id, name)
            }
            "minecraft:sequence" => {
                only(json, kind, &["sequence"])?;
                Self::Sequence(
                    json.member("sequence")?
                        .array()?
                        .iter()
                        .map(|rule| Self::parse(rule, blocks))
                        .collect::<Result<Vec<Rule>>>()?,
                )
            }
            "minecraft:condition" => {
                only(json, kind, &["if_true", "then_run"])?;
                Self::Condition(
                    Condition::parse(json.member("if_true")?)?,
                    Box::new(Self::parse(json.member("then_run")?, blocks)?),
                )
            }
            "minecraft:bandlands" => {
                only(json, kind, &[])?;
                Self::Bandlands
            }
            "minecraft:ore_vein" => {
                only(
                    json,
                    kind,
                    &[
                        "ore_block",
                        "raw_ore_block",
                        "filler_block",
                        "raw_ore_chance",
                        "density",
                        "richness",
                        "filler_gap",
                    ],
                )?;
                Self::OreVein(Box::new(Vein {
                    ore: state("ore_block")?,
                    raw_ore: state("raw_ore_block")?,
                    filler: state("filler_block")?,
                    raw_ore_chance: json.member("raw_ore_chance")?.float()?,
                    density: function("density")?,
                    richness: function("richness")?,
                    filler_gap: function("filler_gap")?,
                }))
            }
            other => bail!("a rule of the kind {other:?} is not known to the emitter"),
        })
    }

    /// How many rules and conditions the tree has, not following references.
    fn nodes(&self) -> usize {
        match self {
            Self::Sequence(rules) => 1 + rules.iter().map(Rule::nodes).sum::<usize>(),
            Self::Condition(_, rule) => 2 + rule.nodes(),
            _ => 1,
        }
    }
}

/// The two registries, read.
pub struct Registries {
    pub rules: BTreeMap<String, Rule>,
    pub conditions: BTreeMap<String, Condition>,
}

impl Registries {
    pub fn load(worldgen: &Worldgen, blocks: &[Block]) -> Result<Self> {
        let mut rules = BTreeMap::new();
        for (name, json) in worldgen.registry("material_rule")? {
            let rule = Rule::parse(json, blocks)
                .with_context(|| format!("reading the material rule {name}"))?;
            rules.insert(name.clone(), rule);
        }
        let mut conditions = BTreeMap::new();
        for (name, json) in worldgen.registry("material_condition")? {
            let condition = Condition::parse(json)
                .with_context(|| format!("reading the material condition {name}"))?;
            conditions.insert(name.clone(), condition);
        }
        Ok(Self { rules, conditions })
    }
}

/// The emitted file of one dimension, with what the router's emitter has to know.
pub struct Emitted {
    /// `material_rule.rs`.
    pub text: String,
    /// The name of the type that is the rule: `OverworldMaterial`.
    pub type_name: String,
    /// The density functions the rule names, in the order of their numbers.
    pub functions: Vec<String>,
}

/// The name of the rule's type for the noise settings `name`: `OverworldMaterial`.
pub fn type_name(name: &str) -> String {
    let mut out = String::new();
    for part in name.split('_') {
        let mut letters = part.chars();
        if let Some(first) = letters.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(letters);
        }
    }
    out.push_str("Material");
    out
}

/// A function of the emitted file.
struct Function {
    name: String,
    doc: Option<String>,
    /// `Option<u16>` for a rule, `bool` for a condition.
    gives: &'static str,
    lines: Vec<String>,
    uses_context: bool,
}

struct Generator<'a> {
    registries: &'a Registries,
    /// The numbers of the biomes in the registry, by name.
    biome_numbers: &'a BTreeMap<String, usize>,
    noise_names: &'a [String],
    /// The registry's rules that were written, by name: the function, and whether it
    /// always gives a block.
    rules: BTreeMap<String, (String, bool)>,
    conditions: BTreeMap<String, String>,
    /// What is being written, to find a rule that names itself.
    open: Vec<String>,
    functions: Vec<Function>,
    helpers: usize,
    /// The biomes that are asked for, by their constant.
    biomes: BTreeMap<String, (usize, String)>,
    noises: Vec<String>,
    randoms: Vec<String>,
    density_functions: Vec<String>,
    veins: Vec<Vein>,
}

/// The lines of a body with whether the context is named in them.
#[derive(Default)]
struct Body {
    lines: Vec<String>,
    uses_context: bool,
}

impl Body {
    fn push(&mut self, depth: usize, text: &str) {
        self.lines.push(format!("{}{text}", "    ".repeat(depth)));
    }
}

fn number_of(list: &mut Vec<String>, name: &str) -> usize {
    match list.iter().position(|known| known == name) {
        Some(number) => number,
        None => {
            list.push(name.to_owned());
            list.len() - 1
        }
    }
}

impl<'a> Generator<'a> {
    /// The function of a rule of the registry, and whether it always gives a block.
    fn rule(&mut self, name: &str) -> Result<(String, bool)> {
        if let Some(known) = self.rules.get(name) {
            return Ok(known.clone());
        }
        ensure!(
            !self.open.iter().any(|open| open == name),
            "the material rule {name} names itself"
        );
        let registries = self.registries;
        let rule = registries
            .rules
            .get(name)
            .with_context(|| format!("{name} is referred to and is no material rule"))?;
        let function = format!("r_{}", identifier(name)?);
        self.open.push(name.to_owned());
        let always = self.function(&function, Some(format!("`{name}`")), rule)?;
        self.open.pop();
        self.rules
            .insert(name.to_owned(), (function.clone(), always));
        Ok((function, always))
    }

    /// Writes `rule` as the function `name` and says whether it always gives a block.
    fn function(&mut self, name: &str, doc: Option<String>, rule: &Rule) -> Result<bool> {
        let mut body = Body::default();
        let always = self.statements(rule, 0, &mut body)?;
        if !always {
            body.push(0, "None");
        }
        self.functions.push(Function {
            name: name.to_owned(),
            doc,
            gives: "Option<u16>",
            lines: body.lines,
            uses_context: body.uses_context,
        });
        Ok(always)
    }

    /// Writes the statements of `rule` into `body`, `depth` `if`s deep, and says
    /// whether they always give a block, so that nothing after them is reached.
    fn statements(&mut self, rule: &Rule, depth: usize, body: &mut Body) -> Result<bool> {
        let nodes = rule.nodes();
        if depth > 0 && nodes >= SMALL && (depth >= DEEPEST || nodes > LARGEST) {
            self.helpers += 1;
            let name = format!("h_{}", self.helpers);
            let always = self.function(&name, None, rule)?;
            return Ok(Self::call(&name, always, depth, body));
        }
        Ok(match rule {
            Rule::Reference(name) => {
                let (function, always) = self.rule(name)?;
                Self::call(&function, always, depth, body)
            }
            Rule::Block(state, name) => {
                let state = u16::try_from(*state).context("a state id over 65535")?;
                body.push(depth, &format!("return Some({state}); // {name}"));
                true
            }
            Rule::Sequence(rules) => {
                for rule in rules {
                    if self.statements(rule, depth, body)? {
                        // What follows in the data is never reached, and is not
                        // written.
                        return Ok(true);
                    }
                }
                false
            }
            Rule::Condition(condition, rule) => {
                let condition = self.condition(condition, body)?;
                let line = format!("if {condition} {{");
                if depth * 4 + 8 + line.len() <= LINE_WIDTH {
                    body.push(depth, &line);
                } else {
                    // Only a list of biomes makes a condition long.
                    let (before, rest) = condition
                        .split_once("(&[")
                        .with_context(|| format!("the condition {condition} is too long"))?;
                    let (list, after) = rest
                        .split_once("])")
                        .with_context(|| format!("the condition {condition} is too long"))?;
                    body.push(depth, &format!("if {before}(&["));
                    for item in list.split(", ") {
                        body.push(depth + 1, &format!("{item},"));
                    }
                    body.push(depth, &format!("]){after} {{"));
                }
                self.statements(rule, depth + 1, body)?;
                body.push(depth, "}");
                false
            }
            Rule::Bandlands => {
                body.uses_context = true;
                body.push(depth, "return Some(c.band());");
                true
            }
            Rule::OreVein(vein) => {
                let number = match self.veins.iter().position(|known| known == &**vein) {
                    Some(number) => number,
                    None => {
                        self.veins.push((**vein).clone());
                        self.veins.len() - 1
                    }
                };
                for function in [&vein.density, &vein.richness, &vein.filler_gap] {
                    number_of(&mut self.density_functions, function);
                }
                body.uses_context = true;
                body.push(
                    depth,
                    &format!("if let Some(state) = c.ore_vein(&VEIN_{number}) {{"),
                );
                body.push(depth + 1, "return Some(state);");
                body.push(depth, "}");
                false
            }
        })
    }

    /// Writes the call of another rule's function.
    fn call(function: &str, always: bool, depth: usize, body: &mut Body) -> bool {
        body.uses_context = true;
        if always {
            body.push(depth, &format!("return {function}(c);"));
        } else {
            body.push(depth, &format!("if let Some(state) = {function}(c) {{"));
            body.push(depth + 1, "return Some(state);");
            body.push(depth, "}");
        }
        always
    }

    /// A condition as an expression.
    fn condition(&mut self, condition: &Condition, body: &mut Body) -> Result<String> {
        body.uses_context = true;
        Ok(match condition {
            Condition::Reference(name) => {
                let function = match self.conditions.get(name) {
                    Some(function) => function.clone(),
                    None => {
                        let registries = self.registries;
                        let target = registries.conditions.get(name).with_context(|| {
                            format!("{name} is referred to and is no material condition")
                        })?;
                        ensure!(
                            !matches!(target, Condition::Reference(_)),
                            "the material condition {name} is only another's name"
                        );
                        let function = format!("q_{}", identifier(name)?);
                        let mut inner = Body::default();
                        let text = self.condition(target, &mut inner)?;
                        self.functions.push(Function {
                            name: function.clone(),
                            doc: Some(format!("`{name}`")),
                            gives: "bool",
                            lines: vec![text],
                            uses_context: true,
                        });
                        self.conditions.insert(name.clone(), function.clone());
                        function
                    }
                };
                format!("{function}(c)")
            }
            Condition::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                ceiling,
            } => format!(
                "c.stone_depth({offset}, {add_surface_depth}, {secondary_depth_range}, {ceiling})"
            ),
            Condition::Not(inner) => format!("!{}", self.condition(inner, body)?),
            Condition::Biome(names) => {
                let mut constants = Vec::new();
                for name in names {
                    let number = *self
                        .biome_numbers
                        .get(name)
                        .with_context(|| format!("{name} is asked for and is no biome"))?;
                    let constant = identifier(name)?.to_uppercase();
                    self.biomes
                        .insert(constant.clone(), (number, name.to_owned()));
                    constants.push(constant);
                }
                format!("c.biome(&[{}])", constants.join(", "))
            }
            Condition::YAbove {
                anchor,
                surface_depth_multiplier,
                add_stone_depth,
            } => format!(
                "c.y_above({}, {surface_depth_multiplier}, {add_stone_depth})",
                anchor.text()
            ),
            Condition::Water {
                offset,
                surface_depth_multiplier,
                add_stone_depth,
            } => format!("c.water({offset}, {surface_depth_multiplier}, {add_stone_depth})"),
            Condition::Hole => "c.hole()".to_owned(),
            Condition::Steep => "c.steep()".to_owned(),
            Condition::Temperature => "c.temperature()".to_owned(),
            Condition::AbovePreliminarySurface => "c.above_preliminary_surface()".to_owned(),
            Condition::NoiseThreshold {
                noise,
                min,
                max,
                is_3d,
            } => {
                ensure!(
                    self.noise_names.contains(noise),
                    "the noise {noise} is asked for and is not in worldgen/noise"
                );
                let number = number_of(&mut self.noises, noise);
                let method = if *is_3d { "noise_3d" } else { "noise" };
                // The greatest double stands for "no upper bound" in the data.
                let max = if *max == f64::MAX {
                    "f64::MAX".to_owned()
                } else {
                    double(*max)?
                };
                format!("c.{method}({number}, {}, {max})", double(*min)?)
            }
            Condition::VerticalGradient {
                random_name,
                true_at_and_below,
                false_at_and_above,
            } => {
                ensure!(
                    random_name.is_ascii(),
                    "the name {random_name} of random numbers is not ASCII"
                );
                let number = number_of(&mut self.randoms, random_name);
                format!(
                    "c.vertical_gradient({number}, {}, {})",
                    true_at_and_below.text(),
                    false_at_and_above.text()
                )
            }
        })
    }

    fn render(&self, settings: &str, root: &str, type_name: &str) -> Result<String> {
        let mut out = String::new();
        writeln!(out, "//! The material rule `{root}`,")?;
        writeln!(
            out,
            "//! which the noise settings `minecraft:{settings}` name: what a block of ground \
             becomes"
        )?;
        writeln!(
            out,
            "//! after the fill, as a function over a context that answers the rule's \
             conditions for"
        )?;
        writeln!(out, "//! the block.")?;
        writeln!(out, "//!")?;
        writeln!(
            out,
            "//! A function `r_…` is a rule of the game's registry, `q_…` a condition of it, and \
             `h_…` a part"
        )?;
        writeln!(
            out,
            "//! of a rule that is a function of its own because it stands deep or is long."
        )?;
        writeln!(out)?;
        writeln!(out, "use crate::density::NoiseParameters;")?;
        if !self.noises.is_empty() {
            writeln!(out, "use crate::generated::noises;")?;
        }
        let mut names = Vec::new();
        if self
            .functions
            .iter()
            .any(|function| function.lines.iter().any(|line| line.contains("Anchor::")))
        {
            names.push("Anchor");
        }
        names.push("Context");
        names.push("MaterialRule");
        if !self.veins.is_empty() {
            names.push("Vein");
        }
        writeln!(out, "use crate::material::{{{}}};", names.join(", "))?;

        if !self.biomes.is_empty() {
            writeln!(out, "\n// Biomes by their numbers in the registry.")?;
            for (constant, (number, _)) in &self.biomes {
                writeln!(out, "const {constant}: u16 = {number};")?;
            }
        }

        for (number, vein) in self.veins.iter().enumerate() {
            let function = |name: &String| {
                self.density_functions
                    .iter()
                    .position(|known| known == name)
                    .context("a vein's density function was not numbered")
            };
            writeln!(out, "\nstatic VEIN_{number}: Vein = Vein {{")?;
            for (field, (state, name)) in [
                ("ore", &vein.ore),
                ("raw_ore", &vein.raw_ore),
                ("filler", &vein.filler),
            ] {
                let state = u16::try_from(*state).context("a state id over 65535")?;
                writeln!(out, "    {field}: {state}, // {name}")?;
            }
            writeln!(out, "    raw_ore_chance: {},", float(vein.raw_ore_chance)?)?;
            writeln!(
                out,
                "    density: {}, // {}",
                function(&vein.density)?,
                vein.density
            )?;
            writeln!(
                out,
                "    richness: {}, // {}",
                function(&vein.richness)?,
                vein.richness
            )?;
            writeln!(
                out,
                "    filler_gap: {}, // {}",
                function(&vein.filler_gap)?,
                vein.filler_gap
            )?;
            writeln!(out, "}};")?;
        }

        writeln!(out, "\n/// The material rule of `minecraft:{settings}`.")?;
        writeln!(out, "pub struct {type_name};")?;
        writeln!(out, "\nimpl MaterialRule for {type_name} {{")?;
        writeln!(out, "    const NAME: &'static str = {root:?};")?;
        let list = |out: &mut String, name: &str, kind: &str, items: &[String]| -> Result<()> {
            if items.is_empty() {
                writeln!(out, "    const {name}: &'static [{kind}] = &[];")?;
            } else {
                writeln!(out, "    const {name}: &'static [{kind}] = &[")?;
                for item in items {
                    writeln!(out, "        {item},")?;
                }
                writeln!(out, "    ];")?;
            }
            Ok(())
        };
        let noises = self
            .noises
            .iter()
            .map(|noise| Ok(format!("&noises::{}", identifier(noise)?.to_uppercase())))
            .collect::<Result<Vec<String>>>()?;
        list(&mut out, "NOISES", "&'static NoiseParameters", &noises)?;
        let quoted = |names: &[String]| -> Vec<String> {
            names.iter().map(|name| format!("{name:?}")).collect()
        };
        list(&mut out, "RANDOMS", "&'static str", &quoted(&self.randoms))?;
        list(
            &mut out,
            "FUNCTIONS",
            "&'static str",
            &quoted(&self.density_functions),
        )?;
        let (function, _) = self
            .rules
            .get(root)
            .context("the dimension's own rule was not written")?;
        let uses = self
            .functions
            .iter()
            .find(|other| other.name == *function)
            .is_some_and(|function| function.uses_context);
        let c = if uses { "c" } else { "_c" };
        writeln!(
            out,
            "\n    fn apply<C: Context>({c}: &mut C) -> Option<u16> {{"
        )?;
        writeln!(out, "        {function}({c})")?;
        writeln!(out, "    }}")?;
        writeln!(out, "}}")?;

        for function in &self.functions {
            writeln!(out)?;
            if let Some(doc) = &function.doc {
                writeln!(out, "/// {doc}")?;
            }
            let c = if function.uses_context { "c" } else { "_c" };
            writeln!(
                out,
                "fn {}<C: Context>({c}: &mut C) -> {} {{",
                function.name, function.gives
            )?;
            for line in &function.lines {
                writeln!(out, "    {line}")?;
            }
            writeln!(out, "}}")?;
        }
        Ok(out)
    }
}

/// Emits the material rule `root` of the noise settings `minecraft:<settings>`.
/// `biomes` are the names of the biomes in the order of their numbers, and `noises`
/// the names of the noises there are.
pub fn emit(
    settings: &str,
    root: &str,
    registries: &Registries,
    biomes: &[String],
    noises: &[String],
) -> Result<Emitted> {
    let biome_numbers: BTreeMap<String, usize> = biomes
        .iter()
        .enumerate()
        .map(|(number, name)| (name.clone(), number))
        .collect();
    let mut generator = Generator {
        registries,
        biome_numbers: &biome_numbers,
        noise_names: noises,
        rules: BTreeMap::new(),
        conditions: BTreeMap::new(),
        open: Vec::new(),
        functions: Vec::new(),
        helpers: 0,
        biomes: BTreeMap::new(),
        noises: Vec::new(),
        randoms: Vec::new(),
        density_functions: Vec::new(),
        veins: Vec::new(),
    };
    generator.rule(root)?;
    for (index, function) in generator.functions.iter().enumerate() {
        ensure!(
            generator.functions[..index]
                .iter()
                .all(|other| other.name != function.name),
            "two rules or conditions would both be the function {}",
            function.name
        );
    }
    let type_name = type_name(settings);
    let text = generator.render(settings, root, &type_name)?;
    check_layout(&text)?;
    Ok(Emitted {
        text,
        type_name,
        functions: generator.density_functions,
    })
}

/// Emits `biomes.rs`: what the material rules read of each biome, in the order of the
/// biomes' numbers. `biomes` are the names in that order, and each has to be an entry
/// of `worldgen/biome`, which has no other.
pub fn biomes(biomes: &[String], worldgen: &Worldgen) -> Result<String> {
    let registry = worldgen.registry("biome")?;
    ensure!(
        registry.len() == biomes.len(),
        "worldgen/biome has {} biomes and the registry of the reports {}",
        registry.len(),
        biomes.len()
    );
    let mut out = String::new();
    writeln!(
        out,
        "//! What the material rules read of each biome: its temperature, and whether that is \
         a frozen"
    )?;
    writeln!(
        out,
        "//! ocean's, which is warmer in patches. In the order of the biomes' numbers in the \
         registry."
    )?;
    writeln!(out)?;
    writeln!(out, "use crate::material::BiomeClimate;")?;
    writeln!(out)?;
    writeln!(
        out,
        "pub static CLIMATE: [BiomeClimate; {}] = [",
        biomes.len()
    )?;
    for name in biomes {
        let json = registry
            .get(name)
            .with_context(|| format!("{name} is in the registry and not in worldgen/biome"))?;
        let frozen = match json.get("temperature_modifier") {
            None => false,
            Some(modifier) => match modifier.string()? {
                "none" => false,
                "frozen" => true,
                other => bail!("the temperature modifier {other:?} of {name} is not known"),
            },
        };
        writeln!(
            out,
            "    BiomeClimate {{ name: {name:?}, temperature: {}, frozen: {frozen} }},",
            float(json.member("temperature")?.float()?)?
        )?;
    }
    writeln!(out, "];")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registries(rules: &[(&str, &str)], conditions: &[(&str, &str)]) -> Registries {
        let block = |name: &str, state: u64| Block {
            name: name.to_owned(),
            first_state: state,
            last_state: state,
            default_state: state,
            properties: Vec::new(),
        };
        let blocks = [block("minecraft:stone", 1), block("minecraft:dirt", 10)];
        Registries {
            rules: rules
                .iter()
                .map(|(name, text)| {
                    let rule = Rule::parse(&Json::parse(text).unwrap(), &blocks).unwrap();
                    ((*name).to_owned(), rule)
                })
                .collect(),
            conditions: conditions
                .iter()
                .map(|(name, text)| {
                    let condition = Condition::parse(&Json::parse(text).unwrap()).unwrap();
                    ((*name).to_owned(), condition)
                })
                .collect(),
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    const ROOT: &str = r#"{"type": "minecraft:sequence", "sequence": [
        {"type": "minecraft:condition",
         "if_true": {"type": "minecraft:biome", "biome_is": ["minecraft:b", "minecraft:a"]},
         "then_run": "minecraft:inner"},
        {"type": "minecraft:condition",
         "if_true": {"type": "minecraft:not", "invert": "minecraft:on_floor"},
         "then_run": {"type": "minecraft:block", "result_state": "minecraft:dirt"}},
        {"type": "minecraft:condition",
         "if_true": {"type": "minecraft:noise_threshold", "noise": "minecraft:surface",
                     "min_threshold": -0.5, "max_threshold": 1.7976931348623157e308},
         "then_run": {"type": "minecraft:bandlands"}},
        {"type": "minecraft:block", "result_state": "minecraft:stone"},
        {"type": "minecraft:block", "result_state": "minecraft:dirt"}
    ]}"#;
    const INNER: &str = r#"{"type": "minecraft:condition",
        "if_true": {"type": "minecraft:vertical_gradient", "random_name": "minecraft:floor",
                    "true_at_and_below": {"above_bottom": 0},
                    "false_at_and_above": {"below_top": 5}},
        "then_run": {"type": "minecraft:block", "result_state": "minecraft:stone"}}"#;
    const ON_FLOOR: &str = r#"{"type": "minecraft:stone_depth", "offset": 0,
        "add_surface_depth": false, "secondary_depth_range": 0, "surface_type": "floor"}"#;

    fn emitted() -> Emitted {
        let registries = registries(
            &[
                ("minecraft:root", ROOT),
                ("minecraft:inner", INNER),
                ("minecraft:unused", INNER),
            ],
            &[("minecraft:on_floor", ON_FLOOR)],
        );
        emit(
            "overworld",
            "minecraft:root",
            &registries,
            &names(&["minecraft:a", "minecraft:b", "minecraft:c"]),
            &names(&["minecraft:surface"]),
        )
        .unwrap()
    }

    #[test]
    fn a_tree_is_written_as_statements_in_the_datas_order() {
        let emitted = emitted();
        assert_eq!(emitted.type_name, "OverworldMaterial");
        let text = emitted.text;
        for line in [
            "const A: u16 = 0;",
            "const B: u16 = 1;",
            "    const NAME: &'static str = \"minecraft:root\";",
            "        &noises::SURFACE,",
            "        \"minecraft:floor\",",
            "    const FUNCTIONS: &'static [&'static str] = &[];",
            "    if c.biome(&[B, A]) {",
            "        if let Some(state) = r_inner(c) {",
            "    if !q_on_floor(c) {",
            "        return Some(10); // minecraft:dirt",
            "    if c.noise(0, -0.5, f64::MAX) {",
            "        return Some(c.band());",
            "    return Some(1); // minecraft:stone",
            "    c.stone_depth(0, false, 0, false)",
            "    if c.vertical_gradient(0, Anchor::AboveBottom(0), Anchor::BelowTop(5)) {",
        ] {
            assert!(text.lines().any(|other| other == line), "{line}\n{text}");
        }
        // The biome that is not asked for is no constant, the rule that is not reached
        // no function, and what follows a rule that always gives a block is not there.
        assert!(!text.contains("const C:"));
        assert!(!text.contains("r_unused"));
        assert_eq!(text.matches("minecraft:dirt").count(), 1);
        assert!(text.contains("fn r_root<C: Context>(c: &mut C) -> Option<u16> {"));
        let root = text.split("fn r_root").nth(1).unwrap();
        assert!(!root.contains("None"), "the rule always gives a block");
    }

    #[test]
    fn two_runs_over_the_same_data_give_the_same_bytes() {
        assert_eq!(emitted().text, emitted().text);
    }

    #[test]
    fn an_ore_vein_numbers_its_density_functions_and_names_its_blocks() {
        let registries = registries(
            &[(
                "minecraft:root",
                r#"{"type": "minecraft:ore_vein", "ore_block": "minecraft:stone",
                    "raw_ore_block": "minecraft:dirt", "filler_block": "minecraft:stone",
                    "raw_ore_chance": 0.02, "density": "minecraft:d",
                    "richness": "minecraft:r", "filler_gap": "minecraft:g"}"#,
            )],
            &[],
        );
        let emitted = emit("nether", "minecraft:root", &registries, &[], &[]).unwrap();
        assert_eq!(
            emitted.functions,
            names(&["minecraft:d", "minecraft:r", "minecraft:g"])
        );
        for line in [
            "static VEIN_0: Vein = Vein {",
            "    raw_ore: 10, // minecraft:dirt",
            "    raw_ore_chance: 0.02,",
            "    filler_gap: 2, // minecraft:g",
            "    if let Some(state) = c.ore_vein(&VEIN_0) {",
            "    None",
        ] {
            assert!(
                emitted.text.lines().any(|other| other == line),
                "{line}\n{}",
                emitted.text
            );
        }
    }

    #[test]
    fn a_rule_that_only_gives_a_block_does_not_name_its_context() {
        let registries = registries(
            &[(
                "minecraft:root",
                r#"{"type": "minecraft:block", "result_state": "minecraft:stone"}"#,
            )],
            &[],
        );
        let text = emit("end", "minecraft:root", &registries, &[], &[])
            .unwrap()
            .text;
        assert!(text.contains("fn r_root<C: Context>(_c: &mut C) -> Option<u16> {"));
        assert!(text.contains("fn apply<C: Context>(_c: &mut C) -> Option<u16> {"));
        assert!(!text.contains("use crate::generated::noises;"));
    }

    #[test]
    fn a_deep_tree_is_split_into_functions_within_the_budgets() {
        // Twenty conditions one in the other, each with a sequence under it.
        let mut text =
            r#"{"type": "minecraft:block", "result_state": "minecraft:stone"}"#.to_owned();
        for _ in 0..20 {
            text = format!(
                r#"{{"type": "minecraft:sequence", "sequence": [
                    {{"type": "minecraft:condition", "if_true": {{"type": "minecraft:hole"}},
                      "then_run": {text}}},
                    {{"type": "minecraft:condition", "if_true": {{"type": "minecraft:steep"}},
                      "then_run": {{"type": "minecraft:block",
                                    "result_state": "minecraft:dirt"}}}}]}}"#
            );
        }
        let registries = registries(&[("minecraft:root", &text)], &[]);
        let emitted = emit("overworld", "minecraft:root", &registries, &[], &[]).unwrap();
        assert!(emitted.text.contains("fn h_1<C: Context>"));
        assert!(
            emitted
                .text
                .lines()
                .all(|line| line.chars().count() <= LINE_WIDTH)
        );
    }

    #[test]
    fn what_the_emitter_does_not_know_fails_it() {
        let blocks: [Block; 0] = [];
        let rule = |text: &str| Rule::parse(&Json::parse(text).unwrap(), &blocks);
        assert!(rule(r#"{"type": "minecraft:new_kind"}"#).is_err());
        assert!(rule(r#"{"type": "minecraft:bandlands", "more": 1}"#).is_err());
        assert!(rule(r#"{"type": "minecraft:block", "result_state": "minecraft:none"}"#).is_err());
        let condition = |text: &str| Condition::parse(&Json::parse(text).unwrap());
        assert!(
            condition(r##"{"type": "minecraft:biome", "biome_is": "#minecraft:tag"}"##).is_err()
        );
        assert!(condition(r#"{"type": "minecraft:hole", "deep": true}"#).is_err());
        // A name that is in neither registry.
        let nothing = registries(&[("minecraft:root", r#""minecraft:nothing""#)], &[]);
        assert!(emit("overworld", "minecraft:root", &nothing, &[], &[]).is_err());
        // A rule that names itself.
        let itself = registries(&[("minecraft:root", r#""minecraft:root""#)], &[]);
        assert!(emit("overworld", "minecraft:root", &itself, &[], &[]).is_err());
    }
}
