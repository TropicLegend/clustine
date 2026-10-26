//! Tests of the placing of a template's entities (`placeEntities`,
//! `docs/groundwork/n7-brief.md`, section 8.10), from the game's formulas worked by
//! hand: where each entity of the census lands under every turn and mirror, which are
//! placed, and what their tags become.
#![expect(
    clippy::excessive_precision,
    reason = "the templates' yaws are written as their files hold them, exactly"
)]

use clustine_data::blocks;
use clustine_noise::WorldgenRandom;
use clustine_protocol::nbt::Nbt;

use super::*;
use crate::template_blocks::TemplateEntity;
use crate::testing::World;

/// Where the census's templates are placed, and about what they turn.
const AT: BlockPos = BlockPos::new(32, 64, -16);
const PIVOT: BlockPos = BlockPos::new(1, 0, 2);

/// An entity of the census (`n7-brief.md`, section 8.11, read from the templates): its
/// template, its `pos`, `blockPos` and yaw, and, for one turn and mirror each, where
/// the formulas put it, worked by hand: its block, its position and its yaw.
struct Census {
    template: &'static str,
    pos: [f64; 3],
    block_pos: [i32; 3],
    yaw: f32,
    case: (Rotation, StructureMirror),
    block: [i32; 3],
    position: [f64; 3],
    turned: f32,
}

const ROTATIONS: [Rotation; 4] = [
    Rotation::None,
    Rotation::Clockwise90,
    Rotation::Clockwise180,
    Rotation::CounterClockwise90,
];
const MIRRORS: [StructureMirror; 3] = [
    StructureMirror::None,
    StructureMirror::LeftRight,
    StructureMirror::FrontBack,
];

/// The census's entities other than the bastion's (the Nether's), each template's
/// villagers once for the five kinds of village, which share them, and two of the 224
/// cushions of the camps.
const CENSUS: [Census; 39] = [
    Census {
        template: "igloo/bottom",
        pos: [2.5, 1.0, 1.5],
        block_pos: [2, 1, 1],
        yaw: -4.32769775390625,
        case: (Rotation::None, StructureMirror::None),
        block: [34, 65, -15],
        position: [34.5, 65.0, -14.5],
        turned: -4.32769775390625,
    },
    Census {
        template: "igloo/bottom",
        pos: [4.5, 1.0, 1.300000011920929],
        block_pos: [4, 1, 1],
        yaw: -95.78887939453125,
        case: (Rotation::None, StructureMirror::LeftRight),
        block: [36, 65, -17],
        position: [36.5, 65.0, -16.30000001192093],
        turned: 275.78887939453125,
    },
    Census {
        template: "pillager_outpost/feature_cage1",
        pos: [3.699999988079071, 1.0, 3.699999988079071],
        block_pos: [3, 1, 3],
        yaw: 0.0,
        case: (Rotation::None, StructureMirror::FrontBack),
        block: [29, 65, -13],
        position: [29.30000001192093, 65.0, -12.300000011920929],
        turned: 0.0,
    },
    Census {
        template: "pillager_outpost/feature_cage_with_allays",
        pos: [4.5355692304166855, 1.0508262449273857, 4.5],
        block_pos: [4, 1, 4],
        yaw: 270.0,
        case: (Rotation::Clockwise90, StructureMirror::None),
        block: [31, 65, -11],
        position: [31.5, 65.05082624492738, -10.464430769583315],
        turned: -360.0,
    },
    Census {
        template: "pillager_outpost/feature_cage_with_allays",
        pos: [4.532911080022236, 1.9317999950649707, 4.483273631254721],
        block_pos: [4, 1, 4],
        yaw: 270.0,
        case: (Rotation::Clockwise90, StructureMirror::LeftRight),
        block: [39, 65, -11],
        position: [39.48327363125472, 65.93179999506498, -10.467088919977764],
        turned: 0.0,
    },
    Census {
        template: "village/common/iron_golem",
        pos: [0.5499999999999972, 1.0, 0.4400000000000013],
        block_pos: [0, 1, 0],
        yaw: 0.0,
        case: (Rotation::Clockwise90, StructureMirror::FrontBack),
        block: [35, 65, -15],
        position: [35.56, 65.0, -14.549999999999997],
        turned: 90.0,
    },
    Census {
        template: "village/common/animals/cat_black",
        pos: [0.594088879085021, 1.0, 0.7762497828982937],
        block_pos: [0, 1, 0],
        yaw: 157.4461212158203,
        case: (Rotation::Clockwise180, StructureMirror::None),
        block: [34, 65, -12],
        position: [34.40591112091498, 65.0, -11.776249782898294],
        turned: 337.44610595703125,
    },
    Census {
        template: "village/common/animals/cat_british",
        pos: [0.10287446715470594, 1.0, 0.6401005165379434],
        block_pos: [0, 1, 0],
        yaw: 130.67245483398438,
        case: (Rotation::Clockwise180, StructureMirror::LeftRight),
        block: [34, 65, -12],
        position: [34.897125532845294, 65.0, -11.359899483462057],
        turned: 229.32754516601562,
    },
    Census {
        template: "village/common/animals/cat_calico",
        pos: [1.034435390287527, 1.0, 1.0372841507700956],
        block_pos: [1, 1, 1],
        yaw: 61.25115966796875,
        case: (Rotation::Clockwise180, StructureMirror::FrontBack),
        block: [35, 65, -13],
        position: [35.03443539028753, 65.0, -12.037284150770095],
        turned: 118.74884033203125,
    },
    Census {
        template: "village/common/animals/cat_jellie",
        pos: [0.594088879085021, 1.0, 0.7762497828982937],
        block_pos: [0, 1, 0],
        yaw: 157.4461212158203,
        case: (Rotation::CounterClockwise90, StructureMirror::None),
        block: [31, 65, -13],
        position: [31.776249782898294, 65.0, -12.594088879085021],
        turned: 427.44610595703125,
    },
    Census {
        template: "village/common/animals/cat_persian",
        pos: [0.7045879820373386, 1.0, 1.0038031046223237],
        block_pos: [0, 1, 1],
        yaw: 79.62802124023438,
        case: (Rotation::CounterClockwise90, StructureMirror::LeftRight),
        block: [30, 65, -13],
        position: [30.996196895377675, 65.0, -12.704587982037339],
        turned: 370.3719787597656,
    },
    Census {
        template: "village/common/animals/cat_ragdoll",
        pos: [0.09318670032310195, 1.0, 0.3996525399343511],
        block_pos: [0, 1, 0],
        yaw: -153.1834259033203,
        case: (Rotation::CounterClockwise90, StructureMirror::FrontBack),
        block: [31, 65, -13],
        position: [31.39965253993435, 65.0, -12.906813299676898],
        turned: 423.18341064453125,
    },
    Census {
        template: "village/common/animals/cat_red",
        pos: [0.6200000000000045, 1.0, -0.07499998807907104],
        block_pos: [0, 1, -1],
        yaw: 0.0,
        case: (Rotation::None, StructureMirror::None),
        block: [32, 65, -17],
        position: [32.620000000000005, 65.0, -16.07499998807907],
        turned: 0.0,
    },
    Census {
        template: "village/common/animals/cat_siamese",
        pos: [0.491924415155637, 1.0, 0.8459992136320515],
        block_pos: [0, 1, 0],
        yaw: 0.0,
        case: (Rotation::None, StructureMirror::LeftRight),
        block: [32, 65, -16],
        position: [32.49192441515564, 65.0, -15.845999213632052],
        turned: 180.0,
    },
    Census {
        template: "village/common/animals/cat_tabby",
        pos: [0.6606286838719058, 1.0, 0.8089603178721241],
        block_pos: [0, 1, 0],
        yaw: -113.60992431640625,
        case: (Rotation::None, StructureMirror::FrontBack),
        block: [32, 65, -16],
        position: [32.339371316128094, 65.0, -15.191039682127876],
        turned: 113.60992431640625,
    },
    Census {
        template: "village/common/animals/cat_white",
        pos: [0.38217550315006577, 1.0, 0.8995760608387542],
        block_pos: [0, 1, 0],
        yaw: 137.05532836914062,
        case: (Rotation::Clockwise90, StructureMirror::None),
        block: [35, 65, -15],
        position: [35.100423939161246, 65.0, -14.617824496849934],
        turned: 227.05532836914062,
    },
    Census {
        template: "village/common/animals/cows_1",
        pos: [0.07499998807907104, 1.0, 0.07499998807907104],
        block_pos: [0, 1, 0],
        yaw: 153.6094207763672,
        case: (Rotation::Clockwise90, StructureMirror::LeftRight),
        block: [35, 65, -15],
        position: [35.07499998807907, 65.0, -14.925000011920929],
        turned: 116.39057922363281,
    },
    Census {
        template: "village/common/animals/cows_1",
        pos: [0.925000011920929, 1.0, 0.925000011920929],
        block_pos: [0, 1, 0],
        yaw: -88.567626953125,
        case: (Rotation::Clockwise90, StructureMirror::FrontBack),
        block: [35, 65, -15],
        position: [35.07499998807907, 65.0, -14.925000011920929],
        turned: 178.567626953125,
    },
    Census {
        template: "village/common/animals/horses_1",
        pos: [0.3232421875, 1.0, 0.6767578125],
        block_pos: [0, 1, 0],
        yaw: 81.71751403808594,
        case: (Rotation::Clockwise180, StructureMirror::None),
        block: [34, 65, -12],
        position: [34.6767578125, 65.0, -11.6767578125],
        turned: 261.717529296875,
    },
    Census {
        template: "village/common/animals/horses_2",
        pos: [0.3232421875, 1.0, 0.3232421875],
        block_pos: [0, 1, 0],
        yaw: 60.22014236450195,
        case: (Rotation::Clockwise180, StructureMirror::LeftRight),
        block: [34, 65, -12],
        position: [34.6767578125, 65.0, -11.6767578125],
        turned: 299.77984619140625,
    },
    Census {
        template: "village/common/animals/horses_2",
        pos: [0.3232421875, 1.0, 0.6767578125],
        block_pos: [0, 1, 0],
        yaw: 0.0,
        case: (Rotation::Clockwise180, StructureMirror::FrontBack),
        block: [34, 65, -12],
        position: [34.3232421875, 65.0, -11.6767578125],
        turned: 180.0,
    },
    Census {
        template: "village/common/animals/horses_3",
        pos: [0.3232421875, 1.0, 0.6767578125],
        block_pos: [0, 1, 0],
        yaw: 1.2443947792053223,
        case: (Rotation::CounterClockwise90, StructureMirror::None),
        block: [31, 65, -13],
        position: [31.6767578125, 65.0, -12.3232421875],
        turned: 271.244384765625,
    },
    Census {
        template: "village/common/animals/horses_3",
        pos: [0.6767578125, 1.0, 0.3232421875],
        block_pos: [0, 1, 0],
        yaw: 0.0,
        case: (Rotation::CounterClockwise90, StructureMirror::LeftRight),
        block: [31, 65, -13],
        position: [31.6767578125, 65.0, -12.6767578125],
        turned: 450.0,
    },
    Census {
        template: "village/common/animals/horses_4",
        pos: [0.3232421875, 1.0, 0.6767578125],
        block_pos: [0, 1, 0],
        yaw: 164.32266235351562,
        case: (Rotation::CounterClockwise90, StructureMirror::FrontBack),
        block: [31, 65, -13],
        position: [31.6767578125, 65.0, -12.6767578125],
        turned: 105.67733764648438,
    },
    Census {
        template: "village/common/animals/horses_5",
        pos: [0.5380230367288732, 1.0, 0.32324218749994316],
        block_pos: [0, 1, 0],
        yaw: -139.907470703125,
        case: (Rotation::None, StructureMirror::None),
        block: [32, 65, -16],
        position: [32.53802303672887, 65.0, -15.676757812500057],
        turned: -139.907470703125,
    },
    Census {
        template: "village/common/animals/pigs_1",
        pos: [0.07499998807907104, 1.0, 0.925000011920929],
        block_pos: [0, 1, 0],
        yaw: 111.30493927001953,
        case: (Rotation::None, StructureMirror::LeftRight),
        block: [32, 65, -16],
        position: [32.07499998807907, 65.0, -15.925000011920929],
        turned: 68.69506072998047,
    },
    Census {
        template: "village/common/animals/pigs_1",
        pos: [0.925000011920929, 1.0, 0.07499998807907104],
        block_pos: [0, 1, 0],
        yaw: -27.401519775390625,
        case: (Rotation::None, StructureMirror::FrontBack),
        block: [32, 65, -16],
        position: [32.07499998807907, 65.0, -15.925000011920929],
        turned: 27.401519775390625,
    },
    Census {
        template: "village/common/animals/sheep_1",
        pos: [0.07499998807907104, 1.0, 0.925000011920929],
        block_pos: [0, 1, 0],
        yaw: 152.29917907714844,
        case: (Rotation::Clockwise90, StructureMirror::None),
        block: [35, 65, -15],
        position: [35.07499998807907, 65.0, -14.925000011920929],
        turned: 242.29917907714844,
    },
    Census {
        template: "village/common/animals/sheep_1",
        pos: [0.925000011920929, 1.0, 0.07499998807907104],
        block_pos: [0, 1, 0],
        yaw: 21.586524963378906,
        case: (Rotation::Clockwise90, StructureMirror::LeftRight),
        block: [35, 65, -15],
        position: [35.07499998807907, 65.0, -14.074999988079071],
        turned: 248.41348266601562,
    },
    Census {
        template: "village/common/animals/sheep_2",
        pos: [0.07499998807907104, 1.0, 0.925000011920929],
        block_pos: [0, 1, 0],
        yaw: 152.29917907714844,
        case: (Rotation::Clockwise90, StructureMirror::FrontBack),
        block: [35, 65, -15],
        position: [35.07499998807907, 65.0, -14.074999988079071],
        turned: -62.29917907714844,
    },
    Census {
        template: "village/common/animals/sheep_2",
        pos: [0.925000011920929, 1.0, 0.07499998807907104],
        block_pos: [0, 1, 0],
        yaw: 21.586524963378906,
        case: (Rotation::Clockwise180, StructureMirror::None),
        block: [34, 65, -12],
        position: [34.07499998807907, 65.0, -11.074999988079071],
        turned: 201.58651733398438,
    },
    Census {
        template: "village/desert/camel_spawn",
        pos: [0.0, 0.0, 0.0],
        block_pos: [0, 0, 1],
        yaw: 0.0,
        case: (Rotation::Clockwise180, StructureMirror::LeftRight),
        block: [34, 64, -11],
        position: [35.0, 64.0, -12.0],
        turned: 360.0,
    },
    Census {
        template: "village/*/villagers/baby",
        pos: [0.36605308557818717, 1.0, 1.0697962117287148],
        block_pos: [0, 1, 1],
        yaw: 0.0,
        case: (Rotation::Clockwise180, StructureMirror::FrontBack),
        block: [34, 65, -13],
        position: [34.36605308557819, 65.0, -12.069796211728715],
        turned: 180.0,
    },
    Census {
        template: "village/*/villagers/{nitwit,unemployed}",
        pos: [0.7204986190855891, 1.0, 0.631455289898156],
        block_pos: [0, 1, 0],
        yaw: 48.821632385253906,
        case: (Rotation::CounterClockwise90, StructureMirror::None),
        block: [31, 65, -13],
        position: [31.631455289898156, 65.0, -12.720498619085589],
        turned: 318.8216247558594,
    },
    Census {
        template: "village/*/zombie/villagers/{nitwit,unemployed}",
        pos: [0.7204986190855891, 1.0, 0.631455289898156],
        block_pos: [0, 1, 0],
        yaw: 48.821632385253906,
        case: (Rotation::CounterClockwise90, StructureMirror::LeftRight),
        block: [31, 65, -13],
        position: [31.368544710101844, 65.0, -12.720498619085589],
        turned: 401.1783752441406,
    },
    Census {
        template: "village/taiga/houses/taiga_armorer_2",
        pos: [2.5, 1.0, 1.5],
        block_pos: [2, 1, 1],
        yaw: 45.0,
        case: (Rotation::CounterClockwise90, StructureMirror::FrontBack),
        block: [32, 65, -11],
        position: [32.5, 65.0, -10.5],
        turned: 225.0,
    },
    Census {
        template: "village/taiga/houses/taiga_armorer_2",
        pos: [4.5, 1.0, 5.5],
        block_pos: [4, 1, 5],
        yaw: 90.0,
        case: (Rotation::None, StructureMirror::None),
        block: [36, 65, -11],
        position: [36.5, 65.0, -10.5],
        turned: 90.0,
    },
    Census {
        template: "abandoned_camp/camp/default/campsite_default_barrel_1",
        pos: [6.5, 0.9375, 6.5],
        block_pos: [6, 0, 6],
        yaw: 180.0,
        case: (Rotation::None, StructureMirror::LeftRight),
        block: [38, 64, -22],
        position: [38.5, 64.9375, -21.5],
        turned: 0.0,
    },
    Census {
        template: "abandoned_camp/camp/default/campsite_default_barrel_10",
        pos: [5.5, 0.9375, 4.5],
        block_pos: [5, 0, 4],
        yaw: 90.0,
        case: (Rotation::None, StructureMirror::FrontBack),
        block: [27, 64, -12],
        position: [27.5, 64.9375, -11.5],
        turned: -90.0,
    },
];

/// A template of eight by four by eight blocks with no blocks and `entities`.
fn holding(entities: Vec<TemplateEntity>) -> TemplateBlocks {
    TemplateBlocks {
        size: [8, 4, 8],
        data_version: clustine_data::DATA_VERSION,
        palettes: vec![Vec::new()],
        entities,
    }
}

/// An entity's tag as a template keeps it: its id, its rotation and a UUID.
fn tag(id: &str, yaw: f32, pitch: f32) -> Nbt {
    Nbt::Compound(vec![
        ("id".into(), Nbt::String(id.into())),
        (
            "Rotation".into(),
            Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(pitch)]),
        ),
        ("UUID".into(), Nbt::IntArray(vec![1, 2, 3, 4])),
    ])
}

fn settings(rotation: Rotation, mirror: StructureMirror) -> StructurePlaceSettings<'static> {
    let mut settings = StructurePlaceSettings::new(0);
    settings.rotation = rotation;
    settings.mirror = mirror;
    settings.rotation_pivot = PIVOT;
    settings.known_shape = true;
    settings.finalize_entities = true;
    settings
}

fn place(
    template: &TemplateBlocks,
    at: BlockPos,
    settings: &StructurePlaceSettings<'_>,
) -> Option<Vec<PlacedEntity>> {
    let mut world = World::flat(blocks::STONE, 0);
    place_in_world(
        template,
        &mut world,
        at,
        at,
        settings,
        &mut WorldgenRandom::from_seed(1),
    )
}

#[test]
fn every_entity_of_the_census_lands_where_the_formulas_put_it() {
    for entry in &CENSUS {
        let template = holding(vec![TemplateEntity {
            pos: entry.pos,
            block_pos: BlockPos::new(entry.block_pos[0], entry.block_pos[1], entry.block_pos[2]),
            nbt: tag("minecraft:cow", entry.yaw, 0.0),
        }]);
        let (rotation, mirror) = entry.case;
        let placed = place(&template, AT, &settings(rotation, mirror)).expect("it places");
        assert_eq!(placed.len(), 1, "{}", entry.template);
        let placed = &placed[0];
        assert_eq!(
            placed.block,
            BlockPos::new(entry.block[0], entry.block[1], entry.block[2]),
            "{}",
            entry.template
        );
        assert_eq!(
            placed.position.map(f64::to_bits),
            entry.position.map(f64::to_bits),
            "{}: {:?}",
            entry.template,
            placed.position
        );
        assert_eq!(
            placed.yaw.to_bits(),
            entry.turned.to_bits(),
            "{}: {}",
            entry.template,
            placed.yaw
        );
        assert!(placed.finalise);
    }
}

#[test]
fn every_entity_of_the_census_is_placed_under_every_turn_and_mirror_in_its_moved_block() {
    for entry in &CENSUS {
        for rotation in ROTATIONS {
            for mirror in MIRRORS {
                let block_pos =
                    BlockPos::new(entry.block_pos[0], entry.block_pos[1], entry.block_pos[2]);
                let template = holding(vec![TemplateEntity {
                    pos: entry.pos,
                    block_pos,
                    nbt: tag("minecraft:cow", entry.yaw, 0.0),
                }]);
                let placed = place(&template, AT, &settings(rotation, mirror)).expect("it places");
                assert_eq!(placed.len(), 1);
                let moved = transform_pos(block_pos, mirror, rotation, PIVOT);
                assert_eq!(
                    placed[0].block,
                    BlockPos::new(moved.x + AT.x, moved.y + AT.y, moved.z + AT.z)
                );
                // An entity stands within a block of its own block however it is turned:
                // the vector's mirror is about the block's far side, the block's about
                // its near one.
                let block = [placed[0].block.x, placed[0].block.y, placed[0].block.z];
                for (axis, (at, block)) in placed[0].position.iter().zip(block).enumerate() {
                    let offset = at - f64::from(block);
                    assert!(
                        (-1.0..=2.0).contains(&offset),
                        "{} {rotation:?} {mirror:?}: axis {axis} {offset}",
                        entry.template
                    );
                }
            }
        }
    }
}

#[test]
fn a_vector_is_mirrored_about_the_far_side_of_its_block_then_turned_about_the_pivot() {
    let pos = [0.75, 1.0, 0.25];
    let pivot = BlockPos::new(1, 0, 2);
    let turned = |rotation, mirror| transform_vec(pos, mirror, rotation, pivot);
    assert_eq!(
        turned(Rotation::None, StructureMirror::None),
        [0.75, 1.0, 0.25]
    );
    // Left to right: z = 1 − 0.25; front to back: x = 1 − 0.75.
    assert_eq!(
        turned(Rotation::None, StructureMirror::LeftRight),
        [0.75, 1.0, 0.75]
    );
    assert_eq!(
        turned(Rotation::None, StructureMirror::FrontBack),
        [0.25, 1.0, 0.25]
    );
    // Clockwise: (px + pz + 1 − z, y, pz − px + x) = (4 − 0.25, 1, 1 + 0.75).
    assert_eq!(
        turned(Rotation::Clockwise90, StructureMirror::None),
        [3.75, 1.0, 1.75]
    );
    // Counter-clockwise: (px − pz + z, y, px + pz + 1 − x) = (−1 + 0.25, 1, 4 − 0.75).
    assert_eq!(
        turned(Rotation::CounterClockwise90, StructureMirror::None),
        [-0.75, 1.0, 3.25]
    );
    // Half a turn: (2px + 1 − x, y, 2pz + 1 − z) = (3 − 0.75, 1, 5 − 0.25).
    assert_eq!(
        turned(Rotation::Clockwise180, StructureMirror::None),
        [2.25, 1.0, 4.75]
    );
    // Mirrored left to right first (z = 0.75), then turned clockwise.
    assert_eq!(
        turned(Rotation::Clockwise90, StructureMirror::LeftRight),
        [3.25, 1.0, 1.75]
    );
}

#[test]
fn a_yaw_is_turned_and_mirrored_from_its_wrapped_self_and_kept_unwrapped() {
    assert_eq!(wrap_degrees(200.0), -160.0);
    assert_eq!(wrap_degrees(-180.0), -180.0);
    assert_eq!(wrap_degrees(180.0), -180.0);
    assert_eq!(wrap_degrees(-540.0), -180.0);
    // rotate(r) + (mirror(m) − yRot): 200 turned clockwise, unmirrored, is
    // (−160 + 90) + (−160 − 200) = −430, which `snapTo` keeps as it is.
    assert_eq!(
        entity_yaw(200.0, StructureMirror::None, Rotation::Clockwise90),
        -430.0
    );
    // Front to back, unturned: 45 + (−45 − 45) = −45.
    assert_eq!(
        entity_yaw(45.0, StructureMirror::FrontBack, Rotation::None),
        -45.0
    );
    // Left to right with half a turn: (90 + 180) + ((180 − 90) − 90) = 270.
    assert_eq!(
        entity_yaw(90.0, StructureMirror::LeftRight, Rotation::Clockwise180),
        270.0
    );
    // Counter-clockwise, unmirrored: (0 + 270) + (0 − 0) = 270.
    assert_eq!(
        entity_yaw(0.0, StructureMirror::None, Rotation::CounterClockwise90),
        270.0
    );
}

/// The writable box of the chunk `(x, 0)`.
fn chunk(x: i32) -> BoundingBox {
    BoundingBox::new(IVec3::new(x * 16, -63, 0), IVec3::new(x * 16 + 15, 319, 15))
}

#[test]
fn the_desert_camel_turned_lands_in_the_chunk_beside_the_one_that_holds_its_block() {
    // `camel_spawn`: its `pos` (0, 0, 0), its `blockPos` (0, 0, 1), its tag `{id}` alone.
    let template = holding(vec![TemplateEntity {
        pos: [0.0, 0.0, 0.0],
        block_pos: BlockPos::new(0, 0, 1),
        nbt: Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:camel".into()))]),
    }]);
    // Turned clockwise about its corner at x 16, the first block of chunk 1: its block
    // goes to x 16 − 1 = 15, in chunk 0, and its position to x 16 + 1 = 17, in chunk 1.
    let at = BlockPos::new(16, 64, 0);
    let mut turned = settings(Rotation::Clockwise90, StructureMirror::None);
    turned.rotation_pivot = BlockPos::new(0, 0, 0);
    turned.bounding_box = Some(chunk(0));
    let placed = place(&template, at, &turned).expect("it places");
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].block, BlockPos::new(15, 64, 0));
    assert_eq!(placed[0].position, [17.0, 64.0, 0.0]);
    // No `Rotation`: a yaw of 0, turned clockwise, 90.
    assert_eq!(placed[0].yaw, 90.0);
    // The run of chunk 1, whose box does not hold its block, places nothing of it.
    turned.bounding_box = Some(chunk(1));
    assert_eq!(place(&template, at, &turned), Some(Vec::new()));
}

#[test]
fn an_entity_whose_block_is_outside_the_box_is_not_placed_and_the_rest_are_in_order() {
    let at_x = |x: f64, block: i32, id: &str| TemplateEntity {
        pos: [x, 1.0, 0.5],
        block_pos: BlockPos::new(block, 1, 0),
        nbt: tag(id, 0.0, 0.0),
    };
    let template = holding(vec![
        at_x(0.5, 0, "minecraft:cow"),
        at_x(6.5, 6, "minecraft:pig"),
        at_x(1.5, 1, "minecraft:sheep"),
    ]);
    // Placed at x 10, the pig's block lies at x 16, in the next chunk.
    let mut clipped = settings(Rotation::None, StructureMirror::None);
    clipped.bounding_box = Some(chunk(0));
    let placed = place(&template, BlockPos::new(10, 64, 0), &clipped).expect("it places");
    let ids: Vec<_> = placed
        .iter()
        .map(|entity| entity.tag.get("id").cloned())
        .collect();
    assert_eq!(
        ids,
        [
            Some(Nbt::String("minecraft:cow".into())),
            Some(Nbt::String("minecraft:sheep".into()))
        ]
    );
}

#[test]
fn ignored_entities_are_not_placed_and_unfinalised_ones_are_handed_back_so() {
    let template = holding(vec![TemplateEntity {
        pos: [0.5, 1.0, 0.5],
        block_pos: BlockPos::new(0, 1, 0),
        nbt: tag("minecraft:villager", 0.0, 0.0),
    }]);
    let mut ignoring = settings(Rotation::None, StructureMirror::None);
    ignoring.ignore_entities = true;
    // With no blocks and its entities ignored the template has nothing to place.
    assert_eq!(place(&template, AT, &ignoring), None);
    let mut kept = settings(Rotation::None, StructureMirror::None);
    kept.finalize_entities = false;
    let placed = place(&template, AT, &kept).expect("it places");
    assert!(!placed[0].finalise);
}

#[test]
fn a_placed_tag_has_its_position_no_uuid_and_its_block_moved() {
    // A cushion of a camp, its `block_pos` where the template was saved.
    let cushion = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:cushion".into())),
        ("color".into(), Nbt::String("red".into())),
        ("block_pos".into(), Nbt::IntArray(vec![-59, -61, -132])),
        (
            "Rotation".into(),
            Nbt::List(vec![Nbt::Float(180.0), Nbt::Float(0.0)]),
        ),
        ("Pos".into(), Nbt::List(vec![Nbt::Double(-58.5); 3])),
        ("UUID".into(), Nbt::IntArray(vec![1, 2, 3, 4])),
    ]);
    let template = holding(vec![TemplateEntity {
        pos: [6.5, 0.9375, 6.5],
        block_pos: BlockPos::new(6, 0, 6),
        nbt: cushion,
    }]);
    let placed = place(
        &template,
        AT,
        &settings(Rotation::Clockwise180, StructureMirror::None),
    )
    .expect("it places");
    let entity = &placed[0];
    // Half a turn about (1, 2): x 2 + 1 − 6.5 = −3.5, z 4 + 1 − 6.5 = −1.5; its block
    // 2 − 6 = −4 and 4 − 6 = −2.
    assert_eq!(entity.position, [28.5, 64.9375, -17.5]);
    assert_eq!(entity.block, BlockPos::new(28, 64, -18));
    assert_eq!(
        entity.tag.get("Pos"),
        Some(&Nbt::List(vec![
            Nbt::Double(28.5),
            Nbt::Double(64.9375),
            Nbt::Double(-17.5)
        ]))
    );
    assert_eq!(entity.tag.get("UUID"), None);
    assert_eq!(
        entity.tag.get("block_pos"),
        Some(&Nbt::IntArray(vec![28, 64, -18]))
    );
    assert_eq!(entity.tag.get("color"), Some(&Nbt::String("red".into())));
    // 180 wraps to −180: half a turn makes it 0, and 0 + (−180 − 180) = −360.
    assert_eq!(entity.yaw, -360.0);
}

#[test]
fn a_template_s_pitch_is_kept_as_set_x_rot_holds_it() {
    let allay = |pitch: f32| TemplateEntity {
        pos: [4.5, 1.05, 4.5],
        block_pos: BlockPos::new(4, 1, 4),
        nbt: tag("minecraft:allay", 270.0, pitch),
    };
    let template = holding(vec![allay(-40.0), allay(400.0), allay(-100.0)]);
    let placed = place(
        &template,
        AT,
        &settings(Rotation::None, StructureMirror::None),
    )
    .expect("it places");
    assert_eq!(placed[0].pitch, -40.0);
    // 400 % 360 is 40; −100 is held to −90.
    assert_eq!(placed[1].pitch, 40.0);
    assert_eq!(placed[2].pitch, -90.0);
    // 270 wraps to −90; unturned and unmirrored, −90 + (−90 − 270) = −450, an angle of
    // the same way, kept unwrapped as `snapTo` keeps it.
    assert_eq!(placed[0].yaw, -450.0);
}
