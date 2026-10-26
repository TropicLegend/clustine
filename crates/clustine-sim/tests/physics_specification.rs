//! Tests of step N4's physics, items and falling blocks, written from
//! `docs/groundwork/n4-plan.md` (all of it, section 7 over the rest), the porting brief
//! `docs/groundwork/n4-brief.md` (the game's code read from the 26.3 jar) and
//! `docs/adr/0028-entities.md`, driving the region by its public interface alone, by
//! someone who did not write the code.
//!
//! The numbers each test expects are worked out here, by a small model of the game's
//! `Entity.move`, `ItemEntity.tick` and `FallingBlockEntity.tick` over a flat ground
//! (the brief's sections 1 to 4 and 7, step for step as the bytecode has them, in the
//! game's `float`s and `double`s), never by calling the code under test. An item or a
//! falling block is "summoned" as the plan's oracle does it: laid in a chunk that the
//! region loads, which takes it in and ticks it from the tick of the load.
//!
//! Player 1 stands at (0.5, -60, 0.5) on a stone floor whose top is y = -60, with the
//! 5×5 chunks around chunk (0, 0) loaded, so that chunk (0, 0) ticks. Items lie at x
//! and z of 4 or more, out of the player's reach, unless a test wants them picked up.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_sim::api::{
    HOTBAR_SLOTS, Inventory, ItemStack, OFF_HAND, PlayerInput, RegionEvent, Tracked,
};
use clustine_sim::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerEvent, PlayerJoin, Region, RegionConfig,
    RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, EdgeId, Entity, EntityChange, EntityData,
    EntityField, EntityId, EntityIds, PlayerId, Section, Vec3,
};
use uuid::Uuid;

/// The game's `ItemEntity.age` of an item's record: the ticks it has lived less what a
/// merge with a younger item took off.
trait Aged {
    fn age(&self) -> u32;
}

impl Aged for Entity {
    fn age(&self) -> u32 {
        match self.data {
            EntityData::Item { age_offset, .. } => self.lived - age_offset,
            _ => self.lived,
        }
    }
}

const EDGE: EdgeId = EdgeId(7);
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const FEET: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// A block set in the origin's chunk, at its world coordinates.
type Placed = (i32, i32, i32, BlockState);

/// What a player holds, slot by slot.
type Holding = Vec<(usize, ItemStack)>;

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

// ------------------------------------------------------------------------------------
// The game's numbers (the brief's sections 1 to 4).

/// `getDefaultGravity()` of items and falling blocks, a `double`.
const GRAVITY: f64 = 0.04;
/// `1.0E-5F` widened: `Mth.equal`'s tolerance and an item's "slow" bound.
const TOLERANCE: f64 = 9.999_999_747_378_752e-6;

/// `getAirDrag()`, `0.98f` widened.
fn drag() -> f64 {
    f64::from(0.98_f32)
}

/// An item's ground friction over a block of `friction`: the float product
/// `0.98f * friction`, widened.
fn on_ground_drag(friction: f32) -> f64 {
    f64::from(0.98_f32 * friction)
}

/// `Block.getFriction()` of the blocks these tests use (the brief's table in 1.8).
const STONE_FRICTION: f32 = 0.6;
const ICE_FRICTION: f32 = 0.98;
const BLUE_ICE_FRICTION: f32 = 0.989;
const SLIME_FRICTION: f32 = 0.8;

// ------------------------------------------------------------------------------------
// The world.

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack { item, count }
}

fn config_holding(inventory: Inventory) -> RegionConfig {
    RegionConfig {
        spawn: FEET,
        starting_inventory: inventory,
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        light: Default::default(),
        features: Default::default(),
    }
}

/// 64 stone in slot 0, which is selected.
fn stone_in_hand() -> Inventory {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(stack(items::STONE, 64));
    Inventory::with_hotbar(hotbar)
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

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

/// The floor with `placed` set in it, at world coordinates within chunk (0, 0).
fn floor_with(placed: &[Placed]) -> Chunk {
    let mut chunk = floor();
    for &(x, y, z, block) in placed {
        let x = usize::try_from(x).expect("in chunk (0, 0)");
        let z = usize::try_from(z).expect("in chunk (0, 0)");
        chunk.set(x, y, z, block);
    }
    chunk
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region pinned to `area` that knows the edge and holds the chunks of `area` near
/// the origin.
fn region_in(area: ChunkArea, inventory: Inventory) -> Region {
    let mut state = RegionState::new(EntityIds::block(1).expect("block 1"));
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
    Region::restore(config_holding(inventory), state, holdings)
}

/// Player 1 joins and enters the world at the spawn.
fn joined(region: &mut Region) {
    region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(1),
                name: "Watcher".to_owned(),
                attempt: 7001,
            },
        )],
        ..TickInputs::default()
    });
    let entered = region
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
        .collect();
    region.tick(&TickInputs {
        entered,
        ..TickInputs::default()
    });
    assert!(region.player(player(1)).is_some(), "player 1 entered");
}

/// Loads the chunks of `area` near the origin for a viewer, each a floor but the
/// origin's, which is `origin`, and hands in `copies` as border copies with them.
fn load_in(
    region: &mut Region,
    area: ChunkArea,
    origin: Chunk,
    copies: Vec<(ChunkPos, Chunk)>,
) -> TickOutput {
    let chunks: Vec<ChunkPos> = around(2)
        .into_iter()
        .filter(|chunk| area.contains(*chunk))
        .collect();
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
                origin.clone()
            } else {
                floor()
            };
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: loaded,
        copy_bases: copies,
        ..TickInputs::default()
    })
}

/// A region over the whole world, player 1 in it, and the origin's chunk `origin`
/// loaded with its neighbours: the region and the output of the tick of the load, in
/// which what `origin` holds is taken in and ticks for the first time.
fn scene_holding(origin: Chunk, inventory: Inventory) -> (Region, TickOutput) {
    let mut region = region_in(ChunkArea::EVERYWHERE, inventory);
    joined(&mut region);
    let output = load_in(&mut region, ChunkArea::EVERYWHERE, origin, Vec::new());
    (region, output)
}

fn scene(placed: &[Placed], entities: Vec<Entity>) -> (Region, TickOutput) {
    let mut origin = floor_with(placed);
    origin.set_entities(entities);
    scene_holding(origin, stone_in_hand())
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// The key of item number `n`, whose game id (`getId()`) is `n`.
fn key(n: u32) -> EntityId {
    EntityId((9 << 32) + u64::from(n))
}

/// An item of one dirt at `position` with `motion`, as a `/summon` would make it: age
/// 1 (not its very first tick), health 5, never to be picked up unless a test says so.
fn item(n: u32, position: [f64; 3], motion: [f64; 3], on_ground: bool) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(entity_types::ITEM).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Item {
            stack: stack(items::DIRT, 1),
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

fn item_of(n: u32, position: [f64; 3], of: ItemStack, delay: u16, age: u32) -> Entity {
    let mut entity = item(n, position, [0.0; 3], true);
    entity.lived = age;
    entity.data = EntityData::Item {
        stack: of,
        pick_up_delay: delay,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    entity
}

/// A falling block of `block` at `position`, at rest, as `/summon falling_block` makes
/// one.
fn falling(n: u32, position: [f64; 3], block: BlockState) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(entity_types::FALLING_BLOCK).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 8_000),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::FallingBlock {
            state: block.0,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// Every record of the entity `key` in the region's loaded chunks, with its chunk.
fn records(region: &Region, key: EntityId) -> Vec<(ChunkPos, Entity)> {
    let mut found = Vec::new();
    for chunk in region.loaded_chunks() {
        let entities = region.chunk(chunk).expect("loaded").entities();
        for entity in entities.iter().filter(|entity| entity.key == key) {
            found.push((chunk, entity.clone()));
        }
    }
    found
}

/// The one record of `key`, if there is one; two are a defect.
fn record(region: &Region, key: EntityId) -> Option<Entity> {
    let found = records(region, key);
    assert!(found.len() <= 1, "{key:?} is there twice: {found:?}");
    found.into_iter().next().map(|(_, entity)| entity)
}

fn block_at(region: &Region, x: i32, y: i32, z: i32) -> BlockState {
    let at = BlockPos::new(x, y, z);
    let (cx, cz) = at.in_chunk();
    region
        .chunk(at.chunk())
        .and_then(|chunk| chunk.get(cx, y, cz))
        .expect("a loaded block")
}

fn data_of(entity: &Entity) -> (ItemStack, u16, i16) {
    let EntityData::Item {
        stack,
        pick_up_delay,
        health,
        ..
    } = entity.data
    else {
        panic!("{entity:?} is not an item");
    };
    (stack, pick_up_delay, health)
}

fn gone_in(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

fn tracked_in(output: &TickOutput, key: EntityId) -> Option<Tracked> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::EntityTracked(tracked) if tracked.entity == key => Some(tracked.clone()),
        _ => None,
    })
}

/// The entities spawned in `output`, as their spawn showed them.
fn spawned(output: &TickOutput) -> Vec<clustine_sim::api::EntityState> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state)
                if matches!(state.kind, clustine_sim::api::EntityKind::Other { .. }) =>
            {
                Some(state.clone())
            }
            _ => None,
        })
        .collect()
}

/// The stack, pick-up delay and motion a spawn shows.
fn spawn_parts(state: &clustine_sim::api::EntityState) -> (EntityData, [f64; 3]) {
    let clustine_sim::api::EntityKind::Other { data, motion, .. } = &state.kind else {
        panic!("{state:?} is a player");
    };
    (data.clone(), *motion)
}

// ------------------------------------------------------------------------------------
// The model: the game's item and falling block over a flat ground.

/// What the model keeps of an entity: what its record has.
#[derive(Debug, Clone, Copy)]
struct Hand {
    p: [f64; 3],
    v: [f64; 3],
    on_ground: bool,
    fall: f64,
}

impl Hand {
    fn of(entity: &Entity) -> Self {
        Self {
            p: entity.position,
            v: entity.motion,
            on_ground: entity.on_ground,
            fall: entity.fall_distance,
        }
    }
}

/// A flat ground under the entity: the top of the block it lies on, everywhere around,
/// and what the block under the entity's feet by the game's rules is like.
#[derive(Debug, Clone, Copy)]
struct Ground {
    top: f64,
    /// The friction of the block at `getOnPos(0.999999f)`: the block below the top,
    /// or below a slab (the brief's 1.5).
    friction: f32,
    /// `getBounceRestitution()` of the block at `getOnPosLegacy()`.
    bounce: f32,
    slime: bool,
}

const STONE_GROUND: Ground = Ground {
    top: FLOOR,
    friction: STONE_FRICTION,
    bounce: 0.0,
    slime: false,
};

/// What a move did that the edge is told.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Step {
    moved: bool,
    bounced: bool,
    horizontal: bool,
    vertical: bool,
    /// The motion changed by more than 0.1 in the tick (`needsSync`).
    sync: bool,
}

/// `Entity.move(SELF, v)` and its `collide` over the ground, for a box `height` high
/// (the brief's 1.2, 1.3, 1.6 and 1.7).
fn entity_move(hand: &mut Hand, ground: &Ground, height: f64, in_water: bool) -> Step {
    let vec = hand.v;
    let y = hand.p[1];
    // The ground's blocks are among the colliders where the box swept by the move
    // overlaps them (`BlockCollisions` over `bb.expandTowards(vec)`).
    let present =
        y + vec[1].min(0.0) < ground.top && y + height + vec[1].max(0.0) > ground.top - 1.0;
    let mut out = vec;
    if present {
        // `Shapes.collide`: a component under 1e-7 is nothing; a fall is cut where it
        // would go into the top (`VoxelShape.collide`, `max(d, top - minY)`).
        if vec[1] != 0.0 {
            out[1] = if vec[1].abs() < 1e-7 {
                0.0
            } else if vec[1] < 0.0 {
                let e = ground.top - y;
                if e <= 1e-7 { vec[1].max(e) } else { vec[1] }
            } else {
                vec[1]
            };
        }
        for axis in [0, 2] {
            if vec[axis] != 0.0 && vec[axis].abs() < 1e-7 {
                out[axis] = 0.0;
            }
        }
    }
    let length = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let d5 = length(out);
    let accepted = d5 > 1e-7 || length(vec) - d5 < 1e-7;
    if accepted {
        assert!(
            hand.fall == 0.0 || d5 < 1.0,
            "the model leaves out the fall's clip"
        );
        hand.p = [hand.p[0] + out[0], hand.p[1] + out[1], hand.p[2] + out[2]];
    }
    let x_collision = (out[0] - vec[0]).abs() >= TOLERANCE;
    let z_collision = (out[2] - vec[2]).abs() >= TOLERANCE;
    let horizontal = x_collision || z_collision;
    let vertical = vec[1] != out[1];
    let below = vertical && vec[1] < 0.0;
    hand.on_ground = below;
    if !in_water && out[1] < 0.0 {
        hand.fall -= f64::from(out[1] as f32);
    }
    if hand.on_ground {
        hand.fall = 0.0;
    }
    let mut bounced = false;
    if (vec[1].abs() > 0.0 && vertical) || horizontal {
        let entity = 0.0_f64;
        let mut w = vec;
        if x_collision {
            w[0] = -vec[0] * entity;
        }
        if z_collision {
            w[2] = -vec[2] * entity;
        }
        if vertical {
            let mut d = entity;
            if below {
                let block = f64::from(ground.bounce * 0.8_f32);
                d = if -vec[1] <= GRAVITY {
                    0.0
                } else {
                    entity.max(block)
                };
            }
            let (a, b) = if d > 0.0 {
                bounced = true;
                let r = out[1] / vec[1];
                (r * GRAVITY, 1.0 + r * (drag() - 1.0))
            } else {
                (0.0, 1.0)
            };
            w[1] = (a - vec[1]) * b * d;
        }
        hand.v = w;
    }
    Step {
        moved: accepted,
        bounced,
        horizontal,
        vertical,
        sync: false,
    }
}

/// `SlimeBlock.stepOn`, for an item on the ground.
fn step_on(hand: &mut Hand, ground: &Ground) {
    if hand.on_ground && ground.slime {
        let up = hand.v[1].abs();
        if up < 0.1 {
            let k = 0.4 + up * 0.2;
            hand.v[0] *= k;
            hand.v[2] *= k;
        }
    }
}

/// Whether an item moves this tick: in the air, fast, or on its fourth tick by
/// `(tickCount + getId()) % 4`, `tickCount` being the region's tick (the plan, D3).
fn moves_now(hand: &Hand, tick: u64, id: i32) -> bool {
    let slow = hand.v[0] * hand.v[0] + hand.v[2] * hand.v[2] <= TOLERANCE;
    let gate = (tick as i32).wrapping_add(id) % 4 == 0;
    !hand.on_ground || !slow || gate
}

/// `ItemEntity.tick` of an item out of any fluid over the ground (the brief's 3.1).
fn item_tick(hand: &mut Hand, tick: u64, id: i32, ground: &Ground) -> Step {
    let start = hand.v;
    let mut step = item_tick_moving(hand, tick, id, ground);
    let change: f64 = (0..3)
        .map(|axis| (hand.v[axis] - start[axis]).powi(2))
        .sum();
    step.sync = change > 0.01;
    step
}

fn item_tick_moving(hand: &mut Hand, tick: u64, id: i32, ground: &Ground) -> Step {
    hand.v[1] += -GRAVITY;
    if moves_now(hand, tick, id) {
        let step = entity_move(hand, ground, 0.25, false);
        step_on(hand, ground);
        let f3 = if hand.on_ground {
            ground_friction(ground)
        } else {
            drag()
        };
        hand.v = [hand.v[0] * f3, hand.v[1] * drag(), hand.v[2] * f3];
        if hand.on_ground && hand.v[1] < 0.0 {
            hand.v[1] *= -0.5;
        }
        step
    } else {
        // The effects of the blocks of its last moves, `stepOn` among them.
        step_on(hand, ground);
        Step::default()
    }
}

fn ground_friction(ground: &Ground) -> f64 {
    on_ground_drag(ground.friction)
}

/// `FallingBlockEntity.tick` while it falls over the ground; true once it has landed.
fn falling_tick(hand: &mut Hand, ground: &Ground) -> bool {
    hand.v[1] += -GRAVITY;
    entity_move(hand, ground, f64::from(0.98_f32), false);
    if hand.on_ground {
        return true;
    }
    hand.v = [hand.v[0] * drag(), hand.v[1] * drag(), hand.v[2] * drag()];
    false
}

/// The record matches the model to the bit.
fn assert_is(entity: &Entity, hand: &Hand, what: &str) {
    let bits = |v: [f64; 3]| v.map(f64::to_bits);
    assert_eq!(
        bits(entity.position),
        bits(hand.p),
        "{what}: position {:?}, the game's {:?}",
        entity.position,
        hand.p
    );
    assert_eq!(
        bits(entity.motion),
        bits(hand.v),
        "{what}: motion {:?}, the game's {:?}",
        entity.motion,
        hand.v
    );
    assert_eq!(entity.on_ground, hand.on_ground, "{what}: on the ground");
    assert_eq!(
        entity.fall_distance.to_bits(),
        hand.fall.to_bits(),
        "{what}: fall distance {}, the game's {}",
        entity.fall_distance,
        hand.fall
    );
}

/// Ticks `region` `ticks` times from the load's `output`, the item `n` against the
/// model over `ground` each tick, the load's tick the first; returns the model's steps
/// and the outputs.
fn follow(
    region: &mut Region,
    load: TickOutput,
    n: u32,
    start: &Entity,
    ground: &Ground,
    ticks: usize,
) -> (Hand, Vec<(Step, TickOutput)>) {
    let mut hand = Hand::of(start);
    let mut seen = Vec::new();
    let mut output = load;
    for count in 0..ticks {
        let step = item_tick(&mut hand, output.tick, n as i32, ground);
        let now = record(region, key(n)).unwrap_or_else(|| panic!("tick {}: gone", output.tick));
        assert_is(
            &now,
            &hand,
            &format!("tick {} ({count} since the load)", output.tick),
        );
        seen.push((step, output));
        output = idle(region);
    }
    (hand, seen)
}

// ------------------------------------------------------------------------------------
// Falling and lying.

/// The brief, sections 1 and 3: an item let go at rest 3 blocks above stone falls by
/// gravity and the air's drag, its fall cut at the floor's top, where it lands; its
/// fall distance grows by the float of each move down and is cleared on landing; then
/// it lies still, its motion cycling 0, -0.04, -0.08, ... as gravity adds up on the
/// ticks it does not move, and is cut to nothing on its fourth ticks, by its game id.
#[test]
fn an_item_dropped_onto_stone_falls_and_lies_where_the_game_puts_it_tick_after_tick() {
    let start = item(3, [8.5, -57.0, 8.5], [0.0; 3], false);
    let (mut region, load) = scene(&[], vec![start.clone()]);
    let (hand, steps) = follow(&mut region, load, 3, &start, &STONE_GROUND, 60);
    assert_eq!(hand.p, [8.5, FLOOR, 8.5]);
    assert!(hand.on_ground);
    // The model landed it within the ticks a fall of three blocks takes.
    let landed = steps
        .iter()
        .position(|(step, _)| step.vertical)
        .expect("it lands");
    assert_eq!(landed, 12, "the model's landing tick, the load's the first");
    // It lies still from then on: no moves, and its position commits nothing.
    for (step, output) in &steps[landed + 1..] {
        assert!(
            !output
                .entity_changes
                .iter()
                .any(|(_, change)| matches!(change,
                EntityChange::Set(at, fields) if *at == key(3)
                    && fields.iter().any(|field| matches!(field, EntityField::Position(_))))),
            "tick {}: {:?}",
            output.tick,
            output.entity_changes
        );
        assert!(!step.bounced);
        // It is reported to the edge only on its fourth ticks, whose move, refused,
        // cuts the motion gravity added (-0.12 at the tick's start) to nothing: by more
        // than 0.1, which asks for a sync (`needsSync`) as the game's does (the plan,
        // section 7, G2).
        let told = tracked_in(output, key(3));
        assert_eq!(told.is_some(), step.sync, "tick {}: {told:?}", output.tick);
        if let Some(told) = told {
            assert_eq!(
                told.flags,
                Tracked::VERTICAL_COLLISION | Tracked::NEEDS_SYNC
            );
            assert_eq!(told.position, [8.5, FLOOR, 8.5]);
        }
    }
}

/// The brief, 1.5: an item lying on a bottom slab rests at its top, y -59.5, and takes
/// its friction from the block under the slab (`getOnPos(0.999999f)`), so that over
/// ice beneath the slab it slides as on ice, farther than over stone.
#[test]
fn an_item_on_a_bottom_slab_lies_at_its_top_and_slides_by_the_friction_of_the_block_beneath() {
    let slab = state("minecraft:stone_slab[type=bottom]");
    let placed = [(8, -60, 8, slab), (8, -61, 8, blocks::ICE)];
    let start = item(5, [8.3, -58.0, 8.5], [0.005, 0.0, 0.0], false);
    let (mut region, load) = scene(&placed, vec![start.clone()]);
    let on_slab = Ground {
        top: -59.5,
        friction: ICE_FRICTION,
        bounce: 0.0,
        slime: false,
    };
    let (hand, _) = follow(&mut region, load, 5, &start, &on_slab, 120);
    assert_eq!(hand.p[1], -59.5);
    assert!(hand.p[0] + 0.125 < 9.0, "the model kept it over the slab");
    // Over stone it would have come to rest nearer.
    let mut over_stone = Hand::of(&start);
    let stone = Ground {
        friction: STONE_FRICTION,
        ..on_slab
    };
    for tick in 4..124 {
        item_tick(&mut over_stone, tick, 5, &stone);
    }
    assert!(over_stone.p[0] < hand.p[0], "{over_stone:?} {hand:?}");
}

/// The brief, 1.8 and 3.1: an item set sliding on ice moves every tick while its
/// horizontal speed is above the game's bound, slowed by `0.98f * 0.98f`; blue ice by
/// `0.98f * 0.989f`; once slow, only on its fourth ticks, and a move cut to under 1e-7
/// squared by nothing at all is refused: it stops.
#[test]
fn an_item_slides_on_ice_and_blue_ice_by_the_game_s_friction_tick_after_tick() {
    for (block, friction) in [
        (blocks::ICE, ICE_FRICTION),
        (blocks::BLUE_ICE, BLUE_ICE_FRICTION),
    ] {
        let placed: Vec<Placed> = (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, -61, z, block)))
            .collect();
        let start = item(6, [4.5, FLOOR, 8.5], [0.3, 0.0, 0.0], true);
        let (mut region, load) = scene(&placed, vec![start.clone()]);
        let ice = Ground {
            friction,
            ..STONE_GROUND
        };
        let (hand, _) = follow(&mut region, load, 6, &start, &ice, 400);
        assert!(hand.p[0] > 11.0 && hand.p[0] < 16.0, "{hand:?}");
    }
}

/// The brief, 1.7 and 1.9: an item that falls onto a slime block bounces, by
/// `(r·g − v) · lerp(r, 1, drag) · 0.8f` where `r` is the share of the move made, and
/// the bounce asks the edge for a position update (`syncPosition`); on the block it is
/// slowed sideways by `stepOn`, and its fourth-tick moves, gravity having added up,
/// bounce it again and again.
#[test]
fn an_item_bounces_on_slime_by_the_game_s_restitution_and_asks_for_its_position_to_be_sent() {
    let placed = [(8, -61, 8, blocks::SLIME_BLOCK)];
    let start = item(7, [8.5, -57.0, 8.5], [0.0; 3], false);
    let (mut region, load) = scene(&placed, vec![start.clone()]);
    let slime = Ground {
        top: FLOOR,
        friction: SLIME_FRICTION,
        bounce: 1.0,
        slime: true,
    };
    let (_, steps) = follow(&mut region, load, 7, &start, &slime, 150);
    let bounces: Vec<u64> = steps
        .iter()
        .filter(|(step, _)| step.bounced)
        .map(|(_, output)| output.tick)
        .collect();
    assert!(bounces.len() >= 3, "{bounces:?}");
    for (step, output) in &steps {
        let flags = tracked_in(output, key(7)).map_or(0, |tracked| tracked.flags);
        assert_eq!(
            flags & Tracked::SYNC_POSITION != 0,
            step.bounced,
            "tick {}: flags {flags:#b}",
            output.tick
        );
    }
}

/// The plan, section 1 and its review (G1, copies aside): an item lying on stone is
/// never moved by the ticks it does not move on, and lies where it fell through a
/// thousand ticks of its fourth-tick moves.
#[test]
fn an_item_at_rest_on_stone_stays_put_through_a_thousand_ticks() {
    let start = item(8, [10.25, FLOOR, 11.75], [0.0; 3], true);
    let (mut region, load) = scene(&[], vec![start.clone()]);
    let (hand, _) = follow(&mut region, load, 8, &start, &STONE_GROUND, 1000);
    assert_eq!(hand.p, start.position);
}

// ------------------------------------------------------------------------------------
// Water and lava (the brief's section 4).

/// A fluid of the test's own: the amount of water in a block, if any (8 a source).
type Water = fn(i32, i32, i32) -> Option<u8>;

/// `FlowingFluid.getFlow` of water that is not falling, from its neighbours' amounts,
/// for blocks with no fluid below their neighbours (the floor is stone).
fn flow(water: Water, x: i32, y: i32, z: i32) -> [f64; 3] {
    let Some(own) = water(x, y, z) else {
        return [0.0; 3];
    };
    let own = f32::from(own) / 9.0;
    let (mut dx, mut dz) = (0.0_f64, 0.0_f64);
    // NORTH, EAST, SOUTH, WEST.
    for (sx, sz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let h = water(x + sx, y, z + sz).map_or(0.0, |amount| f32::from(amount) / 9.0);
        let diff = if h == 0.0 { 0.0 } else { own - h };
        if diff != 0.0 {
            dx += f64::from(sx as f32 * diff);
            dz += f64::from(sz as f32 * diff);
        }
    }
    normalise([dx, 0.0, dz])
}

fn normalise(v: [f64; 3]) -> [f64; 3] {
    let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if d < TOLERANCE {
        [0.0; 3]
    } else {
        [v[0] / d, v[1] / d, v[2] / d]
    }
}

/// `EntityFluidInteraction.update` for water of one type and its current's
/// `applyTo(entity, 0.014)`: the height the item is in water to, and its motion pushed.
fn in_water(hand: &mut Hand, water: Water) -> f64 {
    let (x, y, z) = (hand.p[0], hand.p[1], hand.p[2]);
    let (min_x, min_y, min_z) = (x - 0.125 + 0.001, y + 0.001, z - 0.125 + 0.001);
    let (max_x, max_y, max_z) = (x + 0.125 - 0.001, y + 0.25 - 0.001, z + 0.125 - 0.001);
    let mut height = 0.0_f64;
    let mut current_height = 0.0_f64;
    let mut current = [0.0_f64; 3];
    let mut count = 0;
    for bx in (min_x.floor() as i32)..=(max_x.ceil() as i32 - 1) {
        for by in (min_y.floor() as i32)..=(max_y.ceil() as i32 - 1) {
            for bz in (min_z.floor() as i32)..=(max_z.ceil() as i32 - 1) {
                let Some(amount) = water(bx, by, bz) else {
                    continue;
                };
                let own = if water(bx, by + 1, bz).is_some() {
                    1.0_f32
                } else {
                    f32::from(amount) / 9.0
                };
                let top = f64::from(by) + f64::from(own);
                if top < min_y {
                    continue;
                }
                height = (top - y).max(height);
                let mut pushed = flow(water, bx, by, bz);
                current_height = height.max(current_height);
                if current_height < 0.4 {
                    pushed = pushed.map(|c| c * current_height);
                }
                current = [
                    current[0] + pushed[0],
                    current[1] + pushed[1],
                    current[2] + pushed[2],
                ];
                count += 1;
            }
        }
    }
    if height > 0.0 {
        hand.fall = 0.0;
    }
    let length = current[0] * current[0] + current[1] * current[1] + current[2] * current[2];
    if count > 0 && length >= TOLERANCE {
        let mut c = normalise(current).map(|c| c * 0.014);
        let c_length = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
        if hand.v[0].abs() < 0.003
            && hand.v[2].abs() < 0.003
            && c_length < 0.004_500_000_000_000_000_5
        {
            c = normalise(c).map(|c| c * 0.004_500_000_000_000_000_5);
        }
        hand.v = [hand.v[0] + c[0], hand.v[1] + c[1], hand.v[2] + c[2]];
    }
    height
}

/// `ItemEntity.tick` of an item in water, or out of it, over the stone floor.
fn wet_item_tick(hand: &mut Hand, tick: u64, id: i32, water: Water) {
    let height = in_water(hand, water);
    if height > 0.0 && height > f64::from(0.1_f32) {
        let d = f64::from(0.99_f32);
        let up = if hand.v[1] < f64::from(0.06_f32) {
            f64::from(5.0e-4_f32)
        } else {
            0.0
        };
        hand.v = [hand.v[0] * d, hand.v[1] + up, hand.v[2] * d];
    } else {
        hand.v[1] += -GRAVITY;
    }
    if moves_now(hand, tick, id) {
        entity_move(hand, &STONE_GROUND, 0.25, height > 0.0);
        let f3 = if hand.on_ground {
            on_ground_drag(STONE_FRICTION)
        } else {
            drag()
        };
        hand.v = [hand.v[0] * f3, hand.v[1] * drag(), hand.v[2] * f3];
        if hand.on_ground && hand.v[1] < 0.0 {
            hand.v[1] *= -0.5;
        }
    }
    in_water(hand, water);
}

fn one_source(x: i32, y: i32, z: i32) -> Option<u8> {
    ((x, y, z) == (8, -60, 8)).then_some(8)
}

/// The brief, sections 3.1 and 4: an item under still water has no gravity, rises at
/// 5e-4 a tick and is dragged by 0.98f, until it is in water to no more than 0.1f of a
/// block, where gravity takes it again: it bobs at the surface, tick for tick as the
/// game's. Still water has no current, so nothing pushes it sideways.
#[test]
fn an_item_in_still_water_rises_to_its_surface_and_bobs_there_as_the_game_s() {
    let placed = [(8, -60, 8, blocks::WATER)];
    let start = item(9, [8.5, -59.9, 8.5], [0.0; 3], false);
    let (mut region, load) = scene(&placed, vec![start.clone()]);
    let mut hand = Hand::of(&start);
    let mut output = load;
    let (mut lowest, mut highest) = (f64::MAX, f64::MIN);
    for count in 0..600 {
        wet_item_tick(&mut hand, output.tick, 9, one_source);
        if count >= 300 {
            lowest = lowest.min(hand.p[1]);
            highest = highest.max(hand.p[1]);
        }
        let now = record(&region, key(9)).expect("it floats");
        assert_is(&now, &hand, &format!("tick {} ({count})", output.tick));
        if count < 300 {
            // In water and moving, it is sent every tick (`needsSync`).
            let flags = tracked_in(&output, key(9)).map_or(0, |tracked| tracked.flags);
            assert!(
                flags & Tracked::NEEDS_SYNC != 0,
                "tick {}: {flags:#b}",
                output.tick
            );
        }
        output = idle(&mut region);
    }
    assert_eq!([hand.p[0], hand.p[2]], [8.5, 8.5]);
    // The model's item bobs about where it is in water to 0.1f of a block (the top of
    // the water is at -60 + 8/9f), never leaving the water.
    let bob = -60.0 + f64::from(8.0_f32 / 9.0) - f64::from(0.1_f32);
    assert!(
        lowest > -60.0 && lowest < bob && highest > bob && highest < -59.0,
        "{lowest} to {highest}"
    );
    assert_eq!(block_at(&region, 8, -60, 8), blocks::WATER);
}

/// Water flowing east along z = 8 from a source at x = 5, a block less at each step.
fn channel(x: i32, y: i32, z: i32) -> Option<u8> {
    (y == -60 && z == 8 && (5..=12).contains(&x)).then(|| u8::try_from(13 - x).expect("small"))
}

/// The brief, section 4: flowing water pushes an item along its flow, twice a tick
/// (in `baseTick` and again at the end of `ItemEntity.tick`), each time by its current
/// normalised and scaled by 0.014, while it floats up in it: tick for tick as the
/// game's.
#[test]
fn flowing_water_pushes_an_item_along_its_flow_twice_a_tick() {
    let placed: Vec<Placed> = (5..=12)
        .map(|x| {
            let level = x - 5;
            (x, -60, 8, state(&format!("minecraft:water[level={level}]")))
        })
        .collect();
    let start = item(10, [6.5, FLOOR, 8.5], [0.0; 3], true);
    let (mut region, load) = scene(&placed, vec![start.clone()]);
    let mut hand = Hand::of(&start);
    let mut output = load;
    let mut count = 0;
    while hand.p[0] < 10.5 {
        wet_item_tick(&mut hand, output.tick, 10, channel);
        let now = record(&region, key(10)).expect("it floats");
        assert_is(&now, &hand, &format!("tick {} ({count})", output.tick));
        output = idle(&mut region);
        count += 1;
        assert!(count < 60, "the model's item never got there: {hand:?}");
    }
    assert!(count > 3, "{count}");
    for (x, level) in (5..=12).zip(0..) {
        assert_eq!(
            block_at(&region, x, -60, 8),
            state(&format!("minecraft:water[level={level}]")),
            "the channel stays as it was"
        );
    }
}

/// The brief, 1.9 and 3.3: lava sets an item alight for 300 ticks and hurts it by 4
/// each tick, so an item of five health is gone in its second tick in lava; a
/// netherite ingot, which resists fire (`damage_resistant`), is neither hurt nor set
/// alight however long it lies there.
#[test]
fn lava_burns_an_item_in_two_ticks_and_netherite_never() {
    let placed = [(8, -60, 8, blocks::LAVA)];
    let start = item(11, [8.5, FLOOR, 8.5], [0.0; 3], true);
    let (mut region, first) = scene(&placed, vec![start.clone()]);
    let after = record(&region, key(11)).expect("alive after one tick");
    let (_, _, health) = data_of(&after);
    assert_eq!(health, 1, "{after:?}");
    assert_eq!(after.fire_ticks, 300, "{after:?}");
    assert!(!gone_in(&first, key(11)));
    let second = idle(&mut region);
    assert!(record(&region, key(11)).is_none());
    assert!(gone_in(&second, key(11)), "{:?}", second.events);
    assert!(
        second
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(key(11)))),
        "{:?}",
        second.entity_changes
    );

    let mut ingot = item(12, [8.5, FLOOR, 8.5], [0.0; 3], true);
    ingot.data = EntityData::Item {
        stack: stack(items::NETHERITE_INGOT, 1),
        pick_up_delay: 32_767,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    let (mut region, _) = scene(&placed, vec![ingot]);
    for _ in 0..100 {
        idle(&mut region);
        let now = record(&region, key(12)).expect("netherite does not burn");
        let (_, _, health) = data_of(&now);
        assert_eq!(health, 5);
        assert!(now.fire_ticks <= 0, "{now:?}");
    }
}

/// The brief, 1.9 and 3.3: a cactus hurts an item inside its block by 1 each tick, so
/// one that falls onto it (its top is 15/16 of the block) is gone in the fifth tick
/// from the one it entered the cactus's block in, its health counting down 4, 3, 2,
/// 1.
#[test]
fn an_item_that_falls_onto_a_cactus_is_pricked_to_nothing_in_five_ticks() {
    let placed = [(8, -61, 8, blocks::SAND), (8, -60, 8, blocks::CACTUS)];
    let start = item(13, [8.5, -58.5, 8.5], [0.0; 3], false);
    let (mut region, load) = scene(&placed, vec![start.clone()]);
    let cactus = Ground {
        top: -60.0 + 15.0 / 16.0,
        ..STONE_GROUND
    };
    let mut hand = Hand::of(&start);
    let mut output = load;
    let mut inside = 0;
    loop {
        item_tick(&mut hand, output.tick, 13, &cactus);
        // Its box, a hair smaller, is in the cactus's block.
        if hand.p[1] + 1e-5 < -59.0 {
            inside += 1;
        }
        if inside == 5 {
            assert!(record(&region, key(13)).is_none(), "tick {}", output.tick);
            assert!(gone_in(&output, key(13)));
            break;
        }
        let now = record(&region, key(13)).unwrap_or_else(|| panic!("tick {}: gone", output.tick));
        assert_is(&now, &hand, &format!("tick {}", output.tick));
        assert_eq!(data_of(&now).2, 5 - inside, "tick {}", output.tick);
        output = idle(&mut region);
        assert!(output.tick < 40);
    }
    assert_eq!(block_at(&region, 8, -60, 8), blocks::CACTUS);
}

// ------------------------------------------------------------------------------------
// Merging (the brief's 3.4).

/// The first tick at or after `from` on which an item at rest looks for others to merge
/// with: `tickCount % 40 == 0`.
fn merge_tick(from: u64) -> u64 {
    from.div_ceil(40) * 40
}

/// Two items at rest side by side on stone, and the region ticked up
/// to the tick before they first may merge; returns the region and the records then.
fn side_by_side(first: Entity, second: Entity) -> (Region, Entity, Entity) {
    let (mut region, load) = scene(&[], vec![first.clone(), second.clone()]);
    let mut tick = load.tick;
    while tick + 1 < merge_tick(load.tick) {
        let output = idle(&mut region);
        tick = output.tick;
        assert!(
            output
                .events
                .iter()
                .all(|event| !matches!(event, RegionEvent::EntityGone { .. })),
            "tick {tick}: nothing merges before the 40th tick"
        );
    }
    let a = record(&region, first.key).expect("there");
    let b = record(&region, second.key).expect("there");
    (region, a, b)
}

/// The brief, 3.4: two items of one kind within the box of half a block about each
/// other merge on the tick that looks (`tickCount % 40` for items at rest): the larger
/// stack takes the smaller, on equal counts the other takes the one that ticks (the
/// lower key); the merged item keeps the larger pick-up delay and the smaller age, as
/// the ticking one's are when the merge comes (its delay counted down already, its
/// age not yet counted up), and goes on with its own tick if it ticks after. The
/// merged stack, delay and age are committed, the other is gone, and the edge is told
/// the new stack.
#[test]
fn two_items_merge_by_the_game_s_rules_into_the_larger_with_the_larger_delay_and_smaller_age() {
    // (count of the lower key, count of the higher key), which absorbs which.
    for (low, high, lower_absorbs) in [(3, 5, false), (5, 3, true), (4, 4, false)] {
        let first = item_of(20, [8.5, FLOOR, 8.5], stack(items::DIRT, low), 300, 900);
        let second = item_of(21, [9.2, FLOOR, 8.5], stack(items::DIRT, high), 200, 50);
        let (mut region, a, b) = side_by_side(first, second);
        let (_, delay_a, _) = data_of(&a);
        let (_, delay_b, _) = data_of(&b);
        let output = idle(&mut region);
        assert_eq!(output.tick % 40, 0);
        let (target, source) = if lower_absorbs { (&a, &b) } else { (&b, &a) };
        // The lower key ticks first: its delay has gone down by one when it merges.
        let (expected_delay, expected_age) = if lower_absorbs {
            ((delay_a - 1).max(delay_b), a.age().min(b.age()) + 1)
        } else {
            ((delay_a - 1).max(delay_b) - 1, b.age().min(a.age()) + 1)
        };
        let merged = record(&region, target.key).expect("the larger stays");
        assert!(record(&region, source.key).is_none(), "the smaller is gone");
        assert_eq!(merged.position, target.position, "it stays where it lay");
        let (merged_stack, merged_delay, _) = data_of(&merged);
        let case = format!("{low} and {high}");
        assert_eq!(merged_stack, stack(items::DIRT, low + high), "{case}");
        assert_eq!(merged_delay, expected_delay, "{case}");
        assert_eq!(merged.age(), expected_age, "{case}");
        assert!(gone_in(&output, source.key), "{case}");
        assert!(
            output
                .entity_changes
                .contains(&(ORIGIN, EntityChange::Gone(source.key))),
            "{case}: {:?}",
            output.entity_changes
        );
        let set = output
            .entity_changes
            .iter()
            .find_map(|(_, change)| match change {
                EntityChange::Set(at, fields) if *at == target.key => Some(fields.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{case}: the merge is committed"));
        assert!(
            set.iter().any(|field| matches!(field,
                EntityField::Data(EntityData::Item { stack, .. }) if *stack == merged_stack)),
            "{case}: {set:?}"
        );
        let told = tracked_in(&output, target.key).expect("the edge is told");
        assert!(
            matches!(told.data, Some(EntityData::Item { stack, .. }) if stack == merged_stack),
            "{case}: {told:?}"
        );
    }
}

/// The plan, section 7 (G3), and `Entity`'s rule that a record's counters are as of
/// its tick: a merge's commit carries the merged item's smaller age and larger delay,
/// as the tick leaves them, so that what the commit says of the item is what the
/// region has of it after the tick, and a replay of the log restores it so.
#[test]
fn a_merge_commits_the_merged_age_and_delay_as_the_tick_leaves_them() {
    let mut wrong = Vec::new();
    for (low, high, lower_absorbs) in [(3, 5, false), (5, 3, true), (4, 4, false)] {
        let first = item_of(22, [8.5, FLOOR, 8.5], stack(items::DIRT, low), 300, 900);
        let second = item_of(23, [9.2, FLOOR, 8.5], stack(items::DIRT, high), 200, 50);
        let (mut region, a, b) = side_by_side(first, second);
        let target = if lower_absorbs { a.key } else { b.key };
        let output = idle(&mut region);
        let after = record(&region, target).expect("the larger stays");
        let mut replayed = if lower_absorbs { a.clone() } else { b.clone() };
        for (_, change) in &output.entity_changes {
            if let EntityChange::Set(at, fields) = change
                && *at == target
            {
                for field in fields {
                    field.set(&mut replayed);
                }
            }
        }
        let (_, delay_after, _) = data_of(&after);
        let (_, delay_replayed, _) = data_of(&replayed);
        if (replayed.age(), delay_replayed) != (after.age(), delay_after) {
            wrong.push(format!(
                "{low} and {high}: the commit gives age {} and delay {delay_replayed}, \
                 the region has {} and {delay_after}",
                replayed.age(),
                after.age()
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The brief, 3.4: no merge of items of different kinds, of two that would make more
/// than a stack, of a full stack, of one that is never to be picked up, nor of items
/// farther apart than the box looked in (0.5 beyond each one's own 0.25).
#[test]
fn items_merge_only_by_the_game_s_rules() {
    let never = 32_767;
    let cases = [
        (
            "different kinds",
            stack(items::DIRT, 3),
            stack(items::STONE, 3),
            0.7,
            100,
        ),
        (
            "more than a stack",
            stack(items::DIRT, 40),
            stack(items::DIRT, 30),
            0.7,
            100,
        ),
        (
            "a full stack",
            stack(items::DIRT, 64),
            stack(items::DIRT, 1),
            0.7,
            100,
        ),
        (
            "never picked up",
            stack(items::DIRT, 3),
            stack(items::DIRT, 3),
            0.7,
            never,
        ),
        (
            "too far apart",
            stack(items::DIRT, 3),
            stack(items::DIRT, 3),
            0.76,
            100,
        ),
    ];
    for (case, one, other, apart, delay) in cases {
        let first = item_of(30, [8.5, FLOOR, 8.5], one, delay, 10);
        let second = item_of(31, [8.5 + apart, FLOOR, 8.5], other, 100, 10);
        let (mut region, _, _) = side_by_side(first.clone(), second.clone());
        for _ in 0..81 {
            let output = idle(&mut region);
            assert!(
                !gone_in(&output, first.key) && !gone_in(&output, second.key),
                "{case}"
            );
        }
        assert_eq!(
            data_of(&record(&region, first.key).expect(case)).0,
            one,
            "{case}"
        );
        assert_eq!(
            data_of(&record(&region, second.key).expect(case)).0,
            other,
            "{case}"
        );
    }
    // Just within the box, they do.
    let first = item_of(32, [8.5, FLOOR, 8.5], stack(items::DIRT, 3), 100, 10);
    let second = item_of(33, [9.24, FLOOR, 8.5], stack(items::DIRT, 3), 100, 10);
    let (mut region, _, _) = side_by_side(first.clone(), second.clone());
    let output = idle(&mut region);
    assert!(gone_in(&output, first.key), "{:?}", output.events);
}

// ------------------------------------------------------------------------------------
// Despawning (the brief's 3.1).

/// The brief, 3.1: an item is discarded in the tick its age reaches 6000, having been
/// 5999 the tick before; its going is committed and reported.
#[test]
fn an_item_despawns_in_the_tick_its_age_reaches_6000() {
    let old = item_of(40, [8.5, FLOOR, 8.5], stack(items::DIRT, 1), 32_000, 5990);
    let (mut region, _) = scene(&[], vec![old.clone()]);
    assert_eq!(record(&region, old.key).expect("young enough").age(), 5991);
    for age in 5992..6000 {
        let output = idle(&mut region);
        assert_eq!(record(&region, old.key).expect("young enough").age(), age);
        assert!(!gone_in(&output, old.key));
    }
    let last = idle(&mut region);
    assert!(record(&region, old.key).is_none());
    assert!(gone_in(&last, old.key));
    assert!(
        last.entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(old.key)))
    );
}

/// The plan, section 7 (D1): an item at rest commits nothing, but its chunk is named
/// among those whose entities age, and the chunk as the region would save it has the
/// item's age and motion as of the tick.
#[test]
fn a_resting_item_s_age_is_in_the_saved_chunk_without_a_commit() {
    let lying = item_of(41, [8.5, FLOOR, 8.5], stack(items::DIRT, 1), 32_000, 100);
    let (mut region, _) = scene(&[], vec![lying.clone()]);
    for _ in 0..50 {
        let output = idle(&mut region);
        assert!(
            output.entity_changes.is_empty(),
            "tick {}: {:?}",
            output.tick,
            output.entity_changes
        );
        assert!(
            output.entities_ageing.contains(&ORIGIN),
            "tick {}",
            output.tick
        );
        let now = record(&region, lying.key).expect("there");
        let saved = region.saved_chunk(ORIGIN).expect("loaded");
        let kept = saved
            .entities()
            .iter()
            .find(|entity| entity.key == lying.key)
            .expect("saved with its chunk");
        assert_eq!(*kept, now);
    }
    assert_eq!(record(&region, lying.key).expect("there").age(), 151);
}

// ------------------------------------------------------------------------------------
// Pick-up (the brief's section 5).

/// The player's entity.
fn collector(region: &Region) -> EntityId {
    region.player(player(1)).expect("player 1 is there").0
}

fn slot(region: &Region, slot: usize) -> Option<ItemStack> {
    region.state().players[&player(1)].inventory.0[slot]
}

fn taken_in(output: &TickOutput, item: EntityId) -> Option<(EntityId, i32)> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::ItemTaken {
            item: taken,
            collector,
            count,
            ..
        } if *taken == item => Some((*collector, *count)),
        _ => None,
    })
}

fn slots_told(output: &TickOutput) -> Vec<(u8, Option<ItemStack>)> {
    output
        .player_events
        .iter()
        .filter_map(|(to, event)| match event {
            PlayerEvent::InventorySlot { slot, stack } if *to == player(1) => Some((*slot, *stack)),
            _ => None,
        })
        .collect()
}

/// The brief, section 5: a player picks up what touches their box (0.6 wide, 1.8 high)
/// grown by 1 sideways and 0.5 up and down, once its delay is 0, after the entities'
/// tick: one at x 1.9 beside a player at x 0.5 is taken (its box begins at 1.775, the
/// grown box ends at 1.8), one at 1.95 is not. What is taken goes to the first empty
/// slot, the client is told the slot and everyone the pick-up, and the item goes.
#[test]
fn a_player_picks_up_an_item_within_the_game_s_box_once_its_delay_is_over() {
    let near = item_of(50, [1.9, FLOOR, 0.5], stack(items::DIRT, 2), 0, 10);
    let far = item_of(51, [1.95, FLOOR, 0.5], stack(items::DIRT, 2), 0, 10);
    let waiting = item_of(52, [0.5, FLOOR, 1.9], stack(items::SAND, 1), 3, 10);
    let (mut region, load) = scene(&[], vec![near.clone(), far.clone(), waiting.clone()]);
    let me = collector(&region);
    assert_eq!(taken_in(&load, near.key), Some((me, 2)));
    assert!(gone_in(&load, near.key));
    assert!(record(&region, near.key).is_none());
    assert_eq!(slot(&region, 1), Some(stack(items::DIRT, 2)));
    assert!(
        slots_told(&load).contains(&(1, Some(stack(items::DIRT, 2)))),
        "{:?}",
        load.player_events
    );
    // The taking comes before the going.
    let taken = load
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::ItemTaken { .. }));
    let gone = load.events.iter().position(
        |event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == near.key),
    );
    assert!(taken < gone, "{:?}", load.events);
    assert!(record(&region, far.key).is_some());
    assert_eq!(taken_in(&load, far.key), None);

    // The sand's delay of 3 is 2, 1 and 0 after the ticks of the load and the two
    // after: it is taken in the third.
    assert!(record(&region, waiting.key).is_some());
    let second = idle(&mut region);
    assert_eq!(taken_in(&second, waiting.key), None);
    let third = idle(&mut region);
    assert_eq!(taken_in(&third, waiting.key), Some((me, 1)));
    assert_eq!(slot(&region, 2), Some(stack(items::SAND, 1)));
    for _ in 0..40 {
        let output = idle(&mut region);
        assert_eq!(taken_in(&output, far.key), None);
    }
}

/// The brief, section 5 (`Inventory.add`): what is picked up goes first to the selected
/// slot, then the off hand, then the slots 0 to 35 in order, where a stack of the same
/// item has room; only then to the first empty one of 0 to 35.
#[test]
fn what_is_picked_up_goes_to_the_slot_the_game_chooses() {
    let cases: [(&str, Holding, usize); 4] = [
        ("a free slot", vec![(0, stack(items::STONE, 64))], 1),
        (
            "a stack with room",
            vec![(0, stack(items::STONE, 64)), (3, stack(items::DIRT, 10))],
            3,
        ),
        (
            "the off hand first",
            vec![
                (0, stack(items::STONE, 64)),
                (3, stack(items::DIRT, 10)),
                (OFF_HAND, stack(items::DIRT, 5)),
            ],
            OFF_HAND,
        ),
        (
            "the selected slot first",
            vec![
                (0, stack(items::DIRT, 1)),
                (3, stack(items::DIRT, 10)),
                (OFF_HAND, stack(items::DIRT, 5)),
            ],
            0,
        ),
    ];
    for (case, held, into) in cases {
        let mut inventory = Inventory::default();
        for (at, what) in &held {
            inventory.set(*at, Some(*what));
        }
        let before = inventory.0[into].map_or(0, |stack| stack.count);
        let mut origin = floor();
        origin.set_entities(vec![item_of(
            53,
            [1.0, FLOOR, 1.0],
            stack(items::DIRT, 2),
            0,
            10,
        )]);
        let (region, load) = scene_holding(origin, inventory);
        assert!(taken_in(&load, key(53)).is_some(), "{case}");
        assert_eq!(
            slot(&region, into),
            Some(stack(items::DIRT, before + 2)),
            "{case}"
        );
        for (at, what) in &held {
            if *at != into {
                assert_eq!(slot(&region, *at), Some(*what), "{case}: slot {at}");
            }
        }
    }
}

/// The brief, section 5: a creative player takes the whole item and what does not fit
/// is destroyed; a full inventory takes nothing and the item is gone all the same.
#[test]
fn a_creative_player_takes_the_whole_item_and_what_does_not_fit_is_destroyed() {
    let mut inventory = Inventory::default();
    for at in 0..36 {
        inventory.set(at, Some(stack(items::STONE, 64)));
    }
    inventory.set(0, Some(stack(items::DIRT, 60)));
    let mut origin = floor();
    origin.set_entities(vec![
        item_of(54, [1.0, FLOOR, 1.0], stack(items::DIRT, 10), 0, 10),
        item_of(55, [0.0, FLOOR, 1.0], stack(items::SAND, 7), 0, 10),
    ]);
    let (region, load) = scene_holding(origin, inventory);
    let me = collector(&region);
    assert_eq!(taken_in(&load, key(54)), Some((me, 10)));
    assert_eq!(taken_in(&load, key(55)), Some((me, 7)));
    assert!(record(&region, key(54)).is_none());
    assert!(record(&region, key(55)).is_none());
    assert_eq!(slot(&region, 0), Some(stack(items::DIRT, 64)));
    for at in 1..36 {
        assert_eq!(
            slot(&region, at),
            Some(stack(items::STONE, 64)),
            "slot {at}"
        );
    }
    assert_eq!(slot(&region, OFF_HAND), None);
}

/// The plan, section 1, and the brief, section 5: an item a player throws looking
/// down falls at their feet and is theirs again in the 40th tick of its life, the tick
/// of the throw the first (it is added before the entities' phase and ticks in it):
/// into the selected slot, which has room for it.
#[test]
fn a_thrown_item_is_picked_up_again_after_its_forty_ticks() {
    let (mut region, _) = scene(&[], Vec::new());
    let me = collector(&region);
    let look_down = PlayerInput::Move {
        position: None,
        rotation: Some((0.0, 90.0)),
        on_ground: true,
    };
    let throw = PlayerInput::Drop {
        all: false,
        crouching: false,
    };
    let thrown = region.tick(&TickInputs {
        inputs: vec![
            (EDGE, player(1), me, 1, look_down),
            (EDGE, player(1), me, 2, throw),
        ],
        ..TickInputs::default()
    });
    let made = spawned(&thrown);
    let [made] = &made[..] else {
        panic!("one item: {:?}", thrown.events);
    };
    assert_eq!(slot(&region, 0), Some(stack(items::STONE, 63)));
    let (data, _) = spawn_parts(made);
    assert!(
        matches!(
            data,
            EntityData::Item {
                pick_up_delay: 40,
                ..
            }
        ),
        "{data:?}"
    );
    let after = record(&region, made.entity).expect("it ticked in the throw's tick");
    assert_eq!(data_of(&after).1, 39, "{after:?}");
    for life in 2..40 {
        let output = idle(&mut region);
        assert_eq!(
            taken_in(&output, made.entity),
            None,
            "tick {life} of its life"
        );
    }
    let fortieth = idle(&mut region);
    assert_eq!(taken_in(&fortieth, made.entity), Some((me, 1)));
    assert_eq!(slot(&region, 0), Some(stack(items::STONE, 64)));
    assert!(record(&region, made.entity).is_none());
}

// ------------------------------------------------------------------------------------
// Falling blocks (the brief's sections 6 and 7).

/// The brief, section 7: sand falls by gravity and the drag, its time counting up, and
/// lands on the floor in the tick its fall is cut; it becomes the block at its feet,
/// shown at once and as a transient block, and then goes.
#[test]
fn sand_falls_and_lands_at_the_game_s_tick_and_place() {
    let sand = falling(60, [8.5, -55.0, 8.5], blocks::SAND);
    let (mut region, load) = scene(&[], vec![sand.clone()]);
    let mut hand = Hand::of(&sand);
    let ground = Ground {
        top: FLOOR,
        ..STONE_GROUND
    };
    let mut output = load;
    let mut time = 0;
    loop {
        time += 1;
        let landed = falling_tick(&mut hand, &ground);
        if landed {
            break;
        }
        let now = record(&region, sand.key).expect("falling");
        assert_is(&now, &hand, &format!("tick {}", output.tick));
        assert!(
            matches!(now.data, EntityData::FallingBlock { time: t, .. } if t == time),
            "{now:?}"
        );
        output = idle(&mut region);
        assert!(time < 40);
    }
    assert_eq!(time, 17, "the model's ticks to fall five blocks");
    assert!(record(&region, sand.key).is_none());
    assert_eq!(block_at(&region, 8, -60, 8), blocks::SAND);
    let landing = BlockPos::new(8, -60, 8);
    let shown: Vec<&RegionEvent> = output
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::BlockChanged { position, .. }
                    | RegionEvent::TransientBlock { position, .. } if *position == landing
            ) || matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == sand.key)
        })
        .collect();
    assert!(
        matches!(
            shown[..],
            [
                RegionEvent::BlockChanged { state: changed, .. },
                RegionEvent::TransientBlock { state: transient, .. },
                RegionEvent::EntityGone { .. },
            ] if *changed == blocks::SAND && *transient == blocks::SAND.0
        ),
        "{shown:?}"
    );
}

/// The brief, sections 6 and 7: sand whose support a player breaks is scheduled to
/// fall two ticks after the break (counted from the game time the break was handled
/// at, the tick before); in that tick it becomes an entity at the middle of its block,
/// shown before the air it leaves, which ticks at once; it lands where the game's fall
/// puts it. The creative player's break drops nothing.
#[test]
fn sand_whose_support_is_broken_falls_from_the_game_s_tick_and_lands_on_what_is_below() {
    let placed = [
        (2, -60, 2, blocks::STONE),
        (2, -59, 2, blocks::STONE),
        (2, -58, 2, blocks::SAND),
    ];
    let (mut region, _) = scene(&placed, Vec::new());
    let me = collector(&region);
    let dig = PlayerInput::Dig {
        position: BlockPos::new(2, -59, 2),
        sequence: 1,
    };
    let broken = region.tick(&TickInputs {
        inputs: vec![(EDGE, player(1), me, 1, dig)],
        ..TickInputs::default()
    });
    assert_eq!(block_at(&region, 2, -59, 2), blocks::AIR);
    assert!(
        spawned(&broken).is_empty(),
        "a creative break drops nothing: {:?}",
        broken.events
    );
    assert_eq!(block_at(&region, 2, -58, 2), blocks::SAND, "not yet");
    let started = idle(&mut region);
    let falling_sand = spawned(&started);
    let [entity] = &falling_sand[..] else {
        panic!("one falling block: {:?}", started.events);
    };
    assert_eq!(entity.pose.position, Vec3::new(2.5, -58.0, 2.5));
    let (data, motion) = spawn_parts(entity);
    assert_eq!(motion, [0.0; 3]);
    assert!(
        matches!(data, EntityData::FallingBlock { state, time: 0, .. } if state == blocks::SAND.0),
        "{data:?}"
    );
    let spawn_at = started
        .events
        .iter()
        .position(|event| matches!(event, RegionEvent::EntitySpawned(_)))
        .expect("spawned");
    let air_at = started
        .events
        .iter()
        .position(|event| {
            matches!(event,
            RegionEvent::BlockChanged { position, state, .. }
                if *position == BlockPos::new(2, -58, 2) && *state == blocks::AIR)
        })
        .expect("its block leaves");
    assert!(spawn_at < air_at, "{:?}", started.events);
    // It ticked in the tick it began to fall.
    let now = record(&region, entity.entity).expect("falling");
    let mut hand = Hand {
        p: [2.5, -58.0, 2.5],
        v: [0.0; 3],
        on_ground: false,
        fall: 0.0,
    };
    let ground = Ground {
        top: -59.0,
        ..STONE_GROUND
    };
    assert!(!falling_tick(&mut hand, &ground));
    assert_is(&now, &hand, "its first tick");
    let mut ticks = 1;
    loop {
        let output = idle(&mut region);
        ticks += 1;
        if falling_tick(&mut hand, &ground) {
            assert!(record(&region, entity.entity).is_none());
            assert!(gone_in(&output, entity.entity));
            break;
        }
        assert_is(
            &record(&region, entity.entity).expect("falling"),
            &hand,
            &format!("tick {ticks} of its fall"),
        );
    }
    assert_eq!(ticks, 7);
    assert_eq!(block_at(&region, 2, -59, 2), blocks::SAND);
}

/// The brief, finding 3 and section 7: sand that comes down onto a bottom slab stops
/// at its top in the slab's block, which it cannot replace: it drops one item of sand
/// where it stopped, with the default pick-up delay of 10, set off at 0.2 up and a
/// little sideways (the random constructor of `ItemEntity`); made in the entities'
/// phase, the item does not tick until the next tick, and then goes up as the model
/// says.
#[test]
fn sand_that_cannot_land_drops_one_item_of_itself_where_it_stopped() {
    let slab = state("minecraft:stone_slab[type=bottom]");
    let sand = falling(61, [8.5, -57.0, 8.5], blocks::SAND);
    let (mut region, load) = scene(&[(8, -60, 8, slab)], vec![sand.clone()]);
    let mut output = load;
    let ground = Ground {
        top: -59.5,
        ..STONE_GROUND
    };
    let mut hand = Hand::of(&sand);
    while !falling_tick(&mut hand, &ground) {
        output = idle(&mut region);
        assert!(output.tick < 60);
    }
    assert!(record(&region, sand.key).is_none());
    assert_eq!(block_at(&region, 8, -60, 8), slab);
    let dropped = spawned(&output);
    let [drop] = &dropped[..] else {
        panic!("one item: {:?}", output.events);
    };
    assert_eq!(drop.pose.position, Vec3::new(8.5, -59.5, 8.5));
    let (data, motion) = spawn_parts(drop);
    assert_eq!(
        data,
        EntityData::Item {
            stack: stack(items::SAND, 1),
            pick_up_delay: 10,
            thrower: None,
            health: 5,
            age_offset: 0,
        }
    );
    assert_eq!(motion[1], 0.2);
    for sideways in [motion[0], motion[2]] {
        assert!((-0.1..0.1).contains(&sideways), "{motion:?}");
    }
    let lying = record(&region, drop.entity).expect("laid");
    assert_eq!(lying.position, [8.5, -59.5, 8.5], "it has not ticked");
    assert_eq!(lying.motion, motion);
    // Its first tick goes up by the motion less gravity, in the air.
    idle(&mut region);
    let mut flying = Hand {
        p: [8.5, -59.5, 8.5],
        v: motion,
        on_ground: false,
        fall: 0.0,
    };
    item_tick(&mut flying, 0, drop.entity.0 as i32, &ground);
    assert_is(
        &record(&region, drop.entity).expect("flies"),
        &flying,
        "its first tick",
    );
}

/// The brief, section 7: concrete powder that falls into water lands in the first
/// block of water its feet are in, whatever is below, and hardens to concrete there
/// (`onLand`, `shouldSolidify`).
#[test]
fn concrete_powder_falling_into_water_lands_in_it_and_hardens() {
    let pool: Vec<Placed> = (7..=9)
        .flat_map(|x| (7..=9).map(move |z| (x, -60, z, blocks::WATER)))
        .collect();
    let powder = falling(62, [8.5, -55.0, 8.5], blocks::WHITE_CONCRETE_POWDER);
    let (mut region, load) = scene(&pool, vec![powder.clone()]);
    let mut hand = Hand::of(&powder);
    let air = Ground {
        top: FLOOR,
        ..STONE_GROUND
    };
    let mut output = load;
    loop {
        falling_tick(&mut hand, &air);
        if hand.p[1].floor() as i32 == -60 {
            break;
        }
        assert_is(
            &record(&region, powder.key).expect("falling"),
            &hand,
            "falling",
        );
        assert_eq!(block_at(&region, 8, -60, 8), blocks::WATER);
        output = idle(&mut region);
        assert!(output.tick < 60);
    }
    assert!(
        record(&region, powder.key).is_none(),
        "tick {}",
        output.tick
    );
    assert!(gone_in(&output, powder.key));
    assert_eq!(block_at(&region, 8, -60, 8), blocks::WHITE_CONCRETE);
    assert!(spawned(&output).is_empty(), "it landed: no drop");
}

// ------------------------------------------------------------------------------------
// Drops of blocks broken by a rule (the brief's section 8).

/// `Block.popResource` puts a block's drop within a quarter of a block of its centre
/// each way, its feet half the item's height (0.125) below; the item has the default
/// delay, health 5, 0.2 up and under 0.1 sideways.
fn assert_popped(state: &clustine_sim::api::EntityState, at: BlockPos, what: ItemStack) {
    let p = state.pose.position;
    let (cx, cy, cz) = (
        f64::from(at.x) + 0.5,
        f64::from(at.y) + 0.5 - 0.125,
        f64::from(at.z) + 0.5,
    );
    assert!((cx - 0.25..cx + 0.25).contains(&p.x), "{p:?}");
    assert!((cy - 0.25..cy + 0.25).contains(&p.y), "{p:?}");
    assert!((cz - 0.25..cz + 0.25).contains(&p.z), "{p:?}");
    let (data, motion) = spawn_parts(state);
    assert_eq!(
        data,
        EntityData::Item {
            stack: what,
            pick_up_delay: 10,
            thrower: None,
            health: 5,
            age_offset: 0,
        }
    );
    assert_eq!(motion[1], 0.2);
    assert!(
        (-0.1..0.1).contains(&motion[0]) && (-0.1..0.1).contains(&motion[2]),
        "{motion:?}"
    );
}

/// The brief, 8.1 and 8.4: a torch whose support a creative player breaks breaks by
/// its rule and drops a torch (its loot table), popped out where it was; the break
/// itself drops nothing. Made before the entities' phase, the torch ticks in that tick,
/// as the model has it.
#[test]
fn a_torch_whose_support_goes_drops_a_torch_and_the_creative_break_nothing() {
    let placed = [(3, -60, 3, blocks::STONE), (3, -59, 3, blocks::TORCH)];
    let (mut region, _) = scene(&placed, Vec::new());
    let me = collector(&region);
    let dig = PlayerInput::Dig {
        position: BlockPos::new(3, -60, 3),
        sequence: 1,
    };
    let output = region.tick(&TickInputs {
        inputs: vec![(EDGE, player(1), me, 1, dig)],
        ..TickInputs::default()
    });
    assert_eq!(block_at(&region, 3, -59, 3), blocks::AIR);
    let dropped = spawned(&output);
    let [torch] = &dropped[..] else {
        panic!("one drop, the torch's: {:?}", output.events);
    };
    assert_popped(torch, BlockPos::new(3, -59, 3), stack(items::TORCH, 1));
    let (_, motion) = spawn_parts(torch);
    let mut hand = Hand {
        p: [
            torch.pose.position.x,
            torch.pose.position.y,
            torch.pose.position.z,
        ],
        v: motion,
        on_ground: false,
        fall: 0.0,
    };
    let id = torch.entity.0 as u32 as i32;
    item_tick(&mut hand, output.tick, id, &STONE_GROUND);
    let now = record(&region, torch.entity).expect("it ticked");
    assert_is(&now, &hand, "the tick it was dropped in");
    assert_eq!(data_of(&now).1, 9);
    for _ in 0..30 {
        let output = idle(&mut region);
        item_tick(&mut hand, output.tick, id, &STONE_GROUND);
        assert_is(
            &record(&region, torch.entity).expect("lies"),
            &hand,
            "falling",
        );
    }
    assert!(hand.on_ground);
}

/// The brief, 8.1: water that washes a torch away drops it (`beforeDestroyingBlock`).
#[test]
fn water_that_washes_a_torch_away_drops_the_torch() {
    let placed = [
        (3, -60, 2, blocks::STONE),
        (3, -60, 3, blocks::WATER),
        (4, -60, 3, blocks::TORCH),
    ];
    let (mut region, _) = scene(&placed, Vec::new());
    let me = collector(&region);
    // Breaking the stone beside the water wakes it.
    let dig = PlayerInput::Dig {
        position: BlockPos::new(3, -60, 2),
        sequence: 1,
    };
    let mut output = region.tick(&TickInputs {
        inputs: vec![(EDGE, player(1), me, 1, dig)],
        ..TickInputs::default()
    });
    let mut drops = Vec::new();
    for _ in 0..40 {
        drops.extend(spawned(&output));
        if block_at(&region, 4, -60, 3) != blocks::TORCH {
            break;
        }
        output = idle(&mut region);
    }
    assert_ne!(block_at(&region, 4, -60, 3), blocks::TORCH, "washed away");
    let [torch] = &drops[..] else {
        panic!("one drop, the torch's: {drops:?}");
    };
    assert_popped(torch, BlockPos::new(4, -60, 3), stack(items::TORCH, 1));
}

/// The brief, 8.2 and 8.3: oak leaves that decay drop by their loot table with no
/// tool: a sapling at 1 in 20, a stick or two at 1 in 50, an apple at 1 in 200, and
/// never the leaves themselves; counted over well over a thousand leaves, each within
/// five standard deviations of the game's chance.
#[test]
fn decaying_oak_leaves_drop_saplings_sticks_and_apples_by_the_game_s_chances() {
    let leaves = state("minecraft:oak_leaves[distance=7,persistent=false,waterlogged=false]");
    let mut origin = floor();
    for y in -48..-32 {
        for z in 0..16 {
            for x in 0..16 {
                origin.set(x, y, z, leaves);
            }
        }
    }
    let (mut region, load) = scene_holding(origin, stone_in_hand());
    let mut drops = spawned(&load);
    let left = |region: &Region| {
        let mut count = 0;
        for y in -48..-32 {
            for z in 0..16 {
                for x in 0..16 {
                    if block_at(region, x, y, z) == leaves {
                        count += 1;
                    }
                }
            }
        }
        count
    };
    let mut ticks = 0;
    while 4096 - left(&region) < 1500 {
        drops.extend(spawned(&idle(&mut region)));
        ticks += 1;
        assert!(ticks < 4000, "{} leaves decayed", 4096 - left(&region));
    }
    let decayed = f64::from(4096 - left(&region));
    let (mut saplings, mut sticks, mut stick_items, mut apples, mut twos) = (0, 0, 0, 0, 0);
    for drop in &drops {
        let (data, _) = spawn_parts(drop);
        let EntityData::Item { stack, .. } = data else {
            panic!("{data:?}");
        };
        match stack.item {
            items::OAK_SAPLING => {
                assert_eq!(stack.count, 1);
                saplings += 1;
            }
            items::STICK => {
                assert!((1..=2).contains(&stack.count), "{stack:?}");
                sticks += 1;
                stick_items += stack.count;
                twos += i32::from(stack.count == 2);
            }
            items::APPLE => {
                assert_eq!(stack.count, 1);
                apples += 1;
            }
            other => panic!("leaves dropped item {other}"),
        }
    }
    for (what, count, chance) in [
        ("saplings", saplings, 0.05),
        ("sticks", sticks, 0.02),
        ("apples", apples, 0.005),
    ] {
        let mean = decayed * chance;
        let spread = (decayed * chance * (1.0 - chance)).sqrt();
        let count = f64::from(count);
        assert!(
            (count - mean).abs() <= 5.0 * spread,
            "{what}: {count} of {decayed} leaves, the game's {mean} ± {spread}"
        );
    }
    // A stick drop is one or two, evenly.
    assert!(twos > 0 && twos < sticks, "{twos} twos of {sticks}");
    assert!(stick_items > sticks);
}

// ------------------------------------------------------------------------------------
// A region and its restore (ADR-0014; the plan, D3).

/// Items and a falling block in motion: one falling, one sliding on ice, one bobbing
/// in water, one bouncing on slime, one at rest, and sand falling.
fn busy() -> (Vec<Placed>, Vec<Entity>) {
    let mut placed: Vec<Placed> = (4..12).map(|x| (x, -61, 12, blocks::ICE)).collect();
    placed.push((8, -61, 8, blocks::SLIME_BLOCK));
    placed.push((12, -60, 4, blocks::WATER));
    let entities = vec![
        item(70, [4.5, -56.0, 4.5], [0.01, 0.0, -0.02], false),
        item(71, [4.5, FLOOR, 12.5], [0.25, 0.0, 0.0], true),
        item(72, [12.5, -59.9, 4.5], [0.0; 3], false),
        item(73, [8.5, -57.0, 8.5], [0.0; 3], false),
        item(74, [10.5, FLOOR, 6.5], [0.0; 3], true),
        falling(75, [6.5, -50.0, 10.5], blocks::SAND),
    ];
    (placed, entities)
}

/// ADR-0014 and the plan, D3: a region restored from its state and its chunks as it
/// would save them goes on with its items and falling blocks tick for tick as the
/// region itself does: the same records, the same commits and the same reports, at
/// every point of their motion.
#[test]
fn a_region_and_its_restore_move_items_and_falling_blocks_alike_tick_for_tick() {
    let (placed, entities) = busy();
    for restore_after in [1, 2, 3, 5, 9, 14, 23] {
        let (mut region, _) = scene(&placed, entities.clone());
        for _ in 0..restore_after {
            idle(&mut region);
        }
        let held = around(3);
        let tickets: Vec<(ChunkPos, Ticket)> = around(2)
            .into_iter()
            .map(|chunk| (chunk, Ticket::Viewer))
            .collect();
        let chunks = region
            .loaded_chunks()
            .into_iter()
            .map(|chunk| (chunk, region.saved_chunk(chunk).expect("loaded")))
            .collect();
        let mut restored = Region::restore_loaded(
            config_holding(stone_in_hand()),
            region.state(),
            Holdings {
                held,
                pinned: vec![ChunkArea::EVERYWHERE],
                lent: Vec::new(),
                lent_away: Vec::new(),
            },
            &tickets,
            chunks,
            Vec::new(),
        );
        for tick in 0..60 {
            let one = idle(&mut region);
            let other = idle(&mut restored);
            let case = format!("restored after {restore_after}, tick {tick}");
            assert_eq!(one.tick, other.tick, "{case}");
            assert_eq!(one.entity_changes, other.entity_changes, "{case}");
            let reports = |output: &TickOutput| -> Vec<RegionEvent> {
                output
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            RegionEvent::EntityTracked(_)
                                | RegionEvent::EntityGone { .. }
                                | RegionEvent::EntitySpawned(_)
                                | RegionEvent::TransientBlock { .. }
                        )
                    })
                    .cloned()
                    .collect()
            };
            assert_eq!(reports(&one), reports(&other), "{case}");
            for chunk in region.loaded_chunks() {
                assert_eq!(
                    region.chunk(chunk).map(Chunk::entities),
                    restored.chunk(chunk).map(Chunk::entities),
                    "{case}, chunk {chunk:?}"
                );
            }
        }
    }
}

// ------------------------------------------------------------------------------------
// A pinned line (the plan, section 1 and D2).

/// Pinned west of chunk x = 1: the region holds chunk 0 and those west of it, and
/// copies of the two columns of chunks east of the line, so that chunk (0, 0) ticks.
fn at_a_line(origin: Chunk) -> (Region, TickOutput) {
    let area = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let mut region = region_in(area, stone_in_hand());
    joined(&mut region);
    let copies = around(2)
        .into_iter()
        .filter(|chunk| !area.contains(*chunk))
        .map(|chunk| (chunk, floor()))
        .collect();
    let output = load_in(&mut region, area, origin, copies);
    (region, output)
}

/// The arrivals of `outputs` of the entity `key`, each with its tick.
fn sent(outputs: &[TickOutput], key: EntityId) -> Vec<(u64, ChunkPos, Entity)> {
    outputs
        .iter()
        .flat_map(|output| {
            output
                .arrivals
                .iter()
                .filter(|(_, entity)| entity.key == key)
                .map(|(chunk, entity)| (output.tick, *chunk, entity.clone()))
        })
        .collect()
}

/// Step N3d (`docs/groundwork/n3-plan.md`, section 5), in place of N4's guard, which held
/// the item at the line: an item that slides across a pinned line is sent once, in the
/// tick it crosses, whole with the hops it had, and is gone from the region's chunk in
/// that very tick: neither lost nor doubled, and never in the copy.
#[test]
fn an_item_that_slides_across_a_pinned_line_is_sent_once_and_neither_lost_nor_doubled() {
    let mut origin = floor();
    let thrown = item(80, [15.95, FLOOR, 8.5], [0.3, 0.0, 0.0], true);
    origin.set_entities(vec![thrown.clone()]);
    let (mut region, load) = at_a_line(origin);
    let mut outputs = vec![load];
    for _ in 0..100 {
        outputs.push(idle(&mut region));
    }
    let arrivals = sent(&outputs, thrown.key);
    let [(tick, chunk, whole)] = &arrivals[..] else {
        panic!("once: {arrivals:?}");
    };
    assert_eq!(*chunk, ChunkPos::new(1, 0));
    assert_eq!(whole.chunk(), *chunk);
    assert!(whole.position[0] >= 16.0, "{whole:?}");
    let crossing = outputs
        .iter()
        .find(|output| output.tick == *tick)
        .expect("ticked");
    assert!(
        crossing
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(thrown.key))),
        "{:?}",
        crossing.entity_changes
    );
    assert!(crossing.events.iter().any(|event| matches!(
        event,
        RegionEvent::HandedOn { entity, chunk: ORIGIN, .. } if *entity == thrown.key
    )));
    assert!(records(&region, thrown.key).is_empty());
    for output in &outputs {
        assert!(!gone_in(output, thrown.key), "tick {}", output.tick);
        assert!(output.effects.is_empty(), "{:?}", output.effects);
    }
    let copy = region.copy(ChunkPos::new(1, 0)).expect("copied");
    assert!(copy.entities().is_empty());
}

/// The same with water that carries an item to a pinned line: until it crosses it is
/// in the region's last chunk, once, and then it is sent, once.
#[test]
fn water_carries_an_item_across_a_pinned_line_and_it_is_sent_once() {
    let placed: Vec<Placed> = (9..=15)
        .map(|x| {
            let level = (x - 9).min(6);
            (x, -60, 8, state(&format!("minecraft:water[level={level}]")))
        })
        .collect();
    let mut origin = floor_with(&placed);
    let carried = item(81, [10.5, FLOOR, 8.5], [0.0; 3], true);
    origin.set_entities(vec![carried.clone()]);
    let (mut region, _) = at_a_line(origin);
    let mut outputs = Vec::new();
    for tick in 0..200 {
        let output = idle(&mut region);
        let gone = !sent(std::slice::from_ref(&output), carried.key).is_empty();
        outputs.push(output);
        let found = records(&region, carried.key);
        if gone || sent(&outputs, carried.key).len() == 1 {
            assert!(found.is_empty(), "tick {tick}: {found:?}");
            continue;
        }
        let [(chunk, now)] = &found[..] else {
            panic!("tick {tick}: once: {found:?}");
        };
        assert_eq!(*chunk, ORIGIN, "tick {tick}");
        assert!(now.position[0] < 16.0, "tick {tick}: {now:?}");
    }
    let arrivals = sent(&outputs, carried.key);
    assert_eq!(arrivals.len(), 1, "{arrivals:?}");
    assert_eq!(arrivals[0].1, ChunkPos::new(1, 0));
}

// ------------------------------------------------------------------------------------
// More of the item's life.

/// The plan, section 1, and the brief, 3.1: an item a player throws (Q, looking ahead
/// along x) leaves as its spawn shows it, ticks in the tick of the throw, flies, lands
/// on the stone and lies there, tick for tick as the model has it from its spawn.
#[test]
fn a_thrown_item_flies_lands_on_stone_and_lies_as_the_game_s_tick_after_tick() {
    let (mut region, _) = scene(&[], Vec::new());
    let me = collector(&region);
    let look = PlayerInput::Move {
        position: None,
        rotation: Some((-90.0, 0.0)),
        on_ground: true,
    };
    let throw = PlayerInput::Drop {
        all: false,
        crouching: false,
    };
    let mut output = region.tick(&TickInputs {
        inputs: vec![
            (EDGE, player(1), me, 1, look),
            (EDGE, player(1), me, 2, throw),
        ],
        ..TickInputs::default()
    });
    let made = spawned(&output);
    let [made] = &made[..] else {
        panic!("one item: {:?}", output.events);
    };
    let (_, motion) = spawn_parts(made);
    assert!(motion[0] > 0.2, "thrown along x: {motion:?}");
    let p = made.pose.position;
    let mut hand = Hand {
        p: [p.x, p.y, p.z],
        v: motion,
        on_ground: false,
        fall: 0.0,
    };
    let id = made.entity.0 as u32 as i32;
    let mut landed = None;
    for count in 0..60 {
        let step = item_tick(&mut hand, output.tick, id, &STONE_GROUND);
        if step.vertical && landed.is_none() {
            landed = Some(count);
        }
        assert_is(
            &record(&region, made.entity).expect("there"),
            &hand,
            &format!("tick {} of its life", count + 1),
        );
        output = idle(&mut region);
    }
    assert!(landed.is_some_and(|tick| tick > 3), "{landed:?}");
    assert_eq!(hand.p[1], FLOOR);
    assert!(hand.p[0] > 2.0, "out of the thrower's reach: {hand:?}");
}

/// The brief, 1.9 and 3.3: fire sets an item alight for 160 ticks and hurts it by 1
/// each tick it is in it, after the burning's own 1 on every twentieth tick of its
/// fire (in `baseTick`): an item of five health in fire is at 4 after one tick and gone
/// in its third.
#[test]
fn fire_sets_an_item_alight_and_takes_its_health_in_three_ticks() {
    let start = item(90, [8.5, FLOOR, 8.5], [0.0; 3], true);
    let (mut region, first) = scene(&[(8, -60, 8, state("minecraft:fire"))], vec![start.clone()]);
    let now = record(&region, start.key).expect("alive");
    assert_eq!((data_of(&now).2, now.fire_ticks), (4, 160), "{now:?}");
    // The shared flag is set in `baseTick`, before the fire is caught: it is shown
    // alight from the second tick.
    let told = tracked_in(&first, start.key).map_or(0, |tracked| tracked.flags);
    assert!(told & Tracked::ON_FIRE == 0, "{told:#b}");
    let second = idle(&mut region);
    let now = record(&region, start.key).expect("alive");
    assert_eq!((data_of(&now).2, now.fire_ticks), (2, 160), "{now:?}");
    let told = tracked_in(&second, start.key).expect("told it burns");
    assert!(told.flags & Tracked::ON_FIRE != 0, "{told:?}");
    let third = idle(&mut region);
    assert!(record(&region, start.key).is_none());
    assert!(gone_in(&third, start.key));
}

/// The brief, 3.2 and 3.3: an item that burns out of fire is hurt by 1 whenever its
/// fire's ticks are a multiple of 20, its fire counting down by one a tick: laid with
/// 100 ticks of fire and five health, it is hurt at 100, 80, 60, 40 and 20, and gone
/// in the 81st tick; it is shown alight all the while.
#[test]
fn a_burning_item_loses_a_health_every_twenty_ticks_until_it_is_gone() {
    let mut start = item(91, [8.5, FLOOR, 8.5], [0.0; 3], true);
    start.fire_ticks = 100;
    let (mut region, _) = scene(&[], vec![start.clone()]);
    for tick in 1..=81 {
        if tick > 1 {
            let output = idle(&mut region);
            if tick == 81 {
                assert!(gone_in(&output, start.key));
                break;
            }
        }
        let now = record(&region, start.key).expect("burning");
        let hurts = (tick + 19) / 20;
        assert_eq!(
            i32::from(data_of(&now).2),
            5 - hurts,
            "tick {tick}: {now:?}"
        );
        assert_eq!(now.fire_ticks, 100 - tick, "tick {tick}");
    }
    assert!(record(&region, start.key).is_none());
}

/// The brief, 3.1 (`moveTowardsClosestSpace`): an item inside a block is pushed out of
/// it, without collision, towards the nearest side that is not a full block, by the
/// entity's own draw of a speed in [0.1, 0.3) (a float), and three quarters of its
/// motion on the other axes; out in the air it falls back onto what it came out of.
#[test]
fn an_item_inside_a_block_is_pushed_out_to_the_nearest_open_side_and_lands_on_it() {
    // In the floor, whose sides are all stone: out the top.
    let start = item(92, [8.5, -60.8, 8.5], [0.0; 3], false);
    let (mut region, _) = scene(&[], vec![start.clone()]);
    let now = record(&region, start.key).expect("there");
    let rise = now.position[1] - start.position[1];
    assert!((0.1..0.3).contains(&rise), "pushed up by {rise}: {now:?}");
    // The speed is a float, the move made whole without collision, and then the air's
    // drag taken from the motion.
    let speed = (now.motion[1] / drag()) as f32;
    assert_eq!(f64::from(speed) * drag(), now.motion[1], "{now:?}");
    assert_eq!(
        start.position[1] + f64::from(speed),
        now.position[1],
        "{now:?}"
    );
    assert_eq!([now.motion[0], now.motion[2]], [0.0, 0.0]);
    assert_eq!([now.position[0], now.position[2]], [8.5, 8.5]);
    for _ in 0..40 {
        idle(&mut region);
    }
    // Whether one push takes it clear of the floor depends on the draw: a box still in
    // the floor after the second tick has its middle in the air above, whose nearest open
    // side is north (half a block, the first of the ties), and is pushed that way.
    let rested = record(&region, start.key).expect("there");
    assert_eq!(rested.position[..2], [8.5, FLOOR], "{rested:?}");
    assert!(
        (7.5..=8.5).contains(&rested.position[2]),
        "out north if anywhere: {rested:?}"
    );
    assert!(rested.on_ground);
}

/// Step N3d, in place of N4's guard: an item pushed out of a block towards a pinned
/// line, the only open side, goes across it and is sent, once; until then it is in the
/// region's chunk, once.
#[test]
fn an_item_pushed_out_of_a_block_towards_a_pinned_line_is_sent_across_it_once() {
    let mut origin = floor_with(&[
        (15, -60, 8, blocks::STONE),
        (15, -60, 7, blocks::STONE),
        (15, -60, 9, blocks::STONE),
        (14, -60, 8, blocks::STONE),
        (15, -59, 8, blocks::STONE),
    ]);
    let inside = item(93, [15.5, -59.8, 8.5], [0.0; 3], false);
    origin.set_entities(vec![inside.clone()]);
    let (mut region, _) = at_a_line(origin);
    let mut outputs = Vec::new();
    for tick in 0..100 {
        outputs.push(idle(&mut region));
        let found = records(&region, inside.key);
        if !sent(&outputs, inside.key).is_empty() {
            assert!(found.is_empty(), "tick {tick}: {found:?}");
            continue;
        }
        let [(chunk, now)] = &found[..] else {
            panic!("tick {tick}: once: {found:?}");
        };
        assert_eq!(*chunk, ORIGIN, "tick {tick}");
        assert!(now.position[0] < 16.0, "tick {tick}: {now:?}");
    }
    let arrivals = sent(&outputs, inside.key);
    let [(_, chunk, whole)] = &arrivals[..] else {
        panic!("once: {arrivals:?}");
    };
    assert_eq!(*chunk, ChunkPos::new(1, 0));
    assert!(whole.position[0] >= 16.0, "{whole:?}");
}

/// ADR-0028, section 3: an item that slides into another chunk of the region is gone
/// from the one and whole in the other in the tick it crosses, and is never in both.
#[test]
fn an_item_that_slides_into_another_chunk_of_the_region_is_gone_from_one_and_whole_in_the_other() {
    let start = item(94, [15.3, FLOOR, 8.5], [0.4, 0.0, 0.0], true);
    let (mut region, load) = scene(&[], vec![start.clone()]);
    let east = ChunkPos::new(1, 0);
    let mut output = load;
    let mut hand = Hand::of(&start);
    let mut crossed = false;
    for count in 0..30 {
        let before = hand.p[0];
        item_tick(&mut hand, output.tick, 94, &STONE_GROUND);
        let found = records(&region, start.key);
        let [(chunk, now)] = &found[..] else {
            panic!("tick {count}: once: {found:?}");
        };
        assert_is(now, &hand, &format!("tick {count}"));
        let expected = ChunkPos::containing(hand.p[0], hand.p[2]);
        assert_eq!(*chunk, expected);
        if before < 16.0 && hand.p[0] >= 16.0 {
            crossed = true;
            assert!(
                output
                    .entity_changes
                    .contains(&(ORIGIN, EntityChange::Gone(start.key)))
            );
            assert!(output.entity_changes.iter().any(|(at, change)| *at == east
                && matches!(change, EntityChange::Whole(entity) if entity.key == start.key)));
        }
        output = idle(&mut region);
    }
    assert!(crossed);
}

/// The brief, 3.1: an item that has moved into another block looks for others to
/// merge with every second tick (`tickCount % 2`), one that has not every fortieth:
/// two items falling side by side that cross into the block below on an even tick
/// merge in that tick; crossing on an odd one, they wait for the fortieth.
#[test]
fn falling_items_that_cross_into_another_block_merge_on_an_even_tick() {
    for (from, merges_in) in [(-58.98, None), (-58.95, Some(40))] {
        let mut first = item_of(95, [8.5, from, 8.5], stack(items::DIRT, 2), 100, 10);
        let mut second = item_of(96, [9.2, from, 8.5], stack(items::DIRT, 2), 100, 10);
        first.on_ground = false;
        second.on_ground = false;
        let (mut region, load) = scene(&[], vec![first.clone(), second.clone()]);
        // The load's tick, 4, is even; in it they fall 0.04, from -58.98 into the block
        // below and from -58.95 not.
        assert_eq!(load.tick, 4);
        let Some(tick) = merges_in else {
            assert!(
                gone_in(&load, first.key),
                "merged in the tick it crossed: {:?}",
                load.events
            );
            assert_eq!(
                data_of(&record(&region, second.key).expect("took it")).0,
                stack(items::DIRT, 4)
            );
            continue;
        };
        assert!(!gone_in(&load, first.key));
        let mut output = load;
        while output.tick < tick {
            output = idle(&mut region);
            if output.tick < tick {
                assert!(!gone_in(&output, first.key), "tick {}", output.tick);
            }
        }
        assert!(gone_in(&output, first.key), "merged in tick {tick}");
    }
}

/// The brief, 8.1 (`ServerPlayerGameMode.destroyBlock`): what a creative player breaks
/// drops nothing, a torch, leaves or sand alike, nor do the blocks around that stay.
#[test]
fn what_a_creative_player_breaks_drops_nothing() {
    let leaves = state("minecraft:oak_leaves[distance=1,persistent=true,waterlogged=false]");
    let placed = [
        (2, -60, 2, blocks::TORCH),
        (3, -60, 1, leaves),
        (1, -60, 3, blocks::SAND),
    ];
    let (mut region, _) = scene(&placed, Vec::new());
    let me = collector(&region);
    let digs = [(2, -60, 2), (3, -60, 1), (1, -60, 3)]
        .into_iter()
        .zip(1..)
        .map(|((x, y, z), number)| {
            let dig = PlayerInput::Dig {
                position: BlockPos::new(x, y, z),
                sequence: number,
            };
            (
                EDGE,
                player(1),
                me,
                u64::try_from(number).expect("small"),
                dig,
            )
        })
        .collect();
    let output = region.tick(&TickInputs {
        inputs: digs,
        ..TickInputs::default()
    });
    for (x, y, z) in [(2, -60, 2), (3, -60, 1), (1, -60, 3)] {
        assert_eq!(block_at(&region, x, y, z), blocks::AIR, "{x} {y} {z}");
    }
    let mut made = spawned(&output);
    for _ in 0..20 {
        made.extend(spawned(&idle(&mut region)));
    }
    assert!(made.is_empty(), "{made:?}");
}
