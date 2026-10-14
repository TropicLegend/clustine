//! A start of every kind of structure, as the game's own classes made it: values that
//! `crates/clustine-terrain/tests/java/GameStructures.java` wrote for seed 13579 and
//! that `tests/game_structures.rs` found equal, copied here so that every run of the
//! tests holds the crate to them. They are Mojang's data, computed by its code, and
//! not under the licence of the code around them (NOTICE.md at the root).

use clustine_structures::geometry::BoundingBox;
use clustine_structures::{Start, Structures};
use clustine_terrain::biome::Dimension;
use clustine_worldgen_data::structures::{STRUCTURE_SETS, STRUCTURES};

const SEED: i64 = 13579;

/// The dimension, the structure, the chunk it starts in, how many pieces it has, the
/// start's box, and a hash over every piece's type and box (FNV-1a, 64 bits, over the
/// text `<type> <six numbers>;` of each piece in order).
type Known = (&'static str, &'static str, i32, i32, usize, [i32; 6], u64);

#[rustfmt::skip]
const KNOWN: [Known; 24] = [
    ("overworld", "minecraft:village_plains", 58, -27, 146, [872, 50, -520, 992, 104, -365], 0xe0670dfa716c3c1d),
    ("overworld", "minecraft:village_desert", -65, 19, 60, [-1103, 50, 252, -1023, 105, 372], 0x5f3c9c65dbb4ab22),
    ("overworld", "minecraft:ancient_city", 49, 6, 90, [661, -64, -25, 903, -10, 220], 0x9e99a06fbef7d96a),
    ("overworld", "minecraft:trial_chambers", -55, -63, 228, [-944, -62, -1100, -820, 31, -972], 0xdf06726b01cd3c43),
    ("overworld", "minecraft:trail_ruins", -57, -12, 18, [-935, 44, -214, -886, 88, -139], 0x1dace7b945109303),
    ("overworld", "minecraft:pillager_outpost", -138, 83, 13, [-2251, 55, 1285, -2180, 109, 1356], 0x3ac5ea338b182265),
    ("overworld", "minecraft:stronghold", -64, -64, 211, [-1133, -60, -1093, -970, -6, -932], 0x7e92c51db4b243d2),
    ("overworld", "minecraft:mineshaft", -72, 22, 182, [-1212, 30, 289, -1070, 52, 441], 0x6bedd97bb00e1f34),
    ("overworld", "minecraft:mansion", -117, -152, 630, [-1932, 65, -2444, -1854, 95, -2384], 0xff4c762ca32f2bf2),
    ("overworld", "minecraft:monument", 106, -114, 1, [1667, 39, -1853, 1724, 61, -1796], 0xd1a941502fa4535a),
    ("overworld", "minecraft:desert_pyramid", -30, 68, 1, [-480, 64, 1088, -460, 78, 1108], 0xd05448729a4407b0),
    ("overworld", "minecraft:jungle_pyramid", -46, -10, 1, [-736, 64, -160, -725, 73, -146], 0xbe9c6b3fc0e3dea2),
    ("overworld", "minecraft:swamp_hut", 54, 364, 1, [864, 64, 5824, 872, 70, 5830], 0x9021ddf444df3f24),
    ("overworld", "minecraft:igloo", -154, -154, 10, [-2464, 60, -2461, -2458, 94, -2452], 0x2bf93a24eeafbda1),
    ("overworld", "minecraft:shipwreck", -72, -37, 1, [-1152, 90, -583, -1144, 98, -562], 0xc0e8297fe1b1f449),
    ("overworld", "minecraft:ocean_ruin_cold", 86, -118, 3, [1371, 90, -1894, 1376, 96, -1888], 0x85f282ac281c7a22),
    ("overworld", "minecraft:ruined_portal", -67, -69, 1, [-1074, -42, -1111, -1059, -27, -1101], 0x403c492efe5acad4),
    ("overworld", "minecraft:buried_treasure", -14, -41, 1, [-215, 90, -647, -215, 90, -647], 0x63c73a3adc5a62c9),
    ("overworld", "minecraft:abandoned_camp_forest", 48, -48, 2, [749, 53, -788, 790, 84, -755], 0x22911cebf15687d8),
    ("nether", "minecraft:fortress", -32, -15, 56, [-631, 48, -310, -395, 69, -173], 0xb0f7b052c9c4ff93),
    ("nether", "minecraft:bastion_remnant", -39, 11, 48, [-655, 31, 160, -600, 95, 223], 0x722790570f699c63),
    ("nether", "minecraft:nether_fossil", -40, -38, 1, [-643, 29, -616, -615, 56, -589], 0xdc083e9da338b8cf),
    ("nether", "minecraft:ruined_portal_nether", -18, -25, 1, [-287, 29, -400, -278, 36, -392], 0x8a40e2ed06466ae7),
    ("end", "minecraft:end_city", 144, 146, 29, [2206, 65, 2331, 2314, 131, 2347], 0x1a6f30e13bb90ef5),
];

fn six(bounds: &BoundingBox) -> [i32; 6] {
    [
        bounds.min_x(),
        bounds.min_y(),
        bounds.min_z(),
        bounds.max_x(),
        bounds.max_y(),
        bounds.max_z(),
    ]
}

fn hash(start: &Start) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for piece in &start.pieces {
        let [a, b, c, d, e, f] = six(&piece.bounding_box);
        for byte in format!("{} {a} {b} {c} {d} {e} {f};", piece.piece_type).bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn a_start_of_every_kind_is_the_games() {
    for (name, dimension) in [
        ("overworld", Dimension::Overworld),
        ("nether", Dimension::Nether),
        ("end", Dimension::End),
    ] {
        let structures = Structures::new(SEED, dimension);
        for (of, structure, x, z, pieces, bounds, pieces_hash) in KNOWN {
            if of != name {
                continue;
            }
            let starts = structures.starts(x, z);
            let start = starts
                .iter()
                .find(|start| start.data().name == structure)
                .unwrap_or_else(|| panic!("{structure} does not start in chunk {x} {z}"));
            assert_eq!(start.pieces.len(), pieces, "{structure}");
            assert_eq!(six(&start.bounding_box), bounds, "{structure}");
            assert_eq!(hash(start), pieces_hash, "{structure}");
        }
    }
}

#[test]
fn a_dimension_has_the_sets_whose_structures_can_stand_in_its_biomes() {
    let names = |dimension: Dimension| -> Vec<&'static str> {
        Structures::new(SEED, dimension)
            .possible_sets()
            .iter()
            .map(|set| STRUCTURE_SETS[*set].name.trim_start_matches("minecraft:"))
            .collect()
    };
    assert_eq!(
        names(Dimension::Nether),
        ["nether_complexes", "nether_fossils", "ruined_portals"]
    );
    assert_eq!(names(Dimension::End), ["end_cities"]);
    let overworld = names(Dimension::Overworld);
    assert_eq!(overworld.len(), 18);
    assert!(overworld.contains(&"strongholds") && !overworld.contains(&"end_cities"));
}

#[test]
fn the_strongholds_stand_where_the_game_puts_them() {
    let structures = Structures::new(SEED, Dimension::Overworld);
    let set = STRUCTURE_SETS
        .iter()
        .position(|set| set.name == "minecraft:strongholds")
        .unwrap();
    let rings = &structures.rings()[set];
    assert_eq!(rings.len(), 128);
    assert_eq!(
        rings[..6],
        [
            (-29, 88),
            (-64, -64),
            (94, -29),
            (93, 273),
            (-216, 239),
            (-300, -73)
        ]
    );
    // One whose place is decided by a biome that is exactly tied with another: the
    // game's answer there is what its walk over the cells found before.
    assert_eq!(rings[46], (-776, 435));
    assert_eq!(rings[127], (-1081, 997));
    assert!(structures.is_structure_chunk(set, -29, 88));
    assert!(!structures.is_structure_chunk(set, -29, 89));
}

#[test]
fn a_chunk_refers_to_the_starts_whose_boxes_reach_it() {
    let structures = Structures::new(SEED, Dimension::Overworld);
    let village = STRUCTURES
        .iter()
        .position(|structure| structure.name == "minecraft:village_plains")
        .unwrap();
    let starts_of = |x: i32, z: i32| structures.starts(x, z);
    let at_the_start = Structures::references(58, -27, starts_of);
    assert!(at_the_start.contains(&(village, (58, -27))));
    // The village's box reaches from x 872 to 992: chunk 62 holds 992 and chunk 63 not.
    let reached = Structures::references(62, -27, starts_of);
    assert!(reached.contains(&(village, (58, -27))));
    let beyond = Structures::references(63, -27, starts_of);
    assert!(!beyond.contains(&(village, (58, -27))));
    // The village bends the ground under its houses and not beyond its reach.
    assert!(!Structures::beardifier(58, -27, starts_of).is_empty());
    assert!(
        Structures::beardifier(66, -27, starts_of)
            .junctions()
            .is_empty()
    );
}

#[test]
fn a_set_of_longs_is_walked_in_fastutils_order() {
    use clustine_structures::long_set_order;
    assert_eq!(long_set_order(&[]), Vec::<i64>::new());
    assert_eq!(long_set_order(&[7, 7, 7]), [7]);
    // Zero is kept apart and comes first.
    assert_eq!(long_set_order(&[5, 0, 9])[0], 0);
    let many: Vec<i64> = (1..=40).collect();
    let mut walked = long_set_order(&many);
    assert_eq!(walked.len(), 40);
    assert_ne!(walked, many);
    walked.sort_unstable();
    assert_eq!(walked, many);
}

/// What structures cost, printed: making the dimension's structures with the rings
/// of the strongholds, the starts of the chunks of an area, the references and the
/// beardifier of each, and the fill of the chunks whose ground is bent beside the
/// fill without. Judges nothing.
#[test]
#[ignore = "measures and prints; run it in a release build"]
fn what_starts_references_and_bending_cost() {
    use std::collections::BTreeMap;
    use std::time::Instant;

    use clustine_terrain::noise_chunk::Scratch;

    for (name, dimension, side) in [
        ("overworld", Dimension::Overworld, 48),
        ("nether", Dimension::Nether, 32),
        ("end", Dimension::End, 32),
    ] {
        let began = Instant::now();
        let structures = Structures::new(SEED, dimension);
        let made = began.elapsed();
        let began = Instant::now();
        let rings: usize = structures.rings().iter().map(Vec::len).sum();
        let ringed = began.elapsed();

        // The End's cities stand far out.
        let offset = if dimension == Dimension::End { 100 } else { 0 };
        let mut starts: BTreeMap<(i32, i32), Vec<Start>> = BTreeMap::new();
        let began = Instant::now();
        let mut slowest = std::time::Duration::ZERO;
        for x in offset - side - 8..offset + side + 8 {
            for z in offset - side - 8..offset + side + 8 {
                let one = Instant::now();
                starts.insert((x, z), structures.starts(x, z));
                slowest = slowest.max(one.elapsed());
            }
        }
        let started = began.elapsed();
        let count: usize = starts.values().map(Vec::len).sum();
        let pieces: usize = starts
            .values()
            .flatten()
            .map(|start| start.pieces.len())
            .sum();

        let began = Instant::now();
        let mut bent = Vec::new();
        for x in offset - side..offset + side {
            for z in offset - side..offset + side {
                let beard = Structures::beardifier(x, z, |x, z| starts[&(x, z)].as_slice());
                if !beard.is_empty() {
                    bent.push((x, z, beard));
                }
            }
        }
        let bearded = began.elapsed();
        let chunks = (2 * side * 2 * side) as u32;

        let mut scratch = Scratch::default();
        let began = Instant::now();
        for (x, z, _) in &bent {
            let _ = structures.terrain().fill_with_density(*x, *z, &mut scratch);
        }
        let plain = began.elapsed();
        let began = Instant::now();
        for (x, z, beard) in &bent {
            let _ = structures.terrain().fill_bent(*x, *z, beard, &mut scratch);
        }
        let with_beard = began.elapsed();
        println!(
            "{name}: made in {made:.1?}, {rings} ring positions in {ringed:.1?}; the starts \
             of {} chunks in {started:.1?} ({:.1?} a chunk, the slowest {slowest:.1?}): \
             {count} starts with {pieces} pieces; references and beardifier of {chunks} \
             chunks in {bearded:.1?} ({:.1?} a chunk); {} chunks are bent, their fill takes \
             {:.2?} a chunk without the beardifier and {:.2?} with it",
            starts.len(),
            started / starts.len() as u32,
            bearded / chunks,
            bent.len(),
            plain / bent.len().max(1) as u32,
            with_beard / bent.len().max(1) as u32,
        );
    }
}
