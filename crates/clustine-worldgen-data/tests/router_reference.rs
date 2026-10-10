//! The emitted noise routers against the game's own: every entry of the routers of the
//! overworld, the Nether and the End, at the positions and for the seeds of the files
//! under `reference/`, has to be the same bits.
//!
//! The files are written by `cargo datagen` from what its Java program computes with
//! the classes of the pinned jar and no server: `RandomState.sampleBlockValueUncached`
//! for the game's built-in noise settings. Their form is described in
//! `tools/datagen/src/reference.rs`.

use clustine_worldgen_data::{
    AquiferEntry, EndRouter, NetherRouter, NoiseRouter, OverworldRouter, RouterEntry,
};

/// What a file of reference values holds.
struct Reference {
    settings: String,
    seed: i64,
    /// Each entry's name, and the bits of its value where it is the same everywhere.
    entries: Vec<(String, Option<u32>)>,
    /// A position and the bits of every entry that is not constant.
    rows: Vec<([i32; 3], Vec<u32>)>,
}

impl Reference {
    fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let settings = lines.next().unwrap().strip_prefix("settings ").unwrap();
        let seed = lines.next().unwrap().strip_prefix("seed ").unwrap();
        let mut entries = Vec::new();
        let mut rows = Vec::new();
        for line in lines {
            let fields: Vec<&str> = line.split(' ').collect();
            let bits = |text: &str| u32::from_str_radix(text, 16).unwrap();
            match fields[..] {
                ["entry", name] => entries.push((name.to_owned(), None)),
                ["constant", name, value] => entries.push((name.to_owned(), Some(bits(value)))),
                [x, y, z, ref values @ ..] => rows.push((
                    [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                    values.iter().map(|value| bits(value)).collect(),
                )),
                _ => panic!("the line {line:?} is neither an entry nor a row"),
            }
        }
        Self {
            settings: settings.to_owned(),
            seed: seed.parse().unwrap(),
            entries,
            rows,
        }
    }

    /// The expected bits of every entry at every row, by entry.
    fn expected(&self) -> Vec<(&str, Vec<u32>)> {
        let mut column = 0;
        self.entries
            .iter()
            .map(|(name, constant)| {
                let values = match constant {
                    Some(value) => vec![*value; self.rows.len()],
                    None => {
                        column += 1;
                        self.rows.iter().map(|(_, row)| row[column - 1]).collect()
                    }
                };
                (name.as_str(), values)
            })
            .collect()
    }
}

/// The router's entry with a name of the reference files.
fn sample<R: NoiseRouter>(router: &R, memory: &mut R::Memory, name: &str, at: [i32; 3]) -> f32 {
    let [x, y, z] = at;
    if let Some(name) = name.strip_prefix("aquifers.") {
        let entry = AquiferEntry::ALL
            .into_iter()
            .find(|entry| entry.name() == name)
            .unwrap_or_else(|| panic!("{name} is no entry of the aquifers"));
        return router.aquifer(memory, entry, x, y, z);
    }
    let entry = RouterEntry::ALL
        .into_iter()
        .find(|entry| entry.name() == name)
        .unwrap_or_else(|| panic!("{name} is no entry of a router"));
    router.sample(memory, entry, x, y, z)
}

/// Compares every value of `text` and gives how many were compared. It panics with
/// every entry that differs, how often, and the first place.
fn compare<R: NoiseRouter>(text: &str, positions: usize) -> usize {
    let reference = Reference::parse(text);
    let router = R::new(reference.seed);
    assert_eq!(router.settings().name, reference.settings);
    assert_eq!(reference.rows.len(), positions);
    let expected_entries = if router.settings().has_aquifers {
        14
    } else {
        8
    };
    assert_eq!(reference.entries.len(), expected_entries);

    let mut compared = 0;
    let mut failures = Vec::new();
    // One memory through every entry and position, as a caller keeps one.
    let mut memory = R::Memory::default();
    for (name, expected) in reference.expected() {
        let mut differing = 0;
        let mut first = None;
        for ((at, _), expected) in reference.rows.iter().zip(&expected) {
            let value = sample(&router, &mut memory, name, *at);
            // What was remembered never changes a value.
            let fresh = sample(&router, &mut R::Memory::default(), name, *at);
            assert_eq!(
                value.to_bits(),
                fresh.to_bits(),
                "{name} at {at:?} is another value with a memory that was used before"
            );
            compared += 1;
            if value.to_bits() != *expected {
                differing += 1;
                first.get_or_insert((*at, value, f32::from_bits(*expected)));
            }
        }
        if let Some((at, value, expected)) = first {
            failures.push(format!(
                "{name}: {differing} of {} differ; at {at:?} it is {value:?} ({:08x}), the game \
                 has {expected:?} ({:08x})",
                reference.rows.len(),
                value.to_bits(),
                expected.to_bits()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the router of {} for the seed {} is not the game's:\n{}",
        reference.settings,
        reference.seed,
        failures.join("\n")
    );
    compared
}

const POSITIONS: usize = 256;

#[test]
fn the_overworlds_router_gives_the_games_bits_for_the_seed_13579() {
    let text = include_str!("../reference/router_overworld_13579.txt");
    assert_eq!(compare::<OverworldRouter>(text, POSITIONS), 14 * POSITIONS);
}

#[test]
fn the_overworlds_router_gives_the_games_bits_for_the_seed_0() {
    let text = include_str!("../reference/router_overworld_0.txt");
    assert_eq!(compare::<OverworldRouter>(text, POSITIONS), 14 * POSITIONS);
}

#[test]
fn the_nethers_router_gives_the_games_bits_for_the_seed_13579() {
    let text = include_str!("../reference/router_nether_13579.txt");
    assert_eq!(compare::<NetherRouter>(text, POSITIONS), 8 * POSITIONS);
}

#[test]
fn the_nethers_router_gives_the_games_bits_for_the_seed_0() {
    let text = include_str!("../reference/router_nether_0.txt");
    assert_eq!(compare::<NetherRouter>(text, POSITIONS), 8 * POSITIONS);
}

#[test]
fn the_ends_router_gives_the_games_bits_for_the_seed_13579() {
    let text = include_str!("../reference/router_end_13579.txt");
    assert_eq!(compare::<EndRouter>(text, POSITIONS), 8 * POSITIONS);
}

#[test]
fn the_ends_router_gives_the_games_bits_for_the_seed_0() {
    let text = include_str!("../reference/router_end_0.txt");
    assert_eq!(compare::<EndRouter>(text, POSITIONS), 8 * POSITIONS);
}

#[test]
fn the_reference_values_are_not_all_alike() {
    // A comparison of constants would prove little: every entry of the overworld
    // takes many values over the positions (the surface levels the fewest: they are
    // multiples of eight), and the two seeds differ.
    let first = Reference::parse(include_str!("../reference/router_overworld_13579.txt"));
    let second = Reference::parse(include_str!("../reference/router_overworld_0.txt"));
    assert_eq!(first.rows.len(), second.rows.len());
    for ((name, values), (_, other)) in first.expected().iter().zip(second.expected()) {
        let mut distinct = values.clone();
        distinct.sort_unstable();
        distinct.dedup();
        let least = if name.ends_with("surface_level") {
            8
        } else {
            POSITIONS / 2
        };
        assert!(
            distinct.len() >= least,
            "{name} takes only {} values",
            distinct.len()
        );
        assert_ne!(*values, other, "{name} is the same for both seeds");
    }
    // The positions are the same for every file, and reach below zero and far out.
    let positions: Vec<[i32; 3]> = first.rows.iter().map(|(at, _)| *at).collect();
    assert!(positions.iter().any(|at| at[0] < -1_000_000));
    assert!(positions.iter().any(|at| at[2] > 1_000_000));
    assert!(positions.iter().any(|at| at[1] < -64) && positions.iter().any(|at| at[1] > 319));
}

#[test]
fn a_function_is_reached_by_its_name_or_not_at_all() {
    let router = OverworldRouter::new(13579);
    let mut memory = Default::default();
    let by_name = router.named(&mut memory, "minecraft:overworld/continents", 10, 70, -33);
    let by_entry = router.sample(&mut memory, RouterEntry::Continents, 10, 70, -33);
    assert_eq!(by_name.map(f32::to_bits), Some(by_entry.to_bits()));
    assert!(
        router
            .named(&mut memory, "minecraft:nether/base_3d_noise", 0, 0, 0)
            .is_none()
    );
    assert!(
        router
            .named(&mut memory, "minecraft:overworld/nothing", 0, 0, 0)
            .is_none()
    );
    // Every function the spawn target names is one that can be asked.
    for point in router.settings().spawn_target {
        for range in *point {
            assert!(
                router
                    .named(&mut memory, range.function, 0, 64, 0)
                    .is_some(),
                "{}",
                range.function
            );
        }
    }

    // A dimension without aquifers answers zero for them.
    let nether = NetherRouter::new(13579);
    let mut memory = Default::default();
    assert!(!nether.settings().has_aquifers);
    assert_eq!(
        nether.aquifer(&mut memory, AquiferEntry::Barrier, 1, 2, 3),
        0.0
    );
}
