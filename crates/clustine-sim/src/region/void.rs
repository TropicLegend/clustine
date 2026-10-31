//! The void that kills, and the death it brings (`docs/groundwork/p-plan.md`, sections
//! 19.2 P10d, 19.5 items 1, 2 and 4, and 19.7).
//!
//! - **The void's hurt** (`Entity.checkBelowWorld` → `onBelowWorld`): a player below
//!   their level's lowest y less 64 is hurt by `fell_out_of_world` for 4 each tick,
//!   creative or not (its type bypasses invulnerability), taken as
//!   `LivingEntity.hurtServer` takes it: in full once the hurt cooldown has run down to
//!   10 of its 20 ticks, and before that only what exceeds the last hurt, which for the
//!   void's even 4 is nothing. So five hits about ten ticks apart. Each full hit shows
//!   its `DamageEvent`, which the client plays the hurt from; the health is told.
//! - **The death** (`ServerPlayer.die`): `Durable::Died` with the void's message, the
//!   inventory and experience dropped where the player died (in the void: nowhere), the
//!   effects cleared, entity event 3 for whoever watches. The body is removed 20 ticks
//!   later with entity event 60 (`LivingEntity.tickDeath`), and the stay then waits to be
//!   asked for the respawn (`respawning.rs`); an ask before that ends the body at once.
//!   While dead, every input but `PerformRespawn` is dropped.
//!
//! Named: nothing else hurts a player yet; the death message is not told to other
//! players (there is no chat); the cooldown is kept in memory and starts afresh in a
//! region the player is handed to.

use clustine_world::{PlayerId, Way};

use super::Region;
use crate::api::{
    Durable, Experience, Inventory, PlayerEvent, PlayerInput, RegionEvent, TickOutput,
};
use crate::mobs::body::hurt::DamageType;
use crate::state::PLAYER_MAX_HEALTH;

/// What the void takes each tick (`Entity.onBelowWorld`).
const VOID_HURT: f32 = 4.0;

/// How far below a level's lowest y the void begins (`Entity.checkBelowWorld`).
const VOID_BELOW: f64 = 64.0;

/// The hurt cooldown's length, and the part of it within which only more than the last
/// hurt is taken (`LivingEntity.invulnerableTime`, `hurtServer`).
const HURT_COOLDOWN: u8 = 20;
const HALF_COOLDOWN: u8 = 10;

/// How long a dead player's body stays (`LivingEntity.tickDeath`).
const BODY_TICKS: u64 = 20;

/// The game's death message for the void.
const OUT_OF_WORLD_MESSAGE: &str = "death.attack.outOfWorld";

/// What the region keeps in memory of a player's hurts and death.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Hurting {
    /// `invulnerableTime`: the ticks left of the hurt cooldown.
    cooldown: u8,
    /// `lastHurt`: the last hurt taken within the cooldown.
    last: f32,
    /// The tick the player died in, while their body stays.
    died_at: Option<u64>,
}

impl Region {
    /// The void's hurt of each player below their level, in the order of their ids, and
    /// the end of each dead player's body after its 20 ticks.
    pub(super) fn void_tick(&mut self, output: &mut TickOutput) {
        let bottom = f64::from(self.config.lowest_y) - VOID_BELOW;
        let tick = self.tick;
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            // `baseTick`'s count down of the cooldown.
            player.hurting.cooldown = player.hurting.cooldown.saturating_sub(1);
            if player.dead {
                // A dead player restored from a commit has no tick of death in memory:
                // their body stays its 20 ticks from now.
                let died_at = *player.hurting.died_at.get_or_insert(tick);
                if tick >= died_at + BODY_TICKS {
                    self.end_body(id, output);
                }
                continue;
            }
            if player.pose.position.y < bottom {
                self.hurt_by_the_void(id, output);
            }
        }
    }

    /// `hurtServer` of the void's 4 on player `id`.
    fn hurt_by_the_void(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let hurting = &mut player.hurting;
        let (taken, full) = if hurting.cooldown > HALF_COOLDOWN {
            if VOID_HURT <= hurting.last {
                return;
            }
            let taken = VOID_HURT - hurting.last;
            hurting.last = VOID_HURT;
            (taken, false)
        } else {
            hurting.last = VOID_HURT;
            hurting.cooldown = HURT_COOLDOWN;
            (VOID_HURT, true)
        };
        player.health = (player.health - taken).max(0.0);
        let health = player.health;
        let (entity, chunk) = (player.entity_id, player.chunk());
        self.journal.players.insert(id);
        if full {
            output.events.push(RegionEvent::DamageEvent {
                entity,
                chunk,
                damage_type: DamageType::OUT_OF_WORLD.id().unwrap_or(0),
                cause: None,
                direct: None,
                position: None,
            });
        }
        output
            .player_events
            .push((id, PlayerEvent::Health { health }));
        if health <= 0.0 {
            self.die(id, output);
        }
    }

    /// `ServerPlayer.die` by the void: the death told to their edge, what they carried
    /// dropped into the void, the effects cleared, and the death shown to whoever
    /// watches.
    fn die(&mut self, id: PlayerId, output: &mut TickOutput) {
        if self
            .players
            .get(&id)
            .is_some_and(|player| player.riding.is_some())
        {
            self.dismount_player(id, output);
        }
        self.stop_sleep(id, true, false, output);
        self.menu_leaving(id, false, output);
        let effects: Vec<_> = self
            .players
            .get(&id)
            .map(|player| {
                (player.effects.active.iter())
                    .map(|active| active.effect)
                    .collect()
            })
            .unwrap_or_default();
        for effect in effects {
            self.remove_player_effect(id, effect, output);
        }
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.dead = true;
        player.hurting.died_at = Some(self.tick);
        // `dropAllDeathLoot` where they died: below the world, where nothing stays.
        player.inventory = Inventory::default();
        player.experience = Experience::default();
        let (edge, entity, chunk) = (player.edge, player.entity_id, player.chunk());
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityEvent {
            entity,
            chunk,
            event: 3,
        });
        let died = Durable::Died {
            player: id,
            entity,
            message: OUT_OF_WORLD_MESSAGE.to_owned(),
        };
        self.send(edge, died, output);
    }

    /// The end of a dead player's body (`tickDeath`'s event 60), and their stay kept
    /// waiting to be asked for the respawn, the respawned body made with it (health
    /// full, alive, no effects: the review of 19.7, item 8).
    fn end_body(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        self.journal.players.insert(id);
        let (entity, chunk) = (player.entity_id, player.chunk());
        output.events.push(RegionEvent::EntityEvent {
            entity,
            chunk,
            event: 60,
        });
        output
            .events
            .push(RegionEvent::EntityRemoved { entity, chunk });
        let edge = player.edge;
        let travel = self.respawn_travel(player.respawn, Way::Respawn);
        let mut transfer = player.into_transfer(self.tick, travel.to);
        transfer.health = PLAYER_MAX_HEALTH;
        transfer.dead = false;
        transfer.effects.clear();
        self.wait_to_be_asked(edge, id, transfer, travel);
    }

    /// An input of a dead player, whose body still stays: `PerformRespawn` ends the body
    /// and sends them on to their respawn; anything else is dropped. Whether the player
    /// is dead.
    pub(super) fn dead_input(
        &mut self,
        id: PlayerId,
        input: &PlayerInput,
        output: &mut TickOutput,
    ) -> bool {
        if !self.players.get(&id).is_some_and(|player| player.dead) {
            return false;
        }
        if matches!(input, PlayerInput::PerformRespawn) {
            self.end_body(id, output);
            self.asked_input(id, input);
        }
        true
    }
}
