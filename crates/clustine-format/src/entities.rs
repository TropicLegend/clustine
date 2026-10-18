//! How entities other than players are written: one by itself, a field a tick set, the
//! changes of a commit, and the file of a chunk's entities, which is kept beside the
//! file of its blocks (`docs/adr/0028-entities.md`, sections 2 and 3;
//! `docs/groundwork/n2-plan.md`, sections 4 and 11).
//!
//! All integers are big-endian, and so are the floating-point numbers. An entity is:
//!
//! | Field | Type |
//! |---|---|
//! | Key | u64 |
//! | Type, an index into the game's entity types | u16 |
//! | UUID | u128 |
//! | Position | f64 x, f64 y, f64 z |
//! | Motion | f64 x, f64 y, f64 z |
//! | Yaw, pitch | f32, f32 |
//! | Flags | u8: bit 0 on the ground; the others 0 |
//! | Hops: changes of holder behind it | u32 |
//! | Age: the ticks it has ticked, as of the record's tick | u32 |
//! | Fall distance | f64 |
//! | Fire: ticks left to burn, or below 0 safe from fire | i32 |
//! | Data | a tag u8, then what the tag says |
//!
//! where the data of tag 1, an item, is the item i32, the count i32, the pick-up delay
//! left as of the record's tick u16, u8 1 followed by the thrower's UUID u128, or u8 0
//! where nobody threw it, and its health i16; the data of tag 2, a falling block, is its
//! state u16, the ticks it has fallen i32, and flags u8 (bit 0 it drops as an item where
//! it cannot land, bit 1 its drop is called off; the others 0). Counters are kept relative to the record they are in
//! (section 11 of the plan): regions count their ticks each from their own start.
//!
//! A field a tick set is a tag u8 and its value: 1 the position (3 × f64), 2 the motion
//! (3 × f64), 3 the rotation (yaw f32, pitch f32), 4 on the ground (u8 0 or 1), 5 the
//! data (as above), 6 the age (u32), 7 the fall distance (f64), 8 the fire (i32).
//!
//! A change of an entity is a tag u8 and its body: 1 whole, the entity; 2 gone, its key
//! u64; 3 set, its key u64, a count of fields u32 and the fields.
//!
//! A commit's entity changes are a count of runs (u32), which is not 0, and per run the
//! chunk's x i32 and z i32, a count of changes (u32), which is not 0, and the changes:
//! the changes in the order the tick made them, cut where the chunk changes. A whole
//! entity whose position is not in the chunk it is written under is refused.
//!
//! The entities' file of a chunk is:
//!
//! | Field | Type |
//! |---|---|
//! | Format version | u8 |
//! | Kind | u8, 7 |
//! | Chunk x, z | i32, i32 |
//! | Tick as of which it was saved | u64 |
//! | Epoch of the region that saved it | u64 |
//! | Entities, ascending by key, each once: count, which is not 0, then each | u32 |
//! | CRC-32 of everything before | u32 |
//!
//! A chunk without entities has no such file.

use clustine_world::{
    ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, ItemStack,
};
use uuid::Uuid;

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

/// The kind of the file of a chunk's entities.
pub const KIND_ENTITIES: u8 = 7;

const ON_GROUND: u8 = 1;

const DATA_ITEM: u8 = 1;
const DATA_FALLING_BLOCK: u8 = 2;

const DROP_ITEM: u8 = 1;
const CANCEL_DROP: u8 = 2;

const FIELD_POSITION: u8 = 1;
const FIELD_MOTION: u8 = 2;
const FIELD_ROTATION: u8 = 3;
const FIELD_ON_GROUND: u8 = 4;
const FIELD_DATA: u8 = 5;
const FIELD_AGE: u8 = 6;
const FIELD_FALL_DISTANCE: u8 = 7;
const FIELD_FIRE: u8 = 8;

const CHANGE_WHOLE: u8 = 1;
const CHANGE_GONE: u8 = 2;
const CHANGE_SET: u8 = 3;

/// Bytes an entity takes at the least: an item without a thrower.
const ENTITY_LENGTH: usize = 8 + 2 + 16 + 48 + 8 + 1 + 4 + 4 + 8 + 4 + 1 + 4 + 4 + 2 + 1 + 2;
/// Bytes a change takes at the least: a gone one.
const CHANGE_LENGTH: usize = 9;
/// Bytes a field takes at the least: on the ground.
const FIELD_LENGTH: usize = 2;
/// Bytes a run of changes takes at the least.
const RUN_LENGTH: usize = 12 + CHANGE_LENGTH;

fn put_vector(out: &mut Vec<u8>, vector: [f64; 3]) {
    for coordinate in vector {
        out.extend_from_slice(&coordinate.to_be_bytes());
    }
}

fn take_vector(input: &mut Input<'_>) -> Result<[f64; 3], FormatError> {
    Ok([input.f64()?, input.f64()?, input.f64()?])
}

fn take_flag(input: &mut Input<'_>, what: &'static str) -> Result<bool, FormatError> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(FormatError::Corrupt(what)),
    }
}

fn put_data(out: &mut Vec<u8>, data: &EntityData) {
    match data {
        EntityData::Item {
            stack,
            pick_up_delay,
            thrower,
            health,
        } => {
            out.push(DATA_ITEM);
            out.extend_from_slice(&stack.item.to_be_bytes());
            out.extend_from_slice(&stack.count.to_be_bytes());
            out.extend_from_slice(&pick_up_delay.to_be_bytes());
            match thrower {
                None => out.push(0),
                Some(thrower) => {
                    out.push(1);
                    out.extend_from_slice(&thrower.as_u128().to_be_bytes());
                }
            }
            out.extend_from_slice(&health.to_be_bytes());
        }
        EntityData::FallingBlock {
            state,
            time,
            drop_item,
            cancel_drop,
        } => {
            out.push(DATA_FALLING_BLOCK);
            out.extend_from_slice(&state.to_be_bytes());
            out.extend_from_slice(&time.to_be_bytes());
            let mut flags = 0;
            if *drop_item {
                flags |= DROP_ITEM;
            }
            if *cancel_drop {
                flags |= CANCEL_DROP;
            }
            out.push(flags);
        }
    }
}

fn take_data(input: &mut Input<'_>) -> Result<EntityData, FormatError> {
    match input.u8()? {
        DATA_ITEM => {
            let stack = ItemStack {
                item: input.i32()?,
                count: input.i32()?,
            };
            let pick_up_delay = input.u16()?;
            let thrower = if take_flag(input, "an item's thrower flag")? {
                Some(Uuid::from_u128(input.u128()?))
            } else {
                None
            };
            Ok(EntityData::Item {
                stack,
                pick_up_delay,
                thrower,
                health: input.i16()?,
            })
        }
        DATA_FALLING_BLOCK => {
            let state = input.u16()?;
            let time = input.i32()?;
            let flags = input.u8()?;
            if flags & !(DROP_ITEM | CANCEL_DROP) != 0 {
                return Err(FormatError::Corrupt("a falling block's flags"));
            }
            Ok(EntityData::FallingBlock {
                state,
                time,
                drop_item: flags & DROP_ITEM != 0,
                cancel_drop: flags & CANCEL_DROP != 0,
            })
        }
        _ => Err(FormatError::Corrupt("entity data tag")),
    }
}

/// Appends an entity as the log and the entities' file have it.
pub(crate) fn put_entity(out: &mut Vec<u8>, entity: &Entity) {
    out.extend_from_slice(&entity.key.0.to_be_bytes());
    out.extend_from_slice(&entity.kind.to_be_bytes());
    out.extend_from_slice(&entity.uuid.as_u128().to_be_bytes());
    put_vector(out, entity.position);
    put_vector(out, entity.motion);
    out.extend_from_slice(&entity.yaw.to_be_bytes());
    out.extend_from_slice(&entity.pitch.to_be_bytes());
    out.push(if entity.on_ground { ON_GROUND } else { 0 });
    out.extend_from_slice(&entity.hops.to_be_bytes());
    out.extend_from_slice(&entity.age.to_be_bytes());
    out.extend_from_slice(&entity.fall_distance.to_be_bytes());
    out.extend_from_slice(&entity.fire_ticks.to_be_bytes());
    put_data(out, &entity.data);
}

/// Reads what [`put_entity`] wrote.
pub(crate) fn take_entity(input: &mut Input<'_>) -> Result<Entity, FormatError> {
    let key = EntityId(input.u64()?);
    let kind = input.u16()?;
    let uuid = Uuid::from_u128(input.u128()?);
    let position = take_vector(input)?;
    let motion = take_vector(input)?;
    let (yaw, pitch) = (input.f32()?, input.f32()?);
    let flags = input.u8()?;
    if flags & !ON_GROUND != 0 {
        return Err(FormatError::Corrupt("entity flags"));
    }
    Ok(Entity {
        key,
        kind,
        uuid,
        position,
        motion,
        yaw,
        pitch,
        on_ground: flags & ON_GROUND != 0,
        hops: input.u32()?,
        age: input.u32()?,
        // Read in the order written: struct fields are taken in the order they are here.
        fall_distance: input.f64()?,
        fire_ticks: input.i32()?,
        data: take_data(input)?,
    })
}

fn put_field(out: &mut Vec<u8>, field: &EntityField) {
    match field {
        EntityField::Position(position) => {
            out.push(FIELD_POSITION);
            put_vector(out, *position);
        }
        EntityField::Motion(motion) => {
            out.push(FIELD_MOTION);
            put_vector(out, *motion);
        }
        EntityField::Rotation { yaw, pitch } => {
            out.push(FIELD_ROTATION);
            out.extend_from_slice(&yaw.to_be_bytes());
            out.extend_from_slice(&pitch.to_be_bytes());
        }
        EntityField::OnGround(on_ground) => {
            out.push(FIELD_ON_GROUND);
            out.push(u8::from(*on_ground));
        }
        EntityField::Data(data) => {
            out.push(FIELD_DATA);
            put_data(out, data);
        }
        EntityField::Age(age) => {
            out.push(FIELD_AGE);
            out.extend_from_slice(&age.to_be_bytes());
        }
        EntityField::FallDistance(distance) => {
            out.push(FIELD_FALL_DISTANCE);
            out.extend_from_slice(&distance.to_be_bytes());
        }
        EntityField::FireTicks(ticks) => {
            out.push(FIELD_FIRE);
            out.extend_from_slice(&ticks.to_be_bytes());
        }
    }
}

fn take_field(input: &mut Input<'_>) -> Result<EntityField, FormatError> {
    Ok(match input.u8()? {
        FIELD_POSITION => EntityField::Position(take_vector(input)?),
        FIELD_MOTION => EntityField::Motion(take_vector(input)?),
        FIELD_ROTATION => EntityField::Rotation {
            yaw: input.f32()?,
            pitch: input.f32()?,
        },
        FIELD_ON_GROUND => EntityField::OnGround(take_flag(input, "an entity's on-ground field")?),
        FIELD_DATA => EntityField::Data(take_data(input)?),
        FIELD_AGE => EntityField::Age(input.u32()?),
        FIELD_FALL_DISTANCE => EntityField::FallDistance(input.f64()?),
        FIELD_FIRE => EntityField::FireTicks(input.i32()?),
        _ => return Err(FormatError::Corrupt("entity field tag")),
    })
}

fn put_change(out: &mut Vec<u8>, change: &EntityChange) {
    match change {
        EntityChange::Whole(entity) => {
            out.push(CHANGE_WHOLE);
            put_entity(out, entity);
        }
        EntityChange::Gone(key) => {
            out.push(CHANGE_GONE);
            out.extend_from_slice(&key.0.to_be_bytes());
        }
        EntityChange::Set(key, fields) => {
            out.push(CHANGE_SET);
            out.extend_from_slice(&key.0.to_be_bytes());
            out.extend_from_slice(&(fields.len() as u32).to_be_bytes());
            for field in fields {
                put_field(out, field);
            }
        }
    }
}

/// Reads a change written under `chunk`: a whole entity elsewhere is refused, since its
/// chunk is where its position is.
fn take_change(input: &mut Input<'_>, chunk: ChunkPos) -> Result<EntityChange, FormatError> {
    Ok(match input.u8()? {
        CHANGE_WHOLE => {
            let entity = take_entity(input)?;
            if entity.chunk() != chunk {
                return Err(FormatError::Corrupt(
                    "a whole entity that is not in the chunk it is written under",
                ));
            }
            EntityChange::Whole(entity)
        }
        CHANGE_GONE => EntityChange::Gone(EntityId(input.u64()?)),
        CHANGE_SET => {
            let key = EntityId(input.u64()?);
            let count = input.u32()? as usize;
            // A forged count must not reserve more than the payload can hold.
            let mut fields = Vec::with_capacity(count.min(input.0.len() / FIELD_LENGTH));
            for _ in 0..count {
                fields.push(take_field(input)?);
            }
            EntityChange::Set(key, fields)
        }
        _ => return Err(FormatError::Corrupt("entity change tag")),
    })
}

/// Appends a commit's entity changes, which are not none.
pub(crate) fn put_entity_changes(out: &mut Vec<u8>, changes: &[(ChunkPos, EntityChange)]) {
    let runs: Vec<&[(ChunkPos, EntityChange)]> = changes
        .chunk_by(|(one, _), (other, _)| one == other)
        .collect();
    out.extend_from_slice(&(runs.len() as u32).to_be_bytes());
    for run in runs {
        let chunk = run[0].0;
        out.extend_from_slice(&chunk.x.to_be_bytes());
        out.extend_from_slice(&chunk.z.to_be_bytes());
        out.extend_from_slice(&(run.len() as u32).to_be_bytes());
        for (_, change) in run {
            put_change(out, change);
        }
    }
}

/// Reads what [`put_entity_changes`] wrote: none is refused, as an empty run is, since
/// a commit without entity changes is of another kind.
pub(crate) fn take_entity_changes(
    input: &mut Input<'_>,
) -> Result<Vec<(ChunkPos, EntityChange)>, FormatError> {
    let runs = input.u32()? as usize;
    if runs == 0 {
        return Err(FormatError::Corrupt(
            "a commit with entity changes that has none",
        ));
    }
    let mut changes = Vec::with_capacity(runs.min(input.0.len() / RUN_LENGTH));
    for _ in 0..runs {
        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
        let count = input.u32()? as usize;
        if count == 0 {
            return Err(FormatError::Corrupt(
                "a chunk's entity changes that are none",
            ));
        }
        changes.reserve(count.min(input.0.len() / CHANGE_LENGTH));
        for _ in 0..count {
            changes.push((chunk, take_change(input, chunk)?));
        }
    }
    Ok(changes)
}

/// The entities of a chunk as they are stored, beside the file of its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntitiesFile {
    pub position: ChunkPos,
    /// The tick as of which they were saved.
    pub tick: u64,
    /// The epoch of the region that saved them.
    pub epoch: u64,
    /// Ascending by key, each once, and not none.
    pub entities: Vec<Entity>,
}

impl EntitiesFile {
    /// The bytes of the file.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![FORMAT_VERSION, KIND_ENTITIES];
        out.extend_from_slice(&self.position.x.to_be_bytes());
        out.extend_from_slice(&self.position.z.to_be_bytes());
        out.extend_from_slice(&self.tick.to_be_bytes());
        out.extend_from_slice(&self.epoch.to_be_bytes());
        out.extend_from_slice(&(self.entities.len() as u32).to_be_bytes());
        for entity in &self.entities {
            put_entity(&mut out, entity);
        }
        let checksum = crc32fast::hash(&out);
        out.extend_from_slice(&checksum.to_be_bytes());
        out
    }

    /// Reads a file. One with no entities, with two of one key or out of order, or with
    /// an entity that is not in its chunk is refused: none is ever written.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (front, checksum) = bytes
            .split_last_chunk::<4>()
            .ok_or(FormatError::Truncated)?;
        if crc32fast::hash(front) != u32::from_be_bytes(*checksum) {
            return Err(FormatError::ChecksumMismatch);
        }
        let mut input = Input(front);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        if input.u8()? != KIND_ENTITIES {
            return Err(FormatError::Corrupt("not an entities' file"));
        }
        let position = ChunkPos::new(input.i32()?, input.i32()?);
        let tick = input.u64()?;
        let epoch = input.u64()?;
        let count = input.u32()? as usize;
        if count == 0 {
            return Err(FormatError::Corrupt("an entities' file without entities"));
        }
        let mut entities: Vec<Entity> =
            Vec::with_capacity(count.min(input.0.len() / ENTITY_LENGTH));
        for _ in 0..count {
            let entity = take_entity(&mut input)?;
            if entity.chunk() != position {
                return Err(FormatError::Corrupt(
                    "an entity that is not in the chunk of its file",
                ));
            }
            if entities.last().is_some_and(|last| last.key >= entity.key) {
                return Err(FormatError::Corrupt(
                    "entities not ascending by key, each once",
                ));
            }
            entities.push(entity);
        }
        input.finish()?;
        Ok(Self {
            position,
            tick,
            epoch,
            entities,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use proptest::prelude::*;

    use super::*;

    pub(crate) fn item(key: u64, position: [f64; 3], thrower: Option<u128>) -> Entity {
        Entity {
            key: EntityId(key),
            kind: 71,
            uuid: Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10),
            position,
            motion: [0.5, -0.25, 0.0],
            yaw: 90.0,
            pitch: -1.5,
            on_ground: true,
            hops: 3,
            age: 12,
            data: EntityData::Item {
                stack: ItemStack { item: 1, count: 64 },
                pick_up_delay: 40,
                thrower: thrower.map(Uuid::from_u128),
                health: 5,
            },
            fall_distance: 0.0,
            fire_ticks: -1,
        }
    }

    /// The bytes of section 4 of the plan, with section 11's counters, field by field.
    #[test]
    fn known_answer_of_an_entities_file() {
        let file = EntitiesFile {
            position: ChunkPos::new(-1, 2),
            tick: 5,
            epoch: 9,
            entities: vec![item(0x1_0000_0002, [-0.5, 64.0, 32.25], Some(7))],
        };
        let bytes = file.encode();
        let mut expected = vec![FORMAT_VERSION, 7];
        expected.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 2]); // x, z
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 5]); // tick
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 9]); // epoch
        expected.extend_from_slice(&[0, 0, 0, 1]); // one entity
        expected.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 2]); // key
        expected.extend_from_slice(&[0, 71]); // kind
        expected.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        expected.extend_from_slice(&[0xBF, 0xE0, 0, 0, 0, 0, 0, 0]); // x -0.5
        expected.extend_from_slice(&[0x40, 0x50, 0, 0, 0, 0, 0, 0]); // y 64
        expected.extend_from_slice(&[0x40, 0x40, 0x20, 0, 0, 0, 0, 0]); // z 32.25
        expected.extend_from_slice(&[0x3F, 0xE0, 0, 0, 0, 0, 0, 0]); // motion 0.5
        expected.extend_from_slice(&[0xBF, 0xD0, 0, 0, 0, 0, 0, 0]); // -0.25
        expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // 0
        expected.extend_from_slice(&[0x42, 0xB4, 0, 0]); // yaw 90
        expected.extend_from_slice(&[0xBF, 0xC0, 0, 0]); // pitch -1.5
        expected.push(1); // on the ground
        expected.extend_from_slice(&[0, 0, 0, 3]); // hops
        expected.extend_from_slice(&[0, 0, 0, 12]); // age
        expected.extend_from_slice(&[0; 8]); // fall distance 0
        expected.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // fire -1
        expected.push(1); // an item
        expected.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 64]); // item, count
        expected.extend_from_slice(&[0, 40]); // pick-up delay
        expected.push(1); // a thrower
        expected.extend_from_slice(&7u128.to_be_bytes());
        expected.extend_from_slice(&[0, 5]); // health
        let checksum = crc32fast::hash(&expected);
        expected.extend_from_slice(&checksum.to_be_bytes());
        assert_eq!(bytes, expected);
        assert_eq!(EntitiesFile::decode(&bytes), Ok(file));
    }

    #[test]
    fn an_entities_file_that_makes_no_sense_is_refused() {
        let at = [1.5, 64.0, 1.5];
        let file = |entities| EntitiesFile {
            position: ChunkPos::new(0, 0),
            tick: 1,
            epoch: 1,
            entities,
        };
        // Out of order, twice, none, and an entity of another chunk.
        for entities in [
            vec![item(5, at, None), item(4, at, None)],
            vec![item(5, at, None), item(5, at, None)],
            Vec::new(),
            vec![item(5, [16.0, 64.0, 0.0], None)],
        ] {
            assert!(EntitiesFile::decode(&file(entities.clone()).encode()).is_err());
        }
        let bytes = file(vec![item(4, at, None)]).encode();
        for length in 0..bytes.len() {
            assert!(
                EntitiesFile::decode(&bytes[..length]).is_err(),
                "cut at {length}"
            );
        }
        for index in 0..bytes.len() {
            let mut damaged = bytes.clone();
            damaged[index] ^= 0x10;
            assert!(EntitiesFile::decode(&damaged).is_err(), "byte {index}");
        }
    }

    #[test]
    fn a_whole_entity_written_under_another_chunk_is_refused() {
        let mut out = Vec::new();
        put_entity_changes(
            &mut out,
            &[(
                ChunkPos::new(1, 0),
                EntityChange::Whole(item(1, [15.99, 0.0, 0.0], None)),
            )],
        );
        assert!(take_entity_changes(&mut Input(&out)).is_err());
        // An empty list is never written.
        assert!(take_entity_changes(&mut Input(&0u32.to_be_bytes())).is_err());
    }

    pub(crate) fn entities() -> impl Strategy<Value = Entity> {
        (
            any::<u64>(),
            any::<u16>(),
            any::<u128>(),
            (-1e7..1e7f64, -2048.0..2048.0f64, -1e7..1e7f64),
            any::<[f64; 3]>(),
            (any::<f32>(), any::<f32>(), any::<bool>()),
            (any::<u32>(), any::<u32>(), any::<f64>(), any::<i32>()),
            datas(),
        )
            .prop_map(
                |(
                    key,
                    kind,
                    uuid,
                    (x, y, z),
                    motion,
                    (yaw, pitch, on_ground),
                    (hops, age, fall_distance, fire_ticks),
                    data,
                )| {
                    Entity {
                        key: EntityId(key),
                        kind,
                        uuid: Uuid::from_u128(uuid),
                        position: [x, y, z],
                        motion,
                        yaw,
                        pitch,
                        on_ground,
                        hops,
                        age,
                        data,
                        fall_distance,
                        fire_ticks,
                    }
                },
            )
    }

    fn datas() -> impl Strategy<Value = EntityData> {
        prop_oneof![
            (
                any::<i32>(),
                any::<i32>(),
                any::<u16>(),
                any::<Option<u128>>(),
                any::<i16>(),
            )
                .prop_map(|(item, count, pick_up_delay, thrower, health)| {
                    EntityData::Item {
                        stack: ItemStack { item, count },
                        pick_up_delay,
                        thrower: thrower.map(Uuid::from_u128),
                        health,
                    }
                }),
            (any::<u16>(), any::<i32>(), any::<bool>(), any::<bool>()).prop_map(
                |(state, time, drop_item, cancel_drop)| EntityData::FallingBlock {
                    state,
                    time,
                    drop_item,
                    cancel_drop,
                }
            ),
        ]
    }

    fn fields() -> impl Strategy<Value = EntityField> {
        prop_oneof![
            any::<[f64; 3]>().prop_map(EntityField::Position),
            any::<[f64; 3]>().prop_map(EntityField::Motion),
            (any::<f32>(), any::<f32>())
                .prop_map(|(yaw, pitch)| EntityField::Rotation { yaw, pitch }),
            any::<bool>().prop_map(EntityField::OnGround),
            any::<u32>().prop_map(EntityField::Age),
            datas().prop_map(EntityField::Data),
            any::<f64>().prop_map(EntityField::FallDistance),
            any::<i32>().prop_map(EntityField::FireTicks),
        ]
    }

    /// Changes of any kind, each under the chunk a whole entity is in.
    pub(crate) fn entity_changes() -> impl Strategy<Value = Vec<(ChunkPos, EntityChange)>> {
        let chunk = (-3..3, -3..3).prop_map(|(x, z)| ChunkPos::new(x, z));
        let change = prop_oneof![
            entities().prop_map(|entity| (entity.chunk(), EntityChange::Whole(entity))),
            (chunk.clone(), any::<u64>())
                .prop_map(|(chunk, key)| (chunk, EntityChange::Gone(EntityId(key)))),
            (chunk, any::<u64>(), prop::collection::vec(fields(), 0..5))
                .prop_map(|(chunk, key, fields)| (chunk, EntityChange::Set(EntityId(key), fields))),
        ];
        prop::collection::vec(change, 1..20)
    }

    proptest! {
        #[test]
        fn entity_changes_round_trip_in_their_order(changes in entity_changes()) {
            let mut out = Vec::new();
            put_entity_changes(&mut out, &changes);
            let mut input = Input(&out);
            prop_assert_eq!(take_entity_changes(&mut input), Ok(changes));
            prop_assert!(input.0.is_empty());
        }

        #[test]
        fn entities_files_round_trip(
            x in -1000..1000i32,
            z in -1000..1000i32,
            tick: u64,
            epoch: u64,
            entities in prop::collection::btree_map(any::<u64>(), entities(), 1..20),
        ) {
            let position = ChunkPos::new(x, z);
            let entities: Vec<Entity> = entities
                .into_iter()
                .map(|(key, mut entity)| {
                    entity.key = EntityId(key);
                    // Into the file's chunk, wherever in it.
                    entity.position[0] = f64::from(x) * 16.0 + entity.position[0].rem_euclid(16.0);
                    entity.position[2] = f64::from(z) * 16.0 + entity.position[2].rem_euclid(16.0);
                    entity
                })
                .filter(|entity| entity.chunk() == position)
                .collect();
            prop_assume!(!entities.is_empty());
            let file = EntitiesFile { position, tick, epoch, entities };
            let bytes = file.encode();
            prop_assert_eq!(EntitiesFile::decode(&bytes), Ok(file));
        }

        #[test]
        fn arbitrary_bytes_never_make_an_entities_file_panic(bytes: Vec<u8>) {
            let _ = EntitiesFile::decode(&bytes);
            let _ = take_entity_changes(&mut Input(&bytes));
        }
    }
}
