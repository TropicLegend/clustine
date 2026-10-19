//! Tests of TNT from the game's rules (`docs/groundwork/n6-brief.md`, section 1): each
//! way it is primed, with its fuse and owner; the primed TNT's fall and friction tick
//! by tick and its explosion; and the TNT cart. What the explosions' tests share
//! (`explosions/tests.rs`) is here too.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Double, EdgeId, Entity, EntityData, EntityId,
    EntityIds, Float, Hurt, PlayerId, Vec3,
};
use uuid::Uuid;

use super::super::{Holdings, Player, Region, RegionConfig};
use super::FUSE;
use crate::api::{
    EnderChest, Face, Inventory, Keys, Pose, RegionEvent, TickInputs, TickOutput, Ticket,
};
use crate::placement::Click;
use crate::state::RegionState;

/// The floor's top: stone at y = -61, so a test's blocks stand at -60.
pub(in crate::region) const FLOOR: i32 = -61;

/// The heights a test's blocks may stand at, below and above the floor.
const LOWEST: i32 = FLOOR - 12;
const HIGHEST: i32 = FLOOR + 24;

pub(in crate::region) fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

pub(in crate::region) fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

/// A chunk of the overworld with a stone floor, and in it what `blocks` names.
fn chunk_of(position: ChunkPos, blocks: &dyn Fn(BlockPos) -> Option<BlockState>) -> Chunk {
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("the game has the overworld");
    let mut chunk = Chunk::empty(overworld, Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, FLOOR, z, blocks::STONE);
            for y in LOWEST..=HIGHEST {
                let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                if let Some(state) = blocks(pos) {
                    chunk.set(x, y, z, state);
                }
            }
        }
    }
    chunk
}

fn config(seed: u64) -> RegionConfig {
    RegionConfig {
        spawn: Vec3::new(0.5, -60.0, 0.5),
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: -64,
        seed,
    }
}

/// A region of the world `seed` that holds the chunks `held` and has them loaded, with
/// copies of `copied`, and in both the blocks `blocks` names over the stone floor; it
/// has ticked once, so that its chunks with all their neighbours tick.
fn region_of(
    seed: u64,
    held: &[ChunkPos],
    pinned: Vec<ChunkArea>,
    copied: &[ChunkPos],
    blocks: &dyn Fn(BlockPos) -> Option<BlockState>,
) -> Region {
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| (*position, chunk_of(*position, blocks)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: held.to_vec(),
        pinned,
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let copies = copied
        .iter()
        .map(|position| (*position, chunk_of(*position, blocks)))
        .collect();
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region =
        Region::restore_loaded(config(seed), state, holdings, &tickets, chunks, copies);
    region.tick(&TickInputs::default());
    region
}

/// The 5×5 chunks about the origin's, all held and loaded, the middle 3×3 ticking.
pub(in crate::region) fn world(
    seed: u64,
    blocks: impl Fn(BlockPos) -> Option<BlockState>,
) -> Region {
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region_of(seed, &held, Vec::new(), &[], &blocks)
}

/// The western side of a line between the blocks x = 15 and x = 16: a region pinned to
/// the chunks of x up to 0 that holds those from -2 and copies the column x = 1.
pub(in crate::region) fn west_of_a_line(
    seed: u64,
    blocks: impl Fn(BlockPos) -> Option<BlockState>,
) -> Region {
    let held: Vec<ChunkPos> = (-2..=0)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let copied: Vec<ChunkPos> = (-2..=2).map(|z| ChunkPos::new(1, z)).collect();
    let west = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    region_of(seed, &held, vec![west], &copied, &blocks)
}

/// Only the floor.
pub(in crate::region) fn nothing(_: BlockPos) -> Option<BlockState> {
    None
}

/// The player `number` standing at `feet`, flying or not, put into the region as a
/// creative player is.
pub(in crate::region) fn add_player(
    region: &mut Region,
    number: u128,
    feet: [f64; 3],
    flying: bool,
) -> (PlayerId, EntityId) {
    let id = PlayerId(Uuid::from_u128(number));
    let entity_id = EntityId(1_000_000 + u64::try_from(number).expect("a small number"));
    let player = Player {
        entity_id,
        name: format!("player{number}"),
        pose: Pose::at(Vec3::new(feet[0], feet[1], feet[2])),
        moved_from: None,
        handled_sequence: None,
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: EnderChest::default(),
        last_input: 0,
        handled: None,
        edge: EdgeId(1),
        hops: 0,
        flying,
        attempt: None,
        riding: None,
        keys: Keys::default(),
        kept: Vec::new(),
        menu: None,
    };
    region.players.insert(id, player);
    (id, entity_id)
}

/// A record of the kind `kind` at `position`, still, under the region's next key.
pub(in crate::region) fn record(
    region: &mut Region,
    kind: i32,
    position: [f64; 3],
    data: EntityData,
) -> Entity {
    let chunk = ChunkPos::containing(position[0], position[2]);
    let key = region.take_entity_key(chunk).expect("room and a key");
    Entity {
        key,
        kind: u16::try_from(kind).expect("a type's id"),
        uuid: Uuid::from_u128(u128::from(key.0) + 7),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        age: 0,
        data,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// Puts `entity` into the region, as `addFreshEntity` does.
pub(in crate::region) fn add(region: &mut Region, entity: Entity) -> EntityId {
    let key = entity.key;
    region.add_fresh_entity(entity, &mut TickOutput::default());
    key
}

/// A primed TNT of `fuse` at `position`, still, primed by `owner`.
pub(in crate::region) fn primed(
    region: &mut Region,
    position: [f64; 3],
    fuse: i16,
    owner: Option<Uuid>,
) -> EntityId {
    primed_moving(region, position, [0.0; 3], fuse, owner)
}

/// A primed TNT of `fuse` at `position` moving by `motion`, primed by `owner`.
pub(in crate::region) fn primed_moving(
    region: &mut Region,
    position: [f64; 3],
    motion: [f64; 3],
    fuse: i16,
    owner: Option<Uuid>,
) -> EntityId {
    let data = EntityData::PrimedTnt {
        fuse,
        block: blocks::TNT.0,
        power: Float(4.0),
        owner,
    };
    let mut entity = record(region, entity_types::TNT, position, data);
    entity.motion = motion;
    add(region, entity)
}

/// A minecart of the kind `kind` at `position`, still and unhurt.
pub(in crate::region) fn cart(region: &mut Region, kind: i32, position: [f64; 3]) -> EntityId {
    let data = EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [Double(0.0); 2],
        fuse: -1,
        enabled: true,
    };
    let entity = record(region, kind, position, data);
    add(region, entity)
}

/// The records of every entity of the loaded chunks.
pub(in crate::region) fn records(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .collect()
}

/// The primed TNT of the loaded chunks, with their fuses and owners.
pub(in crate::region) fn primed_tnt(region: &Region) -> Vec<(Entity, i16, Option<Uuid>)> {
    records(region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::PrimedTnt { fuse, owner, .. } => Some((entity.clone(), fuse, owner)),
            _ => None,
        })
        .collect()
}

pub(in crate::region) fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

fn heard(output: &TickOutput, sound: i32) -> Vec<(Vec3, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: heard,
                position,
                except,
                ..
            } if *heard == sound => Some((*position, *except)),
            _ => None,
        })
        .collect()
}

fn click_on(pos: BlockPos, face: Face, held: i32, sneaking: bool) -> Click {
    Click {
        position: pos,
        face,
        cursor: [0.5; 3],
        sneaking,
        held: Some(held),
        feet: Vec3::new(f64::from(pos.x) + 0.5, -60.0, f64::from(pos.z) - 2.5),
        yaw: 0.0,
        pitch: 0.0,
    }
}

/// What every priming of a TNT block makes (`TntBlock.prime`, `PrimedTnt`'s
/// constructor): primed TNT at the middle of the block's floor, 0.2 a tick up and 0.02
/// sideways, with a fuse of 80 and the sound of its priming.
fn assert_primed_at(region: &Region, output: &TickOutput, pos: BlockPos, owner: Option<Uuid>) {
    let [(tnt, fuse, primed_by)] = &primed_tnt(region)[..] else {
        panic!("one primed TNT: {:?}", primed_tnt(region));
    };
    assert_eq!(*fuse, FUSE);
    assert_eq!(*primed_by, owner);
    let middle = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    assert_eq!(tnt.position, middle);
    // `0.2f` widened, and 0.02 along the drawn direction.
    assert_eq!(tnt.motion[1], f64::from(0.2_f32));
    let sideways = (tnt.motion[0] * tnt.motion[0] + tnt.motion[2] * tnt.motion[2]).sqrt();
    assert!((sideways - 0.02).abs() < 1.0e-12, "{:?}", tnt.motion);
    assert_eq!(region.block(pos), Some(blocks::AIR));
    let primings = heard(output, sounds::ENTITY_TNT_PRIMED);
    assert_eq!(
        primings,
        [(Vec3::new(middle[0], middle[1], middle[2]), None)]
    );
}

#[test]
fn tnt_placed_where_power_reaches_it_is_primed_with_no_owner() {
    let mut region = world(1, |pos| {
        (pos == at(2, -60, 2)).then_some(blocks::REDSTONE_BLOCK)
    });
    let mut output = TickOutput::default();
    region.set_block(at(3, -60, 2), blocks::TNT, &mut output);
    assert_primed_at(&region, &output, at(3, -60, 2), None);
}

#[test]
fn tnt_that_power_comes_to_is_primed_with_no_owner() {
    let mut region = world(2, |pos| (pos == at(3, -60, 2)).then_some(blocks::TNT));
    let mut output = TickOutput::default();
    region.set_block(at(3, -59, 2), blocks::DIRT, &mut output);
    assert!(
        primed_tnt(&region).is_empty(),
        "a block that gives no power"
    );
    region.set_block(at(2, -60, 2), blocks::REDSTONE_BLOCK, &mut output);
    assert_primed_at(&region, &output, at(3, -60, 2), None);
}

#[test]
fn a_flint_and_steel_or_a_fire_charge_lights_tnt_for_its_user_and_a_sneaking_user_passes_it_by() {
    for held in [items::FLINT_AND_STEEL, items::FIRE_CHARGE] {
        let pos = at(3, -60, 2);
        let mut region = world(3, |at| (at == pos).then_some(blocks::TNT));
        let player = PlayerId(Uuid::from_u128(77));
        let mut output = TickOutput::default();
        let sneaking = click_on(pos, Face::North, held, true);
        assert!(!region.light_tnt(&sneaking, player, &mut output));
        assert_eq!(region.block(pos), Some(blocks::TNT));
        let click = click_on(pos, Face::North, held, false);
        assert!(region.entity_item_used_on(&click, player, &mut output));
        assert_primed_at(&region, &output, pos, Some(player.0));
    }
}

#[test]
fn a_flint_and_steel_sets_fire_beside_the_face_it_is_used_on_heard_by_all_but_its_user() {
    let mut region = world(4, nothing);
    let player = PlayerId(Uuid::from_u128(78));
    let mut output = TickOutput::default();
    let click = click_on(at(5, FLOOR, 5), Face::Top, items::FLINT_AND_STEEL, false);
    assert!(region.entity_item_used_on(&click, player, &mut output));
    assert_eq!(region.block(at(5, -60, 5)), Some(blocks::FIRE));
    let [(where_heard, except)] = heard(&output, sounds::ITEM_FLINTANDSTEEL_USE)[..] else {
        panic!("one sound");
    };
    assert_eq!(where_heard, Vec3::new(5.5, -59.5, 5.5));
    assert_eq!(except, Some(player));
    // Fire does not go where something is, nor where it cannot stay.
    let mut output = TickOutput::default();
    let into_fire = click_on(at(5, FLOOR, 5), Face::Top, items::FLINT_AND_STEEL, false);
    assert!(!region.fire_item_used_on(&into_fire, player, &mut output));
    let under = click_on(at(6, FLOOR, 6), Face::Bottom, items::FLINT_AND_STEEL, false);
    assert!(!region.fire_item_used_on(&under, player, &mut output));
}

#[test]
fn a_flint_and_steel_lights_a_candle_and_no_fire_beside_it() {
    let candle = state("minecraft:candle[candles=1,lit=false,waterlogged=false]");
    let pos = at(4, -60, 4);
    let mut region = world(5, |at| (at == pos).then_some(candle));
    let player = PlayerId(Uuid::from_u128(79));
    let mut output = TickOutput::default();
    let click = click_on(pos, Face::Top, items::FLINT_AND_STEEL, false);
    assert!(region.entity_item_used_on(&click, player, &mut output));
    let lit = state("minecraft:candle[candles=1,lit=true,waterlogged=false]");
    assert_eq!(region.block(pos), Some(lit));
    assert_eq!(region.block(at(4, -59, 4)), Some(blocks::AIR));
}

#[test]
fn fire_that_burns_tnt_primes_it_with_no_owner_and_the_full_fuse() {
    let tnt = at(4, -60, 4);
    let mut burnt = 0;
    for seed in 0..4 {
        let mut region = world(seed, |pos| (pos == tnt).then_some(blocks::TNT));
        region.set_block(at(5, -60, 4), blocks::FIRE, &mut TickOutput::default());
        for _ in 0..3_000 {
            let output = idle(&mut region);
            if region.block(tnt) == Some(blocks::TNT) {
                continue;
            }
            // Burnt in the block ticks, it ticks once in the entities' phase of the same
            // tick, as the game's entity added in its block ticks does.
            let [(_, fuse, owner)] = &primed_tnt(&region)[..] else {
                panic!("seed {seed}: one primed TNT");
            };
            assert_eq!((*fuse, *owner), (FUSE - 1, None), "seed {seed}");
            assert_eq!(heard(&output, sounds::ENTITY_TNT_PRIMED).len(), 1);
            burnt += 1;
            break;
        }
    }
    assert!(burnt > 0, "fire burns TNT beside it");
}

#[test]
fn an_explosion_primes_the_tnt_it_reaches_with_a_short_fuse_of_ten_to_twenty_nine_by_its_owner() {
    let owner = Some(Uuid::from_u128(5));
    let mut fuses = std::collections::BTreeSet::new();
    for seed in 0..60 {
        let block = at(6, -60, 4);
        let mut region = world(seed, |pos| (pos == block).then_some(blocks::TNT));
        let first = primed(&mut region, [4.5, -60.0, 4.5], 1, owner);
        idle(&mut region);
        assert!(
            region.find_entity(first).is_none(),
            "seed {seed}: it went off"
        );
        assert_eq!(region.block(block), Some(blocks::AIR));
        // Made in the entities' phase, it ticks from the next tick on.
        let [(tnt, fuse, primed_by)] = &primed_tnt(&region)[..] else {
            panic!(
                "seed {seed}: the TNT block primed: {:?}",
                primed_tnt(&region)
            );
        };
        assert!((10..=29).contains(fuse), "seed {seed}: fuse {fuse}");
        assert_eq!(*primed_by, owner);
        assert_eq!(tnt.position, [6.5, -60.0, 4.5]);
        fuses.insert(*fuse);
    }
    assert!(fuses.len() > 8, "the fuse is drawn: {fuses:?}");
}

#[test]
fn primed_tnt_falls_and_slows_as_the_game_moves_it_tick_by_tick_and_goes_off_at_its_fuse() {
    let mut region = world(6, nothing);
    let start = [8.5, -50.0, 8.5];
    let mut motion = [0.01, f64::from(0.2_f32), 0.0];
    let key = primed_moving(&mut region, start, motion, FUSE, None);
    // `tick()` in the air: gravity, the move, the drag of `0.98f`.
    let drag = f64::from(0.98_f32);
    let mut position = start;
    for tick in 1..=6 {
        idle(&mut region);
        motion[1] -= 0.04;
        position = [
            position[0] + motion[0],
            position[1] + motion[1],
            position[2] + motion[2],
        ];
        motion = motion.map(|component| component * drag);
        let (_, record) = region.find_entity(key).expect("still burning");
        assert_eq!(
            record.position.map(f64::to_bits),
            position.map(f64::to_bits),
            "tick {tick}"
        );
        assert_eq!(
            record.motion.map(f64::to_bits),
            motion.map(f64::to_bits),
            "tick {tick}"
        );
        let EntityData::PrimedTnt { fuse, .. } = record.data else {
            panic!("primed TNT");
        };
        assert_eq!(i32::from(fuse), i32::from(FUSE) - tick);
    }
    // On the ground: the drag, and then 0.7 along x and z, -0.5 up.
    let mut ticks = 6;
    loop {
        idle(&mut region);
        ticks += 1;
        let (_, record) = region.find_entity(key).expect("still burning");
        if record.on_ground {
            break;
        }
    }
    let before = region.find_entity(key).expect("there").1.motion;
    idle(&mut region);
    ticks += 1;
    let (_, record) = region.find_entity(key).expect("still burning");
    assert!(record.on_ground);
    assert_eq!(
        record.motion[0].to_bits(),
        (before[0] * drag * 0.7).to_bits()
    );
    assert_eq!(record.position[1], f64::from(FLOOR + 1));
    // It goes off on its 80th tick, from a little above its feet: `getY(0.0625)`.
    let feet = loop {
        let feet = region.find_entity(key).expect("still burning").1.position;
        let output = idle(&mut region);
        ticks += 1;
        if let Some(centre) = output.events.iter().find_map(|event| match event {
            RegionEvent::Explosion { centre, radius, .. } => Some((*centre, *radius)),
            _ => None,
        }) {
            break (feet, centre, output);
        }
        assert!(ticks < 80, "it went off at {ticks}");
    };
    let (feet, (centre, radius), output) = feet;
    assert_eq!(ticks, 80);
    assert_eq!(radius, 4.0);
    assert!(region.find_entity(key).is_none());
    // The last tick moved it on the ground by the motion it had, before it went off.
    assert_eq!(
        centre[1],
        f64::from(FLOOR + 1) + f64::from(0.98_f32) * 0.0625
    );
    assert!((centre[0] - feet[0]).abs() < 0.01);
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
    );
}

/// A powered activator rail at `rail`, held so by a block of redstone beside it.
fn powered_rail(rail: BlockPos) -> impl Fn(BlockPos) -> Option<BlockState> {
    let powered =
        state("minecraft:activator_rail[powered=true,shape=north_south,waterlogged=false]");
    move |pos| {
        if pos == rail {
            Some(powered)
        } else if pos == at(rail.x + 1, rail.y, rail.z) {
            Some(blocks::REDSTONE_BLOCK)
        } else {
            None
        }
    }
}

fn cart_fuse(region: &Region, key: EntityId) -> Option<i32> {
    region.tnt_cart_fuse(key)
}

#[test]
fn a_tnt_cart_on_a_powered_activator_rail_is_primed_and_goes_off_eighty_ticks_later() {
    let rail = at(4, -60, 4);
    let mut region = world(7, powered_rail(rail));
    let key = cart(
        &mut region,
        entity_types::TNT_MINECART,
        [4.5, -59.9375, 4.5],
    );
    let output = idle(&mut region);
    // `activateMinecart` primes it at 80, and its own tick takes one off that tick.
    assert_eq!(cart_fuse(&region, key), Some(FUSE_OF_A_CART - 1));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityEvent { entity, event: 70, .. } if *entity == key
    )));
    assert_eq!(heard(&output, sounds::ENTITY_TNT_PRIMED).len(), 1);
    // It burns down to nought, and goes off in the tick after: 80 ticks after the
    // priming's.
    let mut ticks = 0;
    loop {
        let output = idle(&mut region);
        ticks += 1;
        let exploded = output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::Explosion { .. }));
        if exploded {
            break;
        }
        // Not primed again while it burns.
        assert_eq!(cart_fuse(&region, key), Some(FUSE_OF_A_CART - 1 - ticks));
        assert!(ticks < 100);
    }
    assert_eq!(ticks, 80);
    assert!(region.find_entity(key).is_none());
    // Its rays pass the rail and the block under it: both are still there.
    assert_eq!(
        region.block(rail).map(|state| state.block_id()),
        Some(state("minecraft:activator_rail").block_id())
    );
    assert_eq!(region.block(at(4, FLOOR, 4)), Some(blocks::STONE));
}

/// `primeFuse`'s fuse.
const FUSE_OF_A_CART: i32 = 80;

#[test]
fn a_tnt_cart_an_explosion_reaches_is_primed_with_a_fuse_of_two_draws_and_not_broken() {
    let mut fuses = std::collections::BTreeSet::new();
    for seed in 0..40 {
        let mut region = world(seed, nothing);
        let key = cart(&mut region, entity_types::TNT_MINECART, [6.5, -60.0, 4.5]);
        primed(&mut region, [4.5, -60.0, 4.5], 1, None);
        let output = idle(&mut region);
        let fuse = cart_fuse(&region, key).expect("the cart is not broken");
        assert!((0..=38).contains(&fuse), "seed {seed}: fuse {fuse}");
        assert!(output.events.iter().any(|event| matches!(
            event,
            RegionEvent::EntityEvent { entity, event: 70, .. } if *entity == key
        )));
        fuses.insert(fuse);
    }
    assert!(fuses.len() > 8, "{fuses:?}");
}

#[test]
fn a_tnt_cart_that_falls_three_blocks_or_more_goes_off_as_it_lands() {
    for (height, goes_off) in [(2.0, false), (5.0, true)] {
        let mut region = world(8, nothing);
        let key = cart(
            &mut region,
            entity_types::TNT_MINECART,
            [4.5, -60.0 + height, 4.5],
        );
        let mut exploded = false;
        for _ in 0..40 {
            let output = idle(&mut region);
            exploded |= output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::Explosion { .. }));
        }
        assert_eq!(exploded, goes_off, "a fall of {height}");
        assert_eq!(region.find_entity(key).is_some(), !goes_off);
    }
}

#[test]
fn a_creative_hit_takes_a_tnt_cart_away_without_priming_it() {
    let mut region = world(9, nothing);
    let key = cart(&mut region, entity_types::TNT_MINECART, [4.5, -60.0, 4.5]);
    // `VehicleEntity.hurtServer`: a hit is no source that sets TNT alight, so a
    // creative one discards the cart.
    assert!(region.hurt_vehicle(key, 1.0, true, &mut TickOutput::default()));
    assert!(region.find_entity(key).is_none());
    assert!(primed_tnt(&region).is_empty());
}
