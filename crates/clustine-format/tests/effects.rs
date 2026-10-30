//! What step E2 adds to the world format (`docs/adr/0025-effects-between-regions.md`):
//! a commit that carries effects or takes any is kind 10, and one that does neither is
//! written as before; the store's records of effects dropped and written forward; and
//! the couplings' file.

use clustine_data::blocks;
use clustine_format::{CoupledRegion, CouplingsFile, FormatError, Handshake, LogRecord, read_log};
use clustine_world::{
    BlockPos, ChunkPos, Due, Effect, EffectId, Effected, FiledEffect, SeamKind, Ticked,
};

/// One effect of every kind, each with when it is due.
fn every_kind_of_effect() -> Vec<(Effect, Due)> {
    let at = |x, y, z| BlockPos::new(x, y, z);
    vec![
        (
            Effect {
                at: at(-17, 64, 3),
                what: Effected::Update {
                    direction: 4,
                    kind: SeamKind::Neighbour,
                    source: blocks::AIR,
                    flags: 3,
                    depth: 511,
                },
            },
            Due::FromTaking,
        ),
        (
            Effect {
                at: at(16, -64, -1),
                what: Effected::Update {
                    direction: 1,
                    kind: SeamKind::Shape,
                    source: blocks::STONE,
                    flags: 2,
                    depth: 0,
                },
            },
            Due::FromTaking,
        ),
        (
            Effect {
                at: at(31, 70, 31),
                what: Effected::Tick {
                    of: Ticked::Fluid(2),
                    delay: 5,
                    priority: -1,
                },
            },
            Due::At { tick: 98_765 },
        ),
        (
            Effect {
                at: at(0, 319, 0),
                what: Effected::Tick {
                    of: Ticked::Block(blocks::SAND.0),
                    delay: 2,
                    priority: 0,
                },
            },
            Due::FromTaking,
        ),
        (
            Effect {
                at: at(-1, 62, 15),
                what: Effected::Spread {
                    direction: 5,
                    source: blocks::WATER,
                },
            },
            Due::FromTaking,
        ),
        (
            Effect {
                at: at(7, 80, -9),
                what: Effected::SetIf {
                    expected: blocks::OAK_PLANKS,
                    state: blocks::FIRE,
                    flags: 3,
                },
            },
            Due::FromTaking,
        ),
    ]
}

fn commit(effects: Vec<(Effect, Due)>, taken: Vec<EffectId>) -> LogRecord {
    LogRecord::Commit {
        travellers: Vec::new(),
        region: 3,
        tick: 4_000,
        epoch: 9,
        changes: vec![(BlockPos::new(1, 2, 3), blocks::STONE).into()],
        state: vec![1, 2, 3],
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects,
        taken,
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        claims: Vec::new(),
        raids: Vec::new(),
    }
}

fn id(sequence: u64, index: u32) -> EffectId {
    EffectId { sequence, index }
}

/// The kind of a record: after the frame (length and checksum, four bytes each) and the
/// format version.
fn kind_of(bytes: &[u8]) -> u8 {
    bytes[9]
}

#[test]
fn a_commit_with_every_kind_of_effect_and_takings_round_trips_as_kind_ten() {
    let record = commit(
        every_kind_of_effect(),
        vec![id(7, 0), id(7, 3), id(1 << 40, 9)],
    );
    let bytes = record.encode();
    assert_eq!(kind_of(&bytes), 10);
    assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
}

#[test]
fn a_commit_that_only_takes_effects_is_kind_ten() {
    let record = commit(Vec::new(), vec![id(12, 1)]);
    let bytes = record.encode();
    assert_eq!(kind_of(&bytes), 10);
    assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
}

#[test]
fn a_commit_without_effects_or_takings_is_of_the_kind_it_always_was() {
    let plain = commit(Vec::new(), Vec::new());
    assert_eq!(kind_of(&plain.encode()), 2);
    let LogRecord::Commit {
        region,
        tick,
        epoch,
        changes,
        state,
        stays,
        ..
    } = plain
    else {
        unreachable!()
    };
    let with_marks = LogRecord::Commit {
        travellers: Vec::new(),
        region,
        tick,
        epoch,
        changes,
        state,
        stays,
        ticks: Vec::new(),
        marks_taken: vec![ChunkPos::new(0, 0)],
        seams: Vec::new(),
        effects: Vec::new(),
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes: Vec::new(),
        arrivals: Vec::new(),
        arrivals_taken: Vec::new(),
        claims: Vec::new(),
        raids: Vec::new(),
    };
    assert_eq!(kind_of(&with_marks.encode()), 9);
}

#[test]
fn effects_dropped_and_written_forward_round_trip() {
    let forward: Vec<FiledEffect> = every_kind_of_effect()
        .into_iter()
        .enumerate()
        .map(|(index, (effect, due))| FiledEffect {
            id: id(500, index as u32),
            effect,
            due,
        })
        .collect();
    let records = vec![
        LogRecord::EffectsDropped {
            ids: vec![id(3, 0), id(3, 1), id(400, 0)],
        },
        LogRecord::EffectsForward { effects: forward },
        LogRecord::EffectsDropped { ids: Vec::new() },
    ];
    let bytes: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
    assert_eq!(read_log(&bytes), Ok((records, bytes.len())));
}

#[test]
fn records_with_effects_cut_off_anywhere_end_the_log_at_the_last_whole_one() {
    let first = commit(every_kind_of_effect(), vec![id(2, 0)]);
    let second = LogRecord::EffectsDropped {
        ids: vec![id(9, 4)],
    };
    let first_bytes = first.encode();
    let bytes: Vec<u8> = [first_bytes.clone(), second.encode()].concat();
    for cut in first_bytes.len()..bytes.len() {
        assert_eq!(
            read_log(&bytes[..cut]),
            Ok((vec![first.clone()], first_bytes.len())),
            "cut at {cut}"
        );
    }
    for cut in 0..first_bytes.len() {
        assert_eq!(read_log(&bytes[..cut]), Ok((Vec::new(), 0)), "cut at {cut}");
    }
}

fn couplings() -> CouplingsFile {
    CouplingsFile {
        regions: vec![
            CoupledRegion {
                region: 1,
                offset: Some(-4_000),
                given_up: false,
            },
            CoupledRegion {
                region: 4,
                offset: Some(i64::MAX),
                given_up: false,
            },
            CoupledRegion {
                region: 9,
                offset: None,
                given_up: true,
            },
        ],
        couplings: vec![(1, 4), (1, 7), (4, 7)],
        handshakes: vec![
            Handshake {
                low: 2,
                high: 9,
                low_joins: Some(100),
                high_joins: None,
            },
            Handshake {
                low: 3,
                high: 5,
                low_joins: None,
                high_joins: None,
            },
        ],
    }
}

#[test]
fn the_couplings_file_round_trips_and_an_empty_one_too() {
    for file in [couplings(), CouplingsFile::default()] {
        assert_eq!(CouplingsFile::decode(&file.encode()), Ok(file));
    }
}

#[test]
fn a_couplings_file_out_of_order_or_damaged_is_refused() {
    let mut out_of_order = couplings();
    out_of_order.couplings.swap(0, 1);
    assert_eq!(
        CouplingsFile::decode(&out_of_order.encode()),
        Err(FormatError::Corrupt("couplings out of order"))
    );
    let mut backwards = couplings();
    backwards.couplings = vec![(4, 1)];
    assert_eq!(
        CouplingsFile::decode(&backwards.encode()),
        Err(FormatError::Corrupt("couplings out of order"))
    );
    let mut regions = couplings();
    regions.regions.swap(0, 2);
    assert_eq!(
        CouplingsFile::decode(&regions.encode()),
        Err(FormatError::Corrupt("coupled regions out of order"))
    );
    let mut damaged = couplings().encode();
    damaged[5] ^= 1;
    assert!(CouplingsFile::decode(&damaged).is_err());
    let whole = couplings().encode();
    for cut in 0..whole.len() {
        assert!(
            CouplingsFile::decode(&whole[..cut]).is_err(),
            "cut at {cut}"
        );
    }
}
