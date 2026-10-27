//! Points of interest (`PoiTypes`, `PoiRecord`; `docs/groundwork/n10-plan.md`, section
//! 3): which block states are of which type, and the claims a chunk keeps of them.
//!
//! A point of interest is not kept: it is wherever a block of its type is, so that a
//! chunk's records follow its blocks without a write of their own. What is kept is how
//! many of a record's tickets are free, and only where that is below the most it has (a
//! claim), since a full record is what the blocks alone say.

use clustine_data::{BlockState, blocks};
use serde::{Deserialize, Serialize};

use crate::InChunk;

/// A type of point of interest, numbered as the game's registry numbers them
/// (`PoiTypes`' registration order, `docs/groundwork/n10-dumps.md`, part A, section 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum PoiType {
    Armorer = 0,
    Butcher = 1,
    Cartographer = 2,
    Cleric = 3,
    Farmer = 4,
    Fisherman = 5,
    Fletcher = 6,
    Leatherworker = 7,
    Librarian = 8,
    Mason = 9,
    Shepherd = 10,
    Toolsmith = 11,
    Weaponsmith = 12,
    Home = 13,
    Meeting = 14,
    Beehive = 15,
    BeeNest = 16,
    NetherPortal = 17,
    Lodestone = 18,
    TestInstance = 19,
    LightningRod = 20,
}

impl PoiType {
    /// Every type, by id.
    pub const ALL: [PoiType; 21] = [
        PoiType::Armorer,
        PoiType::Butcher,
        PoiType::Cartographer,
        PoiType::Cleric,
        PoiType::Farmer,
        PoiType::Fisherman,
        PoiType::Fletcher,
        PoiType::Leatherworker,
        PoiType::Librarian,
        PoiType::Mason,
        PoiType::Shepherd,
        PoiType::Toolsmith,
        PoiType::Weaponsmith,
        PoiType::Home,
        PoiType::Meeting,
        PoiType::Beehive,
        PoiType::BeeNest,
        PoiType::NetherPortal,
        PoiType::Lodestone,
        PoiType::TestInstance,
        PoiType::LightningRod,
    ];

    pub fn id(self) -> u8 {
        self as u8
    }

    pub fn from_id(id: u8) -> Option<Self> {
        Self::ALL.get(usize::from(id)).copied()
    }

    /// The registry's name, without its namespace.
    pub fn name(self) -> &'static str {
        match self {
            PoiType::Armorer => "armorer",
            PoiType::Butcher => "butcher",
            PoiType::Cartographer => "cartographer",
            PoiType::Cleric => "cleric",
            PoiType::Farmer => "farmer",
            PoiType::Fisherman => "fisherman",
            PoiType::Fletcher => "fletcher",
            PoiType::Leatherworker => "leatherworker",
            PoiType::Librarian => "librarian",
            PoiType::Mason => "mason",
            PoiType::Shepherd => "shepherd",
            PoiType::Toolsmith => "toolsmith",
            PoiType::Weaponsmith => "weaponsmith",
            PoiType::Home => "home",
            PoiType::Meeting => "meeting",
            PoiType::Beehive => "beehive",
            PoiType::BeeNest => "bee_nest",
            PoiType::NetherPortal => "nether_portal",
            PoiType::Lodestone => "lodestone",
            PoiType::TestInstance => "test_instance",
            PoiType::LightningRod => "lightning_rod",
        }
    }

    /// How many may hold it at once (`PoiType.maxTickets`): one for a job site and a
    /// bed, 32 for the bell, none for the types nobody claims.
    pub fn max_tickets(self) -> u8 {
        match self {
            PoiType::Meeting => 32,
            PoiType::Beehive
            | PoiType::BeeNest
            | PoiType::NetherPortal
            | PoiType::Lodestone
            | PoiType::TestInstance
            | PoiType::LightningRod => 0,
            _ => 1,
        }
    }

    /// How close a path has to come to it (`PoiType.validRange`).
    pub fn valid_range(self) -> u8 {
        match self {
            PoiType::Meeting => 6,
            _ => 1,
        }
    }

    /// One of the thirteen job sites (`#acquirable_job_site`).
    pub fn is_job_site(self) -> bool {
        self.id() <= PoiType::Weaponsmith.id()
    }

    /// Of `#village`: a job site, a bed or the bell.
    pub fn is_village(self) -> bool {
        self.id() <= PoiType::Meeting.id()
    }

    /// The job site a profession, by its registry id, holds (`VillagerProfession.heldJobSite`):
    /// each of the thirteen working professions its own; none for none and the nitwit.
    pub fn held_by(profession: i32) -> Option<Self> {
        use clustine_data::villagers::professions;
        let index = match profession {
            professions::ARMORER..=professions::MASON => profession - professions::ARMORER,
            professions::SHEPHERD..=professions::WEAPONSMITH => {
                profession - professions::SHEPHERD + Self::Shepherd as i32
            }
            _ => return None,
        };
        Self::from_id(u8::try_from(index).ok()?)
    }

    /// Whether a villager of `profession`, by its registry id, may take a job site of
    /// this type (`VillagerProfession.acquirableJobSite`): none takes any job site
    /// (`ALL_ACQUIRABLE_JOBS`), the nitwit none (`PoiType.NONE`), and every other its
    /// own.
    pub fn acquirable_by(self, profession: i32) -> bool {
        if profession == clustine_data::villagers::professions::NONE {
            self.is_job_site()
        } else {
            Self::held_by(profession) == Some(self)
        }
    }

    /// Of `#bee_home`.
    pub fn is_bee_home(self) -> bool {
        matches!(self, PoiType::Beehive | PoiType::BeeNest)
    }
}

/// The type of point of interest a block in `state` is (`PoiTypes.forState`), if any.
/// Only the head of the sixteen coloured beds is a bed's point (the straw bed is none).
pub fn poi_type_of(state: BlockState) -> Option<PoiType> {
    let block = state.block_id()?;
    let of = |other: BlockState| other.block_id() == Some(block);
    let kind = if of(blocks::BLAST_FURNACE) {
        PoiType::Armorer
    } else if of(blocks::SMOKER) {
        PoiType::Butcher
    } else if of(blocks::CARTOGRAPHY_TABLE) {
        PoiType::Cartographer
    } else if of(blocks::BREWING_STAND) {
        PoiType::Cleric
    } else if of(blocks::COMPOSTER) {
        PoiType::Farmer
    } else if of(blocks::BARREL) {
        PoiType::Fisherman
    } else if of(blocks::FLETCHING_TABLE) {
        PoiType::Fletcher
    } else if of(blocks::CAULDRON)
        || of(blocks::WATER_CAULDRON)
        || of(blocks::LAVA_CAULDRON)
        || of(blocks::POWDER_SNOW_CAULDRON)
    {
        PoiType::Leatherworker
    } else if of(blocks::LECTERN) {
        PoiType::Librarian
    } else if of(blocks::STONECUTTER) {
        PoiType::Mason
    } else if of(blocks::LOOM) {
        PoiType::Shepherd
    } else if of(blocks::SMITHING_TABLE) {
        PoiType::Toolsmith
    } else if of(blocks::GRINDSTONE) {
        PoiType::Weaponsmith
    } else if COLOURED_BEDS.iter().any(|&bed| of(bed)) {
        if state.property("part") != Some("head") {
            return None;
        }
        PoiType::Home
    } else if of(blocks::BELL) {
        PoiType::Meeting
    } else if of(blocks::BEEHIVE) {
        PoiType::Beehive
    } else if of(blocks::BEE_NEST) {
        PoiType::BeeNest
    } else if of(blocks::NETHER_PORTAL) {
        PoiType::NetherPortal
    } else if of(blocks::LODESTONE) {
        PoiType::Lodestone
    } else if of(blocks::TEST_INSTANCE_BLOCK) {
        PoiType::TestInstance
    } else if LIGHTNING_RODS.iter().any(|&rod| of(rod)) {
        PoiType::LightningRod
    } else {
        return None;
    };
    Some(kind)
}

/// `BedBlock`s of the sixteen dyes (`PoiTypes.BEDS`), which the straw bed is not.
const COLOURED_BEDS: [BlockState; 16] = [
    blocks::WHITE_BED,
    blocks::ORANGE_BED,
    blocks::MAGENTA_BED,
    blocks::LIGHT_BLUE_BED,
    blocks::YELLOW_BED,
    blocks::LIME_BED,
    blocks::PINK_BED,
    blocks::GRAY_BED,
    blocks::LIGHT_GRAY_BED,
    blocks::CYAN_BED,
    blocks::PURPLE_BED,
    blocks::BLUE_BED,
    blocks::BROWN_BED,
    blocks::GREEN_BED,
    blocks::RED_BED,
    blocks::BLACK_BED,
];

/// `Blocks.LIGHTNING_ROD.asList()`: the four stages of weathering, waxed and not.
const LIGHTNING_RODS: [BlockState; 8] = [
    blocks::LIGHTNING_ROD,
    blocks::EXPOSED_LIGHTNING_ROD,
    blocks::WEATHERED_LIGHTNING_ROD,
    blocks::OXIDIZED_LIGHTNING_ROD,
    blocks::WAXED_LIGHTNING_ROD,
    blocks::WAXED_EXPOSED_LIGHTNING_ROD,
    blocks::WAXED_WEATHERED_LIGHTNING_ROD,
    blocks::WAXED_OXIDIZED_LIGHTNING_ROD,
];

/// A record of the chunk with fewer free tickets than it has at most: who holds them is
/// in the holders' memories, not here (`PoiRecord`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub at: InChunk,
    pub kind: PoiType,
    /// Below `kind.max_tickets()`.
    pub free: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_numbered_as_the_registry_numbers_them() {
        for (id, kind) in PoiType::ALL.iter().enumerate() {
            assert_eq!(usize::from(kind.id()), id);
            assert_eq!(PoiType::from_id(kind.id()), Some(*kind));
        }
        assert_eq!(PoiType::from_id(21), None);
        assert_eq!(PoiType::Beehive.id(), 15);
        assert_eq!(PoiType::BeeNest.id(), 16);
        assert_eq!(PoiType::TestInstance.name(), "test_instance");
    }

    #[test]
    fn only_a_coloured_bed_s_head_is_a_home() {
        let head = blocks::RED_BED
            .with_property("part", "head")
            .expect("a bed has a part");
        let foot = blocks::RED_BED
            .with_property("part", "foot")
            .expect("a bed has a part");
        assert_eq!(poi_type_of(head), Some(PoiType::Home));
        assert_eq!(poi_type_of(foot), None);
        let straw = blocks::STRAW_BED
            .with_property("part", "head")
            .expect("a bed has a part");
        assert_eq!(poi_type_of(straw), None);
    }

    #[test]
    fn job_sites_bells_and_others_have_their_types() {
        assert_eq!(poi_type_of(blocks::LECTERN), Some(PoiType::Librarian));
        assert_eq!(
            poi_type_of(blocks::WATER_CAULDRON),
            Some(PoiType::Leatherworker)
        );
        assert_eq!(poi_type_of(blocks::BELL), Some(PoiType::Meeting));
        assert_eq!(
            poi_type_of(blocks::WAXED_OXIDIZED_LIGHTNING_ROD),
            Some(PoiType::LightningRod)
        );
        assert_eq!(poi_type_of(blocks::STONE), None);
        assert_eq!(poi_type_of(blocks::AIR), None);
    }

    #[test]
    fn each_working_profession_holds_its_own_job_site() {
        use clustine_data::villagers::professions;
        assert_eq!(
            PoiType::held_by(professions::ARMORER),
            Some(PoiType::Armorer)
        );
        assert_eq!(
            PoiType::held_by(professions::LIBRARIAN),
            Some(PoiType::Librarian)
        );
        assert_eq!(PoiType::held_by(professions::MASON), Some(PoiType::Mason));
        assert_eq!(
            PoiType::held_by(professions::SHEPHERD),
            Some(PoiType::Shepherd)
        );
        assert_eq!(
            PoiType::held_by(professions::WEAPONSMITH),
            Some(PoiType::Weaponsmith)
        );
        assert_eq!(PoiType::held_by(professions::NONE), None);
        assert_eq!(PoiType::held_by(professions::NITWIT), None);
    }

    #[test]
    fn tickets_and_tags_are_the_game_s() {
        assert_eq!(PoiType::Meeting.max_tickets(), 32);
        assert_eq!(PoiType::Meeting.valid_range(), 6);
        assert_eq!(PoiType::Home.max_tickets(), 1);
        assert_eq!(PoiType::BeeNest.max_tickets(), 0);
        assert!(PoiType::Weaponsmith.is_job_site());
        assert!(!PoiType::Home.is_job_site());
        assert!(PoiType::Meeting.is_village());
        assert!(!PoiType::Beehive.is_village());
    }
}
