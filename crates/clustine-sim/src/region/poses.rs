//! How a player's entity is posed and drawn for the others: the pose the game's
//! `Player.updatePlayerPose` gives it at the end of every tick of the player, the shared
//! flags of swimming (`Entity.updateSwimming`) and gliding (`Player.tryToStartFallFlying`,
//! `LivingEntity.updateFallFlying`) that decide it, and the skin's parts and the main hand
//! the client asks for (`ServerPlayer.updateOptions`). Each value is reported to the
//! others when it changes, as `SynchedEntityData.set` marks only a value that differs,
//! and every report of the player's entity carries those that are not the defaults, as
//! `ServerEntity.sendPairingData` sends them to whoever comes to see the player.
//!
//! The player's own client is not sent them: the game sends a player's dirty data to the
//! player too (`ServerEntity.sendDirtyEntityData` 44, `sendToTrackingPlayersAndSelf`),
//! but their client poses itself every tick from the same rules (`Player.tick` 335) and
//! keeps its own options, so that nothing it shows changes by it. Named: a start of
//! gliding the region refuses is therefore not undone on the client that asked
//! (`stopFallFlying` sets the flag twice to send it), which checks the same conditions
//! itself before it asks.
//!
//! As in the game, a creative player's elytra does not wear as they glide
//! (`ItemStack.processDurabilityChange` 9–22 spares a player with infinite materials);
//! anyone else's is worn by one every twentieth tick of a glide (n9
//! LivingEntity.updateFallFlying @33-102). Named: the game's choice among the slots that
//! glide (`Util.getRandom`, a draw of the player's random) is drawn for a player whose
//! elytra wears, not for a creative player's, whose wear is nothing.
//!
//! Named differences beside: Riptide is not built, so nothing begins a spin
//! (`isAutoSpinAttack`) and the pose `SPIN_ATTACK` is never taken; a dead player, and one
//! watching the credits, is not posed (the game's `DYING`, which no dying mob is shown
//! either); whether a pose fits does not meet the world border or the shape a scaffolding
//! has for one who sneaks on it (the collision context of an entity that descends); a
//! glider is only the elytra, worn on the chest (the game's `GLIDER` component, which
//! commands can put on other items, is not read); a start of gliding judges the fluids
//! at the player's position then, where the game reads its last tick's fluid update; and
//! what the last fluid update found of the eyes, and the ticks of a glide, are kept in
//! memory only, so that after a hand-over or a restore a swim begins a tick later at most
//! and the glide's vibrations come at other ticks.

use clustine_data::{Face, SupportType, items};
use clustine_world::{BlockPos, PlayerId};

use super::player_effects::{EFFECT_AMBIENCE, EFFECT_PARTICLES, SHARED_FLAGS};
use super::using::{LIVING_FLAGS, living_flags};
use super::vibrations::{Context as Heard, Source, Vibration};
use super::{Player, Region};
use crate::api::{Appearance, RegionEvent, TickOutput};
use crate::mobs::body::effects::{self, Effect};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};
use crate::physics::entities::Mover;
use crate::physics::{self, Rules, World};
use crate::shown::{DataShown, Serialiser};
use crate::state::PlayerPose;

/// `Entity.DATA_POSE`.
const POSE: u8 = 6;
/// `LivingEntity.SLEEPING_POS_ID`.
const SLEEPING_POSITION: u8 = 14;
/// `Avatar.DATA_PLAYER_MAIN_HAND` and `DATA_PLAYER_MODE_CUSTOMISATION`, the two values
/// `Avatar.defineSynchedData` (5–27) defines after the living entity's fourteen.
const MAIN_HAND: u8 = 15;
const SKIN_PARTS: u8 = 16;
/// `HumanoidArm.LEFT`'s id, which its serializer writes (the right is 1).
const LEFT_ARM: i32 = 0;
const RIGHT_ARM: i32 = 1;

/// The shared flags' bits (`Entity.FLAG_*`).
const CROUCHING: u8 = 1 << 1;
const SPRINTING: u8 = 1 << 3;
const SWIMMING: u8 = 1 << 4;
const INVISIBLE: u8 = 1 << 5;
const GLOWING: u8 = 1 << 6;
const FALL_FLYING: u8 = 1 << 7;

/// What the region keeps of a player between their ticks for their pose, in memory only
/// (the module's comment).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Eyes {
    /// Whether the last fluid update found the eyes in water (`isEyeInFluid(#water)`),
    /// which the next tick's `baseTick` takes as `wasEyeInWater`.
    in_water: bool,
    /// `LivingEntity.fallFlyTicks`: the ticks the player has glided.
    fall_fly_ticks: u32,
}

impl Eyes {
    /// `isEyeInFluid(#water)` as the last fluid update left it, which a player's dig
    /// reads (`digging.rs`).
    pub(super) fn in_water(&self) -> bool {
        self.in_water
    }
}

/// The width, height and eye height of a player in `pose` (`Avatar.POSES`, its static
/// initialiser 21–174; the sleeping pose's are `LivingEntity.SLEEPING_DIMENSIONS`).
fn dimensions(pose: PlayerPose) -> (f32, f32, f32) {
    match pose {
        PlayerPose::Standing => (0.6, 1.8, 1.62),
        PlayerPose::Crouching => (0.6, 1.5, 1.27),
        PlayerPose::FallFlying | PlayerPose::Swimming | PlayerPose::SpinAttack => (0.6, 0.6, 0.4),
        PlayerPose::Sleeping => (0.2, 0.2, 0.2),
    }
}

/// What a fluid update finds of a player: whether water and lava reach their box, and
/// whether water reaches their eyes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Wet {
    in_water: bool,
    in_lava: bool,
    eyes_in_water: bool,
}

impl Player {
    /// The player's box in `pose` where they stand now (`EntityDimensions.makeBoundingBox`).
    fn box_in(&self, pose: PlayerPose) -> Aabb {
        let (width, height, _) = dimensions(pose);
        let at = self.pose.position;
        Aabb::of_entity([at.x, at.y, at.z], width, height)
    }

    /// The player's shared flags as the others are shown them, whole
    /// (`docs/groundwork/n9-plan.md`, section 18, decision 6): crouching and sprinting
    /// by their keys, swimming and gliding as the region decided them, invisible and
    /// glowing by their effects.
    pub(super) fn shared_flags(&self) -> u8 {
        let mut flags = 0;
        if self.keys.sneak && !self.flying {
            flags |= CROUCHING;
        }
        if self.keys.sprint {
            flags |= SPRINTING;
        }
        if self.posture.swimming {
            flags |= SWIMMING;
        }
        if self.effects.has(Effect::INVISIBILITY) {
            flags |= INVISIBLE;
        }
        if self.effects.has(Effect::GLOWING) {
            flags |= GLOWING;
        }
        if self.posture.fall_flying {
            flags |= FALL_FLYING;
        }
        flags
    }

    /// The values of the player's entity data that are not the defaults, by index, as
    /// someone who comes to see them is sent them (`SynchedEntityData.getNonDefaultValues`
    /// in `ServerEntity.sendPairingData`).
    pub(super) fn data_shown(&self) -> Vec<(u8, DataShown)> {
        let mut values = Vec::new();
        let flags = self.shared_flags();
        if flags != 0 {
            values.push((SHARED_FLAGS, DataShown::Byte(flags)));
        }
        if self.posture.pose != PlayerPose::Standing {
            values.push((POSE, DataShown::Pose(self.posture.pose.ordinal())));
        }
        let living = living_flags(self.using);
        if living != 0 {
            values.push((LIVING_FLAGS, DataShown::Byte(living)));
        }
        let (particles, ambient) = effects::particles(&self.effects);
        if !particles.is_empty() {
            values.push((EFFECT_PARTICLES, DataShown::Particles(particles)));
        }
        if ambient {
            values.push((EFFECT_AMBIENCE, DataShown::Boolean(true)));
        }
        if let Some(sleep) = self.sleeping {
            values.push((
                SLEEPING_POSITION,
                DataShown::OptionalBlockPos(Some(sleep.head)),
            ));
        }
        if self.appearance.left_handed {
            values.push((
                MAIN_HAND,
                DataShown::Serialised(Serialiser::HumanoidArm, LEFT_ARM),
            ));
        }
        if self.appearance.skin_parts != 0 {
            values.push((SKIN_PARTS, DataShown::Byte(self.appearance.skin_parts)));
        }
        values
    }

    /// `Player.canGlide`: not flying, in the air, riding nothing, not levitating, and
    /// an elytra on the chest that the next wear would not break
    /// (`LivingEntity.canGlide` 0–72, `canGlideUsing`).
    fn can_glide(&self) -> bool {
        if self.flying
            || self.pose.on_ground
            || self.riding.is_some()
            || self.effects.has(Effect::LEVITATION)
        {
            return false;
        }
        self.inventory.0[clustine_world::ARMOUR_START + 2]
            .as_ref()
            .is_some_and(|chest| {
                chest.item == items::ELYTRA
                    && !crate::items::enchantments::next_damage_will_break(chest)
            })
    }

    /// `Player.getDesiredPose`: asleep, swimming, gliding, spinning, sneaking (unless
    /// flying), or standing.
    fn desired_pose(&self) -> PlayerPose {
        if self.sleeping.is_some() {
            PlayerPose::Sleeping
        } else if self.posture.swimming {
            PlayerPose::Swimming
        } else if self.posture.fall_flying {
            PlayerPose::FallFlying
        } else if self.keys.sneak && !self.flying {
            // `isAutoSpinAttack` would come before: Riptide is not built.
            PlayerPose::Crouching
        } else {
            PlayerPose::Standing
        }
    }
}

impl Region {
    /// Every player's swimming, gliding and pose at the end of their tick, in the order
    /// of their ids, each reported to the others where it changed.
    pub(super) fn players_pose_tick(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            self.pose_player(id, output);
        }
    }

    /// The parts of the game's `Player.tick` of a server's player that pose them:
    /// `Entity.baseTick`'s fluid update and `updateSwimming` (Entity 88–105),
    /// `LivingEntity.aiStep`'s `updateFallFlying` (486–494), the count of the glide's
    /// ticks (`LivingEntity.tick` 601–623), and `updatePlayerPose` last (`Player.tick`
    /// 335).
    fn pose_player(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.dead || player.in_credits {
            return;
        }
        let before = player.posture;
        let was_eye_in_water = player.eyes.in_water;
        let wet = self.wet(player);
        let feet_in_water = {
            let view = super::items::View {
                region: self,
                home: player.chunk(),
            };
            let feet = player.pose.position;
            let block = BlockPos::new(
                physics::floor(feet.x),
                physics::floor(feet.y),
                physics::floor(feet.z),
            );
            clustine_blocks::data::has_water(view.state(block))
        };
        let can_glide = player.can_glide();
        let swim_fits = self.fits(player, PlayerPose::Swimming);
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.eyes.in_water = wet.eyes_in_water;
        // `Player.updateSwimming`: never while flying; else `Entity.updateSwimming`, which
        // keeps a swim while the player sprints in water, and begins one where they
        // sprint under water with water at their feet' block (Entity 4240–4290;
        // `isUnderWater` is the eyes of the update before and the box of this one).
        let passenger = player.riding.is_some();
        let sprinting = player.keys.sprint;
        player.posture.swimming = if player.flying {
            false
        } else if player.posture.swimming {
            sprinting && wet.in_water && !passenger
        } else {
            sprinting && was_eye_in_water && wet.in_water && !passenger && feet_in_water
        };
        // `updateFallFlying`, while gliding: it ends where the player cannot glide; every
        // tenth tick of a glide is heard (`ELYTRA_GLIDE`).
        let mut glide_heard = false;
        let mut glide_wears = false;
        if player.posture.fall_flying {
            if can_glide {
                let ticks = player.eyes.fall_fly_ticks + 1;
                glide_heard = ticks % 10 == 0;
                glide_wears = glide_heard && (ticks / 10) % 2 == 0;
            } else {
                player.posture.fall_flying = false;
            }
        }
        player.eyes.fall_fly_ticks = if player.posture.fall_flying {
            player.eyes.fall_fly_ticks.saturating_add(1)
        } else {
            0
        };
        let (entity, feet) = (player.entity_id, player.pose.position);
        if glide_wears && !self.has_infinite_materials(id) {
            self.glider_worn(id, output);
        }
        if glide_heard && let Some(glide) = Vibration::named("minecraft:elytra_glide") {
            let heard = Heard::of(Some(Source::of(entity)), None);
            self.vibrate(glide, [feet.x, feet.y, feet.z], heard);
        }
        // `Player.updatePlayerPose` (0–66): nothing where even swimming does not fit;
        // the pose the player wants where it fits, or they are a passenger; else
        // crouching where that fits, and swimming (crawling) where not.
        if swim_fits {
            let Some(player) = self.players.get(&id) else {
                return;
            };
            let desired = player.desired_pose();
            let pose = if player.riding.is_some() || self.fits(player, desired) {
                desired
            } else if self.fits(player, PlayerPose::Crouching) {
                PlayerPose::Crouching
            } else {
                PlayerPose::Swimming
            };
            if let Some(player) = self.players.get_mut(&id) {
                player.posture.pose = pose;
            }
        }
        self.show_posture(id, before, output);
    }

    /// Reports what changed of the player `id`'s posture since `before`: the shared
    /// flags where swimming or gliding changed, the pose where it did, in one report as
    /// the game's next `sendDirtyEntityData` packs them.
    fn show_posture(
        &mut self,
        id: PlayerId,
        before: crate::state::Posture,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let now = player.posture;
        if now == before {
            return;
        }
        let mut values = Vec::new();
        if (now.swimming, now.fall_flying) != (before.swimming, before.fall_flying) {
            values.push((SHARED_FLAGS, DataShown::Byte(player.shared_flags())));
        }
        if now.pose != before.pose {
            values.push((POSE, DataShown::Pose(now.pose.ordinal())));
        }
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values,
        });
        self.journal.players.insert(id);
    }

    /// `handlePlayerCommand`'s `START_FALL_FLYING` (ServerGamePacketListenerImpl
    /// 245–262): `Player.tryToStartFallFlying` (0–26) lets the player glide where they
    /// do not already, can, and are in no fluid; else `stopFallFlying` (LivingEntity
    /// 6850) ends a glide, setting the flag on and off so that it is sent whatever it
    /// was.
    pub(super) fn start_fall_flying(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let wet = self.wet(player);
        let starts =
            !player.posture.fall_flying && player.can_glide() && !wet.in_water && !wet.in_lava;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.posture.fall_flying = starts;
        self.journal.players.insert(id);
        self.show_shared_flags(id, output);
    }

    /// `ServerPlayer.updateOptions` (56–85): the skin's parts and the main hand, each
    /// reported where it changed.
    pub(super) fn set_appearance(
        &mut self,
        id: PlayerId,
        appearance: Appearance,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let before = player.appearance;
        if before == appearance {
            return;
        }
        player.appearance = appearance;
        let mut values = Vec::new();
        if before.left_handed != appearance.left_handed {
            let arm = if appearance.left_handed {
                LEFT_ARM
            } else {
                RIGHT_ARM
            };
            values.push((
                MAIN_HAND,
                DataShown::Serialised(Serialiser::HumanoidArm, arm),
            ));
        }
        if before.skin_parts != appearance.skin_parts {
            values.push((SKIN_PARTS, DataShown::Byte(appearance.skin_parts)));
        }
        output.events.push(RegionEvent::PlayerData {
            entity: player.entity_id,
            chunk: player.chunk(),
            values,
        });
        self.journal.players.insert(id);
    }

    /// `Player.canPlayerFitWithinBlocksAndEntitiesWhen(pose)` (0–23): the player's box in
    /// `pose`, a little smaller, meets no block and no entity they collide with.
    fn fits(&self, player: &Player, pose: PlayerPose) -> bool {
        let bounds = player.box_in(pose).deflate(1.0e-7);
        let view = super::items::View {
            region: self,
            home: player.chunk(),
        };
        // `EntityCollisionContext` of the player: what is below their feet holds them.
        let context = Context {
            stands_on_lava: false,
            bottom: player.pose.position.y,
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        let mover = Mover {
            key: player.entity_id,
            root: player
                .riding
                .map_or(player.entity_id, |riding| riding.vehicle),
            like_a_vehicle: false,
            happy_ghast: false,
        };
        no_collision(&view, context, &mover, &bounds)
    }

    /// The fluid update of the player's box in their pose now
    /// (`EntityFluidInteraction.update`, n6 0–491): whether water and lava reach it, and
    /// whether water reaches their eyes in their own column, up to its height for the
    /// camera (`FluidState.getHeightForCamera`: a source under a block whose bottom is
    /// sturdy fills its block).
    fn wet(&self, player: &Player) -> Wet {
        let pose = player.posture.pose;
        let bounds = player.box_in(pose);
        let view = super::items::View {
            region: self,
            home: player.chunk(),
        };
        let found = physics::fluids::update(&view, &bounds);
        let mut wet = Wet {
            in_water: found.in_water(),
            in_lava: found.in_lava(),
            eyes_in_water: false,
        };
        if !found.found {
            return wet;
        }
        let (_, _, eye_height) = dimensions(pose);
        let feet = player.pose.position;
        let eye = feet.y + f64::from(eye_height);
        let inner = bounds.deflate(0.001);
        let (x, z) = (physics::floor(feet.x), physics::floor(feet.z));
        let y0 = physics::floor(inner.min[1]);
        let y1 = inner.max[1].ceil() as i32 - 1;
        wet.eyes_in_water = (y0..=y1).any(|y| {
            let pos = BlockPos::new(x, y, z);
            let state = view.state(pos);
            if !clustine_blocks::data::has_water(state) {
                return false;
            }
            let fluid = clustine_blocks::fluid::FluidState::of(state);
            let height = f64::from(clustine_blocks::fluid::height(
                &mut Rules(&view),
                fluid,
                pos,
            ));
            let top = f64::from(y) + height;
            if top < inner.min[1] || eye < f64::from(y) {
                return false;
            }
            let above = view.state(BlockPos::new(x, y + 1, z));
            let camera = if fluid.is_source()
                && clustine_blocks::sturdy(above, Face::Down, SupportType::Full)
            {
                1.0
            } else {
                height
            };
            eye <= f64::from(y) + camera
        });
        wet
    }
}
