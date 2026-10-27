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
pub(crate) fn is_suffocating(state: clustine_data::BlockState) -> bool {
    state.props().collision_is_full_block()
}

impl Region {
    /// `CushionItem.useOn` (`docs/groundwork/n5-brief.md`, section 5.8): a cushion of the
    /// held item's colour laid on the clicked top, in the clicked block if it gives way
    /// and above it if not, at the click's height, facing the player's way snapped to a
    /// side; refused where it would not rest, where it would be buried or where another
    /// cushion is. A creative player keeps the item. Whether it took the click.
    ///
    /// Not built: the click taken again on the collision shape of a cauldron, a hopper
    /// or a composter (`#cushion_uses_collision_shape`), whose outline the click is on.
    pub(crate) fn place_cushion(
        &mut self,
        click: &crate::placement::Click,
        player: clustine_world::PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let _ = player;
        let Some(item) = click.held else {
            return false;
        };
        let first = clustine_data::items::WHITE_CUSHION;
        if !(first..=clustine_data::items::BLACK_CUSHION).contains(&item) {
            return false;
        }
        if click.face != crate::api::Face::Top {
            return false;
        }
        let Some(clicked) = self.own_block(click.position) else {
            return false;
        };
        let (target, _) = crate::placement::target_of(click, clicked, &super::Readable(self));
        let at = [
            f64::from(target.x) + 0.5,
            f64::from(click.position.y) + f64::from(click.cursor[1]),
            f64::from(target.z) + 0.5,
        ];
        let chunk = ChunkPos::containing(at[0], at[2]);
        if !self.ticks_chunk(chunk) {
            return false;
        }
        let bounds = Aabb::of_entity(at, 1.0, 0.25);
        let view = View {
            region: self,
            home: chunk,
        };
        if !has_anchor_below(&view, &bounds)
            || covered_by_suffocating(&view, &bounds)
            || anchor_buried(&view, &bounds)
        {
            return false;
        }
        let cushion = clustine_data::entity_types::CUSHION as u16;
        let taken = self.colliders_in(&bounds).into_iter().any(|found| {
            self.find_entity(found.key)
                .is_some_and(|(_, entity)| entity.kind == cushion)
        });
        if taken {
            return false;
        }
        let Some(key) = self.take_entity_key(chunk) else {
            return false;
        };
        let mut own = self.entity_stream(key).random;
        let uuid = super::entities::insecure_uuid(own.next_u64(), own.next_u64());
        // `EntityType.create` draws the level's random for a yaw that `snapTo` replaces.
        let yaw = match super::riding::direction_from_yaw(click.yaw) {
            clustine_data::Face::South => 0.0,
            clustine_data::Face::West => 90.0,
            clustine_data::Face::North => 180.0,
            _ => 270.0,
        };
        let colour = u8::try_from(item - first).expect("sixteen colours");
        let entity = Entity {
            key,
            kind: clustine_data::entity_types::CUSHION as u16,
            uuid,
            position: at,
            motion: [0.0; 3],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Cushion {
                attached: clustine_world::Attached {
                    block: physics::containing(at),
                    direction: 0,
                },
                colour,
            },
        };
        self.add_fresh_entity(entity, output);
        true
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
        self.spawn_item(entity.position, ItemStack::new(item, 1), output);
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
        match by {
            EntityUse::Attack { creative, .. } => {
                self.hurt_attached(entity.key, Breaker::Player { creative }, output);
            }
            // `Cushion.interact`: sat on, if nobody sits there.
            EntityUse::Interact { player, .. } => {
                self.mount_player(player, entity.key, output);
            }
            // An explosion's hit across a line is step N6g's (`docs/groundwork/n6-plan.md`).
            // Only animals breed.
            EntityUse::Blasted { .. }
            | EntityUse::BreedAsk { .. }
            | EntityUse::MobHit { .. }
            | EntityUse::Thrown { .. }
            | EntityUse::SeatDeclined { .. }
            | EntityUse::BreedAnswer { .. } => {}
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

/// `Cushion.isAnchorBuried(level, box)`: one block's collision covers the whole slab a
/// sixty-fourth thick at the box's floor. **Inferred**: `Shapes.join` is taken as a box
/// of the shape that holds the slab.
fn anchor_buried<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let slab = next_deflated(Aabb::new(
        bounds.min,
        [bounds.max[0], bounds.min[1] + 0.015_625, bounds.max[2]],
    ));
    let low = physics::containing(slab.min);
    let high = physics::containing(slab.max);
    for x in low.x..=high.x {
        for y in low.y..=high.y {
            for z in low.z..=high.z {
                let pos = BlockPos::new(x, y, z);
                let origin = [f64::from(x), f64::from(y), f64::from(z)];
                let covers = world
                    .state(pos)
                    .props()
                    .collision_shape()
                    .boxes()
                    .any(|part| {
                        (0..3).all(|axis| {
                            part[axis] + origin[axis] <= slab.min[axis]
                                && part[axis + 3] + origin[axis] >= slab.max[axis]
                        })
                    });
                if covers {
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
