//! Tests of a Nether portal's travel written from its specification by someone who did
//! not write the code (`docs/groundwork/p-plan.md`, sections 5.1, 5.2, 15.4, 15.5 and 16;
//! `docs/groundwork/p-brief.md`, sections 2.2 and 2.3; ADR-0020, sections 3 to 6).
//!
//! The source's half: a traveller let go or filed in the tick its clock fires, with
//! the target scaled and clamped and its place in the source's portal. The
//! destination's half: the wait for the index's answer, the choice of a portal, the
//! portal made where there is none, and the pose. The rules that end or answer a stay,
//! applied to a stay that waits; and a restore, a split and a merge with stays and
//! travellers waiting.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::components::EffectInstance;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, Double, EdgeId, EffectId, EntityData, EntityId,
    EntityIds, Float, ItemStack, LevelChunk, PlayerId, PortalAxis, RegionId, Travel, Traveller,
    Vec3, Way,
};
use uuid::Uuid;

use super::super::portals::tests::{portal, world_in};
use super::super::tnt::tests::{FLOOR, add, add_player, at, cart, idle, record, state, world};
use super::super::{Holdings, Region, RegionConfig};
use crate::api::{
    Durable, EdgeEvent, Inventory, PlayerChange, PlayerInput, PlayerJoin, PlayerTransfer,
    PortalAnswer, RegionEvent, StayNote, TickInputs, TickOutput, Ticket,
};
use crate::state::{Menu, MenuOf, RegionState};

const EDGE: EdgeId = EdgeId(1);
const OVERWORLD_HOME: RegionId = RegionId(3);
const NETHER_HOME: RegionId = RegionId(7);
const PART: RegionId = RegionId(11);
const ABSORBED: RegionId = RegionId(12);

/// The Nether's floor in the destination's worlds: stone at y = 40, air below and above.
const NETHER_FLOOR: i32 = 40;

// Worlds and frames.

/// A frame of obsidian along `axis`, without its corners, its inside two wide from
/// `along` and three high, standing on the floor at `floor` in the plane `plane`; full
/// of portal blocks of its axis if `lit`.
fn frame(
    axis: PortalAxis,
    along: i32,
    plane: i32,
    floor: i32,
    lit: bool,
) -> impl Fn(BlockPos) -> Option<BlockState> {
    move |pos| {
        let (a, p) = match axis {
            PortalAxis::X => (pos.x, pos.z),
            PortalAxis::Z => (pos.z, pos.x),
        };
        if p != plane {
            return None;
        }
        let bottom = floor + 1;
        let top = floor + 5;
        let inside_along = (along..=along + 1).contains(&a);
        let post = (a == along - 1 || a == along + 2) && pos.y > bottom && pos.y < top;
        let beam = (pos.y == bottom || pos.y == top) && inside_along;
        if post || beam {
            return Some(blocks::OBSIDIAN);
        }
        (lit && inside_along && pos.y > bottom && pos.y < top).then(|| portal(axis_name(axis)))
    }
}

/// The six blocks inside the frame of [`frame`].
fn inside(axis: PortalAxis, along: i32, plane: i32, floor: i32) -> Vec<BlockPos> {
    let mut found = Vec::new();
    for y in floor + 2..=floor + 4 {
        for a in along..=along + 1 {
            found.push(match axis {
                PortalAxis::X => at(a, y, plane),
                PortalAxis::Z => at(plane, y, a),
            });
        }
    }
    found
}

fn axis_name(axis: PortalAxis) -> &'static str {
    match axis {
        PortalAxis::X => "x",
        PortalAxis::Z => "z",
    }
}

type Blocks = Box<dyn Fn(BlockPos) -> Option<BlockState>>;

/// What any of `parts` names.
fn all(parts: Vec<Blocks>) -> impl Fn(BlockPos) -> Option<BlockState> {
    move |pos| parts.iter().find_map(|part| part(pos))
}

fn with_homes(region: &mut Region) {
    region.config.homes = vec![
        (Dimension::Overworld, OVERWORLD_HOME),
        (Dimension::Nether, NETHER_HOME),
    ];
}

fn tick_with(region: &mut Region, inputs: TickInputs) -> TickOutput {
    region.tick(&inputs)
}

fn start_edge(region: &mut Region, start: u64) -> TickOutput {
    tick_with(
        region,
        TickInputs {
            edges: vec![EdgeEvent::Started { edge: EDGE, start }],
            ..TickInputs::default()
        },
    )
}

/// The overworld of the tests of TNT with `blocks`, the homes and a known edge.
fn overworld(blocks: impl Fn(BlockPos) -> Option<BlockState> + 'static) -> Region {
    let mut region = world(1, blocks);
    with_homes(&mut region);
    start_edge(&mut region, 1);
    region
}

/// A Nether region with its floor at [`NETHER_FLOOR`] and stone under it down to the
/// bottom of the world, as the Nether's bedrock is, `blocks`, the homes and a known edge.
fn nether(blocks: impl Fn(BlockPos) -> Option<BlockState>) -> Region {
    nether_about(ChunkPos::new(0, 0), true, blocks)
}

/// The same with nothing under its floor: air down to the bottom of the world, as
/// [`world_in`] makes it.
fn nether_over_the_void(blocks: impl Fn(BlockPos) -> Option<BlockState>) -> Region {
    let mut region = world_in(Dimension::Nether, NETHER_FLOOR, blocks);
    with_homes(&mut region);
    start_edge(&mut region, 1);
    region
}

/// A Nether region as [`world_in`] makes one, but holding the 5×5 chunks about
/// `centre`, with stone under the floor if `solid`, and with the homes and a known edge.
fn nether_about(
    centre: ChunkPos,
    solid: bool,
    blocks: impl Fn(BlockPos) -> Option<BlockState>,
) -> Region {
    let rules = Dimension::Nether.rules();
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(centre.x + x, centre.z + z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    let lowest = if solid { rules.min_y } else { NETHER_FLOOR };
                    for y in lowest..=NETHER_FLOOR {
                        chunk.set(x, y, z, blocks::STONE);
                    }
                    for y in NETHER_FLOOR + 1..=NETHER_FLOOR + 12 {
                        let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                        if let Some(state) = blocks(pos) {
                            chunk.set(x, y, z, state);
                        }
                    }
                }
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let config = RegionConfig {
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: Dimension::Nether,
        spawn: None,
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 9,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    with_homes(&mut region);
    start_edge(&mut region, 1);
    region
}

/// The region as a new owner opens it from its state and its saved chunks: the chunks
/// it holds, all loaded with a viewer's ticket.
fn reopened(region: &Region) -> Region {
    let held: Vec<ChunkPos> = region.loaded_chunks();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    Region::restore_loaded(
        region.config.clone(),
        region.state(),
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    )
}

// What ticks say.

fn departed(output: &TickOutput, id: PlayerId) -> Option<(EdgeId, PlayerTransfer, RegionId)> {
    output
        .durable
        .iter()
        .find_map(|(edge, _, entry)| match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } if *player == id => Some((*edge, transfer.clone(), *to)),
            _ => None,
        })
}

/// The `Placed` of `id` the tick made: its dimension, pose and flying.
fn placed(
    output: &TickOutput,
    id: PlayerId,
) -> Option<(EdgeId, Dimension, crate::api::Pose, bool)> {
    output
        .durable
        .iter()
        .find_map(|(edge, _, entry)| match entry {
            Durable::Placed {
                player,
                dimension,
                pose,
                flying,
                ..
            } if *player == id => Some((*edge, *dimension, *pose, *flying)),
            _ => None,
        })
}

fn ended(output: &TickOutput, id: PlayerId) -> Vec<(EdgeId, EntityId, Option<u64>)> {
    output
        .durable
        .iter()
        .filter_map(|(edge, _, entry)| match entry {
            Durable::Ended {
                player,
                entity,
                attempt,
            } if *player == id => Some((*edge, *entity, *attempt)),
            _ => None,
        })
        .collect()
}

fn removed(output: &TickOutput, entity: EntityId) -> bool {
    output.events.iter().any(
        |event| matches!(event, RegionEvent::EntityRemoved { entity: gone, .. } if *gone == entity),
    )
}

/// Whether any event of the tick names `entity` as shown, moved or removed.
fn shown(output: &TickOutput, entity: EntityId) -> bool {
    output.events.iter().any(|event| match event {
        RegionEvent::EntitySpawned(state) | RegionEvent::EntityEntered(state) => {
            state.entity == entity
        }
        RegionEvent::EntityRemoved { entity: named, .. } => *named == entity,
        _ => false,
    })
}

fn has_notes(output: &TickOutput, id: PlayerId) -> Vec<(EntityId, u32, crate::api::Place, bool)> {
    output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Has {
                player,
                entity,
                hops,
                place,
                let_go,
                ..
            } if *player == id => Some((*entity, *hops, place.clone(), *let_go)),
            _ => None,
        })
        .collect()
}

fn gone_notes(output: &TickOutput, id: PlayerId) -> Vec<(EntityId, u32)> {
    output
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Gone {
                player,
                entity,
                hops,
            } if *player == id => Some((*entity, *hops)),
            _ => None,
        })
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    // The game's entity sizes are floats (0.6F, 1.8F, 0.98F), and so are a pose's
    // parts reckoned from them.
    (a - b).abs() < 1e-6
}

fn assert_at(position: Vec3, expected: [f64; 3]) {
    assert!(
        close(position.x, expected[0])
            && close(position.y, expected[1])
            && close(position.z, expected[2]),
        "{position:?} is not {expected:?}"
    );
}

fn relative_of(travel: &Travel) -> (Option<PortalAxis>, [f64; 3]) {
    match travel.way {
        Way::NetherPortal { axis, relative } => (axis, relative.map(|Double(value)| value)),
        other => panic!("not through a Nether portal: {other:?}"),
    }
}

/// Every Nether portal block of the region's loaded chunks between y `low` and `high`.
fn portal_blocks(region: &Region, low: i32, high: i32) -> Vec<(BlockPos, BlockState)> {
    let mut found = Vec::new();
    for chunk in region.loaded_chunks() {
        for x in chunk.x * 16..chunk.x * 16 + 16 {
            for z in chunk.z * 16..chunk.z * 16 + 16 {
                for y in low..=high {
                    let pos = at(x, y, z);
                    if let Some(state) = region.block(pos)
                        && clustine_blocks::data::is_block(state, blocks::NETHER_PORTAL)
                    {
                        found.push((pos, state));
                    }
                }
            }
        }
    }
    found
}

// The source.

/// Ticks the region twice: a creative player standing in a portal is marked in the
/// first and their clock fires in the second (P8's clock). The second tick's output.
fn fire(region: &mut Region, id: PlayerId) -> TickOutput {
    let marked = idle(region);
    assert!(departed(&marked, id).is_none(), "fired a tick early");
    idle(region)
}

/// A source frame along x off the origin, in the overworld: inside x -20 and -19, in
/// the plane z = -13.
fn overworld_source_x() -> Region {
    overworld(frame(PortalAxis::X, -20, -13, FLOOR, true))
}

/// A source frame along x in the overworld's ticking chunks, where entities tick:
/// inside x -12 and -11, in the plane z = -13.
fn ticking_source_x() -> Region {
    overworld(frame(PortalAxis::X, -12, -13, FLOOR, true))
}

/// Sets where player `id` looks.
fn look(region: &mut Region, id: PlayerId, yaw: f32, pitch: f32) {
    let player = region.players.get_mut(&id).expect("in the region");
    player.pose.yaw = yaw;
    player.pose.pitch = pitch;
}

/// 16.2 (a player, steps 1 to 3), 15.4 (D1, D2), the brief's 2.2 items 1, 2 and 6: a
/// player in the overworld is let go to the Nether's home in the tick their clock
/// fires, their transfer in the Nether's coordinates at the target (x and z divided by
/// 8, y kept, as a block) with the look kept, and their place in the source portal
/// along, up and across; reported removed at the source's chunk; the let-go note keeps
/// the target and the travel, with the source's pose as its face.
#[test]
fn a_player_in_an_overworld_portal_along_x_is_let_go_to_the_nethers_home_at_the_scaled_target() {
    let mut region = overworld_source_x();
    let feet = [-19.5, -58.4, -12.7];
    let (id, entity) = add_player(&mut region, 1, feet, true);
    look(&mut region, id, 30.0, -20.0);
    let fired = fire(&mut region, id);
    let (edge, transfer, to) = departed(&fired, id).expect("let go in the tick it fired");
    assert_eq!(edge, EDGE);
    assert_eq!(to, NETHER_HOME);
    assert!(!region.players.contains_key(&id));
    assert!(region.waiting_state(id).is_none());

    // -19.5 / 8 = -2.4375 and -12.7 / 8 = -1.5875, floored; y -58.4 floored.
    let target = at(-3, -59, -2);
    let travel = transfer.travel.expect("a travel");
    assert_eq!(travel.to, Dimension::Nether);
    assert_eq!(travel.target, target);
    // In the 2 × 3 rectangle from (-20, -59, -13): along (−19.5 − (−20 + 0.3)) / 1.4,
    // up (−58.4 + 59) / 1.2, across −12.7 − (−13 + 0.5).
    let (axis, relative) = relative_of(&travel);
    assert_eq!(axis, Some(PortalAxis::X));
    assert!(close(relative[0], 0.2 / 1.4), "{relative:?}");
    assert!(close(relative[1], 0.5), "{relative:?}");
    assert!(close(relative[2], -0.2), "{relative:?}");

    assert_eq!(transfer.dimension, Dimension::Nether);
    assert_eq!(transfer.entity_id, entity);
    assert!(transfer.flying);
    // At the target block's bottom centre, the look kept.
    assert_at(transfer.pose.position, [-2.5, -59.0, -1.5]);
    assert_eq!((transfer.pose.yaw, transfer.pose.pitch), (30.0, -20.0));

    // Removed for the old dimension's watchers, at the source's chunk.
    assert!(fired.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityRemoved { entity: gone, chunk }
            if *gone == entity && *chunk == ChunkPos::containing(feet[0], feet[2])
    )));

    // The let-go note keeps the place at the target, with the travel, and its face is
    // where they stood at the portal.
    let let_go: Vec<_> = fired
        .stays
        .iter()
        .filter_map(|note| match note {
            StayNote::Has {
                player,
                entity: stay,
                place,
                face,
                let_go: true,
                ..
            } if *player == id => Some((*stay, place.clone(), face.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(let_go.len(), 1, "{:?}", fired.stays);
    let (stay, place, face) = &let_go[0];
    assert_eq!(*stay, entity);
    assert_eq!(place.travel, Some(travel));
    assert_eq!(place.dimension, Dimension::Nether);
    assert_at(place.pose.position, [-2.5, -59.0, -1.5]);
    assert!(
        close(face.position[0], feet[0])
            && close(face.position[1], feet[1])
            && close(face.position[2], feet[2]),
        "the face is the source's pose: {:?}",
        face.position
    );
}

/// The brief's 2.2 item 6 for a portal along z: along is z, across is x.
#[test]
fn a_player_in_a_portal_along_z_takes_their_place_along_z_and_across_x() {
    let mut region = overworld(frame(PortalAxis::Z, 10, 20, FLOOR, true));
    let feet = [20.6, -59.0, 11.0];
    let (id, _) = add_player(&mut region, 1, feet, false);
    let fired = fire(&mut region, id);
    let (_, transfer, _) = departed(&fired, id).expect("let go");
    let travel = transfer.travel.expect("a travel");
    // 20.6 / 8 = 2.575 and 11 / 8 = 1.375.
    assert_eq!(travel.target, at(2, -59, 1));
    let (axis, relative) = relative_of(&travel);
    assert_eq!(axis, Some(PortalAxis::Z));
    // Along: (11 − (10 + 0.3)) / 1.4; across: 20.6 − (20 + 0.5).
    assert!(close(relative[0], 0.5), "{relative:?}");
    assert!(close(relative[1], 0.0), "{relative:?}");
    assert!(close(relative[2], 0.1), "{relative:?}");
}

/// The brief's 2.2 items 1 and 2: from the Nether, to the overworld's home, x and z
/// multiplied by 8 and y kept.
#[test]
fn a_player_in_a_nether_portal_is_let_go_to_the_overworlds_home_at_eight_times_their_place() {
    let mut region = nether(frame(PortalAxis::X, -20, -13, NETHER_FLOOR, true));
    let feet = [-19.0, 42.0, -12.5];
    let (id, entity) = add_player(&mut region, 1, feet, false);
    let fired = fire(&mut region, id);
    let (_, transfer, to) = departed(&fired, id).expect("let go");
    assert_eq!(to, OVERWORLD_HOME);
    let travel = transfer.travel.expect("a travel");
    assert_eq!(travel.to, Dimension::Overworld);
    assert_eq!(travel.target, at(-152, 42, -100));
    assert_eq!(transfer.dimension, Dimension::Overworld);
    assert_at(transfer.pose.position, [-151.5, 42.0, -99.5]);
    let (axis, relative) = relative_of(&travel);
    assert_eq!(axis, Some(PortalAxis::X));
    assert!(close(relative[0], 0.5) && close(relative[1], 0.0) && close(relative[2], 0.0));
    assert!(removed(&fired, entity));
}

/// 16.1 and the brief's 2.2 item 2: the scaled target is clamped inside the
/// destination's border (`clampToBounds`: at most 29 999 984 − 1e−5, as a block).
#[test]
fn a_target_beyond_the_border_is_clamped_inside_it() {
    let centre = ChunkPos::new(234_375, 0);
    let mut region = nether_about(
        centre,
        true,
        frame(PortalAxis::X, 3_749_999, 5, NETHER_FLOOR, true),
    );
    let feet = [3_750_000.0, 42.0, 5.5];
    let (id, _) = add_player(&mut region, 1, feet, false);
    let fired = fire(&mut region, id);
    let (_, transfer, _) = departed(&fired, id).expect("let go");
    let travel = transfer.travel.expect("a travel");
    // 3 750 000 × 8 = 30 000 000, past the border.
    assert_eq!(travel.target, at(29_999_983, 42, 44));
}

/// 16.7 (D5): the player's menu is closed at the firing as at a leave
/// (`menu_leaving`): the stack they carried on the cursor is thrown where they stood.
#[test]
fn a_menu_open_at_the_firing_is_closed_as_at_a_leave() {
    let chest = at(-11, -60, -11);
    let ender = state("minecraft:ender_chest[facing=north,waterlogged=false]");
    let mut region = overworld(all(vec![
        Box::new(frame(PortalAxis::X, -12, -13, FLOOR, true)),
        Box::new(move |pos| (pos == chest).then_some(ender)),
    ]));
    let (id, _) = add_player(&mut region, 1, [-11.5, -59.0, -12.7], false);
    region.players.get_mut(&id).expect("added").menu = Some(Menu {
        serial: 1,
        of: MenuOf::Ender { at: chest },
        kind: crate::api::MenuKind::Rows3,
        carried: Some(ItemStack::new(items::DIAMOND, 3)),
        drag: None,
        trade: None,
    });
    let fired = fire(&mut region, id);
    let (_, transfer, _) = departed(&fired, id).expect("let go");
    // Not carried on into the transfer, but thrown.
    assert!(
        !transfer
            .inventory
            .0
            .iter()
            .flatten()
            .any(|stack| stack.item == items::DIAMOND)
    );
    let thrown = super::super::tnt::tests::records(&region)
        .into_iter()
        .any(|entity| {
            matches!(&entity.data, EntityData::Item { stack, .. } if stack.item == items::DIAMOND && stack.count == 3)
        });
    assert!(thrown, "the carried stack was not thrown");
}

/// 16.7 (D9): an edge's reset after a firing reports no second removal of the player;
/// the firing reported it, at the source's chunk.
#[test]
fn an_edges_reset_after_a_firing_does_not_report_the_traveller_removed_again() {
    let mut region = overworld_source_x();
    let (id, entity) = add_player(&mut region, 1, [-19.5, -59.0, -12.7], false);
    let fired = fire(&mut region, id);
    assert!(departed(&fired, id).is_some());
    assert!(removed(&fired, entity));
    let reset = start_edge(&mut region, 2);
    assert!(!removed(&reset, entity), "{:?}", reset.events);
}

/// An armour stand at `position`.
fn stand(region: &mut Region, position: [f64; 3]) -> clustine_world::Entity {
    record(
        region,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

/// 16.2 (an entity, steps 1 and 2), 5.1 (riders), 15.4 (D6): a cart with an armour
/// stand on it is removed with its rider, each reported gone by a portal with the hops
/// it had, and filed as one traveller for the target's chunk in the Nether, root first,
/// their hops one higher.
#[test]
fn a_cart_and_its_rider_are_filed_as_one_traveller_root_first_with_one_hop_more() {
    let mut region = ticking_source_x();
    let position = [-11.0, -59.0, -12.5];
    let vehicle = cart(&mut region, entity_types::MINECART, position);
    let rider = stand(&mut region, position);
    let rider = add(&mut region, rider);
    assert!(region.start_riding(rider, vehicle, true, &mut TickOutput::default()));
    let marked = idle(&mut region);
    assert!(marked.travellers.is_empty());
    let fired = idle(&mut region);
    assert!(region.find_entity(vehicle).is_none());
    assert!(region.find_entity(rider).is_none());
    assert_eq!(fired.travellers.len(), 1, "{:?}", fired.travellers);
    let traveller = &fired.travellers[0];
    // -11 / 8 = -1.375 and -12.5 / 8 = -1.5625: the block (-2, -59, -2), in chunk
    // (-1, -1).
    assert_eq!(traveller.travel.to, Dimension::Nether);
    assert_eq!(traveller.travel.target, at(-2, -59, -2));
    assert_eq!(
        traveller.chunk,
        LevelChunk::new(Dimension::Nether, ChunkPos::new(-1, -1))
    );
    let keys: Vec<EntityId> = traveller.entities.iter().map(|entity| entity.key).collect();
    assert_eq!(keys, [vehicle, rider]);
    assert!(traveller.entities.iter().all(|entity| entity.hops == 1));
    for key in [vehicle, rider] {
        assert!(
            fired.events.iter().any(|event| matches!(
                event,
                RegionEvent::EntityGone { entity, travelled: true, hops: 0, .. } if *entity == key
            )),
            "{key:?} not reported gone by a portal"
        );
    }
}

/// 16.7 (D8): a vehicle with a player in it does not fire until P9b.
#[test]
fn a_vehicle_with_a_player_in_it_does_not_fire() {
    let mut region = ticking_source_x();
    let position = [-11.0, -59.0, -12.5];
    let vehicle = cart(&mut region, entity_types::MINECART, position);
    let (id, entity) = add_player(&mut region, 1, position, false);
    assert!(region.mount_player(entity, vehicle, &mut TickOutput::default()));
    for _ in 0..20 {
        let output = idle(&mut region);
        assert!(output.travellers.is_empty());
        assert!(departed(&output, id).is_none());
    }
    assert!(region.find_entity(vehicle).is_some());
    assert!(region.players.contains_key(&id));
}

// The destination.

/// The transfer of player `number` let go from an overworld portal along x, at the
/// middle bottom of it, as the source made it; then aimed at `target` in `to`, through
/// a portal of `axis` at `relative`, looking at (`yaw`, `pitch`).
fn traveller_to(
    number: u128,
    to: Dimension,
    target: BlockPos,
    axis: PortalAxis,
    relative: [f64; 3],
) -> (PlayerId, PlayerTransfer) {
    let mut source = overworld(frame(PortalAxis::X, 2, 5, FLOOR, true));
    let (id, _) = add_player(&mut source, number, [3.0, -59.0, 5.5], false);
    look(&mut source, id, 30.0, -20.0);
    let (_, mut transfer, _) = departed(&fire(&mut source, id), id).expect("let go");
    let travel = transfer.travel.as_mut().expect("a travel");
    travel.to = to;
    travel.target = target;
    travel.way = Way::NetherPortal {
        axis: Some(axis),
        relative: relative.map(Double),
    };
    transfer.dimension = to;
    transfer.pose.position = Vec3::new(
        f64::from(target.x) + 0.5,
        f64::from(target.y),
        f64::from(target.z) + 0.5,
    );
    (id, transfer)
}

/// The same into the Nether, from the middle bottom of a portal along x.
fn traveller(number: u128, target: BlockPos) -> (PlayerId, PlayerTransfer) {
    traveller_to(
        number,
        Dimension::Nether,
        target,
        PortalAxis::X,
        [0.5, 0.0, 0.0],
    )
}

fn arrive(region: &mut Region, id: PlayerId, transfer: PlayerTransfer) -> TickOutput {
    tick_with(
        region,
        TickInputs {
            player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer)],
            ..TickInputs::default()
        },
    )
}

/// Answers every query of `output` with `portals`.
fn answer(region: &mut Region, output: &TickOutput, portals: &[BlockPos]) -> TickOutput {
    let answers = output
        .portal_queries
        .iter()
        .map(|query| PortalAnswer {
            id: query.id,
            portals: portals.to_vec(),
        })
        .collect();
    tick_with(
        region,
        TickInputs {
            portal_answers: answers,
            ..TickInputs::default()
        },
    )
}

/// 16.3, 16.7 (D1, D3) and 16.8 (A1): a player arriving with a travel waits apart, not
/// in the world and shown to nobody, held for the edge they came through; the region
/// asks the index once, around the target with the Nether's radius of 16, notes the
/// stay with its travel, and places nothing without the answer, however long it waits
/// (the limit of 200 ticks is on loading only).
#[test]
fn a_player_arriving_with_a_travel_waits_asks_once_and_is_never_placed_without_the_answer() {
    let mut region = nether(|_| None);
    let target = at(6, 41, -3);
    let (id, transfer) = traveller(1, target);
    let entity = transfer.entity_id;
    let travel = transfer.travel.expect("a travel");
    let taken = arrive(&mut region, id, transfer);
    assert!(!region.players.contains_key(&id));
    let waiting = region.waiting_state(id).expect("waiting");
    assert_eq!(waiting.edge, EDGE);
    assert_eq!(waiting.transfer.entity_id, entity);
    assert!(region.entity(entity).is_none());
    assert!(!shown(&taken, entity));
    assert_eq!(taken.portal_queries.len(), 1, "{:?}", taken.portal_queries);
    assert_eq!(taken.portal_queries[0].centre, target);
    assert_eq!(taken.portal_queries[0].radius, 16);
    let notes = has_notes(&taken, id);
    assert!(
        notes
            .iter()
            .any(|(stay, _, place, _)| *stay == entity && place.travel == Some(travel)),
        "{notes:?}"
    );
    for _ in 0..250 {
        let output = idle(&mut region);
        assert!(output.portal_queries.is_empty(), "asked again");
        assert!(placed(&output, id).is_none(), "placed without the answer");
        assert!(!shown(&output, entity));
    }
    assert!(region.waiting_state(id).is_some());
    assert!(!region.players.contains_key(&id));
}

/// The brief's 2.2 item 3: out of the Nether the radius is 128.
#[test]
fn a_player_arriving_in_the_overworld_asks_with_a_radius_of_128() {
    let mut region = overworld(|_| None);
    let target = at(9, -60, 9);
    let (id, transfer) = traveller_to(
        1,
        Dimension::Overworld,
        target,
        PortalAxis::X,
        [0.5, 0.0, 0.0],
    );
    let taken = arrive(&mut region, id, transfer);
    assert!(region.waiting_state(id).is_some());
    assert_eq!(taken.portal_queries.len(), 1);
    assert_eq!(taken.portal_queries[0].centre, target);
    assert_eq!(taken.portal_queries[0].radius, 128);
    // No portal there: one is made on the floor, and they are placed in it in the
    // overworld.
    let output = answer(&mut region, &taken, &[]);
    let (_, dimension, pose, _) = placed(&output, id).expect("placed");
    assert_eq!(dimension, Dimension::Overworld);
    assert_at(pose.position, [10.0, -60.0, 9.5]);
}

/// Two portals along z in the Nether: `A` in the plane x = 4, inside z 0 and 1 and y 42
/// to 44; `B` in the plane x = -4, inside z 0 and 1 and y 48 to 50.
fn two_portals(b: bool) -> Region {
    let mut parts: Vec<Blocks> = vec![Box::new(frame(PortalAxis::Z, 0, 4, NETHER_FLOOR, true))];
    if b {
        parts.push(Box::new(frame(
            PortalAxis::Z,
            0,
            -4,
            NETHER_FLOOR + 6,
            true,
        )));
    }
    nether(all(parts))
}

fn portal_a() -> Vec<BlockPos> {
    inside(PortalAxis::Z, 0, 4, NETHER_FLOOR)
}

fn portal_b() -> Vec<BlockPos> {
    inside(PortalAxis::Z, 0, -4, NETHER_FLOOR + 6)
}

/// 16.4 items 2, 3 and 5, the brief's 2.2 items 3 and 6: of the index's portals the one
/// nearest the target by `distSqr` is chosen, then the lowest; the player is placed at
/// their place in it (along z, across x), the yaw turned by 90 as the axes differ and
/// the pitch kept.
#[test]
fn of_two_portals_as_near_the_lower_is_chosen_and_the_yaw_turns_where_the_axes_differ() {
    let mut region = two_portals(true);
    // From (0, 46, 0) A's nearest block (4, 44, 0) and B's (-4, 48, 0) are both at 20.
    let (id, transfer) = traveller_to(
        1,
        Dimension::Nether,
        at(0, 46, 0),
        PortalAxis::X,
        [0.25, 0.0, -0.2],
    );
    let taken = arrive(&mut region, id, transfer);
    let portals = [portal_a(), portal_b()].concat();
    let output = answer(&mut region, &taken, &portals);
    let (edge, dimension, pose, _) = placed(&output, id).expect("placed");
    assert_eq!(edge, EDGE);
    assert_eq!(dimension, Dimension::Nether);
    // In A, from (4, 42, 0): along z 0.3 + 1.4 × 0.25, across x 0.5 − 0.2, at its bottom.
    assert_at(pose.position, [4.3, 42.0, 0.65]);
    assert_eq!(pose.yaw, 120.0);
    assert_eq!(pose.pitch, -20.0);
    let (entity, at_pose) = region.player(id).expect("in the world");
    assert_eq!(entity, taken_entity(&region, id));
    assert_at(at_pose.position, [4.3, 42.0, 0.65]);
    assert!(region.waiting_state(id).is_none());
}

fn taken_entity(region: &Region, id: PlayerId) -> EntityId {
    region.player(id).expect("in the world").0
}

/// 16.4 item 2: the nearest wins over the lower.
#[test]
fn the_portal_nearest_the_target_is_chosen_though_it_is_higher() {
    let mut region = two_portals(true);
    // From (0, 49, 0): B's (-4, 49, 0) at 16, A's (4, 44, 0) at 41.
    let (id, transfer) = traveller_to(
        1,
        Dimension::Nether,
        at(0, 49, 0),
        PortalAxis::X,
        [0.25, 0.0, -0.2],
    );
    let taken = arrive(&mut region, id, transfer);
    let portals = [portal_a(), portal_b()].concat();
    let output = answer(&mut region, &taken, &portals);
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    assert_at(pose.position, [-3.7, 48.0, 0.65]);
}

/// 16.4 item 1: a candidate in a loaded chunk is kept only if its block is still a
/// portal; the index's word for a block that is not is passed over.
#[test]
fn a_portal_the_index_names_that_is_gone_is_passed_over() {
    let mut region = two_portals(false);
    let (id, transfer) = traveller_to(
        1,
        Dimension::Nether,
        at(0, 49, 0),
        PortalAxis::X,
        [0.25, 0.0, -0.2],
    );
    let taken = arrive(&mut region, id, transfer);
    // B is named, nearer, but its blocks are air.
    let portals = [portal_a(), portal_b()].concat();
    let output = answer(&mut region, &taken, &portals);
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    assert_at(pose.position, [4.3, 42.0, 0.65]);
    // No portal was made.
    assert_eq!(portal_blocks(&region, 0, 127).len(), 6);
}

/// The brief's 2.2 item 6: where the axes match the yaw is kept.
#[test]
fn where_the_axes_match_the_yaw_is_kept() {
    let mut region = nether(frame(PortalAxis::X, 2, 5, NETHER_FLOOR, true));
    let (id, transfer) = traveller(1, at(0, 41, 0));
    let taken = arrive(&mut region, id, transfer);
    let output = answer(
        &mut region,
        &taken,
        &inside(PortalAxis::X, 2, 5, NETHER_FLOOR),
    );
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    assert_at(pose.position, [3.0, 42.0, 5.5]);
    assert_eq!((pose.yaw, pose.pitch), (30.0, -20.0));
}

/// The frame and portal blocks `createPortal` makes at `spot` along `axis` (the brief,
/// section 2.3): obsidian on the edges of 4 × 5 from one before `spot` along and one
/// below it, the portal's 2 × 3 inside.
fn made_portal(axis: PortalAxis, spot: BlockPos) -> (Vec<BlockPos>, Vec<BlockPos>) {
    let step = |along: i32, up: i32| match axis {
        PortalAxis::X => at(spot.x + along, spot.y + up, spot.z),
        PortalAxis::Z => at(spot.x, spot.y + up, spot.z + along),
    };
    let mut obsidian = Vec::new();
    let mut portals = Vec::new();
    for along in -1..=2 {
        for up in -1..=3 {
            if along == -1 || along == 2 || up == -1 || up == 3 {
                obsidian.push(step(along, up));
            } else {
                portals.push(step(along, up));
            }
        }
    }
    (obsidian, portals)
}

/// 16.4 item 4 and the brief's 2.3: where there is no portal, one is made on flat
/// ground at the nearest spot of the spiral, the target's own column on its floor,
/// along the source's axis; its blocks are in the commit of the placing; the player
/// stands in it at their place.
#[test]
fn where_there_is_no_portal_one_is_made_on_the_flat_ground_at_the_target() {
    let mut region = nether(|_| None);
    let (id, transfer) = traveller(1, at(6, 41, -3));
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, &[]);
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    let (obsidian, portals) = made_portal(PortalAxis::X, at(6, 41, -3));
    for pos in &obsidian {
        assert_eq!(region.block(*pos), Some(blocks::OBSIDIAN), "{pos:?}");
    }
    for pos in &portals {
        assert_eq!(region.block(*pos), Some(portal("x")), "{pos:?}");
        assert!(
            output.events.iter().any(|event| matches!(
                event,
                RegionEvent::BlockChanged { position, .. } if position == pos
            )),
            "{pos:?} not changed in the tick of the placing"
        );
    }
    assert_eq!(portal_blocks(&region, 0, 127).len(), 6);
    assert_at(pose.position, [7.0, 41.0, -2.5]);
}

/// The brief's 2.3: the spiral walks each column down to the world's bottom, where the
/// game reads void air below it (replaceable, not solid): a floor with air under it is
/// as good a floor as any, and the portal is made on it, not on the fallback's
/// platform at y = 70.
#[test]
fn a_portal_is_made_on_a_floor_with_air_under_it() {
    let mut region = nether_over_the_void(|_| None);
    let (id, transfer) = traveller(1, at(6, 41, -3));
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, &[]);
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    let (_, portals) = made_portal(PortalAxis::X, at(6, 41, -3));
    for pos in &portals {
        assert_eq!(region.block(*pos), Some(portal("x")), "{pos:?}");
    }
    assert_at(pose.position, [7.0, 41.0, -2.5]);
}

/// The brief's 2.3: a portal made for a traveller through a portal along z lies along z
/// (its direction south).
#[test]
fn a_portal_made_for_a_traveller_through_a_portal_along_z_lies_along_z() {
    let mut region = nether(|_| None);
    let (id, transfer) = traveller_to(
        1,
        Dimension::Nether,
        at(6, 41, -3),
        PortalAxis::Z,
        [0.5, 0.0, 0.0],
    );
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, &[]);
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    let (obsidian, portals) = made_portal(PortalAxis::Z, at(6, 41, -3));
    for pos in &obsidian {
        assert_eq!(region.block(*pos), Some(blocks::OBSIDIAN), "{pos:?}");
    }
    for pos in &portals {
        assert_eq!(region.block(*pos), Some(portal("z")), "{pos:?}");
    }
    assert_at(pose.position, [6.5, 41.0, -2.0]);
}

/// 5.2 step 1, 16.3 item 3: two travellers placed in one tick are placed in order, the
/// second seeing the portal the first made.
#[test]
fn two_players_placed_in_one_tick_share_the_one_portal_made() {
    let mut region = nether(|_| None);
    let target = at(6, 41, -3);
    let (first, one) = traveller(1, target);
    let (second, two) = traveller(2, target);
    let taken = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![
                PlayerChange::Arrive(EDGE, first, one),
                PlayerChange::Arrive(EDGE, second, two),
            ],
            ..TickInputs::default()
        },
    );
    assert_eq!(taken.portal_queries.len(), 2);
    let output = answer(&mut region, &taken, &[]);
    let (_, _, a, _) = placed(&output, first).expect("the first placed");
    let (_, _, b, _) = placed(&output, second).expect("the second placed");
    assert_eq!(portal_blocks(&region, 0, 127).len(), 6);
    assert_at(a.position, [7.0, 41.0, -2.5]);
    assert_at(b.position, [7.0, 41.0, -2.5]);
}

/// 16.4 item 1: the region's own portal blocks within the square are candidates beside
/// the index's answer, so a later traveller whose answer does not have the portal an
/// earlier one made since is placed in it.
#[test]
fn a_later_traveller_finds_the_portal_an_earlier_one_made_though_its_answer_does_not_name_it() {
    let mut region = nether(|_| None);
    let target = at(6, 41, -3);
    let (first, one) = traveller(1, target);
    let (second, two) = traveller(2, target);
    let taken_one = arrive(&mut region, first, one);
    let taken_two = arrive(&mut region, second, two);
    let output = answer(&mut region, &taken_one, &[]);
    assert!(placed(&output, first).is_some());
    let output = answer(&mut region, &taken_two, &[]);
    let (_, _, pose, _) = placed(&output, second).expect("placed");
    assert_at(pose.position, [7.0, 41.0, -2.5]);
    assert_eq!(portal_blocks(&region, 0, 127).len(), 6);
}

/// 16.4 item 6: the player placed is in the world from this tick, shown as an arrival
/// is, with their effects resent (`EffectAdded`), and `Placed` goes to their edge.
#[test]
fn a_player_placed_is_shown_and_their_effects_are_sent_again() {
    let mut region = nether(|_| None);
    let (id, mut transfer) = traveller(1, at(6, 41, -3));
    transfer.effects = vec![EffectInstance {
        effect: "minecraft:speed".to_owned(),
        amplifier: 1,
        duration: 600,
        ambient: false,
        visible: true,
        show_icon: true,
    }];
    let entity = transfer.entity_id;
    let taken = arrive(&mut region, id, transfer);
    let output = answer(&mut region, &taken, &[]);
    let (edge, _, _, flying) = placed(&output, id).expect("placed");
    assert_eq!(edge, EDGE);
    assert!(!flying);
    assert!(shown(&output, entity), "not shown: {:?}", output.events);
    assert!(output.player_events.iter().any(|(player, event)| *player == id
        && matches!(
            event,
            crate::api::PlayerEvent::EffectAdded { instance, .. } if instance.effect == "minecraft:speed"
        )));
}

/// A cart filed from the standard overworld portal, aimed at `target` in the Nether.
fn cart_traveller(target: BlockPos) -> (EntityId, Traveller) {
    let mut source = overworld(frame(PortalAxis::X, 2, 5, FLOOR, true));
    let key = cart(&mut source, entity_types::MINECART, [3.0, -59.0, 5.5]);
    idle(&mut source);
    let mut traveller = idle(&mut source).travellers.remove(0);
    traveller.travel.target = target;
    traveller.chunk = LevelChunk::new(
        Dimension::Nether,
        ChunkPos::containing(f64::from(target.x), f64::from(target.z)),
    );
    (key, traveller)
}

/// 16.7 (D1) and 16.8 (A5): an entity traveller is taken in the tick it comes, its
/// number among the tick's `arrivals_taken`; one handed in again while it waits is
/// passed over; it is placed once, at its place in the portal.
#[test]
fn an_entity_traveller_is_taken_in_the_tick_it_comes_and_placed_once_though_handed_in_twice() {
    let mut region = nether(frame(PortalAxis::X, 2, 5, NETHER_FLOOR, true));
    let (key, traveller) = cart_traveller(at(0, 41, 0));
    let number = EffectId {
        sequence: 4,
        index: 0,
    };
    let taken = tick_with(
        &mut region,
        TickInputs {
            travellers: vec![(number, traveller.clone())],
            ..TickInputs::default()
        },
    );
    assert_eq!(taken.arrivals_taken, [number]);
    assert_eq!(taken.portal_queries.len(), 1);
    assert_eq!(region.state().waiting_travellers.len(), 1);
    assert!(region.find_entity(key).is_none());
    let again = tick_with(
        &mut region,
        TickInputs {
            travellers: vec![(number, traveller)],
            ..TickInputs::default()
        },
    );
    assert!(again.portal_queries.is_empty());
    assert_eq!(region.state().waiting_travellers.len(), 1);
    let output = answer(
        &mut region,
        &taken,
        &inside(PortalAxis::X, 2, 5, NETHER_FLOOR),
    );
    assert!(region.state().waiting_travellers.is_empty());
    let (_, placed) = region.find_entity(key).expect("placed");
    // In the portal from (2, 42, 5): along 0.49 + 1.02 × 0.5, at its bottom, across 0.5.
    let position = placed.position;
    assert!(
        close(position[0], 3.0) && close(position[1], 42.0) && close(position[2], 5.5),
        "{position:?}"
    );
    let carts = super::super::tnt::tests::records(&region)
        .into_iter()
        .filter(|entity| entity.key == key)
        .count();
    assert_eq!(carts, 1);
    assert!(output.travellers.is_empty());
}

// The stay rules of a waiting stay.

/// A Nether region with player 1 waiting there, aimed at (6, 41, -3), their transfer
/// carrying `attempt`.
fn one_waiting(attempt: Option<u64>) -> (Region, PlayerId, PlayerTransfer) {
    let mut region = nether(|_| None);
    let (id, mut transfer) = traveller(1, at(6, 41, -3));
    transfer.attempt = attempt;
    arrive(&mut region, id, transfer.clone());
    assert!(region.waiting_state(id).is_some());
    (region, id, transfer)
}

fn leave_by_entity() -> (TickOutput, Region, PlayerId, PlayerTransfer) {
    let (mut region, id, transfer) = one_waiting(None);
    let output = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Leave(
                EDGE,
                id,
                Some(transfer.entity_id),
                None,
            )],
            ..TickInputs::default()
        },
    );
    (output, region, id, transfer)
}

/// 16.8 (A1): a leave that names the stay's entity ends a waiting stay; its entity,
/// shown to nobody here, is not reported removed, and nobody is told.
#[test]
fn a_leave_by_entity_ends_a_waiting_stay() {
    let (output, region, id, transfer) = leave_by_entity();
    assert!(region.waiting_state(id).is_none());
    assert!(!region.players.contains_key(&id));
    assert!(!removed(&output, transfer.entity_id));
    assert!(ended(&output, id).is_empty());
}

/// 16.8 (A1): "a leave ... end[s] it ... with a `Has` that keeps the travel (a leave)
/// or a `Gone` (a reset)".
#[test]
fn a_leave_of_a_waiting_stay_notes_a_has_that_keeps_the_travel() {
    let (output, _, id, transfer) = leave_by_entity();
    let entity = transfer.entity_id;
    let notes = has_notes(&output, id);
    assert!(
        notes
            .iter()
            .any(|(stay, _, place, _)| *stay == entity && place.travel == transfer.travel),
        "no Has keeping the travel: {:?}",
        output.stays
    );
}

/// 16.8 (A1), ADR-0020 section 4.4: a leave without an entity ends the waiting stay of
/// that edge with that attempt, and no other.
#[test]
fn a_leave_by_attempt_ends_only_the_waiting_stay_of_that_attempt() {
    let (mut region, id, transfer) = one_waiting(Some(5));
    let other = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Leave(EDGE, id, None, Some(6))],
            ..TickInputs::default()
        },
    );
    assert!(
        region.waiting_state(id).is_some(),
        "ended by another attempt"
    );
    assert!(
        has_notes(&other, id)
            .iter()
            .all(|(_, _, _, let_go)| !let_go)
    );
    let output = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Leave(EDGE, id, None, Some(5))],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert!(!removed(&output, transfer.entity_id));
    assert!(ended(&output, id).is_empty(), "a leave tells nobody");
}

/// 16.8 (A1), ADR-0008: an edge's reset (a higher start) ends its waiting stays with a
/// `Gone`, and reports no removal.
#[test]
fn an_edges_reset_ends_its_waiting_stay_with_a_gone() {
    let (mut region, id, transfer) = one_waiting(None);
    let output = start_edge(&mut region, 2);
    assert!(region.waiting_state(id).is_none());
    assert_eq!(
        gone_notes(&output, id),
        [(transfer.entity_id, transfer.hops)]
    );
    assert!(!removed(&output, transfer.entity_id));
    assert!(ended(&output, id).is_empty());
}

/// 16.8 (A1): an edge gone ends its waiting stays as a reset does.
#[test]
fn an_edge_gone_ends_its_waiting_stay_with_a_gone_and_no_removal() {
    let (mut region, id, transfer) = one_waiting(None);
    let output = tick_with(
        &mut region,
        TickInputs {
            edges: vec![EdgeEvent::Gone { edge: EDGE }],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert_eq!(
        gone_notes(&output, id),
        [(transfer.entity_id, transfer.hops)]
    );
    assert!(!removed(&output, transfer.entity_id));
}

/// 16.8 (A1), ADR-0020 section 5: a `Dead` above a waiting stay ends it and tells its
/// edge with `Ended`; one not above it leaves it.
#[test]
fn a_dead_stay_below_the_word_is_ended_and_its_edge_told() {
    let (mut region, id, transfer) = one_waiting(Some(5));
    let entity = transfer.entity_id;
    let same = tick_with(
        &mut region,
        TickInputs {
            dead: vec![(id, entity, transfer.hops)],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_some(), "ended by its own word");
    assert!(ended(&same, id).is_empty());
    let output = tick_with(
        &mut region,
        TickInputs {
            dead: vec![(id, EntityId(entity.0 + 1), 0)],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert_eq!(ended(&output, id), [(EDGE, entity, Some(5))]);
    assert!(!removed(&output, entity));
}

/// ADR-0020 section 5: a stay with fewer hand-overs than the word's is dead too.
#[test]
fn a_waiting_stay_with_fewer_hops_than_the_dead_word_is_ended() {
    let (mut region, id, transfer) = one_waiting(None);
    let output = tick_with(
        &mut region,
        TickInputs {
            dead: vec![(id, transfer.entity_id, transfer.hops + 1)],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert_eq!(ended(&output, id), [(EDGE, transfer.entity_id, None)]);
}

/// 16.8 (A1), ADR-0020 section 6 (a join removes the present and the entering stay):
/// a later join ends a waiting stay and tells its edge.
#[test]
fn a_later_join_ends_a_waiting_stay_and_tells_its_edge() {
    let (mut region, id, transfer) = one_waiting(None);
    let output = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Join(
                EDGE,
                PlayerJoin {
                    player: id,
                    name: "player1".to_owned(),
                    attempt: 9,
                },
            )],
            ..TickInputs::default()
        },
    );
    assert!(region.waiting_state(id).is_none());
    assert!(region.entering_state(id).is_some());
    assert_eq!(ended(&output, id), [(EDGE, transfer.entity_id, None)]);
}

/// ADR-0020 section 6, rule 7: an arrival of an earlier stay is passed over, its edge
/// told; the waiting stay stays.
#[test]
fn an_arrival_of_an_earlier_stay_is_passed_over_and_the_waiting_one_stays() {
    let (mut region, id, transfer) = one_waiting(None);
    let mut earlier = transfer.clone();
    earlier.entity_id = EntityId(transfer.entity_id.0 - 1);
    earlier.travel = None;
    let output = arrive(&mut region, id, earlier.clone());
    let waiting = region.waiting_state(id).expect("still waiting");
    assert_eq!(waiting.transfer.entity_id, transfer.entity_id);
    assert!(!region.players.contains_key(&id));
    assert_eq!(ended(&output, id), [(EDGE, earlier.entity_id, None)]);
}

/// Rule 7 the other way: a later stay arriving ends the waiting one, whose edge is told.
#[test]
fn an_arrival_of_a_later_stay_ends_the_waiting_one() {
    let (mut region, id, transfer) = one_waiting(None);
    let mut later = transfer.clone();
    later.entity_id = EntityId(transfer.entity_id.0 + 1);
    let output = arrive(&mut region, id, later.clone());
    let waiting = region.waiting_state(id).expect("the later one waits");
    assert_eq!(waiting.transfer.entity_id, later.entity_id);
    assert_eq!(ended(&output, id), [(EDGE, transfer.entity_id, None)]);
}

/// 16.7 and 16.8 (c): of a waiting stay's inputs those `has_no_place` passes are kept
/// and applied in order at the placing; the others are passed over; every one is
/// counted applied.
#[test]
fn inputs_that_need_no_place_are_kept_and_applied_at_the_placing_and_the_rest_passed_over() {
    let (mut region, id, transfer) = one_waiting(None);
    let entity = transfer.entity_id;
    let base = transfer.last_input;
    let inputs = vec![
        PlayerInput::Move {
            position: Some(Vec3::new(100.0, 41.0, 100.0)),
            rotation: None,
            on_ground: true,
        },
        PlayerInput::SelectSlot { slot: 4 },
        PlayerInput::Dig {
            position: at(6, 40, -3),
            sequence: 1,
        },
        PlayerInput::SetFlying { flying: true },
        PlayerInput::SelectSlot { slot: 6 },
    ];
    let mut tick = TickInputs::default();
    for (n, input) in inputs.into_iter().enumerate() {
        tick.input(EDGE, id, entity, base + 1 + n as u64, input);
    }
    let output = tick_with(&mut region, tick);
    let waiting = region.waiting_state(id).expect("waiting");
    assert_eq!(
        waiting.kept,
        [
            PlayerInput::SelectSlot { slot: 4 },
            PlayerInput::SetFlying { flying: true },
            PlayerInput::SelectSlot { slot: 6 },
        ]
    );
    assert_eq!(waiting.transfer.last_input, base + 5);
    // The dig was passed over: the floor is there.
    assert_eq!(region.block(at(6, 40, -3)), Some(blocks::STONE));
    assert!(placed(&output, id).is_none());
    // An input sent again is not kept twice.
    let mut again = TickInputs::default();
    again.input(
        EDGE,
        id,
        entity,
        base + 5,
        PlayerInput::SelectSlot { slot: 6 },
    );
    let asked = tick_with(&mut region, again);
    assert_eq!(region.waiting_state(id).expect("waiting").kept.len(), 3);
    assert!(asked.portal_queries.is_empty());

    let query = region
        .waiting_state(id)
        .and_then(|waiting| waiting.asked)
        .expect("asked");
    let output = tick_with(
        &mut region,
        TickInputs {
            portal_answers: vec![PortalAnswer {
                id: query,
                portals: Vec::new(),
            }],
            ..TickInputs::default()
        },
    );
    let (_, _, pose, _) = placed(&output, id).expect("placed");
    // Not moved by the move passed over.
    assert_at(pose.position, [7.0, 41.0, -2.5]);
    let player = &region.players[&id];
    assert_eq!(player.selected_slot, 6);
    assert!(player.flying);
    assert_eq!(player.last_input, base + 5);
}

/// 16.4 item 6 and 16.8 (c): the kept inputs apply at the placing, and `Placed` tells
/// the edge the pose and flying the player has once placed; a `SetFlying` kept while
/// waiting is among what they have.
#[test]
fn placed_says_the_flying_a_kept_set_flying_gave() {
    let (mut region, id, transfer) = one_waiting(None);
    assert!(!transfer.flying);
    let mut tick = TickInputs::default();
    tick.input(
        EDGE,
        id,
        transfer.entity_id,
        transfer.last_input + 1,
        PlayerInput::SetFlying { flying: true },
    );
    tick_with(&mut region, tick);
    let query = region
        .waiting_state(id)
        .and_then(|waiting| waiting.asked)
        .expect("asked");
    let output = tick_with(
        &mut region,
        TickInputs {
            portal_answers: vec![PortalAnswer {
                id: query,
                portals: Vec::new(),
            }],
            ..TickInputs::default()
        },
    );
    let (_, _, _, flying) = placed(&output, id).expect("placed");
    assert!(region.players[&id].flying);
    assert!(flying, "Placed says not flying");
}

// Restore and reshaping.

/// 16.7 (D1), 16.3 and 15.5 (N2): a region opened from its state keeps its waiting
/// stays and travellers, names the stays again with their travel in its first tick,
/// and asks the index again; an answer to a question of before is passed over.
#[test]
fn a_region_restored_keeps_its_waiting_stays_and_travellers_and_asks_again() {
    let mut region = nether(|_| None);
    let (id, transfer) = traveller(1, at(6, 41, -3));
    let (key, cart) = cart_traveller(at(6, 41, -3));
    let number = EffectId {
        sequence: 2,
        index: 0,
    };
    let taken = tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer.clone())],
            travellers: vec![(number, cart)],
            ..TickInputs::default()
        },
    );
    assert_eq!(taken.portal_queries.len(), 2);
    let mut restored = reopened(&region);
    assert!(restored.waiting_state(id).is_some());
    assert_eq!(restored.state().waiting_travellers.len(), 1);
    let first = idle(&mut restored);
    assert_eq!(first.portal_queries.len(), 2, "not asked again");
    for query in &first.portal_queries {
        assert!(
            taken.portal_queries.iter().all(|old| old.id != query.id),
            "an old question's number again"
        );
    }
    assert!(
        has_notes(&first, id)
            .iter()
            .any(|(stay, _, place, let_go)| *stay == transfer.entity_id
                && place.travel == transfer.travel
                && !let_go),
        "{:?}",
        first.stays
    );
    // The answers to the questions of before the restore place nothing.
    let stale = answer(&mut restored, &taken, &[]);
    assert!(placed(&stale, id).is_none());
    assert!(restored.waiting_state(id).is_some());
    assert!(restored.find_entity(key).is_none());
    let output = answer(&mut restored, &first, &[]);
    assert!(placed(&output, id).is_some());
    assert!(restored.find_entity(key).is_some());
    assert_eq!(portal_blocks(&restored, 0, 127).len(), 6);
}

/// 16.7 (D1) and 16.8 (A2): a split gives the part the waiting stays and travellers
/// whose target chunk it takes, the stays with one hop more and named in the
/// `SplitOff`; the others stay.
#[test]
fn a_split_gives_the_part_the_waiting_stays_and_travellers_of_its_chunks() {
    let mut region = nether(|_| None);
    // Someone placed in chunk (1, 0), so that the chunk can be split off.
    let (placed_id, placed_entity) = add_player(&mut region, 9, [24.5, 41.0, 8.5], false);
    idle(&mut region);
    let (going, there) = traveller(1, at(24, 41, 4));
    let (staying, here) = traveller(2, at(-20, 41, -20));
    let (_, cart_there) = cart_traveller(at(25, 41, 5));
    let (_, cart_here) = cart_traveller(at(-21, 41, -21));
    let numbers = [
        EffectId {
            sequence: 3,
            index: 0,
        },
        EffectId {
            sequence: 3,
            index: 1,
        },
    ];
    tick_with(
        &mut region,
        TickInputs {
            player_changes: vec![
                PlayerChange::Arrive(EDGE, going, there.clone()),
                PlayerChange::Arrive(EDGE, staying, here.clone()),
            ],
            travellers: vec![(numbers[0], cart_there), (numbers[1], cart_here)],
            ..TickInputs::default()
        },
    );
    let splitting = region
        .split(&[ChunkPos::new(1, 0)], PART, &[])
        .expect("a split");
    assert!(splitting.chunks.contains(&ChunkPos::new(1, 0)));
    let part = &splitting.part;
    let kept = &splitting.state;
    let went = part.waiting.get(&going).expect("the stay went");
    assert_eq!(went.transfer.hops, there.hops + 1);
    assert_eq!(went.edge, EDGE);
    assert!(!kept.waiting.contains_key(&going));
    assert!(kept.waiting.contains_key(&staying));
    assert!(!part.waiting.contains_key(&staying));
    let travellers = |state: &RegionState| -> Vec<EffectId> {
        state
            .waiting_travellers
            .iter()
            .map(|waiting| waiting.id)
            .collect()
    };
    assert_eq!(travellers(part), [numbers[0]]);
    assert_eq!(travellers(kept), [numbers[1]]);
    assert!(part.edges.contains_key(&EDGE));
    let named: Vec<(PlayerId, EntityId, Option<u64>)> = kept.edges[&EDGE]
        .outbox
        .values()
        .filter_map(|entry| match entry {
            Durable::SplitOff { region, players } if *region == PART => Some(players.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        named.contains(&(going, there.entity_id, there.attempt)),
        "{named:?}"
    );
    assert!(
        named
            .iter()
            .any(|(player, entity, _)| *player == placed_id && *entity == placed_entity)
    );
    assert!(named.iter().all(|(player, ..)| *player != staying));
}

/// A Nether region with player 1 waiting with the stay `entity`.
fn waiting_with(entity: EntityId) -> Region {
    let mut region = nether(|_| None);
    let (id, mut transfer) = traveller(1, at(6, 41, -3));
    transfer.entity_id = entity;
    arrive(&mut region, id, transfer);
    region
}

fn player_one() -> PlayerId {
    PlayerId(Uuid::from_u128(1))
}

/// 16.8 (A2), ADR-0020 section 6 (a merge, by `(entity_id, hops)`): of two waiting
/// stays of a player the merge keeps the later, whichever side has it.
#[test]
fn a_merge_keeps_the_later_of_two_waiting_stays() {
    let id = player_one();
    let early = EntityId(500);
    let late = EntityId(600);
    for (ours, theirs) in [(early, late), (late, early)] {
        let survivor = waiting_with(ours);
        let absorbed = waiting_with(theirs);
        let merged = survivor.absorb(ABSORBED, &absorbed.state());
        let kept = merged.waiting.get(&id).expect("one waits");
        assert_eq!(kept.transfer.entity_id, late, "{ours:?} against {theirs:?}");
        assert!(!merged.players.contains_key(&id));
    }
}

/// 16.8 (A1, A2): a waiting stay is a stay to the merge's rule: against a present stay
/// of the player the later of the two is kept, and only one.
#[test]
fn a_merge_keeps_one_stay_of_a_player_waiting_in_one_region_and_present_in_the_other() {
    let id = player_one();
    // Present here with the entity 1 000 001; waiting there with a later one.
    let mut survivor = nether(|_| None);
    let (_, present) = add_player(&mut survivor, 1, [8.5, 41.0, 8.5], false);
    idle(&mut survivor);
    let absorbed = waiting_with(EntityId(present.0 + 10));
    let merged = survivor.absorb(ABSORBED, &absorbed.state());
    assert!(
        !merged.players.contains_key(&id),
        "the earlier stay kept too"
    );
    assert_eq!(
        merged
            .waiting
            .get(&id)
            .map(|waiting| waiting.transfer.entity_id),
        Some(EntityId(present.0 + 10))
    );
    // And the other way: present with the later stay.
    let mut survivor = nether(|_| None);
    add_player(&mut survivor, 1, [8.5, 41.0, 8.5], false);
    idle(&mut survivor);
    let absorbed = waiting_with(EntityId(present.0 - 10));
    let merged = survivor.absorb(ABSORBED, &absorbed.state());
    assert!(merged.players.contains_key(&id));
    assert!(
        !merged.waiting.contains_key(&id),
        "the earlier stay kept too"
    );
}

/// 16.8 (A2): a merge's edge resets apply to waiting stays: the side that knew a lower
/// start of an edge loses its waiting stays of that edge.
#[test]
fn a_merge_drops_the_waiting_stays_of_an_edge_the_other_side_knows_with_a_higher_start() {
    let id = player_one();
    let survivor = waiting_with(EntityId(500));
    let mut absorbed = nether(|_| None);
    start_edge(&mut absorbed, 2);
    let merged = survivor.absorb(ABSORBED, &absorbed.state());
    assert!(!merged.waiting.contains_key(&id));
}
