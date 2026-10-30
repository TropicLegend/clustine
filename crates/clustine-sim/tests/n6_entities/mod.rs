//! Entities set into the worlds of the second round of N6's tests from the
//! specification, each as its chunk keeps it: items, carts, boats, armour stands,
//! falling blocks and primed TNT. Written from `clustine_world::EntityData` alone; a
//! test file that uses them has `n6_scene` beside them.

// Each test file uses its own part of these.
#![allow(dead_code)]

use clustine_data::{blocks, entity_types, items};
use clustine_world::{Double, Entity, EntityData, Float, Hurt, ItemStack};
use uuid::Uuid;

// Every file that uses these has the scene beside them, whose keys these are.
use super::n6_scene::key;

/// An entity of `kind` standing at `position`, at rest and on the ground.
pub fn entity(n: u32, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        portal: Default::default(),
        key: key(n),
        kind: u16::try_from(kind).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    }
}

/// An item of `stack` lying at `position`, which nobody picks up and which merges with
/// nothing (a pick-up delay of 32767 is the game's "never").
pub fn item(n: u32, stack: ItemStack, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ITEM,
        position,
        EntityData::Item {
            stack,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    )
}

/// One dirt lying at `position`.
pub fn dirt(n: u32, position: [f64; 3]) -> Entity {
    item(n, ItemStack::new(items::DIRT, 1), position)
}

/// An item that despawns within `ticks` ticks: an item is gone at an age of 6000.
pub fn ageing(mut entity: Entity, ticks: u32) -> Entity {
    entity.lived = 6_000 - ticks;
    entity
}

/// A minecart of `kind` (a plain one, a chest cart, a hopper cart, ...) holding
/// `contents`.
pub fn cart_of(n: u32, kind: i32, position: [f64; 3], contents: Vec<(u8, ItemStack)>) -> Entity {
    entity(
        n,
        kind,
        position,
        EntityData::Minecart {
            hurt: Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents,
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    )
}

pub fn cart(n: u32, position: [f64; 3]) -> Entity {
    cart_of(n, entity_types::MINECART, position, Vec::new())
}

pub fn boat(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::OAK_BOAT,
        position,
        EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            status: [3, 3],
            contents: Vec::new(),
        },
    )
}

/// The armour stand's flag that makes it a marker (`ArmorStand.CLIENT_FLAG_MARKER`).
pub const MARKER: u8 = 16;

/// An armour stand with `flags` (0 for an ordinary one, [`MARKER`] for a marker).
pub fn stand_with(n: u32, position: [f64; 3], flags: u8) -> Entity {
    let pose = [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [-10.0, 0.0, -10.0],
        [-15.0, 0.0, 10.0],
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
    ];
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags,
            disabled_slots: 0,
            pose: Box::new(pose.map(|part: [f32; 3]| part.map(Float))),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

pub fn stand(n: u32, position: [f64; 3]) -> Entity {
    stand_with(n, position, 0)
}

/// Sand falling from `position`.
pub fn falling_sand(n: u32, position: [f64; 3]) -> Entity {
    let mut sand = entity(
        n,
        entity_types::FALLING_BLOCK,
        position,
        EntityData::FallingBlock {
            state: blocks::SAND.0,
            time: 1,
            drop_item: true,
            cancel_drop: false,
        },
    );
    sand.on_ground = false;
    sand
}

/// Primed TNT at `position` with `fuse` ticks left and no owner.
pub fn primed(n: u32, position: [f64; 3], fuse: i16) -> Entity {
    entity(
        n,
        entity_types::TNT,
        position,
        EntityData::PrimedTnt {
            fuse,
            block: blocks::TNT.0,
            power: Float(4.0),
            owner: None,
        },
    )
}

/// The contents of a container entity's record.
pub fn contents(entity: &Entity) -> Vec<(u8, ItemStack)> {
    match &entity.data {
        EntityData::Minecart { contents, .. } | EntityData::Boat { contents, .. } => {
            contents.clone()
        }
        other => panic!("not a container entity: {other:?}"),
    }
}

/// How many items of every kind a container entity holds.
pub fn held_count(entity: &Entity) -> i32 {
    contents(entity).iter().map(|(_, stack)| stack.count).sum()
}
