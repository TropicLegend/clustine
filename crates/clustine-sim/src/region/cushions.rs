//! The cushion's own parts (`Cushion`, `docs/groundwork/n5-brief.md`, section 5.8): what
//! holds it (a block whose outline reaches up to its underside, and not every block it
//! lies in suffocating), what it drops (its item, of its colour, but to a creative
//! breaker), and a hit on it from another region's player. Its placing and its sitting
//! come with the riders' step.

use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, EntityUse, ItemStack};

use super::Region;
use super::attached::Breaker;
use super::items::View;
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;
use crate::physics::{self, World};

/// `Math.nextUp` and `Math.nextDown` of a finite double.
fn next_up(value: f64) -> f64 {
    if value == 0.0 {
        return f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits + 1 } else { bits - 1 })
}

fn next_down(value: f64) -> f64 {
    -next_up(-value)
}

/// `AABB.nextDeflated()`.
pub(crate) fn next_deflated(bounds: Aabb) -> Aabb {
    Aabb {
        min: bounds.min.map(next_up),
        max: bounds.max.map(next_down),
    }
}

/// `BlockState.isSuffocating`. **Inferred**: the game's default predicate, a block that
/// blocks motion with a full collision cube; the blocks that override it (glass, leaves,
/// soul sand, mud and the like) are not told apart, the per-block predicates being in
/// classes the dump does not have.
fn is_suffocating(state: clustine_data::BlockState) -> bool {
    state.props().collision_is_full_block()
}

impl Region {
    /// `CushionItem.useOn`: a cushion laid on the clicked top (the riders' step). Filled by its step; until then it does nothing.
    pub(crate) fn place_cushion(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut crate::api::TickOutput,
    ) -> bool {
        let _ = (click, player, output);
        false
    }

    /// `Cushion.survives()` = `wouldSurviveAt(level, box)`: something under it to rest
    /// on, and not all it lies in suffocating.
    pub(crate) fn cushion_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        let view = View {
            region: self,
            home: position,
        };
        let bounds = self.entity_box(entity);
        has_anchor_below(&view, &bounds) && !covered_by_suffocating(&view, &bounds)
    }

    /// `Cushion.tickAtCheckInterval()`: a fluid it lies in would put out or light it,
    /// and fire it lies in hurts it. **Not built**: Clustine keeps no fire on these
    /// entities yet, and a cushion placed into fire comes with the cushion's placing;
    /// a fire that reaches a cushion later leaves it whole (a named difference).
    pub(crate) fn cushion_check_interval(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = (entity, position, output);
    }

    /// `Cushion.dropItem(level, breaker)`: its item of its colour where it is, with the
    /// default pick-up delay, but nothing to a creative breaker. (Its sound and its
    /// particles are a client's; the step that places cushions sends them.)
    pub(crate) fn cushion_drop(
        &mut self,
        entity: &Entity,
        breaker: Breaker,
        output: &mut TickOutput,
    ) {
        let EntityData::Cushion { colour, .. } = entity.data else {
            return;
        };
        if breaker == (Breaker::Player { creative: true }) || colour > 15 {
            return;
        }
        let item = clustine_data::items::WHITE_CUSHION + i32::from(colour);
        self.spawn_item(entity.position, ItemStack { item, count: 1 }, output);
    }

    /// A use of a cushion by another region's player: a hit breaks it
    /// (`BlockAttachedEntity.skipAttackInteraction` → `hurtServer`); sitting on it is a
    /// mount, the riders' step's.
    pub(crate) fn cushion_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        if let EntityUse::Attack { creative, .. } = by {
            self.hurt_attached(entity.key, Breaker::Player { creative }, output);
        }
    }

    /// `Cushion.removePassenger`: the sound of getting up, while it stands.
    pub(crate) fn cushion_passenger_removed(&mut self, cushion: &Entity, output: &mut TickOutput) {
        // The sound is the riders' step's to send, with the sitting.
        let _ = (cushion, output);
    }
}

/// `Cushion.hasAnchorBelow(level, box)`: a block whose outline's bounds reach into the
/// slab from a sixty-fourth below the box's floor to an eighth below that.
fn has_anchor_below<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let probe = Aabb::new(
        [bounds.min[0], bounds.min[1] - 0.015_625, bounds.min[2]],
        [
            next_down(bounds.max[0]),
            bounds.min[1],
            next_down(bounds.max[2]),
        ],
    )
    .expand_towards([0.0, -0.125, 0.0]);
    // **Inferred**: `findBlocksIn` visits every block from the floor of the box's
    // minimum to the floor of its maximum (`BlockScanUtils` is not in the dump); the
    // order does not change the answer.
    let low = physics::containing(probe.min);
    let high = physics::containing(probe.max);
    for x in low.x..=high.x {
        for y in low.y..=high.y {
            for z in low.z..=high.z {
                let pos = BlockPos::new(x, y, z);
                let outline: Vec<[f64; 6]> =
                    world.state(pos).props().outline_shape().boxes().collect();
                let Some(first) = outline.first() else {
                    continue;
                };
                let mut shape_bounds = Aabb::new(
                    [first[0], first[1], first[2]],
                    [first[3], first[4], first[5]],
                );
                for one in &outline[1..] {
                    shape_bounds = shape_bounds.minmax(Aabb::new(
                        [one[0], one[1], one[2]],
                        [one[3], one[4], one[5]],
                    ));
                }
                let placed = shape_bounds.moved([f64::from(x), f64::from(y), f64::from(z)]);
                if placed.intersects(probe) {
                    return true;
                }
            }
        }
    }
    false
}

/// `Cushion.isCoveredBySuffocatingBlocks(level, box)`: every block in the box, a hair
/// smaller, suffocates.
fn covered_by_suffocating<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let inner = next_deflated(*bounds);
    let low = physics::containing(inner.min);
    let high = physics::containing(inner.max);
    for z in low.z.min(high.z)..=low.z.max(high.z) {
        for y in low.y.min(high.y)..=low.y.max(high.y) {
            for x in low.x.min(high.x)..=low.x.max(high.x) {
                if !is_suffocating(world.state(BlockPos::new(x, y, z))) {
                    return false;
                }
            }
        }
    }
    true
}
