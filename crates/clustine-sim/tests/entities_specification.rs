//! Tests of the item a player throws, written from `docs/groundwork/n2-plan.md`
//! (sections 7 and 11) and `docs/adr/0028-entities.md` (sections 1 to 3 and 6) and the
//! public API alone, by someone who did not write the code: where the game puts a
//! thrown item and how it moves off, what Q, ctrl+Q and the creative screen take and
//! throw, what is refused and put back, the item's place in its chunk and in the
//! tick's output, and an entity loaded from the store.
//!
//! Player 1 stands at (0.5, -60, 0.5) on a stone floor at y = -61, with the 5×5 around
//! chunk (0, 0) loaded so that it ticks, and holds 64 stone in slot 0.

use clustine_data::{blocks, entity_types, items};
use clustine_sim::api::{EntityKind, HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, RegionEvent};
use clustine_sim::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerEvent, PlayerJoin, Region, RegionConfig,
    RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, Chunk, ChunkArea, ChunkPos, EdgeId, Entity, EntityChange, EntityData, EntityId,
    EntityIds, MAX_CHUNK_ENTITIES, PlayerId, Section, Vec3,
};
use uuid::Uuid;

const EDGE: EdgeId = EdgeId(7);
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const FEET: Vec3 = Vec3::new(0.5, -60.0, 0.5);

const HELD: ItemStack = ItemStack {
    item: items::STONE,
    count: 64,
};

/// The game's standing and crouching eye heights, floats, and how far below the eyes a
/// thrown item leaves (`ServerPlayer.drop`).
const STANDING_EYES: f32 = 1.62;
const CROUCHING_EYES: f32 = 1.27;
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn config() -> RegionConfig {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(HELD);
    RegionConfig {
        spawn: FEET,
        starting_inventory: Inventory::with_hotbar(hotbar),
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        light: Default::default(),
    }
}

/// An overworld column with a stone floor at y = -61 and air above.
fn floor() -> Chunk {
    let sections = (0..24)
        .map(|_| Section::filled(blocks::AIR, Biome(0)))
        .collect();
    let mut chunk = Chunk::from_sections(-64, sections);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, blocks::STONE);
        }
    }
    chunk
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region that knows the edge and holds the chunks near the origin, with the keys
/// `keys`.
fn region_with(keys: EntityIds) -> Region {
    region_in(keys, ChunkArea::EVERYWHERE)
}

/// The same, pinned to `area`, holding those chunks near the origin that are in it.
fn region_in(keys: EntityIds, area: ChunkArea) -> Region {
    let mut state = RegionState::new(keys);
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let holdings = Holdings {
        held: around(3)
            .into_iter()
            .filter(|chunk| area.contains(*chunk))
            .collect(),
        pinned: vec![area],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    Region::restore(config(), state, holdings)
}

/// Player 1 joined and entered the world.
fn joined(region: &mut Region) {
    region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(1),
                name: "Thrower".to_owned(),
                attempt: 7001,
            },
        )],
        ..TickInputs::default()
    });
    let state = region.state();
    let entered = state
        .entering
        .iter()
        .map(|(player, entering)| Entered {
            unit: None,
            player: *player,
            entity: entering.entity_id,
            place: None,
            holder: None,
        })
        .collect();
    region.tick(&TickInputs {
        entered,
        ..TickInputs::default()
    });
    assert!(region.player(player(1)).is_some(), "player 1 entered");
}

/// Loads `chunks` for a viewer, each a floor, `with` the one at the origin.
fn load(region: &mut Region, chunks: &[ChunkPos], with: Chunk) -> TickOutput {
    region.tick(&TickInputs {
        tickets_added: chunks
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    let loaded = chunks
        .iter()
        .map(|position| {
            let chunk = if *position == ORIGIN {
                with.clone()
            } else {
                floor()
            };
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: loaded,
        ..TickInputs::default()
    })
}

/// The region as these tests begin with it.
fn standing() -> Region {
    let mut region = region_with(EntityIds::block(1).expect("block 1"));
    joined(&mut region);
    load(&mut region, &around(2), floor());
    region
}

/// Player 1's key.
fn thrower(region: &Region) -> EntityId {
    region.player(player(1)).expect("player 1 is there").0
}

/// One tick with player 1's `inputs`, numbered on from `from`.
fn with_inputs(region: &mut Region, from: u64, inputs: Vec<PlayerInput>) -> TickOutput {
    let key = thrower(region);
    let inputs = inputs
        .into_iter()
        .enumerate()
        .map(|(index, input)| (EDGE, player(1), key, from + index as u64, input))
        .collect();
    region.tick(&TickInputs {
        inputs,
        ..TickInputs::default()
    })
}

fn q() -> PlayerInput {
    PlayerInput::Drop {
        all: false,
        crouching: false,
    }
}

fn turn(yaw: f32, pitch: f32) -> PlayerInput {
    PlayerInput::Move {
        position: None,
        rotation: Some((yaw, pitch)),
        on_ground: true,
    }
}

/// The entities a tick made, in order.
fn made(output: &TickOutput) -> Vec<Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(chunk, change)| match change {
            EntityChange::Whole(entity) => {
                assert_eq!(*chunk, entity.chunk(), "a whole entity in its own chunk");
                Some(entity.clone())
            }
            _ => None,
        })
        .collect()
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

/// What the region told player 1 of their inventory in `output`.
fn put_back(output: &TickOutput) -> Vec<(u8, Option<ItemStack>)> {
    output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::InventorySlot { slot, stack } if *to == player(1) => Some((*slot, *stack)),
            _ => None,
        })
        .collect()
}

/// The game's sine table (`Mth.sin`), worked out here from how the game fills it.
fn table_sin(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453_f64;
    let index = (f64::from(radians) * steps) as i64 & 0xFFFF;
    (index as f64 * std::f64::consts::TAU / 65_536.0).sin() as f32
}

fn table_cos(radians: f32) -> f32 {
    let steps = 10_430.378_350_470_453_f64;
    let index = (f64::from(radians) * steps + 16_384.0) as i64 & 0xFFFF;
    (index as f64 * std::f64::consts::TAU / 65_536.0).sin() as f32
}

/// What `LivingEntity.createItemStackToDrop` gives before its draws: x and z along the
/// look, a float each, and y, with its draws at their middle, as floats.
fn without_draws(yaw: f32, pitch: f32) -> [f64; 3] {
    let deg = std::f32::consts::PI / 180.0;
    let (sin_pitch, cos_pitch) = (table_sin(pitch * deg), table_cos(pitch * deg));
    let (sin_yaw, cos_yaw) = (table_sin(yaw * deg), table_cos(yaw * deg));
    [
        f64::from(-sin_yaw * cos_pitch * 0.3),
        f64::from(-sin_pitch * 0.3 + 0.1),
        f64::from(cos_yaw * cos_pitch * 0.3),
    ]
}

/// The plan, section 11: the item leaves at the player's x and z, 0.3 below their
/// eyes; its motion is along their look, a float of each of x and z, plus a turn of
/// at most 0.02 drawn at random (`cos(a) * d`, `sin(a) * d`, `d = 0.02 f`), and y a
/// float within 0.1 of `-sin(pitch) * 0.3 + 0.1`; its yaw is its own draw times 360;
/// it has one of the held stack, a pick-up delay of 40 and its thrower; its UUID is
/// of version 4; and the thrower's arm swings for others before it appears.
#[test]
fn a_thrown_item_leaves_from_below_the_eyes_along_the_look_with_the_game_s_motion() {
    for (yaw, pitch) in [(0.0, 0.0), (90.0, 0.0), (-135.0, 30.0), (30.0, -90.0)] {
        let mut region = standing();
        with_inputs(&mut region, 1, vec![turn(yaw, pitch)]);
        let key = region.state().next_entity_id;
        let output = with_inputs(&mut region, 2, vec![q()]);
        let [item] = &made(&output)[..] else {
            panic!("one item: {:?}", output.entity_changes);
        };
        let case = format!("yaw {yaw}, pitch {pitch}: {item:?}");
        let y = FEET.y + f64::from(STANDING_EYES) - BELOW_THE_EYES;
        assert_eq!(
            item.position.map(f64::to_bits),
            [FEET.x, y, FEET.z].map(f64::to_bits),
            "{case}"
        );
        assert_eq!(item.key, key, "{case}");
        assert_eq!(i32::from(item.kind), entity_types::ITEM, "{case}");
        assert_eq!(
            item.data,
            EntityData::Item {
                stack: ItemStack {
                    item: items::STONE,
                    count: 1
                },
                pick_up_delay: 40,
                thrower: Some(player(1).0),
                health: 5,
                age_offset: 0,
            },
            "{case}"
        );
        assert_eq!((item.pitch, item.on_ground, item.hops), (0.0, false, 0));
        assert!((0.0..360.0).contains(&item.yaw), "{case}");
        assert_eq!(item.uuid.get_version_num(), 4, "{case}");
        assert_eq!(item.uuid.get_variant(), uuid::Variant::RFC4122, "{case}");

        let base = without_draws(yaw, pitch);
        let (dx, dz) = (item.motion[0] - base[0], item.motion[2] - base[2]);
        assert!(
            (dx * dx + dz * dz).sqrt() <= 0.020_000_1,
            "a turn of more than 0.02: {case}"
        );
        assert!((item.motion[1] - base[1]).abs() <= 0.100_001, "{case}");
        // y is cast to a float as a whole, x and z are a float plus a double.
        assert_eq!(f64::from(item.motion[1] as f32), item.motion[1], "{case}");

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
            RegionEvent::ArmSwung { entity, chunk, .. },
            RegionEvent::EntitySpawned(state),
        ] = &shown[..]
        else {
            panic!("a swing, then the item: {shown:?}");
        };
        assert_eq!((*entity, *chunk), (thrower(&region), ORIGIN));
        assert_eq!(state.entity, item.key);
        assert_eq!(state.pose.position, Vec3::new(FEET.x, y, FEET.z));
        let EntityKind::Other {
            kind,
            uuid,
            motion,
            hops,
            data,
            ..
        } = &state.kind
        else {
            panic!("an entity other than a player: {state:?}");
        };
        assert_eq!(
            (*kind, *uuid, motion.map(f64::to_bits), *hops, data),
            (
                item.kind,
                item.uuid,
                item.motion.map(f64::to_bits),
                0,
                &item.data
            )
        );
        // The region has it in its chunk, under its key, and gives out the next.
        let lying = region.chunk(ORIGIN).expect("loaded").entities();
        assert_eq!(lying.len(), 1);
        assert_eq!(lying[0].key, item.key);
        assert_eq!(region.state().next_entity_id, EntityId(key.0 + 1));
    }
}

/// The motion's draws are the thrower's, four a throw: two throws in one tick move
/// off differently, and the same throws in the same world give the same items, to
/// the bit (`docs/adr/0028-entities.md`, section 4).
#[test]
fn two_throws_in_a_tick_draw_on_and_the_same_throws_give_the_same_items() {
    let throws = |region: &mut Region| {
        with_inputs(region, 1, vec![turn(45.0, 10.0)]);
        made(&with_inputs(region, 2, vec![q(), q()]))
    };
    let (mut one, mut other) = (standing(), standing());
    let (first, second) = (throws(&mut one), throws(&mut other));
    assert_eq!(first.len(), 2);
    assert_ne!(
        first[0].motion.map(f64::to_bits),
        first[1].motion.map(f64::to_bits)
    );
    assert_ne!(first[0].uuid, first[1].uuid);
    assert_eq!(first, second);
}

/// A crouching player's eyes are 1.27 above their feet, and the item leaves from 0.3
/// below them.
#[test]
fn a_crouching_player_throws_from_lower_eyes() {
    let mut region = standing();
    let output = with_inputs(
        &mut region,
        1,
        vec![PlayerInput::Drop {
            all: false,
            crouching: true,
        }],
    );
    let [item] = &made(&output)[..] else {
        panic!("one item: {:?}", output.entity_changes);
    };
    assert_eq!(
        item.position[1].to_bits(),
        (FEET.y + f64::from(CROUCHING_EYES) - BELOW_THE_EYES).to_bits()
    );
}

/// Q takes one from the held stack, ctrl+Q the rest of it, in creative too
/// (`Inventory.removeFromSelected`); an empty hand throws nothing and the tick makes
/// no entity change.
#[test]
fn q_throws_one_ctrl_q_the_stack_and_an_empty_hand_nothing() {
    let mut region = standing();
    let output = with_inputs(&mut region, 1, vec![q()]);
    assert_eq!(made(&output).len(), 1);
    assert_eq!(
        held(&region, 0),
        Some(ItemStack {
            item: items::STONE,
            count: 63
        })
    );
    let output = with_inputs(
        &mut region,
        2,
        vec![PlayerInput::Drop {
            all: true,
            crouching: false,
        }],
    );
    let [rest] = &made(&output)[..] else {
        panic!("one item: {:?}", output.entity_changes);
    };
    assert_eq!(
        stack_of(rest),
        ItemStack {
            item: items::STONE,
            count: 63
        }
    );
    assert_eq!(held(&region, 0), None);
    for (number, all) in [(3, false), (4, true)] {
        let output = with_inputs(
            &mut region,
            number,
            vec![PlayerInput::Drop {
                all,
                crouching: false,
            }],
        );
        // The items thrown before fly on and commit their moves; nothing new is made.
        assert!(made(&output).is_empty(), "{:?}", output.entity_changes);
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::EntitySpawned(_))),
            "{:?}",
            output.events
        );
    }
    // The two thrown fall together and merge, as the game's do: one stack of 64.
    let lying: i32 = region
        .chunk(ORIGIN)
        .expect("loaded")
        .entities()
        .iter()
        .map(|entity| stack_of(entity).count)
        .sum();
    assert_eq!(lying, 64);
}

/// The creative screen's throw (`SetCreativeModeSlot` with slot -1) throws the stack
/// named and takes nothing from the inventory; a stack of an item that does not exist,
/// or of none, throws nothing.
#[test]
fn the_creative_screen_throws_the_stack_named_and_leaves_the_inventory_as_it_was() {
    let mut region = standing();
    let dirt = ItemStack {
        item: items::DIRT,
        count: 17,
    };
    let output = with_inputs(&mut region, 1, vec![PlayerInput::Throw { stack: dirt }]);
    let [item] = &made(&output)[..] else {
        panic!("one item: {:?}", output.entity_changes);
    };
    assert_eq!(stack_of(item), dirt);
    let EntityData::Item {
        pick_up_delay,
        thrower,
        ..
    } = &item.data
    else {
        panic!("{:?} is not an item", item);
    };
    assert_eq!((*pick_up_delay, *thrower), (40, Some(player(1).0)));
    assert_eq!(held(&region, 0), Some(HELD));
    for (number, stack) in [
        (
            2,
            ItemStack {
                item: i32::MAX,
                count: 1,
            },
        ),
        (
            3,
            ItemStack {
                item: items::DIRT,
                count: 0,
            },
        ),
    ] {
        let output = with_inputs(&mut region, number, vec![PlayerInput::Throw { stack }]);
        assert!(made(&output).is_empty(), "{stack:?} was thrown");
    }
}

/// Step N3d (`docs/groundwork/n3-plan.md`, section 5, G2), in place of the plan of N2's
/// section 11, which refused it: a throw from a chunk the region has loaded but does not
/// tick (its ring is not loaded) goes as an arrival into that chunk, with no hops, and
/// is neither committed nor shown here; the slot is emptied as for any throw.
#[test]
fn a_throw_from_a_chunk_that_does_not_tick_is_sent_as_an_arrival() {
    let mut region = region_with(EntityIds::block(1).expect("block 1"));
    joined(&mut region);
    load(&mut region, &[ORIGIN], floor());
    assert!(region.chunk(ORIGIN).is_some());
    let output = with_inputs(&mut region, 1, vec![q()]);
    assert!(made(&output).is_empty(), "{:?}", output.entity_changes);
    let [(chunk, thrown)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!((*chunk, thrown.chunk(), thrown.hops), (ORIGIN, ORIGIN, 0));
    assert_eq!(stack_of(thrown), ItemStack { count: 1, ..HELD });
    assert!(
        !output.events.iter().any(|event| matches!(
            event,
            RegionEvent::EntitySpawned(_) | RegionEvent::HandedOn { .. }
        )),
        "{:?}",
        output.events
    );
    assert!(put_back(&output).is_empty());
    assert_eq!(held(&region, 0), Some(ItemStack { count: 63, ..HELD }));
}

/// The same where the chunk holds as many entities as a chunk may.
#[test]
fn a_throw_into_a_full_chunk_is_refused_and_the_slot_put_back() {
    let mut full = floor();
    let crowd: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u64)
        .map(|n| lying(EntityId((5 << 32) + n), 0))
        .collect();
    full.set_entities(crowd);
    let mut region = region_with(EntityIds::block(1).expect("block 1"));
    joined(&mut region);
    load(&mut region, &around(2), full);
    assert_eq!(
        region.chunk(ORIGIN).expect("loaded").entities().len(),
        MAX_CHUNK_ENTITIES
    );
    let output = with_inputs(&mut region, 1, vec![q()]);
    assert!(made(&output).is_empty());
    assert_eq!(put_back(&output), [(0, Some(HELD))]);
    assert_eq!(held(&region, 0), Some(HELD));
}

/// ADR-0028, section 1: a region whose block of keys is used up spawns nothing; the
/// throw is refused and the slot put back.
#[test]
fn a_region_with_no_key_left_throws_nothing_and_puts_the_slot_back() {
    let block = EntityIds::block(1).expect("block 1");
    // One key: the player's.
    let mut region = region_with(EntityIds {
        first: block.first,
        end: EntityId(block.first.0 + 1),
    });
    joined(&mut region);
    load(&mut region, &around(2), floor());
    let output = with_inputs(&mut region, 1, vec![q()]);
    assert!(made(&output).is_empty());
    assert_eq!(put_back(&output), [(0, Some(HELD))]);
}

/// An item at rest, as the store keeps it: at the middle of the origin's chunk.
fn lying(key: EntityId, hops: u32) -> Entity {
    Entity {
        key,
        kind: u16::try_from(entity_types::ITEM).expect("fits"),
        uuid: Uuid::from_u128(u128::from(key.0) | (1 << 100)),
        position: [8.5, -60.0, 8.5],
        motion: [0.0, 0.0, 0.0],
        yaw: 90.0,
        pitch: 0.0,
        on_ground: true,
        hops,
        lived: 5,
        fresh_at: 0,
        data: EntityData::Item {
            stack: ItemStack {
                item: items::DIRT,
                count: 2,
            },
            pick_up_delay: 35,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
    }
}

/// ADR-0028, section 6, and the plan, section 11: a load of a chunk takes its entities
/// in with one more change of holder each, committed whole, with their counters as the
/// store had them; a snapshot's state of them has that `hops`. An item at rest then
/// commits nothing, tick after tick.
#[test]
fn a_loaded_item_is_taken_in_with_one_more_hop_and_then_commits_nothing_while_it_lies() {
    let key = EntityId((5 << 32) + 1);
    let mut with_item = floor();
    with_item.set_entities(vec![lying(key, 2)]);
    let mut region = region_with(EntityIds::block(1).expect("block 1"));
    joined(&mut region);
    let output = load(&mut region, &around(2), with_item);
    let taken: Vec<&(ChunkPos, EntityChange)> = output
        .entity_changes
        .iter()
        .filter(|(_, change)| change.key() == key)
        .collect();
    let [(chunk, EntityChange::Whole(entity))] = taken[..] else {
        panic!("one whole entity: {:?}", output.entity_changes);
    };
    assert_eq!(*chunk, ORIGIN);
    assert_eq!(
        *entity,
        Entity {
            hops: 3,
            ..lying(key, 2)
        }
    );
    for _ in 0..20 {
        let output = region.tick(&TickInputs::default());
        assert!(
            output.entity_changes.is_empty(),
            "an item at rest committed {:?}",
            output.entity_changes
        );
    }
    let kept = &region.chunk(ORIGIN).expect("loaded").entities()[0];
    let EntityData::Item { pick_up_delay, .. } = &kept.data else {
        panic!("{kept:?} is not an item");
    };
    assert_eq!(u32::from(*pick_up_delay) + kept.lived, 40, "{kept:?}");
    assert_eq!(kept.hops, 3);
}

/// ADR-0028, section 3: a copy of a neighbour's chunk is based with its entities and
/// takes the entity changes of later commits in their order; the region reads them
/// there, and never ticks or commits them.
#[test]
fn a_copy_of_a_neighbour_s_chunk_has_its_items_and_takes_their_changes_without_committing_them() {
    let beyond = ChunkPos::new(3, 0);
    // Pinned west of chunk x = 3, which is a neighbour's.
    let mut region = region_in(
        EntityIds::block(1).expect("block 1"),
        ChunkArea {
            min_x: None,
            max_x: Some(3),
        },
    );
    joined(&mut region);
    load(&mut region, &around(2), floor());
    let key = EntityId((6 << 32) + 4);
    let mut item = lying(key, 0);
    item.position = [56.5, -60.0, 8.5];
    let mut base = floor();
    base.set_entities(vec![item.clone()]);
    let output = region.tick(&TickInputs {
        copy_bases: vec![(beyond, base)],
        ..TickInputs::default()
    });
    assert!(
        output.entity_changes.is_empty(),
        "{:?}",
        output.entity_changes
    );
    let copied = region.copy(beyond).expect("the copy is kept");
    assert_eq!(copied.entities(), [item.clone()]);

    let moved = [57.25, -60.0, 8.5];
    let output = region.tick(&TickInputs {
        copy_entity_updates: vec![(
            beyond,
            vec![EntityChange::Set(
                key,
                vec![clustine_world::EntityField::Position(moved)],
            )],
        )],
        ..TickInputs::default()
    });
    assert!(
        output.entity_changes.is_empty(),
        "{:?}",
        output.entity_changes
    );
    let copied = region.copy(beyond).expect("the copy is kept");
    assert_eq!(copied.entities().len(), 1);
    assert_eq!(copied.entities()[0].position, moved);

    let output = region.tick(&TickInputs {
        copy_entity_updates: vec![(beyond, vec![EntityChange::Gone(key)])],
        ..TickInputs::default()
    });
    assert!(output.entity_changes.is_empty());
    assert!(region.copy(beyond).expect("kept").entities().is_empty());
}
