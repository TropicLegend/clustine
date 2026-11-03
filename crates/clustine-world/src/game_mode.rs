//! A player's game mode (`docs/groundwork/v-plan.md`, section 4.1): each player's own,
//! kept with their place, given to a new player from the world's default
//! (`--gamemode`, as the game's `server.properties` `gamemode`). Survival and creative
//! are played; adventure and spectator are the game's and named here, but refused as a
//! default until built (the plan's question 11).

use serde::{Deserialize, Serialize};

/// The game's `GameType`, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GameMode {
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

/// What a game mode lets a player do (`Abilities`, as `GameType.updatePlayerAbilities`
/// sets it; n9 `GameType`, offsets 1-88).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Abilities {
    /// `mayfly`: the client may fly.
    pub may_fly: bool,
    /// `instabuild`: blocks break at once, and nothing placed or used is used up.
    pub instabuild: bool,
    /// `invulnerable`: nothing but the void hurts.
    pub invulnerable: bool,
    /// `mayBuild`: blocks may be placed and broken (not in adventure or spectator).
    pub may_build: bool,
}

impl GameMode {
    /// The mode of a new world's players where nothing says otherwise: survival, as
    /// the game's `server.properties` (`DedicatedServerProperties.<init> @160`). Until
    /// survival is whole (the plan's step V5) Clustine's own default stays creative.
    pub const GAME_DEFAULT: GameMode = GameMode::Survival;

    /// Its id (`GameType.getId`), which the protocol and the store write.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// The mode of an id, if it is one.
    pub fn of_id(id: u8) -> Option<GameMode> {
        match id {
            0 => Some(GameMode::Survival),
            1 => Some(GameMode::Creative),
            2 => Some(GameMode::Adventure),
            3 => Some(GameMode::Spectator),
            _ => None,
        }
    }

    /// Its name as the game's `server.properties` and commands write it.
    pub fn name(self) -> &'static str {
        match self {
            GameMode::Survival => "survival",
            GameMode::Creative => "creative",
            GameMode::Adventure => "adventure",
            GameMode::Spectator => "spectator",
        }
    }

    /// The mode a name says, if it says one.
    pub fn named(name: &str) -> Option<GameMode> {
        [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ]
        .into_iter()
        .find(|mode| mode.name() == name)
    }

    /// Whether Clustine plays it yet: adventure's `can_break` and `can_place_on`, and
    /// the spectator's being unseen and passing through blocks, are not built.
    pub fn is_built(self) -> bool {
        matches!(self, GameMode::Survival | GameMode::Creative)
    }

    /// What it lets a player do. Flying itself is the client's to start, but a mode
    /// that may not fly ends it, and a spectator always flies
    /// ([`GameMode::flies_from`]).
    pub fn abilities(self) -> Abilities {
        let (may_fly, instabuild, invulnerable) = match self {
            GameMode::Creative => (true, true, true),
            GameMode::Spectator => (true, false, true),
            GameMode::Survival | GameMode::Adventure => (false, false, false),
        };
        Abilities {
            may_fly,
            instabuild,
            invulnerable,
            // `!isBlockPlacingRestricted`.
            may_build: !matches!(self, GameMode::Adventure | GameMode::Spectator),
        }
    }

    /// Whether a player who was flying flies once this is their mode: a creative player
    /// keeps what they did, a spectator always flies, the others never
    /// (`updatePlayerAbilities` sets `flying` for all but creative).
    pub fn flies_from(self, flying: bool) -> bool {
        match self {
            GameMode::Creative => flying,
            GameMode::Spectator => true,
            GameMode::Survival | GameMode::Adventure => false,
        }
    }
}

/// A player's mode and the one before it (`previousGameMode`, which the client's F3+F4
/// switcher reads), kept with their place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerMode {
    pub current: GameMode,
    pub previous: Option<GameMode>,
}

impl PlayerMode {
    /// A player in `mode` from the first, with none before it.
    pub fn new(mode: GameMode) -> Self {
        Self {
            current: mode,
            previous: None,
        }
    }

    /// This, changed to `mode`: the old one becomes the previous
    /// (`ServerPlayerGameMode.setGameModeForPlayer`).
    pub fn changed_to(self, mode: GameMode) -> Self {
        Self {
            current: mode,
            previous: Some(self.current),
        }
    }
}

impl Default for PlayerMode {
    /// Creative, as every player was before survival (`docs/groundwork/v-plan.md`):
    /// what a place or a stay made before the mode was kept has.
    fn default() -> Self {
        Self::new(GameMode::Creative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_has_the_game_s_abilities() {
        let creative = GameMode::Creative.abilities();
        assert!(creative.may_fly && creative.instabuild && creative.invulnerable);
        assert!(creative.may_build);
        let survival = GameMode::Survival.abilities();
        assert!(!survival.may_fly && !survival.instabuild && !survival.invulnerable);
        assert!(survival.may_build);
        let adventure = GameMode::Adventure.abilities();
        assert!(!adventure.may_build && !adventure.may_fly);
        let spectator = GameMode::Spectator.abilities();
        assert!(spectator.may_fly && spectator.invulnerable && !spectator.instabuild);
        assert!(!spectator.may_build);
    }

    #[test]
    fn only_creative_keeps_its_flying_and_a_spectator_always_flies() {
        assert!(GameMode::Creative.flies_from(true));
        assert!(!GameMode::Creative.flies_from(false));
        assert!(GameMode::Spectator.flies_from(false));
        assert!(!GameMode::Survival.flies_from(true));
        assert!(!GameMode::Adventure.flies_from(true));
    }

    #[test]
    fn a_mode_is_read_back_by_its_id_and_its_name() {
        for mode in [
            GameMode::Survival,
            GameMode::Creative,
            GameMode::Adventure,
            GameMode::Spectator,
        ] {
            assert_eq!(GameMode::of_id(mode.id()), Some(mode));
            assert_eq!(GameMode::named(mode.name()), Some(mode));
        }
        assert_eq!(GameMode::of_id(4), None);
        assert_eq!(GameMode::named("hardcore"), None);
        assert!(GameMode::Survival.is_built() && GameMode::Creative.is_built());
        assert!(!GameMode::Adventure.is_built() && !GameMode::Spectator.is_built());
    }

    #[test]
    fn a_change_keeps_the_mode_before_it() {
        let mode = PlayerMode::new(GameMode::Survival).changed_to(GameMode::Creative);
        assert_eq!(mode.current, GameMode::Creative);
        assert_eq!(mode.previous, Some(GameMode::Survival));
        assert_eq!(PlayerMode::default(), PlayerMode::new(GameMode::Creative));
    }
}
