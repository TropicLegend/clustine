// The shapes of these records follow SteelMC's data model of structures
// (steel-registry/src/structure/mod.rs, set.rs and template_pool.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), made into statics that name each other by
// index; see NOTICE in this crate. The packed table of templates is Clustine's own
// (ADR-0019, section 1).

//! What the emitted structures, structure sets, template pools and templates are
//! written with.
//!
//! `cargo datagen` writes the game's `worldgen/structure`, `worldgen/structure_set`
//! and `worldgen/template_pool` as statics ([`crate::structures`]), each sorted by
//! name, which is the order a server holds them in when it has loaded its data pack
//! (`crates/clustine-terrain/tests/java/GameStructures.java` writes the game's order
//! out, and the judge of `clustine-structures` compares). A structure names its
//! biomes by Clustine's numbers, a set its structures and a pool its fallback by
//! their place in these lists, and a pool element its template by its place in the
//! table of templates.
//!
//! **Of a structure template only its size and its jigsaw blocks are here**
//! ([`Templates`], from `tables/templates.bin`): what the layout of a structure and
//! the bending of the ground need. The blocks of a template are Mojang's buildings
//! and are read from the operator's own jar (ADR-0019, section 4); a jigsaw's
//! `final_state` is part of the building and is not here either.

use crate::material::Anchor;

/// The step of the game's decoration a structure's blocks are placed in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    UndergroundStructures,
    SurfaceStructures,
    UndergroundDecoration,
}

impl Step {
    /// The step's place among the game's eleven (`GenerationStep.Decoration`).
    #[must_use]
    pub const fn ordinal(self) -> usize {
        match self {
            Self::UndergroundStructures => 3,
            Self::SurfaceStructures => 4,
            Self::UndergroundDecoration => 7,
        }
    }
}

/// What a structure does to the ground around it (`TerrainAdjustment`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TerrainAdaptation {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

/// The game's heightmaps a structure can project its start to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heightmap {
    WorldSurfaceWg,
    OceanFloorWg,
}

/// A provider of heights: one anchor, or evenly between two, both included.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HeightProvider {
    Constant(Anchor),
    Uniform {
        min_inclusive: Anchor,
        max_inclusive: Anchor,
    },
}

/// Whether a template's waterlogged blocks take the water they are placed in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiquidSettings {
    ApplyWaterlogging,
    IgnoreWaterlogging,
}

/// A renaming of pools for one start (`PoolAliasBinding`). Pools are named by text:
/// an alias need not be a pool that exists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoolAlias {
    Direct {
        alias: &'static str,
        target: &'static str,
    },
    /// One of the targets, by weight.
    Random {
        alias: &'static str,
        targets: &'static [(&'static str, i32)],
    },
    /// One of the groups, by weight; a group is direct renamings.
    RandomGroup {
        groups: &'static [(&'static [PoolAlias], i32)],
    },
}

/// A structure assembled from templates (`JigsawStructure`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Jigsaw {
    /// The pool the first piece is drawn from, by its place in the pools.
    pub start_pool: u16,
    /// How many pieces deep the assembly goes (the data's `size`).
    pub max_depth: i32,
    pub use_expansion_hack: bool,
    pub project_start_to_heightmap: Option<Heightmap>,
    pub start_height: HeightProvider,
    /// How far from the start a piece may reach, sideways and up or down.
    pub max_distance_horizontal: i32,
    pub max_distance_vertical: i32,
    pub start_jigsaw_name: Option<&'static str>,
    /// How far the structure keeps from the bottom and the top of the world.
    pub padding_bottom: i32,
    pub padding_top: i32,
    pub pool_aliases: &'static [PoolAlias],
    pub liquid_settings: LiquidSettings,
}

/// Where a ruined portal stands (`RuinedPortalPiece.VerticalPlacement`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortalPlacement {
    OnLandSurface,
    PartlyBuried,
    OnOceanFloor,
    InMountain,
    Underground,
    InNether,
}

/// One of the ways a ruined portal is set up, with its weight.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PortalSetup {
    pub placement: PortalPlacement,
    pub air_pocket_probability: f32,
    pub mossiness: f32,
    pub overgrown: bool,
    pub vines: bool,
    pub can_be_cold: bool,
    pub replace_with_blackstone: bool,
    pub weight: f32,
}

/// The kind of a structure with what the data says of it beyond what all have.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Jigsaw(Jigsaw),
    BuriedTreasure,
    DesertPyramid,
    EndCity,
    Fortress,
    Igloo,
    JungleTemple,
    Mineshaft {
        mesa: bool,
    },
    NetherFossil {
        height: HeightProvider,
    },
    OceanMonument,
    OceanRuin {
        warm: bool,
        large_probability: f32,
        cluster_probability: f32,
    },
    RuinedPortal {
        setups: &'static [PortalSetup],
    },
    Shipwreck {
        beached: bool,
    },
    Stronghold,
    SwampHut,
    WoodlandMansion,
}

/// A structure (`worldgen/structure`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Structure {
    pub name: &'static str,
    /// The biomes it may start in, by Clustine's numbers, ascending.
    pub biomes: &'static [u8],
    pub step: Step,
    pub terrain_adaptation: TerrainAdaptation,
    pub kind: Kind,
}

/// How the start chunks of a region of a random spread are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpreadType {
    Linear,
    Triangular,
}

/// How a placement thins out its start chunks (`FrequencyReductionMethod`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrequencyReduction {
    Default,
    LegacyType1,
    LegacyType2,
    LegacyType3,
}

/// No start within so many chunks of a start chunk of another set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExclusionZone {
    /// The other set, by its place in the sets.
    pub other_set: u16,
    pub chunk_count: i32,
}

/// Where the start chunks of a set are.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Spread {
    /// One start chunk in every square of `spacing` chunks.
    Random {
        spacing: i32,
        separation: i32,
        spread_type: SpreadType,
    },
    /// Rings around the origin, as strongholds stand.
    ConcentricRings {
        distance: i32,
        spread: i32,
        count: i32,
        /// The biomes a position is moved towards, by Clustine's numbers, ascending.
        preferred_biomes: &'static [u8],
    },
}

/// The placement of a structure set (`StructurePlacement`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub spread: Spread,
    pub salt: i32,
    pub frequency: f32,
    pub frequency_reduction: FrequencyReduction,
    pub exclusion_zone: Option<ExclusionZone>,
    pub locate_offset: [i32; 3],
}

/// A structure set (`worldgen/structure_set`): structures that share start chunks.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StructureSet {
    pub name: &'static str,
    /// The structures by their place in the structures, each with its weight, in the
    /// data's order: the choice among them is drawn in it.
    pub structures: &'static [(u16, i32)],
    pub placement: Placement,
}

/// How a pool element meets the ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Projection {
    Rigid,
    TerrainMatching,
}

/// An element of a template pool (`StructurePoolElement`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoolElement {
    Empty,
    /// One template. `legacy` is the element type that keeps the template's air.
    Single {
        /// The template, by its place in [`Templates`].
        template: u16,
        /// The name of its processor list; empty where the data lists none.
        processors: &'static str,
        projection: Projection,
        legacy: bool,
    },
    /// A placed feature, which stands on one block.
    Feature {
        feature: &'static str,
        projection: Projection,
    },
    /// Several elements at one place; the first gives the jigsaws.
    List {
        elements: &'static [PoolElement],
        projection: Projection,
    },
}

impl PoolElement {
    #[must_use]
    pub const fn projection(&self) -> Projection {
        match self {
            Self::Empty => Projection::TerrainMatching,
            Self::Single { projection, .. }
            | Self::Feature { projection, .. }
            | Self::List { projection, .. } => *projection,
        }
    }
}

/// A template pool (`worldgen/template_pool`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TemplatePool {
    pub name: &'static str,
    /// The pool to draw from where this one runs out of depth, by its place in the
    /// pools.
    pub fallback: u16,
    /// The elements with their weights, in the data's order: a draw from the pool
    /// shuffles a list that has each element as often as its weight.
    pub elements: &'static [(PoolElement, i32)],
}

/// The direction a jigsaw block faces and where its top is (`FrontAndTop`), in the
/// game's order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Orientation {
    DownEast,
    DownNorth,
    DownSouth,
    DownWest,
    UpEast,
    UpNorth,
    UpSouth,
    UpWest,
    WestUp,
    EastUp,
    NorthUp,
    SouthUp,
}

impl Orientation {
    /// The orientations in the order of their numbers, with the data's names.
    pub const ALL: [(Orientation, &'static str); 12] = [
        (Self::DownEast, "down_east"),
        (Self::DownNorth, "down_north"),
        (Self::DownSouth, "down_south"),
        (Self::DownWest, "down_west"),
        (Self::UpEast, "up_east"),
        (Self::UpNorth, "up_north"),
        (Self::UpSouth, "up_south"),
        (Self::UpWest, "up_west"),
        (Self::WestUp, "west_up"),
        (Self::EastUp, "east_up"),
        (Self::NorthUp, "north_up"),
        (Self::SouthUp, "south_up"),
    ];
}

/// A jigsaw block of a template: where a piece can be joined to another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JigsawBlock {
    pub position: [i32; 3],
    pub orientation: Orientation,
    /// Whether the joined piece may be turned any way about a vertical jigsaw
    /// (`rollable`), or has to be aligned.
    pub rollable: bool,
    pub name: &'static str,
    pub target: &'static str,
    pub pool: &'static str,
    pub selection_priority: i32,
    pub placement_priority: i32,
}

/// What is known of one template.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Template {
    /// Its name without the namespace, as in `village/plains/houses/plains_small_house_1`.
    pub name: &'static str,
    pub size: [i32; 3],
    /// Whether the jar has it: one pool names a template that does not exist.
    pub exists: bool,
    first_jigsaw: usize,
    jigsaws: usize,
}

/// The four bytes a packed table starts with (ADR-0019, section 1).
const MAGIC: [u8; 4] = *b"CLT1";
/// The kind of the table of templates.
pub const KIND_TEMPLATES: u16 = 3;
/// The version of its layout.
pub const TEMPLATES_LAYOUT: u16 = 1;
const TEMPLATE_BYTES: usize = 9;
const JIGSAW_BYTES: usize = 15;

/// The table of templates, read from the bytes `cargo datagen` packed
/// (`tables/templates.bin`): a section of strings (each a row of its length in two
/// bytes; the texts follow in a section of single bytes), a section of templates
/// (name, three bytes of size, the first jigsaw in two bytes, their number, flags)
/// and a section of jigsaws (three bytes of position, orientation, joint, three
/// strings, two priorities), templates sorted by name and a template's jigsaws by
/// y, then x, then z, which is the order of the game's files.
#[derive(Debug)]
pub struct Templates {
    strings: Vec<&'static str>,
    templates: &'static [u8],
    jigsaws: &'static [u8],
    /// The SHA-1 of the jar the table was made from.
    pub jar_sha1: [u8; 20],
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
}

impl Templates {
    /// Reads a packed table. Panics on bytes that are not one: the table is part of
    /// the build, and a test reads it.
    #[must_use]
    pub fn parse(bytes: &'static [u8]) -> Self {
        assert!(
            bytes.len() >= 30 && bytes[..4] == MAGIC,
            "not a packed table"
        );
        assert_eq!(
            u16_at(bytes, 4),
            usize::from(KIND_TEMPLATES),
            "not the table of templates"
        );
        assert_eq!(
            u16_at(bytes, 6),
            usize::from(TEMPLATES_LAYOUT),
            "another layout"
        );
        let mut jar_sha1 = [0; 20];
        jar_sha1.copy_from_slice(&bytes[8..28]);
        let sections = u16_at(bytes, 28);
        assert_eq!(sections, 4, "the table of templates has four sections");
        let mut at = 30 + sections * 8;
        let mut parts: [&'static [u8]; 4] = [&[]; 4];
        let widths = [2, 1, TEMPLATE_BYTES, JIGSAW_BYTES];
        for (section, part) in parts.iter_mut().enumerate() {
            let rows = u32_at(bytes, 30 + section * 8);
            let width = u32_at(bytes, 34 + section * 8);
            assert_eq!(width, widths[section], "a section of another width");
            *part = &bytes[at..at + rows * width];
            at += rows * width;
        }
        assert_eq!(
            at,
            bytes.len(),
            "the table's length is not what its head says"
        );
        let mut strings = Vec::with_capacity(parts[0].len() / 2);
        let mut text_at = 0;
        for row in parts[0].as_chunks::<2>().0 {
            let length = u16_at(row, 0);
            let text = std::str::from_utf8(&parts[1][text_at..text_at + length])
                .expect("the strings of the table are UTF-8");
            strings.push(text);
            text_at += length;
        }
        Self {
            strings,
            templates: parts[2],
            jigsaws: parts[3],
            jar_sha1,
        }
    }

    /// How many templates the table has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.templates.len() / TEMPLATE_BYTES
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// How many jigsaw blocks all templates have together.
    #[must_use]
    pub fn jigsaw_count(&self) -> usize {
        self.jigsaws.len() / JIGSAW_BYTES
    }

    /// The template at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Template {
        let row = &self.templates[index * TEMPLATE_BYTES..(index + 1) * TEMPLATE_BYTES];
        Template {
            name: self.strings[u16_at(row, 0)],
            size: [i32::from(row[2]), i32::from(row[3]), i32::from(row[4])],
            first_jigsaw: u16_at(row, 5),
            jigsaws: usize::from(row[7]),
            exists: row[8] & 1 == 0,
        }
    }

    /// The place of the template called `name` (without its namespace).
    #[must_use]
    pub fn find(&self, name: &str) -> Option<usize> {
        let (mut low, mut high) = (0, self.len());
        while low < high {
            let middle = (low + high) / 2;
            match self.get(middle).name.cmp(name) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// The jigsaw blocks of a template, in the order of its file.
    pub fn jigsaws(&self, template: &Template) -> impl ExactSizeIterator<Item = JigsawBlock> + '_ {
        let first = template.first_jigsaw;
        (first..first + template.jigsaws).map(move |index| {
            let row = &self.jigsaws[index * JIGSAW_BYTES..(index + 1) * JIGSAW_BYTES];
            JigsawBlock {
                position: [i32::from(row[0]), i32::from(row[1]), i32::from(row[2])],
                orientation: Orientation::ALL[usize::from(row[3])].0,
                rollable: row[4] == 0,
                name: self.strings[u16_at(row, 5)],
                target: self.strings[u16_at(row, 7)],
                pool: self.strings[u16_at(row, 9)],
                selection_priority: i32::from(i16::from_le_bytes([row[11], row[12]])),
                placement_priority: i32::from(i16::from_le_bytes([row[13], row[14]])),
            }
        })
    }
}
