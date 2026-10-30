//! A player's throws and shots (`docs/groundwork/n9-plan.md`, sections 4.4, 4.6 and 4.7;
//! part S4's): the throwables' `use` (the snowball, the eggs, the potions, the
//! experience bottle and the ender pearl with its cooldown), the bow's and the trident's
//! release, the crossbow's charge and its shot, and a rocket used on a block. The rules
//! of the items are `items/`; what is here reads and writes the region: the hands, the
//! inventory, the streams, the making of the projectiles.
//!
//! The bow's and the trident's `releaseUsing` act only for a player (`instanceof
//! Player`, n9 BowItem 0–17, TridentItem 0–17): a mob shoots by its own
//! `performRangedAttack`, so nothing here is called for one.
//!
//! The streams: a throw's or a shot's sound draws from the level's random (the region's
//! `next_draw`), each projectile its UUID and its shot from its own stream at its birth
//! (`make_projectile`), and the crossbow's pitch of each projectile after the first
//! from the shooter's random (the player's stream aside, `entity_stream`).
//!
//! Every player is creative until survival (`hasInfiniteMaterials`): they keep what they
//! throw and shoot, their weapons do not wear and their bows draw intangible arrows.
//! What a survival player would lose is written all the same, and runs once there is
//! survival.
//!
//! Named differences: the game's `spawnProjectileFromRotation` with a roll (the potions'
//! and the bottle's -20) is made here as `shoot` along the rolled look with the
//! thrower's movement added afterwards, which is the same sum (`Aim` has no roll), and a
//! bow's Flame sets the arrow alight after it is made, as the game's
//! `applyOnProjectileSpawned` does after `addFreshEntity`; a weapon that breaks is gone
//! from the hand without the game's sound and animation of the break; where nothing is
//! made (the chunk full, nowhere to make it), nothing is taken from the hand.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId, ItemStack, OFF_HAND, PlayerId};

use super::Region;
use super::projectiles::{Aim, Making, shot_record};
use crate::api::{Hand, PlayerEvent, RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput, Using};
use crate::items::{bow, crossbow, enchantments, rockets, throwables, trident};
use crate::mobs::ai::maths::{RADIANS, table_cos, table_sin};
use crate::mobs::tick::Shot;
use crate::physics::{Vec3, thrown};
use crate::placement::{Click, ItemUse};

/// How far above a player's feet their eyes are, standing and crouching.
const STANDING_EYES: f32 = 1.62;
const CROUCHING_EYES: f32 = 1.27;

/// `Player.hasInfiniteMaterials`: every player is creative until survival.
const fn has_infinite_materials(_player: PlayerId) -> bool {
    true
}

/// The hit sound a crossbow's arrow is given (`setSoundEvent(CROSSBOW_HIT)`).
const CROSSBOW_HIT: &str = "minecraft:item.crossbow.hit";

/// What shot a projectile: which rules make and aim it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weapon {
    Bow,
    Crossbow,
}

/// Where a player is and looks, as a throw or a shot reads it.
#[derive(Debug, Clone, Copy)]
struct Shooter {
    entity: EntityId,
    feet: Vec3,
    eyes: f64,
    yaw: f32,
    pitch: f32,
    known: Vec3,
    on_ground: bool,
}

impl Region {
    /// The throw of what the player `player` holds, used in the air
    /// (`docs/groundwork/n9-plan.md`, section 4.4; `items/throwables.rs`): the throw's
    /// sound, then the projectile made at their eyes and shot along their look with
    /// their movement (`spawnProjectileFromRotation`), one taken from the stack but by a
    /// creative player, then the stack's `use_cooldown` begun (the ender pearl's
    /// second). Whether the held item was a throwable.
    pub(crate) fn throw_held(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(item) = used.held else {
            return false;
        };
        let Some(throwable) = throwables::of(item) else {
            return false;
        };
        let Some(hand) = self.hand_with_item(player, item) else {
            return false;
        };
        let Some(stack) = self.in_hand(player, hand) else {
            return false;
        };
        let Some(shooter) = self.shooter(player, Some((used.sneaking, used.yaw, used.pitch)))
        else {
            return false;
        };
        // `level.playSound(null, x, y, z, …, 0.5, pitch)`, the pitch of the level's
        // random, heard by everyone.
        let pitch = throwables::pitch(self.next_draw().next_float());
        output.events.push(RegionEvent::Sound {
            sound: throwable.sound,
            source: throwable.source,
            position: shooter.feet_vec(),
            volume: throwables::VOLUME,
            pitch,
            except: None,
        });
        let data = EntityData::Thrown {
            // `ThrowableItemProjectile.setItem`: one of the stack, with its components
            // (a potion's contents, an egg's variant).
            stack: stack.with_count(1),
            owner: Some(shooter.entity),
            left_owner: false,
            has_been_shot: false,
        };
        let made = self.make_from_rotation(
            throwable.kind,
            shooter.below_the_eyes(thrown::BELOW_THE_EYES),
            data,
            &shooter,
            (shooter.yaw, shooter.pitch, throwable.roll),
            (throwable.power, throwable.uncertainty),
            output,
        );
        if let Some(key) = made {
            self.fire_from(key, &stack, output);
            self.consume_held(player, hand, &stack, 1, output);
        }
        // `applyAfterUseComponentSideEffects` of the stack as it was before the use.
        if let Some(ticks) = throwables::use_cooldown_ticks(&stack) {
            self.add_cooldown(player, &stack, ticks, output);
        }
        true
    }

    /// `FireworkRocketItem.useOn` of the rocket in the player's hand: a rocket made at the
    /// clicked point moved 0.15 along the face (`Region::make_firework`), unshot, one of
    /// the stack used but by a creative player. Whether the held item was a rocket.
    pub(crate) fn rocket_used_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held != Some(items::FIREWORK_ROCKET) {
            return false;
        }
        let Some(hand) = self.hand_with_item(player, items::FIREWORK_ROCKET) else {
            return false;
        };
        let Some(stack) = self.in_hand(player, hand) else {
            return false;
        };
        let Some(owner) = self.players.get(&player).map(|own| own.entity_id) else {
            return false;
        };
        let p = click.position;
        let clicked = [
            f64::from(p.x) + f64::from(click.cursor[0]),
            f64::from(p.y) + f64::from(click.cursor[1]),
            f64::from(p.z) + f64::from(click.cursor[2]),
        ];
        let at = rockets::placed_at(clicked, click.face);
        // Until the rockets' part makes them, nothing is made, and nothing is taken.
        if self
            .make_firework(&stack, at, None, false, Some(owner), output)
            .is_some()
        {
            self.consume_held(player, hand, &stack, 1, output);
        }
        true
    }

    /// `CrossbowItem.use` of a charged crossbow in the player `id`'s hand `hand`
    /// (`performShooting`, n9 CrossbowItem 46–91 and 395–450): its charge emptied, then
    /// each projectile shot at its angle at the charge's speed, crit for a player.
    pub(crate) fn shoot_charged_crossbow(
        &mut self,
        id: PlayerId,
        hand: Hand,
        output: &mut TickOutput,
    ) {
        let Some(stack) = self.in_hand(id, hand) else {
            return;
        };
        let charged = crossbow::charged(&stack);
        let emptied = crossbow::with_charge(&stack, Vec::new());
        self.set_hand(id, hand, Some(emptied.clone()), output);
        if charged.is_empty() {
            return;
        }
        let power = crossbow::shooting_power(&charged);
        self.shoot_projectiles(
            id,
            hand,
            emptied,
            Weapon::Crossbow,
            charged,
            (power, crossbow::PLAYER_UNCERTAINTY),
            true,
            output,
        );
    }

    /// `CrossbowItem.onUseTick` of the player `id`'s use (n9 CrossbowItem 489–569): the
    /// charging sounds at 0.2 and 0.5 of the charge, and at its end the projectiles
    /// drawn into the crossbow's `charged_projectiles` with the end's sound, where it is
    /// not charged already.
    pub(crate) fn charge_crossbow(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        let Some(stack) = self.in_hand(id, using.hand) else {
            return;
        };
        if stack.item != items::CROSSBOW {
            return;
        }
        let Some(feet) = self.players.get(&id).map(|player| player.pose.position) else {
            return;
        };
        let ticks = super::using::use_duration(&stack).unwrap_or(0) - using.remaining;
        let sounds = crossbow::charging_sounds(&stack);
        let sound = |sound: Option<i32>, volume: f32, pitch: f32, output: &mut TickOutput| {
            if let Some(sound) = sound {
                output.events.push(RegionEvent::Sound {
                    sound,
                    source: SOUND_SOURCE_PLAYERS,
                    position: feet,
                    volume,
                    pitch,
                    except: None,
                });
            }
        };
        if crossbow::reaches(ticks, &stack, crossbow::START_SOUND_AT) {
            sound(sounds.start, 0.5, 1.0, output);
        }
        if crossbow::reaches(ticks, &stack, crossbow::MIDDLE_SOUND_AT) {
            sound(sounds.middle, 0.5, 1.0, output);
        }
        if crossbow::progress(ticks, &stack) < crossbow::LOADED_AT || crossbow::is_charged(&stack) {
            return;
        }
        // `tryLoadProjectiles`: what `getProjectile` finds, drawn into the charge.
        let Some((slot, ammo)) = self.projectile_for(id, &stack) else {
            return;
        };
        let drawn = bow::draw(&stack, &ammo, has_infinite_materials(id));
        if drawn.projectiles.is_empty() {
            return;
        }
        self.take_ammunition(id, slot, &ammo, drawn.taken, output);
        let loaded = crossbow::with_charge(&stack, drawn.projectiles);
        self.set_hand(id, using.hand, Some(loaded), output);
        // The end's sound, of the shooter's source (a player's), its pitch of the
        // level's random.
        let pitch = crossbow::loaded_pitch(self.next_draw().next_float());
        sound(sounds.end, 1.0, pitch, output);
    }

    /// `Item.releaseUsing` of the player `id`'s use: a bow loosed, a trident thrown; a
    /// crossbow's loads nothing (its last tick, which `using.rs` runs, does).
    pub(crate) fn release_using(&mut self, id: PlayerId, using: Using, output: &mut TickOutput) {
        let Some(stack) = self.in_hand(id, using.hand) else {
            return;
        };
        // `getUseDuration − remaining`: the ticks the use has run.
        let ticks = super::using::use_duration(&stack).unwrap_or(0) - using.remaining;
        match stack.item {
            items::BOW => self.loose_bow(id, using.hand, stack, ticks, output),
            items::TRIDENT => self.throw_trident(id, using.hand, stack, ticks, output),
            _ => {}
        }
    }

    /// `BowItem.releaseUsing` (n9 BowItem 14–116): the ammunition `getProjectile` finds,
    /// none below a tenth of the power; drawn, shot at three times the power with the
    /// uncertainty 1, crit at full power; then `arrow_shoot` with its pitch of the
    /// level's random, after the shot.
    fn loose_bow(
        &mut self,
        id: PlayerId,
        hand: Hand,
        stack: ItemStack,
        ticks: i32,
        output: &mut TickOutput,
    ) {
        let Some((slot, ammo)) = self.projectile_for(id, &stack) else {
            return;
        };
        let Some(power) = bow::release_power(ticks) else {
            return;
        };
        let drawn = bow::draw(&stack, &ammo, has_infinite_materials(id));
        self.take_ammunition(id, slot, &ammo, drawn.taken, output);
        if !drawn.projectiles.is_empty() {
            self.shoot_projectiles(
                id,
                hand,
                stack,
                Weapon::Bow,
                drawn.projectiles,
                (power * bow::SPEED, bow::UNCERTAINTY),
                power == 1.0,
                output,
            );
        }
        let Some(feet) = self.players.get(&id).map(|player| player.pose.position) else {
            return;
        };
        let pitch = bow::shoot_pitch(self.next_draw().next_float(), power);
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_ARROW_SHOOT,
            source: SOUND_SOURCE_PLAYERS,
            position: feet,
            volume: 1.0,
            pitch,
            except: None,
        });
    }

    /// `TridentItem.releaseUsing` without Riptide (n9 TridentItem 61–277): after ten
    /// ticks held, the trident worn by one but by a creative player, one of it thrown
    /// from the eyes along the look at 2.5 (`spawnProjectileFromRotation`), only a
    /// creative player picking it up again, and `trident_throw` at it, without a draw.
    fn throw_trident(
        &mut self,
        id: PlayerId,
        hand: Hand,
        stack: ItemStack,
        ticks: i32,
        output: &mut TickOutput,
    ) {
        if !trident::throws(&stack, ticks) {
            return;
        }
        let infinite = has_infinite_materials(id);
        // `hurtWithoutBreaking(1, player)`, nothing for a creative player.
        let stack = if infinite {
            stack
        } else {
            let mut level = self.next_draw();
            let change = enchantments::durability_change(&stack, 1, || level.next_float());
            let worn = enchantments::hurt_without_breaking(&stack, change);
            self.set_hand(id, hand, Some(worn.clone()), output);
            worn
        };
        let Some(shooter) = self.shooter(id, None) else {
            return;
        };
        let thrown = stack.with_count(1);
        let shot = Shot {
            fire_ticks: 0,
            kind: u16::try_from(entity_types::TRIDENT).expect("entity type ids fit 16 bits"),
            stack: thrown,
            weapon: None,
            at: [0.0; 3],
            direction: [0.0; 3],
            power: trident::POWER,
            uncertainty: trident::UNCERTAINTY,
            from_mob: None,
        };
        let Some(mut data) = shot_record(&shot, Some(shooter.entity)) else {
            return;
        };
        if let EntityData::Arrow(arrow) = &mut data {
            // `setOwner` of a player allows anyone; `hasInfiniteMaterials` only creative
            // players.
            arrow.pickup = if infinite {
                clustine_world::Pickup::CreativeOnly
            } else {
                clustine_world::Pickup::Allowed
            };
        }
        let at = shooter.below_the_eyes(thrown::BELOW_THE_EYES);
        let made = self.make_from_rotation(
            entity_types::TRIDENT,
            at,
            data,
            &shooter,
            (shooter.yaw, shooter.pitch, 0.0),
            (trident::POWER, trident::UNCERTAINTY),
            output,
        );
        if made.is_none() {
            return;
        }
        // `consumeAndReturn(1, player)`: kept by a creative player.
        self.consume_held(id, hand, &stack, 1, output);
        // `playSound(null, trident, sound, PLAYERS, 1, 1)`: at the trident.
        output.events.push(RegionEvent::Sound {
            sound: sounds::ITEM_TRIDENT_THROW,
            source: SOUND_SOURCE_PLAYERS,
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// `ProjectileWeaponItem.shoot` of `projectiles` from `weapon` in the player `id`'s
    /// `hand` (n9 ProjectileWeaponItem 49–200): each at its angle of Multishot's spread,
    /// made, shot and added (`spawnProjectile`), set alight by Flame, then the weapon
    /// worn by its durability use, which ends the shot where it breaks.
    #[expect(
        clippy::too_many_arguments,
        reason = "the game's shoot takes as many: the shooter, the hand, the weapon, the projectiles, the speed and uncertainty, the crit"
    )]
    fn shoot_projectiles(
        &mut self,
        id: PlayerId,
        hand: Hand,
        weapon: ItemStack,
        kind: Weapon,
        projectiles: Vec<ItemStack>,
        (speed, uncertainty): (f32, f32),
        crit: bool,
        output: &mut TickOutput,
    ) {
        let spread = enchantments::projectile_spread(&weapon, 0.0);
        let angles = bow::angles(projectiles.len(), spread);
        let mut weapon = weapon;
        for (index, (projectile, angle)) in projectiles.into_iter().zip(angles).enumerate() {
            if projectile.count <= 0 {
                continue;
            }
            let Some(shooter) = self.shooter(id, None) else {
                return;
            };
            let made = match kind {
                Weapon::Bow => {
                    let aim = (shooter.yaw + angle, shooter.pitch, 0.0);
                    self.make_arrow(
                        &shooter,
                        &projectile,
                        &weapon,
                        crit,
                        false,
                        output,
                        |region, at, data, output| {
                            region.make_from_rotation(
                                bow::arrow_kind(projectile.item),
                                at,
                                data,
                                &shooter,
                                aim,
                                (speed, uncertainty),
                                output,
                            )
                        },
                    )
                }
                Weapon::Crossbow => {
                    let direction = crossbow::player_shot_vector(shooter.yaw, shooter.pitch, angle);
                    let made = if projectile.item == items::FIREWORK_ROCKET {
                        let at = shooter.below_the_eyes(crossbow::ROCKET_BELOW_THE_EYES);
                        self.make_firework(
                            &projectile,
                            at,
                            Some((Aim::Direction(direction), speed, uncertainty)),
                            true,
                            Some(shooter.entity),
                            output,
                        )
                    } else {
                        self.make_arrow(
                            &shooter,
                            &projectile,
                            &weapon,
                            crit,
                            true,
                            output,
                            |region, at, data, output| {
                                region.make_projectile(
                                    Making {
                                        kind: u16::try_from(bow::arrow_kind(projectile.item))
                                            .expect("entity type ids fit 16 bits"),
                                        at,
                                        data,
                                        aim: Aim::Direction(direction),
                                        power: speed,
                                        uncertainty,
                                    },
                                    |_, _| {},
                                    output,
                                )
                            },
                        )
                    };
                    // `getShotPitch(shooter.getRandom(), index)`, then `crossbow_shoot` at
                    // the shooter, of their source, for every projectile.
                    let mut stream = self.entity_stream(shooter.entity);
                    let start = stream.random.clone();
                    let pitch = crossbow::shot_pitch(index, || stream.random.next_float());
                    self.entity_drew(shooter.entity, stream.random.draws_since(&start));
                    output.events.push(RegionEvent::Sound {
                        sound: sounds::ITEM_CROSSBOW_SHOOT,
                        source: SOUND_SOURCE_PLAYERS,
                        position: shooter.feet_vec(),
                        volume: 1.0,
                        pitch,
                        except: None,
                    });
                    made
                }
            };
            if let Some(key) = made {
                self.fire_from(key, &projectile, output);
                if weapon.item != projectile.item {
                    self.fire_from(key, &weapon, output);
                }
            }
            // `weapon.hurtAndBreak(getDurabilityUse(projectile), shooter, hand)`: none for
            // a creative player.
            if !has_infinite_materials(id) {
                let use_ = match kind {
                    Weapon::Bow => 1,
                    Weapon::Crossbow => crossbow::durability_use(&projectile),
                };
                let mut level = self.next_draw();
                let change = enchantments::durability_change(&weapon, use_, || level.next_float());
                let worn = enchantments::hurt_and_break(&weapon, change);
                self.set_hand(id, hand, worn.clone(), output);
                match worn {
                    Some(worn) => weapon = worn,
                    None => return,
                }
            }
        }
    }

    /// An arrow of `projectile` from `weapon` made by `make` at the shooter's eyes less
    /// 0.1 (`AbstractArrow`'s constructor of a shooter): its record as a player's
    /// (`setOwner` allows its pickup), crit as asked, pierce of the weapon's Piercing,
    /// and a crossbow's hit sound.
    #[expect(
        clippy::too_many_arguments,
        reason = "the arrow's making reads the shooter, the stacks and the shot"
    )]
    fn make_arrow(
        &mut self,
        shooter: &Shooter,
        projectile: &ItemStack,
        weapon: &ItemStack,
        crit: bool,
        from_crossbow: bool,
        output: &mut TickOutput,
        make: impl FnOnce(&mut Region, Vec3, EntityData, &mut TickOutput) -> Option<EntityId>,
    ) -> Option<EntityId> {
        let shot = Shot {
            fire_ticks: 0,
            kind: u16::try_from(bow::arrow_kind(projectile.item))
                .expect("entity type ids fit 16 bits"),
            stack: projectile.clone(),
            weapon: Some(weapon.clone()),
            at: [0.0; 3],
            direction: [0.0; 3],
            power: 0.0,
            uncertainty: 0.0,
            from_mob: None,
        };
        let mut data = shot_record(&shot, Some(shooter.entity))?;
        if let EntityData::Arrow(arrow) = &mut data {
            arrow.pickup = bow::player_arrow_pickup(projectile);
            arrow.crit = crit;
            let pierce = enchantments::piercing(weapon);
            if pierce > 0 {
                arrow.pierce_level = u8::try_from(pierce.min(127)).expect("a byte");
            }
            if from_crossbow {
                CROSSBOW_HIT.clone_into(&mut arrow.hit_sound);
            }
        }
        let at = shooter.below_the_eyes(thrown::BELOW_THE_EYES);
        make(self, at, data, output)
    }

    /// The projectile `kind` made at `at` with the record `data` and shot as
    /// `spawnProjectileFromRotation(…, roll, power, uncertainty)` of `shooter` looking
    /// along `(yaw, pitch, roll)`: without a roll by the shared `Aim::Rotation`; with one
    /// by `shoot` along the rolled look, then the shooter's movement added as
    /// `shootFromRotation` adds it.
    #[expect(
        clippy::too_many_arguments,
        reason = "spawnProjectileFromRotation takes as many"
    )]
    fn make_from_rotation(
        &mut self,
        kind: i32,
        at: Vec3,
        data: EntityData,
        shooter: &Shooter,
        (yaw, pitch, roll): (f32, f32, f32),
        (power, uncertainty): (f32, f32),
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let kind = u16::try_from(kind).expect("entity type ids fit 16 bits");
        if roll == 0.0 {
            let aim = Aim::Rotation {
                yaw,
                pitch,
                known: shooter.known,
                on_ground: shooter.on_ground,
            };
            let making = Making {
                kind,
                at,
                data,
                aim,
                power,
                uncertainty,
            };
            return self.make_projectile(making, |_, _| {}, output);
        }
        // `shootFromRotation`'s look, its height by the pitch and the roll.
        let x = -table_sin(yaw * RADIANS) * table_cos(pitch * RADIANS);
        let y = -table_sin((pitch + roll) * RADIANS);
        let z = table_cos(yaw * RADIANS) * table_cos(pitch * RADIANS);
        let making = Making {
            kind,
            at,
            data,
            aim: Aim::Direction([f64::from(x), f64::from(y), f64::from(z)]),
            power,
            uncertainty,
        };
        let key = self.make_projectile(making, |_, _| {}, output)?;
        let known = shooter.known;
        let lift = if shooter.on_ground { 0.0 } else { known[1] };
        self.patch_made(
            key,
            |entity| {
                entity.motion = [
                    entity.motion[0] + known[0],
                    entity.motion[1] + lift,
                    entity.motion[2] + known[2],
                ];
            },
            output,
        );
        Some(key)
    }

    /// `applyOnProjectileSpawned`'s `projectile_spawned` effects of `stack` on the
    /// projectile `key`: Flame's fire.
    fn fire_from(&mut self, key: EntityId, stack: &ItemStack, output: &mut TickOutput) {
        let ticks = enchantments::projectile_fire_ticks(stack);
        if ticks > 0 {
            // `igniteForTicks`: set where it burns for less.
            self.patch_made(
                key,
                |entity| entity.fire_ticks = entity.fire_ticks.max(ticks),
                output,
            );
        }
    }

    /// The entity `key` made in this tick changed by `patch`, wherever its making went:
    /// its record in its chunk and the tick's change and spawn, or the arrival sent.
    fn patch_made(&mut self, key: EntityId, patch: impl Fn(&mut Entity), output: &mut TickOutput) {
        for (_, change) in output.entity_changes.iter_mut().rev() {
            let EntityChange::Whole(entity) = change else {
                continue;
            };
            if entity.key != key {
                continue;
            }
            patch(entity);
            let whole = EntityChange::Whole(entity.clone());
            if let Some(loaded) = self.chunks.get_mut(&entity.chunk()) {
                loaded.apply_entity(&whole);
            }
            let state = super::entities::state_of(entity);
            for event in output.events.iter_mut().rev() {
                if let RegionEvent::EntitySpawned(spawned) = event
                    && spawned.entity == key
                {
                    *spawned = state;
                    break;
                }
            }
            return;
        }
        if let Some((_, entity)) = output
            .arrivals
            .iter_mut()
            .rev()
            .find(|(_, entity)| entity.key == key)
        {
            patch(entity);
        }
    }

    /// `Player.getProjectile(weapon)` (n9 Player 4488–4544): the off hand's stack where
    /// the weapon fires it from a hand, else the main hand's, else the first of the
    /// inventory it fires, else an arrow for a creative player; with the slot it is
    /// in, none for the creative player's arrow.
    fn projectile_for(
        &self,
        id: PlayerId,
        weapon: &ItemStack,
    ) -> Option<(Option<usize>, ItemStack)> {
        let player = self.players.get(&id)?;
        let slots = &player.inventory.0;
        for slot in [OFF_HAND, usize::from(player.selected_slot)] {
            if let Some(stack) = &slots[slot]
                && bow::fires_from_hand(weapon.item, stack.item)
            {
                return Some((Some(slot), stack.clone()));
            }
        }
        for (slot, stack) in slots.iter().enumerate() {
            if let Some(stack) = stack
                && bow::fires_from_inventory(stack.item)
            {
                return Some((Some(slot), stack.clone()));
            }
        }
        has_infinite_materials(id).then(|| (None, ItemStack::new(items::ARROW, 1)))
    }

    /// `taken` of the ammunition `ammo` in `slot` split off (`useAmmo`), the slot emptied
    /// where none is left (`Inventory.removeItem`), and the client told.
    fn take_ammunition(
        &mut self,
        id: PlayerId,
        slot: Option<usize>,
        ammo: &ItemStack,
        taken: i32,
        output: &mut TickOutput,
    ) {
        let Some(slot) = slot else {
            return;
        };
        if taken <= 0 {
            return;
        }
        let left = (ammo.count > taken).then(|| ammo.with_count(ammo.count - taken));
        self.set_inventory_slot(id, slot, left, output);
    }

    /// `stack.consume(count, player)` of the stack in `id`'s `hand`: shrunk, but not for a
    /// creative player.
    fn consume_held(
        &mut self,
        id: PlayerId,
        hand: Hand,
        stack: &ItemStack,
        count: i32,
        output: &mut TickOutput,
    ) {
        if has_infinite_materials(id) {
            return;
        }
        let left = (stack.count > count).then(|| stack.with_count(stack.count - count));
        self.set_hand(id, hand, left, output);
    }

    /// The hand of `id` that holds `item`: the main hand where both do, as the client
    /// uses the off hand only where the main hand's use did nothing.
    fn hand_with_item(&self, id: PlayerId, item: i32) -> Option<Hand> {
        [Hand::Main, Hand::Off].into_iter().find(|hand| {
            self.in_hand(id, *hand)
                .is_some_and(|stack| stack.item == item)
        })
    }

    /// The stack in `id`'s `hand` set to `stack`, and their client told.
    fn set_hand(
        &mut self,
        id: PlayerId,
        hand: Hand,
        stack: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let slot = match hand {
            Hand::Main => usize::from(player.selected_slot),
            Hand::Off => OFF_HAND,
        };
        self.set_inventory_slot(id, slot, stack, output);
    }

    /// The inventory's `slot` of `id` set to `stack`, and their client told.
    fn set_inventory_slot(
        &mut self,
        id: PlayerId,
        slot: usize,
        stack: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if !player.inventory.set(slot, stack.clone()) {
            return;
        }
        self.journal.players.insert(id);
        let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
        output
            .player_events
            .push((id, PlayerEvent::InventorySlot { slot, stack }));
    }

    /// Where the player `id` is and looks: their crouch and look as a use gives them, or
    /// as their keys and pose have them.
    fn shooter(&self, id: PlayerId, used: Option<(bool, f32, f32)>) -> Option<Shooter> {
        let player = self.players.get(&id)?;
        let (sneaking, yaw, pitch) =
            used.unwrap_or((player.keys.sneak, player.pose.yaw, player.pose.pitch));
        let eyes = if sneaking && !player.flying {
            CROUCHING_EYES
        } else {
            STANDING_EYES
        };
        let feet = player.pose.position;
        Some(Shooter {
            entity: player.entity_id,
            feet: [feet.x, feet.y, feet.z],
            eyes: f64::from(eyes),
            yaw,
            pitch,
            known: self.player_movement(id),
            on_ground: player.pose.on_ground,
        })
    }
}

impl Shooter {
    /// The shooter's x and z, `below` under their eyes.
    fn below_the_eyes(&self, below: f64) -> Vec3 {
        [self.feet[0], self.feet[1] + self.eyes - below, self.feet[2]]
    }

    /// Their feet, where their sounds play.
    fn feet_vec(&self) -> clustine_world::Vec3 {
        clustine_world::Vec3::new(self.feet[0], self.feet[1], self.feet[2])
    }
}
