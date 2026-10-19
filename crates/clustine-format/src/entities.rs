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
//! | Lived: the ticks it has ticked, as of the record's tick | u32 |
//! | Fresh at: `lived` at its last fresh load | u32 |
//! | Fall distance | f64 |
//! | Fire: ticks left to burn, or below 0 safe from fire | i32 |
//! | Riding: its vehicle, cooldown, passengers and leash | see `put_riding` |
//! | Data | a tag u8, then what the tag says |
//!
//! where the data of tag 1, an item, is the item i32, the count i32, the pick-up delay
//! left as of the record's tick u16, u8 1 followed by the thrower's UUID u128, or u8 0
//! where nobody threw it, its health i16, and what `lived` is ahead of its age u32; the data of tag 2, a falling block, is its
//! state u16, the ticks it has fallen i32, and flags u8 (bit 0 it drops as an item where
//! it cannot land, bit 1 its drop is called off; the others 0). Tags 3 to 9 are N5's
//! kinds (`docs/groundwork/n5-plan.md`): 3 a minecart, 4 a boat, 5 an armour stand, 6 an
//! item frame, 7 a painting, 8 a cushion, 9 a leash's knot, and tag 10 is N6's primed
//! TNT (`docs/groundwork/n6-plan.md`) and tag 11 is N7's mob (`docs/groundwork/n7-plan.md`,
//! section 11.3), each written by `put_data` in the order of its
//! fields (a minecart's TNT fuse i32 and a hopper cart's switch u8 after its push; the
//! TNT's fuse i16, block u16, power f32 and owner as an item's thrower); a block-attached entity's attachment is its block (3 × i32)
//! and direction (u8, 0 to 5), a container's contents a count u8 and per stack its slot
//! u8, item i32 and count i32, ascending by slot. Counters are kept relative to the record they are in
//! (section 11 of the plan): regions count their ticks each from their own start.
//!
//! A field a tick set is a tag u8 and its value: 1 the position (3 × f64), 2 the motion
//! (3 × f64), 3 the rotation (yaw f32, pitch f32), 4 on the ground (u8 0 or 1), 5 the
//! data (as above), 6 `lived` (u32), 7 the fall distance (f64), 8 the fire (i32), 9 the
//! riding (as above), 10 `fresh_at` (u32), 11 the hops (u32), 12 a mob's body part and 14
//! a mind part (each its id u16, version u8, and bytes as a length u32 and the bytes), 13
//! and 15 the id u16 of a body or mind part gone, 16 a mob's type's state (its version u8
//! and bytes). A mob's data is its body's parts and then its mind's, each a count u16 and
//! the parts ascending by id, then its type's state.
//!
//! A passenger is written in its root's chunk, wherever its own position is, so the
//! check that a whole entity lies in the chunk it is written under is made only of an
//! entity that rides nothing.
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
    Attached, BlockPos, ChunkPos, Contents, Double, EdgeId, Entity, EntityChange, EntityData,
    EntityField, EntityId, Entry, Float, Hurt, ItemStack, Leash, LeashHolder, Pending, Riding,
    Seat, TypeState,
};
use uuid::Uuid;

use crate::bytes::Input;
use crate::{FORMAT_VERSION, FormatError};

/// The kind of the file of a chunk's entities.
pub const KIND_ENTITIES: u8 = 7;

const ON_GROUND: u8 = 1;

const DATA_ITEM: u8 = 1;
const DATA_FALLING_BLOCK: u8 = 2;
const DATA_MINECART: u8 = 3;
const DATA_BOAT: u8 = 4;
const DATA_ARMOR_STAND: u8 = 5;
const DATA_ITEM_FRAME: u8 = 6;
const DATA_PAINTING: u8 = 7;
const DATA_CUSHION: u8 = 8;
const DATA_LEASH_KNOT: u8 = 9;
const DATA_PRIMED_TNT: u8 = 10;
const DATA_MOB: u8 = 11;
const DATA_EXPERIENCE_ORB: u8 = 12;

const SEAT_PLAYER: u8 = 1;
const SEAT_PENDING: u8 = 2;

const LEASH_NONE: u8 = 0;
const LEASH_ENTITY: u8 = 1;
const LEASH_FENCE: u8 = 2;

const FRAME_FIXED: u8 = 1;
const FRAME_INVISIBLE: u8 = 2;

const DROP_ITEM: u8 = 1;
const CANCEL_DROP: u8 = 2;

const FIELD_POSITION: u8 = 1;
const FIELD_MOTION: u8 = 2;
const FIELD_ROTATION: u8 = 3;
const FIELD_ON_GROUND: u8 = 4;
const FIELD_DATA: u8 = 5;
const FIELD_LIVED: u8 = 6;
const FIELD_FALL_DISTANCE: u8 = 7;
const FIELD_FIRE: u8 = 8;
const FIELD_RIDING: u8 = 9;
const FIELD_FRESH_AT: u8 = 10;
const FIELD_HOPS: u8 = 11;
const FIELD_BODY_ENTRY: u8 = 12;
const FIELD_BODY_GONE: u8 = 13;
const FIELD_MIND_ENTRY: u8 = 14;
const FIELD_MIND_GONE: u8 = 15;
const FIELD_TYPE_STATE: u8 = 16;

const CHANGE_WHOLE: u8 = 1;
const CHANGE_GONE: u8 = 2;
const CHANGE_SET: u8 = 3;

/// Bytes an entity takes at the least: an item without a thrower, riding nothing.
const ENTITY_LENGTH: usize =
    8 + 2 + 16 + 48 + 8 + 1 + 4 + 4 + 8 + 4 + RIDING_LENGTH + 1 + 4 + 4 + 2 + 1 + 2;
/// Bytes riding nothing takes.
const RIDING_LENGTH: usize = 1 + 2 + 1 + 1;
/// Bytes a change takes at the least: a gone one.
const CHANGE_LENGTH: usize = 9;
/// Bytes a field takes at the least: on the ground.
const FIELD_LENGTH: usize = 2;
/// The fewest bytes a mob's part can take: its id, version and length.
const ENTRY_LENGTH: usize = 7;
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

fn put_position(out: &mut Vec<u8>, position: BlockPos) {
    out.extend_from_slice(&position.x.to_be_bytes());
    out.extend_from_slice(&position.y.to_be_bytes());
    out.extend_from_slice(&position.z.to_be_bytes());
}

fn take_position(input: &mut Input<'_>) -> Result<BlockPos, FormatError> {
    Ok(BlockPos::new(input.i32()?, input.i32()?, input.i32()?))
}

fn put_stack(out: &mut Vec<u8>, stack: ItemStack) {
    out.extend_from_slice(&stack.item.to_be_bytes());
    out.extend_from_slice(&stack.count.to_be_bytes());
}

fn take_stack(input: &mut Input<'_>) -> Result<ItemStack, FormatError> {
    Ok(ItemStack {
        item: input.i32()?,
        count: input.i32()?,
    })
}

fn put_optional_stack(out: &mut Vec<u8>, stack: Option<ItemStack>) {
    match stack {
        None => out.push(0),
        Some(stack) => {
            out.push(1);
            put_stack(out, stack);
        }
    }
}

fn take_optional_stack(input: &mut Input<'_>) -> Result<Option<ItemStack>, FormatError> {
    Ok(if take_flag(input, "an item's presence")? {
        Some(take_stack(input)?)
    } else {
        None
    })
}

/// A count u8, then per stack its slot u8 and the stack: ascending by slot, each once.
fn put_contents(out: &mut Vec<u8>, contents: &Contents) {
    out.push(u8::try_from(contents.len()).expect("a container has at most 255 slots"));
    for (slot, stack) in contents {
        out.push(*slot);
        put_stack(out, *stack);
    }
}

fn take_contents(input: &mut Input<'_>) -> Result<Contents, FormatError> {
    let count = input.u8()?;
    let mut contents: Contents = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let slot = input.u8()?;
        if contents.last().is_some_and(|(last, _)| *last >= slot) {
            return Err(FormatError::Corrupt("a container's slots out of order"));
        }
        contents.push((slot, take_stack(input)?));
    }
    Ok(contents)
}

fn put_hurt(out: &mut Vec<u8>, hurt: Hurt) {
    out.extend_from_slice(&hurt.time.to_be_bytes());
    out.extend_from_slice(&hurt.direction.to_be_bytes());
    out.extend_from_slice(&hurt.damage.0.to_be_bytes());
}

fn take_hurt(input: &mut Input<'_>) -> Result<Hurt, FormatError> {
    Ok(Hurt {
        time: input.i32()?,
        direction: input.i32()?,
        damage: Float(input.f32()?),
    })
}

fn put_attached(out: &mut Vec<u8>, attached: Attached) {
    put_position(out, attached.block);
    out.push(attached.direction);
}

fn take_attached(input: &mut Input<'_>) -> Result<Attached, FormatError> {
    let block = take_position(input)?;
    let direction = input.u8()?;
    if direction > 5 {
        return Err(FormatError::Corrupt("an attached entity's direction"));
    }
    Ok(Attached { block, direction })
}

/// The vehicle (u8 0, or 1 and its key u64), the cooldown u16, the passengers (a count
/// u8, then each one's key u64, flags u8: bit 0 a player, bit 1 pending, and where
/// pending the ticks left u16 and the edge u64), and the leash (u8 0 none, 1 an entity's
/// key u64, 2 a fence's x, y, z i32, then for 1 and 2 the angular momentum f64).
fn put_riding(out: &mut Vec<u8>, riding: &Riding) {
    match riding.vehicle {
        None => out.push(0),
        Some(vehicle) => {
            out.push(1);
            out.extend_from_slice(&vehicle.0.to_be_bytes());
        }
    }
    out.extend_from_slice(&riding.cooldown.to_be_bytes());
    out.push(u8::try_from(riding.passengers.len()).expect("at most 255 passengers"));
    for seat in &riding.passengers {
        out.extend_from_slice(&seat.key.0.to_be_bytes());
        let mut flags = 0;
        if seat.player {
            flags |= SEAT_PLAYER;
        }
        if seat.pending.is_some() {
            flags |= SEAT_PENDING;
        }
        out.push(flags);
        if let Some(pending) = seat.pending {
            out.extend_from_slice(&pending.left.to_be_bytes());
            out.extend_from_slice(&pending.edge.0.to_be_bytes());
        }
    }
    match riding.leash.as_deref() {
        None => out.push(LEASH_NONE),
        Some(leash) => {
            match leash.holder {
                LeashHolder::Entity(key) => {
                    out.push(LEASH_ENTITY);
                    out.extend_from_slice(&key.0.to_be_bytes());
                }
                LeashHolder::Fence(position) => {
                    out.push(LEASH_FENCE);
                    put_position(out, position);
                }
            }
            out.extend_from_slice(&leash.angular_momentum.0.to_be_bytes());
        }
    }
}

fn take_riding(input: &mut Input<'_>) -> Result<Riding, FormatError> {
    let vehicle = if take_flag(input, "an entity's vehicle flag")? {
        Some(EntityId(input.u64()?))
    } else {
        None
    };
    let cooldown = input.u16()?;
    let count = input.u8()?;
    let mut passengers = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let key = EntityId(input.u64()?);
        let flags = input.u8()?;
        if flags & !(SEAT_PLAYER | SEAT_PENDING) != 0
            || (flags & SEAT_PENDING != 0 && flags & SEAT_PLAYER == 0)
        {
            return Err(FormatError::Corrupt("a seat's flags"));
        }
        let pending = if flags & SEAT_PENDING != 0 {
            Some(Pending {
                left: input.u16()?,
                edge: EdgeId(input.u64()?),
            })
        } else {
            None
        };
        passengers.push(Seat {
            key,
            player: flags & SEAT_PLAYER != 0,
            pending,
        });
    }
    let holder = match input.u8()? {
        LEASH_NONE => None,
        LEASH_ENTITY => Some(LeashHolder::Entity(EntityId(input.u64()?))),
        LEASH_FENCE => Some(LeashHolder::Fence(take_position(input)?)),
        _ => return Err(FormatError::Corrupt("a leash's tag")),
    };
    let leash = match holder {
        None => None,
        Some(holder) => Some(Box::new(Leash {
            holder,
            angular_momentum: Double(input.f64()?),
        })),
    };
    Ok(Riding {
        vehicle,
        passengers,
        cooldown,
        leash,
    })
}

fn put_data(out: &mut Vec<u8>, data: &EntityData) {
    match data {
        EntityData::Item {
            stack,
            pick_up_delay,
            thrower,
            health,
            age_offset,
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
            out.extend_from_slice(&age_offset.to_be_bytes());
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
        EntityData::Minecart {
            hurt,
            display,
            display_offset,
            flipped,
            contents,
            fuel,
            push,
            fuse,
            enabled,
        } => {
            out.push(DATA_MINECART);
            put_hurt(out, *hurt);
            match display {
                None => out.push(0),
                Some(state) => {
                    out.push(1);
                    out.extend_from_slice(&state.to_be_bytes());
                }
            }
            match display_offset {
                None => out.push(0),
                Some(offset) => {
                    out.push(1);
                    out.extend_from_slice(&offset.to_be_bytes());
                }
            }
            out.push(u8::from(*flipped));
            put_contents(out, contents);
            out.extend_from_slice(&fuel.to_be_bytes());
            for part in push {
                out.extend_from_slice(&part.0.to_be_bytes());
            }
            out.extend_from_slice(&fuse.to_be_bytes());
            out.push(u8::from(*enabled));
        }
        EntityData::Boat {
            hurt,
            paddles,
            bubble_time,
            out_of_control,
            turning,
            status,
            contents,
        } => {
            out.push(DATA_BOAT);
            put_hurt(out, *hurt);
            out.push(u8::from(paddles[0]) | (u8::from(paddles[1]) << 1));
            out.extend_from_slice(&bubble_time.to_be_bytes());
            out.extend_from_slice(&out_of_control.0.to_be_bytes());
            out.extend_from_slice(&turning.0.to_be_bytes());
            out.extend_from_slice(status);
            put_contents(out, contents);
        }
        EntityData::ArmorStand {
            flags,
            disabled_slots,
            pose,
            equipment,
            health,
            last_hit,
            air,
            frozen,
        } => {
            out.push(DATA_ARMOR_STAND);
            out.push(*flags);
            out.extend_from_slice(&disabled_slots.to_be_bytes());
            for part in pose.iter() {
                for angle in part {
                    out.extend_from_slice(&angle.0.to_be_bytes());
                }
            }
            put_contents(out, equipment);
            out.extend_from_slice(&health.0.to_be_bytes());
            match last_hit {
                None => out.push(0),
                Some(age) => {
                    out.push(1);
                    out.extend_from_slice(&age.to_be_bytes());
                }
            }
            out.extend_from_slice(&air.to_be_bytes());
            out.extend_from_slice(&frozen.to_be_bytes());
        }
        EntityData::ItemFrame {
            attached,
            item,
            rotation,
            drop_chance,
            fixed,
            invisible,
        } => {
            out.push(DATA_ITEM_FRAME);
            put_attached(out, *attached);
            put_optional_stack(out, *item);
            out.push(*rotation);
            out.extend_from_slice(&drop_chance.0.to_be_bytes());
            let mut flags = 0;
            if *fixed {
                flags |= FRAME_FIXED;
            }
            if *invisible {
                flags |= FRAME_INVISIBLE;
            }
            out.push(flags);
        }
        EntityData::Painting { attached, variant } => {
            out.push(DATA_PAINTING);
            put_attached(out, *attached);
            out.extend_from_slice(&variant.to_be_bytes());
        }
        EntityData::Cushion { attached, colour } => {
            out.push(DATA_CUSHION);
            put_attached(out, *attached);
            out.push(*colour);
        }
        EntityData::LeashKnot { attached } => {
            out.push(DATA_LEASH_KNOT);
            put_attached(out, *attached);
        }
        EntityData::PrimedTnt {
            fuse,
            block,
            power,
            owner,
        } => {
            out.push(DATA_PRIMED_TNT);
            out.extend_from_slice(&fuse.to_be_bytes());
            out.extend_from_slice(&block.to_be_bytes());
            out.extend_from_slice(&power.0.to_be_bytes());
            match owner {
                None => out.push(0),
                Some(owner) => {
                    out.push(1);
                    out.extend_from_slice(&owner.as_u128().to_be_bytes());
                }
            }
        }
        EntityData::Mob { body, mind, state } => {
            out.push(DATA_MOB);
            put_entries(out, body);
            put_entries(out, mind);
            put_type_state(out, state);
        }
        EntityData::ExperienceOrb {
            value,
            count,
            health,
            age_offset,
            following,
        } => {
            out.push(DATA_EXPERIENCE_ORB);
            out.extend_from_slice(&value.to_be_bytes());
            out.extend_from_slice(&count.to_be_bytes());
            out.extend_from_slice(&health.to_be_bytes());
            out.extend_from_slice(&age_offset.to_be_bytes());
            match following {
                None => out.push(0),
                Some(player) => {
                    out.push(1);
                    out.extend_from_slice(&player.0.to_be_bytes());
                }
            }
        }
    }
}

fn put_entry(out: &mut Vec<u8>, entry: &Entry) {
    out.extend_from_slice(&entry.id.to_be_bytes());
    out.push(entry.version);
    put_bytes(out, &entry.bytes);
}

fn take_entry(input: &mut Input<'_>) -> Result<Entry, FormatError> {
    Ok(Entry {
        id: input.u16()?,
        version: input.u8()?,
        bytes: take_bytes(input)?,
    })
}

/// A mob's parts: a count u16, then each part, ascending by id.
fn put_entries(out: &mut Vec<u8>, entries: &[Entry]) {
    let count = u16::try_from(entries.len()).expect("a mob has fewer than 65,536 parts");
    out.extend_from_slice(&count.to_be_bytes());
    for entry in entries {
        put_entry(out, entry);
    }
}

fn take_entries(input: &mut Input<'_>) -> Result<Vec<Entry>, FormatError> {
    let count = usize::from(input.u16()?);
    let mut entries: Vec<Entry> = Vec::with_capacity(count.min(input.0.len() / ENTRY_LENGTH));
    for _ in 0..count {
        let entry = take_entry(input)?;
        if entries.last().is_some_and(|last| last.id >= entry.id) {
            return Err(FormatError::Corrupt("a mob's parts out of order"));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn put_type_state(out: &mut Vec<u8>, state: &TypeState) {
    out.push(state.version);
    put_bytes(out, &state.bytes);
}

fn take_type_state(input: &mut Input<'_>) -> Result<TypeState, FormatError> {
    Ok(TypeState {
        version: input.u8()?,
        bytes: take_bytes(input)?,
    })
}

/// Bytes of a part's own encoding: a length u32, then the bytes.
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = u32::try_from(bytes.len()).expect("a part is shorter than 4 GiB");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
}

fn take_bytes(input: &mut Input<'_>) -> Result<Vec<u8>, FormatError> {
    let length = usize::try_from(input.u32()?).map_err(|_| FormatError::Corrupt("a length"))?;
    Ok(input.take(length)?.to_vec())
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
                age_offset: input.u32()?,
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
        DATA_MINECART => {
            let hurt = take_hurt(input)?;
            let display = if take_flag(input, "a minecart's display flag")? {
                Some(input.u16()?)
            } else {
                None
            };
            let display_offset = if take_flag(input, "a minecart's offset flag")? {
                Some(input.i32()?)
            } else {
                None
            };
            Ok(EntityData::Minecart {
                hurt,
                display,
                display_offset,
                flipped: take_flag(input, "a minecart's flipped flag")?,
                contents: take_contents(input)?,
                fuel: input.i32()?,
                push: [Double(input.f64()?), Double(input.f64()?)],
                fuse: input.i32()?,
                enabled: take_flag(input, "a hopper cart's switch")?,
            })
        }
        DATA_BOAT => {
            let hurt = take_hurt(input)?;
            let paddles = input.u8()?;
            if paddles & !3 != 0 {
                return Err(FormatError::Corrupt("a boat's paddles"));
            }
            Ok(EntityData::Boat {
                hurt,
                paddles: [paddles & 1 != 0, paddles & 2 != 0],
                bubble_time: input.i32()?,
                out_of_control: Float(input.f32()?),
                turning: Float(input.f32()?),
                status: [input.u8()?, input.u8()?],
                contents: take_contents(input)?,
            })
        }
        DATA_ARMOR_STAND => {
            let flags = input.u8()?;
            let disabled_slots = input.i32()?;
            let mut pose = Box::new([[Float(0.0); 3]; 6]);
            for part in pose.iter_mut() {
                for angle in part {
                    *angle = Float(input.f32()?);
                }
            }
            let equipment = take_contents(input)?;
            let health = Float(input.f32()?);
            let last_hit = if take_flag(input, "an armour stand's hit flag")? {
                Some(input.u32()?)
            } else {
                None
            };
            let air = input.i16()?;
            let frozen = input.u16()?;
            Ok(EntityData::ArmorStand {
                flags,
                disabled_slots,
                pose,
                equipment,
                health,
                last_hit,
                air,
                frozen,
            })
        }
        DATA_ITEM_FRAME => {
            let attached = take_attached(input)?;
            let item = take_optional_stack(input)?;
            let rotation = input.u8()?;
            if rotation > 7 {
                return Err(FormatError::Corrupt("an item frame's rotation"));
            }
            let drop_chance = Float(input.f32()?);
            let flags = input.u8()?;
            if flags & !(FRAME_FIXED | FRAME_INVISIBLE) != 0 {
                return Err(FormatError::Corrupt("an item frame's flags"));
            }
            Ok(EntityData::ItemFrame {
                attached,
                item,
                rotation,
                drop_chance,
                fixed: flags & FRAME_FIXED != 0,
                invisible: flags & FRAME_INVISIBLE != 0,
            })
        }
        DATA_PAINTING => Ok(EntityData::Painting {
            attached: take_attached(input)?,
            variant: input.u16()?,
        }),
        DATA_CUSHION => Ok(EntityData::Cushion {
            attached: take_attached(input)?,
            colour: input.u8()?,
        }),
        DATA_LEASH_KNOT => Ok(EntityData::LeashKnot {
            attached: take_attached(input)?,
        }),
        DATA_PRIMED_TNT => Ok(EntityData::PrimedTnt {
            fuse: input.i16()?,
            block: input.u16()?,
            power: Float(input.f32()?),
            owner: if take_flag(input, "a primed TNT's owner flag")? {
                Some(Uuid::from_u128(input.u128()?))
            } else {
                None
            },
        }),
        DATA_MOB => Ok(EntityData::Mob {
            body: take_entries(input)?,
            mind: take_entries(input)?,
            state: take_type_state(input)?,
        }),
        DATA_EXPERIENCE_ORB => Ok(EntityData::ExperienceOrb {
            value: input.i32()?,
            count: input.i32()?,
            health: input.i16()?,
            age_offset: input.u32()?,
            following: if take_flag(input, "an orb's followed player flag")? {
                Some(EntityId(input.u64()?))
            } else {
                None
            },
        }),
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
    out.extend_from_slice(&entity.lived.to_be_bytes());
    out.extend_from_slice(&entity.fresh_at.to_be_bytes());
    out.extend_from_slice(&entity.fall_distance.to_be_bytes());
    out.extend_from_slice(&entity.fire_ticks.to_be_bytes());
    put_riding(out, &entity.riding);
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
        lived: input.u32()?,
        fresh_at: input.u32()?,
        // Read in the order written: struct fields are taken in the order they are here.
        fall_distance: input.f64()?,
        fire_ticks: input.i32()?,
        riding: take_riding(input)?,
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
        EntityField::Lived(lived) => {
            out.push(FIELD_LIVED);
            out.extend_from_slice(&lived.to_be_bytes());
        }
        EntityField::FreshAt(fresh_at) => {
            out.push(FIELD_FRESH_AT);
            out.extend_from_slice(&fresh_at.to_be_bytes());
        }
        EntityField::Hops(hops) => {
            out.push(FIELD_HOPS);
            out.extend_from_slice(&hops.to_be_bytes());
        }
        EntityField::BodyEntry(entry) => {
            out.push(FIELD_BODY_ENTRY);
            put_entry(out, entry);
        }
        EntityField::BodyGone(id) => {
            out.push(FIELD_BODY_GONE);
            out.extend_from_slice(&id.to_be_bytes());
        }
        EntityField::MindEntry(entry) => {
            out.push(FIELD_MIND_ENTRY);
            put_entry(out, entry);
        }
        EntityField::MindGone(id) => {
            out.push(FIELD_MIND_GONE);
            out.extend_from_slice(&id.to_be_bytes());
        }
        EntityField::TypeState(state) => {
            out.push(FIELD_TYPE_STATE);
            put_type_state(out, state);
        }
        EntityField::FallDistance(distance) => {
            out.push(FIELD_FALL_DISTANCE);
            out.extend_from_slice(&distance.to_be_bytes());
        }
        EntityField::FireTicks(ticks) => {
            out.push(FIELD_FIRE);
            out.extend_from_slice(&ticks.to_be_bytes());
        }
        EntityField::Riding(riding) => {
            out.push(FIELD_RIDING);
            put_riding(out, riding);
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
        FIELD_LIVED => EntityField::Lived(input.u32()?),
        FIELD_FRESH_AT => EntityField::FreshAt(input.u32()?),
        FIELD_HOPS => EntityField::Hops(input.u32()?),
        FIELD_BODY_ENTRY => EntityField::BodyEntry(take_entry(input)?),
        FIELD_BODY_GONE => EntityField::BodyGone(input.u16()?),
        FIELD_MIND_ENTRY => EntityField::MindEntry(take_entry(input)?),
        FIELD_MIND_GONE => EntityField::MindGone(input.u16()?),
        FIELD_TYPE_STATE => EntityField::TypeState(take_type_state(input)?),
        FIELD_FALL_DISTANCE => EntityField::FallDistance(input.f64()?),
        FIELD_FIRE => EntityField::FireTicks(input.i32()?),
        FIELD_RIDING => EntityField::Riding(take_riding(input)?),
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
/// chunk is where its position is, but for a passenger, whose chunk is its root's.
fn take_change(input: &mut Input<'_>, chunk: ChunkPos) -> Result<EntityChange, FormatError> {
    Ok(match input.u8()? {
        CHANGE_WHOLE => {
            let entity = take_entity(input)?;
            if entity.riding.vehicle.is_none() && entity.chunk() != chunk {
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
            if entity.riding.vehicle.is_none() && entity.chunk() != position {
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
            lived: 12,
            fresh_at: 0,
            data: EntityData::Item {
                stack: ItemStack { item: 1, count: 64 },
                pick_up_delay: 40,
                thrower: thrower.map(Uuid::from_u128),
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            fire_ticks: -1,
            riding: Default::default(),
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
        expected.extend_from_slice(&[0, 0, 0, 12]); // lived
        expected.extend_from_slice(&[0, 0, 0, 0]); // fresh at
        expected.extend_from_slice(&[0; 8]); // fall distance 0
        expected.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // fire -1
        expected.extend_from_slice(&[0, 0, 0, 0, 0]); // no vehicle, no cooldown, no seats, no leash
        expected.push(1); // an item
        expected.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 64]); // item, count
        expected.extend_from_slice(&[0, 40]); // pick-up delay
        expected.push(1); // a thrower
        expected.extend_from_slice(&7u128.to_be_bytes());
        expected.extend_from_slice(&[0, 5]); // health
        expected.extend_from_slice(&[0, 0, 0, 0]); // its age is `lived`
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

    /// A mob's parts are written ascending by id, each once, so that replaying a part's
    /// change finds it by its id; bytes that say otherwise are refused.
    #[test]
    fn a_mob_whose_parts_are_out_of_order_or_twice_is_refused() {
        let part = |id: u16| Entry {
            id,
            version: 0,
            bytes: vec![id as u8],
        };
        for body in [vec![part(3), part(1)], vec![part(2), part(2)]] {
            let mut out = Vec::new();
            put_data(
                &mut out,
                &EntityData::Mob {
                    body,
                    mind: Vec::new(),
                    state: TypeState::default(),
                },
            );
            assert_eq!(
                take_data(&mut Input(&out)),
                Err(FormatError::Corrupt("a mob's parts out of order"))
            );
        }
        let mut out = Vec::new();
        let mob = EntityData::Mob {
            body: vec![part(1), part(3)],
            mind: vec![part(0)],
            state: TypeState {
                version: 4,
                bytes: vec![9, 8],
            },
        };
        put_data(&mut out, &mob);
        // The tag; two body parts, each id, version, length and its one byte; one mind
        // part; the state's version, length and bytes.
        assert_eq!(
            out,
            [
                11, 0, 2, 0, 1, 0, 0, 0, 0, 1, 1, 0, 3, 0, 0, 0, 0, 1, 3, 0, 1, 0, 0, 0, 0, 0, 0,
                1, 0, 4, 0, 0, 0, 2, 9, 8
            ]
        );
        assert_eq!(take_data(&mut Input(&out)), Ok(mob));
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
            (
                any::<u32>(),
                any::<u32>(),
                any::<u32>(),
                any::<f64>(),
                any::<i32>(),
            ),
            datas(),
            ridings(),
        )
            .prop_map(
                |(
                    key,
                    kind,
                    uuid,
                    (x, y, z),
                    motion,
                    (yaw, pitch, on_ground),
                    (hops, lived, fresh_at, fall_distance, fire_ticks),
                    data,
                    riding,
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
                        lived,
                        fresh_at,
                        data,
                        fall_distance,
                        fire_ticks,
                        riding,
                    }
                },
            )
    }

    fn ridings() -> impl Strategy<Value = Riding> {
        let seat = (any::<u64>(), any::<bool>(), any::<Option<(u16, u64)>>()).prop_map(
            |(key, player, pending)| Seat {
                key: EntityId(key),
                // Only a player's seat is kept for them on their way.
                player: player || pending.is_some(),
                pending: pending.map(|(left, edge)| Pending {
                    left,
                    edge: EdgeId(edge),
                }),
            },
        );
        let holder = prop_oneof![
            any::<u64>().prop_map(|key| LeashHolder::Entity(EntityId(key))),
            any::<(i32, i32, i32)>()
                .prop_map(|(x, y, z)| LeashHolder::Fence(BlockPos::new(x, y, z))),
        ];
        (
            any::<Option<u64>>(),
            prop::collection::vec(seat, 0..4),
            any::<u16>(),
            prop::option::of((holder, any::<f64>())),
        )
            .prop_map(|(vehicle, passengers, cooldown, leash)| Riding {
                vehicle: vehicle.map(EntityId),
                passengers,
                cooldown,
                leash: leash.map(|(holder, momentum)| {
                    Box::new(Leash {
                        holder,
                        angular_momentum: Double(momentum),
                    })
                }),
            })
    }

    fn contents() -> impl Strategy<Value = Contents> {
        prop::collection::btree_map(any::<u8>(), any::<(i32, i32)>(), 0..6).prop_map(|slots| {
            slots
                .into_iter()
                .map(|(slot, (item, count))| (slot, ItemStack { item, count }))
                .collect()
        })
    }

    fn hurts() -> impl Strategy<Value = Hurt> {
        any::<(i32, i32, f32)>().prop_map(|(time, direction, damage)| Hurt {
            time,
            direction,
            damage: Float(damage),
        })
    }

    fn attachments() -> impl Strategy<Value = Attached> {
        (any::<(i32, i32, i32)>(), 0..6u8).prop_map(|((x, y, z), direction)| Attached {
            block: BlockPos::new(x, y, z),
            direction,
        })
    }

    fn entry() -> impl Strategy<Value = Entry> {
        (
            any::<u16>(),
            any::<u8>(),
            prop::collection::vec(any::<u8>(), 0..12),
        )
            .prop_map(|(id, version, bytes)| Entry { id, version, bytes })
    }

    /// Parts ascending by id, each once.
    fn entries() -> impl Strategy<Value = Vec<Entry>> {
        prop::collection::btree_map(any::<u16>(), entry(), 0..4).prop_map(|parts| {
            parts
                .into_iter()
                .map(|(id, entry)| Entry { id, ..entry })
                .collect()
        })
    }

    fn type_states() -> impl Strategy<Value = TypeState> {
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..12))
            .prop_map(|(version, bytes)| TypeState { version, bytes })
    }

    fn datas() -> impl Strategy<Value = EntityData> {
        prop_oneof![
            (entries(), entries(), type_states()).prop_map(|(body, mind, state)| EntityData::Mob {
                body,
                mind,
                state
            }),
            (
                any::<i32>(),
                any::<i32>(),
                any::<u16>(),
                any::<Option<u128>>(),
                any::<i16>(),
                any::<u32>(),
            )
                .prop_map(
                    |(item, count, pick_up_delay, thrower, health, age_offset)| {
                        EntityData::Item {
                            stack: ItemStack { item, count },
                            pick_up_delay,
                            thrower: thrower.map(Uuid::from_u128),
                            health,
                            age_offset,
                        }
                    }
                ),
            (any::<u16>(), any::<i32>(), any::<bool>(), any::<bool>()).prop_map(
                |(state, time, drop_item, cancel_drop)| EntityData::FallingBlock {
                    state,
                    time,
                    drop_item,
                    cancel_drop,
                }
            ),
            (
                hurts(),
                any::<(Option<u16>, Option<i32>, bool)>(),
                contents(),
                any::<i32>(),
                any::<[f64; 2]>(),
                any::<(i32, bool)>(),
            )
                .prop_map(
                    |(
                        hurt,
                        (display, display_offset, flipped),
                        contents,
                        fuel,
                        push,
                        (fuse, enabled),
                    )| {
                        EntityData::Minecart {
                            hurt,
                            display,
                            display_offset,
                            flipped,
                            contents,
                            fuel,
                            push: push.map(Double),
                            fuse,
                            enabled,
                        }
                    }
                ),
            (
                hurts(),
                any::<([bool; 2], i32, f32, f32, [u8; 2])>(),
                contents(),
            )
                .prop_map(
                    |(hurt, (paddles, bubble_time, out_of_control, turning, status), contents)| {
                        EntityData::Boat {
                            hurt,
                            paddles,
                            bubble_time,
                            out_of_control: Float(out_of_control),
                            turning: Float(turning),
                            status,
                            contents,
                        }
                    }
                ),
            (
                any::<(u8, i32)>(),
                any::<[[f32; 3]; 6]>(),
                contents(),
                any::<(f32, Option<u32>, i16, u16)>(),
            )
                .prop_map(
                    |(
                        (flags, disabled_slots),
                        pose,
                        equipment,
                        (health, last_hit, air, frozen),
                    )| {
                        EntityData::ArmorStand {
                            flags,
                            disabled_slots,
                            pose: Box::new(pose.map(|part| part.map(Float))),
                            equipment,
                            health: Float(health),
                            last_hit,
                            air,
                            frozen,
                        }
                    }
                ),
            (
                attachments(),
                any::<Option<(i32, i32)>>(),
                0..8u8,
                any::<(f32, bool, bool)>(),
            )
                .prop_map(
                    |(attached, item, rotation, (drop_chance, fixed, invisible))| {
                        EntityData::ItemFrame {
                            attached,
                            item: item.map(|(item, count)| ItemStack { item, count }),
                            rotation,
                            drop_chance: Float(drop_chance),
                            fixed,
                            invisible,
                        }
                    }
                ),
            (attachments(), any::<u16>())
                .prop_map(|(attached, variant)| EntityData::Painting { attached, variant }),
            (attachments(), any::<u8>())
                .prop_map(|(attached, colour)| EntityData::Cushion { attached, colour }),
            attachments().prop_map(|attached| EntityData::LeashKnot { attached }),
            any::<(i16, u16, f32, Option<u128>)>().prop_map(|(fuse, block, power, owner)| {
                EntityData::PrimedTnt {
                    fuse,
                    block,
                    power: Float(power),
                    owner: owner.map(Uuid::from_u128),
                }
            }),
            any::<(i32, i32, i16, u32, Option<u64>)>().prop_map(
                |(value, count, health, age_offset, following)| EntityData::ExperienceOrb {
                    value,
                    count,
                    health,
                    age_offset,
                    following: following.map(EntityId),
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
            any::<u32>().prop_map(EntityField::Lived),
            any::<u32>().prop_map(EntityField::FreshAt),
            any::<u32>().prop_map(EntityField::Hops),
            entry().prop_map(EntityField::BodyEntry),
            any::<u16>().prop_map(EntityField::BodyGone),
            entry().prop_map(EntityField::MindEntry),
            any::<u16>().prop_map(EntityField::MindGone),
            type_states().prop_map(EntityField::TypeState),
            datas().prop_map(EntityField::Data),
            any::<f64>().prop_map(EntityField::FallDistance),
            any::<i32>().prop_map(EntityField::FireTicks),
            ridings().prop_map(EntityField::Riding),
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
