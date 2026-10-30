//! The Nether portal's blocks and the portal clock in a region, from the game's rules
//! (`docs/groundwork/p-brief.md`, sections 2.1, 2.7, 4.1 and 4.2; `p-plan.md`, 5.1),
//! written by someone who did not write the code: lighting by each way fire is set, in
//! each dimension; breaking by each frame block and what does not break a portal; and
//! the clock tick by tick for players and other entities, kept across a restore.
//!
//! The order of a tick is the game's: an entity's `baseTick` counts its clock
//! (`handlePortal`) before its move marks it inside a portal (`applyEffectsFromBlocks`,
//! in `LivingEntity.aiStep` and each entity's own tick). A player's moves are the
//! packets before their tick, recorded and applied in that tick's `aiStep`, after its
//! `handlePortal` (LivingEntity.txt of n7, 8214–8500). So whoever steps into a portal
//! is marked in the tick they step in and fires, where it makes them wait for nothing,
//! in the tick after.
//!
//! The world is that of the tests of redstone (`redstone.rs`): player 1 at (0.5, -60,
//! 0.5) on a floor of stone at y = -61, the chunks within two of chunk (0, 0) loaded
//! and ticking. The frames stand on the floor.

use clustine_blocks::data::set;
use clustine_data::{Face, entity_types, items};
use clustine_world::{
    Entity, EntityChange, EntityData, EntityField, Hurt, PortalAxis, PortalClock, PortalKind,
    PortalState, Seat,
};

use super::redstone::{api_face, block_at, holding, state, ticking_with};
use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::types::villager;
use crate::region::portals::{
    PLAYER_DIMENSION_CHANGING_DELAY, dimension_changing_delay, entity_can_use_portal,
    handle_portal, set_as_inside_portal, transition_time,
};

/// A frame of obsidian whose inside is `width` by `height`; `u` counts along the axis
/// from `base` (x for X, z for Z) and `v` up from it.
#[derive(Clone, Copy)]
struct Frame {
    axis: PortalAxis,
    base: BlockPos,
    width: i32,
    height: i32,
}

impl Frame {
    fn at(&self, u: i32, v: i32) -> BlockPos {
        match self.axis {
            PortalAxis::X => BlockPos::new(self.base.x + u, self.base.y + v, self.base.z),
            PortalAxis::Z => BlockPos::new(self.base.x, self.base.y + v, self.base.z + u),
        }
    }

    fn inside(&self) -> Vec<BlockPos> {
        let mut cells = Vec::new();
        for v in 0..self.height {
            for u in 0..self.width {
                cells.push(self.at(u, v));
            }
        }
        cells
    }

    /// The frame's blocks but its corners.
    fn sides(&self) -> Vec<BlockPos> {
        let mut sides = Vec::new();
        for u in 0..self.width {
            sides.push(self.at(u, -1));
            sides.push(self.at(u, self.height));
        }
        for v in 0..self.height {
            sides.push(self.at(-1, v));
            sides.push(self.at(self.width, v));
        }
        sides
    }

    fn corners(&self) -> [BlockPos; 4] {
        [
            self.at(-1, -1),
            self.at(self.width, -1),
            self.at(-1, self.height),
            self.at(self.width, self.height),
        ]
    }

    fn blocks(&self) -> Vec<(BlockPos, BlockState)> {
        self.sides()
            .into_iter()
            .chain(self.corners())
            .map(|pos| (pos, blocks::OBSIDIAN))
            .collect()
    }

    fn portal(&self) -> BlockState {
        portal(self.axis)
    }
}

fn portal(axis: PortalAxis) -> BlockState {
    let name = match axis {
        PortalAxis::X => "x",
        PortalAxis::Z => "z",
    };
    set(blocks::NETHER_PORTAL, "axis", name)
}

/// The frame of most tests: two wide and three high along X, its inside x = 2..=3 at
/// z = 3, from y = -59 (on its bottom, which stands on the floor) to -57. The player
/// reaches all of it from the spawn.
const FRAME: Frame = Frame {
    axis: PortalAxis::X,
    base: BlockPos::new(2, -59, 3),
    width: 2,
    height: 3,
};

/// Where a player or an entity stands in [`FRAME`]'s portal, on its bottom.
const IN_PORTAL: [f64; 3] = [2.5, -59.0, 3.5];

fn holds(region: &Region, frame: &Frame, state: BlockState) -> bool {
    frame
        .inside()
        .into_iter()
        .all(|pos| block_at(region, pos) == Some(state))
}

fn lit(region: &Region, frame: &Frame) -> bool {
    holds(region, frame, frame.portal())
}

/// No portal block of either axis anywhere in the frame's inside.
fn unlit(region: &Region, frame: &Frame) -> bool {
    frame.inside().into_iter().all(|pos| {
        block_at(region, pos)
            .is_none_or(|state| state.block_id() != blocks::NETHER_PORTAL.block_id())
    })
}

/// Sets `state` at `pos` as a player's or the region's own change (every update), in
/// no tick.
fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
    let mut output = TickOutput::default();
    region.set_block(pos, state, &mut output);
}

/// A region with `frame` built and nothing lit.
fn framed(frame: &Frame) -> Region {
    ticking_with(&frame.blocks())
}

/// A region with [`FRAME`] built and lit by fire set in its bottom corner.
fn portal_region() -> Region {
    let mut region = framed(&FRAME);
    put(&mut region, FRAME.at(0, 0), blocks::FIRE);
    assert!(lit(&region, &FRAME));
    region
}

/// Player 1 clicks `face` of the block at `at` with what is in hand.
fn click(at: BlockPos, face: Face) -> TickInputs {
    static SEQUENCE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
    moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: at,
            face: api_face(face),
            cursor: [0.5; 3],
            sneaking: false,
            sequence: SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        },
    )])
}

/// Player 1 moves to stand at `position`, on the ground.
fn stand(position: [f64; 3]) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(position[0], position[1], position[2])),
            rotation: None,
            on_ground: true,
        },
    )])
}

fn rest(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

fn player_portal(region: &Region) -> PortalState {
    region.players[&player(1)].portal
}

fn clock(entry: BlockPos, time: u32, inside: bool) -> PortalClock {
    PortalClock {
        kind: PortalKind::Nether,
        entry,
        time,
        inside,
    }
}

fn portal_state(clock: Option<PortalClock>, cooldown: u32) -> PortalState {
    PortalState { clock, cooldown }
}

/// The portal blocks a player standing at [`IN_PORTAL`] has their box in: either may
/// be the one the game meets first.
fn entries() -> [BlockPos; 2] {
    [FRAME.at(0, 0), FRAME.at(0, 1)]
}

// Lighting.

/// Flint and steel on the top of the frame's bottom sets fire on it, which fills the
/// frame with portal along X at once, the fire among it.
#[test]
fn flint_and_steel_on_the_frame_s_bottom_lights_the_portal() {
    let mut region = framed(&FRAME);
    checked(&mut region, &holding(items::FLINT_AND_STEEL));
    checked(&mut region, &click(FRAME.at(0, -1), Face::Up));
    assert!(lit(&region, &FRAME));
}

/// Fire set where it could not stay, under the frame's top with air below it, is set
/// all the same because it lights a frame (`canBePlacedAt`, `isPortal`), by flint and
/// steel and by a fire charge, which is used up.
#[test]
fn flint_and_steel_and_a_fire_charge_light_the_frame_from_its_top_row() {
    for item in [items::FLINT_AND_STEEL, items::FIRE_CHARGE] {
        let mut region = framed(&FRAME);
        checked(&mut region, &holding(item));
        checked(&mut region, &click(FRAME.at(1, 3), Face::Down));
        assert!(lit(&region, &FRAME), "{item}");
        // The fire's own tick, scheduled as it was set, finds portal and leaves it.
        for _ in 0..60 {
            rest(&mut region);
        }
        assert!(lit(&region, &FRAME), "{item}, later");
    }
}

/// The axis a player faces only orders the search, which tries the other axis after:
/// a frame along Z is lit by a player facing along X and along Z.
#[test]
fn a_frame_along_z_is_lit_whichever_way_the_player_faces() {
    let along_z = Frame {
        axis: PortalAxis::Z,
        base: BlockPos::new(3, -59, 2),
        width: 2,
        height: 3,
    };
    for yaw in [0.0_f32, 90.0, 180.0, -90.0] {
        let mut region = framed(&along_z);
        checked(
            &mut region,
            &moves(vec![numbered(
                1,
                PlayerInput::Move {
                    position: None,
                    rotation: Some((yaw, 0.0)),
                    on_ground: true,
                },
            )]),
        );
        checked(&mut region, &holding(items::FLINT_AND_STEEL));
        checked(&mut region, &click(along_z.at(0, 3), Face::Down));
        assert!(lit(&region, &along_z), "yaw {yaw}");
    }
}

/// A dispenser of flint and steel facing into the frame's top row, where fire could not
/// stay, sets fire there, which lights the frame.
#[test]
fn a_dispenser_s_flint_and_steel_lights_the_frame_where_fire_could_not_stay() {
    let at = BlockPos::new(2, -57, 2);
    let mut blocks = FRAME.blocks();
    blocks.push((
        at,
        state("minecraft:dispenser[facing=south,triggered=false]"),
    ));
    let mut region = ticking_with(&blocks);
    let mut container = region.container_at(at).expect("a dispenser");
    container.slots[0] = Some(ItemStack::new(items::FLINT_AND_STEEL, 1));
    region.set_container(at, container, &mut TickOutput::default());
    let mut output = TickOutput::default();
    let dispenser = region.block(at).expect("loaded");
    assert!(region.dispenser_tick(at, dispenser, &mut output));
    assert!(lit(&region, &FRAME));
}

/// Fire set by any rule (here the region's own change, as fire spreading or lava
/// sets it) lights a frame in the overworld and in the Nether, along X or Z; and soul
/// fire does too.
#[test]
fn fire_or_soul_fire_set_by_any_rule_lights_a_frame_in_the_overworld_and_the_nether() {
    for dimension in [Dimension::Overworld, Dimension::Nether] {
        for axis in [PortalAxis::X, PortalAxis::Z] {
            for fire in [blocks::FIRE, blocks::SOUL_FIRE] {
                // Floating well above the Nether's floor, in the air.
                let frame = Frame {
                    axis,
                    base: BlockPos::new(5, 20, 6),
                    width: 3,
                    height: 4,
                };
                let mut region = framed(&frame);
                region.config.dimension = dimension;
                put(&mut region, frame.at(0, 0), fire);
                assert!(lit(&region, &frame), "{dimension:?} {axis:?} {fire:?}");
            }
        }
    }
}

/// In the End no frame is lit: fire set in it stays where it can, and flint and steel
/// sets none where it could not stay.
#[test]
fn no_frame_is_lit_in_the_end() {
    let mut region = framed(&FRAME);
    region.config.dimension = Dimension::End;
    put(&mut region, FRAME.at(1, 0), blocks::FIRE);
    assert!(unlit(&region, &FRAME));
    assert_eq!(block_at(&region, FRAME.at(1, 0)), Some(blocks::FIRE));

    let mut region = framed(&FRAME);
    region.config.dimension = Dimension::End;
    checked(&mut region, &holding(items::FLINT_AND_STEEL));
    checked(&mut region, &click(FRAME.at(1, 3), Face::Down));
    assert_eq!(block_at(&region, FRAME.at(1, 2)), Some(blocks::AIR));
    assert!(unlit(&region, &FRAME));
    assert!(!region.fire_would_light_portal(FRAME.at(1, 2), Face::South, None));
}

/// Fire set over fire of the same block lights nothing (`onPlace` returns where the old
/// block was the same): a frame completed around fire already in it stays dark when
/// the fire is set again; soul fire set over that fire lights it.
#[test]
fn fire_set_where_that_fire_was_lights_nothing_and_soul_fire_over_fire_does() {
    let top = FRAME.at(1, 3);
    let blocks: Vec<_> = FRAME
        .blocks()
        .into_iter()
        .filter(|(pos, _)| *pos != top)
        .chain([(FRAME.at(0, 0), blocks::FIRE)])
        .collect();
    let mut region = ticking_with(&blocks);
    put(&mut region, top, blocks::OBSIDIAN);
    assert!(unlit(&region, &FRAME), "completing a frame lights nothing");
    let older = set(blocks::FIRE, "age", "5");
    put(&mut region, FRAME.at(0, 0), older);
    assert!(unlit(&region, &FRAME));
    assert_eq!(block_at(&region, FRAME.at(0, 0)), Some(older));
    put(&mut region, FRAME.at(0, 0), blocks::SOUL_FIRE);
    assert!(lit(&region, &FRAME));
}

/// Fire already in a frame is empty to the search: fire set elsewhere in it fills the
/// whole inside, the old fire's block too.
#[test]
fn fire_already_inside_a_frame_is_replaced_by_the_portal() {
    let mut region = framed(&FRAME);
    put(&mut region, FRAME.at(1, 2), blocks::OBSIDIAN);
    put(&mut region, FRAME.at(1, 1), blocks::FIRE);
    // Not a frame with obsidian inside: the fire stays.
    assert!(unlit(&region, &FRAME));
    put(&mut region, FRAME.at(1, 2), blocks::AIR);
    put(&mut region, FRAME.at(0, 0), blocks::FIRE);
    assert!(lit(&region, &FRAME));
}

/// A frame with a solid block inside is not lit, and the fire stays where it can.
#[test]
fn a_frame_with_a_block_inside_is_not_lit() {
    let mut region = framed(&FRAME);
    put(&mut region, FRAME.at(1, 1), blocks::STONE);
    put(&mut region, FRAME.at(0, 0), blocks::FIRE);
    assert_eq!(block_at(&region, FRAME.at(0, 0)), Some(blocks::FIRE));
    assert!(unlit(&region, &FRAME));
}

/// The largest frame, 21 by 21 inside, is lit from its top row; one block wider is
/// not, and the fire there goes, as it cannot stay in the air.
#[test]
fn the_largest_frame_is_lit_and_one_block_wider_is_not() {
    let largest = Frame {
        axis: PortalAxis::X,
        base: BlockPos::new(2, -59, 5),
        width: 21,
        height: 21,
    };
    let mut region = framed(&largest);
    put(&mut region, largest.at(20, 20), blocks::FIRE);
    assert!(lit(&region, &largest));

    let wider = Frame {
        width: 22,
        ..largest
    };
    let mut region = framed(&wider);
    put(&mut region, wider.at(10, 20), blocks::FIRE);
    assert!(unlit(&region, &wider));
    assert_eq!(block_at(&region, wider.at(10, 20)), Some(blocks::AIR));
}

/// Two frames along X and Z that share a post are each lit along their own axis, and
/// lighting one leaves the other alone; breaking the shared post drops both.
#[test]
fn frames_of_both_axes_sharing_a_post_light_and_break_together_only_by_that_post() {
    let along_x = Frame {
        axis: PortalAxis::X,
        base: BlockPos::new(1, -59, 3),
        width: 2,
        height: 3,
    };
    // Its north post is the east post of the frame along X, at x = 3, z = 3.
    let along_z = Frame {
        axis: PortalAxis::Z,
        base: BlockPos::new(3, -59, 4),
        width: 2,
        height: 3,
    };
    let mut blocks = along_x.blocks();
    blocks.extend(along_z.blocks());
    let mut region = ticking_with(&blocks);
    put(&mut region, along_x.at(1, 1), blocks::FIRE);
    assert!(lit(&region, &along_x));
    assert!(holds(&region, &along_z, blocks::AIR));
    put(&mut region, along_z.at(0, 0), blocks::FIRE);
    assert!(lit(&region, &along_z));
    assert!(lit(&region, &along_x));
    put(&mut region, BlockPos::new(3, -58, 3), blocks::AIR);
    assert!(holds(&region, &along_x, blocks::AIR));
    assert!(holds(&region, &along_z, blocks::AIR));
}

/// Where a block is inside a frame along X and one along Z, fire there lights along X,
/// which the game tries first; the frame along Z, which then holds portal, is not
/// lit by fire set in the rest of it.
#[test]
fn fire_inside_frames_of_both_axes_lights_along_x() {
    let along_x = Frame {
        axis: PortalAxis::X,
        base: BlockPos::new(2, -59, 3),
        width: 2,
        height: 3,
    };
    let along_z = Frame {
        axis: PortalAxis::Z,
        base: BlockPos::new(2, -59, 3),
        width: 2,
        height: 3,
    };
    let mut blocks = along_x.blocks();
    blocks.extend(along_z.blocks());
    let mut region = ticking_with(&blocks);
    put(&mut region, BlockPos::new(2, -59, 3), blocks::FIRE);
    assert!(lit(&region, &along_x));
    let only_z = BlockPos::new(2, -59, 4);
    put(&mut region, only_z, blocks::FIRE);
    assert_eq!(block_at(&region, only_z), Some(blocks::FIRE));
    assert_eq!(
        block_at(&region, BlockPos::new(2, -58, 4)),
        Some(blocks::AIR)
    );
}

// Breaking.

/// Each block of the frame but the corners, taken away in turn, drops the whole
/// portal through the neighbours' updates; a corner taken away drops nothing.
#[test]
fn each_frame_block_taken_away_drops_the_whole_portal_and_a_corner_does_not() {
    for side in FRAME.sides() {
        let mut region = portal_region();
        put(&mut region, side, blocks::AIR);
        assert!(holds(&region, &FRAME, blocks::AIR), "{side:?}");
    }
    for corner in FRAME.corners() {
        let mut region = portal_region();
        put(&mut region, corner, blocks::AIR);
        assert!(lit(&region, &FRAME), "{corner:?}");
    }
}

/// A frame block a creative player breaks drops the whole portal.
#[test]
fn a_frame_block_a_player_breaks_drops_the_whole_portal() {
    let mut region = portal_region();
    let post = FRAME.at(2, 1);
    checked(&mut region, &moves(vec![dig(1, post.x, post.y, post.z, 1)]));
    assert_eq!(block_at(&region, post), Some(blocks::AIR));
    assert!(holds(&region, &FRAME, blocks::AIR));
}

/// One portal block taken away drops the rest.
#[test]
fn one_portal_block_taken_away_drops_the_rest() {
    for cell in FRAME.inside() {
        let mut region = portal_region();
        put(&mut region, cell, blocks::AIR);
        assert!(holds(&region, &FRAME, blocks::AIR), "{cell:?}");
    }
}

/// The largest portal, 441 blocks, all go when one block of its frame does.
#[test]
fn the_largest_portal_goes_whole_when_its_frame_is_broken() {
    let largest = Frame {
        axis: PortalAxis::X,
        base: BlockPos::new(2, -59, 5),
        width: 21,
        height: 21,
    };
    for broken in [largest.at(10, -1), largest.at(21, 20), largest.at(0, 21)] {
        let mut region = framed(&largest);
        put(&mut region, largest.at(0, 0), blocks::FIRE);
        assert!(lit(&region, &largest));
        put(&mut region, broken, blocks::AIR);
        assert!(holds(&region, &largest, blocks::AIR), "{broken:?}");
    }
}

/// A block set in front of a portal block or behind it drops nothing; nor does a
/// portal block of the other axis set there.
#[test]
fn a_block_in_front_of_or_behind_a_portal_drops_nothing() {
    for z in [2, 4] {
        for placed in [blocks::STONE, blocks::OBSIDIAN, portal(PortalAxis::Z)] {
            let mut region = portal_region();
            put(&mut region, BlockPos::new(2, -58, z), placed);
            assert!(lit(&region, &FRAME), "{placed:?} at z = {z}");
            put(&mut region, BlockPos::new(2, -58, z), blocks::AIR);
            assert!(lit(&region, &FRAME), "taken from z = {z}");
        }
    }
}

/// A post block replaced at once by a portal block of the other axis drops nothing:
/// the portal beside it sees a portal neighbour, and the rest of it is not told.
#[test]
fn a_post_replaced_by_a_portal_block_of_the_other_axis_drops_nothing() {
    let mut region = portal_region();
    put(&mut region, FRAME.at(-1, 1), portal(PortalAxis::Z));
    assert!(lit(&region, &FRAME));
}

// The clock, for players.

/// A creative player is marked in the tick they step in and fires in the next, the
/// first that counts them inside; their cooldown is then 10, and standing in the
/// portal keeps it at 10, the clock that fired running out in the tick after.
#[test]
fn a_creative_player_fires_in_the_first_tick_that_counts_them_and_standing_keeps_the_cooldown() {
    let mut region = portal_region();
    let output = checked(&mut region, &stand(IN_PORTAL));
    let marked = player_portal(&region);
    let entry = marked.clock.expect("a clock").entry;
    assert!(entries().contains(&entry), "{entry:?}");
    assert_eq!(marked, portal_state(Some(clock(entry, 0, true)), 0));
    assert!(
        output
            .delta
            .players
            .iter()
            .any(|(id, state)| *id == player(1)
                && state.as_ref().is_some_and(|state| state.portal == marked)),
        "the marked clock is journalled"
    );

    checked(&mut region, &TickInputs::default());
    assert_eq!(
        player_portal(&region),
        portal_state(
            Some(clock(entry, 1, false)),
            PLAYER_DIMENSION_CHANGING_DELAY
        )
    );
    for tick in 0..30 {
        rest(&mut region);
        assert_eq!(
            player_portal(&region),
            portal_state(None, PLAYER_DIMENSION_CHANGING_DELAY),
            "tick {tick}"
        );
    }
}

/// Out of the portal the cooldown counts down by one a tick; back in on cooldown sets
/// it to 10 again and starts no clock; back in on the tick it reaches 0 starts one, which
/// fires in the next.
#[test]
fn a_player_s_cooldown_counts_down_outside_and_is_set_full_again_inside() {
    let mut region = portal_region();
    checked(&mut region, &stand(IN_PORTAL));
    rest(&mut region);
    rest(&mut region);
    checked(&mut region, &stand([0.5, -60.0, 0.5]));
    assert_eq!(player_portal(&region), portal_state(None, 9));
    for left in (6..=8).rev() {
        rest(&mut region);
        assert_eq!(player_portal(&region), portal_state(None, left));
    }
    // Back in with 6 left: counted to 5, then set to 10.
    checked(&mut region, &stand(IN_PORTAL));
    assert_eq!(player_portal(&region), portal_state(None, 10));
    checked(&mut region, &stand([0.5, -60.0, 0.5]));
    for left in (1..=9).rev() {
        assert_eq!(player_portal(&region), portal_state(None, left));
        rest(&mut region);
    }
    assert_eq!(player_portal(&region), portal_state(None, 0));
    checked(&mut region, &stand(IN_PORTAL));
    let entry = player_portal(&region).clock.expect("a new clock").entry;
    assert_eq!(
        player_portal(&region),
        portal_state(Some(clock(entry, 0, true)), 0)
    );
    rest(&mut region);
    assert_eq!(
        player_portal(&region),
        portal_state(Some(clock(entry, 1, false)), 10)
    );
}

/// Back in on the tick the cooldown runs out (1 before it): it is counted to 0 first,
/// and the step in starts a clock.
#[test]
fn a_player_back_in_as_the_cooldown_runs_out_starts_a_clock() {
    let mut region = portal_region();
    checked(&mut region, &stand(IN_PORTAL));
    rest(&mut region);
    checked(&mut region, &stand([0.5, -60.0, 0.5]));
    for _ in 0..8 {
        rest(&mut region);
    }
    assert_eq!(player_portal(&region), portal_state(None, 1));
    checked(&mut region, &stand(IN_PORTAL));
    let state = player_portal(&region);
    assert_eq!(state.cooldown, 0);
    assert!(
        state
            .clock
            .is_some_and(|clock| clock.inside && clock.time == 0)
    );
}

/// A clock left behind decays by four a tick outside the portal and goes at 0, each
/// change journalled; one of another kind is replaced by a Nether clock on the step in.
#[test]
fn a_player_s_clock_decays_by_four_outside_and_one_of_another_kind_is_begun_again() {
    let mut region = portal_region();
    let entry = FRAME.at(0, 0);
    region.players.get_mut(&player(1)).expect("joined").portal =
        portal_state(Some(clock(entry, 10, false)), 0);
    for expected in [Some(6), Some(2), None] {
        let output = rest(&mut region);
        let expected = portal_state(expected.map(|time| clock(entry, time, false)), 0);
        assert_eq!(player_portal(&region), expected);
        let journalled = output
            .delta
            .players
            .iter()
            .find(|(id, _)| *id == player(1))
            .and_then(|(_, state)| state.as_ref().map(|state| state.portal));
        assert_eq!(journalled, Some(expected));
    }

    let mut region = portal_region();
    region.players.get_mut(&player(1)).expect("joined").portal = portal_state(
        Some(PortalClock {
            kind: PortalKind::End,
            entry: BlockPos::new(100, 50, 0),
            time: 9,
            inside: false,
        }),
        0,
    );
    checked(&mut region, &stand(IN_PORTAL));
    let state = player_portal(&region);
    let clock_now = state.clock.expect("a clock");
    assert_eq!(clock_now.kind, PortalKind::Nether);
    assert_eq!(clock_now.time, 0);
    assert!(clock_now.inside);
    assert!(entries().contains(&clock_now.entry));
}

// The clock, for other entities.

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

fn entity(n: u64, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        portal: Default::default(),
        key: key(n),
        kind: kind as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

/// An item no player picks up, at rest at `position`.
fn item(n: u64, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ITEM,
        position,
        EntityData::Item {
            stack: DIRT,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    )
}

fn cart(n: u64, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::MINECART,
        position,
        EntityData::Minecart {
            hurt: Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    )
}

/// A mob of `kind` at `position` at its full health, or at `health`.
fn mob_at(n: u64, kind: i32, position: [f64; 3], mut mob: Mob, health: Option<f32>) -> Entity {
    let full = supplied(
        kind as u16,
        attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a mob") as f32;
    mob.body.vitals.health = health.unwrap_or(full);
    mob.body.vitals.air = crate::mobs::types::of(kind as u16).max_air();
    mob.body.flags.no_ai = true;
    entity(n, kind, position, mob.encode())
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn entity_portal(region: &Region, key: EntityId) -> Option<PortalState> {
    region.find_entity(key).map(|(_, entity)| entity.portal)
}

/// An entity other than a player in a portal is marked in its first tick there and
/// fires in the next, with a cooldown of 300, which standing in the portal keeps full;
/// each change is committed as its `Portal` field.
#[test]
fn an_entity_fires_in_its_second_tick_in_a_portal_and_stays_on_a_cooldown_of_300() {
    let mut region = portal_region();
    let thing = item(1, IN_PORTAL);
    lay(&mut region, &thing);
    let output = rest(&mut region);
    let marked = entity_portal(&region, thing.key).expect("there");
    assert_eq!(
        marked,
        portal_state(Some(clock(FRAME.at(0, 0), 0, true)), 0)
    );
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        EntityChange::Set(key, fields) if *key == thing.key
            && fields.iter().any(|field| matches!(field, EntityField::Portal(p) if *p == marked))
    )));
    rest(&mut region);
    assert_eq!(
        entity_portal(&region, thing.key),
        Some(portal_state(Some(clock(FRAME.at(0, 0), 1, false)), 300))
    );
    for tick in 0..40 {
        rest(&mut region);
        assert_eq!(
            entity_portal(&region, thing.key),
            Some(portal_state(None, 300)),
            "tick {tick}"
        );
    }
}

/// An entity that passes through a portal within one move, ending its tick beyond it,
/// is marked all the same (`checkInsideBlocks` follows the move); one that passes
/// beside it is not.
#[test]
fn an_entity_passing_through_a_portal_in_one_move_is_marked() {
    let mut region = portal_region();
    let mut thrown = item(1, [2.5, -58.5, 3.0]);
    thrown.on_ground = false;
    thrown.motion = [0.0, 0.0, 0.9];
    lay(&mut region, &thrown);
    rest(&mut region);
    let (_, after) = region.find_entity(thrown.key).expect("there");
    assert!(
        after.position[2] > 3.7,
        "it went through: {:?}",
        after.position
    );
    let marked = after.portal.clock.expect("a clock");
    assert!(marked.inside);
    assert_eq!(marked.time, 0);
    assert!(FRAME.inside().contains(&marked.entry), "{:?}", marked.entry);

    let mut region = portal_region();
    let mut beside = item(2, [6.5, -58.5, 3.0]);
    beside.on_ground = false;
    beside.motion = [0.0, 0.0, 0.9];
    lay(&mut region, &beside);
    rest(&mut region);
    assert_eq!(
        entity_portal(&region, beside.key),
        Some(PortalState::default())
    );
}

/// A portal block replaced by a solid block drops the rest, the solid block staying.
#[test]
fn a_portal_block_replaced_by_stone_drops_the_rest() {
    let mut region = portal_region();
    let cell = FRAME.at(1, 1);
    put(&mut region, cell, blocks::STONE);
    assert_eq!(block_at(&region, cell), Some(blocks::STONE));
    for other in FRAME.inside().into_iter().filter(|pos| *pos != cell) {
        assert_eq!(block_at(&region, other), Some(blocks::AIR), "{other:?}");
    }
}

/// Outside a portal an entity's cooldown counts down to 0, and its clock decays by
/// four a tick and goes at 0; in a portal a clock of another kind gives way to a
/// Nether clock begun at 0.
#[test]
fn an_entity_s_cooldown_counts_down_and_its_clock_decays_outside_a_portal() {
    let mut region = portal_region();
    let entry = FRAME.at(0, 0);
    let mut away = item(1, [8.5, -60.0, 8.5]);
    away.portal = portal_state(Some(clock(entry, 9, false)), 3);
    lay(&mut region, &away);
    let expected = [
        portal_state(Some(clock(entry, 5, false)), 2),
        portal_state(Some(clock(entry, 1, false)), 1),
        portal_state(None, 0),
        portal_state(None, 0),
    ];
    for state in expected {
        rest(&mut region);
        assert_eq!(entity_portal(&region, away.key), Some(state));
    }

    let mut region = portal_region();
    let mut inside = item(2, IN_PORTAL);
    inside.portal = portal_state(
        Some(PortalClock {
            kind: PortalKind::Gateway,
            entry: BlockPos::new(100, 50, 0),
            time: 9,
            inside: false,
        }),
        0,
    );
    lay(&mut region, &inside);
    rest(&mut region);
    assert_eq!(
        entity_portal(&region, inside.key),
        Some(portal_state(Some(clock(entry, 0, true)), 0))
    );
}

/// A vehicle in a portal fires for itself, its cooldown 300 with no player in its
/// first seat; its passenger never counts.
#[test]
fn a_cart_in_a_portal_fires_and_its_passenger_never_counts() {
    let mut region = portal_region();
    let mut vehicle = cart(1, IN_PORTAL);
    let mut rider = mob_at(2, entity_types::PIG, [2.5, -58.9, 3.5], Mob::fresh(), None);
    rider.riding.vehicle = Some(vehicle.key);
    vehicle.riding.passengers.push(Seat {
        key: rider.key,
        player: false,
        pending: None,
    });
    lay(&mut region, &rider);
    lay(&mut region, &vehicle);
    rest(&mut region);
    rest(&mut region);
    let cooldown = entity_portal(&region, vehicle.key).map(|state| state.cooldown);
    assert_eq!(cooldown, Some(300));
    for _ in 0..5 {
        rest(&mut region);
        assert_eq!(
            entity_portal(&region, rider.key),
            Some(PortalState::default())
        );
    }
}

/// A dead mob in a portal is never marked.
#[test]
fn a_dead_mob_is_never_marked() {
    let mut region = portal_region();
    let dead = mob_at(1, entity_types::PIG, IN_PORTAL, Mob::fresh(), Some(0.0));
    assert!(!entity_can_use_portal(&dead));
    lay(&mut region, &dead);
    for _ in 0..3 {
        rest(&mut region);
        assert_eq!(
            entity_portal(&region, dead.key),
            Some(PortalState::default())
        );
    }
}

/// A sleeping villager may use no portal; awake it may; and a passenger may not.
#[test]
fn a_sleeping_villager_and_a_passenger_may_use_no_portal() {
    let mut awake = Mob::fresh();
    let at = [8.5, -60.0, 8.5];
    let villager_awake = mob_at(1, entity_types::VILLAGER, at, awake.clone(), None);
    assert!(entity_can_use_portal(&villager_awake));
    villager::update(&mut awake, |own| {
        own.sleeping = Some(BlockPos::new(8, -60, 8))
    });
    let asleep = mob_at(1, entity_types::VILLAGER, at, awake, None);
    assert!(!entity_can_use_portal(&asleep));
    let mut riding = item(3, at);
    riding.riding.vehicle = Some(key(4));
    assert!(!entity_can_use_portal(&riding));
}

/// A vehicle's cooldown is its first passenger's where that is a player, 10, and 300
/// otherwise, whoever rides behind.
#[test]
fn a_vehicle_s_cooldown_is_a_player_s_only_with_a_player_in_its_first_seat() {
    let seat = |n: u64, player: bool| Seat {
        key: key(n),
        player,
        pending: None,
    };
    let mut vehicle = cart(1, [8.5, -60.0, 8.5]);
    assert_eq!(dimension_changing_delay(&vehicle), 300);
    vehicle.riding.passengers = vec![seat(2, true), seat(3, false)];
    assert_eq!(dimension_changing_delay(&vehicle), 10);
    vehicle.riding.passengers = vec![seat(3, false), seat(2, true)];
    assert_eq!(dimension_changing_delay(&vehicle), 300);
}

// The clock's rules, one call at a time.

/// A player who is not invulnerable waits 80 ticks: marked every tick, the clock fires
/// in the 81st count, the one whose time before it is 80.
#[test]
fn a_survival_player_s_clock_fires_on_the_count_that_finds_80() {
    let entry = BlockPos::new(1, 2, 3);
    let mut portal = PortalState::default();
    set_as_inside_portal(&mut portal, PortalKind::Nether, entry, 10);
    let wait = |kind| transition_time(kind, Some(false));
    for count in 1..=80 {
        let fired = handle_portal(&mut portal, || true, wait, 10);
        assert_eq!(fired, None, "count {count}");
        assert_eq!(portal.clock.map(|clock| clock.time), Some(count));
        set_as_inside_portal(&mut portal, PortalKind::Nether, entry, 10);
    }
    let fired = handle_portal(&mut portal, || true, wait, 10);
    assert_eq!(fired, Some(clock(entry, 81, false)));
    assert_eq!(portal, portal_state(Some(clock(entry, 81, false)), 10));
    assert_eq!(transition_time(PortalKind::Nether, Some(true)), 0);
    assert_eq!(transition_time(PortalKind::Nether, None), 0);
    assert_eq!(transition_time(PortalKind::End, Some(false)), 0);
    assert_eq!(transition_time(PortalKind::Gateway, Some(false)), 0);
}

/// Leaving and coming back within the decay goes on from what is left: 40 decays to
/// 32 in two ticks out, and the next count inside makes it 33; the entry is the new
/// one; a second mark in one tick keeps the first entry.
#[test]
fn coming_back_within_the_decay_goes_on_from_what_is_left() {
    let first = BlockPos::new(1, 2, 3);
    let second = BlockPos::new(1, 3, 3);
    let mut portal = portal_state(Some(clock(first, 40, false)), 0);
    let wait = |kind| transition_time(kind, Some(false));
    handle_portal(&mut portal, || true, wait, 10);
    handle_portal(&mut portal, || true, wait, 10);
    assert_eq!(portal, portal_state(Some(clock(first, 32, false)), 0));
    set_as_inside_portal(&mut portal, PortalKind::Nether, second, 10);
    set_as_inside_portal(&mut portal, PortalKind::Nether, first, 10);
    assert_eq!(portal, portal_state(Some(clock(second, 32, true)), 0));
    assert_eq!(handle_portal(&mut portal, || true, wait, 10), None);
    assert_eq!(portal, portal_state(Some(clock(second, 33, false)), 0));
}

/// The decay is four a tick, never below 0, and a clock at 0 goes in that tick; a
/// clock marked but not allowed neither counts nor decays in that tick, and goes only
/// where it is at 0.
#[test]
fn the_decay_is_four_a_tick_and_the_clock_goes_at_0() {
    let entry = BlockPos::new(1, 2, 3);
    let wait = |kind| transition_time(kind, Some(false));
    let mut portal = portal_state(Some(clock(entry, 4, false)), 0);
    handle_portal(&mut portal, || true, wait, 10);
    assert_eq!(portal, PortalState::default());
    let mut portal = portal_state(Some(clock(entry, 5, false)), 0);
    handle_portal(&mut portal, || true, wait, 10);
    assert_eq!(portal, portal_state(Some(clock(entry, 1, false)), 0));
    handle_portal(&mut portal, || true, wait, 10);
    assert_eq!(portal, PortalState::default());

    let mut portal = portal_state(Some(clock(entry, 7, true)), 0);
    handle_portal(&mut portal, || false, wait, 10);
    assert_eq!(portal, portal_state(Some(clock(entry, 7, false)), 0));
    let mut portal = portal_state(Some(clock(entry, 0, true)), 0);
    assert_eq!(handle_portal(&mut portal, || false, |_| 0, 10), None);
    assert_eq!(portal, PortalState::default());
}

/// The cooldown is counted before the clock, and a firing sets it full: one at 1 with
/// a marked clock ends the tick at the full cooldown. On cooldown a mark sets the
/// cooldown full and leaves the clock alone.
#[test]
fn the_cooldown_is_counted_first_and_set_full_by_a_firing_or_a_mark() {
    let entry = BlockPos::new(1, 2, 3);
    let mut portal = portal_state(Some(clock(entry, 0, true)), 1);
    let fired = handle_portal(&mut portal, || true, |_| 0, 300);
    assert_eq!(fired.map(|clock| clock.entry), Some(entry));
    assert_eq!(portal.cooldown, 300);

    let mut portal = portal_state(Some(clock(entry, 3, false)), 5);
    set_as_inside_portal(&mut portal, PortalKind::End, BlockPos::new(9, 9, 9), 300);
    assert_eq!(portal, portal_state(Some(clock(entry, 3, false)), 300));
    let mut idle = PortalState::default();
    assert_eq!(handle_portal(&mut idle, || true, |_| 0, 300), None);
    assert_eq!(idle, PortalState::default());
}

// A restore.

/// The portal state the store would have of the item, from the chunk as it was laid
/// and every entity change committed since, and of player 1 from every state
/// journalled since: what a restore after a crash starts from.
struct Kept {
    chunk: Chunk,
    player: PortalState,
}

impl Kept {
    fn take(&mut self, output: &TickOutput, position: ChunkPos) {
        for (chunk, change) in &output.entity_changes {
            if *chunk == position {
                self.chunk.apply_entity(change);
            }
        }
        for (id, state) in &output.delta.players {
            if *id == player(1)
                && let Some(state) = state
            {
                self.player = state.portal;
            }
        }
    }

    fn of(&self, key: EntityId) -> Option<PortalState> {
        self.chunk
            .entities()
            .iter()
            .find(|entity| entity.key == key)
            .map(|entity| entity.portal)
    }
}

/// A region saved while an item and a player stand in a portal, both marked and about
/// to fire, and restored from what its commits kept: both fire in the restored
/// region's next tick as in the region that went on, and their clocks and cooldowns go
/// on alike tick by tick.
#[test]
fn a_region_restored_while_an_entity_and_a_player_stand_in_a_portal_goes_on_alike() {
    let mut region = portal_region();
    let thing = item(1, IN_PORTAL);
    lay(&mut region, &thing);
    let position = thing.chunk();
    let mut kept = Kept {
        chunk: region.saved_chunk(position).expect("loaded"),
        player: player_portal(&region),
    };
    let output = checked(&mut region, &stand(IN_PORTAL));
    kept.take(&output, position);
    assert!(
        player_portal(&region)
            .clock
            .is_some_and(|clock| clock.inside)
    );
    assert!(
        entity_portal(&region, thing.key)
            .and_then(|state| state.clock)
            .is_some_and(|clock| clock.inside)
    );
    assert_eq!(kept.of(thing.key), entity_portal(&region, thing.key));
    assert_eq!(kept.player, player_portal(&region));

    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|at| {
            let chunk = if *at == position {
                kept.chunk.clone()
            } else {
                region.saved_chunk(*at).expect("loaded")
            };
            (*at, chunk)
        })
        .collect();
    let ticketed: Vec<(ChunkPos, Ticket)> = around.iter().map(|at| (*at, Ticket::Viewer)).collect();
    let mut state = region.state();
    state.players.get_mut(&player(1)).expect("joined").portal = kept.player;
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        state,
        Holdings {
            pinned: Vec::new(),
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &ticketed,
        chunks,
        Vec::new(),
    );
    for tick in 1..=15 {
        let output = rest(&mut region);
        kept.take(&output, position);
        rest(&mut restored);
        assert_eq!(
            entity_portal(&restored, thing.key),
            entity_portal(&region, thing.key),
            "the item, tick {tick}"
        );
        assert_eq!(
            player_portal(&restored),
            player_portal(&region),
            "the player, tick {tick}"
        );
        assert_eq!(
            kept.of(thing.key),
            entity_portal(&region, thing.key),
            "tick {tick}"
        );
        assert_eq!(kept.player, player_portal(&region), "tick {tick}");
        if tick == 1 {
            assert_eq!(player_portal(&restored).cooldown, 10);
            assert_eq!(
                entity_portal(&restored, thing.key).map(|state| state.cooldown),
                Some(300)
            );
        }
    }
}
