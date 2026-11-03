//! Tests of the void, the death, the credits and the respawn, tick by tick, written from
//! the specification by someone who did not write the code: `docs/groundwork/p-plan.md`,
//! section 19.7 with its review, the parts of 19.5 (items 1 to 5) and 19.6 it builds on,
//! and the game's rules in `docs/groundwork/p-respawn-facts.md`. Where the code and the
//! game's bytecode disagree, a test says what the bytecode says, cited, and is kept
//! ignored as a finding.
//!
//! The worlds: regions of the overworld, the Nether and the End with a stone floor, held
//! whole or on either side of a line at block x = 16; the overworld's home holds the
//! chunk of the world spawn, (0, 0). The store's part (passing a stay from one region to
//! the next, answering a join) is played by hand.

use clustine_data::{BlockState, blocks, entity_types};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EffectId, EntityData, EntityId, EntityIds,
    ItemStack, PlayerId, RegionId, Travel, Traveller, Vec3, Way,
};

use super::super::tnt::tests::{FLOOR, add, add_player, at, record, state};
use super::super::{Holdings, Region, RegionConfig};
use super::{join, knowing_the_edges};
use crate::api::{
    Durable, EdgeEvent, Entered, Experience, Inventory, Misdirected, Place, PlayerChange,
    PlayerEvent, PlayerInput, PlayerTransfer, Pose, RegionEvent, Respawn, Respawned, StayNote,
    TickInputs, TickOutput, Ticket,
};
use crate::mobs::body::effects::{Effect, Layer};

/// The edge the players these tests put in place come through.
const PLAYERS_EDGE: EdgeId = EdgeId(1);

/// The homes of the three dimensions.
const OVERWORLD_HOME: RegionId = RegionId(0);
const NETHER_HOME: RegionId = RegionId(1);
const END_HOME: RegionId = RegionId(2);

/// A region of the overworld east of the line at block x = 16, which is no home.
const EAST: RegionId = RegionId(5);

/// A region of the Nether east of the same line.
const NETHER_EAST: RegionId = RegionId(6);

/// The world spawn, on the floor of the overworld's chunk (0, 0).
const WORLD_SPAWN: BlockPos = BlockPos::new(0, FLOOR + 1, 0);

/// Where a player stands on the world spawn: its block's bottom centre.
const AT_THE_WORLD_SPAWN: [f64; 3] = [0.5, (FLOOR + 1) as f64, 0.5];

/// The floor of the Nether and the End in these tests, whose levels begin at y = 0.
const HIGH_FLOOR: i32 = 40;

/// How many ticks a test waits for something the specification says happens at once or
/// within the wait for chunks that are all loaded.
const SOON: usize = 8;

fn floor_of(dimension: Dimension) -> i32 {
    if dimension == Dimension::Overworld {
        FLOOR
    } else {
        HIGH_FLOOR
    }
}

/// The chunks with x in `xs` and z from -2 to 2.
fn columns(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// What a test's region is made of.
struct Land {
    dimension: Dimension,
    /// Held and loaded.
    held: Vec<ChunkPos>,
    /// Copies of other regions' chunks, with what `blocks` puts in them.
    copies: Vec<ChunkPos>,
    /// Chunks the region believes another region holds.
    foreign: Vec<(ChunkPos, RegionId)>,
}

impl Land {
    fn whole(dimension: Dimension) -> Self {
        Self {
            dimension,
            held: columns(-2..=2),
            copies: Vec::new(),
            foreign: Vec::new(),
        }
    }

    /// The land west of the line at x = 16 (its chunk x at most 0), with copies of the
    /// column east of it; `other` holds the land east of it, as the region believes.
    fn west_of_the_line(dimension: Dimension, other: RegionId) -> Self {
        Self {
            dimension,
            held: columns(-2..=0),
            copies: columns(1..=1),
            foreign: (columns(1..=3).into_iter())
                .map(|chunk| (chunk, other))
                .collect(),
        }
    }

    /// The land east of the line (chunk x from 1), with copies of the column west of it;
    /// `other` holds the land west of it, as the region believes. It has no home chunk.
    fn east_of_the_line(dimension: Dimension, other: RegionId) -> Self {
        Self {
            dimension,
            held: columns(1..=3),
            copies: columns(0..=0),
            foreign: (columns(-2..=0).into_iter())
                .map(|chunk| (chunk, other))
                .collect(),
        }
    }
}

/// A chunk of `dimension` with a stone floor and the blocks `blocks` names above it.
fn chunk_of(
    dimension: Dimension,
    position: ChunkPos,
    blocks: &dyn Fn(BlockPos) -> Option<BlockState>,
) -> Chunk {
    let floor = floor_of(dimension);
    let mut chunk = Chunk::empty(dimension.rules(), Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, floor, z, blocks::STONE);
            for y in floor + 1..=floor + 12 {
                let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                if let Some(state) = blocks(pos) {
                    chunk.set(x, y, z, state);
                }
            }
        }
    }
    chunk
}

/// A region on `land`, its entity ids from `block`, with `blocks` above its floor,
/// knowing the homes, its players' edge and the edge joins come through. The overworld's
/// spawn is at [`WORLD_SPAWN`]; a region that holds (0, 0) is its dimension's home.
fn region(land: Land, block: u32, blocks: impl Fn(BlockPos) -> Option<BlockState>) -> Region {
    let dimension = land.dimension;
    let rules = dimension.rules();
    let chunks = (land.held.iter())
        .map(|position| (*position, chunk_of(dimension, *position, &blocks)))
        .collect();
    let copies = (land.copies.iter())
        .map(|position| (*position, chunk_of(dimension, *position, &blocks)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = (land.held.iter())
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: land.held.clone(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let spawn = (dimension == Dimension::Overworld).then(|| {
        Vec3::new(
            AT_THE_WORLD_SPAWN[0],
            AT_THE_WORLD_SPAWN[1],
            AT_THE_WORLD_SPAWN[2],
        )
    });
    let config = RegionConfig {
        world_spawn: WORLD_SPAWN,
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, OVERWORLD_HOME),
            (Dimension::Nether, NETHER_HOME),
            (Dimension::End, END_HOME),
        ],
        dimension,
        spawn,
        starting_inventory: Inventory::default(),
        default_mode: clustine_world::GameMode::Creative,
        force_mode: false,
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 11,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = knowing_the_edges(EntityIds::block(block).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, copies);
    region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: PLAYERS_EDGE,
            start: 1,
        }],
        ..TickInputs::default()
    });
    if !land.foreign.is_empty() {
        let wanted = land
            .foreign
            .iter()
            .map(|(chunk, _)| (*chunk, Ticket::Viewer));
        region.tick(&TickInputs {
            tickets_added: wanted.collect(),
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            foreign: land.foreign.clone(),
            ..TickInputs::default()
        });
    }
    region
}

fn nothing(_: BlockPos) -> Option<BlockState> {
    None
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// The number of the next input of every player of these tests: ascending, as an edge
/// numbers them.
fn next_number() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn input(id: PlayerId, entity: EntityId, input: PlayerInput) -> TickInputs {
    TickInputs {
        inputs: vec![(PLAYERS_EDGE, id, entity, next_number(), input)],
        ..TickInputs::default()
    }
}

fn moved_to(position: [f64; 3]) -> PlayerInput {
    PlayerInput::Move {
        position: Some(Vec3::new(position[0], position[1], position[2])),
        rotation: None,
        on_ground: false,
    }
}

fn arrive(id: PlayerId, transfer: PlayerTransfer) -> TickInputs {
    TickInputs {
        player_changes: vec![PlayerChange::Arrive(PLAYERS_EDGE, id, transfer)],
        ..TickInputs::default()
    }
}

/// Where a region let the player go: the transfer and the region named, by a departure
/// or by an arrival sent on (`NotMine`), which the edge passes on alike.
fn let_go(output: &TickOutput, id: PlayerId) -> Option<(PlayerTransfer, RegionId)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Departed {
            player,
            transfer,
            to,
        } if *player == id => Some((transfer.clone(), *to)),
        Durable::NotMine {
            what: Misdirected::Arrival { player, transfer },
            holder,
        } if *player == id => Some((transfer.clone(), *holder)),
        _ => None,
    })
}

struct Placing {
    dimension: Dimension,
    pose: Pose,
    respawn: Option<Respawned>,
}

fn placed(output: &TickOutput, id: PlayerId) -> Option<Placing> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Placed {
            player,
            dimension,
            pose,
            respawn,
            ..
        } if *player == id => Some(Placing {
            dimension: *dimension,
            pose: *pose,
            respawn: *respawn,
        }),
        _ => None,
    })
}

fn died(output: &TickOutput, id: PlayerId) -> Vec<String> {
    (output.durable.iter())
        .filter_map(|(_, _, entry)| match entry {
            Durable::Died {
                player, message, ..
            } if *player == id => Some(message.key.clone()),
            _ => None,
        })
        .collect()
}

fn won(output: &TickOutput, id: PlayerId) -> usize {
    (output.durable.iter())
        .filter(|(_, _, entry)| matches!(entry, Durable::WonGame { player, .. } if *player == id))
        .count()
}

fn told(output: &TickOutput, id: PlayerId) -> Vec<PlayerEvent> {
    (output.player_events.iter())
        .filter(|(player, _)| *player == id)
        .map(|(_, event)| event.clone())
        .collect()
}

fn health_told(output: &TickOutput, id: PlayerId) -> Option<f32> {
    told(output, id).into_iter().find_map(|event| match event {
        PlayerEvent::Health { health, .. } => Some(health),
        _ => None,
    })
}

fn entity_events(output: &TickOutput, entity: EntityId) -> Vec<u8> {
    (output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::EntityEvent {
                entity: of, event, ..
            } if *of == entity => Some(*event),
            _ => None,
        })
        .collect()
}

fn damage_shown(output: &TickOutput, entity: EntityId) -> bool {
    (output.events.iter())
        .any(|event| matches!(event, RegionEvent::DamageEvent { entity: of, .. } if *of == entity))
}

/// Whether anything of the tick names `entity` for those who watch.
fn shows(output: &TickOutput, entity: EntityId) -> bool {
    output.events.iter().any(|event| match event {
        RegionEvent::EntitySpawned(state) | RegionEvent::EntityEntered(state) => {
            state.entity == entity
        }
        RegionEvent::EntityMoved { entity: of, .. }
        | RegionEvent::EntityEvent { entity: of, .. }
        | RegionEvent::DamageEvent { entity: of, .. } => *of == entity,
        _ => false,
    })
}

/// Ticks `region` with `first`, then idly, until `found` finds something, within
/// [`SOON`] ticks. Returns it and every output.
fn within<T>(
    region: &mut Region,
    first: TickInputs,
    found: impl Fn(&TickOutput) -> Option<T>,
) -> (Option<T>, Vec<TickOutput>) {
    let mut outputs = vec![region.tick(&first)];
    for _ in 0..SOON {
        let last = outputs.last().expect("one tick at least");
        if let Some(found) = found(last) {
            return (Some(found), outputs);
        }
        outputs.push(idle(region));
    }
    let last = outputs.last().expect("one tick at least");
    (found(last), outputs)
}

/// Ticks until player `id` dies, within 60 ticks. Returns the number of ticks before
/// the one they died in.
fn until_dead(region: &mut Region, id: PlayerId) -> usize {
    for tick in 0..60 {
        if !died(&idle(region), id).is_empty() {
            return tick;
        }
    }
    panic!("{id:?} did not die");
}

/// Player `number` at `feet` in `region`, which a test has fall into the void or stand
/// where it says.
fn standing(region: &mut Region, number: u128, feet: [f64; 3]) -> (PlayerId, EntityId) {
    add_player(region, number, feet, false)
}

/// A bed whose head is at `head`, facing `facing`: the head there and the foot behind
/// it.
fn bed(head: BlockPos, facing: &str) -> impl Fn(BlockPos) -> Option<BlockState> {
    let back = match facing {
        "north" => (0, 1),
        "south" => (0, -1),
        "east" => (-1, 0),
        _ => (1, 0),
    };
    let head_state = state(&format!(
        "minecraft:red_bed[facing={facing},occupied=false,part=head]"
    ));
    let foot_state = state(&format!(
        "minecraft:red_bed[facing={facing},occupied=false,part=foot]"
    ));
    move |pos| {
        if pos == head {
            Some(head_state)
        } else if pos == head.offset(back.0, 0, back.1) {
            Some(foot_state)
        } else {
            None
        }
    }
}

fn anchor(charges: u8) -> BlockState {
    state(&format!("minecraft:respawn_anchor[charges={charges}]"))
}

fn respawn_at(pos: BlockPos, yaw: f32, dimension: Dimension) -> Respawn {
    Respawn {
        pos,
        yaw,
        pitch: 0.0,
        dimension,
    }
}

/// `RespawnPosAngle.of`'s yaw (p-respawn-facts.md, section 2): from `stand` towards the
/// bottom centre of `block`.
fn facing(stand: [f64; 3], block: BlockPos) -> f32 {
    let dx = f64::from(block.x) + 0.5 - stand[0];
    let dz = f64::from(block.z) + 0.5 - stand[2];
    let yaw = dz.atan2(dx).to_degrees() - 90.0;
    let wrapped = (yaw + 180.0).rem_euclid(360.0) - 180.0;
    wrapped as f32
}

fn assert_at(pose: &Pose, expected: [f64; 3]) {
    let found = [pose.position.x, pose.position.y, pose.position.z];
    let near = found
        .iter()
        .zip(expected)
        .all(|(found, expected)| (found - expected).abs() < 1e-6);
    assert!(near, "placed at {found:?}, not {expected:?}");
}

fn assert_yaw(pose: &Pose, expected: f32) {
    let difference = (pose.yaw - expected + 180.0).rem_euclid(360.0) - 180.0;
    assert!(
        difference.abs() < 0.01,
        "yaw {} where {expected} faces the block",
        pose.yaw
    );
}

/// Puts player `id`, who is dead or in the credits in the region `from`, through the ask
/// for their respawn, and follows their stay through `regions` as the store and the
/// edge pass it on, until a region places them. Returns the placing and the regions it
/// passed through, in order, beginning with `from`.
fn respawned_through(
    regions: &mut [(RegionId, &mut Region)],
    from: RegionId,
    id: PlayerId,
    entity: EntityId,
) -> (Placing, Vec<RegionId>) {
    let check = |output: &TickOutput| {
        placed(output, id)
            .map(Ok)
            .or_else(|| let_go(output, id).map(Err))
    };
    let mut at = from;
    let mut path = vec![from];
    let (mut found, mut outputs) = within(
        region_of(regions, from),
        input(id, entity, PlayerInput::PerformRespawn),
        check,
    );
    for _ in 0..40 {
        match found.take() {
            Some(Ok(placing)) => return (placing, path),
            Some(Err((transfer, to))) => {
                assert_ne!(to, at, "a region let a stay go to itself");
                at = to;
                path.push(to);
                (found, outputs) = within(region_of(regions, to), arrive(id, transfer), check);
            }
            None => panic!("neither placed nor let go in {at:?}: {outputs:?}"),
        }
    }
    panic!("still let go after 40 hand-overs: {path:?}");
}

/// The region of `regions` that is `of`.
fn region_of<'a>(regions: &'a mut [(RegionId, &mut Region)], of: RegionId) -> &'a mut Region {
    let (_, region) = (regions.iter_mut())
        .find(|(id, _)| *id == of)
        .unwrap_or_else(|| panic!("let go to {of:?}, which the test has not"));
    region
}

/// A player in the overworld at (`x`, `z`) far below its floor, dead, their body gone
/// or not.
fn dead_in(region: &mut Region, number: u128, x: f64, z: f64) -> (PlayerId, EntityId) {
    let (id, entity) = standing(region, number, [x, -200.0, z]);
    until_dead(region, id);
    (id, entity)
}

// --- The void (19.5, item 2) -------------------------------------------------------

/// `Entity.checkBelowWorld` (n7 Entity 1144–1160): hurt only where y is below the
/// level's lowest y less 64 (`dcmpg … ifge`), not at it.
#[test]
fn the_void_hurts_below_sixty_four_under_the_level_and_not_at_that_height() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let bottom = f64::from(Dimension::Overworld.rules().min_y) - 64.0;
    let (at_it, at_entity) = standing(&mut home, 1, [3.5, bottom, 3.5]);
    let (below, below_entity) = standing(&mut home, 2, [6.5, bottom - 0.001, 6.5]);
    let output = idle(&mut home);
    assert_eq!(health_told(&output, at_it), None);
    assert!(!damage_shown(&output, at_entity));
    assert_eq!(health_told(&output, below), Some(16.0));
    assert!(damage_shown(&output, below_entity));
}

/// `LivingEntity.hurtServer` (n9 2884–2930) with the cooldown counted down by
/// `ServerPlayer.tick` (n9 918–937) whatever the player's height: a player hurt, then
/// lifted out of the void and put back into it within the cooldown, takes nothing more
/// (4 does not exceed the last 4) until the cooldown is down to 10, ten ticks after the
/// first hurt.
#[test]
fn a_player_back_in_the_void_within_the_cooldown_waits_for_its_half_before_the_next_hurt() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = standing(&mut home, 1, [3.5, -200.0, 3.5]);
    let mut hurts = Vec::new();
    for tick in 0..14 {
        let inputs = match tick {
            2 => input(id, entity, moved_to([3.5, f64::from(FLOOR + 1), 3.5])),
            6 => input(id, entity, moved_to([3.5, -200.0, 3.5])),
            _ => TickInputs::default(),
        };
        let output = home.tick(&inputs);
        if let Some(health) = health_told(&output, id) {
            hurts.push((tick, health));
        }
    }
    assert_eq!(hurts, [(0, 16.0), (10, 12.0)]);
}

/// 19.5 item 2 and `ServerPlayer.isInvulnerableTo` (n9 3014–3030): within the 60 ticks
/// after a respawn before the client has loaded (`CLIENT_LOADED_TIMEOUT_TIME`, SGPL 13;
/// `hasClientLoaded`, SGPL 6770–6790) the void takes nothing. No `PlayerLoaded` comes
/// in this test, so the timer alone would end it.
#[test]
fn a_player_moved_into_the_void_just_after_a_respawn_is_not_hurt_before_their_client_has_loaded() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = dead_in(&mut home, 1, 3.5, 3.5);
    let (placing, _) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_eq!(placing.respawn.map(|respawn| respawn.keep), Some(0));
    let mut hurt = None;
    for tick in 0..50 {
        let inputs = if tick == 0 {
            input(id, entity, moved_to([3.5, -200.0, 3.5]))
        } else {
            TickInputs::default()
        };
        if health_told(&home.tick(&inputs), id).is_some() {
            hurt = Some(tick);
            break;
        }
    }
    assert_eq!(hurt, None, "hurt within the 50 ticks after the respawn");
}

// --- The death (19.5 items 1 and 2) ----------------------------------------------------

/// `ServerPlayer.die` and `dropAllDeathLoot` (19.5 item 1): in the tick of the fifth hurt
/// the player is told their health is 0, the hurt and then the death (entity event 3)
/// are shown, `Died` goes to their edge with the void's message once, what they carried
/// and their experience are gone, and their effects are cleared and that is told. A dead
/// player is not hurt again.
#[test]
fn a_death_in_the_void_tells_shows_drops_and_clears_in_its_tick_and_is_not_hurt_again() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = standing(&mut home, 1, [3.5, -200.0, 3.5]);
    {
        let player = home.players.get_mut(&id).expect("added");
        player.inventory.set(0, Some(ItemStack::new(1, 64)));
        player.experience = Experience {
            level: 7,
            progress: 0.5,
            total: 120,
        };
    }
    let mut scratch = TickOutput::default();
    let now = home.tick_number();
    home.add_player_effect(id, Effect::SPEED, Layer::new(0, 6000, now), &mut scratch);
    let mut death = None;
    for _ in 0..60 {
        let output = idle(&mut home);
        if !died(&output, id).is_empty() {
            death = Some(output);
            break;
        }
    }
    let death = death.expect("the void kills");
    assert_eq!(died(&death, id), ["death.attack.outOfWorld"]);
    assert_eq!(health_told(&death, id), Some(0.0));
    assert!(
        (told(&death, id).iter()).any(|event| matches!(event, PlayerEvent::EffectRemoved { .. })),
        "the cleared effect is told: {:?}",
        told(&death, id)
    );
    let shown: Vec<usize> = (death.events.iter().enumerate())
        .filter_map(|(index, event)| match event {
            RegionEvent::DamageEvent { entity: of, .. } if *of == entity => Some(index),
            RegionEvent::EntityEvent {
                entity: of,
                event: 3,
                ..
            } if *of == entity => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(shown.len(), 2, "the hurt and the death: {:?}", death.events);
    assert!(
        matches!(
            death.events[shown[1]],
            RegionEvent::EntityEvent { event: 3, .. }
        ),
        "the death after the hurt"
    );
    let body = home.player_state(id).expect("the body stays");
    assert!(body.dead);
    assert_eq!(body.inventory, Inventory::default());
    assert_eq!(body.experience, Experience::default());
    assert!(body.effects.is_empty(), "{:?}", body.effects);
    for _ in 0..12 {
        let output = idle(&mut home);
        assert_eq!(
            health_told(&output, id),
            None,
            "a dead player is hurt no more"
        );
        assert!(!damage_shown(&output, entity));
        assert!(died(&output, id).is_empty(), "one death");
    }
}

/// `LivingEntity.baseTick` (n9 49–50 then 490–509) runs `tickDeath` in the very tick the
/// void kills, after `Entity.baseTick`'s hurt; `tickDeath` (n9 1129–53) counts
/// `deathTime` up and removes the body with event 60 when it reaches 20: in the
/// nineteenth tick after the one of the death.
#[test]
fn the_body_goes_with_event_sixty_in_the_nineteenth_tick_after_the_death() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (_, entity) = dead_in(&mut home, 1, 3.5, 3.5);
    let mut gone = None;
    for tick in 1..=25 {
        let output = idle(&mut home);
        if entity_events(&output, entity).contains(&60) {
            gone = Some(tick);
            break;
        }
    }
    assert_eq!(gone, Some(19));
}

/// 19.7, "Death and its respawn": a player who is neither dead nor in the credits asks
/// for nothing.
#[test]
fn the_ask_for_a_respawn_of_a_living_player_does_nothing() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = standing(&mut home, 1, [3.5, f64::from(FLOOR + 1), 3.5]);
    let output = home.tick(&input(id, entity, PlayerInput::PerformRespawn));
    assert!(placed(&output, id).is_none());
    assert!(let_go(&output, id).is_none());
    assert!(!shows(&output, entity), "{:?}", output.events);
    for _ in 0..SOON {
        let output = idle(&mut home);
        assert!(placed(&output, id).is_none());
        assert!(let_go(&output, id).is_none());
    }
    let (_, pose) = home.player(id).expect("still in the world");
    assert_at(&pose, [3.5, f64::from(FLOOR + 1), 3.5]);
    assert!(home.waiting_state(id).is_none());
}

/// 19.7: while dead, every input but `PerformRespawn` is dropped: a dead player does
/// not move, change their held slot or fly, before the body goes or after.
#[test]
fn the_inputs_of_a_dead_player_but_the_ask_are_dropped_before_and_after_the_body_goes() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = dead_in(&mut home, 1, 3.5, 3.5);
    home.tick(&input(id, entity, PlayerInput::SelectSlot { slot: 4 }));
    home.tick(&input(
        id,
        entity,
        moved_to([3.5, f64::from(FLOOR + 1), 3.5]),
    ));
    let body = home.player_state(id).expect("the body stays");
    assert_eq!(body.selected_slot, 0);
    assert!(body.pose.position.y < -128.0);
    // The body goes, and the stay waits to be asked.
    for _ in 0..25 {
        idle(&mut home);
    }
    assert!(home.player(id).is_none());
    let output = home.tick(&input(id, entity, PlayerInput::SelectSlot { slot: 6 }));
    assert!(placed(&output, id).is_none());
    let output = home.tick(&input(id, entity, PlayerInput::SetFlying { flying: true }));
    assert!(placed(&output, id).is_none());
    let waiting = home.waiting_state(id).expect("waits to be asked");
    assert!(waiting.until_asked);
    assert!(waiting.kept.is_empty(), "nothing kept: {:?}", waiting.kept);
    assert_eq!(waiting.transfer.selected_slot, 0);
    // The ask is still heard, and the inputs before it changed nothing.
    let (placing, _) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_eq!(placing.respawn.map(|respawn| respawn.keep), Some(0));
    let state = home.player_state(id).expect("respawned");
    assert_eq!(state.selected_slot, 0);
    assert!(!state.flying);
}

// --- The credits (19.7, "The credits") ----------------------------------------------------

/// The End with an End portal's block at (2, 41, 2).
fn the_end(land: Land, block: u32) -> Region {
    let portal = at(2, HIGH_FLOOR + 1, 2);
    region(land, block, move |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    })
}

const IN_THE_PORTAL: [f64; 3] = [2.5, (HIGH_FLOOR + 1) as f64, 2.5];

/// `showEndCredits` (facts 6): the player is out of the world in the tick they touch the
/// portal, removed for those who watch, `WonGame` sent once; afterwards they are not
/// ticked, not hurt and not shown, and their moves and every other input but the ask
/// are dropped, nothing of them kept for the placing.
#[test]
fn a_player_in_the_credits_is_hidden_unticked_and_deaf_to_everything_but_the_ask() {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let (watcher, _) = standing(&mut end, 2, [6.5, f64::from(HIGH_FLOOR + 1), 6.5]);
    let (id, entity) = standing(&mut end, 1, IN_THE_PORTAL);
    let output = idle(&mut end);
    assert_eq!(won(&output, id), 1);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityRemoved { entity: of, .. } if *of == entity
    )));
    assert!(end.player(id).is_none());
    assert!(end.player(watcher).is_some());
    // Not ticked: a move into the void is dropped, and nothing of them is shown or hurt.
    let output = end.tick(&input(id, entity, moved_to([2.5, -200.0, 2.5])));
    assert!(!shows(&output, entity));
    end.tick(&input(id, entity, PlayerInput::SelectSlot { slot: 3 }));
    for _ in 0..30 {
        let output = idle(&mut end);
        assert!(!shows(&output, entity), "{:?}", output.events);
        assert_eq!(health_told(&output, id), None);
        assert_eq!(won(&output, id), 0, "the credits are shown once");
        assert!(let_go(&output, id).is_none() && placed(&output, id).is_none());
    }
    let waiting = end.waiting_state(id).expect("waits to be asked");
    assert!(waiting.until_asked);
    assert!(waiting.kept.is_empty(), "{:?}", waiting.kept);
    assert_eq!(waiting.transfer.selected_slot, 0);
    assert_eq!(waiting.transfer.health, 20.0);
    assert!(waiting.transfer.seen_credits);
}

/// `PlayerList.respawn` (facts 5.3) sends `SetExperience` and `restoreFrom` with keep
/// (facts 5.6) keeps the experience: the credits' respawn tells the player the
/// experience they kept (19.5 item 1: the respawn sends `SetExperience`).
#[test]
fn the_credits_respawn_tells_the_experience_the_player_kept() {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let (id, entity) = standing(&mut end, 1, IN_THE_PORTAL);
    let experience = Experience {
        level: 5,
        progress: 0.25,
        total: 60,
    };
    end.players.get_mut(&id).expect("added").experience = experience;
    idle(&mut end);
    let mut home = region(Land::whole(Dimension::Overworld), 100, nothing);
    let (_, outputs) = {
        let output = end.tick(&input(id, entity, PlayerInput::PerformRespawn));
        let (transfer, to) = let_go(&output, id).expect("let go to the overworld's home");
        assert_eq!(to, OVERWORLD_HOME);
        within(&mut home, arrive(id, transfer), |output| placed(output, id))
    };
    let placing = (outputs.iter())
        .find_map(|output| placed(output, id))
        .expect("placed");
    assert_eq!(placing.respawn.map(|respawn| respawn.keep), Some(1));
    let told_experience = (outputs.iter())
        .flat_map(|output| told(output, id))
        .find_map(|event| match event {
            PlayerEvent::Experience(told) => Some(told),
            _ => None,
        });
    assert_eq!(told_experience, Some(experience));
}

// --- Each way's travel ------------------------------------------------------------------

/// 19.7, "One path for three ways": a death in an overworld region that is not the
/// overworld's home lets the player go there, bound for the world spawn, the respawned
/// body made with the travel (the review, item 8); the home places them at the world
/// spawn with keep 0 and no event.
#[test]
fn a_death_away_from_the_home_is_let_go_to_the_home_and_placed_at_the_world_spawn() {
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        50,
        nothing,
    );
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        0,
        nothing,
    );
    let (id, entity) = dead_in(&mut east, 1, 40.5, 8.5);
    let output = east.tick(&input(id, entity, PlayerInput::PerformRespawn));
    let (transfer, to) = let_go(&output, id).expect("let go at the ask");
    assert_eq!(to, OVERWORLD_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::Overworld,
            target: WORLD_SPAWN,
            way: Way::Respawn,
        })
    );
    assert_eq!(transfer.health, 20.0);
    assert!(!transfer.dead);
    assert_eq!(transfer.inventory, Inventory::default());
    let (placing, _) = within(&mut home, arrive(id, transfer), |output| placed(output, id));
    let placing = placing.expect("placed by the home");
    assert_eq!(placing.dimension, Dimension::Overworld);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    let state = home.player_state(id).expect("in the world");
    assert_eq!(state.health, 20.0);
    assert!(!state.dead);
}

/// The bed of these tests: its head at (4, -60, 4) in chunk (0, 0), facing north.
const BED: BlockPos = BlockPos::new(4, FLOOR + 1, 4);

/// Facts 1.2, 1.4 and 2: a bed in the home's own chunk. `s` is north's clockwise, east,
/// kept where the stored yaw 0 does not face it (`-sin 0 · 1 + cos 0 · 0 = 0`, not
/// above 0); the first candidate, `s`, is free on the floor: the player stands at
/// (5.5, -60, 4.5), pitch 0, facing the bed (yaw 90), the bed kept.
#[test]
fn a_death_s_respawn_at_a_bed_in_the_home_s_own_chunk_stands_east_of_it_facing_it() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, bed(BED, "north"));
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    let respawn = respawn_at(BED, 0.0, Dimension::Overworld);
    home.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let (placing, path) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_eq!(path, [OVERWORLD_HOME], "no hand-over");
    let stand = [5.5, f64::from(FLOOR + 1), 4.5];
    assert_at(&placing.pose, stand);
    assert_yaw(&placing.pose, facing(stand, BED));
    assert_yaw(&placing.pose, 90.0);
    assert_eq!(placing.pose.pitch, 0.0);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    let state = home.player_state(id).expect("respawned");
    assert_eq!(state.respawn, Some(respawn), "a bed is never used up");
}

/// Facts 1.4: the stored yaw only picks the bed's side. Yaw -90 looks east, which east
/// faces (`-sin(-90°) · 1 = 1 > 0`), so `s` turns to west: the player stands at
/// (3.5, -60, 4.5) facing the bed, yaw -90.
#[test]
fn the_stored_look_that_faces_the_bed_s_side_turns_the_stand_up_place_to_the_other() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, bed(BED, "north"));
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    home.players.get_mut(&id).expect("dead").respawn =
        Some(respawn_at(BED, -90.0, Dimension::Overworld));
    let (placing, _) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    let stand = [3.5, f64::from(FLOOR + 1), 4.5];
    assert_at(&placing.pose, stand);
    assert_yaw(&placing.pose, -90.0);
}

/// The bed east of the line, in chunk (1, 0).
const EASTERN_BED: BlockPos = BlockPos::new(20, FLOOR + 1, 4);

/// 19.7, "Only the holder checks" (the review, items 4 and 5): the home has a copy of the
/// bed's chunk, which is the east's; it checks nothing there and lets the stay go to the
/// east, the travel unchanged; the east wakes the player beside the bed.
#[test]
fn a_bed_in_a_copy_of_another_region_s_chunk_is_woken_at_by_that_region() {
    let blocks = bed(EASTERN_BED, "north");
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        0,
        &blocks,
    );
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        50,
        &blocks,
    );
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    let respawn = respawn_at(EASTERN_BED, 0.0, Dimension::Overworld);
    home.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let (placing, path) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home), (EAST, &mut east)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_eq!(path, [OVERWORLD_HOME, EAST]);
    let stand = [21.5, f64::from(FLOOR + 1), 4.5];
    assert_at(&placing.pose, stand);
    assert_yaw(&placing.pose, facing(stand, EASTERN_BED));
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_eq!(east.player_state(id).expect("there").respawn, Some(respawn));
}

/// 19.7, case 4 and the event (the review, item 6): a bed gone in the east's chunk. The
/// home lets the stay go to the east, which finds no bed and turns the travel to the
/// world spawn, letting it go back to the home; the home owes the event, since the
/// travel is no longer the respawn's, and forgets the bed after a death.
#[test]
fn a_bed_gone_in_another_region_s_chunk_sends_the_dead_back_to_the_world_spawn_with_the_event() {
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        0,
        nothing,
    );
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        50,
        nothing,
    );
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    home.players.get_mut(&id).expect("dead").respawn =
        Some(respawn_at(EASTERN_BED, 0.0, Dimension::Overworld));
    let (placing, path) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home), (EAST, &mut east)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_eq!(path, [OVERWORLD_HOME, EAST, OVERWORLD_HOME]);
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: true,
            anchor_sound: None,
        })
    );
    assert_eq!(home.player_state(id).expect("there").respawn, None);
}

/// 19.7, the event (the review, item 6): a bed at the world spawn's own block is a bed
/// like any other. A death away from the home sends the stay to the home bound for the
/// world spawn's block, which is the bed's: it wakes the player beside the bed, owing
/// no event.
#[test]
fn a_bed_at_the_world_spawn_s_block_is_woken_at_without_the_missing_block_s_event() {
    let blocks = bed(WORLD_SPAWN, "north");
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        50,
        &blocks,
    );
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        0,
        &blocks,
    );
    let (id, entity) = dead_in(&mut east, 1, 40.5, 8.5);
    let respawn = respawn_at(WORLD_SPAWN, 0.0, Dimension::Overworld);
    east.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let (placing, path) = respawned_through(
        &mut [(EAST, &mut east), (OVERWORLD_HOME, &mut home)],
        EAST,
        id,
        entity,
    );
    assert_eq!(path, [EAST, OVERWORLD_HOME]);
    let stand = [1.5, f64::from(FLOOR + 1), 0.5];
    assert_at(&placing.pose, stand);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_eq!(home.player_state(id).expect("there").respawn, Some(respawn));
}

/// The anchor of these tests, in the Nether's chunk (0, 0).
const ANCHOR: BlockPos = BlockPos::new(4, HIGH_FLOOR + 1, 4);

fn an_anchor(charges: u8) -> impl Fn(BlockPos) -> Option<BlockState> {
    let charged = anchor(charges);
    move |pos| (pos == ANCHOR).then_some(charged)
}

/// Facts 1.2, 1.3, 2 and 5.4: a death's respawn at a charged anchor in the Nether's
/// home: the first of `RESPAWN_OFFSETS`, (0, 0, -1), is free, so the player stands at
/// (4.5, 41, 3.5) facing the anchor (yaw 0), one charge is spent (`UPDATE_ALL`) and the
/// anchor's sound is owed at its block.
#[test]
fn a_death_s_respawn_at_an_anchor_spends_a_charge_and_owes_its_sound() {
    let mut nether = region(Land::whole(Dimension::Nether), 0, an_anchor(2));
    let (id, entity) = standing(&mut nether, 1, [8.5, -200.0, 8.5]);
    until_dead(&mut nether, id);
    let respawn = respawn_at(ANCHOR, 0.0, Dimension::Nether);
    nether.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let (placing, path) =
        respawned_through(&mut [(NETHER_HOME, &mut nether)], NETHER_HOME, id, entity);
    assert_eq!(path, [NETHER_HOME]);
    assert_eq!(placing.dimension, Dimension::Nether);
    let stand = [4.5, f64::from(HIGH_FLOOR + 1), 3.5];
    assert_at(&placing.pose, stand);
    assert_yaw(&placing.pose, 0.0);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: Some(ANCHOR),
        })
    );
    assert_eq!(nether.block(ANCHOR), Some(anchor(1)), "one charge spent");
    assert_eq!(
        nether.player_state(id).expect("there").respawn,
        Some(respawn)
    );
}

/// Facts 5.1 and 5.4: the credits' respawn (`useSpawnBlock` false) at an anchor spends
/// no charge and owes no sound, keep 1; the End, which is not the Nether's home, lets
/// the stay go to the Nether's home.
#[test]
fn the_credits_respawn_at_an_anchor_spends_nothing_and_owes_no_sound() {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let mut nether = region(Land::whole(Dimension::Nether), 100, an_anchor(2));
    let (id, entity) = standing(&mut end, 1, IN_THE_PORTAL);
    let respawn = respawn_at(ANCHOR, 0.0, Dimension::Nether);
    end.players.get_mut(&id).expect("added").respawn = Some(respawn);
    assert_eq!(won(&idle(&mut end), id), 1);
    let (placing, path) = respawned_through(
        &mut [(END_HOME, &mut end), (NETHER_HOME, &mut nether)],
        END_HOME,
        id,
        entity,
    );
    assert_eq!(path, [END_HOME, NETHER_HOME]);
    assert_at(&placing.pose, [4.5, f64::from(HIGH_FLOOR + 1), 3.5]);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 1,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_eq!(nether.block(ANCHOR), Some(anchor(2)), "no charge spent");
}

/// The review, item 5: an anchor in a copy of another region's chunk is neither checked
/// nor spent by the region that copies it; the holder spends the charge.
#[test]
fn an_anchor_in_a_copied_chunk_is_spent_only_by_the_region_that_holds_it() {
    let eastern = BlockPos::new(20, HIGH_FLOOR + 1, 4);
    let charged = anchor(1);
    let blocks = move |pos| (pos == eastern).then_some(charged);
    let mut home = region(
        Land::west_of_the_line(Dimension::Nether, NETHER_EAST),
        0,
        blocks,
    );
    let mut east = region(
        Land::east_of_the_line(Dimension::Nether, NETHER_HOME),
        50,
        blocks,
    );
    let (id, entity) = standing(&mut home, 1, [8.5, -200.0, 8.5]);
    until_dead(&mut home, id);
    home.players.get_mut(&id).expect("dead").respawn =
        Some(respawn_at(eastern, 0.0, Dimension::Nether));
    let output = home.tick(&input(id, entity, PlayerInput::PerformRespawn));
    assert!(
        output.effects.is_empty(),
        "nothing filed for the copied anchor: {:?}",
        output.effects
    );
    let mut sent = let_go(&output, id);
    for _ in 0..SOON {
        if sent.is_some() {
            break;
        }
        let output = idle(&mut home);
        assert!(output.effects.is_empty(), "{:?}", output.effects);
        assert!(placed(&output, id).is_none(), "placed by the copy");
        sent = let_go(&output, id);
    }
    let (transfer, to) = sent.expect("let go to the holder");
    assert_eq!(to, NETHER_EAST);
    let (placing, _) = within(&mut east, arrive(id, transfer), |output| placed(output, id));
    let placing = placing.expect("placed by the holder");
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: Some(eastern),
        })
    );
    assert_eq!(east.block(eastern), Some(anchor(0)));
}

/// Facts 1.2 (`RespawnAnchorBlock.canSetSpawn`, `RESPAWN_ANCHOR_WORKS`): an anchor in the
/// overworld is no respawn. The death's respawn is at the world spawn with the event,
/// the anchor forgotten and untouched, and no sound (the new player has no respawn,
/// PL 505–518).
#[test]
fn an_anchor_in_the_overworld_is_no_respawn() {
    let anchor_here = BlockPos::new(4, FLOOR + 1, 4);
    let charged = anchor(3);
    let mut home = region(Land::whole(Dimension::Overworld), 0, move |pos| {
        (pos == anchor_here).then_some(charged)
    });
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    home.players.get_mut(&id).expect("dead").respawn =
        Some(respawn_at(anchor_here, 0.0, Dimension::Overworld));
    let (placing, _) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: true,
            anchor_sound: None,
        })
    );
    assert_eq!(home.block(anchor_here), Some(anchor(3)));
    assert_eq!(home.player_state(id).expect("there").respawn, None);
}

/// Facts 1.2 (`BedRule.canSetSpawn`): a bed in the End is no respawn. The credits'
/// respawn of a player whose respawn is such a bed travels to the End's home, which is
/// the region they are in: it finds no respawn there and turns the travel to the world
/// spawn, letting it go to the overworld's home, which owes the event and forgets the
/// bed (the credits' way).
#[test]
fn the_credits_respawn_at_a_bed_in_the_end_turns_to_the_world_spawn_with_the_event() {
    let end_bed = BlockPos::new(6, HIGH_FLOOR + 1, 6);
    let beds = bed(end_bed, "north");
    let portal = at(2, HIGH_FLOOR + 1, 2);
    let mut end = region(Land::whole(Dimension::End), 0, move |pos| {
        if pos == portal {
            Some(blocks::END_PORTAL)
        } else {
            beds(pos)
        }
    });
    let mut home = region(Land::whole(Dimension::Overworld), 100, nothing);
    let (id, entity) = standing(&mut end, 1, IN_THE_PORTAL);
    end.players.get_mut(&id).expect("added").respawn =
        Some(respawn_at(end_bed, 0.0, Dimension::End));
    assert_eq!(won(&idle(&mut end), id), 1);
    let (placing, path) = respawned_through(
        &mut [(END_HOME, &mut end), (OVERWORLD_HOME, &mut home)],
        END_HOME,
        id,
        entity,
    );
    assert_eq!(path, [END_HOME, OVERWORLD_HOME]);
    assert_eq!(placing.dimension, Dimension::Overworld);
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 1,
            no_respawn_block: true,
            anchor_sound: None,
        })
    );
    assert_eq!(home.player_state(id).expect("there").respawn, None);
}

/// The End's portal out of the End for a player who has seen the credits.
fn out_of_the_end(respawn: Option<Respawn>) -> (PlayerTransfer, RegionId, PlayerId) {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let (id, _) = standing(&mut end, 1, IN_THE_PORTAL);
    {
        let player = end.players.get_mut(&id).expect("added");
        player.seen_credits = true;
        player.respawn = respawn;
    }
    let (sent, outputs) = within(&mut end, TickInputs::default(), |output| let_go(output, id));
    assert!(outputs.iter().all(|output| won(output, id) == 0));
    let (transfer, to) = sent.expect("the portal sends them out");
    (transfer, to, id)
}

/// 19.7, case 4, and facts 3 (`SP.teleport` sends the event and nothing clears the
/// respawn): out of the End by its portal to a bed gone, the player is placed at the
/// world spawn with keep 3, the event owed, and the bed kept.
#[test]
fn out_of_the_end_to_a_bed_gone_owes_the_event_and_keeps_the_bed() {
    let respawn = respawn_at(BED, 0.0, Dimension::Overworld);
    let (transfer, to, id) = out_of_the_end(Some(respawn));
    assert_eq!(to, OVERWORLD_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::Overworld,
            target: BED,
            way: Way::EndPortal,
        })
    );
    let mut home = region(Land::whole(Dimension::Overworld), 100, nothing);
    let (placing, _) = within(&mut home, arrive(id, transfer), |output| placed(output, id));
    let placing = placing.expect("placed");
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 3,
            no_respawn_block: true,
            anchor_sound: None,
        })
    );
    assert_eq!(home.player_state(id).expect("there").respawn, Some(respawn));
}

/// Out of the End by its portal to a bed in another region's chunk: the home lets the
/// stay go to that region, which wakes the player beside the bed, keep 3, no event.
#[test]
fn out_of_the_end_to_a_bed_in_another_region_s_chunk_is_woken_there_keeping_everything() {
    let respawn = respawn_at(EASTERN_BED, 0.0, Dimension::Overworld);
    let (transfer, to, id) = out_of_the_end(Some(respawn));
    assert_eq!(to, OVERWORLD_HOME);
    let blocks = bed(EASTERN_BED, "north");
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        100,
        &blocks,
    );
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        150,
        &blocks,
    );
    let (sent, _) = within(&mut home, arrive(id, transfer), |output| {
        assert!(placed(output, id).is_none(), "placed by a copy");
        let_go(output, id)
    });
    let (transfer, to) = sent.expect("let go to the bed's holder");
    assert_eq!(to, EAST);
    let (placing, _) = within(&mut east, arrive(id, transfer), |output| placed(output, id));
    let placing = placing.expect("placed");
    assert_at(&placing.pose, [21.5, f64::from(FLOOR + 1), 4.5]);
    assert_eq!(
        placing.respawn,
        Some(Respawned {
            keep: 3,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
}

/// The review, item 4: a waiting stay that keeps being let go because two regions each
/// believe the other holds its target counts its forwards, and at 16 is placed at the
/// world spawn, its respawn kept and no event owed.
#[test]
fn a_stay_passed_back_and_forth_is_placed_at_the_world_spawn_after_sixteen_forwards() {
    let mut home = region(
        Land {
            dimension: Dimension::Overworld,
            held: columns(-2..=0),
            copies: Vec::new(),
            foreign: (columns(1..=3).into_iter())
                .map(|chunk| (chunk, EAST))
                .collect(),
        },
        0,
        nothing,
    );
    let mut east = region(
        Land {
            dimension: Dimension::Overworld,
            held: columns(2..=3),
            copies: Vec::new(),
            foreign: (columns(-2..=1).into_iter())
                .map(|chunk| (chunk, OVERWORLD_HOME))
                .collect(),
        },
        50,
        nothing,
    );
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    let respawn = respawn_at(EASTERN_BED, 0.0, Dimension::Overworld);
    home.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let (placing, path) = respawned_through(
        &mut [(OVERWORLD_HOME, &mut home), (EAST, &mut east)],
        OVERWORLD_HOME,
        id,
        entity,
    );
    assert!(path.len() >= 17, "{path:?}");
    assert_at(&placing.pose, AT_THE_WORLD_SPAWN);
    assert_eq!(
        placing.respawn.map(|respawn| respawn.no_respawn_block),
        Some(false)
    );
}

// --- Leaving while dead or in the credits ------------------------------------------------

/// The `Has` notes of `outputs` for `id`, the last.
fn kept_place(outputs: &[TickOutput], id: PlayerId) -> Option<Place> {
    (outputs.iter().rev())
        .flat_map(|output| output.stays.iter().rev())
        .find_map(|note| match note {
            StayNote::Has { player, place, .. } if *player == id => Some(place.clone()),
            _ => None,
        })
}

/// A join of player `number` to `region` with the place the store kept, answered at
/// once. Returns every output until the region told them `Spawned`, within a few ticks.
fn join_with(region: &mut Region, number: u128, place: Place) -> Vec<TickOutput> {
    let id = PlayerId(uuid::Uuid::from_u128(number));
    let mut outputs = vec![region.tick(&TickInputs {
        player_changes: vec![join(number)],
        ..TickInputs::default()
    })];
    let entity = region
        .entering_state(id)
        .expect("the join is entering")
        .entity_id;
    let (_, more) = within(
        region,
        TickInputs {
            entered: vec![Entered {
                unit: None,
                player: id,
                entity,
                place: Some(place),
                holder: None,
            }],
            ..TickInputs::default()
        },
        |output| {
            told(output, id)
                .into_iter()
                .any(|event| matches!(event, PlayerEvent::Spawned { .. }))
                .then_some(())
        },
    );
    outputs.extend(more);
    outputs
}

fn spawned_pose(outputs: &[TickOutput], id: PlayerId) -> Option<Pose> {
    (outputs.iter())
        .flat_map(|output| told(output, id))
        .find_map(|event| match event {
            PlayerEvent::Spawned { pose, .. } => Some(pose),
            _ => None,
        })
}

/// 19.7, "Leaving while dead" with the review's item 9: a player who leaves while their
/// body stays enters at their respawn at the next join: `Spawned` there, alive and
/// whole, with no `Placed` (no `Respawn` for a client that is entering).
#[test]
fn a_player_who_leaves_while_dead_enters_alive_at_the_world_spawn_at_the_next_join() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = standing(&mut home, 1, [8.5, -200.0, 8.5]);
    let mut outputs = Vec::new();
    while outputs.iter().all(|output| died(output, id).is_empty()) {
        assert!(outputs.len() < 60, "the void kills");
        outputs.push(idle(&mut home));
    }
    outputs.push(home.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(PLAYERS_EDGE, id, Some(entity), None)],
        ..TickInputs::default()
    }));
    let place = kept_place(&outputs, id).expect("the store keeps a place");
    assert!(place.dead, "the store keeps them dead: {place:?}");
    assert!(home.player(id).is_none());
    let outputs = join_with(&mut home, 1, place);
    let pose = spawned_pose(&outputs, id).expect("entered");
    assert_at(&pose, AT_THE_WORLD_SPAWN);
    assert!(outputs.iter().all(|output| placed(output, id).is_none()));
    let state = home.player_state(id).expect("in the world");
    assert!(!state.dead);
    assert_eq!(state.health, 20.0);
}

/// The same for a player who leaves after their body went, while they wait to be asked.
#[test]
fn a_player_who_leaves_on_the_death_screen_enters_alive_at_their_bed_at_the_next_join() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, bed(BED, "north"));
    let (id, entity) = dead_in(&mut home, 1, 8.5, 8.5);
    let respawn = respawn_at(BED, 0.0, Dimension::Overworld);
    home.players.get_mut(&id).expect("dead").respawn = Some(respawn);
    let mut outputs = Vec::new();
    for _ in 0..25 {
        outputs.push(idle(&mut home));
    }
    assert!(
        home.waiting_state(id)
            .is_some_and(|waiting| waiting.until_asked)
    );
    outputs.push(home.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(PLAYERS_EDGE, id, Some(entity), None)],
        ..TickInputs::default()
    }));
    let place = kept_place(&outputs, id).expect("the store keeps a place");
    assert!(home.waiting_state(id).is_none());
    let outputs = join_with(&mut home, 1, place);
    let pose = spawned_pose(&outputs, id).expect("entered");
    assert_at(&pose, [5.5, f64::from(FLOOR + 1), 4.5]);
    assert!(outputs.iter().all(|output| placed(output, id).is_none()));
    let state = home.player_state(id).expect("in the world");
    assert!(!state.dead);
    assert_eq!(state.respawn, Some(respawn));
}

/// 19.7, "Leaving … in the credits": a player who leaves in the credits enters at their
/// respawn at the next join, with `Spawned` and no `Placed`, and is not shown the
/// credits again.
#[test]
fn a_player_who_leaves_in_the_credits_enters_at_the_world_spawn_at_the_next_join() {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let (id, entity) = standing(&mut end, 1, IN_THE_PORTAL);
    let mut outputs = vec![idle(&mut end)];
    assert_eq!(won(&outputs[0], id), 1);
    outputs.push(end.tick(&TickInputs {
        player_changes: vec![PlayerChange::Leave(PLAYERS_EDGE, id, Some(entity), None)],
        ..TickInputs::default()
    }));
    let place = kept_place(&outputs, id).expect("the store keeps a place");
    assert!(place.seen_credits);
    assert_eq!(
        place.dimension,
        Dimension::Overworld,
        "the place is the respawn's: {place:?}"
    );
    let mut home = region(Land::whole(Dimension::Overworld), 100, nothing);
    let outputs = join_with(&mut home, 1, place);
    let pose = spawned_pose(&outputs, id).expect("entered");
    assert_at(&pose, AT_THE_WORLD_SPAWN);
    assert!(outputs.iter().all(|output| placed(output, id).is_none()));
    assert!(outputs.iter().all(|output| won(output, id) == 0));
}

// --- An entity out of the End -------------------------------------------------------------

fn item(region: &mut Region, position: [f64; 3]) -> EntityId {
    let entity = record(
        region,
        entity_types::ITEM,
        position,
        EntityData::Item {
            stack: ItemStack::new(1, 3),
            pick_up_delay: 40,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    );
    add(region, entity)
}

/// An item through the End's portal: the traveller the End files.
fn item_out_of_the_end() -> (EntityId, Traveller) {
    let mut end = the_end(Land::whole(Dimension::End), 0);
    let key = item(&mut end, [2.5, f64::from(HIGH_FLOOR) + 1.2, 2.5]);
    for _ in 0..SOON {
        let output = idle(&mut end);
        if let Some(traveller) = output.travellers.into_iter().next() {
            return (key, traveller);
        }
    }
    panic!("the item did not go through the portal");
}

/// Where `key` is put into `home` from `traveller`, within a few ticks.
fn put_out(home: &mut Region, key: EntityId, traveller: Traveller) -> [f64; 3] {
    let id = EffectId {
        sequence: 1,
        index: 0,
    };
    let (found, _) = within(
        home,
        TickInputs {
            travellers: vec![(id, traveller)],
            ..TickInputs::default()
        },
        |output| {
            output.events.iter().find_map(|event| match event {
                RegionEvent::EntitySpawned(state) if state.entity == key => {
                    let position = state.pose.position;
                    Some([position.x, position.y, position.z])
                }
                _ => None,
            })
        },
    );
    found.expect("put into the overworld")
}

/// `EndPortalBlock.getPortalDestination` (d EndPortalBlock 220–264) with
/// `Entity.adjustSpawnLocation` (d Entity 3544–3568): an entity out of the End comes
/// out at the bottom centre of the block above the top of the world spawn's column, by
/// `MOTION_BLOCKING_NO_LEAVES`.
#[test]
fn an_entity_out_of_the_end_comes_out_on_the_top_of_the_world_spawn_s_column() {
    let (key, traveller) = item_out_of_the_end();
    assert_eq!(traveller.travel.to, Dimension::Overworld);
    assert_eq!(traveller.travel.way, Way::EndPortal);
    let pillar = |pos: BlockPos| {
        (pos.x == WORLD_SPAWN.x && pos.z == WORLD_SPAWN.z && pos.y <= FLOOR + 3)
            .then_some(blocks::STONE)
    };
    let mut home = region(Land::whole(Dimension::Overworld), 100, pillar);
    let at = put_out(&mut home, key, traveller);
    assert_eq!(at, [0.5, f64::from(FLOOR + 4), 0.5]);
}

/// The same with leaves on the column: `MOTION_BLOCKING_NO_LEAVES` passes them over, so
/// the entity comes out on the block under them.
#[test]
fn an_entity_out_of_the_end_is_not_put_on_the_leaves_over_the_world_spawn() {
    let (key, traveller) = item_out_of_the_end();
    let leaves = state("minecraft:oak_leaves[distance=1,persistent=true,waterlogged=false]");
    let canopy = move |pos: BlockPos| {
        (pos.x == WORLD_SPAWN.x && pos.z == WORLD_SPAWN.z && pos.y == FLOOR + 3).then_some(leaves)
    };
    let mut home = region(Land::whole(Dimension::Overworld), 100, canopy);
    let at = put_out(&mut home, key, traveller);
    assert_eq!(at, [0.5, f64::from(FLOOR + 1), 0.5]);
}
