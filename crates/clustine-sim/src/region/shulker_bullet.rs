//! The shulker's bullet (`ShulkerBullet`): family E8 of the End's milestone
//! (`docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6). Its hooks are fixed by
//! P10-0 so that every shared table calls them; until E8 fills them a bullet is left as
//! it is.

use clustine_world::{ChunkPos, Entity};

use super::Region;
use crate::api::TickOutput;

impl Region {
    /// The tick of a shulker's bullet (`ShulkerBullet.tick`): E8's.
    pub(crate) fn tick_shulker_bullet(
        &mut self,
        _entity: Entity,
        _position: ChunkPos,
        _output: &mut TickOutput,
    ) {
    }

    /// A shulker's bullet hurt by anything, which destroys it (`ShulkerBullet.hurtServer`):
    /// E8's.
    pub(crate) fn shulker_bullet_hurt(&mut self, _record: &Entity, _output: &mut TickOutput) {}
}
