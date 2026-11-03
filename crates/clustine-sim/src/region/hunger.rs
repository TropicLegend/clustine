//! A player's hunger and what their client is told of their body (V4b,
//! `docs/groundwork/v-plan.md`, section 4.4; the build design's sections 3 and 7): the
//! food's tick (`crate::food::tick`) after the rest of the player's tick, as
//! `ServerPlayer.doTick` runs `FoodData.tick` after `Player.tick` and the menu's check
//! (@15-46), healing and starving through the player's hurt; eating's food; a drink's
//! instant effects; and at the end of the tick `SetHealth` on the game's rule of change
//! (`doTick @145-264`) and the player's own entity data where it changed.
//!
//! Named: peaceful's regeneration (`ServerPlayer.tickRegeneration`) waits for a
//! difficulty other than easy (`Difficulty::WORLD`); the food's timer counts the ticks a
//! player's body is not run (`food.rs`).

use clustine_world::{Difficulty, GameRules, PlayerId};

use super::Region;
use crate::api::{OwnData, PlayerEvent, TickOutput};
use crate::food::{self, FoodFacts, HealthSent};
use crate::mobs::body::effects::Effect;
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::state::{FoodData, PLAYER_MAX_HEALTH};

/// What a client shows of its own player's body before it is told anything: a new
/// player's.
const NEW_OWN_DATA: OwnData = OwnData {
    air: crate::state::PLAYER_MAX_AIR,
    absorption: 0.0,
    on_fire: false,
    frozen: 0,
    arrows: 0,
};

/// What the region last told each player of their body, in memory only: a region that
/// has told them nothing takes their client to show a new player's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Told {
    health: Option<HealthSent>,
    own: Option<OwnData>,
}

impl Region {
    /// `FoodData.tick` of each living player in the order of their ids: the food data
    /// moved on, a heal applied (`LivingEntity.heal`), a starve hurt (`starve`).
    pub(super) fn players_food_tick(&mut self, output: &mut TickOutput) {
        let now = i64::try_from(self.tick).unwrap_or(i64::MAX);
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let max_health = self.player_attribute(id, clustine_data::attributes::MAX_HEALTH);
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            if player.dead {
                continue;
            }
            let facts = FoodFacts {
                health: player.health,
                max_health: max_health as f32,
                difficulty: Difficulty::WORLD,
                natural_regeneration: GameRules::WORLD.natural_health_regeneration,
            };
            let ticked = food::tick(player.vitals.food, now, &facts);
            if ticked.food != player.vitals.food {
                player.vitals.food = ticked.food;
                self.journal.players.insert(id);
            }
            if let Some(amount) = ticked.heal {
                self.heal_player(id, amount);
            }
            if let Some(amount) = ticked.starve {
                let source = DamageSource::of(DamageType::STARVE);
                self.hurt_player(id, &source, amount, Default::default(), output);
            }
        }
    }

    /// `LivingEntity.heal(amount)` of the region's player `id`: more health, to their
    /// most, for a living one.
    pub(crate) fn heal_player(&mut self, id: PlayerId, amount: f32) {
        let max_health = self.player_attribute(id, clustine_data::attributes::MAX_HEALTH) as f32;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.dead || player.health <= 0.0 {
            return;
        }
        let health = (player.health + amount).min(max_health);
        if health != player.health {
            player.health = health;
            self.journal.players.insert(id);
        }
    }

    /// `FoodData.needsFood()` of the region's player `id`: their food below 20.
    pub(crate) fn needs_food(&self, id: PlayerId) -> bool {
        self.players
            .get(&id)
            .is_some_and(|player| player.vitals.food.food < food::MAX_FOOD)
    }

    /// `FoodData.eat(FoodProperties)` of what the region's player `id` ate.
    pub(crate) fn feed_player(&mut self, id: PlayerId, nutrition: i32, saturation: f32) {
        if let Some(player) = self.players.get_mut(&id) {
            food::eat(&mut player.vitals.food, nutrition, saturation);
            self.journal.players.insert(id);
        }
    }

    /// An instant effect of amplifier `amplifier` on the region's player `id` as a drunk
    /// potion applies it (`MobEffect.applyInstantenousEffect` with no source): Instant
    /// Health heals `max(4 << amp, 0)`, Instant Damage hurts `magic` `6 << amp`, and
    /// Saturation feeds `amp + 1` at 1 (`SaturationMobEffect`, `eat(int, float)`).
    /// Whether `effect` was one.
    pub(crate) fn player_instant(
        &mut self,
        id: PlayerId,
        effect: Effect,
        amplifier: u8,
        output: &mut TickOutput,
    ) -> bool {
        let shift = u32::from(amplifier.min(31));
        if effect == Effect::INSTANT_HEALTH {
            self.heal_player(id, (4_i32 << shift).max(0) as f32);
        } else if effect == Effect::INSTANT_DAMAGE {
            let source = DamageSource::of(DamageType::MAGIC);
            self.hurt_player(
                id,
                &source,
                (6_i32 << shift) as f32,
                Default::default(),
                output,
            );
        } else if effect == Effect::SATURATION {
            if let Some(player) = self.players.get_mut(&id) {
                food::eat_scaled(&mut player.vitals.food, i32::from(amplifier) + 1, 1.0);
                self.journal.players.insert(id);
            }
        } else {
            return false;
        }
        true
    }

    /// What each player is told of their body at the end of the tick: `SetHealth` where
    /// its health, food or saturation's being zero changed since it was last told
    /// (`doTick @145-192`), and their own entity data where it changed; to a player the
    /// region has told nothing yet (a join, an arrival, a respawn), what differs from a
    /// new player's.
    pub(super) fn report_vitals(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            // The kept copy of the absorption, as the health is.
            if player.vitals.absorption != player.effects.absorption {
                player.vitals.absorption = player.effects.absorption;
                self.journal.players.insert(id);
            }
            let sent = HealthSent::of(player.health, &player.vitals.food);
            let own = own_data(player);
            let (health, food) = (player.health, player.vitals.food);
            let before = self.told_vitals.insert(
                id,
                Told {
                    health: Some(sent),
                    own: Some(own),
                },
            );
            // A client the region has told nothing shows a new player's (its own
            // defaults), so only what differs from that is told.
            let before = before.unwrap_or_default();
            let shown = before
                .health
                .unwrap_or_else(|| HealthSent::of(PLAYER_MAX_HEALTH, &FoodData::default()));
            if food::needs_set_health(&shown, &sent) {
                output.player_events.push((
                    id,
                    PlayerEvent::Health {
                        health,
                        food: food.food,
                        saturation: food.saturation,
                    },
                ));
            }
            if before.own.unwrap_or(NEW_OWN_DATA) != own {
                output.player_events.push((id, PlayerEvent::OwnData(own)));
            }
        }
        self.told_vitals
            .retain(|id, _| self.players.contains_key(id));
    }

    /// Notes that the region's player `id` was told their health with their food as it
    /// is (`PlayerList.respawn`'s own `SetHealth`), so that the end of the tick does not
    /// tell it again; their own data is still told.
    pub(super) fn told_health(&mut self, id: PlayerId) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let health = HealthSent::of(player.health, &player.vitals.food);
        self.told_vitals.insert(
            id,
            Told {
                health: Some(health),
                own: None,
            },
        );
    }
}

/// What a player's own entity data says of their body.
fn own_data(player: &super::Player) -> OwnData {
    OwnData {
        air: player.vitals.air,
        absorption: player.effects.absorption,
        on_fire: player.vitals.fire > 0,
        frozen: player.vitals.frozen,
        arrows: player.vitals.arrows,
    }
}
