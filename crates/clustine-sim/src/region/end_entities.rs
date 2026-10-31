//! The hooks of the End's entities that are not mobs: the thrown eye of ender, the end
//! crystal and the shulker's bullet (`docs/groundwork/p-plan.md`, sections 19.2 and
//! 19.5). Their contracts are fixed here (P10-0) so that every shared table calls them;
//! the steps that build each kind fill them (the eye P10b's, the crystal E7's, the
//! bullet E8's). Until then each is left as it is: it neither moves nor changes.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use super::explosions::Explosion;
use super::thrown::Hit;
use crate::api::TickOutput;

impl Region {
    /// The tick of a thrown eye of ender (`EyeOfEnder.tick`): P10b's.
    pub(crate) fn tick_eye_of_ender(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }

    /// The tick of an end crystal (`EndCrystal.tick`): E7's.
    pub(crate) fn tick_end_crystal(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }

    /// The tick of a shulker's bullet (`ShulkerBullet.tick`): E8's.
    pub(crate) fn tick_shulker_bullet(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }

    /// A player's or another region's use of an end crystal (`EndCrystal.hurtServer` of
    /// an attack): E7's.
    pub(crate) fn end_crystal_used(
        &mut self,
        _entity: &Entity,
        _chunk: ChunkPos,
        _by: EntityUse,
        _output: &mut TickOutput,
    ) {
    }

    /// An explosion's hurt of an end crystal, which removes it without an explosion of
    /// its own (the plan, section 19.5 item 13): E7's.
    pub(crate) fn end_crystal_blasted(
        &mut self,
        _record: &Entity,
        _explosion: &Explosion,
        _output: &mut TickOutput,
    ) {
    }

    /// A projectile's hit of an end crystal: E7's.
    pub(crate) fn end_crystal_hit(
        &mut self,
        _record: &Entity,
        _hit: Hit,
        _output: &mut TickOutput,
    ) {
    }

    /// A shulker's bullet hurt by anything, which destroys it (`ShulkerBullet.hurtServer`):
    /// E8's.
    pub(crate) fn shulker_bullet_hurt(&mut self, _record: &Entity, _output: &mut TickOutput) {}
}
