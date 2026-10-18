//! Tests of the item a player throws (step N2c, `docs/groundwork/n2-plan.md`, sections
//! 7 and 11): where the game puts it and how it moves off, what Q, ctrl+Q and the
//! creative inventory take, what is refused and put back, the item's place in its
//! chunk through restores, merges and splits, and its changes of holder.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on a stone floor at y = -61, with
//! the chunks around chunk (0, 0) loaded so that it ticks, and holds the stone of the
//! starting hotbar in slot 0.

use clustine_world::{Entity, EntityChange, EntityData, MAX_CHUNK_ENTITIES};

use super::*;
use crate::region::random_ticks::Random;

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The key of player 1's entity, and of the first entity made after them.
const PLAYER_KEY: EntityId = EntityId(1);
const FIRST_ITEM: EntityId = EntityId(2);

/// The game's sine table at an angle in radians, worked out here from how the game
/// fills it (the sine of each 65,536th of a turn) and looks it up, apart from the code
/// under test.
fn table_sin(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453;
    let index = (f64::from(radians) * steps) as i64 & 0xFFFF;
    libm::sin(index as f64 / steps) as f32
}

/// Its cosine: a quarter turn on.
fn table_cos(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453;
    let index = (f64::from(radians) * steps + 16_384.0) as i64 & 0xFFFF;
    libm::sin(index as f64 / steps) as f32
}

/// The motion of `LivingEntity.createItemStackToDrop` for a thrower turned `yaw` and
/// `pitch`, with the four floats drawn, written out step by step as the game's
/// bytecode has them.
fn game_s_motion(yaw: f32, pitch: f32, draws: [f32; 4]) -> [f64; 3] {
    let deg = 0.017_453_292_f32;
    let (sin_pitch, cos_pitch) = (table_sin(pitch * deg), table_cos(pitch * deg));
    let (sin_yaw, cos_yaw) = (table_sin(yaw * deg), table_cos(yaw * deg));
    let a = draws[0] * 6.283_185_5_f32;
    let d = 0.02_f32 * draws[1];
    let x = ((-sin_yaw) * cos_pitch) * 0.3_f32;
    let y = (((-sin_pitch) * 0.3_f32) + 0.1_f32) + ((draws[2] - draws[3]) * 0.1_f32);
    let z = (cos_yaw * cos_pitch) * 0.3_f32;
    [
        f64::from(x) + libm::cos(f64::from(a)) * f64::from(d),
        f64::from(y),
        f64::from(z) + libm::sin(f64::from(a)) * f64::from(d),
    ]
}

/// The four floats the thrower `key` draws first in the tick `tick` of a world of seed
/// 0, which the tests' regions are.
fn draws_of(key: EntityId, tick: u64, skipped: usize) -> [f32; 4] {
    let mut random = Random::of_entity(0, key, tick);
    for _ in 0..skipped {
        random.next_u64();
    }
    std::array::from_fn(|_| random.next_float())
}

/// Player 1 turns to `yaw` and `pitch` where they stand.
fn turn(yaw: f32, pitch: f32) -> Input {
    numbered(
        1,
        PlayerInput::Move {
            position: None,
            rotation: Some((yaw, pitch)),
            on_ground: true,
        },
    )
}

/// Q, or ctrl+Q if `all`.
fn throw_held(all: bool) -> Input {
    numbered(
        1,
        PlayerInput::Drop {
            all,
            crouching: false,
        },
    )
}

/// The entities of `chunk` in `region`.
fn entities_in(region: &Region, chunk: ChunkPos) -> Vec<Entity> {
    region.chunk(chunk).unwrap().entities().to_vec()
}

/// The item one whole entity change of `output` makes: the throw's, which the item's
/// own first tick in the same tick may follow with what its move set.
fn the_whole(output: &TickOutput) -> Entity {
    let wholes: Vec<&Entity> = output
        .entity_changes
        .iter()
        .filter_map(|(chunk, change)| match change {
            EntityChange::Whole(entity) => {
                assert_eq!(*chunk, entity.chunk());
                Some(entity)
            }
            // What the items thrown before did in the tick: moved, or merged.
            EntityChange::Set(..) | EntityChange::Gone(_) => None,
        })
        .collect();
    let [entity] = wholes[..] else {
        panic!("one whole entity: {:?}", output.entity_changes);
    };
    entity.clone()
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// An item of dirt lying still on the floor at `x` and `z`, out of the player's reach.
fn resting(key: EntityId, x: f64, z: f64) -> Entity {
    Entity {
        position: [x, -60.0, z],
        motion: [0.0; 3],
        on_ground: true,
        age: 0,
        data: EntityData::Item {
            stack: DIRT,
            pick_up_delay: 40,
            thrower: None,
            health: 5,
        },
        ..item_at(key, [x, -60.0, z], 0)
    }
}

fn stack_of(entity: &Entity) -> ItemStack {
    let EntityData::Item { stack, .. } = &entity.data else {
        panic!("{entity:?} is not an item");
    };
    *stack
}

fn held(region: &Region, slot: usize) -> Option<ItemStack> {
    region.state().players[&player(1)].inventory.0[slot]
}

/// A region as these tests begin with, the player looking along `yaw` and `pitch`.
fn turned(yaw: f32, pitch: f32) -> Region {
    let mut region = on_ticking_floor_with(&[]);
    region.tick(&moves(vec![turn(yaw, pitch)]));
    region
}

#[test]
fn a_thrown_item_is_where_the_game_puts_it_with_the_game_s_motion_for_its_draws() {
    let (yaw, pitch) = (30.0, -20.0);
    let mut region = turned(yaw, pitch);
    let output = region.tick(&moves(vec![throw_held(false)]));
    let item = the_whole(&output);

    // At the player's x and z, 0.3 below eyes 1.62 above the feet, the game's floats.
    let y = -60.0 + f64::from(1.62_f32) - 0.300_000_011_920_928_96;
    assert_eq!(
        item.position.map(f64::to_bits),
        [0.5, y, 0.5].map(f64::to_bits)
    );
    assert_eq!(item.chunk(), ORIGIN);
    // The motion of the thrower's first four draws of the tick.
    let expected = game_s_motion(yaw, pitch, draws_of(PLAYER_KEY, output.tick, 0));
    assert_eq!(item.motion.map(f64::to_bits), expected.map(f64::to_bits));
    // Ahead along the yaw (west of south, as 30 degrees is), up, and about 0.3 fast.
    assert!(item.motion[0] < 0.0 && item.motion[2] > 0.0 && item.motion[1] > 0.0);
    let speed = item.motion.iter().map(|m| m * m).sum::<f64>().sqrt();
    assert!((0.25..0.4).contains(&speed), "{speed}");

    // Its own draws at its birth: two for its UUID, of version 4, then one for its bob
    // and one for its yaw.
    let mut own = Random::of_entity(0, FIRST_ITEM, output.tick);
    let (most, least) = (own.next_u64(), own.next_u64());
    assert_eq!(
        item.uuid.as_u64_pair(),
        (most & !0xF000 | 0x4000, least & (u64::MAX >> 2) | 1 << 63)
    );
    assert_eq!(item.uuid.get_version_num(), 4);
    own.next_float();
    assert_eq!(item.yaw.to_bits(), (own.next_float() * 360.0).to_bits());

    assert_eq!(item.key, FIRST_ITEM);
    assert_eq!(i32::from(item.kind), clustine_data::entity_types::ITEM);
    assert_eq!(
        (item.pitch, item.on_ground, item.hops, item.age),
        (0.0, false, 0, 0)
    );
    assert_eq!(
        item.data,
        EntityData::Item {
            stack: STONE,
            pick_up_delay: 40,
            thrower: Some(player(1).0),
            health: 5,
        }
    );
    // The chunk has it, ticked once since: gravity, then the move through the air, then
    // the air's drag (`ItemEntity.tick`); the region's next key is the one after it,
    // and the thrower's arm swings for others before the item appears.
    let drag = f64::from(0.98_f32);
    let fallen = [item.motion[0], item.motion[1] + -0.04, item.motion[2]];
    let ticked = Entity {
        age: 1,
        position: [
            item.position[0] + fallen[0],
            item.position[1] + fallen[1],
            item.position[2] + fallen[2],
        ],
        motion: [fallen[0] * drag, fallen[1] * drag, fallen[2] * drag],
        data: EntityData::Item {
            stack: STONE,
            pick_up_delay: 39,
            thrower: Some(player(1).0),
            health: 5,
        },
        ..item.clone()
    };
    assert_eq!(entities_in(&region, ORIGIN), std::slice::from_ref(&ticked));
    assert_eq!(region.state().next_entity_id, EntityId(3));
    assert_eq!(output.delta.next_entity_id, Some(EntityId(3)));
    let shown: Vec<&RegionEvent> = output
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::ArmSwung { .. } | RegionEvent::EntitySpawned(_)
            )
        })
        .collect();
    let [
        RegionEvent::ArmSwung { entity, chunk },
        RegionEvent::EntitySpawned(state),
    ] = &shown[..]
    else {
        panic!("a swing and a spawn: {shown:?}");
    };
    assert_eq!((*entity, *chunk), (PLAYER_KEY, ORIGIN));
    assert_eq!(state.entity, FIRST_ITEM);
    assert_eq!(state.pose.position, Vec3::new(0.5, y, 0.5));
    assert_eq!(state.pose.yaw, item.yaw);
    assert_eq!(
        state.kind,
        EntityKind::Other {
            kind: item.kind,
            uuid: item.uuid,
            motion: item.motion,
            hops: 0,
            data: item.data.clone(),
        }
    );
    // And it is among the region's entities, after the players, where its first tick
    // took it.
    let position = region.entity(FIRST_ITEM).unwrap().pose.position;
    assert_eq!(
        [position.x, position.y, position.z].map(f64::to_bits),
        ticked.position.map(f64::to_bits)
    );
    let all: Vec<EntityId> = region.entities().map(|state| state.entity).collect();
    assert_eq!(all, [PLAYER_KEY, FIRST_ITEM]);
}

#[test]
fn an_item_thrown_crouching_leaves_from_the_lower_eyes_and_not_while_flying() {
    let crouching = || {
        numbered(
            1,
            PlayerInput::Drop {
                all: false,
                crouching: true,
            },
        )
    };
    let mut region = on_ticking_floor_with(&[]);
    let three = set_slot(1, 0, Some(ItemStack { count: 3, ..STONE }));
    let output = region.tick(&moves(vec![three, crouching()]));
    let y = -60.0 + f64::from(1.27_f32) - 0.300_000_011_920_928_96;
    assert_eq!(the_whole(&output).position[1].to_bits(), y.to_bits());
    let flying = numbered(1, PlayerInput::SetFlying { flying: true });
    let output = region.tick(&moves(vec![flying, crouching()]));
    let y = -60.0 + f64::from(1.62_f32) - 0.300_000_011_920_928_96;
    assert_eq!(the_whole(&output).position[1].to_bits(), y.to_bits());
}

#[test]
fn q_takes_one_ctrl_q_the_stack_creative_lessens_it_and_an_empty_hand_throws_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let ten = ItemStack { count: 10, ..STONE };
    region.tick(&moves(vec![set_slot(1, 0, Some(ten))]));

    // Q: one of the ten, which the player no longer holds (creative players too, as
    // `Inventory.removeFromSelected` takes it), and the client is told nothing.
    let output = region.tick(&moves(vec![throw_held(false)]));
    assert_eq!(
        stack_of(&the_whole(&output)),
        ItemStack { count: 1, ..STONE }
    );
    assert_eq!(held(&region, 0), Some(ItemStack { count: 9, ..STONE }));
    assert!(
        output.player_events.is_empty(),
        "{:?}",
        output.player_events
    );
    // Its change of the player goes to the store with the tick.
    assert!(output.delta.players.iter().any(|(id, _)| *id == player(1)));

    // Ctrl+Q: the nine left.
    let output = region.tick(&moves(vec![throw_held(true)]));
    assert_eq!(
        stack_of(&the_whole(&output)),
        ItemStack { count: 9, ..STONE }
    );
    assert_eq!(held(&region, 0), None);

    // An empty hand throws nothing, and nobody is told anything.
    let output = region.tick(&moves(vec![throw_held(false), throw_held(true)]));
    assert!(
        !output
            .entity_changes
            .iter()
            .any(|(_, change)| matches!(change, EntityChange::Whole(_))),
        "{:?}",
        output.entity_changes
    );
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::EntitySpawned(_)))
    );
    assert!(output.player_events.is_empty());

    // Out of the creative inventory: the stack, and the inventory as it was.
    let five = ItemStack { count: 5, ..DIRT };
    let before = region.state().players[&player(1)].inventory.clone();
    let output = region.tick(&moves(vec![numbered(
        1,
        PlayerInput::Throw { stack: five },
    )]));
    assert_eq!(stack_of(&the_whole(&output)), five);
    assert_eq!(region.state().players[&player(1)].inventory, before);
    // Of an item that does not exist, or of none, nothing.
    for stack in [
        ItemStack {
            item: i32::MAX,
            count: 1,
        },
        ItemStack { item: -1, count: 1 },
        ItemStack { count: 0, ..DIRT },
    ] {
        let output = region.tick(&moves(vec![numbered(1, PlayerInput::Throw { stack })]));
        assert!(
            !output
                .entity_changes
                .iter()
                .any(|(_, change)| matches!(change, EntityChange::Whole(_))),
            "{stack:?}"
        );
    }

    // Three thrown so far, under the keys after the player's, ascending in the chunk;
    // the two of stone fell together and merged, as the game's do.
    assert_eq!(region.state().next_entity_id, EntityId(5));
    let lying: Vec<(u64, ItemStack)> = entities_in(&region, ORIGIN)
        .iter()
        .map(|entity| (entity.key.0, stack_of(entity)))
        .collect();
    let stone: i32 = lying
        .iter()
        .filter(|(_, stack)| stack.item == STONE.item)
        .map(|(_, stack)| stack.count)
        .sum();
    assert_eq!(stone, 10);
    assert!(lying.contains(&(4, five)), "{lying:?}");
    assert!(lying.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn two_throws_in_one_tick_draw_on_from_the_thrower_s_stream() {
    let mut region = turned(0.0, 0.0);
    region.tick(&moves(vec![set_slot(
        1,
        0,
        Some(ItemStack { count: 2, ..STONE }),
    )]));
    let output = region.tick(&moves(vec![throw_held(false), throw_held(false)]));
    let motions: Vec<[f64; 3]> = output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity) => Some(entity.motion),
            _ => None,
        })
        .collect();
    let first = game_s_motion(0.0, 0.0, draws_of(PLAYER_KEY, output.tick, 0));
    let second = game_s_motion(0.0, 0.0, draws_of(PLAYER_KEY, output.tick, 4));
    assert_eq!(motions, [first, second]);
    assert_ne!(first, second);
}

/// The client took the item out of the slot when it threw: where the region does not
/// throw it, the client is told the slot as it still is.
#[test]
fn a_refused_throw_puts_the_slot_back() {
    let put_back = (
        player(1),
        PlayerEvent::InventorySlot {
            slot: 0,
            stack: Some(STONE),
        },
    );
    let refused = |region: &mut Region| {
        let output = region.tick(&moves(vec![throw_held(false)]));
        assert!(output.entity_changes.is_empty());
        assert_eq!(output.player_events, std::slice::from_ref(&put_back));
        assert_eq!(held(region, 0), Some(STONE));
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::ArmSwung { .. }))
        );
    };

    // No key left in the region's block.
    let mut used_up = on_ticking_floor_with(&[]);
    used_up.next_entity_id = used_up.entity_ids.end;
    refused(&mut used_up);

    // A full chunk.
    let mut full = on_ticking_floor_with(&[]);
    let crowd: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u64)
        .map(|n| item_at(EntityId((1 << 40) + n), [8.5, -60.0, 8.5], 0))
        .collect();
    full.chunks.get_mut(&ORIGIN).unwrap().set_entities(crowd);
    refused(&mut full);

    // Out of the creative inventory nothing was taken, and nothing is put back.
    let output = full.tick(&moves(vec![numbered(
        1,
        PlayerInput::Throw { stack: DIRT },
    )]));
    assert!(output.entity_changes.is_empty() && output.player_events.is_empty());
}

/// Step N3d (the plan, section 5, G2; the finding of N2's tests, which refused it and
/// put the slot back): a throw from a chunk the region has loaded but does not tick, as
/// a player's chunk is right after they crossed a line, goes as an arrival into that
/// chunk, with no `hops`, and appears where it is taken.
#[test]
fn a_throw_from_a_chunk_that_does_not_tick_is_sent_as_an_arrival() {
    let mut lonely = on_floor(&[1]);
    assert!(!lonely.ticks_chunk(ORIGIN) && lonely.chunks.contains_key(&ORIGIN));
    let output = lonely.tick(&moves(vec![throw_held(false)]));
    let [(chunk, thrown)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!((*chunk, thrown.key, thrown.hops), (ORIGIN, FIRST_ITEM, 0));
    assert!(output.entity_changes.is_empty());
    assert!(
        output.player_events.is_empty(),
        "{:?}",
        output.player_events
    );
    assert_eq!(held(&lonely, 0), None);
    assert!(entities_in(&lonely, ORIGIN).is_empty());
}

/// A resting item of `key` at `position` with `hops` changes of holder behind it.
fn item_at(key: EntityId, position: [f64; 3], hops: u32) -> Entity {
    Entity {
        key,
        kind: clustine_data::entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0, 0.1, 0.3],
        yaw: 12.0,
        pitch: 0.0,
        on_ground: false,
        hops,
        age: 7,
        data: EntityData::Item {
            stack: DIRT,
            pick_up_delay: 33,
            thrower: None,
            health: 5,
        },
        fall_distance: 0.0,
        fire_ticks: -1,
    }
}

fn counters(entity: &Entity) -> (u32, u16) {
    let EntityData::Item { pick_up_delay, .. } = &entity.data else {
        panic!("{entity:?} is not an item");
    };
    (entity.age, *pick_up_delay)
}

#[test]
fn an_item_at_rest_counts_its_age_while_its_chunk_ticks_and_commits_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    let key = EntityId(1 << 40);
    lay(&mut region, resting(key, 8.5, 8.5));
    for tick in 1..=45 {
        let output = idle(&mut region);
        // Its motion runs down by gravity and is cut back to nothing at each fourth
        // tick's move, and its place does not change: nothing is committed, and its
        // chunk is named among those whose entities aged.
        assert!(output.entity_changes.is_empty(), "tick {tick}");
        assert_eq!(output.entities_ageing, [ORIGIN], "tick {tick}");
        let expected = (tick, 40_u16.saturating_sub(tick as u16));
        let lying = &entities_in(&region, ORIGIN)[0];
        assert_eq!(counters(lying), expected);
        assert_eq!(lying.position, [8.5, -60.0, 8.5]);
        assert!(lying.on_ground);
    }
    // Where its chunk stops ticking, they stand still; the chunk still has it.
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .filter(|position| *position != ORIGIN)
        .collect();
    region.tick(&tickets(vec![], around));
    for _ in 0..5 {
        idle(&mut region);
    }
    assert_eq!(counters(&entities_in(&region, ORIGIN)[0]), (45, 0));
}

/// A region's state, its chunks as saved and its tickets.
type Saved = (RegionState, Vec<(ChunkPos, Chunk)>, Vec<(ChunkPos, Ticket)>);

/// The state, saved chunks and tickets of `region`, as a restore takes them.
fn saved(region: &Region) -> Saved {
    let chunks = region
        .loaded_chunks()
        .into_iter()
        .map(|position| (position, region.saved_chunk(position).unwrap()))
        .collect();
    let tickets = region
        .tickets
        .iter()
        .flat_map(|(position, tickets)| {
            let mut kinds = Vec::new();
            for _ in 0..tickets.viewers {
                kinds.push((*position, Ticket::Viewer));
            }
            kinds
        })
        .collect();
    (region.state(), chunks, tickets)
}

#[test]
fn a_thrown_item_survives_a_restore_of_the_region_and_its_counters_go_on_from_where_they_were() {
    let mut original = on_ticking_floor_with(&[]);
    original.tick(&moves(vec![throw_held(false)]));
    for _ in 0..10 {
        idle(&mut original);
    }
    let item = entities_in(&original, ORIGIN)[0].clone();
    assert_eq!(counters(&item), (11, 29));

    // Restored as it was a tick before, from its state and saved chunks: it goes on as
    // the original does, without taking anything in.
    let (state, chunks, tickets) = saved(&original);
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        ..Holdings::default()
    };
    let mut copy = Region::restore_loaded(
        original.config.clone(),
        state.clone(),
        holdings.clone(),
        &tickets,
        chunks.clone(),
        Vec::new(),
    );
    assert_eq!(copy.entity(item.key), original.entity(item.key));
    for _ in 0..5 {
        let (a, b) = (idle(&mut original), idle(&mut copy));
        assert_eq!(a.entity_changes, b.entity_changes);
        assert_eq!(entities_in(&original, ORIGIN), entities_in(&copy, ORIGIN));
    }

    // Restored by another worker from the store: the chunks come as loads, in which
    // the region takes the item in, one change of holder more, committed whole; its
    // counters are where the chunk was saved, whatever the region's tick, and count on
    // from there in the restored region's own ticks.
    let mut state = state;
    state.tick = 3;
    let mut restored = Region::restore(original.config.clone(), state, holdings);
    let wanted: Vec<ChunkPos> = chunks.iter().map(|(position, _)| *position).collect();
    let asked = restored.tick(&tickets_of(Ticket::Viewer, &wanted, &[]));
    assert!(asked.chunk_requests.contains(&ORIGIN));
    let output = restored.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    let raised = Entity {
        hops: 1,
        ..item.clone()
    };
    // Its tick in the same tick goes on from there: it still slides, and commits what
    // its move set after the whole record.
    let [(ORIGIN, EntityChange::Whole(whole)), rest @ ..] = &output.entity_changes[..] else {
        panic!("{:?}", output.entity_changes);
    };
    assert_eq!(*whole, raised);
    assert!(
        rest.iter().all(|(chunk, change)| *chunk == ORIGIN
            && matches!(change, EntityChange::Set(key, _) if *key == item.key)),
        "{rest:?}"
    );
    assert_eq!(output.tick, 5);
    // The chunks loaded tick in that very tick: the item's count goes on by one.
    assert_eq!(counters(&entities_in(&restored, ORIGIN)[0]), (12, 28));
    assert_eq!(entities_in(&restored, ORIGIN)[0].hops, 1);
}

#[test]
fn hops_rise_at_every_load_and_the_item_is_shown_with_them() {
    let mut region = on_ticking_floor_with(&[]);
    let east = ChunkPos::new(1, 0);
    let mut chunk = floor();
    let lying = item_at(EntityId(1 << 33), [20.0, -60.0, 4.0], 5);
    chunk.set_entities(vec![lying.clone()]);
    // The chunk goes and comes back: each load takes the item in again.
    region.tick(&tickets(vec![], vec![east]));
    for hops in [6, 7] {
        let asked = region.tick(&tickets(vec![east], vec![]));
        assert_eq!(asked.chunk_requests, [east]);
        let output = region.tick(&TickInputs {
            chunks_loaded: vec![(east, chunk.clone())],
            ..TickInputs::default()
        });
        let [(position, EntityChange::Whole(entity))] = &output.entity_changes[..] else {
            panic!("{:?}", output.entity_changes);
        };
        assert_eq!((*position, entity.hops), (east, hops));
        let shown = region.entity(lying.key).unwrap();
        assert!(
            matches!(shown.kind, EntityKind::Other { hops: shown_hops, .. } if shown_hops == hops)
        );
        region.tick(&tickets(vec![], vec![east]));
        assert_eq!(region.entity(lying.key), None);
        // What the store would have: the chunk as the commit left it.
        chunk.apply_entity(&EntityChange::Whole(entity.clone()));
    }
}

/// A region with player 1 standing in chunk (1, 0), outside the home chunk, on a floor
/// of the chunks from x = -1 to 2 and z = -1 to 1, so that their chunk ticks; and an
/// item they threw there.
fn thrown_east() -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let floor_chunks: Vec<ChunkPos> = (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(floor_chunks.clone(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: floor_chunks
            .iter()
            .map(|position| (*position, floor()))
            .collect(),
        ..TickInputs::default()
    });
    let east = numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(20.5, -60.0, 4.5)),
            rotation: None,
            on_ground: true,
        },
    );
    let output = region.tick(&moves(vec![east, throw_held(false)]));
    assert_eq!(the_whole(&output).chunk(), ChunkPos::new(1, 0));
    region
}

/// A merge's and a split's warm chunks are loads as a restore's are: the region they
/// make is what a restore makes of the store's record (ADR-0014), so the item is taken
/// in again, one change of holder more, by the survivor of a merge and by the part of a
/// split, whose word the edge takes over the word it had (ADR-0028, section 6).
#[test]
fn the_regions_a_merge_and_a_split_make_take_in_the_items_of_their_warm_chunks() {
    let region = thrown_east();
    let east = ChunkPos::new(1, 0);
    let taken_in = |region: &mut Region, chunks: Vec<(ChunkPos, Chunk)>, hops: u32| {
        let positions: Vec<ChunkPos> = chunks.iter().map(|(position, _)| *position).collect();
        region.tick(&tickets_of(Ticket::Viewer, &positions, &[]));
        let output = region.tick(&TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        });
        let wholes: Vec<(ChunkPos, EntityId, u32)> = output
            .entity_changes
            .iter()
            .filter_map(|(position, change)| match change {
                EntityChange::Whole(entity) => Some((*position, entity.key, entity.hops)),
                // Its move in the same tick, after the record taken in.
                EntityChange::Set(..) => None,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(wholes, [(east, FIRST_ITEM, hops)]);
        let shown = region.entity(FIRST_ITEM).unwrap();
        assert!(
            matches!(shown.kind, EntityKind::Other { hops: shown_hops, .. } if shown_hops == hops)
        );
    };

    let mut merged = region.clone();
    let absorbed = RegionState::new(EntityIds::block(7).unwrap());
    let state = merged.absorb(RegionId(9), &absorbed);
    let warm = merged.take_absorbed(state, &[], &[], &[], &[]);
    taken_in(&mut merged, warm, 1);

    let mut split = region.clone();
    let splitting = split.split(&[east], RegionId(5), &[]).unwrap();
    assert!(splitting.chunks.contains(&east));
    let (kept, part) = split.take_split(splitting, &[]);
    let mut part_region = part.region;
    taken_in(&mut part_region, part.chunks, 1);
    // What the split left the region has no item.
    let ours: Vec<ChunkPos> = kept.iter().map(|(position, _)| *position).collect();
    split.tick(&tickets_of(Ticket::Viewer, &ours, &[]));
    let output = split.tick(&TickInputs {
        chunks_loaded: kept,
        ..TickInputs::default()
    });
    assert!(output.entity_changes.is_empty());
}

#[test]
fn a_copy_takes_the_entity_changes_of_its_holder_s_commits() {
    let mut region = on_ticking_floor_with(&[]);
    let beyond = ChunkPos::new(2, 0);
    let mut base = floor();
    base.set_entities(vec![item_at(EntityId(1 << 34), [40.0, -60.0, 2.0], 2)]);
    let moved = EntityChange::Set(
        EntityId(1 << 34),
        vec![clustine_world::EntityField::Position([41.0, -60.0, 3.0])],
    );
    let new = item_at(EntityId((1 << 34) + 1), [36.0, -60.0, 9.0], 0);
    // One outside the chunk it is said to be of, which the holder's chunk would not
    // have: passed over.
    let stray = item_at(EntityId((1 << 34) + 2), [4.0, -60.0, 9.0], 0);
    region.tick(&TickInputs {
        copy_bases: vec![(beyond, base)],
        copy_entity_updates: vec![(
            beyond,
            vec![
                moved,
                EntityChange::Whole(new.clone()),
                EntityChange::Whole(stray),
            ],
        )],
        ..TickInputs::default()
    });
    let copy = region.copy(beyond).unwrap();
    let positions: Vec<[f64; 3]> = copy
        .entities()
        .iter()
        .map(|entity| entity.position)
        .collect();
    assert_eq!(positions, [[41.0, -60.0, 3.0], new.position]);
    // A copy's entities are read, never ticked: the counters stay.
    idle(&mut region);
    assert_eq!(
        counters(&region.copy(beyond).unwrap().entities()[0]),
        (7, 33)
    );
    assert_eq!(region.entity(new.key), None);
}
