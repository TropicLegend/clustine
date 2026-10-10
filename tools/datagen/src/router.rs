// Adapted from SteelMC, steel-worldgen/build/density/transpiler/ (codegen_expr.rs,
// codegen_functions.rs, codegen_structs.rs, graph.rs: the scalar half) and
// steel-worldgen/build/noise_parameters.rs at 885c4b3 (AGPL-3.0-or-later, Copyright (C)
// 2026 Alve Jeansson and contributors; see NOTICE). Changed for Clustine in October
// 2026: Rust written as text, laid out here and never formatted, in place of token
// streams; one statement a line, a function for every branch that is not always
// taken, and calls of hand-written functions in place of formulas repeated inline;
// splines as static data; `interpolated` and `cache` as the game has them at a single
// position, in place of a column cache and a cell filler; every entry of the released
// 26.3's settings (`chunk_surface_level`, the aquifers' `exclusion`) and none it lacks.

//! Emits the noise routers of `clustine-worldgen-data` (ADR-0019, section 1, rows 5
//! and 7): for each dimension a struct with the dimension's noises and a method for
//! every density function its noise settings reach, the splines of those functions as
//! statics, and beside them the parameters of every noise and the rest of the noise
//! settings.
//!
//! How the emitted Rust is shaped:
//!
//! - A density function is a method that takes a block position and gives a `float`.
//!   Its body is a list of `let` statements, one for each step, in the order the
//!   function's tree is walked; the arithmetic of each step is a hand-written
//!   function of `clustine_worldgen_data::density`.
//! - An operand that the game does not always evaluate (a branch of a range choice,
//!   the two ends of a `lerp`) is a method of its own, called where it is needed. So
//!   is an operand that would make a body long, which keeps every function far below
//!   the 300 lines ADR-0019 allows.
//! - A function the data marks `cache` is a method that asks the caller's memory first
//!   and leaves its value there.
//!
//! Nothing here depends on a hash map's order or on how a toolchain formats: names are
//! numbered in the order the trees are walked, and a float is written as the shortest
//! decimal that reads back as the same bits.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::density::{
    Axis, BinaryOp, Density, Metric, Registry, Spline, SplineValue, UnaryOp, depends_on_y, reached,
};
use crate::worldgen::{AQUIFER_ENTRIES, NoiseParameters, NoiseSettings, ROUTER_ENTRIES};

/// No emitted line is longer, but for one that a string literal makes so.
pub const LINE_WIDTH: usize = 100;

/// No emitted function has more lines (ADR-0019, section 6).
pub const FUNCTION_LINES: usize = 300;

/// How far the statements of a method's body are indented.
const BODY_INDENT: usize = 8;

/// An operand that would add more statements than this to a body becomes a method of
/// its own.
const INLINE_STATEMENTS: usize = 24;

/// The emitted files of one dimension.
pub struct Emitted {
    /// `router.rs`: the code.
    pub router: String,
    /// `splines.rs`: the splines the code reads, if it has any.
    pub splines: Option<String>,
}

/// Emits the router of the noise settings `minecraft:<name>`. `registry` are the
/// density functions and `noises` the noises that the settings may refer to; a name
/// that is in neither fails.
pub fn emit(
    name: &str,
    settings: &NoiseSettings,
    registry: &Registry,
    noises: &BTreeMap<String, NoiseParameters>,
) -> Result<Emitted> {
    let mut roots: Vec<&Density> = settings.router.iter().collect();
    roots.extend(settings.aquifers.iter().flatten());
    let targets: Vec<Density> = settings
        .spawn_target
        .iter()
        .flatten()
        .map(|(function, _, _)| Density::Reference(function.clone()))
        .collect();
    roots.extend(&targets);
    let named = reached(registry, &roots)?;

    let mut generator = Generator::new(registry);
    for root in &roots {
        generator.collect_leaves(root)?;
    }
    for function in &named {
        generator.collect_leaves(&registry[function])?;
    }
    for noise in &generator.noises {
        ensure!(
            noises.contains_key(noise),
            "the noise {noise} is referred to and is not in worldgen/noise"
        );
    }

    for function in &named {
        let method = format!("f_{}", identifier(function)?);
        ensure!(
            generator.named.values().all(|other| *other != method),
            "two density functions would both be the method {method}"
        );
        generator.named.insert(function.clone(), method);
    }
    for (function, method) in generator.named.clone() {
        generator.function(&method, Some(format!("`{function}`")), &registry[&function])?;
    }
    for (entry, density) in ROUTER_ENTRIES.iter().zip(&settings.router) {
        let doc = format!("The router's `{entry}`.");
        generator.function(&format!("router_{entry}"), Some(doc), density)?;
    }
    for (entry, density) in AQUIFER_ENTRIES
        .iter()
        .zip(settings.aquifers.iter().flatten())
    {
        let doc = format!("The aquifers' `{entry}`.");
        generator.function(&format!("aquifer_{entry}"), Some(doc), density)?;
    }

    let router = generator.render(name, settings)?;
    let splines = generator.render_splines(name);
    for text in std::iter::once(&router).chain(&splines) {
        check_layout(text)?;
    }
    Ok(Emitted { router, splines })
}

/// The name of the router's type for the noise settings `name`: `OverworldRouter`.
pub fn type_name(name: &str) -> String {
    let mut out = String::new();
    for part in name.split('_') {
        let mut letters = part.chars();
        if let Some(first) = letters.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(letters);
        }
    }
    out.push_str("Router");
    out
}

/// A name of the game (`minecraft:overworld/caves/noodle`) as part of a Rust name:
/// `overworld_caves_noodle`. Only the game's own names are known.
pub fn identifier(name: &str) -> Result<String> {
    let path = name
        .strip_prefix("minecraft:")
        .with_context(|| format!("{name} is not one of the game's own names"))?;
    ensure!(
        !path.is_empty()
            && path.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || b"_/".contains(&byte)),
        "{name} cannot be made a Rust name"
    );
    Ok(path.replace('/', "_"))
}

/// A `float` as Rust text that reads back as the same bits: the shortest such decimal,
/// which is what `Display` gives on every toolchain, always with a fraction.
pub fn float(value: f32) -> Result<String> {
    ensure!(value.is_finite(), "{value} cannot be written as a number");
    let mut text = format!("{value}");
    if !text.contains('.') {
        text.push_str(".0");
    }
    ensure!(
        text.parse::<f32>().map(f32::to_bits) == Ok(value.to_bits()) && !text.contains(['e', 'E']),
        "{text} does not read back as the float it was written from"
    );
    Ok(text)
}

/// A `double` likewise.
pub fn double(value: f64) -> Result<String> {
    ensure!(value.is_finite(), "{value} cannot be written as a number");
    let mut text = format!("{value}");
    if !text.contains('.') {
        text.push_str(".0");
    }
    ensure!(
        text.parse::<f64>().map(f64::to_bits) == Ok(value.to_bits()) && !text.contains(['e', 'E']),
        "{text} does not read back as the double it was written from"
    );
    Ok(text)
}

/// A `float` where Rust could not tell its type from what stands around it.
fn literal(value: f32) -> Result<String> {
    Ok(format!("{}_f32", float(value)?))
}

/// Items separated by commas, as many on a line as fit, each line indented.
pub fn wrapped(items: &[String], indent: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for item in items {
        if !line.is_empty() && indent.len() + line.len() + item.len() + 2 > LINE_WIDTH {
            out.push_str(indent);
            out.push_str(line.trim_end());
            out.push('\n');
            line.clear();
        }
        line.push_str(item);
        line.push_str(", ");
    }
    if !line.is_empty() {
        out.push_str(indent);
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Fails if a line is longer than [`LINE_WIDTH`] without a string literal making it
/// so, or a function longer than [`FUNCTION_LINES`].
pub fn check_layout(text: &str) -> Result<()> {
    for line in text.lines() {
        ensure!(
            line.chars().count() <= LINE_WIDTH || line.contains('"'),
            "an emitted line has more than {LINE_WIDTH} columns: {line}"
        );
    }
    let longest = longest_function(text);
    ensure!(
        longest <= FUNCTION_LINES,
        "an emitted function has {longest} lines, over the budget of {FUNCTION_LINES} \
         (docs/adr/0019-data-made-from-mojangs-jar.md, section 6)"
    );
    Ok(())
}

/// The number of lines of the longest function in emitted Rust, from the line of its
/// `fn` to the line of the brace that closes it. It rests on the emitter's layout: a
/// function ends at the first line that is a closing brace at the indentation of its
/// first line.
pub fn longest_function(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    let mut longest = 0;
    for (start, line) in lines.iter().enumerate() {
        let body = line.trim_start();
        if !(body.starts_with("fn ") || body.starts_with("pub fn ")) {
            continue;
        }
        let closing = format!("{}}}", &line[..line.len() - body.len()]);
        let length = lines[start..]
            .iter()
            .position(|other| *other == closing)
            .map_or(lines.len() - start, |end| end + 1);
        longest = longest.max(length);
    }
    longest
}

/// Which of a method's parameters its body names. One that it does not name is
/// written with an underscore, so that the emitted code compiles without a warning.
#[derive(Clone, Copy, Default)]
struct Used {
    memory: bool,
    x: bool,
    y: bool,
    z: bool,
}

impl Used {
    const ALL: Self = Self {
        memory: true,
        x: true,
        y: true,
        z: true,
    };

    fn add(&mut self, other: Self) {
        self.memory |= other.memory;
        self.x |= other.x;
        self.y |= other.y;
        self.z |= other.z;
    }
}

/// The statements of a method's body as they are being written.
#[derive(Default)]
struct Body {
    lines: Vec<String>,
    values: usize,
    used: Used,
}

impl Body {
    /// Adds `let vN = <expression>;` and gives the name.
    fn bind(&mut self, expression: &str) -> String {
        self.values += 1;
        let name = format!("v{}", self.values);
        let line = format!("let {name} = {expression};");
        let call = expression
            .strip_suffix(')')
            .and_then(|rest| rest.split_once('('));
        match call {
            // A call too long for one line has its arguments on lines of their own,
            // a closure apart from what stands before it.
            Some((function, arguments)) if line.len() + BODY_INDENT > LINE_WIDTH => {
                self.lines.push(format!("let {name} = {function}("));
                match arguments.split_once(", &mut |") {
                    Some((plain, closure)) => {
                        self.lines.push(format!("    {plain},"));
                        self.lines.push(format!("    &mut |{closure},"));
                    }
                    None => self.lines.push(format!("    {arguments},")),
                }
                self.lines.push(");".to_owned());
            }
            _ => self.lines.push(line),
        }
        name
    }
}

struct Method {
    name: String,
    doc: Option<String>,
    lines: Vec<String>,
    used: Used,
}

/// An operand that is evaluated only where it is needed: a number, or a method to
/// call with a position.
enum Lazy {
    Literal(String),
    Method(String),
}

struct Generator<'a> {
    registry: &'a Registry,
    /// The reached density functions by name, with their methods.
    named: BTreeMap<String, String>,
    /// The noises the functions ask, by name.
    noises: BTreeSet<String>,
    /// The five numbers of the dimension's `old_blended_noise`, if it has one.
    blended: Option<[f64; 5]>,
    end_islands: bool,
    methods: Vec<Method>,
    helpers: usize,
    memory_slots: usize,
    /// The splines' statics in the order they are numbered, and where each is by its
    /// text, so that a spline that is there twice is emitted once.
    splines: Vec<String>,
    spline_numbers: BTreeMap<String, usize>,
    /// What the splines' coordinates are, by their number.
    coordinates: Vec<Lazy>,
    coordinate_numbers: Vec<Density>,
}

impl<'a> Generator<'a> {
    fn new(registry: &'a Registry) -> Self {
        Self {
            registry,
            named: BTreeMap::new(),
            noises: BTreeSet::new(),
            blended: None,
            end_islands: false,
            methods: Vec::new(),
            helpers: 0,
            memory_slots: 0,
            splines: Vec::new(),
            spline_numbers: BTreeMap::new(),
            coordinates: Vec::new(),
            coordinate_numbers: Vec::new(),
        }
    }

    /// Notes the noises and the two special generators that `density` asks, not
    /// following references.
    fn collect_leaves(&mut self, density: &Density) -> Result<()> {
        if let Some(noise) = density.noise() {
            self.noises.insert(noise.to_owned());
        }
        match density {
            Density::OldBlendedNoise {
                xz_scale,
                y_scale,
                xz_factor,
                y_factor,
                smear_scale_multiplier,
            } => {
                let scales = [
                    *xz_scale,
                    *y_scale,
                    *xz_factor,
                    *y_factor,
                    *smear_scale_multiplier,
                ];
                ensure!(
                    self.blended.is_none_or(|other| other == scales),
                    "two blended noises with different numbers in one dimension"
                );
                self.blended = Some(scales);
            }
            Density::EndOuterIslands => self.end_islands = true,
            _ => {}
        }
        for child in density.children() {
            self.collect_leaves(child)?;
        }
        Ok(())
    }

    /// Emits `density` as the method `name`.
    fn function(&mut self, name: &str, doc: Option<String>, density: &Density) -> Result<()> {
        let mut body = Body::default();
        let value = self.value(density, &mut body)?;
        body.lines.push(value);
        self.methods.push(Method {
            name: name.to_owned(),
            doc,
            lines: body.lines,
            used: body.used,
        });
        Ok(())
    }

    /// How many statements `density` adds to a body when it is written into it, an
    /// operand that becomes a method of its own counting as the one call.
    fn statements(density: &Density) -> usize {
        let operand = |density: &Density| match Self::statements(density) {
            more if more > INLINE_STATEMENTS => 1,
            few => few,
        };
        match density {
            Density::Constant(_) | Density::BlendAlpha | Density::BlendOffset => 0,
            Density::Unary(_, input) | Density::Clamp { input, .. } => 1 + operand(input),
            Density::Blend(input) => Self::statements(input),
            Density::Binary(_, left, right) => 1 + operand(left) + operand(right),
            Density::Noise { shift, .. } => {
                1 + shift
                    .iter()
                    .flat_map(|pair| pair.iter())
                    .map(operand)
                    .sum::<usize>()
            }
            Density::Lerp { alpha, .. } => 11 + operand(alpha),
            Density::RangeChoice { input, .. } => 5 + operand(input),
            Density::IntervalSelect {
                input, functions, ..
            } => 1 + 2 * functions.len() + operand(input),
            Density::FindTopSurface { upper_bound, .. } => 1 + operand(upper_bound),
            _ => 1,
        }
    }

    /// Writes an operand that is always evaluated: into the body, or as a method of
    /// its own where it is large.
    fn operand(&mut self, density: &Density, body: &mut Body) -> Result<String> {
        if Self::statements(density) <= INLINE_STATEMENTS {
            return self.value(density, body);
        }
        let lazy = self.lazy(density)?;
        let call = Self::call(&lazy, body, "x", "y", "z");
        Ok(body.bind(&call))
    }

    /// An operand as a number or a method.
    fn lazy(&mut self, density: &Density) -> Result<Lazy> {
        match density {
            Density::Constant(value) => Ok(Lazy::Literal(literal(*value)?)),
            Density::BlendAlpha => Ok(Lazy::Literal(literal(1.0)?)),
            Density::BlendOffset => Ok(Lazy::Literal(literal(0.0)?)),
            Density::Blend(input) => self.lazy(input),
            Density::Reference(name) => Ok(Lazy::Method(self.method_of(name)?)),
            other => {
                self.helpers += 1;
                let name = format!("h_{}", self.helpers);
                self.function(&name, None, other)?;
                Ok(Lazy::Method(name))
            }
        }
    }

    fn method_of(&self, function: &str) -> Result<String> {
        self.named
            .get(function)
            .cloned()
            .with_context(|| format!("{function} is referred to and is no density function"))
    }

    /// The text that evaluates a lazy operand at the position the three names hold.
    /// `x`, `y` and `z` are the body's own parameters where they are passed on.
    fn call(lazy: &Lazy, body: &mut Body, x: &str, y: &str, z: &str) -> String {
        match lazy {
            Lazy::Literal(text) => text.clone(),
            Lazy::Method(name) => {
                body.used.add(Used {
                    memory: true,
                    x: x == "x",
                    y: y == "y",
                    z: z == "z",
                });
                format!("self.{name}(m, {x}, {y}, {z})")
            }
        }
    }

    /// Writes the statements that compute `density` into `body` and gives what holds
    /// the value: a name or a number.
    fn value(&mut self, density: &Density, body: &mut Body) -> Result<String> {
        Ok(match density {
            Density::Constant(value) => literal(*value)?,
            Density::BlendAlpha => literal(1.0)?,
            Density::BlendOffset => literal(0.0)?,
            Density::Blend(input) => self.value(input, body)?,
            Density::Beardifier => "d::NO_BEARD".to_owned(),
            Density::Reference(name) => {
                let method = self.method_of(name)?;
                body.used.add(Used::ALL);
                body.bind(&format!("self.{method}(m, x, y, z)"))
            }
            Density::Gradient {
                axis,
                from,
                to,
                from_value,
                to_value,
            } => {
                let coordinate = match axis {
                    Axis::X => {
                        body.used.x = true;
                        "x"
                    }
                    Axis::Y => {
                        body.used.y = true;
                        "y"
                    }
                    Axis::Z => {
                        body.used.z = true;
                        "z"
                    }
                };
                body.bind(&format!(
                    "d::gradient({coordinate}, {from}, {to}, {}, {})",
                    float(*from_value)?,
                    float(*to_value)?
                ))
            }
            Density::Noise {
                noise,
                xz_scale,
                y_scale,
                shift,
            } => {
                let field = identifier(noise)?;
                let xz = double(*xz_scale)?;
                body.used.x = true;
                body.used.z = true;
                match shift {
                    Some(shifts) => {
                        ensure!(*y_scale == 0.0, "a shifted noise that is not flat");
                        let shift_x = self.operand(&shifts[0], body)?;
                        let shift_z = self.operand(&shifts[1], body)?;
                        body.bind(&format!(
                            "d::shifted_noise2(&self.{field}, x, z, {xz}, {shift_x}, {shift_z})"
                        ))
                    }
                    None if *y_scale == 0.0 => {
                        body.bind(&format!("d::noise2(&self.{field}, x, z, {xz})"))
                    }
                    None => {
                        body.used.y = true;
                        body.bind(&format!(
                            "d::noise3(&self.{field}, x, y, z, {xz}, {})",
                            double(*y_scale)?
                        ))
                    }
                }
            }
            Density::ShiftA(noise) | Density::ShiftB(noise) => {
                let function = if matches!(density, Density::ShiftA(_)) {
                    "shift_a"
                } else {
                    "shift_b"
                };
                body.used.x = true;
                body.used.z = true;
                body.bind(&format!(
                    "d::{function}(&self.{}, x, z)",
                    identifier(noise)?
                ))
            }
            Density::Binary(op, left, right) => {
                let left = self.operand(left, body)?;
                let right = self.operand(right, body)?;
                body.bind(&match op {
                    BinaryOp::Add => format!("{left} + {right}"),
                    BinaryOp::Sub => format!("{left} - {right}"),
                    BinaryOp::Mul => format!("{left} * {right}"),
                    BinaryOp::Div => format!("{left} / {right}"),
                    BinaryOp::Min => format!("d::min({left}, {right})"),
                    BinaryOp::Max => format!("d::max({left}, {right})"),
                })
            }
            Density::Unary(op, input) => {
                let input = self.operand(input, body)?;
                let function = match op {
                    UnaryOp::Abs => "abs",
                    UnaryOp::Square => "square",
                    UnaryOp::Cube => "cube",
                    UnaryOp::HalfNegative => "half_negative",
                    UnaryOp::QuarterNegative => "quarter_negative",
                    UnaryOp::Squeeze => "squeeze",
                    UnaryOp::Negate => "negate",
                };
                body.bind(&format!("d::{function}({input})"))
            }
            Density::Clamp { input, min, max } => {
                let input = self.operand(input, body)?;
                body.bind(&format!(
                    "d::clamp({input}, {}, {})",
                    float(*min)?,
                    float(*max)?
                ))
            }
            Density::Lerp {
                alpha,
                first,
                second,
            } => {
                let alpha = self.operand(alpha, body)?;
                let first = self.lazy(first)?;
                let second = self.lazy(second)?;
                let first = Self::call(&first, body, "x", "y", "z");
                let second = Self::call(&second, body, "x", "y", "z");
                body.values += 1;
                let name = format!("v{}", body.values);
                body.lines.extend([
                    format!("let {name} = if {alpha} == 0.0 {{"),
                    format!("    {first}"),
                    format!("}} else if {alpha} == 1.0 {{"),
                    format!("    {second}"),
                    "} else {".to_owned(),
                ]);
                let between = format!("    d::lerp({alpha}, {first}, {second})");
                if between.len() + BODY_INDENT <= LINE_WIDTH {
                    body.lines.push(between);
                } else {
                    body.lines.extend([
                        "    d::lerp(".to_owned(),
                        format!("        {alpha},"),
                        format!("        {first},"),
                        format!("        {second},"),
                        "    )".to_owned(),
                    ]);
                }
                body.lines.push("};".to_owned());
                name
            }
            Density::RangeChoice {
                input,
                min_inclusive,
                max_exclusive,
                when_in_range,
                when_out_of_range,
            } => {
                let input = self.operand(input, body)?;
                let inside = self.lazy(when_in_range)?;
                let outside = self.lazy(when_out_of_range)?;
                let inside = Self::call(&inside, body, "x", "y", "z");
                let outside = Self::call(&outside, body, "x", "y", "z");
                body.values += 1;
                let name = format!("v{}", body.values);
                body.lines.extend([
                    format!(
                        "let {name} = if {input} >= {} && {input} < {} {{",
                        float(*min_inclusive)?,
                        float(*max_exclusive)?
                    ),
                    format!("    {inside}"),
                    "} else {".to_owned(),
                    format!("    {outside}"),
                    "};".to_owned(),
                ]);
                name
            }
            Density::IntervalSelect {
                input,
                thresholds,
                functions,
            } => {
                let input = self.operand(input, body)?;
                let mut calls = Vec::new();
                for function in functions {
                    let lazy = self.lazy(function)?;
                    calls.push(Self::call(&lazy, body, "x", "y", "z"));
                }
                let Some((last, earlier)) = calls.split_last() else {
                    bail!("an interval select without functions");
                };
                body.values += 1;
                let name = format!("v{}", body.values);
                for (index, (threshold, call)) in thresholds.iter().zip(earlier).enumerate() {
                    let start = if index == 0 {
                        format!("let {name} = if")
                    } else {
                        "} else if".to_owned()
                    };
                    body.lines
                        .push(format!("{start} {input} < {} {{", float(*threshold)?));
                    body.lines.push(format!("    {call}"));
                }
                if thresholds.is_empty() {
                    body.lines.push(format!("let {name} = {last};"));
                } else {
                    body.lines.extend([
                        "} else {".to_owned(),
                        format!("    {last}"),
                        "};".to_owned(),
                    ]);
                }
                name
            }
            Density::Spline(spline) => {
                let number = self.spline(spline)?;
                body.used.add(Used::ALL);
                body.bind(&format!(
                    "self.spline(m, &splines::SPLINE_{number}, x, y, z)"
                ))
            }
            Density::Interpolated {
                input,
                cell_size_xz,
                cell_size_y,
            } => match self.lazy(input)? {
                // Between equal corners lies the same value, to the bit.
                Lazy::Literal(text) => text,
                Lazy::Method(method) => {
                    body.used.add(Used::ALL);
                    body.bind(&format!(
                        "d::interpolate({cell_size_xz}, {cell_size_y}, x, y, z, \
                         &mut |x, y, z| self.{method}(m, x, y, z))"
                    ))
                }
            },
            Density::Cache(input) => {
                let method = self.cache(input)?;
                body.used.add(Used::ALL);
                body.bind(&format!("self.{method}(m, x, y, z)"))
            }
            Density::EndOuterIslands => {
                body.used.x = true;
                body.used.z = true;
                body.bind("self.end_islands.sample(x, z)")
            }
            Density::Slice {
                axis,
                coordinate,
                input,
            } => {
                let lazy = self.lazy(input)?;
                let fixed = coordinate.to_string();
                let call = match axis {
                    Axis::X => Self::call(&lazy, body, &fixed, "y", "z"),
                    Axis::Y => Self::call(&lazy, body, "x", &fixed, "z"),
                    Axis::Z => Self::call(&lazy, body, "x", "y", &fixed),
                };
                match lazy {
                    Lazy::Literal(text) => text,
                    Lazy::Method(_) => body.bind(&call),
                }
            }
            Density::DistanceToPoint { point, metric } => {
                let Metric::Euclidean = metric;
                body.used.x = true;
                body.used.y = true;
                body.used.z = true;
                body.bind(&format!(
                    "d::distance(x, y, z, [{}, {}, {}])",
                    point[0], point[1], point[2]
                ))
            }
            Density::OldBlendedNoise { .. } => {
                body.used.x = true;
                body.used.y = true;
                body.used.z = true;
                body.bind("d::blended(&self.blended_noise, x, y, z)")
            }
            Density::FindTopSurface {
                density,
                upper_bound,
                lower_bound,
                cell_height,
            } => {
                let upper = self.operand(upper_bound, body)?;
                let lazy = self.lazy(density)?;
                // The density is asked at heights of the search's own.
                let at_height = Self::call(&lazy, body, "x", "height", "z");
                let height = match lazy {
                    Lazy::Literal(_) => "_",
                    Lazy::Method(_) => "height",
                };
                body.bind(&format!(
                    "d::find_top_surface({upper}, {lower_bound}, {cell_height}, \
                     &mut |{height}| {at_height})"
                ))
            }
        })
    }

    /// Emits a function the data marks `cache` as a method that remembers its last
    /// value, and gives the method's name. A function that does not change with y is
    /// remembered for its column.
    fn cache(&mut self, input: &Density) -> Result<String> {
        let slot = self.memory_slots;
        self.memory_slots += 1;
        let name = format!("c_{slot}");
        let y = if depends_on_y(input, self.registry) {
            "y"
        } else {
            "0"
        };

        let mut body = Body::default();
        body.used.add(Used {
            memory: true,
            x: true,
            y: y == "y",
            z: true,
        });
        body.lines.extend([
            format!("if let Some(value) = m.recall({slot}, x, {y}, z) {{"),
            "    return value;".to_owned(),
            "}".to_owned(),
        ]);
        let value = self.value(input, &mut body)?;
        body.lines
            .push(format!("m.remember({slot}, x, {y}, z, {value});"));
        body.lines.push(value);
        self.methods.push(Method {
            name: name.clone(),
            doc: None,
            lines: body.lines,
            used: body.used,
        });
        Ok(name)
    }

    /// The number of the static that holds `spline`, emitting it and those nested in
    /// it if they are not there yet.
    fn spline(&mut self, spline: &Spline) -> Result<usize> {
        let coordinate = match self
            .coordinate_numbers
            .iter()
            .position(|known| *known == *spline.coordinate)
        {
            Some(number) => number,
            None => {
                let lazy = self.lazy(&spline.coordinate)?;
                self.coordinates.push(lazy);
                self.coordinate_numbers.push((*spline.coordinate).clone());
                self.coordinates.len() - 1
            }
        };
        ensure!(
            coordinate <= usize::from(u16::MAX),
            "more coordinates of splines than a 16-bit number counts"
        );

        let mut locations = Vec::new();
        let mut derivatives = Vec::new();
        let mut values = Vec::new();
        for point in &spline.points {
            locations.push(float(point.location)?);
            derivatives.push(float(point.derivative)?);
            values.push(match &point.value {
                SplineValue::Constant(value) => {
                    format!("SplineValue::Constant({})", float(*value)?)
                }
                SplineValue::Spline(nested) => {
                    format!("SplineValue::Spline(&SPLINE_{})", self.spline(nested)?)
                }
            });
        }
        let mut text = String::new();
        writeln!(text, "    coordinate: {coordinate},")?;
        writeln!(text, "    locations: &[")?;
        text.push_str(&wrapped(&locations, "        "));
        writeln!(text, "    ],")?;
        writeln!(text, "    derivatives: &[")?;
        text.push_str(&wrapped(&derivatives, "        "));
        writeln!(text, "    ],")?;
        writeln!(text, "    values: &[")?;
        text.push_str(&wrapped(&values, "        "));
        writeln!(text, "    ],")?;

        if let Some(number) = self.spline_numbers.get(&text) {
            return Ok(*number);
        }
        let number = self.splines.len();
        self.spline_numbers.insert(text.clone(), number);
        self.splines.push(text);
        Ok(number)
    }

    fn render_splines(&self, name: &str) -> Option<String> {
        if self.splines.is_empty() {
            return None;
        }
        let mut out = String::new();
        let _ = writeln!(
            out,
            "//! The splines of the density functions of `minecraft:{name}`. A spline that the \
             data has"
        );
        let _ = writeln!(
            out,
            "//! several times is here once. A coordinate is a number that the router's \
             `coordinate`"
        );
        let _ = writeln!(out, "//! method knows.\n");
        let _ = writeln!(out, "use crate::density::{{Spline, SplineValue}};");
        for (number, text) in self.splines.iter().enumerate() {
            let _ = writeln!(out, "\npub static SPLINE_{number}: Spline = Spline {{");
            out.push_str(text);
            let _ = writeln!(out, "}};");
        }
        Some(out)
    }

    fn render(&self, name: &str, settings: &NoiseSettings) -> Result<String> {
        let router = type_name(name);
        let legacy = settings.legacy_random_source;
        let mut fields: Vec<(String, String, String)> = Vec::new();
        let mut seeded_by_name = false;
        for noise in &self.noises {
            let field = identifier(noise)?;
            let made = match legacy_noise(noise, legacy)? {
                Some(which) => format!("d::legacy_nether_noise(seed, {which})"),
                None => {
                    seeded_by_name = true;
                    format!("d::noise(&splitter, &noises::{})", field.to_uppercase())
                }
            };
            fields.push((field, "NormalNoise".to_owned(), made));
        }
        if let Some(scales) = self.blended {
            let scales = scales
                .iter()
                .map(|scale| double(*scale))
                .collect::<Result<Vec<String>>>()?;
            fields.push((
                "blended_noise".to_owned(),
                "BlendedNoise".to_owned(),
                format!(
                    "d::blended_noise(seed, {legacy}, &splitter, [{}])",
                    scales.join(", ")
                ),
            ));
        }
        if self.end_islands {
            fields.push((
                "end_islands".to_owned(),
                "d::EndIslands".to_owned(),
                "d::EndIslands::new(seed)".to_owned(),
            ));
        }
        for (index, (field, _, _)) in fields.iter().enumerate() {
            ensure!(
                fields[..index].iter().all(|(other, _, _)| other != field),
                "two fields of the router would both be {field}"
            );
        }
        let needs_splitter = seeded_by_name || self.blended.is_some();

        let mut out = String::new();
        writeln!(
            out,
            "//! The noise router of the noise settings `minecraft:{name}`: every density function \
             the"
        )?;
        writeln!(
            out,
            "//! settings reach, as a method that gives its value at a block position."
        )?;
        writeln!(out, "//!")?;
        writeln!(
            out,
            "//! A method `f_…` is a density function of the game's registry, `router_…` and \
             `aquifer_…`"
        )?;
        writeln!(
            out,
            "//! are the entries of the settings, `c_…` a function the data marks `cache`, and \
             `h_…` an"
        )?;
        writeln!(
            out,
            "//! operand that is a method of its own because it is not always evaluated, or is \
             long."
        )?;
        writeln!(out)?;

        let mut noise_types: Vec<&str> = Vec::new();
        if self.blended.is_some() {
            noise_types.push("BlendedNoise");
        }
        if !self.noises.is_empty() {
            noise_types.push("NormalNoise");
        }
        match noise_types[..] {
            [] => {}
            [one] => writeln!(out, "use clustine_noise::{one};\n")?,
            _ => writeln!(out, "use clustine_noise::{{{}}};\n", noise_types.join(", "))?,
        }
        let uses_d = !fields.is_empty()
            || self
                .methods
                .iter()
                .any(|method| method.lines.iter().any(|line| line.contains("d::")));
        let mut density_names = vec![
            "AquiferEntry",
            "Memory",
            "NoiseRouter",
            "NoiseSettings",
            "RouterEntry",
        ];
        if !self.coordinates.is_empty() {
            density_names.push("Spline");
        }
        let d = if uses_d { "self as d, " } else { "" };
        let names = format!("{d}{}", density_names.join(", "));
        if names.len() + "use crate::density::{};".len() <= LINE_WIDTH {
            writeln!(out, "use crate::density::{{{names}}};")?;
        } else {
            writeln!(out, "use crate::density::{{")?;
            writeln!(out, "    {names},")?;
            writeln!(out, "}};")?;
        }
        if !seeded_by_name {
            writeln!(out, "use crate::generated::noise_settings;")?;
        } else {
            writeln!(out, "use crate::generated::{{noise_settings, noises}};")?;
        }
        if !self.splines.is_empty() {
            writeln!(out, "\nuse super::splines;")?;
        }

        writeln!(
            out,
            "\n/// What a caller keeps between calls: the last value of each `cache`."
        )?;
        writeln!(out, "type M = Memory<{}>;", self.memory_slots)?;
        writeln!(
            out,
            "\n/// The noise router of `minecraft:{name}` for one seed, with the noises it reads."
        )?;
        if fields.is_empty() {
            writeln!(out, "pub struct {router} {{}}")?;
        } else {
            writeln!(out, "pub struct {router} {{")?;
            for (field, kind, _) in &fields {
                writeln!(out, "    {field}: {kind},")?;
            }
            writeln!(out, "}}")?;
        }

        writeln!(out, "\nimpl NoiseRouter for {router} {{")?;
        writeln!(out, "    type Memory = M;\n")?;
        let seed = if fields.is_empty() { "_seed" } else { "seed" };
        writeln!(out, "    fn new({seed}: i64) -> Self {{")?;
        if needs_splitter {
            writeln!(out, "        let splitter = d::splitter(seed, {legacy});")?;
        }
        if fields.is_empty() {
            writeln!(out, "        Self {{}}")?;
        } else {
            writeln!(out, "        Self {{")?;
            for (field, _, made) in &fields {
                let line = format!("            {field}: {made},");
                if line.len() <= LINE_WIDTH {
                    writeln!(out, "{line}")?;
                } else {
                    writeln!(out, "            {field}:")?;
                    writeln!(out, "                {made},")?;
                }
            }
            writeln!(out, "        }}")?;
        }
        writeln!(out, "    }}\n")?;
        writeln!(out, "    fn settings(&self) -> &'static NoiseSettings {{")?;
        writeln!(out, "        &noise_settings::{}", name.to_uppercase())?;
        writeln!(out, "    }}\n")?;

        writeln!(
            out,
            "    fn sample(&self, m: &mut M, entry: RouterEntry, x: i32, y: i32, z: i32) -> f32 {{"
        )?;
        writeln!(out, "        match entry {{")?;
        for (variant, entry) in ROUTER_VARIANTS.iter().zip(ROUTER_ENTRIES) {
            writeln!(
                out,
                "            RouterEntry::{variant} => self.router_{entry}(m, x, y, z),"
            )?;
        }
        writeln!(out, "        }}")?;
        writeln!(out, "    }}\n")?;

        if settings.aquifers.is_some() {
            writeln!(
                out,
                "    fn aquifer(&self, m: &mut M, entry: AquiferEntry, x: i32, y: i32, z: i32) -> f32 {{"
            )?;
            writeln!(out, "        match entry {{")?;
            for (variant, entry) in AQUIFER_VARIANTS.iter().zip(AQUIFER_ENTRIES) {
                writeln!(
                    out,
                    "            AquiferEntry::{variant} => self.aquifer_{entry}(m, x, y, z),"
                )?;
            }
            writeln!(out, "        }}")?;
            writeln!(out, "    }}\n")?;
        } else {
            writeln!(
                out,
                "    fn aquifer(&self, _m: &mut M, _entry: AquiferEntry, _x: i32, _y: i32, _z: i32) \
                 -> f32 {{"
            )?;
            writeln!(out, "        0.0")?;
            writeln!(out, "    }}\n")?;
        }

        if self.named.is_empty() {
            writeln!(
                out,
                "    fn named(&self, _m: &mut M, _name: &str, _x: i32, _y: i32, _z: i32) -> \
                 Option<f32> {{"
            )?;
            writeln!(out, "        None")?;
            writeln!(out, "    }}")?;
        } else {
            writeln!(
                out,
                "    fn named(&self, m: &mut M, name: &str, x: i32, y: i32, z: i32) -> Option<f32> {{"
            )?;
            writeln!(out, "        Some(match name {{")?;
            for (function, method) in &self.named {
                writeln!(out, "            {function:?} => {{")?;
                writeln!(out, "                self.{method}(m, x, y, z)")?;
                writeln!(out, "            }}")?;
            }
            writeln!(out, "            _ => return None,")?;
            writeln!(out, "        }})")?;
            writeln!(out, "    }}")?;
        }
        writeln!(out, "}}")?;

        writeln!(out, "\nimpl {router} {{")?;
        let mut first = true;
        if !self.coordinates.is_empty() {
            first = false;
            writeln!(
                out,
                "    /// A spline at a block position, its coordinates being density functions."
            )?;
            writeln!(
                out,
                "    fn spline(&self, m: &mut M, spline: &Spline, x: i32, y: i32, z: i32) -> f32 {{"
            )?;
            writeln!(
                out,
                "        d::spline(spline, &mut |number| self.coordinate(m, number, x, y, z))"
            )?;
            writeln!(out, "    }}\n")?;
            writeln!(
                out,
                "    /// The coordinate of a spline by the number the spline's static has for it."
            )?;
            writeln!(
                out,
                "    fn coordinate(&self, m: &mut M, number: u16, x: i32, y: i32, z: i32) -> f32 {{"
            )?;
            writeln!(out, "        match number {{")?;
            let last = self.coordinates.len() - 1;
            for (number, lazy) in self.coordinates.iter().enumerate() {
                let call = match lazy {
                    Lazy::Literal(text) => text.clone(),
                    Lazy::Method(method) => format!("self.{method}(m, x, y, z)"),
                };
                if number == last {
                    writeln!(out, "            _ => {call},")?;
                } else {
                    writeln!(out, "            {number} => {call},")?;
                }
            }
            writeln!(out, "        }}")?;
            writeln!(out, "    }}")?;
            ensure!(
                self.coordinates
                    .iter()
                    .any(|lazy| matches!(lazy, Lazy::Method(_))),
                "splines whose coordinates are all numbers"
            );
        }
        for method in &self.methods {
            if !first {
                writeln!(out)?;
            }
            first = false;
            if let Some(doc) = &method.doc {
                writeln!(out, "    /// {doc}")?;
            }
            let parameter = |used: bool, name: &str| {
                if used {
                    name.to_owned()
                } else {
                    format!("_{name}")
                }
            };
            let parameters = [
                "&self".to_owned(),
                format!("{}: &mut M", parameter(method.used.memory, "m")),
                format!("{}: i32", parameter(method.used.x, "x")),
                format!("{}: i32", parameter(method.used.y, "y")),
                format!("{}: i32", parameter(method.used.z, "z")),
            ];
            let line = format!(
                "    fn {}({}) -> f32 {{",
                method.name,
                parameters.join(", ")
            );
            if line.len() <= LINE_WIDTH {
                writeln!(out, "{line}")?;
            } else {
                writeln!(out, "    fn {}(", method.name)?;
                for parameter in &parameters {
                    writeln!(out, "        {parameter},")?;
                }
                writeln!(out, "    ) -> f32 {{")?;
            }
            for line in &method.lines {
                writeln!(out, "        {line}")?;
            }
            writeln!(out, "    }}")?;
        }
        writeln!(out, "}}")?;
        Ok(out)
    }
}

/// The noises that a dimension with the old random numbers does not seed by their
/// names, with the number the game adds to the world's seed for each.
pub const LEGACY_NOISES: [(&str, u8); 2] = [
    ("minecraft:nether/temperature", 0),
    ("minecraft:nether/vegetation", 1),
];

/// How the noise `name` is made in a dimension that has the old random numbers or not
/// (`legacy`): `Some` with the number added to the seed for one of [`LEGACY_NOISES`],
/// `None` for a noise seeded by its name.
///
/// The game also makes its offset noise another way in a dimension with the old
/// random numbers (by SteelMC's reading; no such dimension of the jar's data asks
/// it, so nothing here has been compared with the game). It fails rather than guess.
fn legacy_noise(name: &str, legacy: bool) -> Result<Option<u8>> {
    if !legacy {
        return Ok(None);
    }
    ensure!(
        name != "minecraft:offset",
        "a dimension with the old random numbers asks {name}, which the game then makes in a \
         way that was never compared with the game's"
    );
    Ok(LEGACY_NOISES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, which)| *which))
}

/// Fails unless the noises of [`LEGACY_NOISES`] have in `noises` the parameters the
/// game makes them with in a dimension with the old random numbers: two octaves from
/// -7 that count alike. The emitted code does not read the parameters of those two,
/// so data that changed them would otherwise change nothing.
pub fn check_legacy_noises(noises: &BTreeMap<String, NoiseParameters>) -> Result<()> {
    for (name, _) in LEGACY_NOISES {
        let parameters = noises
            .get(name)
            .with_context(|| format!("worldgen/noise has no {name}"))?;
        ensure!(
            parameters.base_octave == -7
                && parameters.octave_count == 2
                && parameters.normalise
                && parameters
                    .amplitude_modifiers
                    .iter()
                    .all(|modifier| *modifier == 1.0),
            "{name} is no longer two octaves from -7 that count alike: {parameters:?}"
        );
    }
    Ok(())
}

/// The variants of the hand-written enums, in the order of the entries' names.
const ROUTER_VARIANTS: [&str; 8] = [
    "Temperature",
    "Vegetation",
    "Continents",
    "Erosion",
    "Depth",
    "Ridges",
    "ChunkSurfaceLevel",
    "FinalDensity",
];
const AQUIFER_VARIANTS: [&str; 6] = [
    "Barrier",
    "FluidLevelFloodedness",
    "FluidLevelSpread",
    "Lava",
    "Exclusion",
    "SurfaceLevel",
];

/// `noises.rs`: the parameters of every noise of the game's registry, by name.
pub fn noises(noises: &BTreeMap<String, NoiseParameters>) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "//! The parameters of every noise of the game's registry `worldgen/noise`, by name."
    )?;
    writeln!(out, "\nuse crate::density::NoiseParameters;")?;
    let mut statics = Vec::new();
    for (name, parameters) in noises {
        let constant = identifier(name)?.to_uppercase();
        ensure!(
            !statics.contains(&constant),
            "two noises would both be the static {constant}"
        );
        writeln!(
            out,
            "\npub static {constant}: NoiseParameters = NoiseParameters {{"
        )?;
        writeln!(out, "    name: {name:?},")?;
        writeln!(out, "    base_octave: {},", parameters.base_octave)?;
        writeln!(
            out,
            "    base_amplitude: {},",
            double(parameters.base_amplitude)?
        )?;
        writeln!(out, "    octave_count: {},", parameters.octave_count)?;
        writeln!(out, "    normalise: {},", parameters.normalise)?;
        let modifiers = parameters
            .amplitude_modifiers
            .iter()
            .map(|modifier| double(*modifier))
            .collect::<Result<Vec<String>>>()?;
        writeln!(out, "    amplitude_modifiers: &[{}],", modifiers.join(", "))?;
        writeln!(out, "}};")?;
        statics.push(constant);
    }
    writeln!(out, "\n/// Every noise, in the order of their names.")?;
    writeln!(
        out,
        "pub static NOISES: [&NoiseParameters; {}] = [",
        statics.len()
    )?;
    let references: Vec<String> = statics.iter().map(|name| format!("&{name}")).collect();
    out.push_str(&wrapped(&references, "    "));
    writeln!(out, "];")?;
    check_layout(&out)?;
    Ok(out)
}

/// `noise_settings.rs`: what the noise settings hold beside their functions.
pub fn noise_settings(settings: &[(&str, &NoiseSettings)]) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "//! What the noise settings of the three dimensions hold beside their density functions."
    )?;
    writeln!(out, "\nuse crate::density::{{NoiseSettings, SpawnRange}};")?;
    for (name, settings) in settings {
        writeln!(
            out,
            "\npub static {}: NoiseSettings = NoiseSettings {{",
            name.to_uppercase()
        )?;
        writeln!(out, "    name: {:?},", format!("minecraft:{name}"))?;
        writeln!(out, "    min_y: {},", settings.min_y)?;
        writeln!(out, "    height: {},", settings.height)?;
        writeln!(out, "    sea_level: {},", settings.sea_level)?;
        writeln!(
            out,
            "    legacy_random_source: {},",
            settings.legacy_random_source
        )?;
        writeln!(
            out,
            "    disable_mob_generation: {},",
            settings.disable_mob_generation
        )?;
        writeln!(out, "    has_aquifers: {},", settings.aquifers.is_some())?;
        writeln!(out, "    default_block: {},", settings.default_block)?;
        writeln!(out, "    default_fluid: {},", settings.default_fluid)?;
        writeln!(out, "    material_rule: {:?},", settings.material_rule)?;
        if settings.spawn_target.is_empty() {
            writeln!(out, "    spawn_target: &[],")?;
        } else {
            writeln!(out, "    spawn_target: &[")?;
            for point in &settings.spawn_target {
                writeln!(out, "        &[")?;
                for (function, min, max) in point {
                    writeln!(
                        out,
                        "            SpawnRange {{ function: {function:?}, min: {}, max: {} }},",
                        float(*min)?,
                        float(*max)?
                    )?;
                }
                writeln!(out, "        ],")?;
            }
            writeln!(out, "    ],")?;
        }
        writeln!(out, "}};")?;
    }
    check_layout(&out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Json;

    fn density(text: &str) -> Density {
        Density::parse(&Json::parse(text).unwrap()).unwrap()
    }

    /// The noises the tests' functions ask, with parameters that do not matter here.
    fn known_noises() -> BTreeMap<String, NoiseParameters> {
        [
            "minecraft:erosion",
            "minecraft:offset",
            "minecraft:nether/temperature",
            "minecraft:nether/vegetation",
        ]
        .into_iter()
        .map(|name| {
            let parameters = NoiseParameters {
                base_octave: -7,
                base_amplitude: 1.0,
                octave_count: 2,
                normalise: true,
                amplitude_modifiers: Vec::new(),
            };
            (name.to_owned(), parameters)
        })
        .collect()
    }

    fn registry(entries: &[(&str, &str)]) -> Registry {
        entries
            .iter()
            .map(|(name, text)| ((*name).to_owned(), density(text)))
            .collect()
    }

    /// Settings whose router is zero but for the final density.
    fn settings(final_density: &str, aquifers: bool) -> NoiseSettings {
        let zero = Density::Constant(0.0);
        let mut router = vec![zero.clone(); 8];
        router[7] = density(final_density);
        NoiseSettings {
            min_y: 0,
            height: 128,
            sea_level: 32,
            legacy_random_source: false,
            disable_mob_generation: false,
            default_block: 1,
            default_fluid: 2,
            material_rule: "minecraft:rule".to_owned(),
            router,
            aquifers: aquifers.then(|| vec![zero; 6]),
            spawn_target: Vec::new(),
        }
    }

    const SMALL: &[(&str, &str)] = &[
        (
            "minecraft:a/noise",
            r#"{"type": "minecraft:cache", "input": {"type": "minecraft:noise",
                "noise": "minecraft:erosion", "xz_scale": 0.25, "y_scale": 0.0,
                "shift_x": {"type": "minecraft:shift_a", "noise": "minecraft:offset"},
                "shift_z": {"type": "minecraft:shift_b", "noise": "minecraft:offset"}}}"#,
        ),
        (
            "minecraft:a/spline",
            r#"{"type": "minecraft:spline", "spline": {"coordinate": "minecraft:a/noise", "points": [
                {"location": -1.0, "derivative": 0.0, "value": 0.5},
                {"location": 1.0, "derivative": 2.0, "value": {"coordinate": "minecraft:a/noise",
                    "points": [{"location": 0.0, "derivative": 0.0, "value": 1.0}]}}]}}"#,
        ),
        ("minecraft:unused", "3.0"),
    ];

    const FINAL: &str = r#"{"type": "minecraft:add",
        "left": {"type": "minecraft:squeeze", "input": {"type": "minecraft:interpolated",
            "cell_size_xz": 4, "cell_size_y": 8, "input": {"type": "minecraft:range_choice",
                "input": "minecraft:a/spline", "min_inclusive": -1.5, "max_exclusive": 0.25,
                "when_in_range": {"type": "minecraft:lerp",
                    "alpha": {"type": "minecraft:gradient", "axis": "y", "from_coordinate": -8,
                        "to_coordinate": 24, "from_value": 0.0, "to_value": 1.0},
                    "first": 2.5,
                    "second": {"type": "minecraft:old_blended_noise", "xz_scale": 0.25,
                        "y_scale": 0.125, "xz_factor": 80.0, "y_factor": 160.0,
                        "smear_scale_multiplier": 8.0}},
                "when_out_of_range": {"type": "minecraft:slice", "axis": "y", "coordinate": 0,
                    "input": {"type": "minecraft:min", "left": "minecraft:a/noise",
                        "right": {"type": "minecraft:end_outer_islands"}}}}}},
        "right": {"type": "minecraft:beardifier"}}"#;

    #[test]
    fn two_runs_over_the_same_data_give_the_same_bytes() {
        let registry = registry(SMALL);
        let settings = settings(FINAL, true);
        let first = emit("small", &settings, &registry, &known_noises()).unwrap();
        let second = emit("small", &settings, &registry, &known_noises()).unwrap();
        assert!(first.router == second.router);
        assert!(first.splines == second.splines);
    }

    #[test]
    fn a_router_has_a_method_for_each_function_it_reaches_and_no_other() {
        let registry = registry(SMALL);
        let emitted = emit("small", &settings(FINAL, true), &registry, &known_noises()).unwrap();
        let router = &emitted.router;
        assert!(router.contains("pub struct SmallRouter {"), "{router}");
        assert!(router.contains("    /// `minecraft:a/noise`\n    fn f_a_noise("));
        assert!(router.contains("fn f_a_spline("));
        assert!(!router.contains("unused"));
        for entry in ROUTER_ENTRIES {
            assert!(router.contains(&format!("fn router_{entry}(")), "{entry}");
        }
        for entry in AQUIFER_ENTRIES {
            assert!(router.contains(&format!("fn aquifer_{entry}(")), "{entry}");
        }
        // The fields: the two noises by name, then the two special ones.
        let fields = "    erosion: NormalNoise,\n    offset: NormalNoise,\n    \
                      blended_noise: BlendedNoise,\n    end_islands: d::EndIslands,\n";
        assert!(router.contains(fields), "{router}");
        assert!(router.contains("let splitter = d::splitter(seed, false);"));
        // A field whose line would be too long has what makes it on the next line.
        assert!(router.contains(
            "            blended_noise:\n                d::blended_noise(seed, false, &splitter, [0.25, 0.125, 80.0, 160.0, 8.0]),\n"
        ));
        assert!(router.contains("erosion: d::noise(&splitter, &noises::EROSION),"));
        assert!(router.contains("&noise_settings::SMALL"));
    }

    #[test]
    fn each_kind_of_function_is_emitted_as_the_call_that_computes_it() {
        let registry = registry(SMALL);
        let router = emit("small", &settings(FINAL, false), &registry, &known_noises())
            .unwrap()
            .router;
        for expected in [
            "let v1 = d::shift_a(&self.offset, x, z);",
            "let v2 = d::shift_b(&self.offset, x, z);",
            "let v3 = d::shifted_noise2(&self.erosion, x, z, 0.25, v1, v2);",
            // The cached noise does not change with y, so it is remembered by column.
            "if let Some(value) = m.recall(0, x, 0, z) {",
            "m.remember(0, x, 0, z, v3);",
            "self.spline(m, &splines::SPLINE_1, x, y, z)",
            "d::interpolate(4, 8, x, y, z, &mut |x, y, z| self.h_1(m, x, y, z));",
            "if v1 >= -1.5 && v1 < 0.25 {",
            "let v1 = d::gradient(y, -8, 24, 0.0, 1.0);",
            "let v2 = if v1 == 0.0 {",
            "    2.5_f32",
            "    d::lerp(v1, 2.5_f32, self.h_3(m, x, y, z))",
            "d::blended(&self.blended_noise, x, y, z)",
            // The slice's input is asked at the fixed height.
            "(m, x, 0, z);",
            "self.end_islands.sample(x, z);",
            "d::squeeze(v1);",
            " + d::NO_BEARD;",
            // A dimension without aquifers answers zero.
            "fn aquifer(&self, _m: &mut M, _entry: AquiferEntry, _x: i32, _y: i32, _z: i32) -> f32 {",
            "            \"minecraft:a/noise\" => {\n                self.f_a_noise(m, x, y, z)",
            "            _ => return None,",
            "type M = Memory<1>;",
        ] {
            assert!(router.contains(expected), "{expected}\n{router}");
        }
        // A parameter the body does not name has an underscore.
        assert!(
            router.contains("fn router_temperature(&self, _m: &mut M, _x: i32, _y: i32, _z: i32)")
        );
        assert!(router.contains("        0.0_f32\n    }"));
    }

    #[test]
    fn a_spline_that_is_there_twice_is_one_static_and_a_nested_one_comes_first() {
        let mut entries = SMALL.to_vec();
        entries.push(("minecraft:a/twice", SMALL[1].1));
        let registry = registry(&entries);
        let final_density = r#"{"type": "minecraft:add", "left": "minecraft:a/spline",
            "right": "minecraft:a/twice"}"#;
        let emitted = emit(
            "small",
            &settings(final_density, false),
            &registry,
            &known_noises(),
        )
        .unwrap();
        let splines = emitted.splines.unwrap();
        assert_eq!(splines.matches("pub static SPLINE_").count(), 2);
        let nested = splines.find("pub static SPLINE_0").unwrap();
        let outer = splines.find("pub static SPLINE_1").unwrap();
        assert!(nested < outer);
        assert!(
            splines.contains(
                "    coordinate: 0,\n    locations: &[\n        -1.0, 1.0,\n    ],\n    \
             derivatives: &[\n        0.0, 2.0,\n    ],\n    values: &[\n        \
             SplineValue::Constant(0.5), SplineValue::Spline(&SPLINE_0),\n    ],\n"
            ),
            "{splines}"
        );
        assert!(emitted.router.contains("_ => self.f_a_noise(m, x, y, z),"));
    }

    #[test]
    fn a_router_without_noises_or_splines_names_nothing_it_does_not_use() {
        let emitted = emit(
            "flat",
            &settings("1.5", false),
            &Registry::new(),
            &known_noises(),
        )
        .unwrap();
        assert!(emitted.splines.is_none());
        let router = emitted.router;
        assert!(router.contains("pub struct FlatRouter {}"), "{router}");
        assert!(router.contains("fn new(_seed: i64) -> Self {\n        Self {}"));
        assert!(!router.contains("clustine_noise") && !router.contains("splitter"));
        assert!(!router.contains("self as d") && !router.contains("noises::"));
        assert!(router.contains("fn named(&self, _m: &mut M, _name: &str"));
        assert!(router.contains("type M = Memory<0>;"));
    }

    #[test]
    fn a_dimension_with_the_old_random_numbers_makes_the_nethers_two_noises_from_the_seed() {
        let noise = |name: &str| {
            format!(
                r#"{{"type": "minecraft:noise", "noise": "{name}", "xz_scale": 0.25, "y_scale": 0.0}}"#
            )
        };
        let mut legacy = settings(&noise("minecraft:nether/temperature"), false);
        legacy.legacy_random_source = true;
        legacy.router[1] = density(&noise("minecraft:nether/vegetation"));
        let router = emit("old", &legacy, &Registry::new(), &known_noises())
            .unwrap()
            .router;
        assert!(
            router.contains("nether_temperature: d::legacy_nether_noise(seed, 0),"),
            "{router}"
        );
        assert!(router.contains("nether_vegetation: d::legacy_nether_noise(seed, 1),"));
        // Neither is seeded by its name, so there is no factory and no parameters.
        assert!(!router.contains("splitter") && !router.contains("noises::"));

        // Another noise of such a dimension is seeded by its name, from the old
        // generator's factory.
        legacy.router[2] = density(&noise("minecraft:erosion"));
        let router = emit("old", &legacy, &Registry::new(), &known_noises())
            .unwrap()
            .router;
        assert!(router.contains("let splitter = d::splitter(seed, true);"));
        assert!(router.contains("erosion: d::noise(&splitter, &noises::EROSION),"));

        // With today's random numbers the two are noises like any other.
        legacy.legacy_random_source = false;
        let router = emit("new", &legacy, &Registry::new(), &known_noises())
            .unwrap()
            .router;
        assert!(
            router
                .contains("nether_temperature: d::noise(&splitter, &noises::NETHER_TEMPERATURE),")
        );

        // What was never compared with the game fails rather than guesses.
        let mut offset = settings(&noise("minecraft:offset"), false);
        offset.legacy_random_source = true;
        let error = emit("old", &offset, &Registry::new(), &known_noises())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("never compared"));
    }

    #[test]
    fn the_nethers_two_noises_have_to_keep_the_parameters_the_emitted_code_assumes() {
        let two_octaves = NoiseParameters {
            base_octave: -7,
            base_amplitude: 0.9494731054427981,
            octave_count: 2,
            normalise: true,
            amplitude_modifiers: Vec::new(),
        };
        let mut all = BTreeMap::new();
        all.insert(
            "minecraft:nether/temperature".to_owned(),
            two_octaves.clone(),
        );
        assert!(
            check_legacy_noises(&all).is_err(),
            "one of the two is missing"
        );
        all.insert(
            "minecraft:nether/vegetation".to_owned(),
            two_octaves.clone(),
        );
        assert!(check_legacy_noises(&all).is_ok());
        for changed in [
            NoiseParameters {
                base_octave: -8,
                ..two_octaves.clone()
            },
            NoiseParameters {
                octave_count: 3,
                ..two_octaves.clone()
            },
            NoiseParameters {
                normalise: false,
                ..two_octaves.clone()
            },
            NoiseParameters {
                amplitude_modifiers: vec![1.0, 0.5],
                ..two_octaves.clone()
            },
        ] {
            all.insert("minecraft:nether/vegetation".to_owned(), changed);
            assert!(check_legacy_noises(&all).is_err());
        }
    }

    #[test]
    fn a_noise_that_the_registry_lacks_fails_the_emitter() {
        let asks = r#"{"type": "minecraft:noise", "noise": "minecraft:nothing", "xz_scale": 1.0, "y_scale": 1.0}"#;
        let error = emit(
            "small",
            &settings(asks, false),
            &Registry::new(),
            &known_noises(),
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("the noise minecraft:nothing is referred to"));
    }

    #[test]
    fn a_reference_that_names_nothing_fails_the_emitter() {
        let error = emit(
            "small",
            &settings("\"minecraft:nothing\"", false),
            &Registry::new(),
            &known_noises(),
        )
        .err()
        .unwrap();
        assert!(format!("{error:#}").contains("minecraft:nothing"));
    }

    #[test]
    fn a_long_chain_is_split_into_methods_below_the_budget() {
        // 120 additions in a row are 240 statements if written into one body. (The
        // reader of JSON nests no deeper than 256, so the chain is built directly.)
        let mut chain = Density::Constant(1.0);
        for _ in 0..120 {
            chain = Density::Binary(
                BinaryOp::Add,
                Box::new(chain),
                Box::new(Density::Gradient {
                    axis: Axis::Y,
                    from: 0,
                    to: 1,
                    from_value: 0.0,
                    to_value: 1.0,
                }),
            );
        }
        let mut settings = settings("0.0", false);
        settings.router[7] = chain;
        let router = emit("long", &settings, &Registry::new(), &known_noises())
            .unwrap()
            .router;
        let longest = longest_function(&router);
        assert!(longest <= INLINE_STATEMENTS + 8, "{longest}");
        assert_eq!(router.matches("d::gradient(").count(), 120);
        // And not a method for every addition: each holds about as much as may be
        // written into one body.
        let methods = router.matches("    fn h_").count();
        assert!((8..=14).contains(&methods), "{methods}");
    }

    #[test]
    fn the_longest_function_is_counted_from_its_first_line_to_its_closing_brace() {
        let text = "impl A {\n    fn short(&self) -> f32 {\n        0.0\n    }\n\n    \
                    pub fn long(&self) -> f32 {\n        let a = if b {\n            1.0\n        \
                    } else {\n            2.0\n        };\n        a\n    }\n}\n";
        assert_eq!(longest_function(text), 8);
        assert_eq!(longest_function("static A: u8 = 1;\n"), 0);
        assert!(check_layout(text).is_ok());
        let long_line = format!("// {}\n", "x".repeat(LINE_WIDTH));
        assert!(check_layout(&long_line).is_err());
        let long_literal = format!("    {:?} => {{\n", "x".repeat(LINE_WIDTH));
        assert!(
            check_layout(&long_literal).is_ok(),
            "a literal may make a line long"
        );
        let mut long_function = "fn f() {\n".to_owned();
        long_function.push_str(&"    a();\n".repeat(FUNCTION_LINES));
        long_function.push_str("}\n");
        let error = check_layout(&long_function).unwrap_err();
        assert!(format!("{error}").contains("over the budget"));
    }

    #[test]
    fn floats_are_written_as_the_shortest_decimal_that_reads_back() {
        assert_eq!(float(1.0).unwrap(), "1.0");
        assert_eq!(float(-0.0).unwrap(), "-0.0");
        assert_eq!(float(-0.225).unwrap(), "-0.225");
        assert_eq!(float(-1.0 / 3.0).unwrap(), "-0.33333334");
        assert_eq!(float(0.0078125).unwrap(), "0.0078125");
        assert_eq!(float(1.0e-7).unwrap(), "0.0000001");
        assert_eq!(float(16_777_216.0).unwrap(), "16777216.0");
        assert_eq!(double(5.0 / 7.0).unwrap(), "0.7142857142857143");
        assert_eq!(double(1.063180125160734).unwrap(), "1.063180125160734");
        assert!(float(f32::NAN).is_err() && float(f32::INFINITY).is_err());
        assert!(double(f64::NEG_INFINITY).is_err());
        assert_eq!(literal(2.5).unwrap(), "2.5_f32");
    }

    #[test]
    fn names_of_the_game_become_rust_names_and_others_are_refused() {
        assert_eq!(
            identifier("minecraft:overworld/caves/spaghetti_2d").unwrap(),
            "overworld_caves_spaghetti_2d"
        );
        assert!(identifier("other:thing").is_err());
        assert!(identifier("minecraft:").is_err());
        assert!(identifier("minecraft:a-b").is_err());
        assert_eq!(type_name("overworld"), "OverworldRouter");
        assert_eq!(type_name("large_biomes"), "LargeBiomesRouter");
    }

    #[test]
    fn items_are_wrapped_at_the_line_width() {
        let items: Vec<String> = (0..40).map(|n| format!("{n}.25")).collect();
        let text = wrapped(&items, "    ");
        assert!(text.lines().count() > 1);
        assert!(
            text.lines()
                .all(|line| line.len() <= LINE_WIDTH && line.starts_with("    "))
        );
        assert!(text.lines().all(|line| line.ends_with(',')));
        assert_eq!(wrapped(&[], "    "), "");
    }

    #[test]
    fn noises_and_settings_are_statics_with_every_field() {
        let mut all = BTreeMap::new();
        all.insert(
            "minecraft:nether/temperature".to_owned(),
            NoiseParameters {
                base_octave: -7,
                base_amplitude: 0.9494731054427981,
                octave_count: 2,
                normalise: true,
                amplitude_modifiers: vec![1.0, 0.0],
            },
        );
        let text = noises(&all).unwrap();
        assert!(
            text.contains(
                "pub static NETHER_TEMPERATURE: NoiseParameters = NoiseParameters {\n    \
             name: \"minecraft:nether/temperature\",\n    base_octave: -7,\n    \
             base_amplitude: 0.9494731054427981,\n    octave_count: 2,\n    normalise: true,\n    \
             amplitude_modifiers: &[1.0, 0.0],\n};"
            ),
            "{text}"
        );
        assert!(text.contains(
            "pub static NOISES: [&NoiseParameters; 1] = [\n    &NETHER_TEMPERATURE,\n];"
        ));

        let mut one = settings("0.0", true);
        one.spawn_target = vec![vec![("minecraft:a/noise".to_owned(), -0.11, 1.0)]];
        let text = noise_settings(&[("small", &one)]).unwrap();
        assert!(text.contains("pub static SMALL: NoiseSettings = NoiseSettings {"));
        assert!(text.contains("    name: \"minecraft:small\",\n    min_y: 0,\n    height: 128,\n"));
        assert!(
            text.contains(
                "    has_aquifers: true,\n    default_block: 1,\n    default_fluid: 2,\n"
            )
        );
        assert!(text.contains(
            "            SpawnRange { function: \"minecraft:a/noise\", min: -0.11, max: 1.0 },"
        ));
    }
}
