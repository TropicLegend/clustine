// The names and shapes here are those of SteelMC's registry of structures
// (steel-registry/src/structure/mod.rs at commit
// 885c4b3e60ed79862c37311780774f76806cb714), over Clustine's emitted data, so that the
// structure code adapted from SteelMC reads its data as it did there. See NOTICE in
// this crate.

//! What a structure's code is given of the structure it makes: the emitted record of
//! `clustine-worldgen-data` under the names the adapted code uses.

use clustine_world::Biome;
use clustine_worldgen_data::structure::{HeightProvider, Jigsaw, Kind, PortalSetup, Structure};
use clustine_worldgen_data::structures::STRUCTURES;

use crate::geometry::Identifier;

pub use clustine_worldgen_data::material::Anchor as VerticalAnchorData;
pub use clustine_worldgen_data::structure::{
    LiquidSettings as LiquidSettingsData, PortalPlacement as RuinedPortalPlacementData,
    PortalSetup as RuinedPortalSetupData, Projection, TerrainAdaptation as TerrainAdjustment,
};

/// The name of a biome by Clustine's number.
#[must_use]
pub fn biome_name(biome: Biome) -> Option<&'static str> {
    clustine_data::synced_registry("minecraft:worldgen/biome")
        .and_then(|registry| registry.entries.get(usize::from(biome.0)).copied())
}

/// What is known of a structure template: its size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TemplateData {
    pub size: [i32; 3],
}

/// The table of templates, asked by name.
#[derive(Clone, Copy, Debug)]
pub struct TemplateLookup;

impl TemplateLookup {
    /// The template of a name, if the jar has it.
    #[must_use]
    pub fn get(&self, name: &Identifier) -> Option<TemplateData> {
        let table = clustine_worldgen_data::templates();
        let template = table.get(table.find(name.path)?);
        (name.namespace == "minecraft" && template.exists).then_some(TemplateData {
            size: template.size,
        })
    }
}

/// A biome as a structure's code is given it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BiomeRef {
    /// Clustine's number of the biome.
    pub key: Biome,
}

/// The biomes a structure may start in.
#[derive(Clone, Copy, Debug)]
pub struct BiomeSet(pub &'static [u8]);

impl BiomeSet {
    #[must_use]
    pub fn contains(&self, biome: &Biome) -> bool {
        u8::try_from(biome.0).is_ok_and(|number| self.0.binary_search(&number).is_ok())
    }
}

/// Which wood a mineshaft is built of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MineshaftTypeData {
    Normal,
    Mesa,
}

/// Which stone an ocean ruin is built of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OceanRuinBiomeTempData {
    Warm,
    Cold,
}

/// A provider of heights (`HeightProvider`).
pub type HeightProviderData = HeightProvider;

/// What the data says of a structure beyond what all have, by its kind.
#[derive(Clone, Copy, Debug)]
pub enum StructureConfigData {
    Jigsaw(&'static Jigsaw),
    Mineshaft {
        mineshaft_type: MineshaftTypeData,
    },
    Shipwreck {
        is_beached: bool,
    },
    OceanRuin {
        biome_temp: OceanRuinBiomeTempData,
        large_probability: f32,
        cluster_probability: f32,
    },
    RuinedPortal {
        setups: &'static [PortalSetup],
    },
    NetherFossil {
        height: HeightProviderData,
    },
    Empty,
}

/// A structure of the game's registry.
#[derive(Clone, Copy, Debug)]
pub struct StructureData {
    /// Its place in `clustine_worldgen_data::structures::STRUCTURES`, which is its
    /// number in the game's registry.
    pub index: usize,
    pub key: Identifier,
    pub allowed_biomes: BiomeSet,
    pub terrain_adjustment: TerrainAdjustment,
    pub config: StructureConfigData,
    /// The emitted record itself.
    pub data: &'static Structure,
}

impl StructureData {
    /// The structure at a place of the registry.
    #[must_use]
    pub fn of(index: usize) -> Self {
        let data = &STRUCTURES[index];
        let config = match &data.kind {
            Kind::Jigsaw(jigsaw) => StructureConfigData::Jigsaw(jigsaw),
            Kind::Mineshaft { mesa } => StructureConfigData::Mineshaft {
                mineshaft_type: if *mesa {
                    MineshaftTypeData::Mesa
                } else {
                    MineshaftTypeData::Normal
                },
            },
            Kind::Shipwreck { beached } => StructureConfigData::Shipwreck {
                is_beached: *beached,
            },
            Kind::OceanRuin {
                warm,
                large_probability,
                cluster_probability,
            } => StructureConfigData::OceanRuin {
                biome_temp: if *warm {
                    OceanRuinBiomeTempData::Warm
                } else {
                    OceanRuinBiomeTempData::Cold
                },
                large_probability: *large_probability,
                cluster_probability: *cluster_probability,
            },
            Kind::RuinedPortal { setups } => StructureConfigData::RuinedPortal { setups },
            Kind::NetherFossil { height } => StructureConfigData::NetherFossil { height: *height },
            _ => StructureConfigData::Empty,
        };
        Self {
            index,
            key: Identifier::of(data.name),
            allowed_biomes: BiomeSet(data.biomes),
            terrain_adjustment: data.terrain_adaptation,
            config,
            data,
        }
    }

    /// How far a start's box is grown beyond its pieces (`Structure.adjustBoundingBox`).
    #[must_use]
    pub fn bb_inflate(&self) -> i32 {
        if self.terrain_adjustment == TerrainAdjustment::None {
            0
        } else {
            12
        }
    }
}
