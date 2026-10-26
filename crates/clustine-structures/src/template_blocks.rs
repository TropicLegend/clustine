// Adapted from SteelMC's `steel-core/src/worldgen/template/loading.rs`.

//! The blocks of structure templates, read from the files the game's jar holds
//! (`data/minecraft/structure/**.nbt`), as the game's `StructureTemplate.load` reads
//! them: the size, one palette or several, every block with its place in the
//! template, its state and its block entity's data, and the entities, which are kept
//! as data and made by whoever places the template (`place::PlacedEntity`).
//!
//! The files are Mojang's and are not committed (the terrain plan's question 3): the
//! binary reads them from the operator's own jar when the server starts and hands the
//! bytes in, so nothing here reads a file. [`TemplateLibrary::from_files`] takes
//! `(name, bytes)` pairs.
//!
//! The game brings a file saved by an older version up to date with its data fixers
//! before reading it. Clustine has none, so a file of another data version than the
//! pinned jar's is refused; every template of the pinned jar is of its own version.
//! Where the game is lenient (a block it does not know becomes air, a property value it
//! does not know is left at the default) this is strict and refuses the file, since in
//! the pinned jar neither happens and either would mean a jar that is not it.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read as _;

use clustine_data::{BlockClass, BlockState, DATA_VERSION};
use clustine_protocol::codec::Reader as NbtReader;
use clustine_protocol::nbt::Nbt;
use clustine_world::BlockPos;
use flate2::read::GzDecoder;

use crate::blocks::is_class;

/// One block of a template (`StructureBlockInfo`): where it stands in the template,
/// its state, and its block entity's data as the file has it.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateBlock {
    pub pos: BlockPos,
    pub state: BlockState,
    /// A compound, with the block entity's `id` among its members.
    pub nbt: Option<Nbt>,
}

/// An entity of a template (`StructureEntityInfo`), kept as data: the placing hands it
/// back moved, and the run makes it (`place::PlacedEntity`).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateEntity {
    /// Where it stands in the template, to a fraction of a block.
    pub pos: [f64; 3],
    /// The block it stands in.
    pub block_pos: BlockPos,
    /// Its data, a compound with its `id`.
    pub nbt: Nbt,
}

/// The blocks of one template (`StructureTemplate`).
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateBlocks {
    pub size: [i32; 3],
    /// The data version the file was saved by.
    pub data_version: i32,
    /// Each palette's blocks in the order the game places them: blocks whose
    /// collision fills the cube first, then the other blocks without data, then those
    /// with data, each part by y, then x, then z (`buildInfoList`).
    pub palettes: Vec<Vec<TemplateBlock>>,
    pub entities: Vec<TemplateEntity>,
}

/// Why a template could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    /// The template's name, where it is known.
    pub template: String,
    pub what: String,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "structure template {}: {}", self.template, self.what)
    }
}

impl std::error::Error for TemplateError {}

/// The templates of a jar, by name.
#[derive(Debug, Clone, Default)]
pub struct TemplateLibrary {
    templates: BTreeMap<String, TemplateBlocks>,
}

impl TemplateLibrary {
    /// Reads every template of `files`, each a name and the bytes of its file as the
    /// jar holds it (gzip-compressed NBT). A name is a template's name as the data
    /// gives it, with or without its namespace (`minecraft:fossil/spine_1`,
    /// `fossil/spine_1`); the library keeps it without `minecraft:`, as the table of
    /// templates in `clustine-worldgen-data` does. Fails on the first file that cannot
    /// be read, naming it.
    pub fn from_files(
        files: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, TemplateError> {
        let mut templates = BTreeMap::new();
        for (name, bytes) in files {
            let name = plain_name(&name).to_string();
            let template = parse_template(&bytes).map_err(|what| TemplateError {
                template: name.clone(),
                what,
            })?;
            templates.insert(name, template);
        }
        Ok(Self { templates })
    }

    /// The template called `name`, with or without `minecraft:`.
    pub fn get(&self, name: &str) -> Option<&TemplateBlocks> {
        self.templates.get(plain_name(name))
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// The names of the templates, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.templates.keys().map(String::as_str)
    }
}

fn plain_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// Reads one template from the bytes of its file: gzip-compressed NBT whose root is a
/// named compound.
pub fn parse_template(bytes: &[u8]) -> Result<TemplateBlocks, String> {
    let mut data = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut data)
        .map_err(|error| format!("is not gzip-compressed: {error}"))?;
    let root = read_named_root(&data)?;
    parse_root(&root)
}

/// The root of an NBT file: a compound with a name, which the network's form has not.
/// The name is skipped and the rest read as the network's form.
pub fn read_named_root(data: &[u8]) -> Result<Nbt, String> {
    if data.first() != Some(&10) {
        return Err("the root of the file is not a compound".into());
    }
    let length = data
        .get(1..3)
        .map(|bytes| usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
        .ok_or("the file ends in the root's name")?;
    let rest = data
        .get(3 + length..)
        .ok_or("the file ends in the root's name")?;
    let mut unnamed = Vec::with_capacity(rest.len() + 1);
    unnamed.push(10);
    unnamed.extend_from_slice(rest);
    let mut reader = NbtReader::new(&unnamed);
    let root = reader
        .nbt()
        .map_err(|error| format!("is not NBT: {error:?}"))?
        .ok_or("the root is empty")?;
    reader
        .finish()
        .map_err(|_| "has bytes after its root".to_string())?;
    Ok(root)
}

/// Reads a template from the root compound of its file (`StructureTemplate.load`).
pub fn parse_root(root: &Nbt) -> Result<TemplateBlocks, String> {
    let data_version = match root.get("DataVersion") {
        Some(Nbt::Int(version)) => *version,
        Some(_) => return Err("its DataVersion is not an int".into()),
        // The game takes a file without one to be of version 500 and fixes it up.
        None => return Err("has no DataVersion, so would need the game's data fixers".into()),
    };
    if data_version != DATA_VERSION {
        return Err(format!(
            "was saved by data version {data_version}, not the pinned jar's {DATA_VERSION}; \
             the game would bring it up to date with its data fixers, which Clustine has not"
        ));
    }
    let size = ints3(root.get("size"), "size")?;
    let palettes: Vec<&[Nbt]> = if let Some(palette) = root.get("palette") {
        vec![list(palette, "palette")?]
    } else if let Some(palettes) = root.get("palettes") {
        list(palettes, "palettes")?
            .iter()
            .map(|palette| list(palette, "a palette of palettes"))
            .collect::<Result<_, _>>()?
    } else {
        return Err("has neither a palette nor palettes".into());
    };
    let blocks = list(root.get("blocks").ok_or("has no blocks")?, "blocks")?;
    let palettes = palettes
        .into_iter()
        .map(|palette| {
            let states = palette
                .iter()
                .map(palette_state)
                .collect::<Result<Vec<_>, _>>()?;
            load_palette(&states, blocks)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let entities = match root.get("entities") {
        Some(entities) => list(entities, "entities")?
            .iter()
            .map(entity)
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    Ok(TemplateBlocks {
        size,
        data_version,
        palettes,
        entities,
    })
}

/// The blocks of one palette, sorted as the game sorts them (`loadPalette`,
/// `addToLists`, `buildInfoList`).
fn load_palette(states: &[BlockState], blocks: &[Nbt]) -> Result<Vec<TemplateBlock>, String> {
    let mut full = Vec::new();
    let mut with_data = Vec::new();
    let mut other = Vec::new();
    for block in blocks {
        let pos = ints3(block.get("pos"), "a block's pos")?;
        let index = match block.get("state") {
            Some(Nbt::Int(index)) => *index,
            _ => return Err("a block has no state".into()),
        };
        let state = usize::try_from(index)
            .ok()
            .and_then(|index| states.get(index).copied())
            .ok_or_else(|| format!("a block's state {index} is not in its palette"))?;
        let nbt = match block.get("nbt") {
            Some(nbt @ Nbt::Compound(_)) => Some(nbt.clone()),
            Some(_) => return Err("a block's nbt is not a compound".into()),
            None => None,
        };
        let info = TemplateBlock {
            pos: BlockPos::new(pos[0], pos[1], pos[2]),
            state,
            nbt,
        };
        if info.nbt.is_some() {
            with_data.push(info);
        } else if !has_dynamic_shape(state) && state.props().collision_is_full_block() {
            full.push(info);
        } else {
            other.push(info);
        }
    }
    let by_y_x_z = |block: &TemplateBlock| (block.pos.y, block.pos.x, block.pos.z);
    // Stable, as Java's `List.sort` is.
    full.sort_by_key(by_y_x_z);
    other.sort_by_key(by_y_x_z);
    with_data.sort_by_key(by_y_x_z);
    full.extend(other);
    full.extend(with_data);
    Ok(full)
}

/// The game's `hasDynamicShape`: blocks whose shape comes from more than their state.
/// Of those, only a shulker box (closed, as it is without its block entity) and a
/// moving piston could otherwise count as filling the cube.
fn has_dynamic_shape(state: BlockState) -> bool {
    is_class(state, BlockClass::ShulkerBoxBlock) || is_class(state, BlockClass::MovingPistonBlock)
}

/// A state of a palette, as 26.3 writes one: `{id: ..., properties: {...}}`.
fn palette_state(entry: &Nbt) -> Result<BlockState, String> {
    let Some(Nbt::String(name)) = entry.get("id") else {
        return Err(format!("a palette's entry {entry} has no id"));
    };
    let mut pairs = Vec::new();
    match entry.get("properties") {
        Some(Nbt::Compound(properties)) => {
            for (property, value) in properties {
                let Nbt::String(value) = value else {
                    return Err(format!("{name}'s property {property} is not a string"));
                };
                pairs.push((property.as_str(), value.as_str()));
            }
        }
        Some(_) => return Err(format!("{name}'s properties are not a compound")),
        None => {}
    }
    BlockState::from_name_and_properties(name, pairs.iter().copied())
        .ok_or_else(|| format!("{name} with {pairs:?} is not a block state of the pinned jar"))
}

fn entity(entry: &Nbt) -> Result<TemplateEntity, String> {
    let pos = match entry.get("pos") {
        Some(Nbt::List(values)) if values.len() == 3 => {
            let mut pos = [0.0; 3];
            for (slot, value) in pos.iter_mut().zip(values) {
                let Nbt::Double(value) = value else {
                    return Err("an entity's pos is not of doubles".into());
                };
                *slot = *value;
            }
            pos
        }
        _ => return Err("an entity has no pos of three doubles".into()),
    };
    let block_pos = ints3(entry.get("blockPos"), "an entity's blockPos")?;
    let nbt = match entry.get("nbt") {
        Some(nbt @ Nbt::Compound(_)) => nbt.clone(),
        _ => return Err("an entity has no nbt".into()),
    };
    Ok(TemplateEntity {
        pos,
        block_pos: BlockPos::new(block_pos[0], block_pos[1], block_pos[2]),
        nbt,
    })
}

fn list<'a>(value: &'a Nbt, what: &str) -> Result<&'a [Nbt], String> {
    match value {
        Nbt::List(items) => Ok(items),
        _ => Err(format!("{what} is not a list")),
    }
}

fn ints3(value: Option<&Nbt>, what: &str) -> Result<[i32; 3], String> {
    match value {
        Some(Nbt::List(items)) if items.len() == 3 => {
            let mut ints = [0; 3];
            for (slot, item) in ints.iter_mut().zip(items) {
                let Nbt::Int(value) = item else {
                    return Err(format!("{what} is not a list of ints"));
                };
                *slot = *value;
            }
            Ok(ints)
        }
        _ => Err(format!("{what} is not a list of three ints")),
    }
}

impl TemplateLibrary {
    /// A library of templates already read, each by its name with or without
    /// `minecraft:`: for the tests of what places templates, which make theirs by hand.
    pub fn from_templates(templates: impl IntoIterator<Item = (String, TemplateBlocks)>) -> Self {
        Self {
            templates: templates
                .into_iter()
                .map(|(name, template)| (plain_name(&name).to_string(), template))
                .collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;

    use clustine_data::blocks;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;
    use crate::blocks::nbt_bytes;

    pub fn compound(members: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(
            members
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
        )
    }

    pub fn ints(values: [i32; 3]) -> Nbt {
        Nbt::List(values.into_iter().map(Nbt::Int).collect())
    }

    pub fn state_entry(name: &str, properties: &[(&str, &str)]) -> Nbt {
        let mut members = vec![("id", Nbt::String(name.into()))];
        if !properties.is_empty() {
            members.push((
                "properties",
                compound(
                    properties
                        .iter()
                        .map(|(key, value)| (*key, Nbt::String((*value).into())))
                        .collect(),
                ),
            ));
        }
        compound(members)
    }

    pub fn block_entry(pos: [i32; 3], state: i32, nbt: Option<Nbt>) -> Nbt {
        let mut members = vec![("pos", ints(pos)), ("state", Nbt::Int(state))];
        if let Some(nbt) = nbt {
            members.push(("nbt", nbt));
        }
        compound(members)
    }

    /// The bytes of a template file as the jar holds one: gzip over a named root.
    pub fn file(root: &Nbt) -> Vec<u8> {
        let unnamed = nbt_bytes(root);
        let mut named = vec![10, 0, 0];
        named.extend_from_slice(&unnamed[1..]);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&named).unwrap();
        encoder.finish().unwrap()
    }

    pub fn root(size: [i32; 3], palette: Vec<Nbt>, blocks: Vec<Nbt>) -> Nbt {
        compound(vec![
            ("size", ints(size)),
            ("entities", Nbt::List(Vec::new())),
            ("blocks", Nbt::List(blocks)),
            ("palette", Nbt::List(palette)),
            ("DataVersion", Nbt::Int(DATA_VERSION)),
        ])
    }

    #[test]
    fn a_template_file_is_read_with_its_blocks_sorted_as_the_game_sorts_them() {
        let chest_data = compound(vec![
            (
                "LootTable",
                Nbt::String("minecraft:chests/igloo_chest".into()),
            ),
            ("id", Nbt::String("minecraft:chest".into())),
        ]);
        let template = root(
            [2, 2, 1],
            vec![
                state_entry("minecraft:oak_fence", &[]),
                state_entry("minecraft:stone", &[]),
                state_entry("minecraft:chest", &[("facing", "east")]),
            ],
            vec![
                block_entry([1, 1, 0], 1, None),
                block_entry([0, 0, 0], 2, Some(chest_data.clone())),
                block_entry([1, 0, 0], 0, None),
                block_entry([0, 1, 0], 1, None),
            ],
        );
        let blocks = parse_template(&file(&template)).unwrap();
        assert_eq!(blocks.size, [2, 2, 1]);
        assert_eq!(blocks.data_version, DATA_VERSION);
        assert_eq!(blocks.palettes.len(), 1);
        let order: Vec<(i32, i32, BlockState)> = blocks.palettes[0]
            .iter()
            .map(|block| (block.pos.x, block.pos.y, block.state))
            .collect();
        let chest = blocks::CHEST.with_property("facing", "east").unwrap();
        assert_eq!(
            order,
            [
                (0, 1, blocks::STONE),
                (1, 1, blocks::STONE),
                (1, 0, blocks::OAK_FENCE),
                (0, 0, chest),
            ],
            "full blocks by y, x and z first, then the others, then those with data"
        );
        assert_eq!(blocks.palettes[0][3].nbt, Some(chest_data));
    }

    #[test]
    fn every_palette_of_a_template_with_several_places_the_same_blocks() {
        let mut template = root([1, 1, 1], Vec::new(), vec![block_entry([0, 0, 0], 0, None)]);
        let Nbt::Compound(members) = &mut template else {
            unreachable!()
        };
        members.retain(|(key, _)| key != "palette");
        members.push((
            "palettes".into(),
            Nbt::List(vec![
                Nbt::List(vec![state_entry("minecraft:oak_planks", &[])]),
                Nbt::List(vec![state_entry("minecraft:spruce_planks", &[])]),
            ]),
        ));
        let blocks = parse_template(&file(&template)).unwrap();
        assert_eq!(blocks.palettes.len(), 2);
        assert_eq!(blocks.palettes[0][0].state, blocks::OAK_PLANKS);
        assert_eq!(blocks.palettes[1][0].state, blocks::SPRUCE_PLANKS);
    }

    #[test]
    fn entities_are_kept_as_data() {
        let mut template = root(
            [1, 1, 1],
            vec![state_entry("minecraft:air", &[])],
            Vec::new(),
        );
        let Nbt::Compound(members) = &mut template else {
            unreachable!()
        };
        let cat = compound(vec![("id", Nbt::String("minecraft:cat".into()))]);
        members.retain(|(key, _)| key != "entities");
        members.push((
            "entities".into(),
            Nbt::List(vec![compound(vec![
                (
                    "pos",
                    Nbt::List(vec![Nbt::Double(0.5), Nbt::Double(0.0), Nbt::Double(0.25)]),
                ),
                ("blockPos", ints([0, 0, 0])),
                ("nbt", cat.clone()),
            ])]),
        ));
        let blocks = parse_template(&file(&template)).unwrap();
        assert_eq!(
            blocks.entities,
            [TemplateEntity {
                pos: [0.5, 0.0, 0.25],
                block_pos: BlockPos::new(0, 0, 0),
                nbt: cat,
            }]
        );
    }

    #[test]
    fn a_file_of_another_version_or_with_a_block_the_jar_has_not_is_refused() {
        let mut old = root(
            [1, 1, 1],
            vec![state_entry("minecraft:stone", &[])],
            Vec::new(),
        );
        let Nbt::Compound(members) = &mut old else {
            unreachable!()
        };
        crate::blocks::put(members, "DataVersion", Nbt::Int(3955));
        let error = parse_template(&file(&old)).unwrap_err();
        assert!(error.contains("3955"), "{error}");

        let unknown = root(
            [1, 1, 1],
            vec![state_entry("minecraft:no_such_block", &[])],
            Vec::new(),
        );
        assert!(parse_template(&file(&unknown)).is_err());
        let bad_value = root(
            [1, 1, 1],
            vec![state_entry("minecraft:oak_log", &[("axis", "w")])],
            Vec::new(),
        );
        assert!(parse_template(&file(&bad_value)).is_err());
        let out_of_palette = root(
            [1, 1, 1],
            vec![state_entry("minecraft:stone", &[])],
            vec![block_entry([0, 0, 0], 1, None)],
        );
        assert!(parse_template(&file(&out_of_palette)).is_err());
        assert!(parse_template(b"not gzip").is_err());
    }

    #[test]
    fn the_library_finds_a_template_with_or_without_its_namespace_and_names_a_bad_file() {
        let stone = root(
            [1, 1, 1],
            vec![state_entry("minecraft:stone", &[])],
            vec![block_entry([0, 0, 0], 0, None)],
        );
        let library = TemplateLibrary::from_files([
            ("minecraft:fossil/spine_1".to_string(), file(&stone)),
            ("desert_well/well".to_string(), file(&stone)),
        ])
        .unwrap();
        assert_eq!(library.len(), 2);
        assert!(library.get("fossil/spine_1").is_some());
        assert!(library.get("minecraft:desert_well/well").is_some());
        assert!(library.get("igloo/top").is_none());
        assert_eq!(
            library.names().collect::<Vec<_>>(),
            ["desert_well/well", "fossil/spine_1"]
        );

        let error =
            TemplateLibrary::from_files([("igloo/top".to_string(), b"x".to_vec())]).unwrap_err();
        assert_eq!(error.template, "igloo/top");
    }
}
