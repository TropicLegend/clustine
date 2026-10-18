//! How water and lava take an entity: how deep it is in each and the current that
//! pushes it (`EntityFluidInteraction`, `docs/groundwork/n4-brief.md`, section 4).
//!
//! The game keeps the trackers on the entity between ticks, but each update starts
//! them again from nothing, so what an update finds is a function of the blocks and
//! the box: nothing here is kept.

use clustine_blocks::fluid::{self, FluidState, Kind};
use clustine_world::{BlockPos, ChunkPos};

use super::aabb::Aabb;
use super::{Rules, Vec3, World};

/// How strongly water pushes (`Entity.updateFluidInteraction`).
const WATER_CURRENT: f64 = 0.014;
/// And lava, where it is not fast (the overworld's).
const LAVA_CURRENT: f64 = 0.002_333_333_333_333_333_5;

/// A current summed over the cells of one kind of fluid (`CurrentAccumulator`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Current {
    height: f64,
    current: Vec3,
    count: u32,
}

/// What one update found.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct InFluids {
    /// Whether any fluid reaches the box (the update's answer).
    pub(crate) found: bool,
    /// How deep the box is in water and in lava (`getFluidHeight`).
    pub(crate) water_height: f64,
    pub(crate) lava_height: f64,
    water: Current,
    lava: Current,
}

impl InFluids {
    pub(crate) fn in_water(&self) -> bool {
        self.water_height > 0.0
    }

    pub(crate) fn in_lava(&self) -> bool {
        self.lava_height > 0.0
    }
}

/// `EntityFluidInteraction.update(entity, false)` for the entity whose box is `bounds`.
pub(crate) fn update<W: World + ?Sized>(world: &W, bounds: &Aabb) -> InFluids {
    let mut found = InFluids::default();
    let inner = bounds.deflate(0.001);
    let x0 = super::floor(inner.min[0]);
    let y0 = super::floor(inner.min[1]);
    let z0 = super::floor(inner.min[2]);
    let x1 = inner.max[0].ceil() as i32 - 1;
    let y1 = inner.max[1].ceil() as i32 - 1;
    let z1 = inner.max[2].ceil() as i32 - 1;
    // `hasFluidAndLoaded`: every chunk around the cells must be there.
    for cx in (x0 - 1) >> 4..=(x1 + 1) >> 4 {
        for cz in (z0 - 1) >> 4..=(z1 + 1) >> 4 {
            if !world.has_chunk(ChunkPos::new(cx, cz)) {
                return found;
            }
        }
    }
    let min_y = bounds.min[1];
    // The trackers, one per fluid (still and flowing water are two), in the order made.
    let mut trackers: Vec<(u8, f64)> = Vec::new();
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                let pos = BlockPos::new(x, y, z);
                let state = world.state(pos);
                let held = FluidState::of(state);
                let Some(kind) = held.kind() else {
                    continue;
                };
                let top = f64::from(y) + f64::from(fluid::height(&mut Rules(world), held, pos));
                if top < inner.min[1] {
                    continue;
                }
                found.found = true;
                let at = match trackers.iter().position(|(id, _)| *id == held.fluid) {
                    Some(at) => at,
                    None => {
                        trackers.push((held.fluid, 0.0));
                        trackers.len() - 1
                    }
                };
                let height = (top - min_y).max(trackers[at].1);
                trackers[at].1 = height;
                let current = match kind {
                    Kind::Water => &mut found.water,
                    Kind::Lava => &mut found.lava,
                };
                let mut flow = fluid::flow(&mut Rules(world), held, pos);
                current.height = height.max(current.height);
                if current.height < 0.4 {
                    flow = super::scale(flow, current.height);
                }
                current.current = super::add(current.current, flow);
                current.count += 1;
            }
        }
    }
    for (id, height) in trackers {
        let deepest = match Kind::of(id) {
            Some(Kind::Water) => &mut found.water_height,
            Some(Kind::Lava) => &mut found.lava_height,
            None => continue,
        };
        if height > *deepest {
            *deepest = height;
        }
    }
    found
}

/// `CurrentAccumulator.applyTo(entity, scale)` for an entity that is not a player: the
/// current made one long and scaled, and at least a little where the entity barely
/// moves.
fn apply(current: &Current, motion: Vec3, scale: f64) -> Vec3 {
    if current.count == 0 || super::length_sqr(current.current) < f64::from(1.0e-5_f32) {
        return motion;
    }
    let mut push = super::scale(super::normalize(current.current), scale);
    if motion[0].abs() < 0.003
        && motion[2].abs() < 0.003
        && super::length_sqr(push).sqrt() < 0.004_500_000_000_000_000_5
    {
        push = super::scale(super::normalize(push), 0.004_500_000_000_000_000_5);
    }
    super::add(motion, push)
}

/// The rest of `Entity.updateFluidInteraction` for an entity that currents push: the
/// push of each fluid it is in, water first.
pub(crate) fn push(found: &InFluids, motion: Vec3) -> Vec3 {
    let mut motion = motion;
    if found.in_water() {
        motion = apply(&found.water, motion, WATER_CURRENT);
    }
    if found.in_lava() {
        motion = apply(&found.lava, motion, LAVA_CURRENT);
    }
    motion
}
