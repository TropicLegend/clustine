//! The village goals of mobs that are not villagers (`docs/groundwork/n10-plan.md`,
//! section 10; the brief's 8.1 and 8.6): `StrollThroughVillageGoal` (a fox's, by night),
//! `MoveThroughVillageGoal` (a zombie's, by night, to the village's points of
//! interest), and what the iron golem's goals share with them: `SectionPos.cube`,
//! `BehaviorUtils.findSectionClosestToVillage` and `PoiManager.find`.
//!
//! The village is read through the view (`MobWorld::sections_to_village` and the
//! points of interest of `MobWorld::pois_in_range`), which a region answers by the
//! stand-in of the kept villages' pieces until villagers claim (`region/pois.rs`).
//!
//! Named differences: `MoveThroughVillageGoal` sets its mob's navigation to open doors
//! once it has made its path, for good (`setCanOpenDoors(true)`); a mob's navigation is
//! its type's here, and so a zombie's paths never open doors, before the goal's first
//! use as after; the goal's path to a door on its way is made with the mob's own doors
//! too, which for a zombie below hard is what the game's `canBreakDoors` gives.

use clustine_blocks::data::{is_block, is_class};
use clustine_data::{BlockClass, blocks};
use clustine_world::BlockPos;
use clustine_world::poi::PoiType;
use serde::{Deserialize, Serialize};

use super::Ai;
use super::maths::reduced_tick_delay;
use super::navigation::{self, Made};
use super::pathfinder::FoundPath;
use super::random_pos;
use super::selector::{Goal, flags};
use crate::mobs::view::MobWorld;
use crate::physics::Vec3;
use crate::region::pois::{Occupancy, PoiRecord, dist_sqr, section_of};

/// `(double)(float)(π / 2)`.
pub(crate) const HALF_PI: f64 = 1.570_796_370_506_286_6;

/// `SectionPos.cube(section, r)`: every section within `r` on each axis, x fastest,
/// then y, then z, each from its least (`docs/groundwork/n10-dumps.md`, part A, 5).
pub(crate) fn section_cube(section: [i32; 3], r: i32) -> Vec<[i32; 3]> {
    let mut cube = Vec::new();
    for z in section[2] - r..=section[2] + r {
        for y in section[1] - r..=section[1] + r {
            for x in section[0] - r..=section[0] + r {
                cube.push([x, y, z]);
            }
        }
    }
    cube
}

/// `SectionPos.center()`: the middle block of a section.
pub(crate) fn section_centre(section: [i32; 3]) -> BlockPos {
    BlockPos::new(
        section[0] * 16 + 8,
        section[1] * 16 + 8,
        section[2] * 16 + 8,
    )
}

/// `Vec3.atBottomCenterOf(pos)`.
pub(crate) fn bottom_centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// `BlockPos.closerToCenterThan(position, distance)`: the block's middle nearer
/// `position` than `distance`.
pub(crate) fn closer_to_centre_than(pos: BlockPos, position: Vec3, distance: f64) -> bool {
    let dx = f64::from(pos.x) + 0.5 - position[0];
    let dy = f64::from(pos.y) + 0.5 - position[1];
    let dz = f64::from(pos.z) + 0.5 - position[2];
    dx * dx + dy * dy + dz * dz < distance * distance
}

/// `BehaviorUtils.findSectionClosestToVillage(level, section, r)`: of the sections of
/// the cube of `r` about `section` nearer a village than `section` is, the nearest, the
/// first of those as near; `section` itself where none is nearer.
pub(crate) fn find_section_closest_to_village(
    world: &dyn MobWorld,
    section: [i32; 3],
    r: i32,
) -> [i32; 3] {
    let level = world.sections_to_village(section);
    let mut best: Option<([i32; 3], i32)> = None;
    for candidate in section_cube(section, r) {
        let distance = world.sections_to_village(candidate);
        if distance < level && best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((candidate, distance));
        }
    }
    best.map_or(section, |(found, _)| found)
}

/// `PoiManager.find(type, test, centre, r, occupancy)`: the first record within `r`, in
/// the query's order, of a wanted type, whose position passes `test`.
pub(crate) fn find_poi(
    world: &dyn MobWorld,
    wanted: &dyn Fn(PoiType) -> bool,
    test: &dyn Fn(BlockPos) -> bool,
    centre: BlockPos,
    r: i32,
    occupancy: Occupancy,
) -> Option<PoiRecord> {
    world
        .pois_in_range(centre, r, wanted, occupancy)
        .into_iter()
        .find(|record| test(record.pos))
}

/// `DoorBlock.isWoodenDoor(level, pos)`: a door a hand opens, any but the iron one.
pub(crate) fn is_wooden_door(world: &dyn MobWorld, pos: BlockPos) -> bool {
    let state = world.state(pos);
    is_class(state, BlockClass::DoorBlock) && !is_block(state, blocks::IRON_DOOR)
}

/// `Level.isBrightOutside()`: the sky darkened by less than four.
fn bright_outside(ai: &Ai<'_, '_>) -> bool {
    ai.tick.world.sky_darken() < 4
}

/// `StrollThroughVillageGoal`'s state: where it wants to go.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct StrollingThroughVillage {
    pub(crate) wanted: Option<BlockPos>,
}

/// `StrollThroughVillageGoal(mob, interval)`: by night, about once in `interval` asks,
/// within six sections of a village, somewhere up to 15 blocks off scored by how near a
/// village it is; ten blocks at a time towards it, on the surface.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StrollThroughVillageGoal {
    pub(crate) interval: i32,
}

impl StrollThroughVillageGoal {
    /// `moveRandomly()`: somewhere within eight blocks on the surface, of two draws.
    fn move_randomly(ai: &mut Ai<'_, '_>) {
        let at = ai.block_position();
        let x = at.x - 8 + ai.tick.draws.next_int(16) as i32;
        let z = at.z - 8 + ai.tick.draws.next_int(16) as i32;
        let Some(y) = ai.tick.world.motion_blocking_no_leaves_height(x, z) else {
            return;
        };
        navigation::move_to(ai, [f64::from(x), f64::from(y), f64::from(z)], 1.0);
    }
}

impl Goal for StrollThroughVillageGoal {
    type State = StrollingThroughVillage;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.carries || bright_outside(ai) {
            return false;
        }
        let interval = reduced_tick_delay(self.interval).max(1) as u32;
        if ai.tick.draws.next_int(interval) != 0 {
            return false;
        }
        if !ai.tick.world.is_close_to_village(ai.block_position(), 6) {
            return false;
        }
        let found = random_pos::land_pos_scored(ai, 15, 7, |ai, pos| {
            -f64::from(ai.tick.world.sections_to_village(section_of(pos)))
        });
        state.wanted = found.map(crate::physics::containing);
        state.wanted.is_some()
    }

    fn can_continue(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        state.wanted.is_some()
            && !navigation::is_done(ai)
            && navigation::target_pos(ai) == state.wanted
    }

    /// Ten blocks along the line from where it stands to a point two fifths of the way
    /// back from the place it wants, on the surface there; somewhere near at random where
    /// no path goes.
    fn tick(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) {
        let Some(wanted) = state.wanted else {
            return;
        };
        let position = ai.position();
        if !navigation::is_done(ai) || closer_to_centre_than(wanted, position, 10.0) {
            return;
        }
        let target = bottom_centre(wanted);
        let back: Vec3 = std::array::from_fn(|axis| (position[axis] - target[axis]) * 0.4);
        let towards: Vec3 = std::array::from_fn(|axis| back[axis] + target[axis]);
        let direction: Vec3 = std::array::from_fn(|axis| towards[axis] - position[axis]);
        let length = direction
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        // `Vec3.normalize()`: nothing for a length below 1e-5.
        let unit: Vec3 = if length < 1.0e-5 {
            [0.0; 3]
        } else {
            direction.map(|value| value / length)
        };
        let step: Vec3 = std::array::from_fn(|axis| unit[axis] * 10.0 + position[axis]);
        let block = crate::physics::containing(step);
        let Some(y) = ai
            .tick
            .world
            .motion_blocking_no_leaves_height(block.x, block.z)
        else {
            return;
        };
        let at = [f64::from(block.x), f64::from(y), f64::from(block.z)];
        if !navigation::move_to(ai, at, 1.0) {
            Self::move_randomly(ai);
        }
    }
}

/// `MoveThroughVillageGoal`'s state: the path its `canUse` made, the point of interest
/// it heads for, and the last points it reached.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MovingThroughVillage {
    pub(crate) path: Option<FoundPath>,
    pub(crate) poi: Option<BlockPos>,
    pub(crate) visited: Vec<BlockPos>,
}

/// `MoveThroughVillageGoal(mob, speed, onlyAtNight, distanceToPoi, canDealWithDoors)`
/// (n7 MoveThroughVillageGoal 55–401): by night (where `only_at_night`), within six
/// sections of a village, it scores ten places up to 15 blocks off by the nearest
/// occupied point of interest of `#village` it has not visited within 10 of each,
/// takes that point of the best place, and paths to it (or, where no path goes, to a
/// place drawn on the way), stopping first at a wooden door on its path. No draw of its
/// own before the places'.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MoveThroughVillageGoal {
    pub(crate) speed: f64,
    pub(crate) only_at_night: bool,
    pub(crate) distance_to_poi: i32,
}

/// Of `#village`.
fn village_poi(kind: PoiType) -> bool {
    kind.is_village()
}

impl MoveThroughVillageGoal {
    /// `getNavigation().createPath(...)`'s path, as a goal keeps it.
    fn found(ai: &Ai<'_, '_>, made: Option<Made>) -> Option<FoundPath> {
        made.and_then(|made| made.path(ai).cloned())
    }
}

impl Goal for MoveThroughVillageGoal {
    type State = MovingThroughVillage;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut MovingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        if !navigation::can_navigate_ground(ai) {
            return false;
        }
        // `updateVisited()`: the oldest let go past fifteen.
        if state.visited.len() > 15 {
            state.visited.remove(0);
        }
        if self.only_at_night && bright_outside(ai) {
            return false;
        }
        let at = ai.block_position();
        if !ai.tick.world.is_close_to_village(at, 6) {
            return false;
        }
        let visited = state.visited.clone();
        let not_visited = |pos: BlockPos| !visited.contains(&pos);
        let scorer = |ai: &Ai<'_, '_>, candidate: BlockPos| -> f64 {
            let world = ai.tick.world;
            if !world.is_village(candidate) {
                return f64::NEG_INFINITY;
            }
            find_poi(
                world,
                &village_poi,
                &not_visited,
                candidate,
                10,
                Occupancy::IsOccupied,
            )
            .map_or(f64::NEG_INFINITY, |record| -dist_sqr(record.pos, at))
        };
        let Some(place) = random_pos::land_pos_scored(ai, 15, 7, scorer) else {
            return false;
        };
        let Some(poi) = find_poi(
            ai.tick.world,
            &village_poi,
            &not_visited,
            crate::physics::containing(place),
            10,
            Occupancy::IsOccupied,
        ) else {
            return false;
        };
        let poi = poi.pos;
        state.poi = Some(poi);
        let made = navigation::create_path(ai, poi, 0);
        let mut path = Self::found(ai, made);
        if path.is_none() {
            let Some(towards) =
                random_pos::default_pos_towards(ai, 10, 7, bottom_centre(poi), HALF_PI)
            else {
                return false;
            };
            let made = navigation::create_path_at(ai, towards, 0);
            path = Self::found(ai, made);
            if path.is_none() {
                return false;
            }
        }
        // The first wooden door on the way, in the cell above a node: a path to that node.
        let nodes = path
            .as_ref()
            .map(|found| found.nodes.clone())
            .unwrap_or_default();
        for node in nodes {
            let above = BlockPos::new(node.x, node.y + 1, node.z);
            if is_wooden_door(ai.tick.world, above) {
                let at = [f64::from(node.x), f64::from(node.y), f64::from(node.z)];
                let made = navigation::create_path_at(ai, at, 0);
                path = Self::found(ai, made);
                break;
            }
        }
        state.path = path;
        state.path.is_some()
    }

    fn can_continue(&self, state: &mut MovingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        if navigation::is_done(ai) {
            return false;
        }
        let Some(poi) = state.poi else {
            return false;
        };
        let reach = f64::from(ai.width() + self.distance_to_poi as f32);
        !closer_to_centre_than(poi, ai.position(), reach)
    }

    fn start(&self, state: &mut MovingThroughVillage, ai: &mut Ai<'_, '_>) {
        let made = state.path.take().map(Made::New);
        navigation::move_to_path(ai, made, self.speed);
    }

    /// A point reached, or whose path is done, is visited.
    fn stop(&self, state: &mut MovingThroughVillage, ai: &mut Ai<'_, '_>) {
        let Some(poi) = state.poi else {
            return;
        };
        if navigation::is_done(ai)
            || closer_to_centre_than(poi, ai.position(), f64::from(self.distance_to_poi))
        {
            state.visited.push(poi);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_cube_goes_x_fastest_then_y_then_z() {
        let cube = section_cube([0, 0, 0], 1);
        assert_eq!(cube.len(), 27);
        assert_eq!(cube[0], [-1, -1, -1]);
        assert_eq!(cube[1], [0, -1, -1]);
        assert_eq!(cube[3], [-1, 0, -1]);
        assert_eq!(cube[9], [-1, -1, 0]);
        assert_eq!(cube[26], [1, 1, 1]);
    }

    #[test]
    fn a_block_s_middle_is_nearer_than_a_distance_only_strictly() {
        let pos = BlockPos::new(0, 0, 0);
        assert!(closer_to_centre_than(pos, [0.5, 0.5, 10.4], 10.0));
        assert!(!closer_to_centre_than(pos, [0.5, 0.5, 10.5], 10.0));
    }
}
