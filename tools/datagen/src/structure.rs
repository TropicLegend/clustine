// Written for Clustine from the game's data (`worldgen/structure/**`,
// `worldgen/structure_set/**`, `worldgen/template_pool/**`, `structure/**/*.nbt`, the
// biome tags) and the codecs of the pinned jar's classes that read it (`Structure`,
// `JigsawStructure`, `StructureSet`, `RandomSpreadStructurePlacement`,
// `ConcentricRingsStructurePlacement`, `StructureTemplatePool`, the pool elements,
// `StructureTemplate`). The records it writes follow SteelMC's data model; see
// `clustine_worldgen_data::structure`.

//! Emits the structures, the structure sets and the template pools of
//! `clustine-worldgen-data` as statics, and of every structure template its size and
//! its jigsaw blocks as a packed table.
//!
//! **Nothing of a template's blocks is written**: not its palette, not its blocks,
//! not its block entities, and not the state a jigsaw block turns into (ADR-0019,
//! section 2). A member or a kind the emitter does not know fails the run, so that
//! nothing the data says is passed over.

use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::{Context, Result, bail, ensure};

use crate::json::Json;
use crate::model::RegistryTags;
use crate::nbt::{self, Tag};
use crate::packed::{self, Section};
use crate::router::{LINE_WIDTH, check_layout, float, wrapped};
use crate::worldgen::Worldgen;

/// The version of the layout of `templates.bin`.
pub const TEMPLATES_LAYOUT: u16 = 1;

/// The largest side of a template and the highest count of jigsaws in one that the
/// table can hold.
const BYTE: i64 = 255;

/// The orientations of a jigsaw block in the order of
/// `clustine_worldgen_data::structure::Orientation`.
const ORIENTATIONS: [&str; 12] = [
    "down_east",
    "down_north",
    "down_south",
    "down_west",
    "up_east",
    "up_north",
    "up_south",
    "up_west",
    "west_up",
    "east_up",
    "north_up",
    "south_up",
];

/// A value of Rust as the emitter lays it out: on one line where it fits, and
/// otherwise a member a line.
#[derive(Debug)]
enum Rust {
    Atom(String),
    /// `Name { field: value, .. }`.
    Record(String, Vec<(&'static str, Rust)>),
    /// `Name(value, ..)`, or a tuple where the name is empty.
    Call(String, Vec<Rust>),
    /// `&[value, ..]`.
    Slice(Vec<Rust>),
}

impl Rust {
    fn atom(text: impl Into<String>) -> Self {
        Self::Atom(text.into())
    }

    fn text(text: &str) -> Self {
        Self::Atom(format!("{text:?}"))
    }

    fn flat(&self) -> String {
        match self {
            Self::Atom(text) => text.clone(),
            Self::Record(name, fields) => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.flat()))
                    .collect();
                format!("{name} {{ {} }}", fields.join(", "))
            }
            Self::Call(name, values) => {
                let values: Vec<String> = values.iter().map(Rust::flat).collect();
                format!("{name}({})", values.join(", "))
            }
            Self::Slice(values) => {
                let values: Vec<String> = values.iter().map(Rust::flat).collect();
                format!("&[{}]", values.join(", "))
            }
        }
    }

    /// Writes the value after `before` (a field's name, or nothing) at `indent`,
    /// with `after` behind it.
    fn write(&self, out: &mut String, indent: usize, before: &str, after: &str) {
        let pad = " ".repeat(indent);
        let flat = self.flat();
        // A pair that holds a name stays on its line although the name makes it
        // longer than the width, as a literal may: an element of a pool with its
        // weight is such a pair, and there are some two thousand.
        let pair_with_text = matches!(self, Self::Call(name, _) if name.is_empty())
            && flat.contains('"')
            && pad.len() + flat.len() <= 2 * LINE_WIDTH;
        if (pad.len() + before.len() + flat.len() + after.len() <= LINE_WIDTH)
            || matches!(self, Self::Atom(_))
            || pair_with_text
        {
            let _ = writeln!(out, "{pad}{before}{flat}{after}");
            return;
        }
        match self {
            Self::Atom(_) => {}
            Self::Record(name, fields) => {
                let _ = writeln!(out, "{pad}{before}{name} {{");
                for (field, value) in fields {
                    value.write(out, indent + 4, &format!("{field}: "), ",");
                }
                let _ = writeln!(out, "{pad}}}{after}");
            }
            Self::Call(name, values) => {
                let _ = writeln!(out, "{pad}{before}{name}(");
                for value in values {
                    value.write(out, indent + 4, "", ",");
                }
                let _ = writeln!(out, "{pad}){after}");
            }
            Self::Slice(values) => {
                let _ = writeln!(out, "{pad}{before}&[");
                if values.iter().all(|value| matches!(value, Self::Atom(_))) {
                    // Numbers and names stand as many on a line as fit.
                    let items: Vec<String> = values.iter().map(Rust::flat).collect();
                    out.push_str(&wrapped(&items, &" ".repeat(indent + 4)));
                } else {
                    for value in values {
                        value.write(out, indent + 4, "", ",");
                    }
                }
                let _ = writeln!(out, "{pad}]{after}");
            }
        }
    }
}

/// The members `json` may have.
fn only(json: &Json, what: &str, known: &[&str]) -> Result<()> {
    for (key, _) in json.members()? {
        ensure!(
            known.contains(&key.as_str()),
            "the member {key:?} of {what} is not known to the emitter"
        );
    }
    Ok(())
}

fn boolean_or(json: &Json, name: &str, default: bool) -> Result<bool> {
    match json.get(name) {
        Some(value) => value.boolean(),
        None => Ok(default),
    }
}

fn integer_or(json: &Json, name: &str, default: i32) -> Result<i32> {
    match json.get(name) {
        Some(value) => value.integer(),
        None => Ok(default),
    }
}

/// The place of `name` in a registry's sorted names.
fn place(names: &[&String], name: &str, what: &str) -> Result<u16> {
    let index = names
        .iter()
        .position(|known| *known == name)
        .with_context(|| format!("{name} is referred to and is no {what}"))?;
    u16::try_from(index).with_context(|| format!("more than 65,536 of {what}"))
}

/// A list of biomes as the data names it (a tag, a biome, or a list of biomes), as
/// their numbers, ascending.
fn biomes(json: &Json, names: &[String], tags: &RegistryTags) -> Result<Rust> {
    let number = |name: &str| -> Result<usize> {
        names
            .iter()
            .position(|known| known == name)
            .with_context(|| format!("{name} is no biome"))
    };
    let mut numbers = Vec::new();
    match json {
        Json::Array(items) => {
            for item in items {
                numbers.push(number(item.string()?)?);
            }
        }
        _ => {
            let name = json.string()?;
            if let Some(tag) = name.strip_prefix('#') {
                let found = tags
                    .tags
                    .iter()
                    .find(|known| known.name == tag)
                    .with_context(|| format!("the generated data has no biome tag {tag}"))?;
                numbers.extend(&found.entries);
            } else {
                numbers.push(number(name)?);
            }
        }
    }
    numbers.sort_unstable();
    numbers.dedup();
    let mut items = Vec::new();
    for number in numbers {
        ensure!(number < 256, "a biome's number does not fit a byte");
        items.push(Rust::atom(number.to_string()));
    }
    Ok(Rust::Slice(items))
}

/// A height against the terrain's.
fn anchor(json: &Json) -> Result<Rust> {
    let [(kind, value)] = json.members()? else {
        bail!("a height has one member");
    };
    let value = value.integer()?;
    Ok(Rust::atom(match kind.as_str() {
        "absolute" => format!("Anchor::Absolute({value})"),
        "above_bottom" => format!("Anchor::AboveBottom({value})"),
        "below_top" => format!("Anchor::BelowTop({value})"),
        other => bail!("a height that is {other:?} is not known to the emitter"),
    }))
}

/// A provider of heights: an anchor by itself is a constant.
fn height_provider(json: &Json) -> Result<Rust> {
    let Some(kind) = json.get("type") else {
        return Ok(Rust::Call(
            "HeightProvider::Constant".to_owned(),
            vec![anchor(json)?],
        ));
    };
    match kind.string()? {
        "minecraft:uniform" => {
            only(
                json,
                "a uniform height",
                &["type", "min_inclusive", "max_inclusive"],
            )?;
            Ok(Rust::Record(
                "HeightProvider::Uniform".to_owned(),
                vec![
                    ("min_inclusive", anchor(json.member("min_inclusive")?)?),
                    ("max_inclusive", anchor(json.member("max_inclusive")?)?),
                ],
            ))
        }
        "minecraft:constant" => {
            only(json, "a constant height", &["type", "value"])?;
            Ok(Rust::Call(
                "HeightProvider::Constant".to_owned(),
                vec![anchor(json.member("value")?)?],
            ))
        }
        other => bail!("a provider of heights that is {other:?} is not known to the emitter"),
    }
}

fn weighted_list(json: &Json, what: &str) -> Result<Vec<(String, i32)>> {
    let mut entries = Vec::new();
    for entry in json.array()? {
        only(entry, what, &["data", "weight"])?;
        entries.push((
            entry.member("data")?.string()?.to_owned(),
            entry.member("weight")?.integer()?,
        ));
    }
    Ok(entries)
}

/// A renaming of pools.
fn pool_alias(json: &Json) -> Result<Rust> {
    match json.member("type")?.string()? {
        "minecraft:direct" => {
            only(json, "a direct alias", &["type", "alias", "target"])?;
            Ok(Rust::Record(
                "PoolAlias::Direct".to_owned(),
                vec![
                    ("alias", Rust::text(json.member("alias")?.string()?)),
                    ("target", Rust::text(json.member("target")?.string()?)),
                ],
            ))
        }
        "minecraft:random" => {
            only(json, "a random alias", &["type", "alias", "targets"])?;
            let targets = weighted_list(json.member("targets")?, "a target of an alias")?
                .into_iter()
                .map(|(target, weight)| {
                    Rust::Call(
                        String::new(),
                        vec![Rust::text(&target), Rust::atom(weight.to_string())],
                    )
                })
                .collect();
            Ok(Rust::Record(
                "PoolAlias::Random".to_owned(),
                vec![
                    ("alias", Rust::text(json.member("alias")?.string()?)),
                    ("targets", Rust::Slice(targets)),
                ],
            ))
        }
        "minecraft:random_group" => {
            only(json, "a random group of aliases", &["type", "groups"])?;
            let mut groups = Vec::new();
            for group in json.member("groups")?.array()? {
                only(group, "a group of aliases", &["data", "weight"])?;
                let mut members = Vec::new();
                for member in group.member("data")?.array()? {
                    ensure!(
                        member.member("type")?.string()? == "minecraft:direct",
                        "a group of aliases holds direct aliases"
                    );
                    members.push(pool_alias(member)?);
                }
                groups.push(Rust::Call(
                    String::new(),
                    vec![
                        Rust::Slice(members),
                        Rust::atom(group.member("weight")?.integer()?.to_string()),
                    ],
                ));
            }
            Ok(Rust::Record(
                "PoolAlias::RandomGroup".to_owned(),
                vec![("groups", Rust::Slice(groups))],
            ))
        }
        other => bail!("an alias that is {other:?} is not known to the emitter"),
    }
}

/// What every structure has, beside what its kind has.
const COMMON: [&str; 5] = [
    "type",
    "biomes",
    "step",
    "spawn_overrides",
    "terrain_adaptation",
];

fn with_common(known: &[&'static str]) -> Vec<&'static str> {
    COMMON.iter().chain(known).copied().collect()
}

/// The kind of a structure with its own members.
fn kind(json: &Json, pools: &[&String]) -> Result<Rust> {
    let kind = json.member("type")?.string()?;
    let bare = |name: &str| -> Result<Rust> {
        only(json, kind, &COMMON)?;
        Ok(Rust::atom(format!("Kind::{name}")))
    };
    match kind {
        "minecraft:jigsaw" => {
            only(
                json,
                "a jigsaw structure",
                &with_common(&[
                    "start_pool",
                    "size",
                    "start_height",
                    "use_expansion_hack",
                    "project_start_to_heightmap",
                    "max_distance_from_center",
                    "start_jigsaw_name",
                    "dimension_padding",
                    "pool_aliases",
                    "liquid_settings",
                ]),
            )?;
            let heightmap = match json.get("project_start_to_heightmap") {
                None => "None",
                Some(name) => match name.string()? {
                    "WORLD_SURFACE_WG" => "Some(Heightmap::WorldSurfaceWg)",
                    "OCEAN_FLOOR_WG" => "Some(Heightmap::OceanFloorWg)",
                    other => bail!("the heightmap {other:?} is not known to the emitter"),
                },
            };
            let distance = json.member("max_distance_from_center")?;
            let (horizontal, vertical) = match distance {
                Json::Object(_) => {
                    only(
                        distance,
                        "a distance from the centre",
                        &["horizontal", "vertical"],
                    )?;
                    let horizontal = distance.member("horizontal")?.integer()?;
                    // The codec's default.
                    (horizontal, integer_or(distance, "vertical", 4064)?)
                }
                _ => (distance.integer()?, distance.integer()?),
            };
            let (bottom, top) = match json.get("dimension_padding") {
                None => (0, 0),
                Some(padding @ Json::Object(_)) => {
                    only(padding, "a padding", &["bottom", "top"])?;
                    (
                        integer_or(padding, "bottom", 0)?,
                        integer_or(padding, "top", 0)?,
                    )
                }
                Some(padding) => (padding.integer()?, padding.integer()?),
            };
            let mut aliases = Vec::new();
            if let Some(list) = json.get("pool_aliases") {
                for alias in list.array()? {
                    aliases.push(pool_alias(alias)?);
                }
            }
            let liquid = match json.get("liquid_settings").map(Json::string).transpose()? {
                None | Some("apply_waterlogging") => "LiquidSettings::ApplyWaterlogging",
                Some("ignore_waterlogging") => "LiquidSettings::IgnoreWaterlogging",
                Some(other) => bail!("the liquid settings {other:?} are not known to the emitter"),
            };
            let name = match json.get("start_jigsaw_name") {
                None => Rust::atom("None"),
                Some(name) => Rust::Call("Some".to_owned(), vec![Rust::text(name.string()?)]),
            };
            let record = Rust::Record(
                "Jigsaw".to_owned(),
                vec![
                    (
                        "start_pool",
                        Rust::atom(
                            place(pools, json.member("start_pool")?.string()?, "template pool")?
                                .to_string(),
                        ),
                    ),
                    (
                        "max_depth",
                        Rust::atom(json.member("size")?.integer()?.to_string()),
                    ),
                    (
                        "use_expansion_hack",
                        Rust::atom(json.member("use_expansion_hack")?.boolean()?.to_string()),
                    ),
                    ("project_start_to_heightmap", Rust::atom(heightmap)),
                    (
                        "start_height",
                        height_provider(json.member("start_height")?)?,
                    ),
                    (
                        "max_distance_horizontal",
                        Rust::atom(horizontal.to_string()),
                    ),
                    ("max_distance_vertical", Rust::atom(vertical.to_string())),
                    ("start_jigsaw_name", name),
                    ("padding_bottom", Rust::atom(bottom.to_string())),
                    ("padding_top", Rust::atom(top.to_string())),
                    ("pool_aliases", Rust::Slice(aliases)),
                    ("liquid_settings", Rust::atom(liquid)),
                ],
            );
            Ok(Rust::Call("Kind::Jigsaw".to_owned(), vec![record]))
        }
        "minecraft:buried_treasure" => bare("BuriedTreasure"),
        "minecraft:desert_pyramid" => bare("DesertPyramid"),
        "minecraft:end_city" => bare("EndCity"),
        "minecraft:fortress" => bare("Fortress"),
        "minecraft:igloo" => bare("Igloo"),
        "minecraft:jungle_temple" => bare("JungleTemple"),
        "minecraft:ocean_monument" => bare("OceanMonument"),
        "minecraft:stronghold" => bare("Stronghold"),
        "minecraft:swamp_hut" => bare("SwampHut"),
        "minecraft:woodland_mansion" => bare("WoodlandMansion"),
        "minecraft:mineshaft" => {
            only(json, "a mineshaft", &with_common(&["mineshaft_type"]))?;
            let mesa = match json.member("mineshaft_type")?.string()? {
                "normal" => false,
                "mesa" => true,
                other => bail!("a mineshaft that is {other:?} is not known to the emitter"),
            };
            Ok(Rust::atom(format!("Kind::Mineshaft {{ mesa: {mesa} }}")))
        }
        "minecraft:nether_fossil" => {
            only(json, "a nether fossil", &with_common(&["height"]))?;
            Ok(Rust::Record(
                "Kind::NetherFossil".to_owned(),
                vec![("height", height_provider(json.member("height")?)?)],
            ))
        }
        "minecraft:ocean_ruin" => {
            only(
                json,
                "an ocean ruin",
                &with_common(&["biome_temp", "large_probability", "cluster_probability"]),
            )?;
            let warm = match json.member("biome_temp")?.string()? {
                "warm" => true,
                "cold" => false,
                other => bail!("an ocean ruin that is {other:?} is not known to the emitter"),
            };
            Ok(Rust::Record(
                "Kind::OceanRuin".to_owned(),
                vec![
                    ("warm", Rust::atom(warm.to_string())),
                    (
                        "large_probability",
                        Rust::atom(float(json.member("large_probability")?.float()?)?),
                    ),
                    (
                        "cluster_probability",
                        Rust::atom(float(json.member("cluster_probability")?.float()?)?),
                    ),
                ],
            ))
        }
        "minecraft:ruined_portal" => {
            only(json, "a ruined portal", &with_common(&["setups"]))?;
            let mut setups = Vec::new();
            for setup in json.member("setups")?.array()? {
                only(
                    setup,
                    "a setup of a ruined portal",
                    &[
                        "placement",
                        "air_pocket_probability",
                        "mossiness",
                        "overgrown",
                        "vines",
                        "can_be_cold",
                        "replace_with_blackstone",
                        "weight",
                    ],
                )?;
                let placement = match setup.member("placement")?.string()? {
                    "on_land_surface" => "OnLandSurface",
                    "partly_buried" => "PartlyBuried",
                    "on_ocean_floor" => "OnOceanFloor",
                    "in_mountain" => "InMountain",
                    "underground" => "Underground",
                    "in_nether" => "InNether",
                    other => bail!("a portal that stands {other:?} is not known to the emitter"),
                };
                let number = |name: &str| -> Result<Rust> {
                    Ok(Rust::atom(float(setup.member(name)?.float()?)?))
                };
                let flag = |name: &str| -> Result<Rust> {
                    Ok(Rust::atom(setup.member(name)?.boolean()?.to_string()))
                };
                setups.push(Rust::Record(
                    "PortalSetup".to_owned(),
                    vec![
                        (
                            "placement",
                            Rust::atom(format!("PortalPlacement::{placement}")),
                        ),
                        ("air_pocket_probability", number("air_pocket_probability")?),
                        ("mossiness", number("mossiness")?),
                        ("overgrown", flag("overgrown")?),
                        ("vines", flag("vines")?),
                        ("can_be_cold", flag("can_be_cold")?),
                        ("replace_with_blackstone", flag("replace_with_blackstone")?),
                        ("weight", number("weight")?),
                    ],
                ));
            }
            Ok(Rust::Record(
                "Kind::RuinedPortal".to_owned(),
                vec![("setups", Rust::Slice(setups))],
            ))
        }
        "minecraft:shipwreck" => {
            only(json, "a shipwreck", &with_common(&["is_beached"]))?;
            let beached = boolean_or(json, "is_beached", false)?;
            Ok(Rust::atom(format!(
                "Kind::Shipwreck {{ beached: {beached} }}"
            )))
        }
        other => bail!("a structure that is {other:?} is not known to the emitter"),
    }
}

/// The placement of a structure set.
fn placement(json: &Json, sets: &[&String], names: &[String], tags: &RegistryTags) -> Result<Rust> {
    let common = [
        "type",
        "salt",
        "frequency",
        "frequency_reduction_method",
        "exclusion_zone",
        "locate_offset",
    ];
    let spread = match json.member("type")?.string()? {
        "minecraft:random_spread" => {
            let mut known = common.to_vec();
            known.extend(["spacing", "separation", "spread_type"]);
            only(json, "a random spread", &known)?;
            let spacing = json.member("spacing")?.integer()?;
            let separation = json.member("separation")?.integer()?;
            ensure!(
                spacing > separation,
                "a spacing has to be above its separation"
            );
            let spread_type = match json.get("spread_type").map(Json::string).transpose()? {
                None | Some("linear") => "Linear",
                Some("triangular") => "Triangular",
                Some(other) => bail!("a spread that is {other:?} is not known to the emitter"),
            };
            Rust::Record(
                "Spread::Random".to_owned(),
                vec![
                    ("spacing", Rust::atom(spacing.to_string())),
                    ("separation", Rust::atom(separation.to_string())),
                    (
                        "spread_type",
                        Rust::atom(format!("SpreadType::{spread_type}")),
                    ),
                ],
            )
        }
        "minecraft:concentric_rings" => {
            let mut known = common.to_vec();
            known.extend(["distance", "spread", "count", "preferred_biomes"]);
            only(json, "concentric rings", &known)?;
            Rust::Record(
                "Spread::ConcentricRings".to_owned(),
                vec![
                    (
                        "distance",
                        Rust::atom(json.member("distance")?.integer()?.to_string()),
                    ),
                    (
                        "spread",
                        Rust::atom(json.member("spread")?.integer()?.to_string()),
                    ),
                    (
                        "count",
                        Rust::atom(json.member("count")?.integer()?.to_string()),
                    ),
                    (
                        "preferred_biomes",
                        biomes(json.member("preferred_biomes")?, names, tags)?,
                    ),
                ],
            )
        }
        other => bail!("a placement that is {other:?} is not known to the emitter"),
    };
    let reduction = match json
        .get("frequency_reduction_method")
        .map(Json::string)
        .transpose()?
    {
        None | Some("default") => "Default",
        Some("legacy_type_1") => "LegacyType1",
        Some("legacy_type_2") => "LegacyType2",
        Some("legacy_type_3") => "LegacyType3",
        Some(other) => bail!("a frequency reduction that is {other:?} is not known to the emitter"),
    };
    let exclusion = match json.get("exclusion_zone") {
        None => Rust::atom("None"),
        Some(zone) => {
            only(zone, "an exclusion zone", &["other_set", "chunk_count"])?;
            Rust::Call(
                "Some".to_owned(),
                vec![Rust::Record(
                    "ExclusionZone".to_owned(),
                    vec![
                        (
                            "other_set",
                            Rust::atom(
                                place(sets, zone.member("other_set")?.string()?, "structure set")?
                                    .to_string(),
                            ),
                        ),
                        (
                            "chunk_count",
                            Rust::atom(zone.member("chunk_count")?.integer()?.to_string()),
                        ),
                    ],
                )],
            )
        }
    };
    let offset = match json.get("locate_offset") {
        None => "[0, 0, 0]".to_owned(),
        Some(offset) => {
            let [x, y, z] = offset.array()? else {
                bail!("an offset has three numbers");
            };
            format!("[{}, {}, {}]", x.integer()?, y.integer()?, z.integer()?)
        }
    };
    let frequency = match json.get("frequency") {
        None => 1.0,
        Some(frequency) => frequency.float()?,
    };
    Ok(Rust::Record(
        "Placement".to_owned(),
        vec![
            ("spread", spread),
            (
                "salt",
                Rust::atom(json.member("salt")?.integer()?.to_string()),
            ),
            ("frequency", Rust::atom(float(frequency)?)),
            (
                "frequency_reduction",
                Rust::atom(format!("FrequencyReduction::{reduction}")),
            ),
            ("exclusion_zone", exclusion),
            ("locate_offset", Rust::atom(offset)),
        ],
    ))
}

/// `structures.rs`: the structures and the structure sets.
pub fn structures(
    worldgen: &Worldgen,
    biome_names: &[String],
    tags: &[RegistryTags],
) -> Result<String> {
    let biome_tags = tags
        .iter()
        .find(|tags| tags.registry == "worldgen/biome")
        .context("the generated data has no tags of biomes")?;
    let structures = worldgen.registry("structure")?;
    let sets = worldgen.registry("structure_set")?;
    let pools = worldgen.registry("template_pool")?;
    let structure_names: Vec<&String> = structures.keys().collect();
    let set_names: Vec<&String> = sets.keys().collect();
    let pool_names: Vec<&String> = pools.keys().collect();

    let mut out = String::new();
    for line in [
        "//! The game's structures (`worldgen/structure`) and structure sets",
        "//! (`worldgen/structure_set`), each sorted by name, which is the order of the game's",
        "//! registries when a server has loaded them. A structure names its biomes by their",
        "//! numbers, a set its structures by their place in `STRUCTURES`, and a jigsaw structure",
        "//! its first pool by its place in `crate::template_pools::POOLS`.",
        "",
        "use crate::material::Anchor;",
        "use crate::structure::{",
        "    ExclusionZone, FrequencyReduction, HeightProvider, Heightmap, Jigsaw, Kind,",
        "    LiquidSettings, Placement, PoolAlias, PortalPlacement, PortalSetup, Spread, SpreadType,",
        "    Step, Structure, StructureSet, TerrainAdaptation,",
        "};",
        "",
    ] {
        writeln!(out, "{line}")?;
    }
    writeln!(
        out,
        "pub static STRUCTURES: [Structure; {}] = [",
        structures.len()
    )?;
    for (name, json) in structures {
        let read = || -> Result<Rust> {
            let step = match json.member("step")?.string()? {
                "underground_structures" => "UndergroundStructures",
                "surface_structures" => "SurfaceStructures",
                "underground_decoration" => "UndergroundDecoration",
                other => bail!("the step {other:?} is not known to the emitter"),
            };
            let adaptation = match json
                .get("terrain_adaptation")
                .map(Json::string)
                .transpose()?
            {
                None | Some("none") => "None",
                Some("bury") => "Bury",
                Some("beard_thin") => "BeardThin",
                Some("beard_box") => "BeardBox",
                Some("encapsulate") => "Encapsulate",
                Some(other) => bail!("the adaptation {other:?} is not known to the emitter"),
            };
            Ok(Rust::Record(
                "Structure".to_owned(),
                vec![
                    ("name", Rust::text(name)),
                    (
                        "biomes",
                        biomes(json.member("biomes")?, biome_names, biome_tags)?,
                    ),
                    ("step", Rust::atom(format!("Step::{step}"))),
                    (
                        "terrain_adaptation",
                        Rust::atom(format!("TerrainAdaptation::{adaptation}")),
                    ),
                    ("kind", kind(json, &pool_names)?),
                ],
            ))
        };
        read()
            .with_context(|| format!("reading the structure {name}"))?
            .write(&mut out, 4, "", ",");
    }
    writeln!(out, "];\n")?;

    writeln!(
        out,
        "pub static STRUCTURE_SETS: [StructureSet; {}] = [",
        sets.len()
    )?;
    for (name, json) in sets {
        let read = || -> Result<Rust> {
            only(json, "a structure set", &["structures", "placement"])?;
            let mut members = Vec::new();
            for entry in json.member("structures")?.array()? {
                only(
                    entry,
                    "an entry of a structure set",
                    &["structure", "weight"],
                )?;
                let weight = entry.member("weight")?.integer()?;
                ensure!(weight > 0, "a weight of {weight}");
                members.push(Rust::atom(format!(
                    "({}, {weight})",
                    place(
                        &structure_names,
                        entry.member("structure")?.string()?,
                        "structure"
                    )?
                )));
            }
            Ok(Rust::Record(
                "StructureSet".to_owned(),
                vec![
                    ("name", Rust::text(name)),
                    ("structures", Rust::Slice(members)),
                    (
                        "placement",
                        placement(
                            json.member("placement")?,
                            &set_names,
                            biome_names,
                            biome_tags,
                        )?,
                    ),
                ],
            ))
        };
        read()
            .with_context(|| format!("reading the structure set {name}"))?
            .write(&mut out, 4, "", ",");
    }
    writeln!(out, "];")?;
    check_layout(&out)?;
    Ok(out)
}

/// What is kept of a template.
pub struct Template {
    /// Without the namespace.
    pub name: String,
    pub size: [u8; 3],
    pub exists: bool,
    pub jigsaws: Vec<Jigsaw>,
}

/// A jigsaw block of a template.
pub struct Jigsaw {
    pub position: [u8; 3],
    pub orientation: u8,
    pub rollable: bool,
    pub name: String,
    pub target: String,
    pub pool: String,
    pub selection_priority: i16,
    pub placement_priority: i16,
}

fn byte(value: i64, what: &str) -> Result<u8> {
    ensure!(
        (0..=BYTE).contains(&value),
        "{what} is {value}, which does not fit a byte"
    );
    Ok(value as u8)
}

/// Reads of one template file its size and its jigsaw blocks.
pub fn template(name: &str, bytes: &[u8]) -> Result<Template> {
    let root = nbt::read_gzip(bytes)?;
    let size = root.member("size")?.list()?;
    let [x, y, z] = size else {
        bail!("a size has three numbers");
    };
    let size = [
        byte(x.integer()?, "a side")?,
        byte(y.integer()?, "a side")?,
        byte(z.integer()?, "a side")?,
    ];
    let (palette, several) = match (root.get("palette"), root.get("palettes")) {
        (Some(palette), None) => (palette.list()?, false),
        (None, Some(palettes)) => {
            let palettes = palettes.list()?;
            let first = palettes.first().context("a template without a palette")?;
            (first.list()?, palettes.len() > 1)
        }
        _ => bail!("a template has a palette or several"),
    };
    let mut jigsaws = Vec::new();
    for block in root.member("blocks")?.list()? {
        let state = usize::try_from(block.member("state")?.integer()?)?;
        let state = palette
            .get(state)
            .context("a block of a state the palette lacks")?;
        // A palette entry of 26.3 is the block's name alone, or `{id}`, or
        // `{id, properties}`; older files have `{Name, Properties}`.
        let block_name = match state {
            Tag::Text(name) => name.as_str(),
            _ => match state.get("id") {
                Some(name) => name.text()?,
                None => state.member("Name")?.text()?,
            },
        };
        if block_name != "minecraft:jigsaw" {
            continue;
        }
        ensure!(
            !several,
            "a template with several palettes has a jigsaw block, which the table cannot say"
        );
        let [x, y, z] = block.member("pos")?.list()? else {
            bail!("a position has three numbers");
        };
        let position = [
            byte(x.integer()?, "a coordinate")?,
            byte(y.integer()?, "a coordinate")?,
            byte(z.integer()?, "a coordinate")?,
        ];
        let properties = match state.get("properties") {
            Some(properties) => properties,
            None => state.member("Properties")?,
        };
        let orientation = properties.member("orientation")?.text()?;
        let orientation = ORIENTATIONS
            .iter()
            .position(|known| *known == orientation)
            .with_context(|| format!("the orientation {orientation:?} is not known"))?;
        let tags = block.member("nbt")?;
        let text = |name: &str, default: &str| -> Result<String> {
            Ok(match tags.get(name) {
                Some(value) => value.text()?.to_owned(),
                None => default.to_owned(),
            })
        };
        // The game's default: a jigsaw that faces sideways is aligned, one that faces
        // up or down can be rolled (`JigsawBlockEntity.JointType`).
        let rollable = match tags.get("joint").map(Tag::text).transpose()? {
            Some("rollable") => true,
            Some("aligned") => false,
            Some(other) => bail!("the joint {other:?} is not known"),
            None => orientation < 8,
        };
        let priority = |name: &str| -> Result<i16> {
            let value = match tags.get(name) {
                Some(value) => value.integer()?,
                None => 0,
            };
            i16::try_from(value).with_context(|| format!("the priority {value}"))
        };
        jigsaws.push(Jigsaw {
            position,
            orientation: orientation as u8,
            rollable,
            name: text("name", "minecraft:empty")?,
            target: text("target", "minecraft:empty")?,
            pool: text("pool", "minecraft:empty")?,
            selection_priority: priority("selection_priority")?,
            placement_priority: priority("placement_priority")?,
        });
    }
    // The game sorts a template's blocks with tags by y, then x, then z when it
    // loads it (`StructureTemplate.buildInfoList`), and the files of this jar are
    // in that order already. The sort is stable, as the game's is.
    let before: Vec<[u8; 3]> = jigsaws.iter().map(|jigsaw| jigsaw.position).collect();
    jigsaws.sort_by_key(|jigsaw| (jigsaw.position[1], jigsaw.position[0], jigsaw.position[2]));
    ensure!(
        before
            .iter()
            .zip(&jigsaws)
            .all(|(before, jigsaw)| *before == jigsaw.position),
        "the jigsaw blocks of the file are not in the order y, x, z (ADR-0019, section 1)"
    );
    byte(jigsaws.len() as i64, "the number of jigsaw blocks")?;
    Ok(Template {
        name: name.to_owned(),
        size,
        exists: true,
        jigsaws,
    })
}

/// Every template of the jar, and a row that says "absent" for each template a pool
/// names and the jar does not have, sorted by name.
pub fn templates(worldgen: &Worldgen) -> Result<Vec<Template>> {
    let mut templates = BTreeMap::new();
    for (path, bytes) in &worldgen.templates {
        let name = path
            .strip_suffix(".nbt")
            .with_context(|| format!("structure/{path} is not a file of tags"))?;
        let read = template(name, bytes).with_context(|| format!("reading structure/{path}"))?;
        templates.insert(name.to_owned(), read);
    }
    fn named(json: &Json, into: &mut Vec<String>) -> Result<()> {
        if let Some(location) = json.get("location") {
            into.push(location.string()?.to_owned());
        }
        if let Some(elements) = json.get("elements") {
            for element in elements.array()? {
                named(element, into)?;
            }
        }
        Ok(())
    }
    let mut referred = Vec::new();
    for json in worldgen.registry("template_pool")?.values() {
        for entry in json.member("elements")?.array()? {
            named(entry.member("element")?, &mut referred)?;
        }
    }
    for location in referred {
        let name = location
            .strip_prefix("minecraft:")
            .with_context(|| format!("the template {location} is not the game's"))?;
        templates
            .entry(name.to_owned())
            .or_insert_with(|| Template {
                name: name.to_owned(),
                size: [0; 3],
                exists: false,
                jigsaws: Vec::new(),
            });
    }
    Ok(templates.into_values().collect())
}

/// `templates.bin`, as `clustine_worldgen_data::structure::Templates` reads it.
pub fn templates_table(templates: &[Template], sha1: &str) -> Result<Vec<u8>> {
    let mut strings: Vec<&str> = Vec::new();
    for template in templates {
        strings.push(&template.name);
        for jigsaw in &template.jigsaws {
            strings.extend([
                jigsaw.name.as_str(),
                jigsaw.target.as_str(),
                jigsaw.pool.as_str(),
            ]);
        }
    }
    strings.sort_unstable();
    strings.dedup();
    let number = |text: &str| -> Result<[u8; 2]> {
        let index = strings
            .binary_search(&text)
            .ok()
            .context("a string went missing")?;
        Ok(u16::try_from(index)
            .context("more than 65,536 strings")?
            .to_le_bytes())
    };

    let mut lengths = Section::new(2);
    let mut texts = Section::new(1);
    for text in &strings {
        lengths.push(
            &u16::try_from(text.len())
                .context("a string too long")?
                .to_le_bytes(),
        );
        for byte in text.bytes() {
            texts.push(&[byte]);
        }
    }
    let mut rows = Section::new(9);
    let mut jigsaws = Section::new(15);
    for template in templates {
        let first = u16::try_from(jigsaws.rows()).context("more than 65,536 jigsaw blocks")?;
        let mut row = Vec::with_capacity(9);
        row.extend(number(&template.name)?);
        row.extend(template.size);
        row.extend(first.to_le_bytes());
        row.push(template.jigsaws.len() as u8);
        row.push(u8::from(!template.exists));
        rows.push(&row);
        for jigsaw in &template.jigsaws {
            let mut row = Vec::with_capacity(15);
            row.extend(jigsaw.position);
            row.push(jigsaw.orientation);
            row.push(u8::from(!jigsaw.rollable));
            row.extend(number(&jigsaw.name)?);
            row.extend(number(&jigsaw.target)?);
            row.extend(number(&jigsaw.pool)?);
            row.extend(jigsaw.selection_priority.to_le_bytes());
            row.extend(jigsaw.placement_priority.to_le_bytes());
            jigsaws.push(&row);
        }
    }
    packed::file(
        packed::KIND_TEMPLATES,
        TEMPLATES_LAYOUT,
        sha1,
        &[lengths, texts, rows, jigsaws],
    )
}

/// One element of a pool.
fn element(json: &Json, templates: &[Template]) -> Result<Rust> {
    let projection = |json: &Json| -> Result<Rust> {
        Ok(Rust::atom(match json.member("projection")?.string()? {
            "rigid" => "Projection::Rigid",
            "terrain_matching" => "Projection::TerrainMatching",
            other => bail!("the projection {other:?} is not known to the emitter"),
        }))
    };
    let kind = json.member("element_type")?.string()?;
    match kind {
        "minecraft:empty_pool_element" => {
            only(json, "an empty element", &["element_type"])?;
            Ok(Rust::atom("PoolElement::Empty"))
        }
        "minecraft:single_pool_element" | "minecraft:legacy_single_pool_element" => {
            only(
                json,
                "an element of one template",
                &["element_type", "location", "processors", "projection"],
            )?;
            let location = json.member("location")?.string()?;
            let name = location
                .strip_prefix("minecraft:")
                .with_context(|| format!("the template {location} is not the game's"))?;
            let template = templates
                .binary_search_by(|template| template.name.as_str().cmp(name))
                .ok()
                .with_context(|| format!("the template {name} went missing"))?;
            let processors = match json.member("processors")? {
                Json::String(name) => name.clone(),
                inline => {
                    only(inline, "a list of processors", &["processors"])?;
                    ensure!(
                        inline.member("processors")?.array()?.is_empty(),
                        "an element lists processors of its own, which the emitter does not know"
                    );
                    String::new()
                }
            };
            Ok(Rust::Record(
                "PoolElement::Single".to_owned(),
                vec![
                    ("template", Rust::atom(template.to_string())),
                    ("processors", Rust::text(&processors)),
                    ("projection", projection(json)?),
                    (
                        "legacy",
                        Rust::atom((kind == "minecraft:legacy_single_pool_element").to_string()),
                    ),
                ],
            ))
        }
        "minecraft:feature_pool_element" => {
            only(
                json,
                "an element of a feature",
                &["element_type", "feature", "projection"],
            )?;
            Ok(Rust::Record(
                "PoolElement::Feature".to_owned(),
                vec![
                    ("feature", Rust::text(json.member("feature")?.string()?)),
                    ("projection", projection(json)?),
                ],
            ))
        }
        "minecraft:list_pool_element" => {
            only(
                json,
                "a list of elements",
                &["element_type", "elements", "projection"],
            )?;
            let mut elements = Vec::new();
            for inner in json.member("elements")?.array()? {
                elements.push(element(inner, templates)?);
            }
            ensure!(!elements.is_empty(), "a list of no elements");
            Ok(Rust::Record(
                "PoolElement::List".to_owned(),
                vec![
                    ("elements", Rust::Slice(elements)),
                    ("projection", projection(json)?),
                ],
            ))
        }
        other => bail!("a pool element that is {other:?} is not known to the emitter"),
    }
}

/// `template_pools.rs`: the template pools.
pub fn template_pools(worldgen: &Worldgen, templates: &[Template]) -> Result<String> {
    let pools = worldgen.registry("template_pool")?;
    let pool_names: Vec<&String> = pools.keys().collect();
    let mut out = String::new();
    for line in [
        "//! The game's template pools (`worldgen/template_pool`), sorted by name. A pool names",
        "//! its fallback by its place in `POOLS` and an element its template by its place in the",
        "//! table of templates (`crate::templates`); the elements are in the data's order.",
        "",
        "use crate::structure::{PoolElement, Projection, TemplatePool};",
        "",
    ] {
        writeln!(out, "{line}")?;
    }
    writeln!(out, "pub static POOLS: [TemplatePool; {}] = [", pools.len())?;
    for (name, json) in pools {
        let read = || -> Result<Rust> {
            only(json, "a template pool", &["fallback", "elements"])?;
            let mut elements = Vec::new();
            for entry in json.member("elements")?.array()? {
                only(entry, "an entry of a template pool", &["element", "weight"])?;
                let weight = entry.member("weight")?.integer()?;
                ensure!((1..=150).contains(&weight), "a weight of {weight}");
                elements.push(Rust::Call(
                    String::new(),
                    vec![
                        element(entry.member("element")?, templates)?,
                        Rust::atom(weight.to_string()),
                    ],
                ));
            }
            Ok(Rust::Record(
                "TemplatePool".to_owned(),
                vec![
                    ("name", Rust::text(name)),
                    (
                        "fallback",
                        Rust::atom(
                            place(
                                &pool_names,
                                json.member("fallback")?.string()?,
                                "template pool",
                            )?
                            .to_string(),
                        ),
                    ),
                    ("elements", Rust::Slice(elements)),
                ],
            ))
        };
        read()
            .with_context(|| format!("reading the template pool {name}"))?
            .write(&mut out, 4, "", ",");
    }
    writeln!(out, "];")?;
    check_layout(&out)?;
    Ok(out)
}

/// A packed table of templates as text, a template or a jigsaw block a line.
pub fn dump(table: &packed::Parsed<'_>, out: &mut String) -> Result<()> {
    let [lengths, texts, templates, jigsaws] = &table.sections[..] else {
        bail!("the table of templates has four sections");
    };
    let mut strings = Vec::new();
    let mut at = 0;
    let all: Vec<u8> = texts.rows().map(|row| row[0]).collect();
    for row in lengths.rows() {
        let length = usize::from(u16::from_le_bytes([row[0], row[1]]));
        strings.push(std::str::from_utf8(&all[at..at + length])?.to_owned());
        at += length;
    }
    let string = |row: &[u8], at: usize| -> Result<&String> {
        strings
            .get(usize::from(u16::from_le_bytes([row[at], row[at + 1]])))
            .context("a string the table lacks")
    };
    for row in templates.rows() {
        let first = usize::from(u16::from_le_bytes([row[5], row[6]]));
        writeln!(
            out,
            "{} {}x{}x{}{}",
            string(row, 0)?,
            row[2],
            row[3],
            row[4],
            if row[8] & 1 == 1 { " absent" } else { "" }
        )?;
        for index in first..first + usize::from(row[7]) {
            let jigsaw = jigsaws.row(index)?;
            writeln!(
                out,
                "  jigsaw {},{},{} {} {} name={} target={} pool={} selection={} placement={}",
                jigsaw[0],
                jigsaw[1],
                jigsaw[2],
                ORIENTATIONS
                    .get(usize::from(jigsaw[3]))
                    .context("an orientation the table does not know")?,
                if jigsaw[4] == 0 {
                    "rollable"
                } else {
                    "aligned"
                },
                string(jigsaw, 5)?,
                string(jigsaw, 7)?,
                string(jigsaw, 9)?,
                i16::from_le_bytes([jigsaw[11], jigsaw[12]]),
                i16::from_le_bytes([jigsaw[13], jigsaw[14]]),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_laid_out_on_one_line_where_it_fits_and_a_member_a_line_otherwise() {
        let short = Rust::Record("A".to_owned(), vec![("b", Rust::atom("1"))]);
        let mut out = String::new();
        short.write(&mut out, 4, "", ",");
        assert_eq!(out, "    A { b: 1 },\n");

        let long = Rust::Record(
            "Long".to_owned(),
            vec![
                (
                    "first",
                    Rust::Slice((0..40).map(|n| Rust::atom(n.to_string())).collect()),
                ),
                ("second", Rust::text("x")),
            ],
        );
        let mut out = String::new();
        long.write(&mut out, 0, "", ",");
        assert!(out.starts_with("Long {\n    first: &[\n        0, 1, 2, "));
        assert!(out.ends_with("    second: \"x\",\n},\n"));
        assert!(out.lines().all(|line| line.len() <= LINE_WIDTH));
    }

    #[test]
    fn a_height_is_an_anchor_or_evenly_between_two() {
        let constant = Json::parse(r#"{"absolute": -27}"#).unwrap();
        assert_eq!(
            height_provider(&constant).unwrap().flat(),
            "HeightProvider::Constant(Anchor::Absolute(-27))"
        );
        let uniform = Json::parse(
            r#"{"type": "minecraft:uniform", "min_inclusive": {"absolute": 32},
                "max_inclusive": {"below_top": 2}}"#,
        )
        .unwrap();
        assert_eq!(
            height_provider(&uniform).unwrap().flat(),
            "HeightProvider::Uniform { min_inclusive: Anchor::Absolute(32), \
             max_inclusive: Anchor::BelowTop(2) }"
        );
        let unknown = Json::parse(r#"{"type": "minecraft:trapezoid"}"#).unwrap();
        assert!(height_provider(&unknown).is_err());
    }

    #[test]
    fn a_member_the_emitter_does_not_know_fails() {
        let pools: Vec<&String> = Vec::new();
        let structure = Json::parse(
            r#"{"type": "minecraft:shipwreck", "biomes": [], "step": "surface_structures",
                "spawn_overrides": {}, "is_beached": true, "is_haunted": true}"#,
        )
        .unwrap();
        let error = kind(&structure, &pools).unwrap_err();
        assert!(format!("{error:#}").contains("is_haunted"));
    }
}
