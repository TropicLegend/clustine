//! Fireballs, large and small (`AbstractHurtingProjectile`, `Fireball`, `LargeFireball`,
//! `SmallFireball`): family E2 of P12 (`docs/groundwork/p-plan.md`, sections 18.3, 18.5
//! and 18.6) fills each hook here: the flight and its acceleration, a fireball handed
//! across a line as a thrown item is and discarded only where its owner is the region's
//! own and gone or where it reaches land nobody holds, its deflection by a player's hit
//! (here or across a line, by the look and the UUID the attack carries, its velocity
//! sent at once), its impact (the large one's explosion with fire by `mob_griefing`, the
//! small one's fire and damage; TNT and campfires lit), and a mob's shot through `Local`.
//!
//! Until E2 builds them, a fireball does nothing and nothing makes one.

use clustine_world::{ChunkPos, Entity, EntityId, EntityUse};

use super::Region;
use super::thrown::Hit;
use crate::api::{TickOutput, Tracked};

impl Region {
    /// A fireball's tick (`AbstractHurtingProjectile.tick`).
    pub(crate) fn tick_fireball(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = (entity, position, output);
    }

    /// A player's hit on or use of a fireball, here or from across a line: a hit
    /// deflects it (`Player.attack`'s `REDIRECTABLE_PROJECTILE` branch).
    pub(crate) fn fireball_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = (entity, position, by, output);
    }

    /// What an explosion does to a fireball (`AbstractHurtingProjectile.hurtServer`):
    /// the flags of its report; until E2, its motion sent as a projectile's.
    pub(crate) fn fireball_blasted(&mut self, entity: &Entity, output: &mut TickOutput) -> u8 {
        let _ = (entity, output);
        Tracked::SYNC_VELOCITY
    }

    /// A fireball of the entity type `kind` that the mob `owner` shoots from `at` along
    /// `direction` (`Local::ShootFireball`): made and added by section 4.1's rule. Until
    /// E2, nothing is made.
    pub(crate) fn shoot_fireball(
        &mut self,
        owner: EntityId,
        kind: u16,
        at: crate::physics::Vec3,
        direction: crate::physics::Vec3,
        explosion_power: u8,
        output: &mut TickOutput,
    ) {
        let _ = (owner, kind, at, direction, explosion_power, output);
    }

    /// A thrown item's or another projectile's hit on a fireball.
    pub(crate) fn fireball_hit(&mut self, entity: &Entity, hit: Hit, output: &mut TickOutput) {
        let _ = (entity, hit, output);
    }
}
