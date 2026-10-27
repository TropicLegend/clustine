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
    /// `isSprinting`: an armadillo rolls up at a sprinting player near it.
    pub(crate) sprinting: bool,
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

    /// Where the non-spectator players stand that a despawning mob measures to
    /// (`docs/groundwork/n9-plan.md`, section 9.5): those it reads, and in a region the
    /// world's players beyond them; none until the region has heard of the world's
    /// players, when a mob measures to those it reads but is not removed.
    fn despawn_players(&self) -> Option<Vec<[f64; 3]>> {
        Some(
            self.players()
                .into_iter()
                .filter(|player| !player.spectator)
                .map(|player| player.position)
                .collect(),
        )
    }

    /// `Level.getSeaLevel()`: the world's (a region's configuration), the overworld's
    /// 63 where a view knows no other.
    fn sea_level(&self) -> i32 {
        63
    }

    /// The points of interest within `r` of `centre` of a type `wanted` passes, with the
    /// tickets `occupancy` asks, in the fixed order of a query
    /// (`PoiManager.getInRange`; `docs/groundwork/n10-plan.md`, section 3.3): the land's,
    /// the copies' and the published margin's. None where a view knows none.
    fn pois_in_range(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: &dyn Fn(clustine_world::poi::PoiType) -> bool,
        occupancy: crate::region::pois::Occupancy,
    ) -> Vec<crate::region::pois::PoiRecord> {
        let _ = (centre, r, wanted, occupancy);
        Vec::new()
    }

    /// The same of `PoiManager.getInSquare`: within `r` on x and on z, at any height.
    fn pois_in_square(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: &dyn Fn(clustine_world::poi::PoiType) -> bool,
        occupancy: crate::region::pois::Occupancy,
    ) -> Vec<crate::region::pois::PoiRecord> {
        let _ = (centre, r, wanted, occupancy);
        Vec::new()
    }

    /// The type of the point of interest at `pos`, where the view knows it.
    fn poi_type_at(&self, pos: BlockPos) -> Option<clustine_world::poi::PoiType> {
        let _ = pos;
        None
    }

    /// `PoiManager.sectionsToVillage(section)`: 0 to 5, or 7 where no village reaches
    /// (section 3.4).
    fn sections_to_village(&self, section: [i32; 3]) -> i32 {
        let _ = section;
        crate::region::pois::NO_VILLAGE
    }

    /// `ServerLevel.isCloseToVillage(pos, r)`.
    fn is_close_to_village(&self, pos: BlockPos, r: i32) -> bool {
        r <= 6 && self.sections_to_village(crate::region::pois::section_of(pos)) <= r
    }

    /// `ServerLevel.isVillage(pos)`.
    fn is_village(&self, pos: BlockPos) -> bool {
        self.is_close_to_village(pos, 1)
    }

    /// `ServerLevel.getRaidAt(pos)`: the nearest active raid within 96 blocks, as other
    /// regions read raids, with the chunk its record is kept in, where every news of it
    /// goes (`docs/groundwork/n10-plan.md`, section 9.2). None where a view knows none.
    fn raid_at(&self, pos: BlockPos) -> Option<crate::api::RaidSeen> {
        let _ = pos;
        None
    }

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

    /// The effects raids and villagers read of the player whose stay's entity is `key`
    /// (bad omen, raid omen, Hero of the Village, `PlayerFace.omens`), where the view
    /// knows them; none where it does not.
    fn player_omens(&self, key: EntityId) -> crate::api::Omens {
        let _ = key;
        crate::api::Omens::default()
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

    /// `Level.getHeight(MOTION_BLOCKING, x, z)`: the y above the highest block that
    /// blocks motion in the column, as the chunk's heightmap keeps it; `None` where the
    /// chunk is not read.
    fn motion_blocking_height(&self, x: i32, z: i32) -> Option<i32> {
        let _ = (x, z);
        None
    }

    /// `Level.getHeight(MOTION_BLOCKING_NO_LEAVES, x, z)`, as
    /// [`MobWorld::motion_blocking_height`] for the heightmap that leaves leaves out (a
    /// patrol's walk); `None` where the chunk is not read.
    fn motion_blocking_no_leaves_height(&self, x: i32, z: i32) -> Option<i32> {
        let _ = (x, z);
        None
    }

    /// The light at `pos`: the chunk's stored light, or where a change left the chunk
    /// none, the sky's above its highest block that blocks motion and none below, and no
    /// light of blocks (the plan's 9.7.2, until N8). `None` where the chunk is not read.
    fn light(&self, pos: BlockPos) -> Option<LightAt>;

    /// The name of the biome at `pos`, where the chunk is read: what the environment
    /// attributes of a place follow (a snow golem's melting). None where a view knows
    /// no biomes.
    fn biome(&self, pos: BlockPos) -> Option<&'static str> {
        let _ = pos;
        None
    }
}
