//! What a mob's tick reads of the world (`docs/groundwork/n7-plan.md`, section 11.3,
//! item 5): the blocks of its region's land and copies, air beyond them; the entities in
//! a box; the players; the light. Nothing else.

// What the living body's and the AI's parts (N7b, N7c) read; until they do, nothing
// but the tests reads it.
#![allow(
    dead_code,
    reason = "the read view of the parts N7b and N7c build next"
)]

use clustine_world::{BlockPos, Entity, EntityId, ItemStack};

use crate::physics::aabb::Aabb;
use crate::physics::item::Region as Holds;

/// An entity a mob sees: its record and whether it is in a copy (another region's,
/// which the mob may read and never write).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Seen<'a> {
    pub(crate) entity: &'a Entity,
    pub(crate) bounds: Aabb,
    pub(crate) copied: bool,
}

/// A player a mob sees, as far as a mob reads one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeenPlayer {
    /// Their stay's entity.
    pub(crate) key: EntityId,
    pub(crate) uuid: u128,
    /// Their feet.
    pub(crate) position: [f64; 3],
    pub(crate) eye_height: f64,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) main_hand: Option<ItemStack>,
    pub(crate) off_hand: Option<ItemStack>,
    pub(crate) sneaking: bool,
    /// Every player is a creative one, whom no mob takes as a target (the brief's
    /// 2.0.1).
    pub(crate) creative: bool,
    pub(crate) spectator: bool,
    /// What they ride, if anything.
    pub(crate) vehicle: Option<EntityId>,
}

/// A block's light, as the chunk keeps it (`getBrightness` of each layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LightAt {
    pub(crate) sky: u8,
    pub(crate) block: u8,
}

/// The world a mob's tick reads. Its blocks and its loaded chunks are the physics'
/// ([`crate::physics::World`]): what is neither land nor copy reads as `None`, which a
/// path search takes as an unloaded chunk's air.
pub(crate) trait MobWorld: Holds {
    /// Whether the block at `pos` is in the region's land or copies (`isLoaded`, which a
    /// remembered thing beyond answers as "unknown": the plan's 7.5.1).
    fn is_loaded(&self, pos: BlockPos) -> bool;

    /// The entities whose boxes meet `bounds`: the region's own, then its copies', each
    /// by chunk, then section, then key (ADR-0028, section 2), where the game takes them
    /// in the order they were added (named).
    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>>;

    /// The entity `key`, where the region's land or copies have it: what a goal holds by
    /// key (its target), looked up as the game holds it by reference.
    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        let _ = key;
        None
    }

    /// The players a mob reads: the region's own, then those of other regions that its
    /// copies show (N7a2's faces), each by key.
    fn players(&self) -> Vec<SeenPlayer>;

    /// The light at `pos`: the chunk's stored light, or where a change left the chunk
    /// none, the sky's above its highest block that blocks motion and none below, and no
    /// light of blocks (the plan's 9.7.2, until N8). `None` where the chunk is not read.
    fn light(&self, pos: BlockPos) -> Option<LightAt>;
}
