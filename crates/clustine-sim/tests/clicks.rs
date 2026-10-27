//! Tests of what a region does with a player's click, a bucket and a new player's
//! place, alone and across regions, written from `docs/groundwork/w6-plan.md`
//! (sections 2, 3.2, 4.3's all-or-nothing rule, 4.5, 4.6 and 5) and the public API
//! alone, by someone who has not read how the region does it.
//!
//! The rules of placing themselves are W6d's and W6f's to judge: these tests use
//! rules whose outcome the plan states fully (stone, a log's axis, a torch on a wall,
//! a door, a bed) only to see that the region applies them, in the right region and
//! all or nothing.
//!
//! Where the plan leaves the region's side open, the reading taken is marked
//! "Reading:" in a comment.

use clustine_data::{BlockState, blocks, items};
use clustine_sim::api::{
    Face, HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, Pose, RegionEvent, RemoteAction,
    RemoteStep,
};
use clustine_sim::placement::Click;
use clustine_sim::{
    Durable, EdgeEvent, Entered, Holdings, Knowledge, Misdirected, PlayerChange, PlayerEvent,
    PlayerJoin, Region, RegionConfig, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Section, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);

/// The chunk region A holds, and the one region B holds east of it.
const CHUNK_A: ChunkPos = ChunkPos::new(0, 0);
const CHUNK_B: ChunkPos = ChunkPos::new(1, 0);
/// A chunk south of A's, in A's pinned stripe.
const CHUNK_SOUTH: ChunkPos = ChunkPos::new(0, 1);

const REGION_A: RegionId = RegionId(0);
const REGION_B: RegionId = RegionId(1);

/// Region A's spawn point, two blocks from region B, on the stone of [`stone_chunk`].
const SPAWN_A: Vec3 = Vec3::new(14.5, 64.0, 8.5);
/// The block whose column the spawn point is in.
const SPAWN_COLUMN: BlockPos = BlockPos::new(14, 64, 8);

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn stack(item: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, 1))
}

fn starting_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [const { None }; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack::new(items::STONE, 64));
    hotbar
}

fn config(spawn: Vec3) -> RegionConfig {
    RegionConfig {
        spawn,
        starting_inventory: Inventory::with_hotbar(starting_hotbar()),
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
    }
}

fn holdings(held: Vec<ChunkPos>, pinned: ChunkArea) -> Holdings {
    Holdings {
        held,
        pinned: vec![pinned],
        lent: Vec::new(),
        lent_away: Vec::new(),
    }
}

/// Region A's stripe: every chunk with x below 1.
const STRIPE_A: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
/// Region B's stripe: the chunks with x equal to 1.
const STRIPE_B: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: Some(2),
};

fn ids_a() -> EntityIds {
    EntityIds::block(3).expect("block 3 exists")
}

fn ids_b() -> EntityIds {
    EntityIds::block(4).expect("block 4 exists")
}

/// An overworld column of stone up to y = 63 and air above.
fn stone_chunk() -> Chunk {
    let sections = (0..24)
        .map(|index| {
            let state = if index < 8 {
                blocks::STONE
            } else {
                blocks::AIR
            };
            Section::filled(state, Biome(0))
        })
        .collect();
    Chunk::from_sections(-64, sections)
}

/// [`stone_chunk`] with the blocks given set, each where it is in its chunk.
fn chunk_with(blocks: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = stone_chunk();
    for (position, state) in blocks {
        let (x, z) = position.in_chunk();
        chunk
            .set(x, position.y, z, *state)
            .expect("the block is within the world");
    }
    chunk
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

/// A tick with the world store's answers to the stays the region holds as entering
/// (each may enter, with no place), which holds the region to its delta.
fn tick(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    let mut inputs = inputs.clone();
    let answers = region
        .state()
        .entering
        .iter()
        .map(|(player, entering)| Entered {
            unit: None,
            player: *player,
            entity: entering.entity_id,
            place: None,
            holder: None,
        })
        .collect::<Vec<_>>();
    inputs.entered.extend(answers);
    let before = region.state();
    let output = region.tick(&inputs);
    let mut applied = before;
    applied.apply(&output.delta);
    assert_eq!(applied, region.state(), "the delta makes the state after");
    output
}

fn idle(region: &mut Region) -> TickOutput {
    tick(region, &TickInputs::default())
}

fn started() -> Vec<EdgeEvent> {
    vec![EdgeEvent::Started { edge: E, start: 10 }]
}

/// A region holding `own` (the chunk of its spawn point) with `chunk` loaded there, as
/// clients' tickets load it, and believing `across` to be `neighbour`'s.
fn bordering(
    config: RegionConfig,
    ids: EntityIds,
    holdings: Holdings,
    own: ChunkPos,
    chunk: Chunk,
    across: ChunkPos,
    neighbour: RegionId,
) -> Region {
    let mut region = Region::new(config, ids, holdings);
    let output = tick(
        &mut region,
        &TickInputs {
            edges: started(),
            tickets_added: vec![(own, Ticket::Viewer), (across, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, vec![own]);
    assert_eq!(output.claims, vec![across]);
    tick(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(own, chunk)],
            foreign: vec![(across, neighbour)],
            ..TickInputs::default()
        },
    );
    assert!(region.chunk(own).is_some());
    assert_eq!(region.knowledge(across), Knowledge::Foreign(neighbour));
    region
}

fn region_a(chunk: Chunk) -> Region {
    bordering(
        config(SPAWN_A),
        ids_a(),
        holdings(vec![CHUNK_A], STRIPE_A),
        CHUNK_A,
        chunk,
        CHUNK_B,
        REGION_B,
    )
}

fn region_b(chunk: Chunk) -> Region {
    bordering(
        config(Vec3::new(24.5, 64.0, 8.5)),
        ids_b(),
        holdings(vec![CHUNK_B], STRIPE_B),
        CHUNK_B,
        chunk,
        CHUNK_A,
        REGION_A,
    )
}

/// One region holding both A's and B's chunk, loaded, with A's spawn point.
fn region_of_both(a: Chunk, b: Chunk) -> Region {
    let mut region = Region::new(
        config(SPAWN_A),
        ids_a(),
        holdings(vec![CHUNK_A, CHUNK_B], ChunkArea::EVERYWHERE),
    );
    let output = tick(
        &mut region,
        &TickInputs {
            edges: started(),
            tickets_added: vec![(CHUNK_A, Ticket::Viewer), (CHUNK_B, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    let mut requests = output.chunk_requests.clone();
    requests.sort();
    assert_eq!(requests, vec![CHUNK_A, CHUNK_B]);
    tick(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(CHUNK_A, a), (CHUNK_B, b)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.loaded_chunk_count(), 2);
    region
}

/// A player in a region, and the number of their last input.
struct Actor {
    id: PlayerId,
    entity: EntityId,
    number: u64,
}

fn join(n: u128) -> PlayerChange {
    PlayerChange::Join(
        E,
        PlayerJoin {
            player: player(n),
            name: format!("player-{n}"),
            attempt: 7000 + n as u64,
        },
    )
}

/// Player `n` joins through `E` and enters in the tick after, as the store answers.
/// Returns the player and the pose they were spawned with.
fn enter(region: &mut Region, n: u128) -> (Actor, Pose) {
    let mut inputs = TickInputs::default();
    inputs.change(join(n));
    tick(region, &inputs);
    let output = idle(region);
    let pose = output
        .player_events
        .iter()
        .find_map(|(id, event)| match event {
            PlayerEvent::Spawned { pose, .. } if *id == player(n) => Some(*pose),
            _ => None,
        })
        .expect("the player is spawned in the tick of the store's answer");
    let (entity, _) = region.player(player(n)).expect("the player is in");
    let actor = Actor {
        id: player(n),
        entity,
        number: 0,
    };
    (actor, pose)
}

/// A tick in which `actor` does `inputs`, in order.
fn act(region: &mut Region, actor: &mut Actor, inputs: Vec<PlayerInput>) -> TickOutput {
    let mut tick_inputs = TickInputs::default();
    for input in inputs {
        actor.number += 1;
        tick_inputs.input(E, actor.id, actor.entity, actor.number, input);
    }
    tick(region, &tick_inputs)
}

/// What puts `item` into the hand: the first hotbar slot, selected.
fn hold(item: Option<i32>) -> Vec<PlayerInput> {
    vec![
        PlayerInput::SetInventorySlot {
            slot: 0,
            stack: item.and_then(stack),
        },
        PlayerInput::SelectSlot { slot: 0 },
    ]
}

fn click(position: BlockPos, face: Face, sneaking: bool, sequence: i32) -> PlayerInput {
    PlayerInput::UseItemOn {
        position,
        face,
        cursor: cursor_on(face),
        sneaking,
        sequence,
    }
}

/// The middle of `face` of a block, as a cursor.
fn cursor_on(face: Face) -> [f32; 3] {
    match face {
        Face::Bottom => [0.5, 0.0, 0.5],
        Face::Top => [0.5, 1.0, 0.5],
        Face::North => [0.5, 0.5, 0.0],
        Face::South => [0.5, 0.5, 1.0],
        Face::West => [0.0, 0.5, 0.5],
        Face::East => [1.0, 0.5, 0.5],
    }
}

/// `item` taken into the hand and then a click with it, in one tick.
fn click_with(
    region: &mut Region,
    actor: &mut Actor,
    item: Option<i32>,
    input: PlayerInput,
) -> TickOutput {
    let mut inputs = hold(item);
    inputs.push(input);
    act(region, actor, inputs)
}

fn changed(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

fn acknowledged(output: &TickOutput, id: PlayerId) -> Vec<i32> {
    output
        .player_events
        .iter()
        .filter_map(|(p, event)| match event {
            PlayerEvent::Acknowledged { sequence } if *p == id => Some(*sequence),
            _ => None,
        })
        .collect()
}

/// The `Remote`s of a tick, with the edge whose outbox each is in.
fn remotes(output: &TickOutput) -> Vec<(EdgeId, RemoteAction, Option<RegionId>)> {
    output
        .durable
        .iter()
        .filter_map(|(edge, _, entry)| match entry {
            Durable::Remote { action, to } => Some((*edge, action.clone(), *to)),
            _ => None,
        })
        .collect()
}

/// The sequences that `RemoteDone`s of a tick answer.
fn done(output: &TickOutput) -> Vec<i32> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, entry)| match entry {
            Durable::RemoteDone { sequence, .. } => Some(*sequence),
            _ => None,
        })
        .collect()
}

/// A tick of `region` with one remote action through `E`.
fn take(region: &mut Region, action: RemoteAction) -> TickOutput {
    tick(
        region,
        &TickInputs {
            remote_actions: vec![(E, action)],
            ..TickInputs::default()
        },
    )
}

/// Carries what `output` of region A passes on from region to region, as edges do,
/// until every action is done. Returns the sequences answered done, in order, and the
/// steps that were passed on.
fn settle(a: &mut Region, b: &mut Region, output: &TickOutput) -> (Vec<i32>, Vec<RemoteStep>) {
    let mut pending: Vec<(RegionId, RemoteAction)> = remotes(output)
        .into_iter()
        .map(|(edge, action, to)| {
            assert_eq!(edge, E, "the player's own action goes to their edge");
            (to.expect("the neighbour is known"), action)
        })
        .collect();
    let mut answered = Vec::new();
    let mut steps = Vec::new();
    for _ in 0..8 {
        if pending.is_empty() {
            break;
        }
        let (to, action) = pending.remove(0);
        steps.push(action.step.clone());
        let region = if to == REGION_A { &mut *a } else { &mut *b };
        let output = take(region, action);
        for (edge, _, entry) in &output.durable {
            assert_eq!(*edge, E, "an answer goes to the edge the action came from");
            match entry {
                Durable::RemoteDone { sequence, .. } => answered.push(*sequence),
                Durable::Remote { action, to } => {
                    pending.push((to.expect("the neighbour is known"), action.clone()));
                }
                Durable::NotMine {
                    what: Misdirected::Remote(action),
                    holder,
                } => pending.push((*holder, action.clone())),
                other => panic!("unexpected entry {other:?}"),
            }
        }
    }
    assert!(pending.is_empty(), "the actions come to an end");
    (answered, steps)
}

fn door(facing: &str, half: &str, open: bool) -> BlockState {
    let open = if open { "true" } else { "false" };
    state(&format!(
        "minecraft:oak_door[facing={facing},half={half},hinge=left,open={open},powered=false]"
    ))
}

// Section 2: where a new player stands.

/// Region A holding its home chunk, which storage delivers as `chunk` before anyone
/// joins, without a ticket.
fn home_loaded(chunk: Chunk) -> Region {
    let mut region = Region::new(config(SPAWN_A), ids_a(), holdings(vec![CHUNK_A], STRIPE_A));
    let output = tick(
        &mut region,
        &TickInputs {
            edges: started(),
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, vec![CHUNK_A]);
    tick(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(CHUNK_A, chunk)],
            ..TickInputs::default()
        },
    );
    assert!(region.chunk(CHUNK_A).is_some(), "the home chunk is taken");
    region
}

fn spawned_on(chunk: Chunk) -> Vec3 {
    let mut region = home_loaded(chunk);
    let (_, pose) = enter(&mut region, 1);
    let (_, now) = region.player(player(1)).expect("the player is in");
    assert_eq!(
        now.position, pose.position,
        "the region has them where it said"
    );
    pose.position
}

fn column(from_y: i32, to_y: i32, state: BlockState) -> Vec<(BlockPos, BlockState)> {
    (from_y..=to_y)
        .map(|y| (BlockPos::new(SPAWN_COLUMN.x, y, SPAWN_COLUMN.z), state))
        .collect()
}

#[test]
fn a_new_player_stands_on_the_spawn_column_as_generated() {
    assert_eq!(spawned_on(stone_chunk()), Vec3::new(14.5, 64.0, 8.5));
}

#[test]
fn a_new_player_stands_on_a_tower_built_at_the_spawn() {
    let chunk = chunk_with(&column(64, 69, blocks::STONE));
    assert_eq!(spawned_on(chunk), Vec3::new(14.5, 70.0, 8.5));
}

#[test]
fn a_new_player_stands_in_a_hole_dug_at_the_spawn() {
    let chunk = chunk_with(&column(61, 63, blocks::AIR));
    assert_eq!(spawned_on(chunk), Vec3::new(14.5, 61.0, 8.5));
}

#[test]
fn a_new_player_stands_on_a_fence_at_its_top() {
    let chunk = chunk_with(&column(64, 64, blocks::OAK_FENCE));
    assert_eq!(spawned_on(chunk), Vec3::new(14.5, 65.5, 8.5));
}

#[test]
fn a_new_player_stands_on_a_bottom_slab_at_its_top() {
    let slab = state("minecraft:oak_slab[type=bottom,waterlogged=false]");
    let chunk = chunk_with(&column(64, 64, slab));
    assert_eq!(spawned_on(chunk), Vec3::new(14.5, 64.5, 8.5));
}

#[test]
fn a_new_player_stands_on_water_over_the_floor() {
    let chunk = chunk_with(&column(64, 66, blocks::WATER));
    assert_eq!(spawned_on(chunk), Vec3::new(14.5, 67.0, 8.5));
}

#[test]
fn a_new_player_enters_at_the_spawn_height_over_an_empty_column() {
    let chunk = chunk_with(&column(-64, 63, blocks::AIR));
    assert_eq!(spawned_on(chunk), SPAWN_A);
}

#[test]
fn a_new_player_stands_on_what_others_have_built_and_dug_since() {
    let mut region = home_loaded(stone_chunk());
    let (mut builder, _) = enter(&mut region, 1);
    act(
        &mut region,
        &mut builder,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(12.5, 64.0, 8.5)),
            rotation: Some((-90.0, 0.0)),
            on_ground: true,
        }],
    );
    let below = SPAWN_COLUMN.offset(0, -1, 0);
    let output = click_with(
        &mut region,
        &mut builder,
        Some(items::STONE),
        click(below, Face::Top, false, 1),
    );
    assert_eq!(changed(&output), vec![(SPAWN_COLUMN, blocks::STONE)]);
    let output = act(
        &mut region,
        &mut builder,
        vec![click(SPAWN_COLUMN, Face::Top, false, 2)],
    );
    assert_eq!(
        changed(&output),
        vec![(SPAWN_COLUMN.offset(0, 1, 0), blocks::STONE)]
    );
    let (_, pose) = enter(&mut region, 2);
    assert_eq!(pose.position, Vec3::new(14.5, 66.0, 8.5));

    // Then dug down to below the floor.
    let mut digs = Vec::new();
    for (sequence, y) in (61..=65).rev().enumerate() {
        digs.push(PlayerInput::Dig {
            position: BlockPos::new(SPAWN_COLUMN.x, y, SPAWN_COLUMN.z),
            sequence: 3 + sequence as i32,
        });
    }
    act(&mut region, &mut builder, digs);
    assert_eq!(
        block_at(&region, BlockPos::new(14, 61, 8)),
        Some(blocks::AIR)
    );
    let (_, pose) = enter(&mut region, 3);
    assert_eq!(pose.position, Vec3::new(14.5, 61.0, 8.5));
}

#[test]
fn before_the_home_chunk_is_loaded_a_new_player_enters_at_the_spawn_and_is_not_moved_when_it_arrives()
 {
    let mut region = Region::new(config(SPAWN_A), ids_a(), holdings(vec![CHUNK_A], STRIPE_A));
    let output = tick(
        &mut region,
        &TickInputs {
            edges: started(),
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, vec![CHUNK_A]);
    let (_, pose) = enter(&mut region, 1);
    assert_eq!(pose.position, SPAWN_A);

    let output = tick(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(CHUNK_A, chunk_with(&column(64, 69, blocks::STONE)))],
            ..TickInputs::default()
        },
    );
    assert!(region.chunk(CHUNK_A).is_some());
    assert!(
        !output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::EntityMoved { .. })),
        "nobody is moved when the chunk arrives"
    );
    let (_, now) = region.player(player(1)).expect("the player is in");
    assert_eq!(now.position, SPAWN_A);

    // Whoever enters now stands on the tower.
    let (_, pose) = enter(&mut region, 2);
    assert_eq!(pose.position, Vec3::new(14.5, 70.0, 8.5));
}

#[test]
fn the_home_chunk_is_asked_of_storage_without_a_ticket_and_kept_when_its_last_ticket_goes() {
    let mut region = home_loaded(stone_chunk());
    assert_eq!(region.loaded_chunk_count(), 1);
    tick(
        &mut region,
        &TickInputs {
            tickets_added: vec![(CHUNK_A, Ticket::Viewer), (CHUNK_A, Ticket::Guest)],
            ..TickInputs::default()
        },
    );
    let output = tick(
        &mut region,
        &TickInputs {
            tickets_removed: vec![(CHUNK_A, Ticket::Viewer), (CHUNK_A, Ticket::Guest)],
            ..TickInputs::default()
        },
    );
    assert!(output.returns.is_empty());
    for _ in 0..3 {
        let output = idle(&mut region);
        assert!(
            output.returns.is_empty(),
            "the home chunk is never given back"
        );
        assert!(output.chunk_requests.is_empty(), "nor read again");
    }
    assert!(
        region.chunk(CHUNK_A).is_some(),
        "the home chunk stays loaded"
    );
}

#[test]
fn a_held_chunk_that_is_not_the_home_chunk_is_not_asked_of_storage_without_a_ticket() {
    // The contrast to the home chunk: what section 2 changes is the home chunk's alone.
    let mut region = Region::new(
        config(SPAWN_A),
        ids_a(),
        holdings(vec![CHUNK_A, CHUNK_SOUTH], STRIPE_A),
    );
    let output = idle(&mut region);
    assert_eq!(output.chunk_requests, vec![CHUNK_A]);
}

#[test]
fn a_restored_home_region_asks_for_its_home_chunk_again_and_places_by_it_once_it_is_there() {
    let region = home_loaded(chunk_with(&column(64, 64, blocks::OAK_FENCE)));
    let saved = region.state();
    let mut restored = Region::restore(config(SPAWN_A), saved, holdings(vec![CHUNK_A], STRIPE_A));
    assert!(restored.chunk(CHUNK_A).is_none());
    let output = idle(&mut restored);
    assert_eq!(output.chunk_requests, vec![CHUNK_A]);
    tick(
        &mut restored,
        &TickInputs {
            chunks_loaded: vec![(CHUNK_A, chunk_with(&column(64, 64, blocks::OAK_FENCE)))],
            ..TickInputs::default()
        },
    );
    let (_, pose) = enter(&mut restored, 1);
    assert_eq!(pose.position, Vec3::new(14.5, 65.5, 8.5));
}

// Sections 3.2 and 4.3: a click in one region.

#[test]
fn a_click_with_stone_places_it_against_the_face_and_is_acknowledged() {
    let mut region = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut region, 1);
    let output = click_with(
        &mut region,
        &mut actor,
        Some(items::STONE),
        click(BlockPos::new(12, 63, 7), Face::Top, false, 4),
    );
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(12, 64, 7), blocks::STONE)]
    );
    assert_eq!(acknowledged(&output, actor.id), vec![4]);
    assert!(remotes(&output).is_empty());
    assert_eq!(
        block_at(&region, BlockPos::new(12, 64, 7)),
        Some(blocks::STONE)
    );
}

#[test]
fn a_log_placed_against_a_side_lies_along_it() {
    let pillar = BlockPos::new(11, 64, 7);
    let mut region = region_a(chunk_with(&[(pillar, blocks::STONE)]));
    let (mut actor, _) = enter(&mut region, 1);
    let output = click_with(
        &mut region,
        &mut actor,
        Some(items::OAK_LOG),
        click(pillar, Face::East, false, 1),
    );
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(12, 64, 7), state("minecraft:oak_log[axis=x]"))]
    );
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
}

#[test]
fn a_click_on_a_door_opens_and_closes_both_halves_and_sneaking_places_against_it() {
    let lower = BlockPos::new(12, 64, 9);
    let upper = lower.offset(0, 1, 0);
    let mut region = region_a(chunk_with(&[
        (lower, door("west", "lower", false)),
        (upper, door("west", "upper", false)),
    ]));
    let (mut actor, _) = enter(&mut region, 1);

    // With an empty hand: the early return for an empty hand is gone (3.2).
    let output = click_with(
        &mut region,
        &mut actor,
        None,
        click(lower, Face::East, false, 1),
    );
    let mut seen = changed(&output);
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (lower, door("west", "lower", true)),
            (upper, door("west", "upper", true)),
        ]
    );
    assert_eq!(acknowledged(&output, actor.id), vec![1]);

    // With stone in hand, not sneaking: the door is used, and nothing is placed.
    let output = click_with(
        &mut region,
        &mut actor,
        Some(items::STONE),
        click(upper, Face::East, false, 2),
    );
    let mut seen = changed(&output);
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (lower, door("west", "lower", false)),
            (upper, door("west", "upper", false)),
        ]
    );
    assert_eq!(block_at(&region, lower.offset(1, 0, 0)), Some(blocks::AIR));
    assert_eq!(acknowledged(&output, actor.id), vec![2]);

    // Sneaking with stone: placed against the door, which stays shut.
    let output = act(
        &mut region,
        &mut actor,
        vec![click(lower, Face::East, true, 3)],
    );
    assert_eq!(
        changed(&output),
        vec![(lower.offset(1, 0, 0), blocks::STONE)]
    );
    assert_eq!(block_at(&region, lower), Some(door("west", "lower", false)));
    assert_eq!(acknowledged(&output, actor.id), vec![3]);
}

#[test]
fn a_click_with_an_empty_hand_on_a_block_that_is_not_used_changes_nothing_and_is_acknowledged() {
    let mut region = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut region, 1);
    let output = click_with(
        &mut region,
        &mut actor,
        None,
        click(BlockPos::new(12, 63, 7), Face::Top, false, 1),
    );
    assert!(changed(&output).is_empty());
    assert!(output.durable.is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
}

/// Region A holding its chunk and the one south of it, with only its own loaded, and a
/// player standing near the line between the two.
fn half_loaded() -> (Region, Actor) {
    let spawn = Vec3::new(12.5, 64.0, 13.5);
    let mut region = Region::new(
        config(spawn),
        ids_a(),
        holdings(vec![CHUNK_A, CHUNK_SOUTH], STRIPE_A),
    );
    let output = tick(
        &mut region,
        &TickInputs {
            edges: started(),
            tickets_added: vec![(CHUNK_A, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.chunk_requests, vec![CHUNK_A]);
    tick(
        &mut region,
        &TickInputs {
            chunks_loaded: vec![(CHUNK_A, stone_chunk())],
            ..TickInputs::default()
        },
    );
    assert!(region.chunk(CHUNK_SOUTH).is_none());
    let (actor, pose) = enter(&mut region, 1);
    assert_eq!(pose.position, spawn);
    (region, actor)
}

#[test]
fn a_bed_whose_head_would_be_in_a_chunk_not_loaded_is_not_placed_at_all() {
    let (mut region, mut actor) = half_loaded();
    // Facing south, a bed's head goes one block south of its foot (4.4): across z = 16.
    let output = click_with(
        &mut region,
        &mut actor,
        Some(items::RED_BED),
        click(BlockPos::new(12, 63, 15), Face::Top, false, 1),
    );
    assert!(changed(&output).is_empty(), "all or nothing (4.3)");
    assert_eq!(
        block_at(&region, BlockPos::new(12, 64, 15)),
        Some(blocks::AIR)
    );
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
    assert!(output.durable.is_empty(), "the chunk is this region's own");

    // One block north, both halves are in the chunk the region has, and both go in.
    let output = act(
        &mut region,
        &mut actor,
        vec![click(BlockPos::new(12, 63, 14), Face::Top, false, 2)],
    );
    let foot = block_at(&region, BlockPos::new(12, 64, 14)).expect("loaded");
    let head = block_at(&region, BlockPos::new(12, 64, 15)).expect("loaded");
    assert_eq!(foot.block_id(), blocks::RED_BED.block_id());
    assert_eq!(foot.property("part"), Some("foot"));
    assert_eq!(foot.property("facing"), Some("south"));
    assert_eq!(head.block_id(), blocks::RED_BED.block_id());
    assert_eq!(head.property("part"), Some("head"));
    assert_eq!(changed(&output).len(), 2);
    assert_eq!(acknowledged(&output, actor.id), vec![2]);
}

#[test]
fn a_target_or_a_clicked_block_in_a_held_chunk_not_loaded_places_nothing() {
    let (mut region, mut actor) = half_loaded();
    // The target is across z = 16: never read as air.
    let output = click_with(
        &mut region,
        &mut actor,
        Some(items::STONE),
        click(BlockPos::new(12, 63, 15), Face::South, false, 1),
    );
    assert!(changed(&output).is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
    assert!(output.durable.is_empty());
    // The clicked block itself is there.
    let output = act(
        &mut region,
        &mut actor,
        vec![click(BlockPos::new(12, 63, 16), Face::Top, false, 2)],
    );
    assert!(changed(&output).is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![2]);
    assert!(output.durable.is_empty());
}

// Sections 4.5 and 4.6: across regions.

/// The click a player at A's spawn point, looking ahead, makes with `held`.
fn click_at_spawn(position: BlockPos, face: Face, sneaking: bool, held: Option<i32>) -> Click {
    Click {
        position,
        face,
        cursor: cursor_on(face),
        sneaking,
        held,
        feet: SPAWN_A,
        yaw: 0.0,
        pitch: 0.0,
    }
}

#[test]
fn a_click_on_the_other_regions_block_goes_there_whole_and_is_placed_and_answered_there() {
    let mut a = region_a(stone_chunk());
    let mut b = region_b(stone_chunk());
    let (mut actor, _) = enter(&mut a, 1);
    let clicked = BlockPos::new(16, 63, 8);
    let output = click_with(
        &mut a,
        &mut actor,
        Some(items::STONE),
        click(clicked, Face::Top, false, 5),
    );
    let expected = RemoteAction {
        player: actor.id,
        entity: actor.entity,
        sequence: 5,
        step: RemoteStep::UseOn {
            click: click_at_spawn(clicked, Face::Top, false, Some(items::STONE)),
        },
    };
    assert_eq!(
        remotes(&output),
        vec![(E, expected.clone(), Some(REGION_B))]
    );
    assert!(changed(&output).is_empty());
    assert!(
        acknowledged(&output, actor.id).is_empty(),
        "not acknowledged before region B has done it"
    );
    assert_eq!(a.state().players[&actor.id].handled, None);

    let output = take(&mut b, expected);
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(16, 64, 8), blocks::STONE)]
    );
    assert_eq!(
        output.durable,
        vec![(
            E,
            1,
            Durable::RemoteDone {
                player: actor.id,
                entity: actor.entity,
                sequence: 5,
            }
        )]
    );
}

#[test]
fn a_torch_on_the_side_of_a_block_across_the_line_goes_on_that_side() {
    let wall = BlockPos::new(15, 64, 8);
    let mut a = region_a(chunk_with(&[(wall, blocks::STONE)]));
    let mut b = region_b(stone_chunk());
    let (mut actor, _) = enter(&mut a, 1);
    let output = click_with(
        &mut a,
        &mut actor,
        Some(items::TORCH),
        click(wall, Face::East, false, 2),
    );
    let target = BlockPos::new(16, 64, 8);
    let expected = RemoteAction {
        player: actor.id,
        entity: actor.entity,
        sequence: 2,
        step: RemoteStep::PlaceAt {
            click: click_at_spawn(wall, Face::East, false, Some(items::TORCH)),
            target,
            clicked: blocks::STONE,
        },
    };
    assert_eq!(
        remotes(&output),
        vec![(E, expected.clone(), Some(REGION_B))]
    );
    assert!(changed(&output).is_empty());
    assert!(acknowledged(&output, actor.id).is_empty());

    // Region B does not hold the wall, and places against the state the step carries.
    assert!(b.chunk(CHUNK_A).is_none());
    let output = take(&mut b, expected);
    let wall_torch = state("minecraft:wall_torch[facing=east]");
    assert_eq!(changed(&output), vec![(target, wall_torch)]);
    assert_eq!(done(&output), vec![2]);
}

#[test]
fn a_placement_across_the_line_reads_the_clicked_block_from_the_step_and_not_as_air() {
    // The same step with air as the clicked block: no wall to hang on, and the torch
    // stands on the stone below instead. Region B reads the clicked block from the
    // step, whatever is there in fact.
    let mut b = region_b(stone_chunk());
    let wall = BlockPos::new(15, 64, 8);
    let target = BlockPos::new(16, 64, 8);
    let output = take(
        &mut b,
        RemoteAction {
            player: player(1),
            entity: EntityId(9001),
            sequence: 3,
            step: RemoteStep::PlaceAt {
                click: click_at_spawn(wall, Face::East, false, Some(items::TORCH)),
                target,
                clicked: blocks::AIR,
            },
        },
    );
    assert_eq!(changed(&output), vec![(target, blocks::TORCH)]);
    assert_eq!(done(&output), vec![3]);
}

#[test]
fn a_click_on_the_other_regions_block_whose_target_is_back_in_this_region_comes_back_as_a_place_at()
{
    let pillar = BlockPos::new(16, 64, 10);
    let mut a = region_a(stone_chunk());
    let mut b = region_b(chunk_with(&[(pillar, blocks::STONE)]));
    let (mut actor, _) = enter(&mut a, 1);
    let output = click_with(
        &mut a,
        &mut actor,
        Some(items::STONE),
        click(pillar, Face::West, false, 6),
    );
    let (answered, steps) = settle(&mut a, &mut b, &output);
    assert_eq!(answered, vec![6]);
    let the_click = click_at_spawn(pillar, Face::West, false, Some(items::STONE));
    assert_eq!(
        steps,
        vec![
            RemoteStep::UseOn {
                click: the_click.clone()
            },
            RemoteStep::PlaceAt {
                click: the_click,
                target: BlockPos::new(15, 64, 10),
                clicked: blocks::STONE,
            },
        ]
    );
    assert_eq!(block_at(&a, BlockPos::new(15, 64, 10)), Some(blocks::STONE));
    // Reading: region A applies the step's placement as a remote action of its own
    // player, answered with a `RemoteDone` (the action was passed on, so the
    // acknowledgement is the edge's to make), not acknowledged as its own input.
    assert_eq!(a.state().players[&actor.id].handled, None);
}

#[test]
fn a_door_across_the_line_opens_with_an_empty_hand_and_with_stone_in_hand() {
    let lower = BlockPos::new(16, 64, 10);
    let upper = lower.offset(0, 1, 0);
    let mut a = region_a(stone_chunk());
    let mut b = region_b(chunk_with(&[
        (lower, door("east", "lower", false)),
        (upper, door("east", "upper", false)),
    ]));
    let (mut actor, _) = enter(&mut a, 1);

    let output = click_with(&mut a, &mut actor, None, click(lower, Face::West, false, 1));
    let expected = RemoteAction {
        player: actor.id,
        entity: actor.entity,
        sequence: 1,
        step: RemoteStep::UseOn {
            click: click_at_spawn(lower, Face::West, false, None),
        },
    };
    assert_eq!(
        remotes(&output),
        vec![(E, expected.clone(), Some(REGION_B))]
    );
    assert!(acknowledged(&output, actor.id).is_empty());
    let output = take(&mut b, expected);
    let mut seen = changed(&output);
    seen.sort();
    assert_eq!(
        seen,
        vec![
            (lower, door("east", "lower", true)),
            (upper, door("east", "upper", true)),
        ]
    );
    assert_eq!(done(&output), vec![1]);

    // And shut again by a click with stone in hand, which places nothing.
    let output = click_with(
        &mut a,
        &mut actor,
        Some(items::STONE),
        click(upper, Face::West, false, 2),
    );
    let (answered, _) = settle(&mut a, &mut b, &output);
    assert_eq!(answered, vec![2]);
    assert_eq!(block_at(&b, lower), Some(door("east", "lower", false)));
    assert_eq!(block_at(&b, upper), Some(door("east", "upper", false)));
    assert_eq!(block_at(&a, BlockPos::new(15, 64, 10)), Some(blocks::AIR));
}

#[test]
fn two_regions_end_up_with_what_one_region_holding_both_chunks_does_with_the_same_clicks() {
    let wall = BlockPos::new(15, 64, 8);
    let pillar = BlockPos::new(16, 64, 10);
    let lower = BlockPos::new(16, 64, 11);
    let upper = lower.offset(0, 1, 0);
    let chunk_a = || chunk_with(&[(wall, blocks::STONE)]);
    let chunk_b = || {
        chunk_with(&[
            (pillar, blocks::STONE),
            (lower, door("east", "lower", false)),
            (upper, door("east", "upper", false)),
        ])
    };
    let clicks: Vec<(Option<i32>, PlayerInput)> = vec![
        (Some(items::TORCH), click(wall, Face::East, false, 1)),
        (
            Some(items::STONE),
            click(BlockPos::new(16, 63, 9), Face::Top, false, 2),
        ),
        (Some(items::STONE), click(pillar, Face::West, false, 3)),
        (None, click(lower, Face::West, false, 4)),
        (
            Some(items::OAK_FENCE),
            click(BlockPos::new(15, 63, 6), Face::Top, false, 5),
        ),
        (Some(items::OAK_LOG), click(pillar, Face::Top, false, 6)),
        (Some(items::STONE), click(upper, Face::West, true, 7)),
    ];

    let mut one = region_of_both(chunk_a(), chunk_b());
    let (mut alone, _) = enter(&mut one, 1);
    let mut a = region_a(chunk_a());
    let mut b = region_b(chunk_b());
    let (mut actor, _) = enter(&mut a, 1);

    for (item, input) in clicks {
        let PlayerInput::UseItemOn { sequence, .. } = input else {
            unreachable!("only clicks");
        };
        let output = click_with(&mut one, &mut alone, item, input.clone());
        assert_eq!(acknowledged(&output, alone.id), vec![sequence]);
        assert!(output.durable.is_empty());

        let output = click_with(&mut a, &mut actor, item, input);
        let (answered, _) = settle(&mut a, &mut b, &output);
        let mut answers = acknowledged(&output, actor.id);
        answers.extend(answered);
        assert_eq!(answers, vec![sequence], "each click is answered once");
    }

    for x in 0..32 {
        for y in 60..72 {
            for z in 0..16 {
                let position = BlockPos::new(x, y, z);
                let split = if x < 16 { &a } else { &b };
                assert_eq!(
                    block_at(split, position),
                    block_at(&one, position),
                    "at {position:?}"
                );
            }
        }
    }
    // Something did happen on both sides of the line.
    assert_eq!(
        block_at(&one, BlockPos::new(16, 64, 8)),
        Some(state("minecraft:wall_torch[facing=east]"))
    );
    assert_eq!(
        block_at(&one, BlockPos::new(15, 64, 10)),
        Some(blocks::STONE)
    );
}

// Section 4.6: the old steps.

fn old_step(step: RemoteStep, sequence: i32) -> RemoteAction {
    RemoteAction {
        player: player(1),
        entity: EntityId(9001),
        sequence,
        step,
    }
}

#[test]
fn an_old_place_against_puts_the_given_state_into_air_against_a_block() {
    let mut b = region_b(stone_chunk());
    // A state no rule would give for a click on top of stone: placed as given.
    let given =
        state("minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]");
    let output = take(
        &mut b,
        old_step(
            RemoteStep::PlaceAgainst {
                against: BlockPos::new(16, 63, 8),
                target: BlockPos::new(16, 64, 8),
                block: given,
                placer: Vec3::new(10.5, 64.0, 8.5),
            },
            1,
        ),
    );
    assert_eq!(changed(&output), vec![(BlockPos::new(16, 64, 8), given)]);
    assert_eq!(done(&output), vec![1]);

    // With nothing to place against: nothing.
    let output = take(
        &mut b,
        old_step(
            RemoteStep::PlaceAgainst {
                against: BlockPos::new(18, 70, 8),
                target: BlockPos::new(18, 71, 8),
                block: blocks::STONE,
                placer: Vec3::new(10.5, 64.0, 8.5),
            },
            2,
        ),
    );
    assert!(changed(&output).is_empty());
    assert_eq!(done(&output), vec![2]);
}

#[test]
fn an_old_place_puts_the_given_state_into_air_and_not_over_a_block() {
    let mut b = region_b(stone_chunk());
    let given = state("minecraft:oak_log[axis=z]");
    let output = take(
        &mut b,
        old_step(
            RemoteStep::Place {
                target: BlockPos::new(17, 64, 8),
                block: given,
                placer: Vec3::new(10.5, 64.0, 8.5),
            },
            1,
        ),
    );
    assert_eq!(changed(&output), vec![(BlockPos::new(17, 64, 8), given)]);
    assert_eq!(done(&output), vec![1]);

    let output = take(
        &mut b,
        old_step(
            RemoteStep::Place {
                target: BlockPos::new(17, 63, 8),
                block: given,
                placer: Vec3::new(10.5, 64.0, 8.5),
            },
            2,
        ),
    );
    assert!(changed(&output).is_empty());
    assert_eq!(block_at(&b, BlockPos::new(17, 63, 8)), Some(blocks::STONE));
    assert_eq!(done(&output), vec![2]);
}

// Section 5: buckets, the region's side.

fn use_item(yaw: f32, pitch: f32, sequence: i32) -> PlayerInput {
    PlayerInput::UseItem {
        rotation: (yaw, pitch),
        sneaking: false,
        sequence,
    }
}

#[test]
fn a_water_bucket_looking_at_the_floor_places_water_and_turns_the_player() {
    let mut region = region_a(stone_chunk());
    let (mut actor, pose) = enter(&mut region, 1);
    assert_eq!((pose.yaw, pose.pitch), (0.0, 0.0));
    // Looking west and down from (14.5, 65.62, 8.5): the ray meets the floor's top at
    // x = 13.57. With the player's own rotation (ahead, level) it would meet nothing.
    let mut inputs = hold(Some(items::WATER_BUCKET));
    inputs.push(use_item(90.0, 60.0, 3));
    let output = act(&mut region, &mut actor, inputs);
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(13, 64, 8), blocks::WATER)]
    );
    assert_eq!(acknowledged(&output, actor.id), vec![3]);
    let (_, now) = region.player(actor.id).expect("the player is in");
    assert_eq!((now.yaw, now.pitch), (90.0, 60.0));
    assert_eq!(now.position, SPAWN_A);
    // In creative the bucket stays as it is.
    assert_eq!(
        region.state().players[&actor.id].inventory.0[0],
        stack(items::WATER_BUCKET)
    );
}

#[test]
fn an_empty_bucket_takes_a_source_and_the_filled_bucket_goes_into_the_hotbar() {
    let source = BlockPos::new(13, 64, 8);
    let mut region = region_a(chunk_with(&[(source, blocks::WATER)]));
    let (mut actor, _) = enter(&mut region, 1);
    let mut inputs = hold(Some(items::BUCKET));
    inputs.push(use_item(90.0, 60.0, 1));
    let output = act(&mut region, &mut actor, inputs);
    assert_eq!(changed(&output), vec![(source, blocks::AIR)]);
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
    let hotbar = region.state().players[&actor.id].inventory.hotbar();
    assert_eq!(hotbar[0], stack(items::BUCKET), "the empty bucket stays");
    // Reading: the starting hotbar's stone was replaced in slot 0, so slot 1 is the
    // first empty one; the count of the filled bucket put there is taken to be one.
    assert_eq!(hotbar[1], stack(items::WATER_BUCKET));
}

#[test]
fn a_bucket_that_reaches_nothing_changes_nothing_and_is_acknowledged() {
    let mut region = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut region, 1);
    let mut inputs = hold(Some(items::WATER_BUCKET));
    inputs.push(use_item(0.0, -30.0, 1));
    let output = act(&mut region, &mut actor, inputs);
    assert!(changed(&output).is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
    let (_, now) = region.player(actor.id).expect("the player is in");
    assert_eq!((now.yaw, now.pitch), (0.0, -30.0));
}

#[test]
fn a_bucket_whose_ray_reaches_the_other_regions_chunk_does_nothing() {
    let mut region = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut region, 1);
    // Looking east and down from (14.5, 65.62, 8.5): the floor's top at x = 16.43, in
    // region B's chunk, which the ray stops at (section 5).
    let mut inputs = hold(Some(items::WATER_BUCKET));
    inputs.push(use_item(-90.0, 40.0, 1));
    let output = act(&mut region, &mut actor, inputs);
    assert!(changed(&output).is_empty());
    assert!(output.durable.is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);

    // A little steeper, it meets region A's own floor.
    let mut inputs = hold(Some(items::WATER_BUCKET));
    inputs.push(use_item(-90.0, 60.0, 2));
    let output = act(&mut region, &mut actor, inputs);
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(15, 64, 8), blocks::WATER)]
    );
}

#[test]
fn a_lava_bucket_places_lava_and_an_empty_bucket_takes_it_back() {
    let mut region = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut region, 1);
    let mut inputs = hold(Some(items::LAVA_BUCKET));
    inputs.push(use_item(90.0, 60.0, 1));
    let output = act(&mut region, &mut actor, inputs);
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(13, 64, 8), blocks::LAVA)]
    );
    let mut inputs = hold(Some(items::BUCKET));
    inputs.push(use_item(90.0, 60.0, 2));
    let output = act(&mut region, &mut actor, inputs);
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(13, 64, 8), blocks::AIR)]
    );
    assert_eq!(
        region.state().players[&actor.id].inventory.0[1],
        stack(items::LAVA_BUCKET)
    );
}

// More of 4.3 and 4.5: what a region reads, and whom it does not build into.

#[test]
fn a_remote_click_on_a_block_of_a_held_chunk_not_loaded_places_nothing_and_is_done() {
    // Region B holds its chunk but storage has not delivered it. Read as air, the
    // clicked block would be replaced (4.1); 4.3 says nothing is placed instead.
    let mut b = Region::new(
        config(Vec3::new(24.5, 64.0, 8.5)),
        ids_b(),
        holdings(vec![CHUNK_B], STRIPE_B),
    );
    tick(
        &mut b,
        &TickInputs {
            edges: started(),
            ..TickInputs::default()
        },
    );
    assert!(b.chunk(CHUNK_B).is_none());
    let clicked = BlockPos::new(16, 63, 8);
    let output = take(
        &mut b,
        old_step(
            RemoteStep::UseOn {
                click: click_at_spawn(clicked, Face::Top, false, Some(items::STONE)),
            },
            4,
        ),
    );
    assert!(changed(&output).is_empty());
    assert_eq!(done(&output), vec![4]);
    assert!(remotes(&output).is_empty());
}

#[test]
fn a_region_that_has_the_clicked_block_loaded_reads_it_rather_than_the_steps_state() {
    // One region holds both sides, as after a merge, and is given a step made before
    // it: the wall is there, though the step says air, and what it has wins (4.3).
    let wall = BlockPos::new(15, 64, 8);
    let mut one = region_of_both(chunk_with(&[(wall, blocks::STONE)]), stone_chunk());
    let target = BlockPos::new(16, 64, 8);
    let output = take(
        &mut one,
        old_step(
            RemoteStep::PlaceAt {
                click: click_at_spawn(wall, Face::East, false, Some(items::TORCH)),
                target,
                clicked: blocks::AIR,
            },
            5,
        ),
    );
    assert_eq!(
        changed(&output),
        vec![(target, state("minecraft:wall_torch[facing=east]"))]
    );
    assert_eq!(done(&output), vec![5]);
}

#[test]
fn a_bed_whose_head_would_be_in_the_other_regions_chunk_is_not_placed_at_all() {
    let mut a = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut a, 1);
    // Facing east, the head goes one block east of the foot, into region B.
    act(
        &mut a,
        &mut actor,
        vec![PlayerInput::Move {
            position: None,
            rotation: Some((-90.0, 0.0)),
            on_ground: true,
        }],
    );
    let output = click_with(
        &mut a,
        &mut actor,
        Some(items::RED_BED),
        click(BlockPos::new(15, 63, 8), Face::Top, false, 1),
    );
    assert!(changed(&output).is_empty(), "all or nothing (4.3)");
    assert_eq!(block_at(&a, BlockPos::new(15, 64, 8)), Some(blocks::AIR));
    // Reading: the target is region A's own, so A judges the placement and nothing is
    // passed on; the click is acknowledged as one of A's.
    assert!(output.durable.is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);

    // Facing north, the same bed beside it stays in region A and goes in.
    act(
        &mut a,
        &mut actor,
        vec![PlayerInput::Move {
            position: None,
            rotation: Some((180.0, 0.0)),
            on_ground: true,
        }],
    );
    let output = act(
        &mut a,
        &mut actor,
        vec![click(BlockPos::new(15, 63, 6), Face::Top, false, 2)],
    );
    assert_eq!(changed(&output).len(), 2);
    let head = block_at(&a, BlockPos::new(15, 64, 5)).expect("loaded");
    assert_eq!(head.property("part"), Some("head"));
}

#[test]
fn a_click_out_of_reach_on_the_other_regions_block_is_acknowledged_and_sends_nothing() {
    let mut a = region_a(stone_chunk());
    let (mut actor, _) = enter(&mut a, 1);
    let output = click_with(
        &mut a,
        &mut actor,
        None,
        click(BlockPos::new(29, 63, 8), Face::Top, false, 1),
    );
    assert!(output.durable.is_empty());
    assert_eq!(acknowledged(&output, actor.id), vec![1]);
}

#[test]
fn a_remote_placement_is_not_made_into_a_player_of_the_region_that_places() {
    let mut b = region_b(stone_chunk());
    let (_, pose) = enter(&mut b, 2);
    assert_eq!(pose.position, Vec3::new(24.5, 64.0, 8.5));
    let under_them = BlockPos::new(24, 63, 8);
    let output = take(
        &mut b,
        old_step(
            RemoteStep::UseOn {
                click: Click {
                    feet: Vec3::new(21.5, 64.0, 8.5),
                    ..click_at_spawn(under_them, Face::Top, false, Some(items::STONE))
                },
            },
            6,
        ),
    );
    assert!(changed(&output).is_empty(), "the player stands there (4.3)");
    assert_eq!(done(&output), vec![6]);

    // Beside them it goes in.
    let beside = BlockPos::new(22, 63, 8);
    let output = take(
        &mut b,
        old_step(
            RemoteStep::UseOn {
                click: Click {
                    feet: Vec3::new(21.5, 64.0, 6.5),
                    ..click_at_spawn(beside, Face::Top, false, Some(items::STONE))
                },
            },
            7,
        ),
    );
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(22, 64, 8), blocks::STONE)]
    );
    assert_eq!(done(&output), vec![7]);
}

#[test]
fn a_placement_is_not_made_into_another_player_of_the_region() {
    let mut region = region_a(stone_chunk());
    let (mut builder, _) = enter(&mut region, 1);
    act(
        &mut region,
        &mut builder,
        vec![PlayerInput::Move {
            position: Some(Vec3::new(12.5, 64.0, 8.5)),
            rotation: None,
            on_ground: true,
        }],
    );
    let (_, pose) = enter(&mut region, 2);
    assert_eq!(pose.position, SPAWN_A);
    let output = act(
        &mut region,
        &mut builder,
        vec![click(SPAWN_COLUMN.offset(0, -1, 0), Face::Top, false, 1)],
    );
    assert!(changed(&output).is_empty());
    assert_eq!(acknowledged(&output, builder.id), vec![1]);

    // Between the two players there is room.
    let output = act(
        &mut region,
        &mut builder,
        vec![click(BlockPos::new(13, 63, 8), Face::Top, false, 2)],
    );
    assert_eq!(
        changed(&output),
        vec![(BlockPos::new(13, 64, 8), blocks::STONE)]
    );
}
