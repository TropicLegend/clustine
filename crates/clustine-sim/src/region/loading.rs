//! Whether a player's client has loaded the level it shows (`docs/groundwork/v-plan.md`,
//! sections 2.1 and 4.1): while it has not, the player takes no damage.
//!
//! - **The game's rule** (`ServerGamePacketListenerImpl`): `hasClientLoaded @0-19` is
//!   `!waitingForRespawn && clientLoadedTimeoutTimer <= 0`. The timer is set to
//!   `CLIENT_LOADED_TIMEOUT_TIME`, 60, by `restartClientLoadTimerAfterRespawn @0-11`,
//!   which the listener's constructor calls at the join (`<init> @90`) and
//!   `handleClientCommand` after a respawn from a death (@167) or from the credits
//!   (@105); `ServerPlayer.tick @0-4` counts it down by one each tick
//!   (`tickClientLoadTimeout`); `handleAcceptPlayerLoad @13`, the client's
//!   `player_loaded`, sets it to 0 (`markClientLoaded`). `waitingForRespawn` is set by a
//!   death (`ServerPlayer.die @365`) and cleared by the respawn: here a dead player is
//!   [`super::Player`]'s `dead`, whom nothing hurts anyway.
//! - **Its use**: `ServerPlayer.isInvulnerableTo @26-33` refuses every damage while
//!   the client has not loaded, whatever its type: the void's `fell_out_of_world` too,
//!   which passes over the creative player's invulnerability but not this. Each kind of
//!   damage asks [`Region::client_loaded`]; today only the void hurts a player
//!   (`void.rs`), and survival's damage (the plan's step V4) asks it as well.
//!   An explosion's push is no damage: `ServerExplosion.hurtEntities` pushes (@422)
//!   whatever its `hurtServer` (@357) made of the hit, so a player who has not loaded
//!   is pushed (`explosions.rs`).
//!
//! Named differences:
//! - **A change of level** by a portal restarts the timer here, as a respawn does, and
//!   the client's `player_loaded` after the level's `Respawn` packet ends it. The game
//!   keeps no timer for it: `ServerPlayer.teleport @106` sets `isChangingDimension`,
//!   which `isInvulnerableTo @9-23` takes as invulnerable to all but an ender pearl's
//!   damage until the client confirms the teleport (`handleAcceptTeleportPacket @146`,
//!   `hasChangedDimension`). The two end at about the same moment for a client that
//!   loads; a pearl thrown through a portal hurts no one here until then.
//! - **In memory only**, as the rest of `void::Hurting`: a player handed to another
//!   region, by a crossing, a merge or a split, or restored from a commit, has loaded.
//!   Since the game takes no move of a client that has not loaded, a player crosses a
//!   line within their 60 ticks only where a merge or split moves the line over them.
//! - **The rest of `hasClientLoaded`'s uses are not made**: the game takes no move
//!   (`handleMovePlayer @107`, `handleMoveVehicle @73`), no action, use, attack or
//!   interaction (`handlePlayerAction @13`, `handleUseItemOn @13`, `handleUseItem @13`,
//!   `handleAttack @13`, `handleInteract @13`), no `player_command` (@13) and no change
//!   of the keys' sneaking (`handlePlayerInput @24`) before the client has loaded. A
//!   real client sends none of them from its loading screen, so nobody sees it.

use clustine_world::PlayerId;

use super::Region;

/// How long a player is taken not to have loaded where their client never says it has
/// (`ServerGamePacketListenerImpl.CLIENT_LOADED_TIMEOUT_TIME`).
pub(super) const CLIENT_LOADED_TIMEOUT: u64 = 60;

/// Whether a player's client has loaded the level, as the region's ticks count it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Loading {
    /// The first tick in which the player counts as loaded however silent their client
    /// is: the timer's end. 0, the default, for a player who has loaded.
    until: u64,
}

impl Loading {
    /// `restartClientLoadTimerAfterRespawn` in the region's tick `tick`.
    pub(super) fn restart(&mut self, tick: u64) {
        self.until = tick + CLIENT_LOADED_TIMEOUT;
    }

    /// `markClientLoaded`: the client said it has loaded.
    pub(super) fn mark_loaded(&mut self) {
        self.until = 0;
    }

    /// Whether the client counts as loaded in the region's tick `tick`.
    pub(super) fn loaded(self, tick: u64) -> bool {
        tick >= self.until
    }
}

impl Region {
    /// `hasClientLoaded` of player `id` in the tick under way: their client has said it
    /// has loaded, or the timeout has run out since their join, respawn or change of
    /// level, and they are not dead. Every damage to a player asks it first
    /// (`ServerPlayer.isInvulnerableTo @26-33`); a player the region does not hold has
    /// not.
    pub(super) fn client_loaded(&self, id: PlayerId) -> bool {
        self.players
            .get(&id)
            .is_some_and(|player| !player.dead && player.hurting.loading.loaded(self.tick))
    }
}
