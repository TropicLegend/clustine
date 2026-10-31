//! The end crystal (`EndCrystal`; `docs/groundwork/p-brief.md`, section 6.3): family E7
//! of the End's milestone (`docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6).
//! Its hooks are fixed by P10-0 so that every shared table calls them; until E7 fills
//! them a crystal is left as it is.

use clustine_world::{ChunkPos, Entity, EntityUse};

use super::Region;
use super::explosions::Explosion;
use super::thrown::Hit;
use crate::api::TickOutput;

impl Region {
    /// The tick of an end crystal (`EndCrystal.tick`): E7's.
    pub(crate) fn tick_end_crystal(
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
}
