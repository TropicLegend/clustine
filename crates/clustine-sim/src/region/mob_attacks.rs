//! A player's hit on a mob (`Player.attack`, `causeExtraKnockback`, `doSweepAttack`;
//! the brief's section 1.9): the survival hit whatever the player's game mode (finding
//! 4), its attack strength, its critical hit while falling, its sweep with a sword,
//! the two pushes, and the attacker's sounds.
//!
//! The attack strength is the player's state (`experience.rs`). What the player holds
//! adds to their attack's
//! damage, speed and push by its default attribute modifiers.
//!
//! Named differences: the player's own last movement is not kept, so a sword sweeps on
//! the ground however fast they move; the attacker's own motion after the push and the
//! end of their sprint are their client's; no particle is sent (the critical hit's and
//! the sweep's, the hearts of the damage); no statistic is counted; a sweep breaks an
//! armour stand at once whoever swings, as a creative player's does, until survival's
//! breaking (V2, `materials.rs`).

use clustine_blocks::data::{has_water, in_tag, item_in_tag};
use clustine_data::{ItemComponents, attributes as ids, sounds};
use clustine_noise::trig;
use clustine_world::{EntityData, EntityId, ItemStack, PlayerId};

use super::Region;
use super::mob_bodies::player_kind;
use super::stands::MARKER;
use crate::api::{RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput};
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::body::{Instance, Modifier, attribute};
use crate::physics::{self, Vec3};

/// What a player's hit is now (`Player.attack`'s numbers before it lands).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Attack {
    /// What it deals: the scaled damage, a critical hit's half more.
    pub(crate) amount: f32,
    /// The scaled damage, which a sweep starts from.
    pub(crate) base: f32,
    /// `getAttackStrengthScale(0.5f)`.
    pub(crate) strength: f32,
    pub(crate) strong: bool,
    pub(crate) crit: bool,
    /// A strong hit while sprinting, which pushes half a block more.
    pub(crate) sprint_knock: bool,
    pub(crate) sweep: bool,
    /// `getKnockback`, and the sprint's half.
    pub(crate) knockback: f32,
    pub(crate) source: DamageSource,
    /// The attacker's yaw and feet.
    pub(crate) yaw: f32,
    pub(crate) feet: Vec3,
    /// Whether it lands at all: a hit of no damage plays nothing.
    pub(crate) lands: bool,
}

/// `AttributeInstance.getValue()` of a player's attribute `attribute` of the base
/// `base`, with the modifiers of `held` in the main hand.
pub(crate) fn held_value(base: f64, attribute_id: i32, held: Option<&ItemStack>) -> f64 {
    let attribute = attribute(attribute_id);
    let mut instance = Instance {
        attribute,
        base,
        modifiers: Vec::new(),
    };
    if let Some(stack) = held {
        // `EquipmentSlotGroup`: any slot, the main hand, either hand.
        for modifier in ItemComponents::of(stack.item).modifiers {
            if modifier.attribute == attribute && matches!(modifier.slot, 0 | 1 | 3) {
                instance.add_modifier(Modifier {
                    id: modifier.id.to_owned(),
                    amount: modifier.amount,
                    operation: modifier.operation,
                });
            }
        }
    }
    instance.value()
}

impl Region {
    /// The hit the region's player `id` makes now with what their main hand holds
    /// (`Player.attack` up to the hurt): `None` for a player the region does not have.
    pub(crate) fn attack_of(&self, id: PlayerId) -> Option<Attack> {
        let player = self.players.get(&id)?;
        let held = player.held();
        let feet = [
            player.pose.position.x,
            player.pose.position.y,
            player.pose.position.z,
        ];
        // The player's base damage 1 (`Player.createAttributes`), speed 4 and push 0.
        let damage = held_value(1.0, ids::ATTACK_DAMAGE, held.as_ref()) as f32;
        let push = held_value(0.0, ids::ATTACK_KNOCKBACK, held.as_ref()) as f32;
        let strength = self.attack_strength(id);
        // `createAttackSource`: the stack's own damage type, else the player's attack.
        let kind = held
            .as_ref()
            .and_then(|stack| ItemComponents::of(stack.item).damage_type)
            .map_or(DamageType::PLAYER_ATTACK, DamageType);
        let source = DamageSource::by(
            kind,
            Causer {
                name: Some(player.name.clone()),
                key: player.entity_id,
                kind: player_kind(),
                position: feet,
                player: Some(id.0.as_u128()),
                living: true,
            },
        );
        // No weapon is enchanted: nothing more of enchantments.
        let enchanted = 0.0_f32;
        let base = damage * (0.2 + strength * strength * 0.8);
        let lands = base > 0.0 || enchanted > 0.0;
        let strong = strength > 0.9;
        let sprinting = player.keys.sprint;
        let sprint_knock = sprinting && strong;
        let at = physics::containing(feet);
        let state = self.block(at).unwrap_or(clustine_data::blocks::AIR);
        let crit = strong
            && player.vitals.fall_distance > 0.0
            && !player.pose.on_ground
            && !in_tag(state, "minecraft:climbable")
            && !has_water(state)
            && player.riding.is_none()
            && !sprinting;
        let base_hit = base;
        let amount = if crit { base * 1.5 } else { base } + enchanted;
        let sword = held.is_some_and(|stack| item_in_tag(stack.item, "minecraft:swords"));
        let sweep = strong && !crit && !sprint_knock && player.pose.on_ground && sword;
        let knockback = push / 2.0 + if sprint_knock { 0.5 } else { 0.0 };
        Some(Attack {
            amount,
            base: base_hit,
            strength,
            strong,
            crit,
            sprint_knock,
            sweep,
            knockback,
            source,
            yaw: player.pose.yaw,
            feet,
            lands,
        })
    }

    /// `Player.attack(target)` of the region's player `player` on the region's mob
    /// `target`: the sprint's push sound before the hurt; the hurt; where it took, the
    /// attack's own push along the player's look, the sweep, and the critical hit's, the
    /// sweep's or the strong or weak hit's sound; where it did not, the sound of no
    /// damage.
    pub(crate) fn player_attack(
        &mut self,
        player: PlayerId,
        target: EntityId,
        part: Option<u8>,
        output: &mut TickOutput,
    ) {
        let Some(attack) = self.attack_of(player) else {
            return;
        };
        if !attack.lands {
            return;
        }
        // `onAttack`, behind the strength the hit was made with.
        self.reset_attack_strength(player);
        let attacker = attack.source.cause.as_ref().map(|cause| cause.key);
        if attack.sprint_knock {
            self.attack_sound(sounds::ENTITY_PLAYER_ATTACK_KNOCKBACK, attack.feet, output);
        }
        // `canCriticalAttack` asks the target to be a `LivingEntity`: a dragon's part,
        // which a hit names, is not, so a hit on it is never critical.
        let crit = attack.crit && part.is_none();
        let amount = if attack.crit && !crit {
            attack.amount - attack.base * 0.5
        } else {
            attack.amount
        };
        let took = self.hurt_mob_at(target, part, &attack.source, amount, output);
        if !took {
            self.attack_sound(sounds::ENTITY_PLAYER_ATTACK_NODAMAGE, attack.feet, output);
            return;
        }
        // `setLastHurtMob(target)`, which a tamed wolf of theirs reads.
        let now = self.clock.game_time;
        if let Some(held) = self.players.get_mut(&player) {
            held.last_hurt_mob = Some((target, now));
        }
        // `ItemStack.postHurtEnemy`: a weapon (`minecraft:weapon`) worn by its
        // `item_damage_per_attack`, but a creative player's (`materials.rs`).
        if let Some(weapon) = self
            .in_hand(player, crate::api::Hand::Main)
            .and_then(|held| ItemComponents::of(held.item).weapon)
        {
            let amount = i32::try_from(weapon.item_damage_per_attack).unwrap_or(i32::MAX);
            self.hurt_and_break_in_hand(player, crate::api::Hand::Main, amount, output);
        }
        let (sin, cos) = look(attack.yaw);
        if attack.knockback > 0.0 {
            // `causeExtraKnockback`: the attacker's motion and sprint after it are its
            // client's.
            self.knock_mob(target, f64::from(attack.knockback), sin, -cos, output);
        }
        if attack.sweep {
            self.sweep(&attack, target, output);
        }
        if crit {
            self.attack_sound(sounds::ENTITY_PLAYER_ATTACK_CRIT, attack.feet, output);
            if let Some(attacker) = attacker {
                output.events.push(RegionEvent::Animated {
                    target,
                    attacker,
                    chunk: physics::containing(attack.feet).chunk(),
                    action: 1,
                });
            }
        } else if !attack.sweep {
            let sound = if attack.strong {
                sounds::ENTITY_PLAYER_ATTACK_STRONG
            } else {
                sounds::ENTITY_PLAYER_ATTACK_WEAK
            };
            self.attack_sound(sound, attack.feet, output);
        }
    }

    /// `Player.attack` of another player on the region's player `target`, as the use `by`
    /// carries it (V4): from the attacker's region when they are this one's, else as it
    /// came across a line. The hurt (`player_attack`, which `pvp` off refuses); where it
    /// took, the attack's push along the attacker's look, the target's client told its
    /// motion at once and the motion the server keeps put back as it was before the
    /// attack (`causeExtraKnockback @124-166`). Named: across a line the attacker's
    /// strength is taken as full (its push is the sprint's where they sprint) and their
    /// weapon's wear, the sweep and the hit's sounds are not made; within the region the
    /// sweep passes over players.
    pub(crate) fn player_attacked(
        &mut self,
        target: PlayerId,
        by: &clustine_world::EntityUse,
        output: &mut TickOutput,
    ) {
        let clustine_world::EntityUse::Attack {
            attacker,
            sprinting,
            damage,
            from,
            look: [yaw, _],
            player,
            ref weapon,
            ..
        } = *by
        else {
            return;
        };
        let own = player
            .map(PlayerId)
            .filter(|id| self.players.contains_key(id));
        let attack = own.and_then(|id| self.attack_of(id));
        let name = player.and_then(|uuid| self.player_name(PlayerId(uuid)));
        let source = attack.as_ref().map_or_else(
            || {
                DamageSource::by(
                    DamageType::PLAYER_ATTACK,
                    Causer {
                        name,
                        key: attacker,
                        kind: player_kind(),
                        position: from.map(|value| value.0),
                        player: player.map(|uuid| uuid.as_u128()),
                        living: true,
                    },
                )
            },
            |attack| attack.source.clone(),
        );
        let amount = attack.as_ref().map_or(damage.0, |attack| attack.amount);
        let knockback = attack.as_ref().map_or_else(
            || {
                let push = held_value(0.0, ids::ATTACK_KNOCKBACK, weapon.as_ref()) as f32;
                push / 2.0 + if sprinting { 0.5 } else { 0.0 }
            },
            |attack| attack.knockback,
        );
        let yaw = attack.as_ref().map_or(yaw.0, |attack| attack.yaw);
        if let Some(id) = own {
            self.reset_attack_strength(id);
        }
        let before = self.players.get(&target).map(|player| player.vitals.motion);
        let hurting = crate::player_hurt::Hurting {
            weapon: weapon.as_ref(),
            pierce_level: 0,
        };
        if !self.hurt_player(target, &source, amount, hurting, output) {
            return;
        }
        let target_entity = self
            .players
            .get(&target)
            .map_or(attacker, |player| player.entity_id);
        let now = self.clock.game_time;
        if let Some(id) = own
            && let Some(held) = self.players.get_mut(&id)
        {
            held.last_hurt_mob = Some((target_entity, now));
        }
        if knockback > 0.0 {
            let (sin, cos) = look(yaw);
            self.knock_player(target, f64::from(knockback), sin, -cos, output);
        }
        // `causeExtraKnockback`: the motion told, and the server's put back.
        if let (Some(before), Some(player)) = (before, self.players.get_mut(&target)) {
            player.vitals.motion = before;
        }
    }

    /// The name of the region's player `id`, for a death message; none for one it does
    /// not have (named: a face carries no name).
    fn player_name(&self, id: PlayerId) -> Option<String> {
        self.players.get(&id).map(|player| player.name.clone())
    }

    /// `doSweepAttack(target, base, source, strength)`: its sound, then each living
    /// entity of the region's in the target's box grown a block sideways and a quarter
    /// up and down, within 3 of the attacker, but the target and markers, hurt by the
    /// sweep's 1 (no Sweeping Edge) times the strength and pushed by 0.4 along the look.
    fn sweep(&mut self, attack: &Attack, target: EntityId, output: &mut TickOutput) {
        self.attack_sound(sounds::ENTITY_PLAYER_ATTACK_SWEEP, attack.feet, output);
        let Some((_, record)) = self.find_entity(target) else {
            return;
        };
        let reach = self.entity_box(record).inflate([1.0, 0.25, 1.0]);
        let swept = 1.0_f32 * attack.strength;
        let (sin, cos) = look(attack.yaw);
        for (key, chunk) in self.entities_in(&reach) {
            if key == target {
                continue;
            }
            let Some(other) = self.entity_record(key, chunk).cloned() else {
                continue;
            };
            if physics::distance_sqr(attack.feet, other.position) >= 9.0 {
                continue;
            }
            match other.data {
                EntityData::Mob { .. } => {
                    if self.hurt_mob(key, &attack.source, swept, output) {
                        self.knock_mob(key, 0.400_000_005_960_464_5, sin, -cos, output);
                    }
                }
                // A creative player's hurt breaks a stand at once, and every player's
                // does until survival's breaking (V2).
                EntityData::ArmorStand { flags, .. } if flags & MARKER == 0 => {
                    self.hit_armor_stand(key, true, output);
                }
                _ => {}
            }
        }
    }

    /// `playServerSideSound(sound)`: at the attacker, from the players' source.
    fn attack_sound(&self, sound: i32, at: Vec3, output: &mut TickOutput) {
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_PLAYERS,
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }
}

/// `Mth.sin` and `Mth.cos` of the yaw `yaw` in degrees (`yRot * 0.017453292f`), the
/// game's table.
fn look(yaw: f32) -> (f64, f64) {
    let radians = f64::from(yaw * 0.017_453_292);
    (f64::from(trig::sin(radians)), f64::from(trig::cos(radians)))
}
