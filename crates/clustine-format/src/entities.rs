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
//! where a stack is written as `stacks.rs` says (its item, count and components), the
//! data of tag 1, an item, is its stack, the pick-up delay
//! left as of the record's tick u16, u8 1 followed by the thrower's UUID u128, or u8 0
//! where nobody threw it, its health i16, and what `lived` is ahead of its age u32; the data of tag 2, a falling block, is its
//! state u16, the ticks it has fallen i32, and flags u8 (bit 0 it drops as an item where
//! it cannot land, bit 1 its drop is called off; the others 0). Tags 3 to 9 are N5's
//! kinds (`docs/groundwork/n5-plan.md`): 3 a minecart, 4 a boat, 5 an armour stand, 6 an
//! item frame, 7 a painting, 8 a cushion, 9 a leash's knot, and tag 10 is N6's primed
//! TNT (`docs/groundwork/n6-plan.md`) and tag 11 is N7's mob (`docs/groundwork/n7-plan.md`,
//! section 11.3) and tag 12 an orb of experience, and tag 13 is N7k's thrown item
//! (`docs/groundwork/n7k-plan.md`, section 10): its stack, u8 1
//! followed by its owner's key u64 or u8 0 where it has none, and flags u8 (bit 0 it
//! has left its owner, bit 1 it has been shot; the others 0). Tags 14 to 18 are N9's
//! projectiles (`docs/groundwork/n9-plan.md`, sections 4.1 and 18): 14 an arrow, a
//! spectral arrow or a trident (see `put_arrow`), 15 the llama's spit (its owner as a
//! thrown item's and the same flags), 16 a firework rocket (its stack, its owner, flags
//! u8 as a thrown item's with bit 2 shot at an angle, its life i32 and lifetime i32), 17
//! an area effect cloud (see `put_cloud`) and 18 the evoker's fangs (its owner and its
//! warmup i32); a new tag needed no new format, since no world made before holds one.
//! Each kind is written by `put_data` in the order of its
//! fields (a minecart's TNT fuse i32 and a hopper cart's switch u8 after its push; the
//! TNT's fuse i16, block u16, power f32 and owner as an item's thrower); a block-attached entity's attachment is its block (3 × i32)
//! and direction (u8, 0 to 5), a container's contents a count u8 and per stack its slot
//! u8 and the stack, ascending by slot. Counters are kept relative to the record they are in
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
    Arrow, ArrowKind, Attached, BlockPos, ChunkPos, Cloud, Contents, Double, EdgeId, Entity,
    EntityChange, EntityData, EntityField, EntityId, Entry, Fangs, Firework, Float, Hurt, InGround,
    Leash, LeashHolder, Pending, Pickup, PortalClock, PortalKind, PortalState, Riding, Seat, Spit,
    TypeState,
};
use uuid::Uuid;

use crate::bytes::Input;
use crate::stacks::{
    put_optional_stack, put_potion_contents, put_stack, put_string, take_optional_stack,
    take_potion_contents, take_stack, take_string,
};
use crate::{FORMAT_VERSION, FormatError};

/// The kind of the file of a chunk's entities.
pub const KIND_ENTITIES: u8 = 7;

const ON_GROUND: u8 = 1;
/// The entity's record ends with its portal state, which is left out where it is idle.
const WITH_PORTAL: u8 = 2;

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
const DATA_THROWN: u8 = 13;
// N9's projectiles (`docs/groundwork/n9-plan.md`, sections 4.1 and 18). A tag that is
// new is one that no world made before it holds, so these need no raise of the format
// (finding 18): an older build never meets them, and this one reads every older record
// as before.
const DATA_ARROW: u8 = 14;
const DATA_SPIT: u8 = 15;
const DATA_FIREWORK: u8 = 16;
const DATA_CLOUD: u8 = 17;
const DATA_FANGS: u8 = 18;

const SEAT_PLAYER: u8 = 1;
const SEAT_PENDING: u8 = 2;
/// A kept seat granted across a line and not taken yet; only with [`SEAT_PENDING`].
const SEAT_GRANTED: u8 = 4;

const LEASH_NONE: u8 = 0;
const LEASH_ENTITY: u8 = 1;
const LEASH_FENCE: u8 = 2;

const FRAME_FIXED: u8 = 1;
const FRAME_INVISIBLE: u8 = 2;

const DROP_ITEM: u8 = 1;
const CANCEL_DROP: u8 = 2;

const LEFT_OWNER: u8 = 1;
const HAS_BEEN_SHOT: u8 = 2;
/// An arrow's third flag beside its owner's two: a critical arrow.
const CRIT: u8 = 4;
/// A firework's third flag beside its owner's two: shot at an angle.
const SHOT_AT_ANGLE: u8 = 4;

const PICKUP_DISALLOWED: u8 = 0;
const PICKUP_ALLOWED: u8 = 1;
const PICKUP_CREATIVE_ONLY: u8 = 2;

const ARROW_PLAIN: u8 = 0;
const ARROW_SPECTRAL: u8 = 1;
const ARROW_TRIDENT: u8 = 2;

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
const FIELD_PORTAL: u8 = 17;

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

/// A count u8, then per stack its slot u8 and the stack: ascending by slot, each once.
fn put_contents(out: &mut Vec<u8>, contents: &Contents) {
    out.push(u8::try_from(contents.len()).expect("a container has at most 255 slots"));
    for (slot, stack) in contents {
        out.push(*slot);
        put_stack(out, stack);
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
        if let Some(pending) = seat.pending {
            flags |= SEAT_PENDING;
            if pending.granted {
                flags |= SEAT_GRANTED;
            }
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
        if flags & !(SEAT_PLAYER | SEAT_PENDING | SEAT_GRANTED) != 0
            || (flags & SEAT_PENDING != 0 && flags & SEAT_PLAYER == 0)
            || (flags & SEAT_GRANTED != 0 && flags & SEAT_PENDING == 0)
        {
            return Err(FormatError::Corrupt("a seat's flags"));
        }
        let pending = if flags & SEAT_PENDING != 0 {
            Some(Pending {
                left: input.u16()?,
                edge: EdgeId(input.u64()?),
                granted: flags & SEAT_GRANTED != 0,
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
            put_stack(out, stack);
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
            put_optional_stack(out, item.as_ref());
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
        EntityData::Thrown {
            stack,
            owner,
            left_owner,
            has_been_shot,
        } => {
            out.push(DATA_THROWN);
            put_stack(out, stack);
            match owner {
                None => out.push(0),
                Some(owner) => {
                    out.push(1);
                    out.extend_from_slice(&owner.0.to_be_bytes());
                }
            }
            let mut flags = 0;
            if *left_owner {
                flags |= LEFT_OWNER;
            }
            if *has_been_shot {
                flags |= HAS_BEEN_SHOT;
            }
            out.push(flags);
        }
        EntityData::Arrow(arrow) => {
            out.push(DATA_ARROW);
            put_arrow(out, arrow);
        }
        EntityData::Spit(spit) => {
            out.push(DATA_SPIT);
            put_owner(out, spit.owner);
            out.push(shot_flags(spit.left_owner, spit.has_been_shot));
        }
        EntityData::Firework(firework) => {
            out.push(DATA_FIREWORK);
            put_stack(out, &firework.stack);
            put_owner(out, firework.owner);
            let mut flags = shot_flags(firework.left_owner, firework.has_been_shot);
            if firework.shot_at_angle {
                flags |= SHOT_AT_ANGLE;
            }
            out.push(flags);
            out.extend_from_slice(&firework.life.to_be_bytes());
            out.extend_from_slice(&firework.lifetime.to_be_bytes());
        }
        EntityData::Cloud(cloud) => {
            out.push(DATA_CLOUD);
            put_cloud(out, cloud);
        }
        EntityData::Fangs(fangs) => {
            out.push(DATA_FANGS);
            put_owner(out, fangs.owner);
            out.extend_from_slice(&fangs.warmup.to_be_bytes());
        }
    }
}

/// A projectile's owner: u8 1 followed by its key u64, or u8 0 where it has none.
fn put_owner(out: &mut Vec<u8>, owner: Option<EntityId>) {
    match owner {
        None => out.push(0),
        Some(owner) => {
            out.push(1);
            out.extend_from_slice(&owner.0.to_be_bytes());
        }
    }
}

fn take_owner(input: &mut Input<'_>, what: &'static str) -> Result<Option<EntityId>, FormatError> {
    Ok(if take_flag(input, what)? {
        Some(EntityId(input.u64()?))
    } else {
        None
    })
}

/// The flags of what `Projectile` keeps: bit 0 it has left its owner, bit 1 it has been
/// shot.
fn shot_flags(left_owner: bool, has_been_shot: bool) -> u8 {
    let mut flags = 0;
    if left_owner {
        flags |= LEFT_OWNER;
    }
    if has_been_shot {
        flags |= HAS_BEEN_SHOT;
    }
    flags
}

/// A count of a list in a projectile's record, u32, refused where it is beyond what is
/// left of the record, since each element takes a byte at least.
fn take_count(input: &mut Input<'_>, what: &'static str) -> Result<usize, FormatError> {
    let count = usize::try_from(input.u32()?).map_err(|_| FormatError::Corrupt(what))?;
    if count > input.0.len() {
        return Err(FormatError::Corrupt(what));
    }
    Ok(count)
}

fn put_count(out: &mut Vec<u8>, count: usize) {
    let count = u32::try_from(count).expect("a projectile's list is shorter than 2^32");
    out.extend_from_slice(&count.to_be_bytes());
}

/// An arrow: its pickup u8 (the game's ids, 0 disallowed to 2 creative only), its
/// pickup stack, the weapon as an optional stack, its owner, flags u8 (bit 0 it has left
/// its owner, bit 1 it has been shot, bit 2 critical), its base damage f64, its pierce
/// level u8, the entities pierced (a count u32 and each key u64, in the order pierced),
/// its hit sound as a string, where it is stuck (u8 0, or u8 1 followed by the state u16
/// and the `lived` it landed at u32), the `lived` its shake ends at u32, and its kind: u8
/// 0 an arrow; 1 a spectral arrow, followed by the glowing's duration i32; 2 a trident,
/// followed by u8 1 or 0 whether it has dealt its damage and its returning i32.
fn put_arrow(out: &mut Vec<u8>, arrow: &Arrow) {
    out.push(match arrow.pickup {
        Pickup::Disallowed => PICKUP_DISALLOWED,
        Pickup::Allowed => PICKUP_ALLOWED,
        Pickup::CreativeOnly => PICKUP_CREATIVE_ONLY,
    });
    put_stack(out, &arrow.pickup_stack);
    put_optional_stack(out, arrow.weapon.as_ref());
    put_owner(out, arrow.owner);
    let mut flags = shot_flags(arrow.left_owner, arrow.has_been_shot);
    if arrow.crit {
        flags |= CRIT;
    }
    out.push(flags);
    out.extend_from_slice(&arrow.base_damage.0.to_be_bytes());
    out.push(arrow.pierce_level);
    put_count(out, arrow.pierced.len());
    for key in &arrow.pierced {
        out.extend_from_slice(&key.0.to_be_bytes());
    }
    put_string(out, &arrow.hit_sound);
    match arrow.in_ground {
        None => out.push(0),
        Some(in_ground) => {
            out.push(1);
            out.extend_from_slice(&in_ground.state.to_be_bytes());
            out.extend_from_slice(&in_ground.since.to_be_bytes());
        }
    }
    out.extend_from_slice(&arrow.shake_until.to_be_bytes());
    match arrow.of_kind {
        ArrowKind::Arrow => out.push(ARROW_PLAIN),
        ArrowKind::Spectral { glowing_duration } => {
            out.push(ARROW_SPECTRAL);
            out.extend_from_slice(&glowing_duration.to_be_bytes());
        }
        ArrowKind::Trident {
            dealt_damage,
            returning,
        } => {
            out.push(ARROW_TRIDENT);
            out.push(u8::from(dealt_damage));
            out.extend_from_slice(&returning.to_be_bytes());
        }
    }
}

fn take_arrow(input: &mut Input<'_>) -> Result<Arrow, FormatError> {
    let pickup = match input.u8()? {
        PICKUP_DISALLOWED => Pickup::Disallowed,
        PICKUP_ALLOWED => Pickup::Allowed,
        PICKUP_CREATIVE_ONLY => Pickup::CreativeOnly,
        _ => return Err(FormatError::Corrupt("an arrow's pickup")),
    };
    let pickup_stack = take_stack(input)?;
    let weapon = take_optional_stack(input, "an arrow's weapon flag")?;
    let owner = take_owner(input, "an arrow's owner flag")?;
    let flags = input.u8()?;
    if flags & !(LEFT_OWNER | HAS_BEEN_SHOT | CRIT) != 0 {
        return Err(FormatError::Corrupt("an arrow's flags"));
    }
    let base_damage = Double(input.f64()?);
    let pierce_level = input.u8()?;
    let count = take_count(input, "an arrow's pierced entities")?;
    let mut pierced = Vec::with_capacity(count.min(input.0.len() / 8));
    for _ in 0..count {
        pierced.push(EntityId(input.u64()?));
    }
    let hit_sound = take_string(input)?;
    let in_ground = if take_flag(input, "an arrow's in-ground flag")? {
        Some(InGround {
            state: input.u16()?,
            since: input.u32()?,
        })
    } else {
        None
    };
    let shake_until = input.u32()?;
    let of_kind = match input.u8()? {
        ARROW_PLAIN => ArrowKind::Arrow,
        ARROW_SPECTRAL => ArrowKind::Spectral {
            glowing_duration: input.i32()?,
        },
        ARROW_TRIDENT => ArrowKind::Trident {
            dealt_damage: take_flag(input, "a trident's dealt-damage flag")?,
            returning: input.i32()?,
        },
        _ => return Err(FormatError::Corrupt("an arrow's kind")),
    };
    Ok(Arrow {
        pickup,
        pickup_stack,
        weapon,
        owner,
        left_owner: flags & LEFT_OWNER != 0,
        has_been_shot: flags & HAS_BEEN_SHOT != 0,
        base_damage,
        crit: flags & CRIT != 0,
        pierce_level,
        pierced,
        hit_sound,
        in_ground,
        shake_until,
        of_kind,
    })
}

/// An area effect cloud: its duration i32, wait time i32, reapplication delay i32,
/// radius f32, the `lived` its radius is as of u32, radius on use f32, radius per tick
/// f32, duration on use i32, its potion as a stack's `potion_contents` is written, its
/// duration scale f32, its owner, and its victims: a count u32 and each one's key u64
/// and the `lived` it may be affected again from u32, ascending by key.
fn put_cloud(out: &mut Vec<u8>, cloud: &Cloud) {
    out.extend_from_slice(&cloud.duration.to_be_bytes());
    out.extend_from_slice(&cloud.wait_time.to_be_bytes());
    out.extend_from_slice(&cloud.reapplication_delay.to_be_bytes());
    out.extend_from_slice(&cloud.radius.0.to_be_bytes());
    out.extend_from_slice(&cloud.radius_at.to_be_bytes());
    out.extend_from_slice(&cloud.radius_on_use.0.to_be_bytes());
    out.extend_from_slice(&cloud.radius_per_tick.0.to_be_bytes());
    out.extend_from_slice(&cloud.duration_on_use.to_be_bytes());
    put_potion_contents(out, &cloud.potion);
    out.extend_from_slice(&cloud.duration_scale.0.to_be_bytes());
    put_owner(out, cloud.owner);
    put_count(out, cloud.victims.len());
    for (key, until) in &cloud.victims {
        out.extend_from_slice(&key.0.to_be_bytes());
        out.extend_from_slice(&until.to_be_bytes());
    }
}

fn take_cloud(input: &mut Input<'_>) -> Result<Cloud, FormatError> {
    let duration = input.i32()?;
    let wait_time = input.i32()?;
    let reapplication_delay = input.i32()?;
    let radius = Float(input.f32()?);
    let radius_at = input.u32()?;
    let radius_on_use = Float(input.f32()?);
    let radius_per_tick = Float(input.f32()?);
    let duration_on_use = input.i32()?;
    let potion = take_potion_contents(input)?;
    let duration_scale = Float(input.f32()?);
    let owner = take_owner(input, "a cloud's owner flag")?;
    let count = take_count(input, "a cloud's victims")?;
    let mut victims: Vec<(EntityId, u32)> = Vec::with_capacity(count.min(input.0.len() / 12));
    for _ in 0..count {
        let victim = (EntityId(input.u64()?), input.u32()?);
        if victims.last().is_some_and(|last| last.0 >= victim.0) {
            return Err(FormatError::Corrupt("a cloud's victims out of order"));
        }
        victims.push(victim);
    }
    Ok(Cloud {
        duration,
        wait_time,
        reapplication_delay,
        radius,
        radius_at,
        radius_on_use,
        radius_per_tick,
        duration_on_use,
        potion,
        duration_scale,
        owner,
        victims,
    })
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
            let stack = take_stack(input)?;
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
            let item = take_optional_stack(input, "an item's presence")?;
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
        DATA_THROWN => {
            let stack = take_stack(input)?;
            let owner = if take_flag(input, "a thrown item's owner flag")? {
                Some(EntityId(input.u64()?))
            } else {
                None
            };
            let flags = input.u8()?;
            if flags & !(LEFT_OWNER | HAS_BEEN_SHOT) != 0 {
                return Err(FormatError::Corrupt("a thrown item's flags"));
            }
            Ok(EntityData::Thrown {
                stack,
                owner,
                left_owner: flags & LEFT_OWNER != 0,
                has_been_shot: flags & HAS_BEEN_SHOT != 0,
            })
        }
        DATA_ARROW => Ok(EntityData::Arrow(Box::new(take_arrow(input)?))),
        DATA_SPIT => {
            let owner = take_owner(input, "a spit's owner flag")?;
            let flags = input.u8()?;
            if flags & !(LEFT_OWNER | HAS_BEEN_SHOT) != 0 {
                return Err(FormatError::Corrupt("a spit's flags"));
            }
            Ok(EntityData::Spit(Spit {
                owner,
                left_owner: flags & LEFT_OWNER != 0,
                has_been_shot: flags & HAS_BEEN_SHOT != 0,
            }))
        }
        DATA_FIREWORK => {
            let stack = take_stack(input)?;
            let owner = take_owner(input, "a firework's owner flag")?;
            let flags = input.u8()?;
            if flags & !(LEFT_OWNER | HAS_BEEN_SHOT | SHOT_AT_ANGLE) != 0 {
                return Err(FormatError::Corrupt("a firework's flags"));
            }
            Ok(EntityData::Firework(Box::new(Firework {
                stack,
                owner,
                left_owner: flags & LEFT_OWNER != 0,
                has_been_shot: flags & HAS_BEEN_SHOT != 0,
                life: input.i32()?,
                lifetime: input.i32()?,
                shot_at_angle: flags & SHOT_AT_ANGLE != 0,
            })))
        }
        DATA_CLOUD => Ok(EntityData::Cloud(Box::new(take_cloud(input)?))),
        DATA_FANGS => Ok(EntityData::Fangs(Fangs {
            owner: take_owner(input, "the fangs' owner flag")?,
            warmup: input.i32()?,
        })),
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
    let portal = !entity.portal.is_idle();
    out.push(if entity.on_ground { ON_GROUND } else { 0 } | if portal { WITH_PORTAL } else { 0 });
    out.extend_from_slice(&entity.hops.to_be_bytes());
    out.extend_from_slice(&entity.lived.to_be_bytes());
    out.extend_from_slice(&entity.fresh_at.to_be_bytes());
    out.extend_from_slice(&entity.fall_distance.to_be_bytes());
    out.extend_from_slice(&entity.fire_ticks.to_be_bytes());
    put_riding(out, &entity.riding);
    put_data(out, &entity.data);
    if portal {
        put_portal(out, &entity.portal);
    }
}

/// Appends a portal state: its cooldown, then whether a clock follows and the clock.
fn put_portal(out: &mut Vec<u8>, portal: &PortalState) {
    out.extend_from_slice(&portal.cooldown.to_be_bytes());
    match &portal.clock {
        None => out.push(0),
        Some(clock) => {
            out.push(match clock.kind {
                PortalKind::Nether => 1,
                PortalKind::End => 2,
                PortalKind::Gateway => 3,
            });
            put_position(out, clock.entry);
            out.extend_from_slice(&clock.time.to_be_bytes());
            out.push(u8::from(clock.inside));
        }
    }
}

/// Reads what [`put_portal`] wrote.
fn take_portal(input: &mut Input<'_>) -> Result<PortalState, FormatError> {
    let cooldown = input.u32()?;
    let kind = match input.u8()? {
        0 => {
            return Ok(PortalState {
                clock: None,
                cooldown,
            });
        }
        1 => PortalKind::Nether,
        2 => PortalKind::End,
        3 => PortalKind::Gateway,
        _ => return Err(FormatError::Corrupt("portal kind")),
    };
    let clock = PortalClock {
        kind,
        entry: take_position(input)?,
        time: input.u32()?,
        inside: take_flag(input, "a portal clock's mark")?,
    };
    Ok(PortalState {
        clock: Some(clock),
        cooldown,
    })
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
    if flags & !(ON_GROUND | WITH_PORTAL) != 0 {
        return Err(FormatError::Corrupt("entity flags"));
    }
    let mut entity = Entity {
        portal: PortalState::default(),
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
    };
    if flags & WITH_PORTAL != 0 {
        entity.portal = take_portal(input)?;
        if entity.portal.is_idle() {
            // Written only where it is not, so that an entity has one record.
            return Err(FormatError::Corrupt("an idle portal state written"));
        }
    }
    Ok(entity)
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
        EntityField::Portal(portal) => {
            out.push(FIELD_PORTAL);
            put_portal(out, portal);
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
        FIELD_PORTAL => EntityField::Portal(take_portal(input)?),
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
    use clustine_world::ItemStack;
    use clustine_world::components::{
        Component, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, PotionContents,
    };
    use proptest::prelude::*;

    use super::*;

    pub(crate) fn item(key: u64, position: [f64; 3], thrower: Option<u128>) -> Entity {
        Entity {
            portal: Default::default(),
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
                stack: ItemStack::new(1, 64),
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
        expected.extend_from_slice(&[0, 0, 0, 0]); // no components added and none removed
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

    /// A thrown item's data, byte by byte, and what is refused of one: flags beyond its
    /// two, an owner flag that is neither 0 nor 1, and any cut.
    #[test]
    fn a_thrown_item_is_written_as_its_stack_owner_and_flags_and_nothing_else_is_read() {
        let thrown = EntityData::Thrown {
            stack: ItemStack::new(0x0102, 1),
            owner: Some(EntityId(0x0A0B)),
            left_owner: true,
            has_been_shot: true,
        };
        let mut out = Vec::new();
        put_data(&mut out, &thrown);
        assert_eq!(
            out,
            [
                13, 0, 0, 1, 2, 0, 0, 0, 1, // the tag, the item and the count
                0, 0, 0, 0, // no components added and none removed
                1, 0, 0, 0, 0, 0, 0, 0x0A, 0x0B, // an owner and its key
                3,    // left its owner and shot
            ]
        );
        assert_eq!(take_data(&mut Input(&out)), Ok(thrown));
        let unowned = EntityData::Thrown {
            stack: ItemStack::new(7, 1),
            owner: None,
            left_owner: false,
            has_been_shot: false,
        };
        let mut short = Vec::new();
        put_data(&mut short, &unowned);
        assert_eq!(short, [13, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(take_data(&mut Input(&short)), Ok(unowned));
        let mut flags = out.clone();
        *flags.last_mut().expect("the flags") = 4;
        assert_eq!(
            take_data(&mut Input(&flags)),
            Err(FormatError::Corrupt("a thrown item's flags"))
        );
        let mut owner_flag = out.clone();
        owner_flag[13] = 2;
        assert_eq!(
            take_data(&mut Input(&owner_flag)),
            Err(FormatError::Corrupt("a thrown item's owner flag"))
        );
        for length in 0..out.len() {
            assert!(
                take_data(&mut Input(&out[..length])).is_err(),
                "cut at {length}"
            );
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

    /// Writes `data` and reads it back, and checks that the read took every byte.
    fn round_trip(data: &EntityData) {
        let mut out = Vec::new();
        put_data(&mut out, data);
        let mut input = Input(&out);
        assert_eq!(take_data(&mut input).as_ref(), Ok(data));
        assert!(input.0.is_empty(), "{} bytes left", input.0.len());
        for length in 0..out.len() {
            assert!(
                take_data(&mut Input(&out[..length])).is_err(),
                "cut at {length}"
            );
        }
    }

    fn rocket() -> ItemStack {
        let mut stack = ItemStack::new(1_100, 1);
        stack.components.set(Component::Fireworks(Fireworks {
            flight_duration: 2,
            explosions: vec![
                FireworkExplosion {
                    shape: FireworkShape::Creeper,
                    colours: vec![0x00ff_0000, 0x0000_ff00],
                    fade_colours: vec![0x0000_00ff],
                    trail: true,
                    twinkle: false,
                },
                FireworkExplosion {
                    shape: FireworkShape::Burst,
                    colours: vec![0x00ff_ffff],
                    fade_colours: Vec::new(),
                    trail: false,
                    twinkle: true,
                },
            ],
        }));
        stack
    }

    fn tipped_arrow() -> ItemStack {
        let mut stack = ItemStack::new(803, 1);
        stack
            .components
            .set(Component::PotionContents(PotionContents {
                potion: Some("minecraft:long_poison".into()),
                ..PotionContents::default()
            }));
        stack
    }

    #[test]
    fn an_arrow_of_each_kind_with_every_field_set_reads_as_it_was_written() {
        let mut bow = ItemStack::new(810, 1);
        bow.components.set(Component::Enchantments(vec![(
            "minecraft:piercing".into(),
            4,
        )]));
        let in_the_ground = Arrow {
            pickup: Pickup::Allowed,
            pickup_stack: tipped_arrow(),
            weapon: Some(bow.clone()),
            owner: Some(EntityId(0x0102_0304_0506_0708)),
            left_owner: true,
            has_been_shot: true,
            base_damage: Double(2.5),
            crit: true,
            pierce_level: 4,
            pierced: vec![EntityId(9), EntityId(3), EntityId(u64::MAX)],
            hit_sound: "minecraft:item.crossbow.hit".into(),
            in_ground: Some(InGround {
                state: 0x1234,
                since: 0x0010_0000,
            }),
            shake_until: 0x0010_0007,
            of_kind: ArrowKind::Arrow,
        };
        round_trip(&EntityData::Arrow(Box::new(in_the_ground.clone())));
        let flying_spectral = Arrow {
            pickup: Pickup::CreativeOnly,
            pickup_stack: ItemStack::new(804, 1),
            weapon: Some(bow),
            in_ground: None,
            pierced: Vec::new(),
            of_kind: ArrowKind::Spectral {
                glowing_duration: 200,
            },
            ..in_the_ground.clone()
        };
        round_trip(&EntityData::Arrow(Box::new(flying_spectral.clone())));
        round_trip(&EntityData::Arrow(Box::new(Arrow {
            in_ground: in_the_ground.in_ground,
            ..flying_spectral.clone()
        })));
        let trident = Arrow {
            pickup: Pickup::Disallowed,
            pickup_stack: ItemStack::new(1_150, 1),
            weapon: None,
            owner: None,
            left_owner: false,
            has_been_shot: false,
            crit: false,
            hit_sound: "minecraft:item.trident.hit".into(),
            of_kind: ArrowKind::Trident {
                dealt_damage: true,
                returning: -7,
            },
            ..in_the_ground
        };
        round_trip(&EntityData::Arrow(Box::new(trident.clone())));
        round_trip(&EntityData::Arrow(Box::new(Arrow {
            in_ground: None,
            of_kind: ArrowKind::Trident {
                dealt_damage: false,
                returning: 0,
            },
            ..trident
        })));
    }

    #[test]
    fn a_spit_reads_as_it_was_written() {
        round_trip(&EntityData::Spit(Spit {
            owner: Some(EntityId(77)),
            left_owner: true,
            has_been_shot: true,
        }));
        round_trip(&EntityData::Spit(Spit {
            owner: None,
            left_owner: false,
            has_been_shot: true,
        }));
    }

    #[test]
    fn a_firework_whose_rocket_has_two_explosions_reads_as_it_was_written() {
        let firework = Firework {
            stack: rocket(),
            owner: Some(EntityId(0x0A0B)),
            left_owner: true,
            has_been_shot: true,
            life: 13,
            lifetime: 37,
            shot_at_angle: true,
        };
        round_trip(&EntityData::Firework(Box::new(firework.clone())));
        round_trip(&EntityData::Firework(Box::new(Firework {
            owner: None,
            left_owner: false,
            shot_at_angle: false,
            life: 0,
            ..firework
        })));
    }

    fn cloud() -> Cloud {
        let effect = |effect: &str, amplifier, duration| EffectInstance {
            effect: effect.into(),
            amplifier,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
        };
        Cloud {
            duration: 600,
            wait_time: 10,
            reapplication_delay: 20,
            radius: Float(3.0),
            radius_at: 0x0002_0000,
            radius_on_use: Float(-0.5),
            radius_per_tick: Float(-0.005),
            duration_on_use: -20,
            potion: PotionContents {
                potion: Some("minecraft:strong_harming".into()),
                custom_colour: Some(0x0012_3456),
                custom_effects: vec![
                    effect("minecraft:slowness", 3, 400),
                    effect("minecraft:weakness", 0, -1),
                ],
                custom_name: Some("lingering".into()),
            },
            duration_scale: Float(0.25),
            owner: Some(EntityId(5)),
            victims: vec![
                (EntityId(1), 0x0002_0014),
                (EntityId(4), 3),
                (EntityId(u64::MAX), u32::MAX),
            ],
        }
    }

    #[test]
    fn a_cloud_with_two_custom_effects_and_three_victims_reads_as_it_was_written() {
        round_trip(&EntityData::Cloud(Box::new(cloud())));
        round_trip(&EntityData::Cloud(Box::new(Cloud {
            potion: PotionContents::default(),
            owner: None,
            victims: Vec::new(),
            ..cloud()
        })));
    }

    /// A cloud's victims are kept ascending by key, each once, and a record that has
    /// them otherwise is refused.
    #[test]
    fn a_cloud_whose_victims_are_out_of_order_or_twice_is_refused() {
        for victims in [
            vec![(EntityId(4), 0), (EntityId(1), 0)],
            vec![(EntityId(4), 0), (EntityId(4), 1)],
        ] {
            let mut out = Vec::new();
            put_data(
                &mut out,
                &EntityData::Cloud(Box::new(Cloud { victims, ..cloud() })),
            );
            assert_eq!(
                take_data(&mut Input(&out)),
                Err(FormatError::Corrupt("a cloud's victims out of order"))
            );
        }
    }

    #[test]
    fn the_fangs_read_as_they_were_written() {
        round_trip(&EntityData::Fangs(Fangs {
            owner: Some(EntityId(0x0102)),
            warmup: 12,
        }));
        round_trip(&EntityData::Fangs(Fangs {
            owner: None,
            warmup: -3,
        }));
    }

    /// An arrow's data, byte by byte, and what is refused of one: flags beyond its three,
    /// a pickup, an in-ground flag or a kind not known, and any cut.
    #[test]
    fn known_answer_of_an_arrow() {
        let arrow = EntityData::Arrow(Box::new(Arrow {
            pickup: Pickup::Allowed,
            pickup_stack: ItemStack::new(0x0102, 1),
            weapon: Some(ItemStack::new(5, 1)),
            owner: Some(EntityId(0x0A0B)),
            left_owner: true,
            has_been_shot: false,
            base_damage: Double(2.0),
            crit: true,
            pierce_level: 1,
            pierced: vec![EntityId(7)],
            hit_sound: "hit".into(),
            in_ground: Some(InGround {
                state: 0x0123,
                since: 0x40,
            }),
            shake_until: 0x47,
            of_kind: ArrowKind::Trident {
                dealt_damage: true,
                returning: 3,
            },
        }));
        let mut out = Vec::new();
        put_data(&mut out, &arrow);
        let expected: Vec<u8> = [
            &[14][..],                                // the tag
            &[1],                                     // pickup allowed
            &[0, 0, 1, 2, 0, 0, 0, 1, 0, 0, 0, 0],    // the pickup stack, plain
            &[1, 0, 0, 0, 5, 0, 0, 0, 1, 0, 0, 0, 0], // a weapon, a plain stack
            &[1, 0, 0, 0, 0, 0, 0, 0x0A, 0x0B],       // an owner and its key
            &[5],                                     // left its owner and critical, not shot
            &[0x40, 0, 0, 0, 0, 0, 0, 0],             // base damage 2.0
            &[1],                                     // pierce level
            &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7],    // one entity pierced, its key
            &[0, 0, 0, 3, b'h', b'i', b't'],          // the hit sound
            &[1, 0x01, 0x23, 0, 0, 0, 0x40],          // in the ground: the state, since
            &[0, 0, 0, 0x47],                         // its shake ends
            &[2, 1, 0, 0, 0, 3], // a trident that has dealt its damage, returning
        ]
        .concat();
        assert_eq!(out, expected);
        assert_eq!(take_data(&mut Input(&out)), Ok(arrow));
        let refused = |at: usize, value: u8, what: &'static str| {
            let mut bytes = out.clone();
            bytes[at] = value;
            assert_eq!(
                take_data(&mut Input(&bytes)),
                Err(FormatError::Corrupt(what)),
                "{at}"
            );
        };
        refused(1, 3, "an arrow's pickup");
        refused(14, 2, "an arrow's weapon flag");
        refused(27, 2, "an arrow's owner flag");
        refused(36, 8, "an arrow's flags");
        refused(65, 2, "an arrow's in-ground flag");
        refused(76, 3, "an arrow's kind");
        refused(77, 2, "a trident's dealt-damage flag");
        for length in 0..out.len() {
            assert!(
                take_data(&mut Input(&out[..length])).is_err(),
                "cut at {length}"
            );
        }
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
            portals(),
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
                    portal,
                )| {
                    Entity {
                        portal,
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

    /// Idle half the time, as most entities' are.
    fn portals() -> impl Strategy<Value = PortalState> {
        let clock = (
            0..3u8,
            any::<(i32, i32, i32)>(),
            any::<u32>(),
            any::<bool>(),
        )
            .prop_map(|(kind, (x, y, z), time, inside)| PortalClock {
                kind: [PortalKind::Nether, PortalKind::End, PortalKind::Gateway][kind as usize],
                entry: BlockPos::new(x, y, z),
                time,
                inside,
            });
        prop_oneof![
            Just(PortalState::default()),
            (proptest::option::of(clock), any::<u32>())
                .prop_map(|(clock, cooldown)| PortalState { clock, cooldown }),
        ]
    }

    fn ridings() -> impl Strategy<Value = Riding> {
        let seat = (
            any::<u64>(),
            any::<bool>(),
            any::<Option<(u16, u64, bool)>>(),
        )
            .prop_map(|(key, player, pending)| Seat {
                key: EntityId(key),
                // Only a player's seat is kept for them on their way.
                player: player || pending.is_some(),
                pending: pending.map(|(left, edge, granted)| Pending {
                    granted,
                    left,
                    edge: EdgeId(edge),
                }),
            });
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

    /// `data` with every stack it holds given a component of every kind.
    fn with_every_component_in(mut data: EntityData) -> EntityData {
        let rich = |stack: &mut ItemStack| {
            *stack = ItemStack {
                item: stack.item,
                count: stack.count,
                ..crate::stacks::tests::with_every_component()
            };
        };
        match &mut data {
            EntityData::Item { stack, .. } | EntityData::Thrown { stack, .. } => rich(stack),
            EntityData::Arrow(arrow) => {
                rich(&mut arrow.pickup_stack);
                arrow.weapon.iter_mut().for_each(rich);
            }
            EntityData::Firework(firework) => rich(&mut firework.stack),
            EntityData::ItemFrame { item, .. } => item.iter_mut().for_each(rich),
            EntityData::Minecart { contents, .. }
            | EntityData::Boat { contents, .. }
            | EntityData::ArmorStand {
                equipment: contents,
                ..
            } => contents.iter_mut().for_each(|(_, stack)| rich(stack)),
            _ => {}
        }
        data
    }

    fn contents() -> impl Strategy<Value = Contents> {
        prop::collection::btree_map(any::<u8>(), any::<(i32, i32)>(), 0..6).prop_map(|slots| {
            slots
                .into_iter()
                .map(|(slot, (item, count))| (slot, ItemStack::new(item, count)))
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

    fn optional_stacks() -> impl Strategy<Value = Option<ItemStack>> {
        any::<Option<(i32, i32)>>()
            .prop_map(|stack| stack.map(|(item, count)| ItemStack::new(item, count)))
    }

    fn arrows() -> impl Strategy<Value = Arrow> {
        let kind = prop_oneof![
            Just(ArrowKind::Arrow),
            any::<i32>().prop_map(|glowing_duration| ArrowKind::Spectral { glowing_duration }),
            any::<(bool, i32)>().prop_map(|(dealt_damage, returning)| ArrowKind::Trident {
                dealt_damage,
                returning,
            }),
        ];
        let pickup = prop_oneof![
            Just(Pickup::Disallowed),
            Just(Pickup::Allowed),
            Just(Pickup::CreativeOnly),
        ];
        (
            (pickup, any::<(i32, i32)>(), optional_stacks()),
            (any::<Option<u64>>(), any::<[bool; 3]>(), any::<f64>()),
            (any::<u8>(), prop::collection::vec(any::<u64>(), 0..6)),
            ("[a-z:._]{0,24}", any::<Option<(u16, u32)>>(), any::<u32>()),
            kind,
        )
            .prop_map(
                |(
                    (pickup, (item, count), weapon),
                    (owner, [left_owner, has_been_shot, crit], base_damage),
                    (pierce_level, pierced),
                    (hit_sound, in_ground, shake_until),
                    of_kind,
                )| Arrow {
                    pickup,
                    pickup_stack: ItemStack::new(item, count),
                    weapon,
                    owner: owner.map(EntityId),
                    left_owner,
                    has_been_shot,
                    base_damage: Double(base_damage),
                    crit,
                    pierce_level,
                    pierced: pierced.into_iter().map(EntityId).collect(),
                    hit_sound,
                    in_ground: in_ground.map(|(state, since)| InGround { state, since }),
                    shake_until,
                    of_kind,
                },
            )
    }

    fn effects() -> impl Strategy<Value = EffectInstance> {
        ("[a-z:_]{0,16}", any::<(i32, i32, [bool; 3])>()).prop_map(
            |(effect, (amplifier, duration, [ambient, visible, show_icon]))| EffectInstance {
                effect,
                amplifier,
                duration,
                ambient,
                visible,
                show_icon,
            },
        )
    }

    fn clouds() -> impl Strategy<Value = Cloud> {
        let potion = (
            prop::option::of("[a-z:_]{0,16}"),
            any::<Option<i32>>(),
            prop::collection::vec(effects(), 0..3),
            prop::option::of(".{0,8}"),
        )
            .prop_map(|(potion, custom_colour, custom_effects, custom_name)| {
                PotionContents {
                    potion,
                    custom_colour,
                    custom_effects,
                    custom_name,
                }
            });
        (
            any::<(i32, i32, i32, i32)>(),
            any::<(f32, u32, f32, f32, f32)>(),
            potion,
            any::<Option<u64>>(),
            // Ascending by key, each once.
            prop::collection::btree_map(any::<u64>(), any::<u32>(), 0..5),
        )
            .prop_map(
                |(
                    (duration, wait_time, reapplication_delay, duration_on_use),
                    (radius, radius_at, radius_on_use, radius_per_tick, duration_scale),
                    potion,
                    owner,
                    victims,
                )| Cloud {
                    duration,
                    wait_time,
                    reapplication_delay,
                    radius: Float(radius),
                    radius_at,
                    radius_on_use: Float(radius_on_use),
                    radius_per_tick: Float(radius_per_tick),
                    duration_on_use,
                    potion,
                    duration_scale: Float(duration_scale),
                    owner: owner.map(EntityId),
                    victims: victims
                        .into_iter()
                        .map(|(key, until)| (EntityId(key), until))
                        .collect(),
                },
            )
    }

    fn datas() -> impl Strategy<Value = EntityData> {
        prop_oneof![
            arrows().prop_map(|arrow| EntityData::Arrow(Box::new(arrow))),
            any::<(Option<u64>, bool, bool)>().prop_map(|(owner, left_owner, has_been_shot)| {
                EntityData::Spit(Spit {
                    owner: owner.map(EntityId),
                    left_owner,
                    has_been_shot,
                })
            }),
            any::<((i32, i32), Option<u64>, [bool; 3], i32, i32)>().prop_map(
                |(
                    (item, count),
                    owner,
                    [left_owner, has_been_shot, shot_at_angle],
                    life,
                    lifetime,
                )| {
                    EntityData::Firework(Box::new(Firework {
                        stack: ItemStack::new(item, count),
                        owner: owner.map(EntityId),
                        left_owner,
                        has_been_shot,
                        life,
                        lifetime,
                        shot_at_angle,
                    }))
                }
            ),
            clouds().prop_map(|cloud| EntityData::Cloud(Box::new(cloud))),
            any::<(Option<u64>, i32)>().prop_map(|(owner, warmup)| EntityData::Fangs(Fangs {
                owner: owner.map(EntityId),
                warmup,
            })),
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
                            stack: ItemStack::new(item, count),
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
                            item: item.map(|(item, count)| ItemStack::new(item, count)),
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
            any::<(i32, i32, Option<u64>, bool, bool)>().prop_map(
                |(item, count, owner, left_owner, has_been_shot)| EntityData::Thrown {
                    stack: ItemStack::new(item, count),
                    owner: owner.map(EntityId),
                    left_owner,
                    has_been_shot,
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
            portals().prop_map(EntityField::Portal),
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

        /// Every stack an entity holds (an item's, a frame's, a thrown item's, a cart's
        /// or a boat's contents, a stand's equipment) keeps its components.
        #[test]
        fn every_stack_an_entity_holds_keeps_a_component_of_every_kind(data in datas()) {
            let data = with_every_component_in(data);
            let mut out = Vec::new();
            put_data(&mut out, &data);
            let mut input = Input(&out);
            prop_assert_eq!(take_data(&mut input), Ok(data));
            prop_assert!(input.0.is_empty());
        }

        #[test]
        fn arbitrary_bytes_never_make_an_entities_file_panic(bytes: Vec<u8>) {
            let _ = EntitiesFile::decode(&bytes);
            let _ = take_entity_changes(&mut Input(&bytes));
        }
    }
}
