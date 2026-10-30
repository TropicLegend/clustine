//! Tests of the Nether portal's blocks and the portal clock from the game's rules
//! (`docs/groundwork/p-brief.md`, sections 2.1, 2.7, 4.1 and 4.2): a frame lit by
//! flint and steel, a fire charge or a dispenser, in the overworld and the Nether and
//! never in the End; a portal dropped with its frame; and the clocks of players and of
//! other entities. The overworld's worlds are those of the tests of TNT
//! (`tnt/tests.rs`): a stone floor at y = -61.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EntityData, EntityId, EntityIds, Float, ItemStack,
    PlayerId, PortalClock, PortalKind, PortalState, Vec3,
};
use uuid::Uuid;

use super::super::tnt::tests::{
    FLOOR, add, add_player, at, cart, idle, nothing, record, state, world,
};
use super::super::{Holdings, Region, RegionConfig};
use super::*;
use crate::api::{Face as ClickFace, Inventory, TickInputs, TickOutput, Ticket};
use crate::placement::Click;
use crate::state::RegionState;

/// The frame's plane, and the x of its inside's two columns.
const Z: i32 = 5;
const LEFT: i32 = 2;
const RIGHT: i32 = 3;

/// A frame of obsidian across x (its inside two wide and three high, at x 2 and 3 and
/// from one above `floor` up), without its corners, standing on the floor at `floor`.
fn frame_x(floor: i32) -> impl Fn(BlockPos) -> Option<BlockState> {
    move |pos| {
        let bottom = floor + 1;
        let top = bottom + 4;
        if pos.z != Z {
            return None;
        }
        let post = (pos.x == LEFT - 1 || pos.x == RIGHT + 1) && pos.y > bottom && pos.y < top;
        let beam = (pos.y == bottom || pos.y == top) && (LEFT..=RIGHT).contains(&pos.x);
        (post || beam).then_some(blocks::OBSIDIAN)
    }
}

/// The same frame along z, its inside at z 2 and 3 in the plane x = 5.
fn frame_z(floor: i32) -> impl Fn(BlockPos) -> Option<BlockState> {
    let along_x = frame_x(floor);
    move |pos| along_x(BlockPos::new(pos.z, pos.y, pos.x))
}

/// The six blocks inside the frame of [`frame_x`] on the floor at `floor`.
fn inside(floor: i32) -> Vec<BlockPos> {
    (floor + 2..=floor + 4)
        .flat_map(|y| [at(LEFT, y, Z), at(RIGHT, y, Z)])
        .collect()
}

fn portal(axis: &str) -> BlockState {
    state(&format!("minecraft:nether_portal[axis={axis}]"))
}

/// A click with `held` on the face `face` of `pos`, the player facing south (yaw 0).
fn click_on(pos: BlockPos, face: ClickFace, held: i32) -> Click {
    Click {
        position: pos,
        face,
        cursor: [0.5; 3],
        sneaking: false,
        held: Some(held),
        feet: Vec3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) - 2.5,
        ),
        yaw: 0.0,
        pitch: 0.0,
    }
}

/// Uses `held` on the face `face` of `pos`; whether it did anything.
fn use_on(region: &mut Region, pos: BlockPos, face: ClickFace, held: i32) -> bool {
    let player = PlayerId(Uuid::from_u128(77));
    let click = click_on(pos, face, held);
    region.entity_item_used_on(&click, player, &mut TickOutput::default())
}

/// A region of `dimension` holding the 5×5 chunks about the origin's, loaded, with a
/// stone floor at `floor` and what `blocks` names, ticked once: [`world`]'s, in any
/// dimension.
fn world_in(
    dimension: Dimension,
    floor: i32,
    blocks: impl Fn(BlockPos) -> Option<BlockState>,
) -> Region {
    let rules = dimension.rules();
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
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
        dimension,
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
    region
}

/// The overworld's world of the tests of TNT with a frame across x lit by flint and
/// steel on its bottom beam.
fn lit_portal() -> Region {
    let mut region = world(1, frame_x(FLOOR));
    assert!(use_on(
        &mut region,
        at(LEFT, FLOOR + 1, Z),
        ClickFace::Top,
        items::FLINT_AND_STEEL
    ));
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")), "{pos:?}");
    }
    region
}

fn entity_portal(region: &Region, key: EntityId) -> PortalState {
    region.find_entity(key).expect("the entity").1.portal
}

fn player_portal(region: &Region, id: PlayerId) -> PortalState {
    region.players[&id].portal
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

fn clock(time: u32, inside: bool) -> PortalClock {
    PortalClock {
        kind: PortalKind::Nether,
        entry: at(LEFT, FLOOR + 2, Z),
        time,
        inside,
    }
}

// Lighting (`BaseFireBlock.onPlace`, `canBePlacedAt`, `isPortal`).

#[test]
fn a_frame_lit_by_flint_and_steel_in_the_overworld_becomes_a_portal_along_its_axis_and_size() {
    let region = lit_portal();
    // The frame and what is about it are as they were; no fire is left.
    assert_eq!(
        region.block(at(LEFT - 1, FLOOR + 3, Z)),
        Some(blocks::OBSIDIAN)
    );
    assert_eq!(region.block(at(LEFT, FLOOR + 5, Z)), Some(blocks::OBSIDIAN));
    assert_eq!(region.block(at(LEFT, FLOOR + 2, Z - 1)), Some(blocks::AIR));
    assert_eq!(region.block(at(LEFT - 1, FLOOR + 5, Z)), Some(blocks::AIR));

    let mut region = world(2, frame_z(FLOOR));
    assert!(use_on(
        &mut region,
        at(Z, FLOOR + 1, LEFT),
        ClickFace::Top,
        items::FLINT_AND_STEEL
    ));
    for pos in inside(FLOOR) {
        let turned = at(pos.z, pos.y, pos.x);
        assert_eq!(region.block(turned), Some(portal("z")), "{turned:?}");
    }
}

#[test]
fn flint_and_steel_lights_a_frame_from_where_fire_could_not_stay() {
    // The middle of the inside, beside the left post: no block below to hold fire.
    let mut region = world(3, frame_x(FLOOR));
    assert!(use_on(
        &mut region,
        at(LEFT - 1, FLOOR + 3, Z),
        ClickFace::East,
        items::FLINT_AND_STEEL
    ));
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
    // Beside obsidian with no frame about it, fire that cannot stay is not set.
    let mut region = world(3, |pos| {
        (pos == at(8, FLOOR + 3, 8)).then_some(blocks::OBSIDIAN)
    });
    assert!(!use_on(
        &mut region,
        at(8, FLOOR + 3, 8),
        ClickFace::East,
        items::FLINT_AND_STEEL
    ));
    assert_eq!(region.block(at(9, FLOOR + 3, 8)), Some(blocks::AIR));
}

#[test]
fn a_fire_charge_lights_a_frame_as_flint_and_steel_does() {
    let mut region = world(4, frame_x(FLOOR));
    assert!(use_on(
        &mut region,
        at(RIGHT + 1, FLOOR + 4, Z),
        ClickFace::West,
        items::FIRE_CHARGE
    ));
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
}

#[test]
fn a_frame_in_the_nether_is_lit() {
    let floor = 60;
    let mut region = world_in(Dimension::Nether, floor, frame_x(floor));
    assert!(use_on(
        &mut region,
        at(LEFT, floor + 1, Z),
        ClickFace::Top,
        items::FLINT_AND_STEEL
    ));
    for pos in inside(floor) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
}

#[test]
fn a_frame_in_the_end_is_never_lit_and_fire_stays_only_where_it_can() {
    let floor = 60;
    let mut region = world_in(Dimension::End, floor, frame_x(floor));
    // On the bottom beam the fire stays, as fire.
    assert!(use_on(
        &mut region,
        at(LEFT, floor + 1, Z),
        ClickFace::Top,
        items::FLINT_AND_STEEL
    ));
    let fire = region.block(at(LEFT, floor + 2, Z)).expect("loaded");
    assert!(clustine_blocks::data::is_block(fire, blocks::FIRE));
    // In the middle, where it could not stay, it is not set at all.
    assert!(!use_on(
        &mut region,
        at(LEFT - 1, floor + 3, Z),
        ClickFace::East,
        items::FLINT_AND_STEEL
    ));
    for pos in inside(floor).into_iter().skip(1) {
        assert_eq!(region.block(pos), Some(blocks::AIR), "{pos:?}");
    }
}

#[test]
fn fire_set_by_any_rule_in_an_empty_frame_lights_it_and_fire_over_fire_does_not() {
    let mut region = world(5, frame_x(FLOOR));
    let mut output = TickOutput::default();
    region.set_block(at(RIGHT, FLOOR + 4, Z), blocks::SOUL_FIRE, &mut output);
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
    // Fire set anew where fire is (an age) asks nothing of a frame.
    let mut region = world(5, |pos| {
        frame_x(FLOOR)(pos).or((pos == at(LEFT, FLOOR + 2, Z)).then_some(blocks::FIRE))
    });
    let aged = clustine_blocks::data::set(blocks::FIRE, "age", "3");
    region.set_block(at(LEFT, FLOOR + 2, Z), aged, &mut output);
    assert_eq!(region.block(at(LEFT, FLOOR + 2, Z)), Some(aged));
    assert_eq!(region.block(at(RIGHT, FLOOR + 3, Z)), Some(blocks::AIR));
}

#[test]
fn a_dispenser_facing_into_an_empty_frame_lights_it() {
    let dispenser = at(LEFT, FLOOR + 3, Z - 1);
    let mut region = world(6, move |pos| {
        if pos == dispenser {
            return Some(state("minecraft:dispenser[facing=south,triggered=false]"));
        }
        frame_x(FLOOR)(pos)
    });
    let mut output = TickOutput::default();
    let mut container = region.container_at(dispenser).expect("a container");
    container.slots[0] = Some(ItemStack::new(items::FLINT_AND_STEEL, 1));
    region.set_container(dispenser, container, &mut output);
    let at_dispenser = region.block(dispenser).expect("loaded");
    assert!(region.dispenser_tick(dispenser, at_dispenser, &mut output));
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
}

// Breaking (`NetherPortalBlock.updateShape`).

#[test]
fn a_frame_block_mined_drops_the_whole_portal() {
    let mut region = lit_portal();
    let mut output = TickOutput::default();
    region.set_block(at(LEFT - 1, FLOOR + 3, Z), blocks::AIR, &mut output);
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(blocks::AIR), "{pos:?}");
    }
}

#[test]
fn a_portal_block_removed_drops_the_rest_and_a_block_in_front_drops_nothing() {
    let mut region = lit_portal();
    let mut output = TickOutput::default();
    // In front of it and behind it: nothing is judged.
    region.set_block(at(LEFT, FLOOR + 3, Z - 1), blocks::DIRT, &mut output);
    region.set_block(at(RIGHT, FLOOR + 3, Z + 1), blocks::DIRT, &mut output);
    region.set_block(at(RIGHT, FLOOR + 3, Z + 1), blocks::AIR, &mut output);
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(portal("x")));
    }
    region.set_block(at(RIGHT, FLOOR + 4, Z), blocks::AIR, &mut output);
    for pos in inside(FLOOR) {
        assert_eq!(region.block(pos), Some(blocks::AIR), "{pos:?}");
    }
}

// The clock (`setAsInsidePortal`, `handlePortal`, `PortalProcessor`).

#[test]
fn a_creative_player_standing_in_a_portal_fires_on_their_first_tick_inside_it() {
    let mut region = lit_portal();
    let feet = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let (id, _) = add_player(&mut region, 1, feet, false);
    idle(&mut region);
    // Marked inside where they stand, nothing counted yet.
    assert_eq!(
        player_portal(&region, id),
        PortalState {
            clock: Some(PortalClock {
                entry: at(LEFT, FLOOR + 2, Z),
                ..clock(0, true)
            }),
            cooldown: 0,
        }
    );
    idle(&mut region);
    // It fired: the cooldown is a player's 10, kept full while they stand in it.
    let fired = player_portal(&region, id);
    assert_eq!(fired.cooldown, PLAYER_DIMENSION_CHANGING_DELAY);
    assert_eq!(fired.clock.map(|clock| clock.time), Some(1));
}

#[test]
fn a_survival_player_fires_after_80_ticks_inside_and_an_invulnerable_one_at_once() {
    let mut portal = PortalState::default();
    let entry = at(0, 0, 0);
    let mut ticks = 0;
    let fired = loop {
        set_as_inside_portal(
            &mut portal,
            PortalKind::Nether,
            entry,
            PLAYER_DIMENSION_CHANGING_DELAY,
        );
        ticks += 1;
        let fired = handle_portal(
            &mut portal,
            || true,
            |kind| transition_time(kind, Some(false)),
            PLAYER_DIMENSION_CHANGING_DELAY,
        );
        if let Some(clock) = fired {
            break clock;
        }
        assert!(ticks < 200, "the clock never fired");
    };
    // The time before counting is 80 at the 81st handle: 80 ticks spent inside.
    assert_eq!(ticks, 81);
    assert_eq!(fired.time, 81);
    assert_eq!(portal.cooldown, PLAYER_DIMENSION_CHANGING_DELAY);
    assert_eq!(transition_time(PortalKind::Nether, Some(true)), 0);
    assert_eq!(transition_time(PortalKind::Nether, None), 0);
    assert_eq!(transition_time(PortalKind::End, Some(false)), 0);
}

#[test]
fn standing_in_the_arrival_portal_keeps_the_cooldown_full_and_leaving_it_lets_it_run_down() {
    let mut region = lit_portal();
    let feet = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let (id, _) = add_player(&mut region, 2, feet, false);
    region.players.get_mut(&id).expect("added").portal.cooldown = PLAYER_DIMENSION_CHANGING_DELAY;
    for _ in 0..30 {
        idle(&mut region);
        assert_eq!(
            player_portal(&region, id),
            PortalState {
                clock: None,
                cooldown: PLAYER_DIMENSION_CHANGING_DELAY,
            }
        );
    }
    region.players.get_mut(&id).expect("added").pose.position = Vec3::new(8.5, -60.0, 8.5);
    idle(&mut region);
    assert_eq!(player_portal(&region, id).cooldown, 9);
    for _ in 0..9 {
        idle(&mut region);
    }
    assert!(player_portal(&region, id).is_idle());
}

#[test]
fn an_entity_not_inside_its_portal_counts_down_by_four_and_its_clock_goes_at_nothing() {
    let mut region = world(7, nothing);
    let mut minecart = record(
        &mut region,
        entity_types::MINECART,
        [8.5, f64::from(FLOOR + 1), 8.5],
        EntityData::Minecart {
            hurt: Default::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [clustine_world::Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    );
    minecart.portal = PortalState {
        clock: Some(clock(10, false)),
        cooldown: 3,
    };
    let key = add(&mut region, minecart);
    let output = idle(&mut region);
    assert_eq!(
        entity_portal(&region, key),
        PortalState {
            clock: Some(clock(6, false)),
            cooldown: 2,
        }
    );
    // Committed, so that a restore finds the clock as the tick left it.
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        clustine_world::EntityChange::Set(changed, fields)
            if *changed == key && fields.contains(&clustine_world::EntityField::Portal(
                entity_portal(&region, key)
            ))
    )));
    idle(&mut region);
    assert_eq!(entity_portal(&region, key).clock, Some(clock(2, false)));
    idle(&mut region);
    assert_eq!(entity_portal(&region, key), PortalState::default());
}

#[test]
fn a_cart_in_a_portal_fires_on_its_next_tick_and_is_kept_on_cooldown_while_in_it() {
    let mut region = lit_portal();
    let key = cart(
        &mut region,
        entity_types::MINECART,
        [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5],
    );
    idle(&mut region);
    let marked = entity_portal(&region, key);
    assert_eq!(marked.cooldown, 0);
    assert_eq!(
        marked.clock.map(|clock| (clock.time, clock.inside)),
        Some((0, true))
    );
    idle(&mut region);
    let fired = entity_portal(&region, key);
    assert_eq!(fired.cooldown, DIMENSION_CHANGING_DELAY);
    assert_eq!(fired.clock.map(|clock| clock.time), Some(1));
    idle(&mut region);
    // Unmarked as it fired, the clock runs out; the cooldown is set full again.
    assert_eq!(
        entity_portal(&region, key),
        PortalState {
            clock: None,
            cooldown: DIMENSION_CHANGING_DELAY,
        }
    );
}

#[test]
fn a_passengers_clock_never_runs_and_its_vehicle_counts_for_both() {
    let mut region = lit_portal();
    let position = [f64::from(RIGHT), f64::from(FLOOR + 2), f64::from(Z) + 0.5];
    let vehicle = cart(&mut region, entity_types::MINECART, position);
    let mut rider = stand(&mut region, position);
    // Marked inside before it got on: the mark is spent, and the clock counts nothing.
    rider.portal = PortalState {
        clock: Some(clock(5, true)),
        cooldown: 0,
    };
    let rider = add(&mut region, rider);
    assert!(region.start_riding(rider, vehicle, true, &mut TickOutput::default()));
    idle(&mut region);
    assert_eq!(entity_portal(&region, rider).clock, Some(clock(5, false)));
    idle(&mut region);
    assert_eq!(entity_portal(&region, rider).clock, Some(clock(1, false)));
    for _ in 0..5 {
        idle(&mut region);
        assert!(entity_portal(&region, rider).is_idle());
    }
    // The vehicle stood in the portal all along and fired.
    assert_eq!(
        entity_portal(&region, vehicle).cooldown,
        DIMENSION_CHANGING_DELAY
    );
}

#[test]
fn who_may_use_a_portal_follows_the_game() {
    let mut region = world(8, nothing);
    let mut armour = stand(&mut region, [8.5, -60.0, 8.5]);
    assert!(entity_can_use_portal(&armour));
    armour.riding.vehicle = Some(EntityId(1));
    assert!(!entity_can_use_portal(&armour));
    let mut dragon = stand(&mut region, [8.5, -60.0, 8.5]);
    dragon.kind = u16::try_from(entity_types::ENDER_DRAGON).expect("an id");
    assert!(!entity_can_use_portal(&dragon));
    let mut snowball = record(
        &mut region,
        entity_types::SNOWBALL,
        [8.5, -60.0, 8.5],
        EntityData::Thrown {
            stack: ItemStack::new(items::SNOWBALL, 1),
            owner: None,
            left_owner: false,
            has_been_shot: false,
        },
    );
    // A thrown projectile may even when it rides.
    snowball.riding.vehicle = Some(EntityId(1));
    assert!(entity_can_use_portal(&snowball));
    let mut carried = armour.clone();
    carried.riding.vehicle = None;
    carried.riding.passengers.push(clustine_world::Seat {
        key: EntityId(9),
        player: true,
        pending: None,
    });
    assert_eq!(
        dimension_changing_delay(&carried),
        PLAYER_DIMENSION_CHANGING_DELAY
    );
    carried.riding.passengers[0].player = false;
    assert_eq!(dimension_changing_delay(&carried), DIMENSION_CHANGING_DELAY);
}

#[test]
fn a_clock_of_another_kind_is_begun_anew_and_one_marked_keeps_its_first_entry() {
    let mut portal = PortalState {
        clock: Some(PortalClock {
            kind: PortalKind::End,
            entry: at(1, 1, 1),
            time: 7,
            inside: false,
        }),
        cooldown: 0,
    };
    set_as_inside_portal(&mut portal, PortalKind::Nether, at(2, 2, 2), 300);
    assert_eq!(
        portal.clock,
        Some(PortalClock {
            kind: PortalKind::Nether,
            entry: at(2, 2, 2),
            time: 0,
            inside: true,
        })
    );
    set_as_inside_portal(&mut portal, PortalKind::Nether, at(3, 3, 3), 300);
    assert_eq!(portal.clock.map(|clock| clock.entry), Some(at(2, 2, 2)));
}
