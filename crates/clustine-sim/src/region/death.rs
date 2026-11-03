//! What a player's death leaves (V4c, `docs/groundwork/v-plan.md`, section 2.4; the
//! build design's sections 3 and 7): the message the combat tracker chooses, and the
//! loot: what they carried, Curse of Vanishing destroyed, dropped as `Inventory.dropAll`
//! drops it, and their experience as orbs.
//!
//! Named: a fall's message is always `death.fell.accident.generic` (the game reads the
//! fall's location and an attacker that knocked them off: `CombatTracker.getFallMessage`);
//! an intentional game design's message has no link; a killer across a line has no name.

use clustine_world::{GameRules, PlayerId};

use super::Region;
use super::entities::Thrown;
use crate::api::{DeathMessage, Inventory, TickOutput};
use crate::mobs::body::hurt::DamageType;

/// The most experience a death drops (`Player.getBaseExperienceReward`: `level · 7`, at
/// most 100).
const MOST_DEATH_EXPERIENCE: i32 = 100;

impl Region {
    /// `CombatTracker.getDeathMessage` of the region's player `id`: from the hurt that
    /// killed them, its type's message (`death.attack.<id>`), naming its causer, or
    /// where it had none, whoever hurt them last (`.player`, `getKillCredit`); a fall's
    /// own message; `death.attack.generic` where nothing hurt them.
    pub(crate) fn death_message(&self, id: PlayerId) -> DeathMessage {
        let Some(player) = self.players.get(&id) else {
            return DeathMessage::plain("death.attack.generic");
        };
        let entries = &player.vitals.combat;
        let Some(kill) = entries.last() else {
            return DeathMessage::plain("death.attack.generic");
        };
        let Some(kind) = DamageType::named(&kill.damage_type) else {
            return DeathMessage::plain("death.attack.generic");
        };
        if kind.is("minecraft:is_fall") {
            return DeathMessage::plain("death.fell.accident.generic");
        }
        let key = format!("death.attack.{}", kind.message_id());
        if kind.death_message_type() == clustine_data::DeathMessageType::IntentionalGameDesign {
            return DeathMessage::plain(&format!("{key}.message"));
        }
        if let Some(killer) = &kill.killer {
            return DeathMessage {
                key,
                killer: Some(killer.clone()),
                item: None,
            };
        }
        // `getKillCredit`: who hurt them last, as an earlier entry names them.
        let tick = i64::try_from(self.tick).unwrap_or(i64::MAX);
        let credit = player
            .vitals
            .last_hurt_by
            .filter(|by| by.until >= tick)
            .and_then(|by| {
                entries
                    .iter()
                    .rev()
                    .filter_map(|entry| entry.killer.as_ref())
                    .find(|killer| killer.entity == by.entity)
            });
        match credit {
            Some(killer) => DeathMessage {
                key: format!("{key}.player"),
                killer: Some(killer.clone()),
                item: None,
            },
            None => DeathMessage::plain(&key),
        }
    }

    /// `ServerPlayer.dropAllDeathLoot` of the region's player `id`, unless a spectator
    /// or `keep_inventory`: `destroyVanishingCursedItems`, then `Inventory.dropAll` (the
    /// 36 slots, the armour, the off hand, in order, each as [`Region::drop_at_death`]),
    /// then `min(level · 7, 100)` of experience as orbs at their feet
    /// (`ExperienceOrb.award`). The inventory is empty after.
    pub(crate) fn drop_all_death_loot(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let spectator = player.mode.current == clustine_world::GameMode::Spectator;
        if spectator || GameRules::WORLD.keep_inventory {
            return;
        }
        let carried = std::mem::take(&mut player.inventory);
        let level = player.experience.level;
        let feet = player.pose.position;
        self.journal.players.insert(id);
        for stack in carried.0.iter().flatten() {
            if crate::items::enchantments::level(stack, "minecraft:vanishing_curse") > 0 {
                continue;
            }
            let dropped = Thrown {
                player: id,
                stack: stack.clone(),
                crouching: false,
            };
            self.drop_at_death(dropped, output);
        }
        let mut left = level.saturating_mul(7).min(MOST_DEATH_EXPERIENCE);
        let mut random = self.next_draw();
        while left > 0 {
            let value = crate::mobs::body::hurt::experience_value(left);
            left -= value;
            let draw = random.next_int(40);
            self.award_orb([feet.x, feet.y, feet.z], value, draw, output);
        }
        if let Some(player) = self.players.get_mut(&id) {
            player.inventory = Inventory::default();
        }
    }
}
