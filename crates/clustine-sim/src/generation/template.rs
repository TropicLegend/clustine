//! The entities structures make as their chunks are generated
//! (`docs/groundwork/n7l-plan.md`, sections 3.4, 3.5 and 5.4; `docs/groundwork/n7-brief.md`,
//! sections 8.10 to 8.17): a template's entities from their tags, and the mobs and
//! minecarts the pieces' own code makes.
//!
//! A mob is made as `EntityType.create` and `load` make it: constructed of its own
//! stream (keyed by its making), then what its load reads of its template's tag
//! ([`types::from_template`]), then put where its template's transform or its piece's
//! code says, then, where it is finalised, its `finalizeSpawn` for the reason
//! `STRUCTURE`, drawing from the run's region random. A mob made in the middle of that
//! (a baby zombie villager's chicken) is made by the run too ([`Beside`]), and the two
//! come back as one unit, the vehicle first. A type no part has built yet makes nothing
//! and draws nothing, as its egg makes nothing.
//!
//! Armour stands and cushions are made as their records are when a player places one,
//! from what their tags say; a mineshaft's chest minecart as a placed cart is, empty.

use clustine_data::{BIOME_SPAWNS, ENTITY_TYPES, entity_types, item_id_by_name};
use clustine_world::generation::Placed;
use clustine_world::{
    Attached, Biome, Contents, Double, Entity, EntityData, EntityId, Float, Hurt, ItemStack, Seat,
};

use crate::living::{MAX_AIR, slots};
use crate::mobs::making;
use crate::mobs::types::{self, Companion, CompanionSeat, Companions, SpawnReason, Spawning};
use crate::region::{Random, insecure_uuid};

mod end_crystal;

/// A tag's value, as far as a template's entity is read: numbers of every kind as a
/// double, strings, lists and compounds (members in their order), and the arrays read
/// as lists of numbers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tag {
    Number(f64),
    Text(String),
    List(Vec<Tag>),
    Compound(Vec<(String, Tag)>),
}

impl Tag {
    /// The member `name` of a compound.
    pub(crate) fn get(&self, name: &str) -> Option<&Tag> {
        match self {
            Self::Compound(members) => members
                .iter()
                .find(|(member, _)| member == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    fn list(&self) -> &[Tag] {
        match self {
            Self::List(items) => items,
            _ => &[],
        }
    }
}

/// A tag in the binary form of NBT without a name, its type byte first; `None` for one
/// cut short, of a type NBT does not have, or nested deeper than the game allows.
pub(crate) fn parse(tag: &[u8]) -> Option<Tag> {
    let (&kind, rest) = tag.split_first()?;
    read(kind, rest, 0).map(|(value, _)| value)
}

fn read(kind: u8, data: &[u8], depth: u32) -> Option<(Tag, &[u8])> {
    if depth > 512 {
        return None;
    }
    let bytes =
        |count: usize| -> Option<(&[u8], &[u8])> { Some((data.get(..count)?, data.get(count..)?)) };
    let length = |data: &[u8]| -> Option<usize> {
        usize::try_from(i32::from_be_bytes(data.get(..4)?.try_into().ok()?)).ok()
    };
    Some(match kind {
        1 => {
            let (value, rest) = bytes(1)?;
            (Tag::Number(f64::from(value[0] as i8)), rest)
        }
        2 => {
            let (value, rest) = bytes(2)?;
            let value = i16::from_be_bytes(value.try_into().ok()?);
            (Tag::Number(f64::from(value)), rest)
        }
        3 => {
            let (value, rest) = bytes(4)?;
            let value = i32::from_be_bytes(value.try_into().ok()?);
            (Tag::Number(f64::from(value)), rest)
        }
        4 => {
            let (value, rest) = bytes(8)?;
            let value = i64::from_be_bytes(value.try_into().ok()?);
            (Tag::Number(value as f64), rest)
        }
        5 => {
            let (value, rest) = bytes(4)?;
            let value = f32::from_be_bytes(value.try_into().ok()?);
            (Tag::Number(f64::from(value)), rest)
        }
        6 => {
            let (value, rest) = bytes(8)?;
            (
                Tag::Number(f64::from_be_bytes(value.try_into().ok()?)),
                rest,
            )
        }
        7 => {
            let count = length(data)?;
            let values = data.get(4..4 + count)?;
            let items = values
                .iter()
                .map(|&value| Tag::Number(f64::from(value as i8)))
                .collect();
            (Tag::List(items), data.get(4 + count..)?)
        }
        8 => {
            let (text, rest) = string(data)?;
            (Tag::Text(String::from_utf8_lossy(text).into_owned()), rest)
        }
        9 => {
            let (&element, after) = data.split_first()?;
            let count = length(after)?;
            let mut rest = after.get(4..)?;
            let mut items = Vec::new();
            for _ in 0..count {
                let (item, next) = read(element, rest, depth + 1)?;
                items.push(item);
                rest = next;
            }
            (Tag::List(items), rest)
        }
        10 => {
            let mut rest = data;
            let mut members = Vec::new();
            loop {
                let (&inner, after) = rest.split_first()?;
                if inner == 0 {
                    break (Tag::Compound(members), after);
                }
                let (name, after) = string(after)?;
                let (value, next) = read(inner, after, depth + 1)?;
                members.push((String::from_utf8_lossy(name).into_owned(), value));
                rest = next;
            }
        }
        11 | 12 => {
            let width = if kind == 11 { 4 } else { 8 };
            let count = length(data)?;
            let values = data.get(4..4 + count.checked_mul(width)?)?;
            let items = values
                .chunks_exact(width)
                .map(|value| {
                    Tag::Number(if width == 4 {
                        f64::from(i32::from_be_bytes([value[0], value[1], value[2], value[3]]))
                    } else {
                        i64::from_be_bytes([
                            value[0], value[1], value[2], value[3], value[4], value[5], value[6],
                            value[7],
                        ]) as f64
                    })
                })
                .collect();
            (Tag::List(items), data.get(4 + count * width..)?)
        }
        _ => return None,
    })
}

/// A string as NBT writes one, its length first, and what follows it.
fn string(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = usize::from(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    Some((data.get(2..2 + length)?, data.get(2 + length..)?))
}

/// The entity type a tag's `id` names, by its index in `clustine_data::ENTITY_TYPES`.
fn kind_named(id: &str) -> Option<u16> {
    let at = ENTITY_TYPES.iter().position(|name| *name == id)?;
    u16::try_from(at).ok()
}

/// The name of the biome `biome`, which a mob's `finalizeSpawn` reads.
fn biome_name(biome: Biome) -> Option<&'static str> {
    BIOME_SPAWNS
        .get(usize::from(biome.0))
        .map(|biome| biome.name)
}

/// The block a position is in (`BlockPos.containing`).
fn containing(position: [f64; 3]) -> clustine_world::BlockPos {
    crate::physics::containing(position)
}

/// A template's entity, as `placeEntities` makes it (see the module's head): `None`
/// where it is no entity this makes; else its unit, the vehicle first.
pub(crate) fn template_entity(
    world: u64,
    tag: &[u8],
    finalise: bool,
    placed: Placed<'_>,
) -> Vec<Entity> {
    let Some(parsed) = parse(tag) else {
        return Vec::new();
    };
    let Some(kind) = parsed.get("id").and_then(Tag::text).and_then(kind_named) else {
        return Vec::new();
    };
    match i32::from(kind) {
        entity_types::ARMOR_STAND => armour_stand(world, kind, &parsed, placed)
            .into_iter()
            .collect(),
        entity_types::CUSHION => cushion(world, kind, &parsed, placed).into_iter().collect(),
        entity_types::END_CRYSTAL => end_crystal::end_crystal(world, kind, &parsed, placed)
            .into_iter()
            .collect(),
        _ => make_mob(world, kind, Some(tag), false, finalise, placed),
    }
}

/// A mob a structure's code makes (the swamp hut's witch and cat, the monument's elders,
/// the mansion's illagers and allays, the ocean ruins' drowned), always finalised.
pub(crate) fn structure_mob(
    world: u64,
    kind: u16,
    persistent: bool,
    placed: Placed<'_>,
) -> Vec<Entity> {
    make_mob(world, kind, None, persistent, true, placed)
}

/// An entity that is no mob a structure's code makes: a mineshaft's chest minecart,
/// empty, as a placed cart is made. `None` for any other type.
pub(crate) fn structure_entity(world: u64, kind: u16, placed: Placed<'_>) -> Option<Entity> {
    if !matches!(
        i32::from(kind),
        entity_types::CHEST_MINECART | entity_types::MINECART
    ) {
        return None;
    }
    let Placed {
        position,
        yaw,
        pitch,
        makings,
        ..
    } = placed;
    let key = makings.next()?.key()?;
    // `Entity.<init>`: the UUID's two longs from the new entity's own random.
    let mut own = Random::of_generated(world, key);
    let uuid = insecure_uuid(own.next_u64(), own.next_u64());
    Some(Entity {
        portal: Default::default(),
        key,
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Minecart {
            hurt: Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    })
}

/// A mob of the type `kind`: constructed, loaded from `tag` where a template has one,
/// kept from despawning where `persistent`, put where `placed` says and, where
/// `finalise`, finalised for the reason `STRUCTURE` (see the module's head). Nothing,
/// having drawn nothing, for a type not built yet.
fn make_mob(
    world: u64,
    kind: u16,
    tag: Option<&[u8]>,
    persistent: bool,
    finalise: bool,
    placed: Placed<'_>,
) -> Vec<Entity> {
    if !types::is_built(kind) {
        return Vec::new();
    }
    let Placed {
        position,
        yaw,
        pitch,
        structure,
        biome,
        makings,
        level,
    } = placed;
    let Some(making) = makings.next() else {
        return Vec::new();
    };
    let Some(mut own) = super::making_stream(world, making) else {
        return Vec::new();
    };
    let key = making.key().expect("a making with a stream has a key");
    let (uuid, mut mob) = making::construct(kind, &mut own, yaw);
    if let Some(tag) = tag {
        types::from_template(kind, &mut mob, tag);
    }
    if persistent {
        mob.body.flags.persistent = true;
    }
    let mut beside = Beside {
        world,
        makings,
        made: Vec::new(),
    };
    if finalise {
        let mut group = None;
        making::finalize(
            kind,
            &mut Spawning {
                mob: &mut mob,
                level,
                own: &mut own,
                biome: biome_name(biome),
                at: containing(position),
                // A `WorldGenRegion` has no time layer: its day is the first, under a
                // full moon, at the game's default difficulty, easy, as a chunk's
                // spawn's (`docs/groundwork/n7l-plan.md`, section 3.6).
                day: 0,
                game_time: 0,
                reason: SpawnReason::Structure,
                group: &mut group,
                structure,
                moon_brightness: 1.0,
                difficulty: (0.0, 0.0),
                companions: Some(&mut beside),
            },
        );
    }
    let mut record = making::fresh_record(key, kind, uuid, position, yaw, &mob);
    record.pitch = pitch;
    unit(record, beside.made)
}

/// A mob made with those made beside it, as the run keeps them (ADR-0028, section 5: a
/// unit, each member a record of the root's chunk), the vehicle first, then the mob,
/// then its riders; each names the others by its making's key, which the scheduler
/// replaces with their keys.
///
/// A mob whose vehicle was made in its `finalizeSpawn` is kept as it was when that
/// vehicle was added: the game's proto chunk saves the vehicle with its passenger then,
/// and does not save a passenger again when it is added itself
/// (`docs/groundwork/n7l-plan.md`, section 9 item 9). Its later draws were made all the
/// same, from the region's random, which is what counts of them.
pub(crate) fn unit(mut record: Entity, made: Vec<Companion>) -> Vec<Entity> {
    let mut vehicles = Vec::new();
    let mut riders = Vec::new();
    for companion in made {
        match companion.seat {
            CompanionSeat::Vehicle => vehicles.push(companion),
            CompanionSeat::Passenger => riders.push(companion),
        }
    }
    let mut out = Vec::new();
    let mut vehicles = vehicles.into_iter();
    if let Some(vehicle) = vehicles.next() {
        let mut mount = vehicle.entity;
        mount.riding.passengers.push(Seat {
            key: record.key,
            player: false,
            pending: None,
        });
        record.riding.vehicle = Some(mount.key);
        record.data = vehicle.beside.encode();
        out.push(mount);
    }
    // No type makes two vehicles for itself; one that did would leave the others
    // standing by it.
    out.extend(vehicles.map(|vehicle| vehicle.entity));
    let riders: Vec<Entity> = riders
        .into_iter()
        .map(|rider| {
            let mut rider = rider.entity;
            record.riding.passengers.push(Seat {
                key: rider.key,
                player: false,
                pending: None,
            });
            rider.riding.vehicle = Some(record.key);
            rider
        })
        .collect();
    out.push(record);
    out.extend(riders);
    out
}

/// The run's side of a mob made beside another (`types::Companions`): the next of the
/// run's makings and its stream. A region of world generation holds no entity before
/// the one being made (`docs/groundwork/n7-brief.md`, section 8.0 item 9), so a baby
/// zombie's search for a chicken to ride finds none, and none is ever ridden that way.
pub(crate) struct Beside<'m> {
    pub(crate) world: u64,
    pub(crate) makings: &'m mut clustine_world::generation::Makings,
    pub(crate) made: Vec<Companion>,
}

impl Companions for Beside<'_> {
    fn next(&mut self) -> Option<(EntityId, Random)> {
        let making = self.makings.next()?;
        let key = making.key()?;
        Some((key, Random::of_generated(self.world, key)))
    }

    fn add(&mut self, companion: Companion) {
        self.made.push(companion);
    }

    fn nearby(&self, _: &crate::physics::aabb::Aabb) -> Vec<(EntityId, u16, bool)> {
        Vec::new()
    }

    fn ride(&mut self, _: EntityId) {
        // Never asked: nothing is nearby.
    }
}

/// `ArmorStand`'s `DATA_CLIENT_FLAGS` bits as its tag names them, with the record's
/// invisibility (`region/stands.rs`).
const STAND_FLAGS: [(&str, u8); 5] = [
    ("Small", 1),
    ("ShowArms", 4),
    ("NoBasePlate", 8),
    ("Marker", 16),
    ("Invisible", 32),
];

/// The parts of a stand's pose as its tag names them, in the record's order.
const POSE_PARTS: [&str; 6] = ["Head", "Body", "LeftArm", "RightArm", "LeftLeg", "RightLeg"];

/// `EquipmentSlot`'s serialised names, by the record's ordinals.
const SLOT_NAMES: [(&str, u8); 8] = [
    ("mainhand", slots::MAIN_HAND),
    ("offhand", slots::OFF_HAND),
    ("feet", slots::FEET),
    ("legs", slots::LEGS),
    ("chest", slots::CHEST),
    ("head", slots::HEAD),
    ("body", slots::BODY),
    ("saddle", slots::SADDLE),
];

/// An armour stand of a template (the taiga armourer's): its own draws as a placed
/// stand's (its UUID, then `LivingEntity`'s yaw, which `snapTo` replaces), and what its
/// load reads: its flags, disabled slots, pose, equipment (`equipment`, by slot),
/// health and air. Not a mob: nothing is finalised.
fn armour_stand(world: u64, kind: u16, tag: &Tag, placed: Placed<'_>) -> Option<Entity> {
    let Placed {
        position,
        yaw,
        pitch,
        makings,
        ..
    } = placed;
    let key = makings.next()?.key()?;
    let mut own = Random::of_generated(world, key);
    let uuid = insecure_uuid(own.next_u64(), own.next_u64());
    let _constructor_yaw = own.next_float();
    let flag = |name: &str| {
        tag.get(name)
            .and_then(Tag::number)
            .is_some_and(|v| v != 0.0)
    };
    let flags = STAND_FLAGS
        .iter()
        .filter(|(name, _)| flag(name))
        .fold(0, |flags, (_, bit)| flags | bit);
    let disabled_slots = tag
        .get("DisabledSlots")
        .and_then(Tag::number)
        .map_or(0, |value| value as i32);
    let mut pose = crate::region::STAND_POSE;
    if let Some(given) = tag.get("Pose") {
        for (part, name) in pose.iter_mut().zip(POSE_PARTS) {
            let angles: Vec<f32> = given
                .get(name)
                .map(Tag::list)
                .unwrap_or_default()
                .iter()
                .filter_map(|angle| angle.number().map(|value| value as f32))
                .collect();
            if let [x, y, z] = angles[..] {
                *part = [x, y, z];
            }
        }
    }
    let mut equipment: Contents = Vec::new();
    if let Some(Tag::Compound(worn)) = tag.get("equipment") {
        for (name, stack) in worn {
            let Some(&(_, slot)) = SLOT_NAMES.iter().find(|(known, _)| known == name) else {
                continue;
            };
            let Some(item) = stack
                .get("id")
                .and_then(Tag::text)
                .and_then(item_id_by_name)
            else {
                continue;
            };
            let count = stack
                .get("count")
                .and_then(Tag::number)
                .map_or(1, |count| count as i32);
            equipment.push((slot, ItemStack::new(item, count)));
        }
    }
    equipment.sort_by_key(|(slot, _)| *slot);
    let health = tag
        .get("Health")
        .and_then(Tag::number)
        .map_or(crate::region::STAND_HEALTH, |health| health as f32);
    let air = tag
        .get("Air")
        .and_then(Tag::number)
        .map_or(MAX_AIR as i16, |air| air as i16);
    Some(Entity {
        portal: Default::default(),
        key,
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::ArmorStand {
            flags,
            disabled_slots,
            pose: Box::new(pose.map(|part| part.map(Float))),
            equipment,
            health: Float(health),
            last_hit: None,
            air,
            frozen: 0,
        },
    })
}

/// The dye colours in the game's order (`DyeColor`), as a cushion's tag names them.
const COLOURS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// A cushion of a template (the abandoned camps'): its UUID drawn as a placed
/// cushion's, attached to its `block_pos` (which the placing set to its template block,
/// moved), of its `color`. Not a mob: nothing is finalised. `None` for a tag without
/// its block.
fn cushion(world: u64, kind: u16, tag: &Tag, placed: Placed<'_>) -> Option<Entity> {
    let block: Vec<i32> = tag
        .get("block_pos")
        .map(Tag::list)
        .unwrap_or_default()
        .iter()
        .filter_map(|value| value.number().map(|value| value as i32))
        .collect();
    let [x, y, z] = block[..] else {
        return None;
    };
    let colour = tag
        .get("color")
        .and_then(Tag::text)
        .and_then(|name| COLOURS.iter().position(|known| *known == name))
        .map_or(0, |at| at as u8);
    let Placed {
        position,
        yaw,
        pitch,
        makings,
        ..
    } = placed;
    let key = makings.next()?.key()?;
    let mut own = Random::of_generated(world, key);
    let uuid = insecure_uuid(own.next_u64(), own.next_u64());
    Some(Entity {
        portal: Default::default(),
        key,
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Cushion {
            attached: Attached {
                block: clustine_world::BlockPos::new(x, y, z),
                direction: 0,
            },
            colour,
        },
    })
}

/// The mob of a record a test makes, decoded.
#[cfg(test)]
pub(crate) fn mob_of(entity: &Entity) -> crate::mobs::Mob {
    crate::mobs::Mob::decode(&entity.data).expect("a generated mob's record decodes")
}
