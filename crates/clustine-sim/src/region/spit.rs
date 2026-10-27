//! The llama's spit: its flight and its hit (`docs/groundwork/n9-plan.md`, section 4.4;
//! `LlamaSpit.tick`, n7 LlamaSpit 69–140, `onHitEntity` 146–189 and `onHitBlock`
//! 190–202; M9 3.8). The llama spits it (`mobs/types/llama.rs`).
//!
//! Each tick: `Projectile.tick` (its first tick's `projectile_shoot`, whether it has
//! left its owner, the fluids it is in, the floor of the world); the hit test along its
//! motion and the hit; its turn towards its motion; gone where its box holds no air or
//! it is in water; else its motion kept by 0.99, 0.06 down, and it moves by the motion
//! it had. Its hit on an entity hurts it by `spit(spit, owner)` 1 where its owner is a
//! living entity the region has, and it flies on; on a block it is gone. No draws but
//! its making's.
//!
//! Named differences: the blocks of its box are those from the floor of its least
//! corner to the floor of its greatest (`Level.findBlocksIn` is not dumped:
//! **Inferred**); a player is creative and refuses its hurt.

use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{ChunkPos, Double, Entity, EntityData, Float, HurtBy, projectiles::Spit};

use super::Region;
use super::items::View;
use super::projectiles::TargetKind;
use super::thrown::{Flying, Struck};
use super::vibrations::{Context, Source, Vibration};
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;
use crate::physics::thrown;
use crate::physics::{self, World, fluids};

/// `LlamaSpit.getDefaultGravity()` and `getAirDrag()`.
const GRAVITY: f64 = 0.06;
const AIR_DRAG: f32 = 0.99;

impl Region {
    /// The tick of the spit `entity` of the loaded chunk `position` (`LlamaSpit.tick`); it
    /// commits through `write_projectile`.
    pub(crate) fn tick_spit(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Spit(spit) = entity.data else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(entity_types::LLAMA_SPIT);
        let owner_root = spit.owner.and_then(|owner| self.unit_root(owner));
        // `Projectile.tick`.
        if !spit.has_been_shot {
            self.shot_vibration(spit.owner, owner_root, entity.position);
        }
        let left_owner = spit.left_owner
            || self.has_left_owner(key, owner_root, entity.position, info, entity.motion);
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        let view = View {
            region: self,
            home: position,
        };
        let in_water = fluids::update(&view, &bounds).in_water();
        if self.below_the_world(position, entity.position) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let motion = entity.motion;
        let flying = Flying {
            key,
            width: info.width,
            height: info.height,
            owner_root,
            left_owner,
            tick_count: entity.tick_count().saturating_add(1),
            fall_distance: entity.fall_distance,
        };
        let spit = Spit {
            left_owner,
            has_been_shot: true,
            ..spit
        };
        if let Some(struck) = self.hit_on_move(&flying, position, entity.position, motion) {
            let now = Entity {
                data: EntityData::Spit(spit),
                ..entity.clone()
            };
            if self.spit_struck(&now, struck, output) {
                self.remove_entity(key, entity.hops, position, output);
                return;
            }
        }
        let to = physics::add(entity.position, motion);
        let [yaw, pitch] = thrown::update_rotation(entity.yaw, entity.pitch, motion);
        // `findBlocksIn(box).filterState(isAir).noneMatched()`: no air in its box.
        let view = View {
            region: self,
            home: position,
        };
        if !has_air(&view, &bounds) || in_water {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let drag = physics::scale(motion, f64::from(AIR_DRAG));
        let after = Entity {
            position: to,
            motion: [drag[0], drag[1] - GRAVITY, drag[2]],
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            data: EntityData::Spit(spit),
            ..entity.clone()
        };
        self.write_projectile(&entity, after, position, output);
    }

    /// `hitTargetOrDeflectSelf` of the spit `spit` at what it struck: an entity hurt by
    /// `spit(spit, owner)` 1 where its owner is a living entity the region has; a block's
    /// hook, and the spit gone. Whether it is gone.
    fn spit_struck(&mut self, spit: &Entity, struck: Struck, output: &mut TickOutput) -> bool {
        let key = spit.key;
        let EntityData::Spit(Spit { owner, .. }) = spit.data else {
            return false;
        };
        match struck {
            Struck::Entity { target, kind, at } => {
                if let Some(owner) = owner.and_then(|owner| self.causer_of(owner))
                    && kind != TargetKind::Face
                {
                    let hurt = HurtBy {
                        damage_type: "minecraft:spit".to_owned(),
                        cause: Some(owner),
                        direct: Some(clustine_world::Causer {
                            key,
                            kind: spit.kind,
                            position: spit.position.map(Double),
                            player: None,
                            living: false,
                        }),
                        position: None,
                        projectile_motion: Some(spit.motion.map(Double)),
                        amount: Float(1.0),
                        knockback: None,
                        fire_before: 0,
                        fire_after: 0,
                        effects: Vec::new(),
                        arrow: false,
                        extras: Vec::new(),
                        sound: None,
                        hit: None,
                    };
                    self.hurt_target(target, kind, hurt, output);
                }
                self.vibrate(
                    Vibration::PROJECTILE_LAND,
                    at,
                    Context::of(Some(Source::of(key)), None),
                );
                false
            }
            Struck::Block { hit, .. } => {
                let state = self.block(hit.pos);
                if let Some(state) = state {
                    self.projectile_hit(spit, state, &hit, output);
                }
                self.vibrate_at(
                    Vibration::PROJECTILE_LAND,
                    hit.pos,
                    Context::of(Some(Source::of(key)), state),
                );
                true
            }
        }
    }
}

/// Whether any block of `bounds` is air.
fn has_air<W: World + ?Sized>(world: &W, bounds: &Aabb) -> bool {
    let low = physics::containing(bounds.min);
    let high = physics::containing(bounds.max);
    for x in low.x..=high.x {
        for y in low.y..=high.y {
            for z in low.z..=high.z {
                let state = world.state(clustine_world::BlockPos::new(x, y, z));
                if state.props().is_air() {
                    return true;
                }
            }
        }
    }
    false
}
