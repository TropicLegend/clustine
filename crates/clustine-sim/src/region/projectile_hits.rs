//! What a projectile does to the block it hits (`docs/groundwork/n9-plan.md`, section
//! 4.2, step 4; part S2's): the blocks' `onProjectileHit` (a target block's signal, a
//! bell's ring, TNT lit by a burning projectile), in another region's land a `SetIf` or
//! the block's effect as ADR-0025 has them; and an arrow's part in a button's
//! `checkPressed`, from the button's own tick and from an arrow's box inside it
//! (`ButtonBlock.entityInside`).
//!
//! The lead fixed the signatures in N9b: every projectile's tick calls
//! [`Region::projectile_hit`] at a block hit, before its own `onHitBlock`, and the button's
//! tick in `redstone.rs` asks [`Region::arrow_on_button`].
//!
//! A hit's side (`BlockHitResult.getDirection`) is not in the hit the clip hands on
//! (`physics::thrown::BlockHit`): it is worked out again from where the hit lies on the
//! block's shape and which way the projectile moved, as `AABB.clip` finds it (x, then y,
//! then z), and for a hit from inside the shape as the side facing back along the
//! motion (`Direction.getApproximateNearest(delta).getOpposite()`).
//!
//! Named differences:
//! - **Inferred**: `TargetBlock` is not in the dumps. Its hit is taken as the game's
//!   known code has it: a power of `max(1, ceil(15 · clamp((0.5 − d) / 0.5, 0, 1)))`, `d`
//!   the farther of the hit's two distances from the middle of its side, set with every
//!   update and looked at again 20 ticks on for an arrow or a trident, 8 for anything
//!   else, unless a look is due already (the target's tick, `diodes.rs`, lets it fall
//!   back). In another region's land the set and the look go as effects and the look
//!   due there is not known: a second hit within the first's time sets it again.
//! - `ChorusFlowerBlock`, `PointedDripstoneBlock` and `DecoratedPotBlock` are not in the
//!   dumps (the brief names them as dumped): a projectile breaks none of them yet.
//! - A bell in another region's land is not rung: no effect carries a ring.
//! - `mayInteract` is true: no player of Clustine's is kept from building, and mob
//!   griefing is on.

use clustine_blocks::data::{flag, is_class, relative, set_flag, tick_id};
use clustine_blocks::signal;
use clustine_data::{BlockClass, BlockState, Face, entity_types, sounds};
use clustine_world::{BlockPos, Entity, EntityData, EntityId, Ticked};

use super::Region;
use super::updates::{UPDATE_ALL, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use crate::api::TickOutput;
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;
use crate::physics::thrown::BlockHit;

/// How long a wooden button stays pressed (`BlockSetType`'s, 30 for wood).
const WOODEN_BUTTON_DELAY: i32 = 30;

/// **Inferred**: how long a target block holds its power: 20 ticks for an arrow or a
/// trident, 8 for anything else.
const TARGET_BY_ARROW: i32 = 20;
const TARGET_BY_OTHER: i32 = 8;

/// `BellBlock.isProperHit`'s highest hit on a side.
const BELL_TOP: f64 = 0.812_399_983_406_066_9;

/// Whether the entity type `kind` is an `AbstractArrow`: an arrow, a spectral arrow or
/// a trident.
pub(crate) fn is_arrow_kind(kind: u16) -> bool {
    let kind = i32::from(kind);
    kind == entity_types::ARROW
        || kind == entity_types::SPECTRAL_ARROW
        || kind == entity_types::TRIDENT
}

/// The owner of a projectile's record, by key.
fn owner_of(projectile: &Entity) -> Option<EntityId> {
    match &projectile.data {
        EntityData::Arrow(arrow) => arrow.owner,
        EntityData::Thrown { owner, .. } => *owner,
        EntityData::Spit(spit) => spit.owner,
        EntityData::Firework(firework) => firework.owner,
        EntityData::Fireball(fireball) => fireball.owner.map(|owner| owner.key),
        _ => None,
    }
}

/// Whether `state` is a button an arrow presses (`canButtonBeActivatedByArrows`): a
/// wooden one, not stone's or polished blackstone's.
fn arrows_press(state: BlockState) -> bool {
    is_class(state, BlockClass::ButtonBlock)
        && !state
            .block()
            .is_some_and(|block| block.name.contains("stone"))
}

/// A wooden button's click as it is pressed or let go, by its wood (`BlockSetType`).
fn wooden_button_sound(state: BlockState, on: bool) -> i32 {
    use sounds as s;
    let name = state.block().map_or("", |block| block.name);
    let (on_sound, off_sound) = if name.contains("cherry") {
        (
            s::BLOCK_CHERRY_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_CHERRY_WOOD_BUTTON_CLICK_OFF,
        )
    } else if name.contains("bamboo") {
        (
            s::BLOCK_BAMBOO_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_BAMBOO_WOOD_BUTTON_CLICK_OFF,
        )
    } else if name.contains("crimson") || name.contains("warped") {
        (
            s::BLOCK_NETHER_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_NETHER_WOOD_BUTTON_CLICK_OFF,
        )
    } else {
        (
            s::BLOCK_WOODEN_BUTTON_CLICK_ON,
            s::BLOCK_WOODEN_BUTTON_CLICK_OFF,
        )
    };
    if on { on_sound } else { off_sound }
}

/// The bounds of the outline shape of `state` at `pos` (`getShape(…).bounds()`), the
/// block's own cell where it has none.
fn outline_bounds(state: BlockState, pos: BlockPos) -> Aabb {
    let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    let mut bounds: Option<Aabb> = None;
    for [x0, y0, z0, x1, y1, z1] in state.props().outline_shape().boxes() {
        let placed = Aabb::new(
            [at[0] + x0, at[1] + y0, at[2] + z0],
            [at[0] + x1, at[1] + y1, at[2] + z1],
        );
        bounds = Some(bounds.map_or(placed, |so_far| so_far.minmax(placed)));
    }
    bounds.unwrap_or_else(|| Aabb::of_block(pos))
}

/// `Direction.getApproximateNearest(x, y, z)`: the side the vector points most along,
/// in the order of the directions where two tie.
fn nearest_face(v: Vec3) -> Face {
    let mut best = Face::North;
    let mut most = f64::MIN;
    for face in Face::ALL {
        let normal = face_normal(face);
        let along = v[0] * normal[0] + v[1] * normal[1] + v[2] * normal[2];
        if along > most {
            most = along;
            best = face;
        }
    }
    best
}

fn face_normal(face: Face) -> Vec3 {
    match face {
        Face::Down => [0.0, -1.0, 0.0],
        Face::Up => [0.0, 1.0, 0.0],
        Face::North => [0.0, 0.0, -1.0],
        Face::South => [0.0, 0.0, 1.0],
        Face::West => [-1.0, 0.0, 0.0],
        Face::East => [1.0, 0.0, 0.0],
    }
}

/// The side of the block at `pos`, of `state`, that the hit at `location` of a
/// projectile moving by `motion` came in through (the module's note).
pub(crate) fn hit_face(state: BlockState, pos: BlockPos, location: Vec3, motion: Vec3) -> Face {
    const NEAR: f64 = 1.0e-7;
    let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    let faces = [
        (Face::West, Face::East),
        (Face::Down, Face::Up),
        (Face::North, Face::South),
    ];
    for axis in 0..3 {
        for [x0, y0, z0, x1, y1, z1] in state.props().collision_shape().boxes() {
            let (low, high) = ([x0, y0, z0][axis], [x1, y1, z1][axis]);
            let (near_low, near_high) = faces[axis];
            if motion[axis] > NEAR && (location[axis] - (at[axis] + low)).abs() < NEAR {
                return near_low;
            }
            if motion[axis] < -NEAR && (location[axis] - (at[axis] + high)).abs() < NEAR {
                return near_high;
            }
        }
    }
    nearest_face(motion).opposite()
}

/// `getRedstoneStrength`, **Inferred**: the power a hit at `location` on the side `face`
/// gives a target block.
pub(crate) fn target_strength(location: Vec3, face: Face) -> u8 {
    let from_middle = |value: f64| (value - value.floor() - 0.5).abs();
    let [dx, dy, dz] = location.map(from_middle);
    let distance = match face {
        Face::Down | Face::Up => dx.max(dz),
        Face::North | Face::South => dx.max(dy),
        Face::West | Face::East => dy.max(dz),
    };
    let share = ((0.5 - distance) / 0.5).clamp(0.0, 1.0);
    (15.0 * share).ceil().max(1.0) as u8
}

/// `BellBlock.isProperHit` of a hit on the side `face` at `height` up the bell's block.
fn bell_hit_properly(state: BlockState, face: Face, height: f64) -> bool {
    let horizontal = !matches!(face, Face::Up | Face::Down);
    if !horizontal || height > BELL_TOP {
        return false;
    }
    let Some(facing) = clustine_blocks::data::facing(state) else {
        return false;
    };
    let same_axis = |a: Face, b: Face| a == b || a == b.opposite();
    match state.property("attachment") {
        Some("floor") => same_axis(facing, face),
        Some("single_wall" | "double_wall") => !same_axis(facing, face),
        Some("ceiling") => true,
        _ => false,
    }
}

impl Region {
    /// `state.onProjectileHit(level, state, hit, projectile)` of the block `hit.pos`, in
    /// state `state`, hit by `projectile` as it is after the step that hit (its kind,
    /// owner, fire and motion are read from it).
    pub(crate) fn projectile_hit(
        &mut self,
        projectile: &Entity,
        state: BlockState,
        hit: &BlockHit,
        output: &mut TickOutput,
    ) {
        let pos = hit.pos;
        if is_class(state, BlockClass::TargetBlock) {
            self.target_hit(projectile, state, hit, output);
        } else if is_class(state, BlockClass::BellBlock) {
            let face = hit_face(state, pos, hit.location, projectile.motion);
            let height = hit.location[1] - f64::from(pos.y);
            if bell_hit_properly(state, face, height) && self.ring_bell(pos, Some(face), output) {
                // `attemptToRing`'s `block_change`, of the owner where it is a player.
                let player =
                    owner_of(projectile).filter(|owner| self.player_with_entity(*owner).is_some());
                self.vibrate_at(
                    Vibration::BLOCK_CHANGE,
                    pos,
                    Context::of(player.map(Source::of), None),
                );
            }
        } else if is_class(state, BlockClass::TntBlock) && projectile.fire_ticks > 0 {
            // `TntBlock.onProjectileHit`: a burning projectile primes it, owned by its
            // owner where that is living, and the block goes.
            if !self.chunks.contains_key(&pos.chunk()) {
                self.tnt_burnt_across(pos, state);
                return;
            }
            let owner = owner_of(projectile).and_then(|owner| {
                if let Some(id) = self.player_with_entity(owner) {
                    return Some(id.0);
                }
                self.find_entity(owner)
                    .filter(|(_, record)| matches!(record.data, EntityData::Mob { .. }))
                    .map(|(_, record)| record.uuid)
            });
            if self.prime_tnt(pos, owner, output) {
                self.set_block(pos, clustine_data::blocks::AIR, output);
            }
        }
    }

    /// **Inferred** `TargetBlock.onProjectileHit` (the module's note).
    fn target_hit(
        &mut self,
        projectile: &Entity,
        state: BlockState,
        hit: &BlockHit,
        output: &mut TickOutput,
    ) {
        let face = hit_face(state, hit.pos, hit.location, projectile.motion);
        let power = target_strength(hit.location, face);
        let delay = if is_arrow_kind(projectile.kind) {
            TARGET_BY_ARROW
        } else {
            TARGET_BY_OTHER
        };
        let look = Ticked::Block(tick_id(state));
        if self.chunks.contains_key(&hit.pos.chunk()) && self.has_scheduled_tick(hit.pos, look) {
            return;
        }
        let Some(powered) = state.with_property("power", &power.to_string()) else {
            return;
        };
        self.set_block_updated(hit.pos, powered, UPDATE_ALL, UPDATE_LIMIT, output);
        self.schedule_tick(hit.pos, look, delay, 0);
    }

    /// `ButtonBlock.checkPressed`'s look for arrows: whether an arrow's box meets the
    /// button at `pos`, in state `state`, so that it stays pressed (a wooden button
    /// only).
    pub(crate) fn arrow_on_button(&self, pos: BlockPos, state: BlockState) -> bool {
        arrows_press(state) && self.arrows_meet(&outline_bounds(state, pos), None)
    }

    /// Whether an arrow of the region's loaded or copied chunks has a box that meets
    /// `bounds` (`getEntitiesOfClass(AbstractArrow, bounds)`), `ticking` being the arrow
    /// whose tick asks, by its key and its box where its tick has it.
    fn arrows_meet(&self, bounds: &Aabb, ticking: Option<(EntityId, Aabb)>) -> bool {
        if let Some((_, own)) = ticking
            && own.intersects(*bounds)
        {
            return true;
        }
        let except = ticking.map(|(key, _)| key);
        let low = clustine_world::ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = clustine_world::ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(clustine_world::ChunkPos::new(x, z)) else {
                    continue;
                };
                let found = chunk.entities().iter().any(|entity| {
                    Some(entity.key) != except
                        && is_arrow_kind(entity.kind)
                        && self.entity_box(entity).intersects(*bounds)
                });
                if found {
                    return true;
                }
            }
        }
        false
    }

    /// `ButtonBlock.entityInside` for the arrow `arrow` (its key and its box where its
    /// tick has it), whose walk met the button `state` at `pos`: an unpowered wooden
    /// button looks for arrows (`checkPressed`) and, finding one, is pressed, tells the
    /// blocks around it and the block it hangs on, clicks, and looks again in 30 ticks.
    pub(crate) fn arrow_inside_button(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        arrow: (EntityId, Aabb),
        output: &mut TickOutput,
    ) {
        if !arrows_press(state) || flag(state, "powered") {
            return;
        }
        if !self.arrows_meet(&outline_bounds(state, pos), Some(arrow)) {
            return;
        }
        let pressed = set_flag(state, "powered", true);
        self.set_block(pos, pressed, output);
        // `ButtonBlock.updateNeighbours`: around the button, then around what it hangs
        // on.
        self.update_neighbours_at(pos, pressed, output);
        if let Some(towards) = signal::attached_towards(pressed) {
            self.update_neighbours_at(relative(pos, towards.opposite()), pressed, output);
        }
        super::redstone::play(
            pos,
            wooden_button_sound(state, true),
            1.0,
            1.0,
            None,
            output,
        );
        self.vibrate_at(
            Vibration::BLOCK_ACTIVATE,
            pos,
            Context::of(Some(Source::of(arrow.0)), None),
        );
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), WOODEN_BUTTON_DELAY, 0);
    }
}

/// The box of an entity of `kind` at `at` (`EntityDimensions` of its type).
pub(crate) fn box_of(kind: u16, at: Vec3) -> Aabb {
    let info = clustine_data::EntityTypeInfo::of(i32::from(kind));
    Aabb::of_entity(at, info.width, info.height)
}
