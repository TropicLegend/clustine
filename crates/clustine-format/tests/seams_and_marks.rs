//! Tests of what step B6 adds to the world format, written from sections 4 to 6 of
//! `docs/adr/0024-block-behaviour-in-a-region.md` by someone who did not write the code:
//! a chunk file carries a chunk's seam entries behind bit 1 of its flags, and a chunk
//! without any is written exactly as before there were seams; the log's commit with
//! records of chunks (kind 9) carries the chunks whose marks were taken and the seam
//! entries of chunks whose entries changed, whole, each of them alone.

use clustine_data::{BlockState, DIMENSION_TYPES, blocks};
use clustine_format::{ChunkFile, LogRecord, read_log};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, InChunk, ScheduledTick, Seam, SeamKind, Ticked,
};

/// Where a chunk file keeps its flags: after the version and kind (one byte each), the
/// position (two i32), the lowest y (i32), the tick and the epoch (u64 each) and the
/// generator's version (u16).
const FLAGS: usize = 32;
const WITH_SEAMS: u8 = 2;

/// A chunk with blocks, a tick and marks, and no seam entries.
fn chunk_without_seams() -> Chunk {
    let overworld = DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .unwrap();
    let mut chunk = Chunk::empty(overworld, Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -64, z, blocks::BEDROCK);
            chunk.set(x, -63, z, blocks::DIRT);
        }
    }
    chunk.set(3, -62, 5, blocks::OAK_FENCE);
    chunk.set(4, -62, 5, blocks::WATER);
    chunk.set_ticks(vec![ScheduledTick {
        at: InChunk::new(4, -62, 5),
        of: Ticked::Fluid(1),
        delay: 3,
        priority: -1,
    }]);
    chunk.set_marks(vec![InChunk::new(3, -62, 5), InChunk::new(4, -62, 5)]);
    chunk
}

fn file(chunk: Chunk) -> ChunkFile {
    ChunkFile {
        position: ChunkPos::new(-7, 12),
        tick: 4242,
        epoch: 3,
        chunk,
    }
}

/// [`chunk_without_seams`] in a file as the format wrote it at the commit before seam
/// entries were added to it (57e11ae), encoded there by the same code as here.
const BEFORE_SEAMS: &str = "0205fffffff90000000cffffffc000000000000010920000000000000003000000\
001801000006ee0000005628b52ffd60ee05650200940200050000000a0058005921da249249129492490003800\
001010145ffc203ff0000000235ffc245ffc20d002e231497c54685c26eb836817f4005f0805b6f03842bc7b420\
48ad0c60ebeaf580daaf";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn seam(at: BlockPos, direction: u8, kind: SeamKind, source: BlockState) -> Seam {
    Seam {
        at,
        direction,
        kind,
        source,
        flags: 2,
        depth: 511,
    }
}

/// Entries of both kinds, of every direction, reaching beyond the chunk on every side,
/// at the bottom and the top of the world, with flags and depths of every sort.
fn some_seams() -> Vec<Seam> {
    let mut seams = Vec::new();
    for direction in 0..6 {
        let at = BlockPos::new(-113 + i32::from(direction), -64, 192);
        seams.push(seam(at, direction, SeamKind::Shape, blocks::AIR));
        let mut told = seam(at, direction, SeamKind::Neighbour, blocks::STONE);
        told.flags = 1;
        told.depth = 0;
        seams.push(told);
    }
    let mut deep = seam(
        BlockPos::new(-96, 319, 207),
        5,
        SeamKind::Shape,
        blocks::WATER,
    );
    deep.flags = 0xFFFF_FFFF;
    deep.depth = i32::MIN;
    seams.push(deep);
    seams
}

/// A chunk without seam entries is written byte for byte as before there were any, its
/// flag clear, and is read back without any.
#[test]
fn a_chunk_without_seams_is_written_byte_for_byte_as_before() {
    let file = file(chunk_without_seams());
    let bytes = file.encode();
    assert_eq!(hex(&bytes), BEFORE_SEAMS);
    assert_eq!(bytes[FLAGS] & WITH_SEAMS, 0);
    let (read, _) = ChunkFile::decode(&bytes).unwrap();
    assert_eq!(read, file);
    assert_eq!(read.chunk.seams(), []);
}

/// A chunk whose entries have all been made is written as one that never had any.
#[test]
fn a_chunk_whose_seams_are_gone_is_written_as_one_that_never_had_any() {
    let mut chunk = chunk_without_seams();
    chunk.set_seams(some_seams());
    chunk.set_seams(Vec::new());
    assert_eq!(hex(&file(chunk).encode()), BEFORE_SEAMS);
}

/// A chunk with seam entries says so in bit 1 of its flags and is read back with them,
/// in the order of their keys; written again, it is the same bytes.
#[test]
fn a_chunk_with_seams_round_trips_through_a_chunk_file() {
    let mut chunk = chunk_without_seams();
    let mut seams = some_seams();
    // Given out of order, kept in the order of their keys.
    seams.reverse();
    chunk.set_seams(seams);
    let file = file(chunk);
    let bytes = file.encode();
    assert_eq!(bytes[FLAGS] & WITH_SEAMS, WITH_SEAMS);
    let (read, _) = ChunkFile::decode(&bytes).unwrap();
    assert_eq!(read, file);
    let mut expected = some_seams();
    expected.sort_by_key(Seam::key);
    assert_eq!(read.chunk.seams(), expected);
    assert_eq!(read.chunk.marks(), file.chunk.marks());
    assert_eq!(read.chunk.ticks(), file.chunk.ticks());
    assert_eq!(read.encode(), bytes);
}

/// The flag of a failure and that of seams are two bits: a chunk with both has both,
/// and is read back with both.
#[test]
fn a_chunk_with_a_failure_and_seams_keeps_both_flags() {
    let mut chunk = chunk_without_seams();
    chunk.mark_generated(7, true);
    chunk.set_seams(some_seams());
    let file = file(chunk);
    let bytes = file.encode();
    assert_eq!(bytes[FLAGS], 1 | WITH_SEAMS);
    let (read, _) = ChunkFile::decode(&bytes).unwrap();
    assert_eq!(read, file);
    assert!(read.chunk.with_failure());
    assert_eq!(read.chunk.generated(), 7);
}

/// One entry per block reached, direction and kind: of two with one key the chunk keeps
/// the later, and so does the file.
#[test]
fn of_two_entries_with_one_key_the_later_is_kept_and_written() {
    let at = BlockPos::new(-97, 10, 200);
    let earlier = seam(at, 4, SeamKind::Shape, blocks::STONE);
    let later = seam(at, 4, SeamKind::Shape, blocks::DIRT);
    let other_kind = seam(at, 4, SeamKind::Neighbour, blocks::STONE);
    let mut chunk = chunk_without_seams();
    chunk.set_seams(vec![earlier, other_kind, later]);
    assert_eq!(chunk.seams(), [later, other_kind]);
    chunk.add_seam(earlier);
    assert_eq!(chunk.seams(), [earlier, other_kind]);
    let (read, _) = ChunkFile::decode(&file(chunk).encode()).unwrap();
    assert_eq!(read.chunk.seams(), [earlier, other_kind]);
}

/// A commit as a tick without stay notes or ticks makes it, with `marks_taken` and
/// `seams`.
fn commit(marks_taken: Vec<ChunkPos>, seams: Vec<(ChunkPos, Vec<Seam>)>) -> LogRecord {
    LogRecord::Commit {
        region: 3,
        tick: 77,
        epoch: 5,
        changes: vec![(BlockPos::new(1, -60, 2), blocks::OAK_FENCE)],
        state: b"a delta".to_vec(),
        stays: Vec::new(),
        ticks: Vec::new(),
        marks_taken,
        seams,
        effects: Vec::new(),
        taken: Vec::new(),
    }
}

/// The kind a framed record is written as: the byte after the record's version, which
/// follows the frame's header of eight bytes.
fn kind_of(bytes: &[u8]) -> u8 {
    bytes[9]
}

/// A commit whose only records of chunks are the marks taken is a commit with records
/// of chunks (kind 9), and is read back as it was.
#[test]
fn a_commit_with_only_marks_taken_round_trips_as_kind_nine() {
    let record = commit(vec![ChunkPos::new(-3, 0), ChunkPos::new(2, -1)], Vec::new());
    let bytes = record.encode();
    assert_eq!(kind_of(&bytes), 9);
    assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
}

/// A commit whose only records of chunks are seam entries is of kind 9 too, and is read
/// back as it was: also a chunk whose entries all went, which a commit carries as an
/// empty list, so that a replay empties them.
#[test]
fn a_commit_with_only_seams_round_trips_as_kind_nine() {
    let mut seams = some_seams();
    seams.sort_by_key(Seam::key);
    let record = commit(
        Vec::new(),
        vec![
            (ChunkPos::new(-7, 12), seams),
            (ChunkPos::new(4, 4), Vec::new()),
        ],
    );
    let bytes = record.encode();
    assert_eq!(kind_of(&bytes), 9);
    assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));

    let emptied = commit(Vec::new(), vec![(ChunkPos::new(0, 0), Vec::new())]);
    let bytes = emptied.encode();
    assert_eq!(kind_of(&bytes), 9);
    assert_eq!(read_log(&bytes), Ok((vec![emptied], bytes.len())));
}

/// A commit with ticks, marks taken and seams together is read back with all three,
/// and one with none of them is not of kind 9.
#[test]
fn a_commit_with_every_record_of_chunks_round_trips_and_one_without_is_not_kind_nine() {
    let mut record = commit(
        vec![ChunkPos::new(0, 0)],
        vec![(
            ChunkPos::new(0, 0),
            vec![seam(
                BlockPos::new(16, -60, 3),
                4,
                SeamKind::Shape,
                blocks::AIR,
            )],
        )],
    );
    if let LogRecord::Commit { ticks, .. } = &mut record {
        ticks.push((
            ChunkPos::new(0, 0),
            vec![ScheduledTick {
                at: InChunk::new(1, -60, 2),
                of: Ticked::Block(9),
                delay: -1,
                priority: 2,
            }],
        ));
    }
    let bytes = record.encode();
    assert_eq!(kind_of(&bytes), 9);
    assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));

    let plain = commit(Vec::new(), Vec::new());
    let bytes = plain.encode();
    assert_ne!(kind_of(&bytes), 9);
    assert_eq!(read_log(&bytes), Ok((vec![plain], bytes.len())));
}
