//! The End's dragon fight as the world keeps it (`EnderDragonFight`'s saved data,
//! `docs/groundwork/p-brief.md`, section 6.1). The End's home region keeps it in its
//! state (`docs/groundwork/p-plan.md`, section 15.2).

use serde::{Deserialize, Serialize};

use crate::{BlockPos, EntityId};

/// What the game saves of the fight, with its defaults in [`DragonFight::default`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DragonFight {
    pub needs_state_scanning: bool,
    pub dragon_killed: bool,
    pub previously_killed: bool,
    /// The stage of a respawn under way, by its place in `DragonRespawnStage`.
    pub respawn_stage: Option<u8>,
    pub respawn_time: i32,
    /// The fight's own dragon (`dragon_uuid`), by its key.
    pub dragon: Option<EntityId>,
    pub exit_portal: Option<BlockPos>,
    /// The gateways still to open, by their index from 0 to 19, the next last.
    pub gateways: Vec<i32>,
    /// The crystals of a respawn under way (`respawn_crystals`), by their keys.
    pub respawn_crystals: Vec<EntityId>,
}

impl Default for DragonFight {
    /// `EnderDragonFight.createDefault`: a fight that has never been looked at.
    fn default() -> Self {
        Self {
            needs_state_scanning: true,
            dragon_killed: false,
            previously_killed: false,
            respawn_stage: None,
            respawn_time: 0,
            dragon: None,
            exit_portal: None,
            gateways: Vec::new(),
            respawn_crystals: Vec::new(),
        }
    }
}
