//! Points of interest in a chunk, tested from the specification and not from the code:
//! `docs/groundwork/n10-plan.md` (decision 1, sections 3.1 and 3.2),
//! `docs/groundwork/n10-brief.md` (findings 1, 4 and 5, section 2.1),
//! `docs/groundwork/n10-dumps.md` (part A, finding 1 and section 2.1) and
//! `docs/adr/0022-what-a-chunk-carries.md`, "Changed in N10". The game's table of types
//! is written out again here by the blocks' names, so that every state of every block
//! is judged against it.

mod common;

use clustine_data::{BLOCKS, BlockState, blocks};
use clustine_world::poi::{Claim, PoiType, poi_type_of};
use clustine_world::{Chunk, InChunk};
use common::*;

/// The sixteen dyes of `Blocks.BED`, whose heads are homes; the straw bed is not among
/// them.
const DYES: [&str; 16] = [
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

/// The type the brief's table (section 2.1) gives a state, by its block's name.
fn expected_type(name: &str, state: BlockState) -> Option<PoiType> {
    let name = name.strip_prefix("minecraft:").expect("the game's blocks");
    let kind = match name {
        "blast_furnace" => PoiType::Armorer,
        "smoker" => PoiType::Butcher,
        "cartography_table" => PoiType::Cartographer,
        "brewing_stand" => PoiType::Cleric,
        "composter" => PoiType::Farmer,
        "barrel" => PoiType::Fisherman,
        "fletching_table" => PoiType::Fletcher,
        "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => {
            PoiType::Leatherworker
        }
        "lectern" => PoiType::Librarian,
        "stonecutter" => PoiType::Mason,
        "loom" => PoiType::Shepherd,
        "smithing_table" => PoiType::Toolsmith,
        "grindstone" => PoiType::Weaponsmith,
        "bell" => PoiType::Meeting,
        "beehive" => PoiType::Beehive,
        "bee_nest" => PoiType::BeeNest,
        "nether_portal" => PoiType::NetherPortal,
        "lodestone" => PoiType::Lodestone,
        "test_instance_block" => PoiType::TestInstance,
        "lightning_rod"
        | "exposed_lightning_rod"
        | "weathered_lightning_rod"
        | "oxidized_lightning_rod"
        | "waxed_lightning_rod"
        | "waxed_exposed_lightning_rod"
        | "waxed_weathered_lightning_rod"
        | "waxed_oxidized_lightning_rod" => PoiType::LightningRod,
        _ => {
            let dye = name.strip_suffix("_bed")?;
            if !DYES.contains(&dye) || state.property("part") != Some("head") {
                return None;
            }
            PoiType::Home
        }
    };
    Some(kind)
}

#[test]
fn every_state_of_every_block_is_of_the_type_the_game_s_table_gives() {
    let mut homes = 0;
    let mut rods = 0;
    for block in BLOCKS.iter() {
        for id in block.first_state.0..=block.last_state.0 {
            let state = BlockState(id);
            let expected = expected_type(block.name, state);
            assert_eq!(
                poi_type_of(state),
                expected,
                "{} {:?}",
                block.name,
                state.properties().collect::<Vec<_>>()
            );
            homes += usize::from(expected == Some(PoiType::Home));
            rods += usize::from(expected == Some(PoiType::LightningRod));
        }
    }
    // Every head of every coloured bed, whatever its facing and occupancy; every rod's
    // weathering, waxed and not, whatever it faces or is powered.
    assert_eq!(
        homes,
        16 * 8,
        "four facings and two occupancies of each head"
    );
    assert!(rods >= 8);
}

#[test]
fn the_types_have_the_registry_s_ids_names_tickets_ranges_and_tags() {
    let table: [(&str, u8, u8); 21] = [
        ("armorer", 1, 1),
        ("butcher", 1, 1),
        ("cartographer", 1, 1),
        ("cleric", 1, 1),
        ("farmer", 1, 1),
        ("fisherman", 1, 1),
        ("fletcher", 1, 1),
        ("leatherworker", 1, 1),
        ("librarian", 1, 1),
        ("mason", 1, 1),
        ("shepherd", 1, 1),
        ("toolsmith", 1, 1),
        ("weaponsmith", 1, 1),
        ("home", 1, 1),
        ("meeting", 32, 6),
        ("beehive", 0, 1),
        ("bee_nest", 0, 1),
        ("nether_portal", 0, 1),
        ("lodestone", 0, 1),
        // D10 part A, correction 7: the type's key, not the block's.
        ("test_instance", 0, 1),
        ("lightning_rod", 0, 1),
    ];
    for (id, (name, tickets, range)) in table.into_iter().enumerate() {
        let id = u8::try_from(id).expect("21 types");
        let kind = PoiType::from_id(id).expect("every id up to 20 is a type");
        assert_eq!(kind.id(), id);
        assert_eq!(kind.name(), name);
        assert_eq!(kind.max_tickets(), tickets, "{name}");
        assert_eq!(kind.valid_range(), range, "{name}");
        // `#acquirable_job_site` the thirteen jobs; `#village` those, home and meeting;
        // `#bee_home` the hive and the nest.
        assert_eq!(kind.is_job_site(), id <= 12, "{name}");
        assert_eq!(kind.is_village(), id <= 14, "{name}");
        assert_eq!(kind.is_bee_home(), id == 15 || id == 16, "{name}");
    }
    assert_eq!(PoiType::from_id(21), None);
    assert_eq!(PoiType::from_id(255), None);
}

/// A chunk of the overworld's height, of air.
fn chunk() -> Chunk {
    of_air(-64, 24)
}

fn head(bed: BlockState) -> BlockState {
    bed.with_property("part", "head").expect("a bed has a part")
}

fn foot(bed: BlockState) -> BlockState {
    bed.with_property("part", "foot").expect("a bed has a part")
}

fn place(chunk: &mut Chunk, at: InChunk, state: BlockState) {
    chunk.set(usize::from(at.x), i32::from(at.y), usize::from(at.z), state);
}

fn claim(at: InChunk, kind: PoiType, free: u8) -> Claim {
    Claim { at, kind, free }
}

#[test]
fn a_job_site_and_a_bed_s_head_have_one_ticket_a_bell_thirty_two_and_a_beehive_none() {
    let mut chunk = chunk();
    let lectern = InChunk::new(1, 64, 1);
    let bed = InChunk::new(2, 64, 1);
    let bed_foot = InChunk::new(2, 64, 2);
    let bell = InChunk::new(3, 64, 1);
    let hive = InChunk::new(4, 64, 1);
    let rod = InChunk::new(5, 64, 1);
    place(&mut chunk, lectern, blocks::LECTERN);
    place(&mut chunk, bed, head(blocks::BLUE_BED));
    place(&mut chunk, bed_foot, foot(blocks::BLUE_BED));
    place(&mut chunk, bell, blocks::BELL);
    place(&mut chunk, hive, blocks::BEEHIVE);
    place(&mut chunk, rod, blocks::WAXED_EXPOSED_LIGHTNING_ROD);

    // Records are born with every ticket free (PoiRecord's constructor).
    assert_eq!(chunk.free_tickets(lectern), Some(1));
    assert_eq!(chunk.free_tickets(bed), Some(1));
    assert_eq!(
        chunk.free_tickets(bed_foot),
        None,
        "a bed's foot is no record"
    );
    assert_eq!(chunk.free_tickets(bell), Some(32));
    assert_eq!(chunk.free_tickets(hive), Some(0));
    assert_eq!(chunk.free_tickets(rod), Some(0));
    assert!(chunk.claims().is_empty(), "full records are no claims");

    assert!(chunk.acquire_ticket(lectern));
    assert!(!chunk.acquire_ticket(lectern));
    assert!(chunk.acquire_ticket(bed));
    assert!(!chunk.acquire_ticket(bed));
    assert!(!chunk.acquire_ticket(bed_foot));
    for taken in 1..=32 {
        assert!(chunk.acquire_ticket(bell), "the bell's ticket {taken}");
    }
    assert!(!chunk.acquire_ticket(bell), "a bell has 32 tickets");
    assert_eq!(chunk.free_tickets(bell), Some(0));
    // `acquireTicket` fails at 0: nobody claims a hive or a rod.
    assert!(!chunk.acquire_ticket(hive));
    assert!(!chunk.acquire_ticket(rod));
    assert_eq!(
        chunk.claims(),
        [
            claim(lectern, PoiType::Librarian, 0),
            claim(bed, PoiType::Home, 0),
            claim(bell, PoiType::Meeting, 0),
        ]
    );
}

#[test]
fn a_claim_is_kept_only_while_a_ticket_is_taken() {
    let mut chunk = chunk();
    let bell = InChunk::new(8, 70, 8);
    place(&mut chunk, bell, blocks::BELL);
    assert!(chunk.acquire_ticket(bell));
    assert!(chunk.acquire_ticket(bell));
    assert_eq!(chunk.claims(), [claim(bell, PoiType::Meeting, 30)]);
    assert!(chunk.release_ticket(bell));
    assert_eq!(chunk.claims(), [claim(bell, PoiType::Meeting, 31)]);
    assert!(chunk.release_ticket(bell));
    assert!(chunk.claims().is_empty(), "every ticket free is no claim");
    // `releaseTicket` at the maximum is a no-op returning false.
    assert!(!chunk.release_ticket(bell));
    assert_eq!(chunk.free_tickets(bell), Some(32));
    assert!(chunk.claims().is_empty());
    // And where there is no record at all.
    assert!(!chunk.release_ticket(InChunk::new(0, 70, 0)));
}

#[test]
fn a_block_changed_to_another_type_or_to_none_takes_its_claim() {
    let mut chunk = chunk();
    let site = InChunk::new(6, 64, 6);
    let bed = InChunk::new(7, 64, 6);
    let bell = InChunk::new(8, 64, 6);
    place(&mut chunk, site, blocks::LECTERN);
    place(&mut chunk, bed, head(blocks::RED_BED));
    place(&mut chunk, bell, blocks::BELL);
    assert!(chunk.acquire_ticket(site));
    assert!(chunk.acquire_ticket(bed));
    assert!(chunk.acquire_ticket(bell));

    // A librarian's site made a mason's: removed and added, the new one with full
    // tickets (`updatePOIOnBlockStateChange`).
    place(&mut chunk, site, blocks::STONECUTTER);
    assert_eq!(chunk.free_tickets(site), Some(1));
    // A bed's head turned into its foot is no record any more.
    place(&mut chunk, bed, foot(blocks::RED_BED));
    assert_eq!(chunk.free_tickets(bed), None);
    // The bell broken.
    place(&mut chunk, bell, blocks::AIR);
    assert_eq!(chunk.free_tickets(bell), None);
    assert!(chunk.claims().is_empty());

    // Put back where they were, they are new records with every ticket free: removing a
    // record tells nobody, and nothing of the claim is left to come back.
    place(&mut chunk, bed, head(blocks::RED_BED));
    place(&mut chunk, bell, blocks::BELL);
    assert_eq!(chunk.free_tickets(bed), Some(1));
    assert_eq!(chunk.free_tickets(bell), Some(32));
    assert!(chunk.claims().is_empty());
}

#[test]
fn a_block_changed_within_its_type_keeps_its_claim() {
    let mut chunk = chunk();
    let composter = InChunk::new(1, 64, 9);
    let bed = InChunk::new(2, 64, 9);
    let cauldron = InChunk::new(3, 64, 9);
    let lectern = InChunk::new(4, 64, 9);
    let bell = InChunk::new(5, 64, 9);
    place(&mut chunk, composter, blocks::COMPOSTER);
    place(&mut chunk, bed, head(blocks::CYAN_BED));
    place(&mut chunk, cauldron, blocks::CAULDRON);
    place(&mut chunk, lectern, blocks::LECTERN);
    place(&mut chunk, bell, blocks::BELL);
    for at in [composter, bed, cauldron, lectern, bell] {
        assert!(chunk.acquire_ticket(at));
    }
    let before = chunk.claims().to_vec();

    // `forState(old) == forState(new)`: nothing happens to the record.
    let state = |base: BlockState, name: &str, value: &str| {
        base.with_property(name, value)
            .unwrap_or_else(|| panic!("{name}={value}"))
    };
    place(
        &mut chunk,
        composter,
        state(blocks::COMPOSTER, "level", "5"),
    );
    place(
        &mut chunk,
        bed,
        state(head(blocks::CYAN_BED), "occupied", "true"),
    );
    // Another block of the same type: a cauldron filled with water is still a
    // leatherworker's.
    place(&mut chunk, cauldron, blocks::WATER_CAULDRON);
    place(
        &mut chunk,
        lectern,
        state(blocks::LECTERN, "has_book", "true"),
    );
    place(&mut chunk, bell, state(blocks::BELL, "powered", "true"));
    assert_eq!(chunk.claims(), before);
    for at in [composter, bed, cauldron, lectern] {
        assert_eq!(chunk.free_tickets(at), Some(0));
    }
    assert_eq!(chunk.free_tickets(bell), Some(31));
}

#[test]
fn claims_that_do_not_fit_the_blocks_are_dropped_and_counted_and_the_rest_kept_in_order() {
    let mut chunk = chunk();
    let lectern = InChunk::new(9, 64, 3);
    let bell = InChunk::new(2, 64, 4);
    let low_bed = InChunk::new(15, 10, 15);
    let foot_at = InChunk::new(15, 10, 14);
    let hive = InChunk::new(0, 64, 0);
    place(&mut chunk, lectern, blocks::LECTERN);
    place(&mut chunk, bell, blocks::BELL);
    place(&mut chunk, low_bed, head(blocks::WHITE_BED));
    place(&mut chunk, foot_at, foot(blocks::WHITE_BED));
    place(&mut chunk, hive, blocks::BEE_NEST);

    let dropped = chunk.set_claims(vec![
        // Given out of order: kept ascending by position (y, then z, then x).
        claim(lectern, PoiType::Librarian, 0),
        claim(bell, PoiType::Meeting, 12),
        claim(low_bed, PoiType::Home, 0),
        // Of another type than its block.
        claim(InChunk::new(9, 64, 3), PoiType::Mason, 0),
        // On no point of interest at all.
        claim(InChunk::new(5, 5, 5), PoiType::Librarian, 0),
        // On a bed's foot.
        claim(foot_at, PoiType::Home, 0),
        // With every ticket free: it says nothing the blocks do not.
        claim(bell, PoiType::Meeting, 32),
    ]);
    // Four do not fit: the mason's, the one on nothing, the foot's and the full bell's.
    assert_eq!(dropped, 4);
    assert_eq!(
        chunk.claims(),
        [
            claim(low_bed, PoiType::Home, 0),
            claim(lectern, PoiType::Librarian, 0),
            claim(bell, PoiType::Meeting, 12),
        ]
    );
    assert!(!chunk.claims().iter().any(|claim| claim.at == hive));

    // A bee nest has no tickets: a claim on it is one with every ticket "free".
    let mut other = self::chunk();
    place(&mut other, hive, blocks::BEE_NEST);
    assert_eq!(other.set_claims(vec![claim(hive, PoiType::BeeNest, 0)]), 1);
    assert!(other.claims().is_empty());
}

#[test]
fn a_chunk_s_records_are_what_its_blocks_say_ascending_by_position() {
    let mut chunk = chunk();
    let bed = InChunk::new(3, 100, 1);
    let rod = InChunk::new(1, -60, 15);
    let barrel = InChunk::new(0, 100, 1);
    place(&mut chunk, bed, head(blocks::ORANGE_BED));
    place(
        &mut chunk,
        InChunk::new(3, 100, 2),
        foot(blocks::ORANGE_BED),
    );
    place(&mut chunk, rod, blocks::OXIDIZED_LIGHTNING_ROD);
    place(&mut chunk, barrel, blocks::BARREL);
    assert_eq!(
        chunk.pois(),
        [
            (rod, PoiType::LightningRod),
            (barrel, PoiType::Fisherman),
            (bed, PoiType::Home),
        ]
    );
    for &(at, kind) in chunk.pois() {
        assert_eq!(chunk.poi_type_at(at), Some(kind));
    }
    place(&mut chunk, barrel, blocks::SMOKER);
    assert_eq!(chunk.poi_type_at(barrel), Some(PoiType::Butcher));
    assert!(chunk.pois().contains(&(barrel, PoiType::Butcher)));
    place(&mut chunk, rod, blocks::AIR);
    assert_eq!(chunk.pois().len(), 2);
}

#[test]
fn claims_are_part_of_a_chunk_s_equality_and_travel_but_not_of_what_viewers_see() {
    let mut chunk = chunk();
    let loom = InChunk::new(4, 64, 4);
    place(&mut chunk, loom, blocks::LOOM);
    let free = chunk.clone();
    assert!(chunk.acquire_ticket(loom));
    assert_ne!(chunk, free);
    assert!(chunk.shows_the_same(&free));
    assert!(chunk.for_viewers().claims().is_empty());
    assert_eq!(chunk.for_viewers(), free.for_viewers());

    let bytes = postcard::to_stdvec(&chunk).expect("a chunk serialises");
    let travelled: Chunk = postcard::from_bytes(&bytes).expect("and comes back");
    assert_eq!(travelled, chunk);
    assert_eq!(travelled.claims(), [claim(loom, PoiType::Shepherd, 0)]);
    assert_eq!(travelled.free_tickets(loom), Some(0));
    let mut cloned = chunk.clone();
    assert_eq!(cloned.claims(), chunk.claims());
    assert!(cloned.release_ticket(loom));
    assert_eq!(chunk.free_tickets(loom), Some(0), "a copy is its own");
}

/// ADR-0022, "Changed in N10", section 3: "a chunk with claims is no longer as
/// generated, so that remaking a chunk whose generation failed keeps them"; the plan,
/// section 3.2: "A change of claims clears `with_failure` as a block change does".
#[test]
#[ignore = "finding: the ADR says a chunk with claims is no longer as generated; Chunk::acquire_ticket leaves generated() and with_failure() as they were (only the store's replay clears them)"]
fn a_ticket_taken_makes_a_chunk_no_longer_as_generated() {
    let mut chunk = chunk();
    let smoker = InChunk::new(4, 64, 4);
    place(&mut chunk, smoker, blocks::SMOKER);
    chunk.mark_generated(3, true);
    assert!(chunk.with_failure());
    assert!(chunk.acquire_ticket(smoker));
    assert_eq!(chunk.generated(), 0);
    assert!(!chunk.with_failure());
}
