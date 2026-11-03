//! The world's game rules that survival reads (V4, `docs/groundwork/v-plan.md`, section
//! 4.3, and its build design's section 7), by the game's names, at the game's defaults.
//! Nothing sets them yet: when the commands to change them are built they become a
//! setting the world keeps, as the difficulty would, so that every worker reads the same.

/// The game rules that a player's survival reads (`GameRules`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameRules {
    /// `pvp`: a player's attack on another player, or their projectile's, is refused
    /// when off (`ServerPlayer.hurtServer`).
    pub pvp: bool,
    /// `natural_health_regeneration`: food heals (`FoodData.tick`,
    /// `ServerPlayer.tickRegeneration`).
    pub natural_health_regeneration: bool,
    /// `keep_inventory`: a death keeps the inventory and the experience.
    pub keep_inventory: bool,
    /// `drowning_damage`, `fall_damage`, `fire_damage`, `freeze_damage`: when off, a
    /// player is invulnerable to those types (`Player.isInvulnerableTo @12-139`).
    pub drowning_damage: bool,
    pub fall_damage: bool,
    pub fire_damage: bool,
    pub freeze_damage: bool,
    /// `show_death_messages`: a death's message is told.
    pub show_death_messages: bool,
    /// `forgive_dead_players`: neutral mobs angry at a player forgive them at their death.
    pub forgive_dead_players: bool,
}

impl GameRules {
    /// The world's: the game's defaults.
    pub const WORLD: GameRules = GameRules {
        pvp: true,
        natural_health_regeneration: true,
        keep_inventory: false,
        drowning_damage: true,
        fall_damage: true,
        fire_damage: true,
        freeze_damage: true,
        show_death_messages: true,
        forgive_dead_players: true,
    };
}
