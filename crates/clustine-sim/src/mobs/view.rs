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
    /// `hasMovedHorizontallyRecently`: they moved sideways between their last two
    /// ticks (a dolphin follows a boat whose rider does).
    pub(crate) moved_horizontally: bool,
    /// Whether they live (`isAlive`).
    pub(crate) alive: bool,
    /// The game time they lay down at, while they sleep in a bed (`isSleeping`, and
    /// `getSleepTimer` reckoned from it).
    pub(crate) sleeping: Option<u64>,
    /// The mob they last hurt and the game time of it (`getLastHurtMob` and its
    /// timestamp), which a tamed wolf attacks.
    pub(crate) last_hurt_mob: Option<(EntityId, u64)>,
    /// The mob that last hurt them and the game time of it (`getLastHurtByMob` and its
    /// timestamp), which a tamed wolf defends them from.
    pub(crate) last_hurt_by: Option<(EntityId, u64)>,
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

    /// The world's game time as the region's clock has it (`Level.getGameTime()`): what
    /// a wolf's anger ends at (none where a view knows no clock).
    fn game_time(&self) -> i64 {
        0
    }

    /// A player of the world wherever they are, by their UUID: those the mob reads
    /// ([`MobWorld::players`]) first, then the world's players the store publishes once
    /// a second (position and game mode, a second late), as a pet resolves its owner
    /// beyond the copies (`docs/groundwork/n7-plan.md`, section 7.3, item 3).
    fn player_anywhere(&self, uuid: u128) -> Option<SeenPlayer> {
        self.players()
            .into_iter()
            .find(|player| player.uuid == uuid)
    }

    /// `Level.getSkyDarken()`: how far the sky's light is below full by the time of day
    /// (none where a view knows no clock).
    fn sky_darken(&self) -> i32 {
        0
    }

    /// The overworld clock's ticks (`WorldClock::day`), which the day's timeline
    /// follows: what a bee's `bees_stay_in_hive` reads (the morning where a view knows
    /// no clock).
    fn day(&self) -> u64 {
        0
    }

    /// The data of the block entity at `pos`, where the region's land or copies have
    /// one (a hive's occupants).
    fn block_entity_data(&self, pos: BlockPos) -> Option<&[u8]> {
        let _ = pos;
        None
    }

    /// The light at `pos`: the chunk's stored light, or where a change left the chunk
    /// none, the sky's above its highest block that blocks motion and none below, and no
    /// light of blocks (the plan's 9.7.2, until N8). `None` where the chunk is not read.
    fn light(&self, pos: BlockPos) -> Option<LightAt>;
}
