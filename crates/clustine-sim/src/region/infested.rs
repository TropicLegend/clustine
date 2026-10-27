//! Infested blocks (`InfestedBlock`; `docs/groundwork/n9-plan.md`, section 8.4; n9
//! InfestedBlock): stone, cobblestone, the four stone bricks and deepslate with a
//! silverfish in them, which comes out when the block is broken by a rule or an
//! explosion (`spawnAfterBreak`, `block_drops` being on and no tool's silk touch), and
//! which a silverfish merges into (`mobs/types/silverfish.rs`) and wakes.

use clustine_data::{BlockState, blocks, entity_types};

use crate::mobs::types::silverfish::is_infested;
use clustine_world::BlockPos;

use super::{Random, Region};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;

impl Region {
    /// `InfestedBlock.spawnAfterBreak`: what comes of the block `state` broken at `pos`
    /// (by a rule, after its drops, or by an explosion, before them): a silverfish where
    /// it is an infested block (`block_drops` is on, and no tool here has silk touch), at
    /// the bottom middle of the block facing 0, with no `finalizeSpawn`, and its
    /// `spawnAnim` (entity event 20).
    pub(crate) fn infested_broken(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_infested(state) {
            return;
        }
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let kind = entity_types::SILVERFISH as u16;
        let mut prepare = |_: &mut Region, _: &mut Mob, _: &mut Random| {};
        let Some(key) = self.make_mob(kind, at, (0.0, 0.0), &mut prepare, output) else {
            return;
        };
        if let Some((chunk, _)) = self.find_entity(key) {
            output.events.push(RegionEvent::EntityEvent {
                entity: key,
                chunk,
                event: 20,
            });
        }
    }

    /// `level.destroyBlock(pos, true, silverfish)` of the infested block a silverfish
    /// wakes (`Local::BreakBlock`): broken with its drops, which make its silverfish, where
    /// the region holds it. **Named**: in another region's land it gives way to air only
    /// if it is still what the silverfish found (`SetIf`), with no silverfish, and the
    /// break's game event names no breaker anywhere.
    pub(crate) fn mob_breaks_block(&mut self, pos: BlockPos, output: &mut TickOutput) {
        if self.own_block(pos).is_some() {
            self.destroy_block(pos, true, super::updates::UPDATE_LIMIT, output);
        } else {
            self.set_block_updated(
                pos,
                blocks::AIR,
                super::updates::UPDATE_ALL,
                super::updates::UPDATE_LIMIT,
                output,
            );
        }
    }
}
