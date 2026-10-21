//! Emits `spawning.rs`: whom a mob may spawn on each block state (`isValidSpawn`), the
//! block states dangerous to each entity type (`isBlockDangerous`), and each type's
//! placement and height map (`SpawnPlacements`), as the extract program asked the game
//! (`docs/groundwork/n7l-plan.md`, section 5.1, L0).

use std::fmt::Write as _;

use anyhow::{Result, bail};

use crate::extract::{Extract, ValidSpawn};

pub fn emit(extract: &Extract) -> Result<String> {
    let mut out = String::new();
    writeln!(
        out,
        "//! Whom a mob may spawn on each block state, the states dangerous to each entity \
         type, and"
    )?;
    writeln!(out, "//! each type's placement and height map.")?;
    writeln!(out)?;
    writeln!(out, "use crate::spawning::{{Heightmap, Placement}};")?;
    writeln!(out)?;
    // Runs of states valid for every type, as their first id and their end.
    let mut runs = Vec::new();
    let mut some = Vec::new();
    let mut start = None;
    for (id, valid) in extract.valid_spawn.iter().enumerate() {
        let id = u32::try_from(id)?;
        match valid {
            ValidSpawn::All => {
                start.get_or_insert(id);
            }
            other => {
                if let Some(first) = start.take() {
                    runs.push((first, id));
                }
                if let ValidSpawn::Some(types) = other {
                    some.push((id, types));
                }
            }
        }
    }
    if let Some(first) = start {
        runs.push((first, u32::try_from(extract.valid_spawn.len())?));
    }
    writeln!(
        out,
        "/// The states on which a mob of any type may spawn, as runs of their ids, the end \
         left out."
    )?;
    writeln!(
        out,
        "pub static VALID_FOR_ALL: [(u32, u32); {}] = [",
        runs.len()
    )?;
    for (first, end) in &runs {
        writeln!(out, "    ({first}, {end}),")?;
    }
    writeln!(out, "];")?;
    writeln!(out)?;
    writeln!(
        out,
        "/// The states on which only some types may spawn, ascending, with those types' ids."
    )?;
    writeln!(
        out,
        "pub static VALID_FOR_SOME: [(u32, &[u16]); {}] = [",
        some.len()
    )?;
    for (id, types) in &some {
        let list: Vec<String> = types.iter().map(u16::to_string).collect();
        writeln!(out, "    ({id}, &[{}]),", list.join(", "))?;
    }
    writeln!(out, "];")?;
    writeln!(out)?;
    // Most types share one list, and a list is a few runs of ids (the fire's 512
    // states among them): each distinct list once, as runs, and a type its list's place.
    let mut sets: Vec<&Vec<u32>> = Vec::new();
    let mut of_type = Vec::new();
    for states in &extract.dangerous {
        let place = match sets.iter().position(|known| *known == states) {
            Some(place) => place,
            None => {
                sets.push(states);
                sets.len() - 1
            }
        };
        of_type.push(place);
    }
    writeln!(
        out,
        "/// The distinct sets of states dangerous to a type, each as runs of their ids, the \
         end left out."
    )?;
    writeln!(
        out,
        "pub static DANGER_SETS: [&[(u32, u32)]; {}] = [",
        sets.len()
    )?;
    for states in &sets {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &id in states.iter() {
            match runs.last_mut() {
                Some(run) if run.1 == id => run.1 = id + 1,
                _ => runs.push((id, id + 1)),
            }
        }
        let list: Vec<String> = runs.iter().map(|(a, b)| format!("({a}, {b})")).collect();
        writeln!(out, "    &[{}],", list.join(", "))?;
    }
    writeln!(out, "];")?;
    writeln!(out)?;
    writeln!(
        out,
        "/// For each entity type, by its id, the place of its set in [`DANGER_SETS`]."
    )?;
    let places: Vec<String> = of_type.iter().map(usize::to_string).collect();
    writeln!(
        out,
        "pub static DANGEROUS: [u8; {}] = [{}];",
        of_type.len(),
        places.join(", ")
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "/// For each entity type, by its id, its placement and the height map it is placed \
         by."
    )?;
    writeln!(
        out,
        "pub static PLACEMENTS: [(Placement, Heightmap); {}] = [",
        extract.spawn_placements.len()
    )?;
    for (placement, heightmap) in &extract.spawn_placements {
        let placement = match placement.as_str() {
            "on_ground" => "OnGround",
            "in_water" => "InWater",
            "in_lava" => "InLava",
            "no_restrictions" => "NoRestrictions",
            other => bail!("a placement {other}"),
        };
        let heightmap = match heightmap.as_str() {
            "MOTION_BLOCKING" => "MotionBlocking",
            "MOTION_BLOCKING_NO_LEAVES" => "MotionBlockingNoLeaves",
            "WORLD_SURFACE" => "WorldSurface",
            "OCEAN_FLOOR" => "OceanFloor",
            other => bail!("a height map {other}"),
        };
        writeln!(out, "    (Placement::{placement}, Heightmap::{heightmap}),")?;
    }
    writeln!(out, "];")?;
    Ok(out)
}
