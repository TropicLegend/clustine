//! Reads the data generator's output, and the extract program's, into the shape the
//! emitters need.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::extract::Extract;
use crate::loot;
use crate::worldgen::Worldgen;

/// Registries the server sends to the client during configuration, in the order the
/// vanilla server sends them (`RegistryDataLoader.SYNCHRONIZED_REGISTRIES`).
const SYNCED_REGISTRIES: [&str; 32] = [
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
    "decorated_pot_pattern",
    "block_transformer",
    "worldgen/block_state_provider",
];

/// Built-in registries whose tags the client needs. Their ids are fixed by the game.
const TAGGED_STATIC_REGISTRIES: [&str; 7] = [
    "block",
    "entity_type",
    "fluid",
    "game_event",
    "item",
    "point_of_interest_type",
    "potion",
];

/// Protocol states in connection order, then directions.
const PACKET_STATES: [&str; 5] = ["handshake", "status", "login", "configuration", "play"];
const PACKET_DIRECTIONS: [&str; 2] = ["clientbound", "serverbound"];

pub struct GameData {
    pub version: Version,
    pub packets: Vec<PacketSet>,
    pub blocks: Vec<Block>,
    pub items: Vec<Item>,
    pub entity_types: Vec<String>,
    /// The sound events by the ids of `registries.json`, which the sound packets name.
    pub sound_events: Vec<String>,
    pub synced_registries: Vec<Registry>,
    pub dimension_types: Vec<DimensionType>,
    pub tags: Vec<RegistryTags>,
    /// The fluids and the block entity types by the ids of `registries.json`.
    pub fluids: Vec<String>,
    pub block_entity_types: Vec<String>,
    /// The data generator's report of the biome parameter lists, which the lists of
    /// the extract program are checked against.
    pub biome_reports: Vec<BiomeReport>,
    /// What the extract program wrote.
    pub extract: Extract,
    /// The world-generation data, from the entries of the game's own jar.
    pub worldgen: Worldgen,
    /// The components of each item that the step of items reads, by item id.
    pub item_components: Vec<ItemComponents>,
    /// The loot tables of blocks and entities, compiled, with what smelting makes.
    pub loot: loot::Compiled,
}

/// What the default components of an item say that Clustine reads
/// (`docs/groundwork/n4-brief.md`, section 11, item 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemComponents {
    /// `minecraft:max_stack_size`.
    pub max_stack_size: u8,
    /// `minecraft:max_damage`, for an item that can be damaged.
    pub max_damage: Option<u16>,
    /// The damage type tag of `minecraft:damage_resistant`, without its `#`.
    pub damage_resistant: Option<String>,
    /// `minecraft:equippable`: the slot it is worn in, as the ordinal of the game's
    /// `EquipmentSlot`, and the sound putting it on plays, by its name.
    pub equippable: Option<(u8, String)>,
    /// `minecraft:attack_animation`: its kind's id and its duration, `None` where the
    /// component has the codec's defaults (`SwingAnimation.DEFAULT`).
    pub attack_animation: Option<(u8, u8)>,
    /// The entity type `minecraft:entity_data` names: what a spawn egg makes.
    pub entity: Option<String>,
    /// `minecraft:attribute_modifiers`, in the item's order.
    pub modifiers: Vec<ModifierReport>,
    /// `minecraft:weapon`: the durability an attack costs and the seconds it disables a
    /// shield for.
    pub weapon: Option<(u32, u32)>,
    /// `minecraft:damage_type`: the damage type an attack with it deals, by name.
    pub damage_type: Option<String>,
    /// `minecraft:minimum_attack_charge`, as the float's bits.
    pub minimum_attack_charge: Option<u32>,
    /// `minecraft:attack_range`: the least and greatest reach, for a survival and a
    /// creative player, the margin added to a target's box, and the factor for a mob
    /// that wields it, each as the float's bits.
    pub attack_range: Option<[u32; 6]>,
}

/// One of an item's attribute modifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModifierReport {
    pub attribute: String,
    pub id: String,
    /// The amount's double bits.
    pub amount: u64,
    /// The ordinal of `AttributeModifier.Operation`.
    pub operation: u8,
    /// The ordinal of `EquipmentSlotGroup`.
    pub slot: u8,
}

/// `AttributeModifier.Operation`'s serialised names, by ordinal.
const OPERATIONS: [&str; 3] = ["add_value", "add_multiplied_base", "add_multiplied_total"];

/// `EquipmentSlotGroup`'s serialised names, by ordinal.
const SLOT_GROUPS: [&str; 11] = [
    "any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body",
    "saddle",
];

/// The game's `SwingAnimationType`s by their serialised names, in the order of their
/// ids.
const SWING_KINDS: [&str; 3] = ["none", "whack", "stab"];

/// `SwingAnimation.DEFAULT`: what the component's codec gives for what it leaves out.
const DEFAULT_SWING: (u8, u8) = (1, 6);

/// The game's `EquipmentSlot`s by their serialised names, in the order of their
/// ordinals.
const EQUIPMENT_SLOTS: [&str; 8] = [
    "mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle",
];

/// The sound `Equippable`'s codec gives an item whose component names none
/// (**Inferred**: `Equippable` is not in the dump; the reports leave the sound out for
/// heads, pumpkins and the shield, as a codec with a default does).
const DEFAULT_EQUIP_SOUND: &str = "minecraft:item.armor.equip_generic";

/// Reads the default components of an item from the report of its components.
pub fn item_components_of(report: &Value) -> Result<ItemComponents> {
    let components = report["components"]
        .as_object()
        .context("the report has no components")?;
    let max_stack_size = components
        .get("minecraft:max_stack_size")
        .and_then(Value::as_u64)
        .context("no maximum stack size")?;
    ensure!(
        (1..=99).contains(&max_stack_size),
        "a maximum stack size of {max_stack_size}"
    );
    let max_damage = match components.get("minecraft:max_damage") {
        None => None,
        Some(value) => {
            let damage = value
                .as_u64()
                .context("the maximum damage is not a number")?;
            ensure!(
                (1..=u64::from(u16::MAX)).contains(&damage),
                "a maximum damage of {damage}"
            );
            // A stack of a damageable item starts undamaged; the step reads no other.
            ensure!(
                components.get("minecraft:damage").and_then(Value::as_u64) == Some(0),
                "a damageable item whose default damage is not 0"
            );
            Some(damage as u16)
        }
    };
    let damage_resistant = match components.get("minecraft:damage_resistant") {
        None => None,
        Some(value) => {
            let object = value
                .as_object()
                .context("the damage resistance is not an object")?;
            ensure!(
                object.len() == 1,
                "a damage resistance of more than its types is not understood"
            );
            let types = object
                .get("types")
                .and_then(Value::as_str)
                .context("a damage resistance without one tag of types")?;
            let tag = types
                .strip_prefix('#')
                .context("a damage resistance to other than a tag is not understood")?;
            Some(tag.to_owned())
        }
    };
    let equippable = match components.get("minecraft:equippable") {
        None => None,
        Some(value) => {
            let slot = value
                .get("slot")
                .and_then(Value::as_str)
                .context("an equippable component without a slot")?;
            let slot = EQUIPMENT_SLOTS
                .iter()
                .position(|name| *name == slot)
                .with_context(|| format!("an equipment slot {slot}"))?;
            let sound = match value.get("equip_sound") {
                None => DEFAULT_EQUIP_SOUND.to_owned(),
                Some(sound) => sound
                    .as_str()
                    .context("an equip sound other than a sound event's name")?
                    .to_owned(),
            };
            Some((slot as u8, sound))
        }
    };
    let attack_animation = match components.get("minecraft:attack_animation") {
        None => None,
        Some(value) => {
            let object = value
                .as_object()
                .context("an attack animation that is not an object")?;
            let kind = match object.get("type") {
                None => DEFAULT_SWING.0,
                Some(kind) => {
                    let kind = kind
                        .as_str()
                        .context("an attack animation's type that is not a name")?;
                    SWING_KINDS
                        .iter()
                        .position(|name| *name == kind)
                        .with_context(|| format!("a swing animation {kind}"))?
                        as u8
                }
            };
            let duration = match object.get("duration") {
                None => DEFAULT_SWING.1,
                Some(duration) => {
                    let duration = duration
                        .as_u64()
                        .context("an attack animation's duration that is not a number")?;
                    u8::try_from(duration)
                        .with_context(|| format!("an attack animation of {duration} ticks"))?
                }
            };
            Some((kind, duration)).filter(|swing| *swing != DEFAULT_SWING)
        }
    };
    let entity = match components.get("minecraft:entity_data") {
        None => None,
        Some(value) => Some(
            value
                .get("id")
                .and_then(Value::as_str)
                .context("entity data without the id of a type")?
                .to_owned(),
        ),
    };
    let mut modifiers = Vec::new();
    for modifier in components
        .get("minecraft:attribute_modifiers")
        .and_then(Value::as_array)
        .context("no attribute modifiers")?
    {
        let text = |key: &str| {
            modifier
                .get(key)
                .and_then(Value::as_str)
                .with_context(|| format!("an attribute modifier without its {key}"))
        };
        let operation = text("operation")?;
        let slot = modifier
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or("any");
        modifiers.push(ModifierReport {
            attribute: text("type")?.to_owned(),
            id: text("id")?.to_owned(),
            amount: modifier
                .get("amount")
                .and_then(Value::as_f64)
                .context("an attribute modifier without its amount")?
                .to_bits(),
            operation: OPERATIONS
                .iter()
                .position(|name| *name == operation)
                .with_context(|| format!("an operation {operation}"))? as u8,
            slot: SLOT_GROUPS
                .iter()
                .position(|name| *name == slot)
                .with_context(|| format!("a slot group {slot}"))? as u8,
        });
    }
    // A float a report writes as a decimal, which the game's codec read into a float.
    let float = |value: &Value, what: &str| -> Result<u32> {
        Ok((value
            .as_f64()
            .with_context(|| format!("{what} that is not a number"))? as f32)
            .to_bits())
    };
    let weapon = match components.get("minecraft:weapon") {
        None => None,
        Some(weapon) => {
            let damage = weapon
                .get("item_damage_per_attack")
                .map_or(Ok(1), |value| {
                    value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .context("a weapon's damage per attack")
                })?;
            let seconds = match weapon.get("disable_blocking_for_seconds") {
                None => 0.0_f32.to_bits(),
                Some(value) => float(value, "a weapon's seconds")?,
            };
            Some((damage, seconds))
        }
    };
    let damage_type = components
        .get("minecraft:damage_type")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("a damage type that is not a name")
        })
        .transpose()?;
    let minimum_attack_charge = components
        .get("minecraft:minimum_attack_charge")
        .map(|value| float(value, "a minimum attack charge"))
        .transpose()?;
    let attack_range = match components.get("minecraft:attack_range") {
        None => None,
        Some(range) => {
            let mut values = [0; 6];
            for (value, key) in values.iter_mut().zip([
                "min_reach",
                "max_reach",
                "min_creative_reach",
                "max_creative_reach",
                "hitbox_margin",
                "mob_factor",
            ]) {
                *value = float(
                    range
                        .get(key)
                        .with_context(|| format!("an attack range without its {key}"))?,
                    key,
                )?;
            }
            Some(values)
        }
    };
    Ok(ItemComponents {
        max_stack_size: max_stack_size as u8,
        max_damage,
        damage_resistant,
        equippable,
        attack_animation,
        entity,
        modifiers,
        weapon,
        damage_type,
        minimum_attack_charge,
        attack_range,
    })
}

/// One parameter list as the data generator reports it: numbers as decimals.
pub struct BiomeReport {
    /// `overworld` or `nether`.
    pub name: String,
    pub entries: Vec<BiomeReportEntry>,
}

pub struct BiomeReportEntry {
    pub biome: String,
    /// The lower and upper bound of temperature, humidity, continentalness, erosion,
    /// depth and weirdness.
    pub bounds: [[f64; 2]; 6],
    pub offset: f64,
}

/// The six parameters of a climate in the order the game's point has them.
pub const CLIMATE_PARAMETERS: [&str; 6] = [
    "temperature",
    "humidity",
    "continentalness",
    "erosion",
    "depth",
    "weirdness",
];

pub struct Version {
    pub id: String,
    pub protocol: i64,
    pub data: i64,
}

/// The packets of one protocol state and direction; a packet's index is its id.
pub struct PacketSet {
    pub state: &'static str,
    pub direction: &'static str,
    pub packets: Vec<String>,
}

/// A block with its contiguous range of state ids.
pub struct Block {
    pub name: String,
    pub first_state: u64,
    pub last_state: u64,
    pub default_state: u64,
    /// The properties in the order the game numbers the states by, which is by name,
    /// each with its values in the game's order. The first counts most.
    pub properties: Vec<Property>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Property {
    pub name: String,
    pub values: Vec<String>,
}

pub struct Item {
    pub name: String,
    /// Default state of the block with the same name, if there is one.
    pub block: Option<u64>,
}

/// A synchronised registry; an entry's index is the id Clustine assigns to it.
pub struct Registry {
    pub name: String,
    pub entries: Vec<String>,
}

pub struct DimensionType {
    pub name: String,
    pub min_y: i64,
    pub height: i64,
}

pub struct RegistryTags {
    pub registry: String,
    pub tags: Vec<Tag>,
}

pub struct Tag {
    pub name: String,
    pub entries: Vec<usize>,
}

impl GameData {
    pub fn load(jar: &Path, generated: &Path, extract: &Path) -> Result<Self> {
        let reports = generated.join("reports");
        let data = generated.join("data/minecraft");
        let registries = read_json(&reports.join("registries.json"))?;

        let block_names = static_registry(&registries, "block")?;
        let blocks = load_blocks(&reports, &block_names)?;
        let default_states: BTreeMap<&str, u64> = blocks
            .iter()
            .map(|block| (block.name.as_str(), block.default_state))
            .collect();
        let item_names = static_registry(&registries, "item")?;
        let mut item_components = Vec::new();
        for name in &item_names {
            let path = reports.join(format!(
                "minecraft/components/item/{}.json",
                local_name(name)
            ));
            item_components.push(
                item_components_of(&read_json(&path)?)
                    .with_context(|| format!("reading {}", path.display()))?,
            );
        }
        let entity_types = static_registry(&registries, "entity_type")?;
        let names = loot::Names {
            blocks: &blocks,
            items: &item_names,
            entities: &entity_types,
        };
        let predicates = loot::PredicateDirectory(&data.join("predicate"));
        let mut loot = loot::load(&data.join("loot_table/blocks"), &predicates, &names)?;
        loot.add_entities(&data.join("loot_table"), &predicates, &names)?;
        loot.add_smelting(&data.join("recipe"), &names)?;
        let items = item_names
            .into_iter()
            .map(|name| Item {
                block: default_states.get(name.as_str()).copied(),
                name,
            })
            .collect();

        let mut synced_registries = Vec::new();
        for name in SYNCED_REGISTRIES {
            let entries = entry_names(&data.join(name))?;
            ensure!(!entries.is_empty(), "registry {name} has no entries");
            synced_registries.push(Registry {
                name: name.to_owned(),
                entries,
            });
        }

        let mut dimension_types = Vec::new();
        let dimension_registry = synced_registries
            .iter()
            .find(|registry| registry.name == "dimension_type")
            .context("dimension_type is not a synchronised registry")?;
        for entry in &dimension_registry.entries {
            let file = data
                .join("dimension_type")
                .join(format!("{}.json", local_name(entry)));
            let json = read_json(&file)?;
            dimension_types.push(DimensionType {
                name: entry.clone(),
                min_y: integer(&json["min_y"], "min_y", &file)?,
                height: integer(&json["height"], "height", &file)?,
            });
        }

        let mut tags = Vec::new();
        for name in TAGGED_STATIC_REGISTRIES {
            let entries = static_registry(&registries, name)?;
            tags.push(load_tags(&data.join("tags").join(name), name, &entries)?);
        }
        for registry in &synced_registries {
            let dir = data.join("tags").join(&registry.name);
            if dir.is_dir() {
                tags.push(load_tags(&dir, &registry.name, &registry.entries)?);
            }
        }

        let fluids = static_registry(&registries, "fluid")?;
        let block_entity_types = static_registry(&registries, "block_entity_type")?;
        let extract = Extract::load(extract)?;
        // The ids the program wrote are the game's own; the tables name fluids and
        // block entity types by the report's ids, so the two have to be the same.
        ensure!(
            extract.fluids == fluids,
            "the extract program numbers the fluids otherwise than registries.json"
        );
        ensure!(
            extract.block_entity_types == block_entity_types,
            "the extract program numbers the block entity types otherwise than registries.json"
        );

        let mut biome_reports = Vec::new();
        for name in ["overworld", "nether"] {
            let path = reports.join(format!("biome_parameters/minecraft/{name}.json"));
            biome_reports.push(BiomeReport {
                name: name.to_owned(),
                entries: biome_report_entries(&read_json(&path)?)
                    .with_context(|| format!("reading {}", path.display()))?,
            });
        }

        Ok(Self {
            version: load_version(jar)?,
            packets: load_packets(&reports)?,
            blocks,
            items,
            entity_types,
            sound_events: static_registry(&registries, "sound_event")?,
            synced_registries,
            dimension_types,
            tags,
            fluids,
            block_entity_types,
            biome_reports,
            extract,
            worldgen: Worldgen::load(jar, generated)?,
            item_components,
            loot,
        })
    }

    /// The ids of the blocks in the block tag called `name`.
    pub fn block_tag(&self, name: &str) -> Result<BTreeSet<usize>> {
        let tag = self
            .tags
            .iter()
            .find(|tags| tags.registry == "block")
            .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
            .with_context(|| format!("the generated data has no block tag {name}"))?;
        Ok(tag.entries.iter().copied().collect())
    }

    /// The names of the biomes in the order Clustine numbers them, which is by name.
    pub fn biomes(&self) -> Result<&[String]> {
        let registry = self
            .synced_registries
            .iter()
            .find(|registry| registry.name == "worldgen/biome")
            .context("worldgen/biome is not a synchronised registry")?;
        Ok(&registry.entries)
    }
}

/// Reads a report of biome parameters: a list of entries, each a biome and its
/// parameters, a parameter being one number or a lower and an upper bound.
pub fn biome_report_entries(report: &Value) -> Result<Vec<BiomeReportEntry>> {
    let entries = report["biomes"]
        .as_array()
        .context("the report has no list `biomes`")?;
    let mut read = Vec::new();
    for entry in entries {
        let biome = entry["biome"]
            .as_str()
            .context("an entry has no biome")?
            .to_owned();
        let parameters = &entry["parameters"];
        let mut bounds = [[0.0; 2]; 6];
        for (slot, name) in bounds.iter_mut().zip(CLIMATE_PARAMETERS) {
            *slot = match &parameters[name] {
                Value::Array(pair) => match pair.as_slice() {
                    [low, high] => [number(low, name)?, number(high, name)?],
                    _ => bail!("{name} of {biome} is not a pair of bounds"),
                },
                point => [number(point, name)?, number(point, name)?],
            };
        }
        read.push(BiomeReportEntry {
            biome,
            bounds,
            offset: number(&parameters["offset"], "offset")?,
        });
    }
    Ok(read)
}

fn number(value: &Value, what: &str) -> Result<f64> {
    value
        .as_f64()
        .with_context(|| format!("{what} is not a number"))
}

fn load_version(jar: &Path) -> Result<Version> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)?;
    let mut text = String::new();
    archive
        .by_name("version.json")
        .context("the server jar has no version.json")?
        .read_to_string(&mut text)?;
    let json: Value = serde_json::from_str(&text)?;
    Ok(Version {
        id: json["id"]
            .as_str()
            .context("version.json has no id")?
            .to_owned(),
        protocol: integer(&json["protocol_version"], "protocol_version", jar)?,
        data: integer(&json["world_version"], "world_version", jar)?,
    })
}

fn load_packets(reports: &Path) -> Result<Vec<PacketSet>> {
    let path = reports.join("packets.json");
    let report = read_json(&path)?;
    let states = object(&report, &path)?;
    for state in states.keys() {
        ensure!(
            PACKET_STATES.contains(&state.as_str()),
            "unknown protocol state {state} in {}",
            path.display()
        );
    }

    let mut sets = Vec::new();
    for state in PACKET_STATES {
        for direction in PACKET_DIRECTIONS {
            let Some(packets) = report[state][direction].as_object() else {
                continue;
            };
            let ids = packets.iter().map(|(name, value)| {
                Ok((name.clone(), integer(&value["protocol_id"], name, &path)?))
            });
            sets.push(PacketSet {
                state,
                direction,
                packets: dense(ids, &format!("{state} {direction} packets"))?,
            });
        }
    }
    Ok(sets)
}

pub(crate) fn load_blocks(reports: &Path, names: &[String]) -> Result<Vec<Block>> {
    let path = reports.join("blocks.json");
    let report = read_json(&path)?;
    let mut blocks = Vec::new();
    let mut next_state = 0;
    for name in names {
        let states = report[name]["states"]
            .as_array()
            .with_context(|| format!("{name} has no states in {}", path.display()))?;
        let mut ids = Vec::new();
        let mut default_state = None;
        for state in states {
            let id = integer(&state["id"], name, &path)? as u64;
            if state["default"] == true {
                default_state = Some(id);
            }
            ids.push(id);
        }
        ids.sort_unstable();
        let first_state = ids[0];
        let last_state = ids[ids.len() - 1];
        // The tables store one range per block, which relies on vanilla numbering states
        // block by block in registry order.
        ensure!(
            first_state == next_state && last_state - first_state + 1 == ids.len() as u64,
            "the states of {name} are not a contiguous range following the previous block"
        );
        next_state = last_state + 1;
        let properties = block_properties(&report[name], first_state)
            .with_context(|| format!("reading the properties of {name}"))?;
        blocks.push(Block {
            name: name.clone(),
            first_state,
            last_state,
            default_state: default_state.with_context(|| format!("{name} has no default state"))?,
            properties,
        });
    }
    ensure!(
        next_state <= u64::from(u16::MAX) + 1,
        "{next_state} block states no longer fit the 16-bit BlockState"
    );
    Ok(blocks)
}

/// The properties of one block of `blocks.json`, in the order that numbers its states.
///
/// The game numbers the states of a block as a number written in mixed radix: each
/// property is a digit, the properties sorted by name with the first counting most, and
/// a digit is the place of the value among the property's values. The committed table
/// rests on that to turn a name and properties into a state id, so every state the
/// report lists is checked against it here.
pub fn block_properties(block: &Value, first_state: u64) -> Result<Vec<Property>> {
    let mut properties = Vec::new();
    if let Some(listed) = block["properties"].as_object() {
        for (name, values) in listed {
            let values = values
                .as_array()
                .with_context(|| format!("the values of {name} are not a list"))?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .with_context(|| format!("a value of {name} is not text"))?;
            ensure!(!values.is_empty(), "{name} has no values");
            properties.push(Property {
                name: name.clone(),
                values,
            });
        }
    }
    properties.sort_by(|a, b| a.name.cmp(&b.name));

    let states = block["states"].as_array().context("no states")?;
    let combinations: usize = properties
        .iter()
        .map(|property| property.values.len())
        .product();
    ensure!(
        combinations == states.len(),
        "{} states for {combinations} combinations of property values",
        states.len()
    );
    for state in states {
        let id = state["id"].as_u64().context("a state has no id")?;
        let mut index = 0;
        for property in &properties {
            let value = state["properties"][&property.name]
                .as_str()
                .with_context(|| format!("state {id} has no value of {}", property.name))?;
            let place = property
                .values
                .iter()
                .position(|known| known == value)
                .with_context(|| format!("{value} is no value of {}", property.name))?;
            index = index * property.values.len() + place;
        }
        ensure!(
            first_state + index as u64 == id,
            "state {id} is not where its property values put it"
        );
    }
    Ok(properties)
}

fn load_tags(dir: &Path, registry: &str, entries: &[String]) -> Result<RegistryTags> {
    let ids: BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(id, name)| (name.as_str(), id))
        .collect();

    let mut raw = BTreeMap::new();
    for name in entry_names(dir)? {
        let file = dir.join(format!("{}.json", local_name(&name)));
        let json = read_json(&file)?;
        let values = json["values"]
            .as_array()
            .with_context(|| format!("{} has no values", file.display()))?;
        let mut members = Vec::new();
        for value in values {
            let (id, required) = match value {
                Value::String(id) => (id.as_str(), true),
                _ => (
                    value["id"]
                        .as_str()
                        .with_context(|| format!("malformed entry in {}", file.display()))?,
                    value["required"] != false,
                ),
            };
            members.push((id.to_owned(), required));
        }
        raw.insert(name, members);
    }

    let mut tags = Vec::new();
    for name in raw.keys() {
        let mut resolved = Vec::new();
        resolve_tag(name, &raw, &ids, &mut Vec::new(), &mut resolved)
            .with_context(|| format!("resolving tag {name} of registry {registry}"))?;
        let mut seen = BTreeSet::new();
        resolved.retain(|id| seen.insert(*id));
        tags.push(Tag {
            name: name.clone(),
            entries: resolved,
        });
    }
    Ok(RegistryTags {
        registry: registry.to_owned(),
        tags,
    })
}

/// Appends the entry ids of `tag` to `out`, following references to other tags.
fn resolve_tag<'a>(
    tag: &'a str,
    raw: &'a BTreeMap<String, Vec<(String, bool)>>,
    ids: &BTreeMap<&str, usize>,
    stack: &mut Vec<&'a str>,
    out: &mut Vec<usize>,
) -> Result<()> {
    ensure!(!stack.contains(&tag), "tag {tag} refers to itself");
    stack.push(tag);
    let members = raw.get(tag).with_context(|| format!("unknown tag {tag}"))?;
    for (member, required) in members {
        if let Some(nested) = member.strip_prefix('#') {
            if raw.contains_key(nested) || *required {
                resolve_tag(nested, raw, ids, stack, out)?;
            }
        } else if let Some(id) = ids.get(member.as_str()) {
            out.push(*id);
        } else if *required {
            bail!("tag {tag} lists unknown entry {member}");
        }
    }
    stack.pop();
    Ok(())
}

/// The entries of a built-in registry from `registries.json`, indexed by protocol id.
pub(crate) fn static_registry(registries: &Value, name: &str) -> Result<Vec<String>> {
    let entries = registries[format!("minecraft:{name}")]["entries"]
        .as_object()
        .with_context(|| format!("registries.json has no registry {name}"))?;
    let ids = entries
        .iter()
        .map(|(entry, value)| Ok((entry.clone(), integer(&value["protocol_id"], entry, name)?)));
    dense(ids, &format!("registry {name}"))
}

/// Turns `(name, id)` pairs into a list indexed by id, requiring the ids to be `0..n`.
fn dense(pairs: impl Iterator<Item = Result<(String, i64)>>, what: &str) -> Result<Vec<String>> {
    let pairs = pairs.collect::<Result<Vec<_>>>()?;
    let mut names = vec![String::new(); pairs.len()];
    for (name, id) in pairs {
        let slot = usize::try_from(id)
            .ok()
            .and_then(|id| names.get_mut(id))
            .with_context(|| format!("id {id} of {name} is out of range in {what}"))?;
        ensure!(slot.is_empty(), "id {id} is used twice in {what}");
        *slot = name;
    }
    Ok(names)
}

/// Names of the JSON files below `dir` as `minecraft:` identifiers, sorted.
fn entry_names(dir: &Path) -> Result<Vec<String>> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
        let entries = fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name = file_name
                .to_str()
                .with_context(|| format!("non-UTF-8 name in {}", dir.display()))?;
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), &format!("{prefix}{file_name}/"), out)?;
            } else if let Some(stem) = file_name.strip_suffix(".json") {
                out.push(format!("minecraft:{prefix}{stem}"));
            }
        }
        Ok(())
    }

    let mut names = Vec::new();
    walk(dir, "", &mut names)?;
    names.sort();
    Ok(names)
}

fn local_name(identifier: &str) -> &str {
    identifier.strip_prefix("minecraft:").unwrap_or(identifier)
}

pub(crate) fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn object<'a>(value: &'a Value, path: &Path) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("{} is not a JSON object", path.display()))
}

fn integer(value: &Value, field: &str, source: impl AsRef<Path>) -> Result<i64> {
    value
        .as_i64()
        .with_context(|| format!("{field} is not an integer in {}", source.as_ref().display()))
}
