//! The emitted structures, sets, pools and the table of templates hang together:
//! every reference by number names something, and the table has what ADR-0019 says
//! of the pinned jar.

use clustine_worldgen_data::structure::{Kind, PoolElement, Spread};
use clustine_worldgen_data::structures::{STRUCTURE_SETS, STRUCTURES};
use clustine_worldgen_data::template_pools::POOLS;
use clustine_worldgen_data::templates;

#[test]
fn the_table_of_templates_has_every_template_of_the_jar_and_one_that_is_absent() {
    let table = templates();
    assert_eq!(table.len(), 1512);
    assert_eq!(table.jigsaw_count(), 4272);
    let absent: Vec<&str> = (0..table.len())
        .map(|index| table.get(index))
        .filter(|template| !template.exists)
        .map(|template| template.name)
        .collect();
    assert_eq!(
        absent,
        ["ancient_city/walls/intact_horizontal_wall_stairs_5"]
    );
    let lacking = table.get(table.find(absent[0]).unwrap());
    assert_eq!(lacking.size, [0, 0, 0]);
    assert_eq!(table.jigsaws(&lacking).len(), 0);
    for index in 0..table.len() {
        let template = table.get(index);
        assert!(template.size.iter().all(|side| (0..=48).contains(side)));
        assert_eq!(table.find(template.name), Some(index));
    }
}

#[test]
fn a_templates_jigsaw_blocks_are_in_the_order_of_the_games_list() {
    let table = templates();
    let mut most = 0;
    for index in 0..table.len() {
        let template = table.get(index);
        let jigsaws: Vec<_> = table.jigsaws(&template).collect();
        most = most.max(jigsaws.len());
        for pair in jigsaws.windows(2) {
            let key = |position: [i32; 3]| (position[1], position[0], position[2]);
            assert!(
                key(pair[0].position) < key(pair[1].position),
                "{}",
                template.name
            );
        }
        for jigsaw in &jigsaws {
            for (side, at) in template.size.iter().zip(jigsaw.position) {
                assert!(at < *side, "{}", template.name);
            }
            assert!((0..=2).contains(&jigsaw.selection_priority));
            assert!((0..=3).contains(&jigsaw.placement_priority));
        }
    }
    assert_eq!(most, 47);
}

#[test]
fn a_known_template_reads_as_the_jar_has_it() {
    let table = templates();
    let house = table.get(
        table
            .find("village/plains/houses/plains_small_house_1")
            .unwrap(),
    );
    assert_eq!(house.size, [7, 7, 7]);
    let jigsaws: Vec<_> = table.jigsaws(&house).collect();
    assert_eq!(jigsaws.len(), 2);
    assert!(
        jigsaws
            .iter()
            .any(|jigsaw| jigsaw.name == "minecraft:building_entrance")
    );
}

#[test]
fn every_reference_by_number_names_something() {
    let table = templates();
    assert_eq!(STRUCTURES.len(), 52);
    assert_eq!(STRUCTURE_SETS.len(), 21);
    assert_eq!(POOLS.len(), 245);
    assert!(
        STRUCTURES
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(
        STRUCTURE_SETS
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    );
    assert!(POOLS.windows(2).all(|pair| pair[0].name < pair[1].name));

    for structure in &STRUCTURES {
        assert!(!structure.biomes.is_empty(), "{}", structure.name);
        assert!(structure.biomes.windows(2).all(|pair| pair[0] < pair[1]));
        if let Kind::Jigsaw(jigsaw) = structure.kind {
            assert!(usize::from(jigsaw.start_pool) < POOLS.len());
            assert!((0..=20).contains(&jigsaw.max_depth));
        }
    }
    let mut used = vec![false; STRUCTURES.len()];
    for set in &STRUCTURE_SETS {
        for (structure, weight) in set.structures {
            assert!(*weight > 0);
            assert!(!std::mem::replace(&mut used[usize::from(*structure)], true));
        }
        if let Some(zone) = set.placement.exclusion_zone {
            assert!(usize::from(zone.other_set) < STRUCTURE_SETS.len());
        }
        if let Spread::Random {
            spacing,
            separation,
            ..
        } = set.placement.spread
        {
            assert!(spacing > separation);
        }
    }
    assert!(used.iter().all(|used| *used), "a structure is in no set");

    fn check(element: &PoolElement, templates: usize, counts: &mut [usize; 4]) {
        match element {
            PoolElement::Empty => counts[0] += 1,
            PoolElement::Single { template, .. } => {
                assert!(usize::from(*template) < templates);
                counts[1] += 1;
            }
            PoolElement::Feature { .. } => counts[2] += 1,
            PoolElement::List { elements, .. } => {
                counts[3] += 1;
                for inner in *elements {
                    check(inner, templates, counts);
                }
            }
        }
    }
    let mut counts = [0; 4];
    let mut entries = 0;
    for pool in &POOLS {
        assert!(usize::from(pool.fallback) < POOLS.len());
        for (element, weight) in pool.elements {
            assert!(*weight > 0);
            entries += 1;
            check(element, table.len(), &mut counts);
        }
    }
    // ADR-0019's count of the jar: 2,281 weighted elements, of which three lists.
    assert_eq!(entries, 2281);
    assert_eq!(counts[0], 31);
    assert_eq!(counts[2], 57);
    assert_eq!(counts[3], 3);
}
