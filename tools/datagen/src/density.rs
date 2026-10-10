// Adapted from SteelMC, steel-worldgen/build/density/types.rs and
// steel-worldgen/build/density/functions.rs at 885c4b3 (AGPL-3.0-or-later, Copyright
// (C) 2026 Alve Jeansson and contributors; see NOTICE). Changed for Clustine in
// October 2026: read from the jar's own files through a reader that keeps numbers as
// text, floats checked against the way through a double, the types and the fields the
// released 26.3 has (`aquifers.exclusion`, `noise_router.chunk_surface_level`, no vein
// entries), and a type the data uses and this does not know fails the run.

//! The density functions of the game's world-generation data, as the emitter of noise
//! routers reads them (ADR-0019, section 1, row 5).
//!
//! A density function is a tree: a number, the name of another function, or an object
//! with a `type`. Only the types that the jar's data uses are known here; another one
//! fails the run and says where it stands, so that a new version of the game cannot
//! add one in silence.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;

#[derive(Clone, Debug, PartialEq)]
pub enum Density {
    Constant(f32),
    /// Another function of the registry, by its name.
    Reference(String),
    /// A value that runs evenly from `from_value` at `from` to `to_value` at `to` along
    /// an axis, and stays at the nearer end outside.
    Gradient {
        axis: Axis,
        from: i32,
        to: i32,
        from_value: f32,
        to_value: f32,
    },
    Noise {
        noise: String,
        xz_scale: f64,
        y_scale: f64,
        /// What is added to the scaled x and z before the noise is asked; `None`
        /// where the data gives neither.
        shift: Option<Box<[Density; 2]>>,
    },
    /// `shift_a` and `shift_b`: the offset noise read flat, in two orientations.
    ShiftA(String),
    ShiftB(String),
    Binary(BinaryOp, Box<Density>, Box<Density>),
    Unary(UnaryOp, Box<Density>),
    Clamp {
        input: Box<Density>,
        min: f32,
        max: f32,
    },
    Lerp {
        alpha: Box<Density>,
        first: Box<Density>,
        second: Box<Density>,
    },
    RangeChoice {
        input: Box<Density>,
        min_inclusive: f32,
        max_exclusive: f32,
        when_in_range: Box<Density>,
        when_out_of_range: Box<Density>,
    },
    /// `functions[i]` where the input is below `thresholds[i]` and not below the one
    /// before; the last function above them all.
    IntervalSelect {
        input: Box<Density>,
        thresholds: Vec<f32>,
        functions: Vec<Density>,
    },
    Spline(Spline),
    /// The input at the corners of a cell, interpolated within it.
    Interpolated {
        input: Box<Density>,
        cell_size_xz: i32,
        cell_size_y: i32,
    },
    /// Says that the value is worth remembering; it changes no value.
    Cache(Box<Density>),
    BlendAlpha,
    BlendOffset,
    Blend(Box<Density>),
    /// What structures add to the terrain around them.
    Beardifier,
    EndOuterIslands,
    Slice {
        axis: Axis,
        coordinate: i32,
        input: Box<Density>,
    },
    DistanceToPoint {
        point: [i32; 3],
        metric: Metric,
    },
    OldBlendedNoise {
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_scale_multiplier: f64,
    },
    /// The highest multiple of `cell_height`, from `upper_bound` down to
    /// `lower_bound`, at which `density` is positive.
    FindTopSurface {
        density: Box<Density>,
        upper_bound: Box<Density>,
        lower_bound: i32,
        cell_height: i32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Abs,
    Square,
    Cube,
    HalfNegative,
    QuarterNegative,
    Squeeze,
    Negate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Euclidean,
}

/// A cubic spline: a coordinate, and at each location a value, which is a number or
/// another spline, with the slope there.
#[derive(Clone, Debug, PartialEq)]
pub struct Spline {
    pub coordinate: Box<Density>,
    pub points: Vec<SplinePoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplinePoint {
    pub location: f32,
    pub derivative: f32,
    pub value: SplineValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SplineValue {
    Constant(f32),
    Spline(Spline),
}

impl Density {
    /// Reads one density function as the data writes it.
    pub fn parse(json: &Json) -> Result<Self> {
        match json {
            Json::Number(_) => Ok(Self::Constant(json.float()?)),
            Json::String(name) => Ok(Self::Reference(name.clone())),
            Json::Object(_) => {
                let kind = json.member("type")?.string()?;
                Self::parse_typed(kind, json).with_context(|| format!("in a {kind}"))
            }
            other => bail!("a density function cannot be {other:?}"),
        }
    }

    fn parse_typed(kind: &str, json: &Json) -> Result<Self> {
        let child = |key: &str| -> Result<Box<Density>> {
            Ok(Box::new(
                Self::parse(json.member(key)?).with_context(|| format!("in its {key}"))?,
            ))
        };
        let float = |key: &str| {
            json.member(key)?
                .float()
                .with_context(|| format!("in its {key}"))
        };
        let double = |key: &str| {
            json.member(key)?
                .double()
                .with_context(|| format!("in its {key}"))
        };
        let integer = |key: &str| {
            json.member(key)?
                .integer()
                .with_context(|| format!("in its {key}"))
        };
        let axis = |key: &str| -> Result<Axis> {
            match json.member(key)?.string()? {
                "x" => Ok(Axis::X),
                "y" => Ok(Axis::Y),
                "z" => Ok(Axis::Z),
                other => bail!("{other:?} is no axis"),
            }
        };
        // Every member has to be one that is read: a field this does not know would
        // otherwise change what the game computes and not what is emitted.
        let only = |known: &[&str]| -> Result<()> {
            for (key, _) in json.members()? {
                ensure!(
                    key == "type" || known.contains(&key.as_str()),
                    "the member {key:?} is not known to the emitter"
                );
            }
            Ok(())
        };
        let binary = |op: BinaryOp| -> Result<Self> {
            only(&["left", "right"])?;
            Ok(Self::Binary(op, child("left")?, child("right")?))
        };
        let unary = |op: UnaryOp| -> Result<Self> {
            only(&["input"])?;
            Ok(Self::Unary(op, child("input")?))
        };

        let name = kind
            .strip_prefix("minecraft:")
            .with_context(|| format!("{kind} is not one of the game's own types"))?;
        match name {
            "add" => binary(BinaryOp::Add),
            "sub" => binary(BinaryOp::Sub),
            "mul" => binary(BinaryOp::Mul),
            "div" => binary(BinaryOp::Div),
            "min" => binary(BinaryOp::Min),
            "max" => binary(BinaryOp::Max),
            "abs" => unary(UnaryOp::Abs),
            "square" => unary(UnaryOp::Square),
            "cube" => unary(UnaryOp::Cube),
            "half_negative" => unary(UnaryOp::HalfNegative),
            "quarter_negative" => unary(UnaryOp::QuarterNegative),
            "squeeze" => unary(UnaryOp::Squeeze),
            "negate" => unary(UnaryOp::Negate),
            "gradient" => {
                only(&[
                    "axis",
                    "from_coordinate",
                    "to_coordinate",
                    "from_value",
                    "to_value",
                ])?;
                let (from, to) = (integer("from_coordinate")?, integer("to_coordinate")?);
                ensure!(from < to, "a gradient from {from} to {to}");
                Ok(Self::Gradient {
                    axis: axis("axis")?,
                    from,
                    to,
                    from_value: float("from_value")?,
                    to_value: float("to_value")?,
                })
            }
            "noise" => {
                only(&["noise", "xz_scale", "y_scale", "shift_x", "shift_z"])?;
                let shift = match (json.get("shift_x"), json.get("shift_z")) {
                    (None, None) => None,
                    (Some(_), Some(_)) => Some(Box::new([*child("shift_x")?, *child("shift_z")?])),
                    _ => bail!("one of shift_x and shift_z without the other"),
                };
                let y_scale = double("y_scale")?;
                ensure!(
                    shift.is_none() || y_scale == 0.0,
                    "a shifted noise that is not flat; how the game shifts its y is not known here"
                );
                Ok(Self::Noise {
                    noise: json.member("noise")?.string()?.to_owned(),
                    xz_scale: double("xz_scale")?,
                    y_scale,
                    shift,
                })
            }
            "shift_a" => {
                only(&["noise"])?;
                Ok(Self::ShiftA(json.member("noise")?.string()?.to_owned()))
            }
            "shift_b" => {
                only(&["noise"])?;
                Ok(Self::ShiftB(json.member("noise")?.string()?.to_owned()))
            }
            "clamp" => {
                only(&["input", "min", "max"])?;
                let (min, max) = (float("min")?, float("max")?);
                ensure!(min <= max, "a clamp from {min} to {max}");
                Ok(Self::Clamp {
                    input: child("input")?,
                    min,
                    max,
                })
            }
            "lerp" => {
                only(&["alpha", "first", "second"])?;
                Ok(Self::Lerp {
                    alpha: child("alpha")?,
                    first: child("first")?,
                    second: child("second")?,
                })
            }
            "range_choice" => {
                only(&[
                    "input",
                    "min_inclusive",
                    "max_exclusive",
                    "when_in_range",
                    "when_out_of_range",
                ])?;
                Ok(Self::RangeChoice {
                    input: child("input")?,
                    min_inclusive: float("min_inclusive")?,
                    max_exclusive: float("max_exclusive")?,
                    when_in_range: child("when_in_range")?,
                    when_out_of_range: child("when_out_of_range")?,
                })
            }
            "interval_select" => {
                only(&["input", "thresholds", "functions"])?;
                let thresholds = json
                    .member("thresholds")?
                    .array()?
                    .iter()
                    .map(Json::float)
                    .collect::<Result<Vec<f32>>>()?;
                let functions = json
                    .member("functions")?
                    .array()?
                    .iter()
                    .map(Self::parse)
                    .collect::<Result<Vec<Density>>>()?;
                ensure!(
                    functions.len() == thresholds.len() + 1,
                    "{} thresholds and {} functions",
                    thresholds.len(),
                    functions.len()
                );
                ensure!(
                    thresholds.windows(2).all(|pair| pair[0] <= pair[1]),
                    "the thresholds do not ascend"
                );
                Ok(Self::IntervalSelect {
                    input: child("input")?,
                    thresholds,
                    functions,
                })
            }
            "spline" => {
                only(&["spline"])?;
                match Spline::parse_value(json.member("spline")?)? {
                    SplineValue::Constant(value) => Ok(Self::Constant(value)),
                    SplineValue::Spline(spline) => Ok(Self::Spline(spline)),
                }
            }
            "interpolated" => {
                only(&["input", "cell_size_xz", "cell_size_y"])?;
                let (cell_size_xz, cell_size_y) =
                    (integer("cell_size_xz")?, integer("cell_size_y")?);
                ensure!(
                    (1..=64).contains(&cell_size_xz) && (1..=64).contains(&cell_size_y),
                    "cells of {cell_size_xz} by {cell_size_y}"
                );
                Ok(Self::Interpolated {
                    input: child("input")?,
                    cell_size_xz,
                    cell_size_y,
                })
            }
            "cache" => {
                only(&["input"])?;
                Ok(Self::Cache(child("input")?))
            }
            "blend_alpha" => only(&[]).map(|()| Self::BlendAlpha),
            "blend_offset" => only(&[]).map(|()| Self::BlendOffset),
            "blend_density" => {
                only(&["input"])?;
                Ok(Self::Blend(child("input")?))
            }
            "beardifier" => only(&[]).map(|()| Self::Beardifier),
            "end_outer_islands" => only(&[]).map(|()| Self::EndOuterIslands),
            "slice" => {
                only(&["axis", "coordinate", "input"])?;
                Ok(Self::Slice {
                    axis: axis("axis")?,
                    coordinate: integer("coordinate")?,
                    input: child("input")?,
                })
            }
            "distance_to_point" => {
                only(&["metric", "point"])?;
                let point = json
                    .member("point")?
                    .array()?
                    .iter()
                    .map(Json::integer)
                    .collect::<Result<Vec<i32>>>()?;
                let point: [i32; 3] = point
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("a point does not have three coordinates"))?;
                let metric = match json.member("metric")?.string()? {
                    "euclidean" => Metric::Euclidean,
                    other => bail!("the metric {other:?} is not known to the emitter"),
                };
                Ok(Self::DistanceToPoint { point, metric })
            }
            "old_blended_noise" => {
                only(&[
                    "xz_scale",
                    "y_scale",
                    "xz_factor",
                    "y_factor",
                    "smear_scale_multiplier",
                ])?;
                Ok(Self::OldBlendedNoise {
                    xz_scale: double("xz_scale")?,
                    y_scale: double("y_scale")?,
                    xz_factor: double("xz_factor")?,
                    y_factor: double("y_factor")?,
                    smear_scale_multiplier: double("smear_scale_multiplier")?,
                })
            }
            "find_top_surface" => {
                only(&["density", "upper_bound", "lower_bound", "cell_height"])?;
                let cell_height = integer("cell_height")?;
                ensure!(cell_height > 0, "a cell height of {cell_height}");
                Ok(Self::FindTopSurface {
                    density: child("density")?,
                    upper_bound: child("upper_bound")?,
                    lower_bound: integer("lower_bound")?,
                    cell_height,
                })
            }
            other => bail!("the type {other:?} is not known to the emitter"),
        }
    }

    /// The functions directly below this one, splines' coordinates and nested splines
    /// included.
    pub fn children(&self) -> Vec<&Density> {
        match self {
            Self::Constant(_)
            | Self::Reference(_)
            | Self::Gradient { .. }
            | Self::ShiftA(_)
            | Self::ShiftB(_)
            | Self::BlendAlpha
            | Self::BlendOffset
            | Self::Beardifier
            | Self::EndOuterIslands
            | Self::DistanceToPoint { .. }
            | Self::OldBlendedNoise { .. } => Vec::new(),
            Self::Noise { shift, .. } => shift.iter().flat_map(|pair| pair.iter()).collect(),
            Self::Binary(_, left, right) => vec![left, right],
            Self::Unary(_, input)
            | Self::Clamp { input, .. }
            | Self::Interpolated { input, .. }
            | Self::Cache(input)
            | Self::Blend(input)
            | Self::Slice { input, .. } => vec![input],
            Self::Lerp {
                alpha,
                first,
                second,
            } => vec![alpha, first, second],
            Self::RangeChoice {
                input,
                when_in_range,
                when_out_of_range,
                ..
            } => vec![input, when_in_range, when_out_of_range],
            Self::IntervalSelect {
                input, functions, ..
            } => std::iter::once(&**input).chain(functions).collect(),
            Self::Spline(spline) => spline.coordinates(),
            Self::FindTopSurface {
                density,
                upper_bound,
                ..
            } => vec![density, upper_bound],
        }
    }

    /// The names of the noises this function itself asks, not those below it.
    pub fn noise(&self) -> Option<&str> {
        match self {
            Self::Noise { noise, .. } | Self::ShiftA(noise) | Self::ShiftB(noise) => Some(noise),
            _ => None,
        }
    }
}

impl Spline {
    fn parse_value(json: &Json) -> Result<SplineValue> {
        if let Json::Number(_) = json {
            return Ok(SplineValue::Constant(json.float()?));
        }
        for (key, _) in json.members()? {
            ensure!(
                key == "coordinate" || key == "points",
                "the member {key:?} of a spline is not known to the emitter"
            );
        }
        let coordinate =
            Density::parse(json.member("coordinate")?).context("in a spline's coordinate")?;
        let mut points = Vec::new();
        for point in json.member("points")?.array()? {
            for (key, _) in point.members()? {
                ensure!(
                    ["location", "value", "derivative"].contains(&key.as_str()),
                    "the member {key:?} of a spline's point is not known to the emitter"
                );
            }
            points.push(SplinePoint {
                location: point.member("location")?.float()?,
                derivative: point.member("derivative")?.float()?,
                value: Self::parse_value(point.member("value")?)?,
            });
        }
        ensure!(!points.is_empty(), "a spline without points");
        ensure!(
            points
                .windows(2)
                .all(|pair| pair[0].location < pair[1].location),
            "the locations of a spline do not ascend"
        );
        Ok(SplineValue::Spline(Spline {
            coordinate: Box::new(coordinate),
            points,
        }))
    }

    /// The coordinates of this spline and of those nested in it.
    fn coordinates(&self) -> Vec<&Density> {
        let mut found = vec![&*self.coordinate];
        for point in &self.points {
            if let SplineValue::Spline(nested) = &point.value {
                found.extend(nested.coordinates());
            }
        }
        found
    }
}

/// The registry of density functions, by name.
pub type Registry = BTreeMap<String, Density>;

/// Reads every function of the registry. `files` are the registry's entries by name.
pub fn registry(files: &BTreeMap<String, Json>) -> Result<Registry> {
    files
        .iter()
        .map(|(name, json)| {
            let density = Density::parse(json)
                .with_context(|| format!("reading the density function {name}"))?;
            Ok((name.clone(), density))
        })
        .collect()
}

/// The named functions that `roots` reach, through references, each once and sorted.
/// A reference that names nothing fails, as does one that leads back to itself.
pub fn reached(registry: &Registry, roots: &[&Density]) -> Result<BTreeSet<String>> {
    fn visit(
        density: &Density,
        registry: &Registry,
        path: &mut Vec<String>,
        found: &mut BTreeSet<String>,
    ) -> Result<()> {
        if let Density::Reference(name) = density {
            ensure!(
                !path.contains(name),
                "the density function {name} refers to itself through {}",
                path.join(", ")
            );
            let target = registry.get(name).with_context(|| match path.last() {
                Some(from) => format!("{from} refers to {name}, which is no density function"),
                None => format!("{name} is referred to and is no density function"),
            })?;
            if found.insert(name.clone()) {
                path.push(name.clone());
                visit(target, registry, path, found)?;
                path.pop();
            }
            return Ok(());
        }
        for child in density.children() {
            visit(child, registry, path, found)?;
        }
        Ok(())
    }

    let mut found = BTreeSet::new();
    for root in roots {
        visit(root, registry, &mut Vec::new(), &mut found)?;
    }
    Ok(found)
}

/// Whether the value can change with y. It errs towards yes: a function wrongly taken
/// to depend on y is only remembered less often.
pub fn depends_on_y(density: &Density, registry: &Registry) -> bool {
    match density {
        Density::Constant(_)
        | Density::ShiftA(_)
        | Density::ShiftB(_)
        | Density::BlendAlpha
        | Density::BlendOffset
        | Density::EndOuterIslands => false,
        Density::Beardifier | Density::DistanceToPoint { .. } | Density::OldBlendedNoise { .. } => {
            true
        }
        Density::Reference(name) => registry
            .get(name)
            .is_none_or(|target| depends_on_y(target, registry)),
        Density::Gradient { axis, .. } => *axis == Axis::Y,
        Density::Noise { y_scale, shift, .. } => {
            *y_scale != 0.0
                || shift
                    .iter()
                    .flat_map(|pair| pair.iter())
                    .any(|shift| depends_on_y(shift, registry))
        }
        Density::Slice { axis, input, .. } => *axis != Axis::Y && depends_on_y(input, registry),
        Density::FindTopSurface { upper_bound, .. } => depends_on_y(upper_bound, registry),
        other => other
            .children()
            .into_iter()
            .any(|child| depends_on_y(child, registry)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Density> {
        Density::parse(&Json::parse(text).unwrap())
    }

    fn registry_of(entries: &[(&str, &str)]) -> Registry {
        entries
            .iter()
            .map(|(name, text)| ((*name).to_owned(), parse(text).unwrap()))
            .collect()
    }

    #[test]
    fn a_number_a_name_and_an_object_are_the_three_ways_to_write_a_function() {
        assert_eq!(parse("-0.225").unwrap(), Density::Constant(-0.225));
        assert_eq!(
            parse("\"minecraft:overworld/depth\"").unwrap(),
            Density::Reference("minecraft:overworld/depth".to_owned())
        );
        let parsed = parse(
            r#"{"type": "minecraft:min", "left": {"type": "minecraft:abs", "input": 1.5},
                "right": {"type": "minecraft:noise", "noise": "minecraft:a", "xz_scale": 0.25,
                          "y_scale": 0.0, "shift_x": "minecraft:shift_x", "shift_z": 2.0}}"#,
        )
        .unwrap();
        let Density::Binary(BinaryOp::Min, left, right) = parsed else {
            panic!("not a min");
        };
        assert_eq!(
            *left,
            Density::Unary(UnaryOp::Abs, Box::new(Density::Constant(1.5)))
        );
        assert_eq!(right.noise(), Some("minecraft:a"));
        assert_eq!(right.children().len(), 2);
    }

    #[test]
    fn what_the_emitter_does_not_know_fails_and_says_where() {
        let unknown_type = parse(r#"{"type": "minecraft:add", "left": 1, "right": {"type": "minecraft:log", "input": 2}}"#)
            .unwrap_err();
        let text = format!("{unknown_type:#}");
        assert!(text.contains("\"log\" is not known"), "{text}");
        assert!(text.contains("in its right"), "{text}");

        for text in [
            // A member that would change what the game computes.
            r#"{"type": "minecraft:gradient", "axis": "y", "tiling": "repeat", "from_coordinate": 0, "to_coordinate": 1, "from_value": 0, "to_value": 1}"#,
            r#"{"type": "minecraft:noise", "noise": "minecraft:a", "xz_scale": 1, "y_scale": 1, "shift_y": 0}"#,
            // A shift on a noise that is not flat.
            r#"{"type": "minecraft:noise", "noise": "minecraft:a", "xz_scale": 1, "y_scale": 1, "shift_x": 0, "shift_z": 0}"#,
            r#"{"type": "minecraft:distance_to_point", "metric": "manhattan", "point": [0, 0, 0]}"#,
            r#"{"type": "minecraft:interval_select", "input": 0, "thresholds": [0.5], "functions": [1]}"#,
            r#"{"type": "other:add", "left": 1, "right": 2}"#,
            r#"{"type": "minecraft:spline", "spline": {"coordinate": 0, "points": []}}"#,
            "[1]",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_float_that_a_double_would_read_differently_fails_inside_a_function() {
        let error = parse(
            r#"{"type": "minecraft:clamp", "input": 0, "min": 0, "max": 1.00000005960464477550}"#,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("through a double"));
    }

    #[test]
    fn a_spline_nests_and_gives_its_coordinates_as_children() {
        let parsed = parse(
            r#"{"type": "minecraft:spline", "spline": {"coordinate": "minecraft:c", "points": [
                {"location": -1.0, "derivative": 0.0, "value": 0.5},
                {"location": 1.0, "derivative": 2.0, "value": {"coordinate": "minecraft:e", "points": [
                    {"location": 0.0, "derivative": 0.0, "value": 1.0}]}}]}}"#,
        )
        .unwrap();
        let Density::Spline(spline) = &parsed else {
            panic!("not a spline");
        };
        assert_eq!(spline.points.len(), 2);
        assert_eq!(spline.points[1].derivative, 2.0);
        let names: Vec<&Density> = parsed.children();
        assert_eq!(
            names,
            [
                &Density::Reference("minecraft:c".to_owned()),
                &Density::Reference("minecraft:e".to_owned())
            ]
        );
        assert_eq!(
            parse(r#"{"type": "minecraft:spline", "spline": 0.25}"#).unwrap(),
            Density::Constant(0.25)
        );
    }

    #[test]
    fn what_is_reached_is_found_once_and_a_name_that_is_nothing_fails() {
        let registry = registry_of(&[
            (
                "minecraft:a",
                r#"{"type": "minecraft:add", "left": "minecraft:b", "right": "minecraft:b"}"#,
            ),
            (
                "minecraft:b",
                r#"{"type": "minecraft:cache", "input": "minecraft:c"}"#,
            ),
            ("minecraft:c", "1.0"),
            ("minecraft:unused", "2.0"),
            (
                "minecraft:broken",
                r#"{"type": "minecraft:abs", "input": "minecraft:nothing"}"#,
            ),
            (
                "minecraft:loop",
                r#"{"type": "minecraft:abs", "input": "minecraft:loop"}"#,
            ),
        ]);
        let root = Density::Reference("minecraft:a".to_owned());
        let found: Vec<String> = reached(&registry, &[&root]).unwrap().into_iter().collect();
        assert_eq!(found, ["minecraft:a", "minecraft:b", "minecraft:c"]);

        let broken = Density::Reference("minecraft:broken".to_owned());
        let error = reached(&registry, &[&broken]).unwrap_err();
        assert!(format!("{error}").contains("minecraft:broken refers to minecraft:nothing"));
        let looping = Density::Reference("minecraft:loop".to_owned());
        assert!(reached(&registry, &[&looping]).is_err());
    }

    #[test]
    fn dependence_on_y_follows_references_slices_and_flat_noises() {
        let registry = registry_of(&[
            (
                "minecraft:flat",
                r#"{"type": "minecraft:noise", "noise": "minecraft:n", "xz_scale": 1, "y_scale": 0}"#,
            ),
            (
                "minecraft:deep",
                r#"{"type": "minecraft:noise", "noise": "minecraft:n", "xz_scale": 1, "y_scale": 1}"#,
            ),
        ]);
        let y = |text: &str| depends_on_y(&parse(text).unwrap(), &registry);
        assert!(!y("\"minecraft:flat\""));
        assert!(y("\"minecraft:deep\""));
        assert!(y("\"minecraft:not_there\""), "it errs towards yes");
        assert!(!y(
            r#"{"type": "minecraft:slice", "axis": "y", "coordinate": 0, "input": "minecraft:deep"}"#
        ));
        assert!(y(
            r#"{"type": "minecraft:slice", "axis": "x", "coordinate": 0, "input": "minecraft:deep"}"#
        ));
        assert!(y(
            r#"{"type": "minecraft:gradient", "axis": "y", "from_coordinate": 0, "to_coordinate": 1, "from_value": 0, "to_value": 1}"#
        ));
        assert!(!y(
            r#"{"type": "minecraft:gradient", "axis": "x", "from_coordinate": 0, "to_coordinate": 1, "from_value": 0, "to_value": 1}"#
        ));
        assert!(!y(
            r#"{"type": "minecraft:find_top_surface", "density": "minecraft:deep", "upper_bound": "minecraft:flat", "lower_bound": 0, "cell_height": 8}"#
        ));
        assert!(!y(
            r#"{"type": "minecraft:mul", "left": "minecraft:flat", "right": {"type": "minecraft:end_outer_islands"}}"#
        ));
    }
}
