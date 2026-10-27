//! Tests of entities crossing a line in the sim (step N3d), written from
//! `docs/adr/0028-entities.md`, sections 5 to 7, and `docs/groundwork/n3-plan.md` (its
//! sections 5 and 6 over the rest), driving the region by its public interface alone, by
//! someone who did not write the code.
//!
//! What the record asks of a region: an entity whose move ends in a chunk the region does
//! not have loaded is **sent**, wherever that chunk is (another region's, lent or being
//! lent, the region's own and not loaded, asked for, unknown): `Gone` in its chunk's
//! commit, whole in the tick's arrivals with the `hops` it had, reported handed on with
//! its pose at the line, and never held at the line. Into a loaded chunk of the region's
//! own that does not tick it moves in and stops; into one that is full, N4's guard holds
//! it. An arrival handed in for a loaded chunk is taken after the effects and before the
//! entities tick, whole with `hops` one higher, reported spawned before its moves, and
//! ticks in that very tick; it is taken into a full chunk all the same; one whose key the
//! region has already is counted taken and the region keeps its own. A throw into a chunk
//! the region does not tick goes as an arrival with no `hops` and no handing on. A chunk
//! an arrival waits for is loaded with an arrival ticket, which ends a hundred ticks after
//! the taking; one held for an arrival alone is given back then.
//!
//! The numbers a test expects are worked out here, by a small model of the game's
//! `ItemEntity.tick` over a flat stone floor (as `physics_specification.rs` has it,
//! written from the N4 brief), never by calling the code under test.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_sim::api::{HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, RegionEvent};
use clustine_sim::{
    EdgeState, Entered, Holdings, Knowledge, PlayerChange, PlayerJoin, Region, RegionConfig,
    RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Arrival, Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Due, EdgeId, Effect, EffectId, Effected,
    Entity, EntityChange, EntityData, EntityId, EntityIds, FiledEffect, MAX_CHUNK_ENTITIES,
    PlayerId, RegionId, Section, Vec3,
};
use uuid::Uuid;

const EDGE: EdgeId = EdgeId(7);
const ORIGIN: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);
const FEET: Vec3 = Vec3::new(0.5, -60.0, 0.5);
const NEIGHBOUR: RegionId = RegionId(2);

/// The top of the stone floor.
const FLOOR: f64 = -60.0;

// ------------------------------------------------------------------------------------
// The world.

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn config(return_after: u64) -> RegionConfig {
    let mut hotbar = [const { None }; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack::new(items::STONE, 64));
    RegionConfig {
        spawn: FEET,
        starting_inventory: Inventory::with_hotbar(hotbar),
        return_after,
        lowest_y: -64,
        seed: 0,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
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

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

fn west(chunks: Vec<ChunkPos>) -> Vec<ChunkPos> {
    chunks.into_iter().filter(|chunk| chunk.x <= 0).collect()
}

fn east(chunks: Vec<ChunkPos>) -> Vec<ChunkPos> {
    chunks.into_iter().filter(|chunk| chunk.x >= 1).collect()
}

/// Pinned west of chunk x = 1.
const WEST_OF_THE_LINE: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};

/// Pinned from chunk x = 1 east.
const EAST_OF_THE_LINE: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// How a scene is laid out: what the region is pinned to and holds, which of its
/// chunks it loads (with a viewer's ticket each) and which chunks it has copies of.
struct Layout {
    pinned: ChunkArea,
    held: Vec<ChunkPos>,
    loaded: Vec<ChunkPos>,
    copies: Vec<ChunkPos>,
    block: u32,
    return_after: u64,
}

impl Layout {
    /// Pinned west of the line, holding its side, loading chunks x = -2 to 0 and
    /// copying x = 1 and 2: chunk (0, 0) ticks, and (1, 0) is nobody the region knows.
    fn west_of_a_line() -> Self {
        Self {
            pinned: WEST_OF_THE_LINE,
            held: west(around(3)),
            loaded: west(around(2)),
            copies: east(around(2)),
            block: 1,
            return_after: 0,
        }
    }

    /// Pinned everywhere and holding the chunks out to 3, but loading only those west
    /// of x = 1, with copies beyond: chunk (1, 0) is the region's own and not loaded.
    fn own_land_not_loaded() -> Self {
        Self {
            pinned: ChunkArea::EVERYWHERE,
            held: around(3),
            ..Self::west_of_a_line()
        }
    }

    /// Pinned everywhere, holding only the chunks west of x = 1: chunk (1, 0) is of its
    /// pinned land, not held and not lent away, as `Unknown`.
    fn pinned_land_not_held() -> Self {
        Self {
            pinned: ChunkArea::EVERYWHERE,
            held: west(around(3)),
            ..Self::west_of_a_line()
        }
    }

    /// Pinned everywhere, holding and loading every chunk out to 2: only chunk (0, 0)
    /// ticks entities, as only its 5x5 is all loaded; (1, 0) is loaded and does not.
    fn all_loaded() -> Self {
        Self {
            pinned: ChunkArea::EVERYWHERE,
            held: around(3),
            loaded: around(2),
            copies: Vec::new(),
            block: 1,
            return_after: 0,
        }
    }

    /// Holding and loading every chunk out to 3, so that chunks (0, 0) and (1, 0) both
    /// tick: one region over both sides of the line, as the game's one world is.
    fn one_region_over_the_line() -> Self {
        Self {
            pinned: ChunkArea::EVERYWHERE,
            held: around(4),
            loaded: around(3),
            copies: Vec::new(),
            block: 3,
            return_after: 0,
        }
    }

    /// Two regions coupled at the line x = 16, each holding and loading out to 3 on its
    /// side, with copies of the other's: chunk (0, 0) ticks in the west, (1, 0) in the
    /// east.
    fn coupled(side: ChunkArea) -> Self {
        let held: Vec<ChunkPos> = around(4)
            .into_iter()
            .filter(|chunk| side.contains(*chunk))
            .collect();
        let loaded: Vec<ChunkPos> = around(3)
            .into_iter()
            .filter(|chunk| side.contains(*chunk))
            .collect();
        let copies: Vec<ChunkPos> = around(3)
            .into_iter()
            .filter(|chunk| !side.contains(*chunk))
            .collect();
        Self {
            pinned: side,
            held,
            loaded,
            copies,
            block: if side == WEST_OF_THE_LINE { 1 } else { 2 },
            return_after: 0,
        }
    }
}

/// The region of `layout`, its chunks loaded with `entities` in theirs, and the output
/// of the tick of the load, in which those entities are taken in and tick first.
fn scene(layout: &Layout, entities: &[Entity]) -> (Region, TickOutput) {
    let mut state = RegionState::new(EntityIds::block(layout.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let holdings = Holdings {
        held: layout.held.clone(),
        pinned: vec![layout.pinned],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let mut region = Region::restore(config(layout.return_after), state, holdings);
    region.tick(&TickInputs {
        tickets_added: layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    let loaded = layout
        .loaded
        .iter()
        .map(|position| {
            let mut chunk = floor();
            chunk.set_entities(
                entities
                    .iter()
                    .filter(|entity| entity.chunk() == *position)
                    .cloned()
                    .collect(),
            );
            (*position, chunk)
        })
        .collect();
    let output = region.tick(&TickInputs {
        chunks_loaded: loaded,
        copy_bases: layout
            .copies
            .iter()
            .map(|chunk| (*chunk, floor()))
            .collect(),
        ..TickInputs::default()
    });
    for chunk in &layout.loaded {
        assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
    }
    (region, output)
}

/// Player 1 joins and enters the world at the spawn.
fn joined(region: &mut Region) -> EntityId {
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
    region.player(player(1)).expect("player 1 entered").0
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// The key of item number `n`, from a block no region of these tests is given.
fn key(n: u32) -> EntityId {
    EntityId((9 << 32) + u64::from(n))
}

/// An item of one dirt at `position` with `motion`, never to be picked up, with `hops`
/// changes of holder behind it.
fn item(n: u32, position: [f64; 3], motion: [f64; 3], hops: u32) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(entity_types::ITEM).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Item {
            stack: ItemStack::new(items::DIRT, 1),
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

/// An item lying on the floor at `position` with `motion` along it.
fn lying(n: u32, position: [f64; 3], motion: [f64; 3]) -> Entity {
    Entity {
        on_ground: true,
        ..item(n, position, motion, 0)
    }
}

/// An item flying east in the air of chunk (0, 0), which crosses the line at x = 16 a
/// few ticks after its load and lands well beyond it.
fn flying_east(n: u32, hops: u32) -> Entity {
    item(n, [13.5, -56.0, 8.5], [0.45, 0.2, 0.0], hops)
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

/// What the edges are told of the entity `key` in `output`.
fn reports_of(output: &TickOutput, key: EntityId) -> Vec<RegionEvent> {
    output
        .events
        .iter()
        .filter(|event| match event {
            RegionEvent::EntityTracked(tracked) => tracked.entity == key,
            RegionEvent::EntitySpawned(state) => state.entity == key,
            RegionEvent::EntityGone { entity, .. } | RegionEvent::HandedOn { entity, .. } => {
                *entity == key
            }
            _ => false,
        })
        .cloned()
        .collect()
}

/// The arrivals of `output` of the entity `key`.
fn sent_of(output: &TickOutput, key: EntityId) -> Vec<(ChunkPos, Entity)> {
    output
        .arrivals
        .iter()
        .filter(|(_, entity)| entity.key == key)
        .cloned()
        .collect()
}

/// The arrival numbered `index` of `entity` into its chunk.
fn arrival(index: u32, entity: &Entity) -> Arrival {
    Arrival {
        id: EffectId {
            sequence: 40,
            index,
        },
        chunk: entity.chunk(),
        entity: entity.clone(),
        due: Due::FromTaking,
    }
}

fn arriving(arrivals: Vec<Arrival>) -> TickInputs {
    TickInputs {
        arrivals,
        ..TickInputs::default()
    }
}

// ------------------------------------------------------------------------------------
// The model: the game's item over a flat stone floor (the N4 brief's sections 1 and 3).

const GRAVITY: f64 = 0.04;
/// `1.0E-5F` widened.
const TOLERANCE: f64 = 9.999_999_747_378_752e-6;

fn drag() -> f64 {
    f64::from(0.98_f32)
}

fn stone_drag() -> f64 {
    f64::from(0.98_f32 * 0.6_f32)
}

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

/// `Entity.move(SELF, v)` over the floor, for an item's box 0.25 high.
fn entity_move(hand: &mut Hand) {
    let vec = hand.v;
    let y = hand.p[1];
    let height = 0.25;
    let present = y + vec[1].min(0.0) < FLOOR && y + height + vec[1].max(0.0) > FLOOR - 1.0;
    let mut out = vec;
    if present {
        if vec[1] != 0.0 {
            out[1] = if vec[1].abs() < 1e-7 {
                0.0
            } else if vec[1] < 0.0 {
                let e = FLOOR - y;
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
    if d5 > 1e-7 || length(vec) - d5 < 1e-7 {
        hand.p = [hand.p[0] + out[0], hand.p[1] + out[1], hand.p[2] + out[2]];
    }
    let vertical = vec[1] != out[1];
    let below = vertical && vec[1] < 0.0;
    hand.on_ground = below;
    if out[1] < 0.0 {
        hand.fall -= f64::from(out[1] as f32);
    }
    if hand.on_ground {
        hand.fall = 0.0;
    }
    if vec[1].abs() > 0.0 && vertical {
        // Stone does not bounce: the fall's motion goes.
        hand.v[1] = 0.0;
    }
}

/// `ItemEntity.tick` out of any fluid over the floor, `tick` being the region's tick
/// and `id` the item's game id.
fn item_tick(hand: &mut Hand, tick: u64, id: i32) {
    hand.v[1] += -GRAVITY;
    let slow = hand.v[0] * hand.v[0] + hand.v[2] * hand.v[2] <= TOLERANCE;
    let gate = (tick as i32).wrapping_add(id) % 4 == 0;
    if !hand.on_ground || !slow || gate {
        entity_move(hand);
        let f3 = if hand.on_ground { stone_drag() } else { drag() };
        hand.v = [hand.v[0] * f3, hand.v[1] * drag(), hand.v[2] * f3];
        if hand.on_ground && hand.v[1] < 0.0 {
            hand.v[1] *= -0.5;
        }
    }
}

fn bits(v: [f64; 3]) -> [u64; 3] {
    v.map(f64::to_bits)
}

/// The entity is where the model has it, to the bit.
fn assert_is(entity: &Entity, hand: &Hand, what: &str) {
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
}

// ------------------------------------------------------------------------------------
// Sending.

/// The model of `start` after the load's tick `load`, checked against the region's
/// record.
fn loaded(region: &Region, load: &TickOutput, start: &Entity) -> Hand {
    assert!(load.arrivals.is_empty(), "sent in the load's tick");
    let mut hand = Hand::of(start);
    item_tick(&mut hand, load.tick, start.key.0 as u32 as i32);
    let now = record(region, start.key).expect("taken in at the load");
    assert_is(&now, &hand, "the load's tick");
    hand
}

/// One tick of `region` with `inputs`, the model of `key` ticked with it and checked,
/// unless the tick sent it.
fn along(region: &mut Region, hand: &mut Hand, key: EntityId, inputs: &TickInputs) -> TickOutput {
    let output = region.tick(inputs);
    item_tick(hand, output.tick, key.0 as u32 as i32);
    if sent_of(&output, key).is_empty() {
        let now = record(region, key)
            .unwrap_or_else(|| panic!("tick {}: neither here nor sent", output.tick));
        assert_is(&now, hand, &format!("tick {}", output.tick));
        assert!(
            now.position[0] < 16.0,
            "tick {}: past the line: {now:?}",
            output.tick
        );
    }
    output
}

/// Ticks `region` until it sends `key`, at most `ticks` times, checking the item
/// against the model `hand` as it goes; returns the output of the tick that sent it and
/// the model after it.
fn until_sent(
    region: &mut Region,
    mut hand: Hand,
    key: EntityId,
    ticks: usize,
    inputs: impl Fn(u64) -> TickInputs,
) -> (TickOutput, Hand) {
    for _ in 0..ticks {
        let next = region.tick_number() + 1;
        let output = along(region, &mut hand, key, &inputs(next));
        if !sent_of(&output, key).is_empty() {
            return (output, hand);
        }
    }
    panic!("not sent in {ticks} ticks");
}

/// What the record asks of a sending, whatever the chunk it goes to: in the tick it
/// crosses, its chunk commits it `Gone` and nothing else of it; the tick hands it on
/// once, whole, as the game would have it after its move, with the `hops` it had; the
/// edges are told it is handed on, with that pose, and not that it is gone; and the
/// region has it nowhere from then on, nor sends it again.
fn assert_sent_once(region: &mut Region, sending: &TickOutput, hand: &Hand, start: &Entity) {
    let sent = sent_of(sending, start.key);
    let [(to, whole)] = &sent[..] else {
        panic!("once: {sent:?}");
    };
    assert_eq!(*to, EAST);
    assert_eq!(whole.chunk(), EAST);
    assert_is(whole, hand, "the arrival");
    // The load that took it in raised its hops by one (the record, section 6); the
    // sending keeps them, and the taking raises them.
    let had = start.hops + 1;
    assert_eq!(whole.hops, had, "the hops it had");
    assert_eq!(whole.data, start.data);
    assert_eq!(whole.uuid, start.uuid);
    let changes: Vec<&(ChunkPos, EntityChange)> = sending
        .entity_changes
        .iter()
        .filter(|(_, change)| match change {
            EntityChange::Gone(key) => *key == start.key,
            EntityChange::Whole(entity) => entity.key == start.key,
            EntityChange::Set(key, _) => *key == start.key,
        })
        .collect();
    assert_eq!(changes, [&(ORIGIN, EntityChange::Gone(start.key))]);
    assert_eq!(
        reports_of(sending, start.key),
        [RegionEvent::HandedOn {
            entity: start.key,
            chunk: ORIGIN,
            hops: had,
            position: whole.position,
            motion: whole.motion,
        }]
    );
    assert!(records(region, start.key).is_empty());
    for _ in 0..40 {
        let output = idle(region);
        assert!(sent_of(&output, start.key).is_empty(), "sent again");
        assert!(reports_of(&output, start.key).is_empty());
        assert!(records(region, start.key).is_empty());
    }
}

/// The record, section 5: into another region's chunk, as the store has said it is.
#[test]
fn an_item_flying_into_another_regions_chunk_is_sent_whole_with_its_hops_and_handed_on() {
    let start = flying_east(1, 2);
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    let mut hand = loaded(&region, &load, &start);
    along(
        &mut region,
        &mut hand,
        start.key,
        &TickInputs {
            foreign: vec![(EAST, NEIGHBOUR)],
            tickets_added: vec![(EAST, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(region.knowledge(EAST), Knowledge::Foreign(NEIGHBOUR));
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |_| TickInputs::default());
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C1): into a chunk nobody the region knows of, which it does not
/// want and is not pinned to.
#[test]
fn an_item_flying_into_a_chunk_the_region_knows_nothing_of_is_sent() {
    let start = flying_east(2, 0);
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    assert_eq!(region.knowledge(EAST), Knowledge::Unknown);
    let hand = loaded(&region, &load, &start);
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |_| TickInputs::default());
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C1, C2): into the region's own chunk that it holds and has not
/// loaded; the store files the arrival for the region itself.
#[test]
fn an_item_flying_into_the_regions_own_chunk_not_loaded_is_sent_not_held_at_the_line() {
    let start = flying_east(3, 5);
    let (mut region, load) = scene(&Layout::own_land_not_loaded(), std::slice::from_ref(&start));
    assert_eq!(region.knowledge(EAST), Knowledge::Held);
    assert!(region.chunk(EAST).is_none());
    let hand = loaded(&region, &load, &start);
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |_| TickInputs::default());
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C2): into a chunk of the region's pinned land that it does not
/// hold and has not lent away (`Unknown`, pinned).
#[test]
fn an_item_flying_into_pinned_land_the_region_does_not_hold_is_sent() {
    let start = flying_east(4, 1);
    let (mut region, load) = scene(
        &Layout::pinned_land_not_held(),
        std::slice::from_ref(&start),
    );
    assert_eq!(region.knowledge(EAST), Knowledge::Unknown);
    assert!(region.pins(EAST));
    let hand = loaded(&region, &load, &start);
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |_| TickInputs::default());
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C1): into a chunk the region has asked for and not been
/// granted: sent all the same, never held at the line until the answer.
#[test]
fn an_item_flying_into_a_chunk_asked_for_and_not_granted_is_sent() {
    let start = flying_east(5, 0);
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    let mut hand = loaded(&region, &load, &start);
    // A viewer wants the chunk: the region claims it, and has no answer.
    let output = along(
        &mut region,
        &mut hand,
        start.key,
        &TickInputs {
            tickets_added: vec![(EAST, Ticket::Viewer)],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.claims, [EAST]);
    assert_eq!(region.knowledge(EAST), Knowledge::Asked);
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |_| TickInputs::default());
    assert_eq!(region.knowledge(EAST), Knowledge::Asked);
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C1): into a chunk the region knows nothing of, not pinned, that
/// a viewer comes to want in the very tick the item crosses: not yet asked for, and
/// sent.
#[test]
fn an_item_flying_into_a_chunk_wanted_in_that_very_tick_is_sent() {
    let start = flying_east(6, 0);
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    let hand = loaded(&region, &load, &start);
    // When it crosses, from a run of its own.
    let crossing = {
        let mut trial = region.clone();
        let (sending, _) = until_sent(&mut trial, hand, start.key, 30, |_| TickInputs::default());
        sending.tick
    };
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |tick| {
        if tick == crossing {
            TickInputs {
                tickets_added: vec![(EAST, Ticket::Viewer)],
                ..TickInputs::default()
            }
        } else {
            TickInputs::default()
        }
    });
    assert_eq!(sending.tick, crossing);
    assert_eq!(sending.claims, [EAST], "wanted and unknown in that tick");
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The record, section 5: into a chunk the region is lending, which it has let go of.
#[test]
fn an_item_flying_into_a_chunk_the_region_lends_is_sent() {
    let start = flying_east(7, 3);
    let layout = Layout {
        loaded: around(2),
        copies: Vec::new(),
        ..Layout::own_land_not_loaded()
    };
    let (mut region, load) = scene(&layout, std::slice::from_ref(&start));
    assert!(region.chunk(EAST).is_some());
    let mut hand = loaded(&region, &load, &start);
    let asked = along(
        &mut region,
        &mut hand,
        start.key,
        &TickInputs {
            lends_asked: vec![clustine_sim::LendAsked {
                lend: 1,
                chunks: vec![EAST],
                to: NEIGHBOUR,
            }],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        asked.lend_answers,
        [clustine_sim::LendAnswer::Lending {
            lend: 1,
            chunks: vec![EAST],
        }],
        "the region lends the chunk"
    );
    assert!(region.lending_away(EAST));
    // The chunk lent away is dropped at the start of the next tick, and the runner
    // hands in its copy, so that (0, 0) goes on ticking beside it.
    let first = region.tick_number() + 1;
    let (sending, hand) = until_sent(&mut region, hand, start.key, 30, |tick| {
        if tick == first {
            TickInputs {
                copy_bases: vec![(EAST, floor())],
                ..TickInputs::default()
            }
        } else {
            TickInputs::default()
        }
    });
    assert!(region.chunk(EAST).is_none(), "let go of for the lend");
    assert_sent_once(&mut region, &sending, &hand, &start);
}

/// The plan, section 5 (C2): into a loaded chunk of the region's own that does not
/// tick, an entity moves in and stops ticking, as the game's do; nothing is sent.
#[test]
fn an_item_sliding_into_a_loaded_chunk_of_its_own_that_does_not_tick_moves_in_and_stops() {
    let start = lying(8, [31.9, FLOOR, 8.5], [0.4, 0.0, 0.0]);
    let (mut region, load) = scene(&Layout::all_loaded(), std::slice::from_ref(&start));
    // Chunk (1, 0) ticks, its neighbours loaded; (2, 0), at the edge of what is loaded,
    // does not. In the load's tick the item slides 0.4 east, into (2, 0).
    let (from, into) = (EAST, ChunkPos::new(2, 0));
    let mut hand = Hand::of(&start);
    item_tick(&mut hand, load.tick, start.key.0 as u32 as i32);
    assert!(load.arrivals.is_empty(), "{:?}", load.arrivals);
    let found = records(&region, start.key);
    let [(chunk, now)] = &found[..] else {
        panic!("once: {found:?}");
    };
    assert_eq!(*chunk, into);
    assert_is(now, &hand, "moved in");
    assert!(
        load.entity_changes
            .contains(&(from, EntityChange::Gone(start.key)))
    );
    assert!(
        load.entity_changes
            .iter()
            .any(|(chunk, change)| *chunk == into
                && matches!(change, EntityChange::Whole(entity) if entity.key == start.key))
    );
    let stopped = now.clone();
    for _ in 0..20 {
        let output = idle(&mut region);
        assert!(output.arrivals.is_empty());
        assert!(
            output
                .entity_changes
                .iter()
                .all(|(chunk, _)| *chunk != into),
            "{:?}",
            output.entity_changes
        );
    }
    assert_eq!(record(&region, start.key), Some(stopped));
}

/// The plan, section 5 (G1): into a loaded chunk of the region's own that is full, N4's
/// guard holds it back; nothing is sent.
#[test]
fn an_item_sliding_into_a_full_loaded_chunk_of_its_own_is_held_back_by_the_guard() {
    let start = lying(9, [15.9, FLOOR, 8.5], [0.4, 0.0, 0.0]);
    let mut entities: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u32)
        .map(|n| lying(1_000 + n, [24.5, FLOOR, 4.5], [0.0; 3]))
        .collect();
    entities.push(start.clone());
    let (mut region, load) = scene(&Layout::all_loaded(), &entities);
    assert!(load.arrivals.is_empty());
    let now = record(&region, start.key).expect("held here");
    assert_eq!(now.chunk(), ORIGIN);
    assert!(now.position[0] < 16.0, "{now:?}");
    for _ in 0..20 {
        assert!(idle(&mut region).arrivals.is_empty());
    }
    assert_eq!(record(&region, start.key).expect("here").chunk(), ORIGIN);
    assert_eq!(
        region.chunk(EAST).expect("loaded").entities().len(),
        MAX_CHUNK_ENTITIES
    );
}

/// The plan, section 5 (G1): pistons send from the block entities' phase, the `Gone`
/// and the arrival in that tick's commit. A piston at (13, -60, 2) facing east, powered
/// by a redstone block another region's rule sets beside it, pushes stone from x = 14
/// to 15 and the item lying at x = 15.5 before it over the line.
#[test]
fn an_item_a_piston_pushes_across_a_line_is_sent_in_that_ticks_commit() {
    let start = lying(10, [15.5, FLOOR, 2.5], [0.0; 3]);
    let (mut region, _) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    let set = |region: &mut Region, at: BlockPos, block: BlockState| {
        region.tick(&TickInputs {
            effects: vec![FiledEffect {
                id: EffectId {
                    sequence: 2,
                    index: at.x as u32,
                },
                effect: Effect {
                    at,
                    what: Effected::SetIf {
                        expected: blocks::AIR,
                        state: block,
                        flags: 3,
                    },
                },
                due: Due::FromTaking,
            }],
            ..TickInputs::default()
        })
    };
    // The piston and the stone, set with no update so that nothing moves yet, then the
    // power.
    set(
        &mut region,
        BlockPos::new(13, -60, 2),
        state("minecraft:piston[facing=east]"),
    );
    set(&mut region, BlockPos::new(14, -60, 2), blocks::STONE);
    let mut outputs = vec![set(
        &mut region,
        BlockPos::new(12, -60, 2),
        blocks::REDSTONE_BLOCK,
    )];
    for _ in 0..10 {
        outputs.push(idle(&mut region));
    }
    let sending: Vec<&TickOutput> = outputs
        .iter()
        .filter(|output| !sent_of(output, start.key).is_empty())
        .collect();
    let [sending] = &sending[..] else {
        panic!(
            "sent once: {:?}",
            outputs
                .iter()
                .map(|output| (output.tick, sent_of(output, start.key)))
                .collect::<Vec<_>>()
        );
    };
    let [(to, whole)] = &sent_of(sending, start.key)[..] else {
        unreachable!();
    };
    assert_eq!(*to, EAST);
    assert!(whole.position[0] > 16.0, "{whole:?}");
    assert_eq!(whole.hops, 1, "its load's");
    assert!(
        sending
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(start.key)))
    );
    assert!(sending.events.iter().any(|event| matches!(
        event,
        RegionEvent::HandedOn { entity, chunk: ORIGIN, hops: 1, position, .. }
            if *entity == start.key && *position == whole.position
    )));
    assert!(records(&region, start.key).is_empty());
    // It was pushed once before: there, and not sent, the tick before.
    assert!(
        outputs
            .iter()
            .filter(|output| output.tick < sending.tick)
            .all(|output| sent_of(output, start.key).is_empty())
    );
}

/// The record, section 5: a falling block that falls into another region's chunk is
/// sent before it lands; it lands there, not here.
#[test]
fn a_falling_block_falling_across_a_line_is_sent_and_lands_nothing_here() {
    let sand = Entity {
        kind: u16::try_from(entity_types::FALLING_BLOCK).expect("fits"),
        data: EntityData::FallingBlock {
            state: blocks::SAND.0,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        },
        ..item(11, [15.5, -57.0, 8.5], [0.3, 0.0, 0.0], 4)
    };
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&sand));
    let mut outputs = vec![load];
    for _ in 0..20 {
        outputs.push(idle(&mut region));
    }
    let sent: Vec<(u64, ChunkPos, Entity)> = outputs
        .iter()
        .flat_map(|output| {
            sent_of(output, sand.key)
                .into_iter()
                .map(|(chunk, entity)| (output.tick, chunk, entity))
        })
        .collect();
    let [(tick, to, whole)] = &sent[..] else {
        panic!("once: {sent:?}");
    };
    assert_eq!(*to, EAST);
    assert_eq!(whole.hops, 5, "four, and its load's");
    assert!(matches!(whole.data, EntityData::FallingBlock { .. }));
    for output in &outputs {
        if output.tick >= *tick {
            assert!(
                output.events.iter().all(|event| !matches!(
                    event,
                    RegionEvent::BlockChanged { position, .. } if position.chunk() == ORIGIN
                )),
                "landed here: {:?}",
                output.events
            );
        }
    }
    assert!(records(&region, sand.key).is_empty());
    let block = region
        .chunk(ORIGIN)
        .and_then(|chunk| chunk.get(15, -61, 8))
        .expect("loaded");
    assert_eq!(block, blocks::STONE);
}

// ------------------------------------------------------------------------------------
// Taking.

/// The record, sections 5 and 6: an arrival handed in for a chunk that ticks is taken
/// in that tick, whole with `hops` one higher, reported spawned before its moves, and
/// ticks in that very tick: after it, it is one tick of the game's on from where it was
/// sent.
#[test]
fn an_arrival_is_taken_whole_with_one_more_hop_and_ticks_in_the_tick_that_takes_it() {
    let (mut region, _) = scene(&Layout::all_loaded(), &[]);
    let coming = item(20, [8.5, -55.0, 8.5], [0.1, 0.05, -0.1], 4);
    let handed = arrival(3, &coming);
    let output = region.tick(&arriving(vec![handed.clone()]));
    assert_eq!(output.arrivals_taken, [handed.id]);
    let raised = Entity {
        hops: 5,
        ..coming.clone()
    };
    let changes: Vec<&(ChunkPos, EntityChange)> = output
        .entity_changes
        .iter()
        .filter(|(_, change)| match change {
            EntityChange::Whole(entity) => entity.key == coming.key,
            EntityChange::Set(key, _) | EntityChange::Gone(key) => *key == coming.key,
        })
        .collect();
    assert_eq!(
        changes.first(),
        Some(&&(ORIGIN, EntityChange::Whole(raised))),
        "whole, as it came, with one more hop: {changes:?}"
    );
    let mut hand = Hand::of(&coming);
    item_tick(&mut hand, output.tick, coming.key.0 as u32 as i32);
    let now = record(&region, coming.key).expect("taken");
    assert_is(&now, &hand, "ticked once in the tick that took it");
    assert_eq!(now.hops, 5);
    let reports = reports_of(&output, coming.key);
    let spawned = reports
        .iter()
        .position(|event| matches!(event, RegionEvent::EntitySpawned(_)))
        .expect("reported spawned");
    let moved = reports
        .iter()
        .position(|event| matches!(event, RegionEvent::EntityTracked(_)))
        .expect("its move reported");
    assert!(spawned < moved, "{reports:?}");
    assert!(
        reports
            .iter()
            .all(|event| !matches!(event, RegionEvent::HandedOn { .. }))
    );
    // Taken once: the next tick ticks it on and takes nothing.
    let output = idle(&mut region);
    assert!(output.arrivals_taken.is_empty());
    item_tick(&mut hand, output.tick, coming.key.0 as u32 as i32);
    assert_is(&record(&region, coming.key).expect("there"), &hand, "next");
}

/// The plan, section 5 (G1): an arrival cannot be refused; a full chunk takes it.
#[test]
fn an_arrival_into_a_full_chunk_is_taken_all_the_same() {
    let crowd: Vec<Entity> = (0..MAX_CHUNK_ENTITIES as u32)
        .map(|n| lying(2_000 + n, [4.5, FLOOR, 4.5], [0.0; 3]))
        .collect();
    let (mut region, _) = scene(&Layout::all_loaded(), &crowd);
    let coming = lying(21, [9.5, FLOOR, 9.5], [0.0; 3]);
    let handed = arrival(0, &coming);
    let output = region.tick(&arriving(vec![handed.clone()]));
    assert_eq!(output.arrivals_taken, [handed.id]);
    assert_eq!(record(&region, coming.key).expect("taken").hops, 1);
    assert_eq!(
        region.chunk(ORIGIN).expect("loaded").entities().len(),
        MAX_CHUNK_ENTITIES + 1
    );
}

/// The plan, section 6 (9): a key the holder has already is a malformed stream: the
/// region keeps its own entity and counts the arrival taken, so that the stream heals.
#[test]
fn an_arrival_whose_key_the_region_has_is_counted_taken_and_the_region_keeps_its_own() {
    let own = lying(22, [9.5, FLOOR, 9.5], [0.0; 3]);
    let (mut region, _) = scene(
        &Layout::all_loaded(),
        &[Entity {
            hops: 3,
            ..own.clone()
        }],
    );
    let other = Entity {
        position: [4.5, FLOOR, 4.5],
        hops: 7,
        ..own.clone()
    };
    let handed = arrival(5, &other);
    let output = region.tick(&arriving(vec![handed.clone()]));
    assert_eq!(output.arrivals_taken, [handed.id]);
    let found = records(&region, own.key);
    let [(_, kept)] = &found[..] else {
        panic!("once: {found:?}");
    };
    // Its own, as its load raised it, not the arrival's.
    assert_eq!(kept.hops, 4);
    assert_eq!(kept.position, own.position);
    assert!(
        reports_of(&output, own.key)
            .iter()
            .all(|event| !matches!(event, RegionEvent::EntitySpawned(_)))
    );
}

/// Arrivals are deduplicated by number, never by key (the record, section 5); but a
/// region never holds one key twice: two arrivals of one key in a tick (a malformed
/// stream) make one entity, and both are counted taken.
#[test]
fn two_arrivals_of_one_key_in_a_tick_make_one_entity_and_both_are_counted_taken() {
    let (mut region, _) = scene(&Layout::all_loaded(), &[]);
    let coming = lying(23, [9.5, FLOOR, 9.5], [0.0; 3]);
    let first = arrival(0, &coming);
    let second = arrival(
        1,
        &Entity {
            position: [3.5, FLOOR, 3.5],
            ..coming.clone()
        },
    );
    let output = region.tick(&arriving(vec![first.clone(), second.clone()]));
    assert_eq!(output.arrivals_taken, [first.id, second.id]);
    let found = records(&region, coming.key);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].1.position, coming.position, "the first is taken");
}

/// The record, section 5: an arrival is ready once its chunk is loaded, with no need
/// of its 3x3; into a loaded chunk that does not tick it is taken, with one more hop,
/// and stands still there, as the game's entities do in a chunk that does not tick.
#[test]
fn an_arrival_into_a_loaded_chunk_that_does_not_tick_is_taken_and_stands_still() {
    let (mut region, _) = scene(&Layout::all_loaded(), &[]);
    let far = ChunkPos::new(2, 0);
    let coming = item(25, [40.5, -55.0, 8.5], [0.2, 0.0, 0.0], 2);
    assert_eq!(coming.chunk(), far);
    let handed = arrival(0, &coming);
    let output = region.tick(&arriving(vec![handed.clone()]));
    assert_eq!(output.arrivals_taken, [handed.id]);
    assert!(output.entity_changes.contains(&(
        far,
        EntityChange::Whole(Entity {
            hops: 3,
            ..coming.clone()
        })
    )));
    for _ in 0..10 {
        idle(&mut region);
    }
    let now = record(&region, coming.key).expect("taken");
    assert_eq!(now.position, coming.position, "it does not tick there");
    assert_eq!(now.hops, 3);
}

/// The record, section 5: an arrival is ready once its chunk is loaded; one handed in
/// for a chunk not loaded is neither taken nor counted taken, and waits.
#[test]
fn an_arrival_for_a_chunk_not_loaded_is_not_taken_nor_counted_taken() {
    let (mut region, _) = scene(&Layout::own_land_not_loaded(), &[]);
    let coming = lying(24, [24.5, FLOOR, 8.5], [0.0; 3]);
    assert_eq!(coming.chunk(), EAST);
    let output = region.tick(&arriving(vec![arrival(0, &coming)]));
    assert!(output.arrivals_taken.is_empty());
    assert!(
        output
            .entity_changes
            .iter()
            .all(|(_, change)| !matches!(change, EntityChange::Whole(_)))
    );
    assert!(reports_of(&output, coming.key).is_empty());
}

// ------------------------------------------------------------------------------------
// Coupled.

/// The record, section 5, "While coupled", and the plan, section 6 (D4): the holder
/// runs border time t + 1 once the sender is durable through t, and takes the arrival
/// in it. An item flying across the line between two coupled regions is, tick for
/// tick, where it would be in one region over both chunks: sent in the tick it
/// crosses, taken in the next and ticked in it, neither ticked twice nor skipped.
#[test]
fn an_item_flying_across_a_coupled_line_ticks_once_a_tick_as_in_one_region() {
    let start = flying_east(30, 0);
    let (mut one, _) = scene(
        &Layout::one_region_over_the_line(),
        std::slice::from_ref(&start),
    );
    let (mut west, _) = scene(
        &Layout::coupled(WEST_OF_THE_LINE),
        std::slice::from_ref(&start),
    );
    let (mut east, _) = scene(&Layout::coupled(EAST_OF_THE_LINE), &[]);
    assert_eq!(one.tick_number(), west.tick_number());
    assert_eq!(west.tick_number(), east.tick_number());
    let mut pending: Vec<Arrival> = Vec::new();
    let mut sent_at = None;
    let mut taken_at = None;
    for _ in 0..40 {
        let reference = idle(&mut one);
        let from_west = idle(&mut west);
        let into_east = east.tick(&arriving(std::mem::take(&mut pending)));
        assert_eq!(reference.tick, from_west.tick);
        assert_eq!(reference.tick, into_east.tick);
        let tick = reference.tick;
        let game = record(&one, start.key).expect("in the one region");
        if !into_east.arrivals_taken.is_empty() {
            taken_at = Some(tick);
        }
        for (index, (chunk, entity)) in from_west.arrivals.iter().enumerate() {
            assert_eq!(entity.key, start.key);
            sent_at = Some(tick);
            // Sent as the game has it after its move in that tick.
            assert_eq!(bits(entity.position), bits(game.position), "tick {tick}");
            assert_eq!(bits(entity.motion), bits(game.motion), "tick {tick}");
            pending.push(Arrival {
                id: EffectId {
                    sequence: tick,
                    index: index as u32,
                },
                chunk: *chunk,
                entity: entity.clone(),
                due: Due::At { tick: tick + 1 },
            });
        }
        let here = record(&west, start.key);
        let there = record(&east, start.key);
        match (here, there) {
            (Some(entity), None) | (None, Some(entity)) => {
                assert_eq!(bits(entity.position), bits(game.position), "tick {tick}");
                assert_eq!(bits(entity.motion), bits(game.motion), "tick {tick}");
                assert_eq!(entity.on_ground, game.on_ground, "tick {tick}");
            }
            (None, None) => assert_eq!(sent_at, Some(tick), "tick {tick}: lost"),
            (Some(a), Some(b)) => panic!("tick {tick}: twice: {a:?} {b:?}"),
        }
    }
    let sent_at = sent_at.expect("it crossed");
    assert_eq!(taken_at, Some(sent_at + 1));
    let there = record(&east, start.key).expect("in the east");
    assert_eq!(there.hops, 2, "its load's in the west and the taking's");
    assert!(there.on_ground, "landed beyond the line: {there:?}");
}

// ------------------------------------------------------------------------------------
// Throwing.

fn look_east_and_throw(
    entity: EntityId,
    first: u64,
) -> Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)> {
    vec![
        (
            EDGE,
            player(1),
            entity,
            first,
            PlayerInput::Move {
                position: None,
                rotation: Some((-90.0, 0.0)),
                on_ground: true,
            },
        ),
        (
            EDGE,
            player(1),
            entity,
            first + 1,
            PlayerInput::Drop {
                all: false,
                crouching: false,
            },
        ),
    ]
}

/// The plan, section 1 and section 5 (G2): a Q pressed right after crossing into land
/// the region does not tick (its own pinned land, asked for and not granted yet)
/// throws: the item goes as an arrival into that chunk as it is made, with `hops` 0,
/// no room asked for and nothing reported handed on; it appears where it is taken.
#[test]
fn a_q_pressed_right_after_crossing_into_land_not_ticked_throws_as_an_arrival() {
    let (mut region, _) = scene(&Layout::pinned_land_not_held(), &[]);
    let me = joined(&mut region);
    let walked = region.tick(&TickInputs {
        inputs: vec![(
            EDGE,
            player(1),
            me,
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(17.5, FLOOR, 8.5)),
                rotation: None,
                on_ground: true,
            },
        )],
        ..TickInputs::default()
    });
    assert_eq!(walked.claims, [EAST], "the chunk stood in is claimed");
    assert!(region.player(player(1)).is_some(), "still the region's");
    let output = region.tick(&TickInputs {
        inputs: look_east_and_throw(me, 2),
        ..TickInputs::default()
    });
    let [(to, thrown)] = &output.arrivals[..] else {
        panic!("thrown as an arrival: {:?}", output.arrivals);
    };
    assert_eq!(*to, EAST);
    assert_eq!(thrown.chunk(), EAST);
    assert_eq!(thrown.hops, 0);
    let EntityData::Item {
        ref stack, thrower, ..
    } = thrown.data
    else {
        panic!("{thrown:?}");
    };
    assert_eq!(stack.item, items::STONE);
    assert_eq!(stack.count, 1);
    assert_eq!(thrower, Some(player(1).0));
    assert!(
        reports_of(&output, thrown.key).is_empty(),
        "{:?}",
        output.events
    );
    assert!(
        output
            .entity_changes
            .iter()
            .all(|(_, change)| !matches!(change, EntityChange::Whole(_)))
    );
    // Thrown once: the stack in hand is one less.
    let held = &region
        .player_state(player(1))
        .expect("there")
        .inventory
        .hotbar()[0];
    assert_eq!(held.as_ref().map(|stack| stack.count), Some(63));
}

/// The plan, section 1: a thrown item that flies across a pinned line is the region's
/// until it crosses (spawned and ticked here), and is then sent once with the `hops` it
/// had, 0, and handed on.
#[test]
fn a_thrown_item_that_flies_across_a_pinned_line_is_sent_once_and_handed_on() {
    let (mut region, _) = scene(&Layout::west_of_a_line(), &[]);
    let me = joined(&mut region);
    region.tick(&TickInputs {
        inputs: vec![(
            EDGE,
            player(1),
            me,
            1,
            PlayerInput::Move {
                position: Some(Vec3::new(14.5, FLOOR, 8.5)),
                rotation: None,
                on_ground: true,
            },
        )],
        ..TickInputs::default()
    });
    let throw = region.tick(&TickInputs {
        inputs: look_east_and_throw(me, 2),
        ..TickInputs::default()
    });
    assert!(throw.arrivals.is_empty(), "made here: {:?}", throw.arrivals);
    let made: Vec<EntityId> = throw
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) if state.entity != me => Some(state.entity),
            _ => None,
        })
        .collect();
    let [made] = made[..] else {
        panic!("one spawned: {:?}", throw.events);
    };
    let mut outputs = Vec::new();
    for _ in 0..40 {
        outputs.push(idle(&mut region));
    }
    let sendings: Vec<&TickOutput> = outputs
        .iter()
        .filter(|output| !sent_of(output, made).is_empty())
        .collect();
    let [sending] = &sendings[..] else {
        panic!("sent once: {}", sendings.len());
    };
    let [(to, whole)] = &sent_of(sending, made)[..] else {
        unreachable!();
    };
    assert_eq!(*to, EAST);
    assert_eq!(whole.hops, 0);
    assert!(sending.events.iter().any(|event| matches!(
        event,
        RegionEvent::HandedOn { entity, hops: 0, .. } if *entity == made
    )));
    assert!(records(&region, made).is_empty());
}

// ------------------------------------------------------------------------------------
// Arrival tickets.

/// The plan, section 6 (D3, 5): a chunk of the region's pinned land an arrival waits
/// for, `Unknown`, is wanted and so claimed before it is loaded; granted, it is asked
/// of storage and loaded with an arrival ticket, and kept while the arrival waits.
#[test]
fn a_chunk_an_arrival_waits_for_in_pinned_land_not_held_is_claimed_and_loaded() {
    let (mut region, _) = scene(&Layout::pinned_land_not_held(), &[]);
    let far = ChunkPos::new(6, 6);
    assert_eq!(region.knowledge(far), Knowledge::Unknown);
    let waiting = |granted: Vec<ChunkPos>, loaded: Vec<(ChunkPos, Chunk)>| TickInputs {
        arrivals_waiting: vec![far],
        granted,
        chunks_loaded: loaded,
        ..TickInputs::default()
    };
    let output = region.tick(&waiting(Vec::new(), Vec::new()));
    assert_eq!(output.claims, [far]);
    let output = region.tick(&waiting(vec![far], Vec::new()));
    assert_eq!(output.chunk_requests, [far]);
    assert!(output.returns.is_empty());
    let output = region.tick(&waiting(Vec::new(), vec![(far, floor())]));
    assert!(output.returns.is_empty());
    assert!(region.chunk(far).is_some(), "loaded for the arrival");
    // Kept, used, while it waits: nothing gives it back, though the region keeps an
    // unused chunk for no tick at all.
    for _ in 0..50 {
        let output = region.tick(&waiting(Vec::new(), Vec::new()));
        assert!(output.returns.is_empty());
    }
    assert!(region.chunk(far).is_some());
}

/// The record, section 5: an arrival ticket lasts until the arrival is taken and then
/// 100 ticks; a chunk held only for an arrival (granted with it, the land nobody held)
/// is given back as soon as the ticket ends, not after `return_after`.
#[test]
fn a_chunk_granted_with_an_arrival_is_given_back_when_its_ticket_ends() {
    let layout = Layout {
        return_after: 100_000,
        ..Layout::west_of_a_line()
    };
    let (mut region, _) = scene(&layout, &[]);
    let far = ChunkPos::new(6, 6);
    assert!(!region.pins(far));
    let coming = lying(40, [100.5, FLOOR, 100.5], [0.0; 3]);
    assert_eq!(coming.chunk(), far);
    let output = region.tick(&TickInputs {
        granted: vec![far],
        arrivals_waiting: vec![far],
        ..TickInputs::default()
    });
    assert_eq!(output.chunk_requests, [far]);
    region.tick(&TickInputs {
        chunks_loaded: vec![(far, floor())],
        arrivals_waiting: vec![far],
        ..TickInputs::default()
    });
    let output = region.tick(&arriving(vec![arrival(0, &coming)]));
    assert_eq!(output.arrivals_taken.len(), 1);
    let taken = output.tick;
    let mut given_back = None;
    for _ in 0..300 {
        let output = idle(&mut region);
        if output.returns.contains(&far) {
            given_back = Some(output.tick);
            break;
        }
        assert!(region.chunk(far).is_some(), "tick {}", output.tick);
    }
    let given_back = given_back.expect("given back");
    let after = given_back - taken;
    assert!(
        (100..=101).contains(&after),
        "given back {after} ticks after the taking"
    );
    assert_eq!(region.knowledge(far), Knowledge::Unknown);
}

/// The record, section 5: a pinned region's own land beyond its viewers is loaded for
/// an arrival and let go of again a hundred ticks after the taking, not given back.
#[test]
fn held_land_loaded_for_an_arrival_is_let_go_of_a_hundred_ticks_after_the_taking() {
    let (mut region, _) = scene(&Layout::own_land_not_loaded(), &[]);
    let far = ChunkPos::new(3, 0);
    assert_eq!(region.knowledge(far), Knowledge::Held);
    assert!(region.chunk(far).is_none());
    let coming = lying(41, [56.5, FLOOR, 8.5], [0.0; 3]);
    let output = region.tick(&TickInputs {
        arrivals_waiting: vec![far],
        ..TickInputs::default()
    });
    assert_eq!(output.chunk_requests, [far]);
    region.tick(&TickInputs {
        chunks_loaded: vec![(far, floor())],
        arrivals_waiting: vec![far],
        ..TickInputs::default()
    });
    let output = region.tick(&arriving(vec![arrival(0, &coming)]));
    assert_eq!(output.arrivals_taken.len(), 1);
    let taken = output.tick;
    let mut let_go = None;
    for _ in 0..300 {
        let output = idle(&mut region);
        assert!(!output.returns.contains(&far), "pinned land is kept");
        if region.chunk(far).is_none() {
            let_go = Some(output.tick);
            break;
        }
    }
    let let_go = let_go.expect("let go of");
    assert!(
        (100..=102).contains(&(let_go - taken)),
        "let go of {} ticks after the taking",
        let_go - taken
    );
    assert_eq!(region.knowledge(far), Knowledge::Held);
}

// ------------------------------------------------------------------------------------
// At the line.

/// The record, section 7, and the plan, section 5 (G8): the chunks within four blocks
/// of where an entity was sent count as entities working at that line for 600 ticks,
/// by the region it went to, and then no longer.
#[test]
fn a_sending_counts_as_entities_at_the_line_for_six_hundred_ticks() {
    let start = flying_east(50, 0);
    let (mut region, load) = scene(&Layout::west_of_a_line(), std::slice::from_ref(&start));
    region.tick(&TickInputs {
        foreign: vec![(EAST, NEIGHBOUR)],
        tickets_added: vec![(EAST, Ticket::Viewer)],
        ..TickInputs::default()
    });
    assert!(region.entities_at_line().is_empty());
    let _ = load;
    let mut sent_at = None;
    for _ in 0..30 {
        let output = idle(&mut region);
        if !sent_of(&output, start.key).is_empty() {
            sent_at = Some(output.tick);
            break;
        }
    }
    let sent_at = sent_at.expect("sent");
    let at_line = region.entities_at_line();
    assert!(
        at_line.contains(&(ORIGIN, Some(NEIGHBOUR))) && at_line.contains(&(EAST, Some(NEIGHBOUR))),
        "{at_line:?}"
    );
    let mut ended = None;
    for _ in 0..700 {
        let output = idle(&mut region);
        if region.entities_at_line().is_empty() {
            ended = Some(output.tick);
            break;
        }
    }
    let ended = ended.expect("forgotten");
    assert_eq!(ended - sent_at, 600, "counted for 600 ticks");
}
