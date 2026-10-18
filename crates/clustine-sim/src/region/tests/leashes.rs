//! Tests of leashes (step N5g, `docs/groundwork/n5-plan.md` with its section 7 item 3;
//! `docs/groundwork/n5-brief.md`, sections 0.36 and 7). The numbers each test expects
//! are worked out here from the game's formulas as its bytecode has them
//! (`Leashable.tickLeash`, `checkElasticInteractions`, `computeElasticInteraction`,
//! `computeDampenedSpringInteraction`, `angularFriction`), written afresh rather than
//! taken from what the region computes.
//!
//! The regions are those of the tests of redstone (`redstone.rs`): player 1, whose stay
//! is the entity 1, at the spawn, (0.5, -60, 0.5), on a stone floor whose top is
//! y = -60, with the chunks about chunk (0, 0) ticking.

use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_noise::trig;
use clustine_world::{Double, Entity, EntityChange, EntityData, Float, Hurt, Leash, LeashHolder};

use super::arrivals::{WEST_OF_THE_LINE, at_a_line};
use super::redstone::{click, holding, sounds as sounds_of, state, ticking_with};
use super::*;
use crate::region::leashes::{End, SNAP_DISTANCE, elastic_pull, leash_distance};

/// Player 1's stay.
const HOLDER: EntityId = EntityId(1);

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

fn boat(n: u64, position: [f64; 3]) -> Entity {
    Entity {
        key: key(n),
        kind: entity_types::OAK_BOAT as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        age: 1,
        data: EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            status: [3; 2],
            contents: Vec::new(),
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// `entity` with a leash held by `holder` and no swing.
fn leashed_to(entity: Entity, holder: LeashHolder) -> Entity {
    let mut entity = entity;
    entity.riding.leash = Some(Box::new(Leash {
        holder,
        angular_momentum: Double(0.0),
    }));
    entity
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// Lays `entity`, whose leash has found its holder already, as one tied in this
/// region's life.
fn lay_found(region: &mut Region, entity: &Entity) {
    lay(region, entity);
    region.note_leash_found(entity.key);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn leash(region: &Region, key: EntityId) -> Option<Leash> {
    record(region, key).and_then(|entity| entity.riding.leash.as_deref().copied())
}

fn interact(target: EntityId, sneaking: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: target,
            location: [0.0, 0.3, 0.0],
            sneaking,
        },
    )])
}

fn attack(target: EntityId) -> TickInputs {
    moves(vec![numbered(1, PlayerInput::Attack { entity: target })])
}

fn walk_to(x: f64, z: f64) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(x, -60.0, z)),
            rotation: None,
            on_ground: true,
        },
    )])
}

fn links(output: &TickOutput) -> Vec<(EntityId, Option<EntityId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Leashed { entity, holder, .. } => Some((*entity, *holder)),
            _ => None,
        })
        .collect()
}

fn of_kind(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn leads(region: &Region) -> usize {
    of_kind(region, entity_types::ITEM)
        .iter()
        .filter(|entity| {
            matches!(entity.data, EntityData::Item { stack, .. } if stack.item == items::LEAD)
        })
        .count()
}

fn spawned_kinds(output: &TickOutput, kind: i32) -> Vec<EntityId> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) => match state.kind {
                EntityKind::Other { kind: of, .. } if i32::from(of) == kind => Some(state.entity),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn gone(output: &TickOutput) -> Vec<EntityId> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityGone { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect()
}

fn fence() -> BlockState {
    state("minecraft:oak_fence")
}

/// `Vec3.yRot(angle)` through the game's table of sines, written afresh.
fn turned([x, y, z]: [f64; 3], angle: f32) -> [f64; 3] {
    let c = f64::from(trig::cos(f64::from(angle)));
    let s = f64::from(trig::sin(f64::from(angle)));
    [x * c + z * s, y, z * c - x * s]
}

/// `computeDampenedSpringInteraction` and `checkElasticInteractions` for one pair of
/// points, as the bytecode reads: the force and the momentum's change, and the motion's.
fn expected_pull(leashee: &End, holder: &End) -> Option<([f64; 3], f64)> {
    let (leashee_yaw, leashee_size, leashee_movement) = (
        leashee.yaw,
        (leashee.width, leashee.height),
        leashee.movement,
    );
    let (holder_yaw, holder_size, holder_movement) =
        (holder.yaw, (holder.width, holder.height), holder.movement);
    let (leashee, holder) = (leashee.position, holder.position);
    let degrees = 0.017_453_292_f32;
    let (we, he) = (f64::from(leashee_size.0), f64::from(leashee_size.1));
    let (wh, hh) = (f64::from(holder_size.0), f64::from(holder_size.1));
    let oe = turned([0.0 * we, 0.5 * he, 0.5 * we], -(leashee_yaw * degrees));
    let pe = [leashee[0] + oe[0], leashee[1] + oe[1], leashee[2] + oe[2]];
    let oh = turned([0.0 * wh, 0.5 * hh, 0.0 * wh], -(holder_yaw * degrees));
    let ph = [holder[0] + oh[0], holder[1] + oh[1], holder[2] + oh[2]];
    let d = [ph[0] - pe[0], ph[1] - pe[1], ph[2] - pe[2]];
    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if length < 6.0 {
        return None;
    }
    let mut f = [
        d[0] / length * (length - 6.0),
        d[1] / length * (length - 6.0),
        d[2] / length * (length - 6.0),
    ];
    let torque = oe[2] * f[0] - oe[0] * f[2];
    let along =
        leashee_movement[0] * f[0] + leashee_movement[1] * f[1] + leashee_movement[2] * f[2];
    if along >= 0.0 {
        let k = f64::from(0.3_f32);
        f = [f[0] * k, f[1] * k, f[2] * k];
    }
    let f = [(0.0 + f[0]) * 1.0, (0.0 + f[1]) * 1.0, (0.0 + f[2]) * 1.0];
    let torque = (0.0 + torque) * 1.0;
    let relative = [
        holder_movement[0] - leashee_movement[0],
        holder_movement[1] - leashee_movement[1],
        holder_movement[2] - leashee_movement[2],
    ];
    let pull = [
        f[0] * 0.8 + relative[0] * 0.11,
        f[1] * 0.2 + relative[1] * 0.11,
        f[2] * 0.8 + relative[2] * 0.11,
    ];
    Some((pull, 10.0 * torque))
}

fn boat_size() -> (f32, f32) {
    let info = EntityTypeInfo::of(entity_types::OAK_BOAT);
    (info.width, info.height)
}

// The pull.

/// The spring between the points of attachment, for a boat turned and moving and a
/// holder moving too: the motion gains the force by axis and a ninth of the difference
/// of the movements, and the momentum ten times the torque, bit for bit.
#[test]
fn the_elastic_pull_is_the_games_spring_bit_for_bit() {
    let (width, height) = boat_size();
    for (yaw, movement, holder_movement) in [
        (0.0_f32, [0.0; 3], [0.0; 3]),
        (37.5, [0.05, -0.01, 0.2], [-0.1, 0.0, 0.03]),
        (-160.0, [-0.3, 0.0, -0.1], [0.2, 0.1, 0.0]),
    ] {
        let leashee = End {
            position: [8.25, -60.0, 1.5],
            yaw,
            width,
            height,
            movement,
        };
        let holder = End {
            position: [0.5, -60.0, 0.5],
            yaw: 45.0,
            width: 0.6,
            height: 1.8,
            movement: holder_movement,
        };
        let mut motion = [0.01, 0.0, -0.02];
        let mut momentum = 0.25;
        assert!(elastic_pull(&leashee, &holder, &mut motion, &mut momentum));
        let (pull, turn) = expected_pull(&leashee, &holder).expect("beyond six");
        let wanted = [0.01 + pull[0], 0.0 + pull[1], -0.02 + pull[2]];
        assert_eq!(motion.map(f64::to_bits), wanted.map(f64::to_bits), "{yaw}");
        assert_eq!(momentum.to_bits(), (0.25 + turn).to_bits(), "{yaw}");
    }
}

/// A lead used on a boat ties it to the player, who keeps the lead, and every client
/// is told; in the same tick the boat, seven blocks off on the stone floor, is pulled,
/// and turns by its swing, which the stone's friction then slows: its yaw and kept
/// swing are the game's, bit for bit.
#[test]
fn a_lead_ties_a_boat_to_the_player_and_its_pull_turns_it_by_the_games_numbers() {
    let mut region = ticking_with(&[]);
    let target = boat(1, [7.5, -60.0, 0.5]);
    lay(&mut region, &target);
    checked(&mut region, &holding(items::LEAD));
    let output = checked(&mut region, &interact(target.key, false));
    assert_eq!(links(&output), [(target.key, Some(HOLDER))]);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ITEM_LEAD_TIED)
    );
    let held = region.players[&player(1)].held();
    assert_eq!(
        held.map(|stack| (stack.item, stack.count)),
        Some((items::LEAD, 1))
    );
    // Nobody rides it, so the player did not get on.
    assert!(region.players[&player(1)].riding.is_none());

    let (width, height) = boat_size();
    let leashee = End {
        position: target.position,
        yaw: 0.0,
        width,
        height,
        movement: [0.0; 3],
    };
    let player_end = End {
        position: [0.5, -60.0, 0.5],
        yaw: 0.0,
        width: 0.6,
        height: 1.8,
        movement: [0.0; 3],
    };
    let (pull, turn) = expected_pull(&leashee, &player_end).expect("beyond six");
    assert!(pull[0] < 0.0, "towards the player");
    let momentum = 0.0 + turn;
    let yaw = (0.0_f64 - momentum) as f32;
    let friction = f64::from(0.6_f32 * 0.91_f32);
    let after = record(&region, target.key).unwrap();
    assert_eq!(after.yaw.to_bits(), yaw.to_bits());
    let kept = leash(&region, target.key).expect("still tied");
    assert_eq!(kept.holder, LeashHolder::Entity(HOLDER));
    assert_eq!(
        kept.angular_momentum.0.to_bits(),
        (momentum * friction).to_bits()
    );
    assert!(
        after.position[0] < target.position[0],
        "{:?}",
        after.position
    );
}

/// The holder's use of their own boat lets it go, keeping the lead (every player is
/// creative), and clients are told nobody holds it.
#[test]
fn the_holders_use_of_their_boat_lets_it_go_without_a_lead() {
    let mut region = ticking_with(&[]);
    let target = leashed_to(boat(1, [3.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &target);
    let output = checked(&mut region, &interact(target.key, false));
    assert_eq!(links(&output), [(target.key, None)]);
    assert!(leash(&region, target.key).is_none());
    assert_eq!(leads(&region), 0);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ITEM_LEAD_UNTIED)
    );
}

/// A leashed boat broken drops its lead as it goes, before anything else, even to a
/// creative player's hit, and clients are told.
#[test]
fn a_leashed_boat_broken_drops_its_lead() {
    let mut region = ticking_with(&[]);
    let target = leashed_to(boat(1, [3.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &target);
    let output = checked(&mut region, &attack(target.key));
    assert!(record(&region, target.key).is_none());
    assert_eq!(links(&output), [(target.key, None)]);
    assert_eq!(leads(&region), 1);
}

/// Beyond twelve between the boxes' centres the leash breaks, with its sound at the
/// holder and a lead dropped at the boat's feet, and clients are told.
#[test]
fn a_leash_snaps_beyond_twelve_dropping_a_lead() {
    let mut region = ticking_with(&[]);
    let target = leashed_to(boat(1, [3.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &target);
    // 1.5 apart along x, and 0.61875 along y: no pull, the boat stays.
    let output = checked(&mut region, &walk_to(2.0, 0.5));
    assert!(leash(&region, target.key).is_some());
    assert!(links(&output).is_empty());
    assert_eq!(
        record(&region, target.key).unwrap().position,
        target.position
    );
    // 13 apart.
    let output = checked(&mut region, &walk_to(-9.5, 0.5));
    assert!(leash(&region, target.key).is_none());
    assert_eq!(links(&output), [(target.key, None)]);
    assert_eq!(leads(&region), 1);
    let breaks: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound, position, ..
            } if *sound == sounds::ITEM_LEAD_BREAK => Some(*position),
            _ => None,
        })
        .collect();
    assert_eq!(breaks, [Vec3::new(-9.5, -60.0, 0.5)]);
}

/// The distance the snap is judged by is between the boxes' centres.
#[test]
fn the_snap_distance_is_between_the_boxes_centres() {
    let (width, height) = boat_size();
    let leashee = End {
        position: [12.0, -60.0, 0.5],
        yaw: 0.0,
        width,
        height,
        movement: [0.0; 3],
    };
    let holder = End {
        position: [0.5, -60.0, 0.5],
        yaw: 0.0,
        width: 0.6,
        height: 1.8,
        movement: [0.0; 3],
    };
    let dy = (-60.0 + f64::from(1.8_f32) * 0.5) - (-60.0 + f64::from(height) * 0.5);
    let wanted = (11.5_f64 * 11.5 + dy * dy).sqrt();
    assert_eq!(
        leash_distance(&leashee, &holder).to_bits(),
        wanted.to_bits()
    );
    assert!(wanted < SNAP_DISTANCE);
}

/// Shears used on a boat that holds another cut that leash, a lead dropped, with the
/// snip, and nobody gets on.
#[test]
fn shears_cut_a_boats_leash_and_drop_the_lead() {
    let mut region = ticking_with(&[]);
    // Held by another boat beside it.
    let holder = boat(2, [3.5, -60.0, 3.5]);
    lay(&mut region, &holder);
    let target = leashed_to(boat(1, [3.5, -60.0, 0.5]), LeashHolder::Entity(holder.key));
    lay_found(&mut region, &target);
    checked(&mut region, &holding(items::SHEARS));
    let output = checked(&mut region, &interact(holder.key, false));
    assert!(leash(&region, target.key).is_none());
    assert_eq!(links(&output), [(target.key, None)]);
    assert_eq!(leads(&region), 1);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ITEM_SHEARS_SNIP)
    );
    // Nobody got on the boat the shears were used on.
    assert!(region.players[&player(1)].riding.is_none());
}

/// A sneaking player's use of a boat ties the boats they hold to it.
#[test]
fn a_sneaking_players_boats_are_tied_to_the_boat_they_use() {
    let mut region = ticking_with(&[]);
    let held = leashed_to(boat(1, [3.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &held);
    let other = boat(2, [3.5, -60.0, 3.5]);
    lay(&mut region, &other);
    let output = checked(&mut region, &interact(other.key, true));
    assert_eq!(links(&output), [(held.key, Some(other.key))]);
    assert_eq!(
        leash(&region, held.key).map(|leash| leash.holder),
        Some(LeashHolder::Entity(other.key))
    );
}

// Knots.

/// A lead used on a fence ties the player's boat to a knot made there, at the fence's
/// middle, 0.375 above its bottom; a second binding finds that knot; a hit breaks it,
/// with its sound and nothing dropped of its own, and each boat it held drops a lead on
/// its next tick.
#[test]
fn a_fence_binds_to_a_knot_made_then_found_and_a_broken_knot_drops_its_leashees_leads() {
    let post = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(post, fence())]);
    let first = leashed_to(boat(1, [4.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &first);
    checked(&mut region, &holding(items::LEAD));
    let output = checked(&mut region, &click(post));
    let knots = of_kind(&region, entity_types::LEASH_KNOT);
    let [knot] = &knots[..] else {
        panic!("one knot: {knots:?}");
    };
    assert_eq!(knot.position, [2.5, -59.625, 2.5]);
    assert_eq!(spawned_kinds(&output, entity_types::LEASH_KNOT), [knot.key]);
    assert_eq!(links(&output), [(first.key, Some(knot.key))]);
    assert_eq!(
        leash(&region, first.key).map(|leash| leash.holder),
        Some(LeashHolder::Fence(post))
    );
    // The fence block is not placed against: the click is taken.
    assert_eq!(
        region.block(BlockPos::new(2, -59, 2)),
        Some(clustine_data::blocks::AIR)
    );

    let second = leashed_to(boat(2, [0.5, -60.0, 4.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &second);
    let output = checked(&mut region, &click(post));
    assert!(spawned_kinds(&output, entity_types::LEASH_KNOT).is_empty());
    assert_eq!(links(&output), [(second.key, Some(knot.key))]);
    assert_eq!(of_kind(&region, entity_types::LEASH_KNOT).len(), 1);

    let output = checked(&mut region, &attack(knot.key));
    assert!(of_kind(&region, entity_types::LEASH_KNOT).is_empty());
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ITEM_LEAD_UNTIED)
    );
    assert!(leash(&region, first.key).is_none());
    assert!(leash(&region, second.key).is_none());
    assert_eq!(leads(&region), 2);
    let mut cut = links(&output);
    cut.sort();
    assert_eq!(cut, [(first.key, None), (second.key, None)]);
}

/// A fence used with an empty hand binds as well (`FenceBlock.useWithoutItem`), and the
/// knot holds while its block is a fence.
#[test]
fn a_fence_used_with_an_empty_hand_binds_too() {
    let post = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(post, fence())]);
    let held = leashed_to(boat(1, [4.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &held);
    checked(
        &mut region,
        &moves(vec![set_slot(1, 0, None), select(1, 0)]),
    );
    checked(&mut region, &click(post));
    assert_eq!(
        leash(&region, held.key).map(|leash| leash.holder),
        Some(LeashHolder::Fence(post))
    );
    for _ in 0..120 {
        checked(&mut region, &TickInputs::default());
    }
    assert_eq!(of_kind(&region, entity_types::LEASH_KNOT).len(), 1);
    assert!(leash(&region, held.key).is_some());
}

/// Where the player's boat is too far from the fence to be tied, a lead in the main
/// hand makes a knot twice, by the fence's use and by the lead's, each sent and gone
/// again, and the boat stays with the player.
#[test]
fn a_binding_that_ties_nothing_makes_and_discards_two_knots() {
    let post = BlockPos::new(2, -60, 0);
    let mut region = ticking_with(&[(post, fence())]);
    // 11.5 from the player, 13.5 from the knot, within sixteen of the fence.
    let far = leashed_to(boat(1, [-11.0, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &far);
    checked(&mut region, &holding(items::LEAD));
    let output = checked(&mut region, &click(post));
    let made = spawned_kinds(&output, entity_types::LEASH_KNOT);
    assert_eq!(made.len(), 2, "{made:?}");
    let went = gone(&output);
    assert!(made.iter().all(|knot| went.contains(knot)), "{went:?}");
    assert!(of_kind(&region, entity_types::LEASH_KNOT).is_empty());
    assert_eq!(
        leash(&region, far.key).map(|leash| leash.holder),
        Some(LeashHolder::Entity(HOLDER))
    );
}

/// A player's use of a knot takes the boats it holds back; a sneaking one's leaves
/// them, and one who holds boats ties them to it.
#[test]
fn a_knot_used_gives_its_boats_to_the_player_unless_they_sneak() {
    let post = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(post, fence())]);
    let held = leashed_to(boat(1, [4.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &held);
    checked(&mut region, &holding(items::LEAD));
    checked(&mut region, &click(post));
    let knot = of_kind(&region, entity_types::LEASH_KNOT)[0].clone();
    checked(&mut region, &interact(knot.key, true));
    assert_eq!(
        leash(&region, held.key).map(|leash| leash.holder),
        Some(LeashHolder::Fence(post))
    );
    let output = checked(&mut region, &interact(knot.key, false));
    assert_eq!(
        leash(&region, held.key).map(|leash| leash.holder),
        Some(LeashHolder::Entity(HOLDER))
    );
    // Its last leashee gone, the knot discards itself, silently.
    assert_eq!(gone(&output), [knot.key]);
    assert!(of_kind(&region, entity_types::LEASH_KNOT).is_empty());
    assert_eq!(leads(&region), 0);
}

// Restoring a saved leash.

/// A boat taken in with its leash held by a fence where no knot is makes the knot
/// there in its first tick, and clients are told.
#[test]
fn a_saved_leash_by_a_fence_makes_its_knot_again() {
    let post = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(post, fence())]);
    let saved = leashed_to(boat(1, [4.5, -60.0, 0.5]), LeashHolder::Fence(post));
    lay(&mut region, &saved);
    let output = checked(&mut region, &TickInputs::default());
    let knots = of_kind(&region, entity_types::LEASH_KNOT);
    let [knot] = &knots[..] else {
        panic!("one knot: {knots:?}");
    };
    assert_eq!(links(&output), [(saved.key, Some(knot.key))]);
    assert!(leash(&region, saved.key).is_some());
    let output = checked(&mut region, &TickInputs::default());
    assert!(links(&output).is_empty());
}

/// A saved leash whose holder is not found is looked for a hundred ticks from the
/// boat's take-in, and on its 101st tick lets go with a lead dropped and no client
/// told, as none was told of it.
#[test]
fn a_saved_leash_whose_holder_is_not_found_drops_a_lead_after_a_hundred_ticks() {
    let mut region = ticking_with(&[]);
    let saved = leashed_to(
        boat(1, [4.5, -60.0, 0.5]),
        LeashHolder::Entity(EntityId(1 << 42)),
    );
    lay(&mut region, &saved);
    for _ in 0..100 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(links(&output).is_empty());
    }
    assert!(leash(&region, saved.key).is_some());
    assert_eq!(leads(&region), 0);
    let output = checked(&mut region, &TickInputs::default());
    assert!(leash(&region, saved.key).is_none());
    assert!(links(&output).is_empty());
    assert_eq!(leads(&region), 1);
}

/// A saved leash held by the player is found by their stay's key at once.
#[test]
fn a_saved_leash_held_by_a_player_is_found_by_their_key() {
    let mut region = ticking_with(&[]);
    let saved = leashed_to(boat(1, [4.5, -60.0, 0.5]), LeashHolder::Entity(HOLDER));
    lay(&mut region, &saved);
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(links(&output), [(saved.key, Some(HOLDER))]);
    assert!(region.leash_found(saved.key));
}

// Across a line.

/// A leash found whose holder the region neither has nor sees (a player of another
/// region, whom no copy shows) is out of reach: it drops with a lead on the boat's
/// next tick, and clients are told.
#[test]
fn a_holder_in_another_region_snaps_the_leash() {
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let elsewhere = EntityId(2);
    let target = leashed_to(boat(1, [14.5, -60.0, 8.5]), LeashHolder::Entity(elsewhere));
    lay_found(&mut region, &target);
    let output = checked(&mut region, &TickInputs::default());
    assert!(leash(&region, target.key).is_none());
    assert_eq!(links(&output), [(target.key, None)]);
    assert_eq!(leads(&region), 1);
}

/// A held leash whose boat is within the snap distance of another region's chunks
/// counts as entities at work at that line, by that region, so that regions that
/// follow players merge; once it is let go it counts no more.
#[test]
fn a_held_leash_near_a_line_counts_as_entities_working_there() {
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let Knowledge::Foreign(neighbour) = region.knowledge(ChunkPos::new(1, 0)) else {
        panic!("{:?}", region.knowledge(ChunkPos::new(1, 0)));
    };
    assert!(region.entities_at_line().is_empty());
    let target = leashed_to(boat(1, [10.5, -60.0, 8.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &target);
    checked(&mut region, &walk_to(5.5, 8.5));
    assert!(leash(&region, target.key).is_some());
    let at_line = region.entities_at_line();
    assert!(
        at_line.contains(&(ChunkPos::new(1, 0), Some(neighbour))),
        "{at_line:?}"
    );
    assert!(at_line.iter().all(|(_, other)| *other == Some(neighbour)));
    // A boat far from the line counts nowhere.
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let inland = leashed_to(boat(1, [-12.5, -60.0, 8.5]), LeashHolder::Entity(HOLDER));
    lay_found(&mut region, &inland);
    assert!(region.entities_at_line().is_empty());
}
