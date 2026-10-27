//! A player's throws (`docs/groundwork/n9-plan.md`, section 4.6; part S4's): the
//! snowball today, the egg, the potions, the experience bottle and the ender pearl with
//! S4's `items/throwables.rs`.
//!
//! Named differences:
//! - `SnowballItem` is not in the dumps: its use is inferred from what calls it
//!   (**Inferred**: the throw's sound at the player's feet, of the neutral source, at
//!   0.5 and a pitch of `0.4 / (nextFloat() · 0.4 + 0.8)` of the level's random, then
//!   `spawnProjectileFromRotation(Snowball::new, level, stack, player, 0, 1.5, 1)`, the
//!   stack taken but by a creative player, and no cooldown).

use clustine_data::{entity_types, items, sounds};
use clustine_world::{EntityData, ItemStack, PlayerId};

use super::Region;
use super::projectiles::{Aim, Making};
use crate::api::{RegionEvent, TickOutput};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::thrown;
use crate::placement::ItemUse;

/// How far above a player's feet their eyes are, standing and crouching.
const STANDING_EYES: f32 = 1.62;
const CROUCHING_EYES: f32 = 1.27;

impl Region {
    /// `SnowballItem.use` of the player `player`'s held snowball in the air: the throw's
    /// sound, then the snowball made at their eyes and shot along their look with their
    /// movement (`spawnProjectileFromRotation`); a creative player keeps the stack, and
    /// a snowball has no cooldown. Whether the held item was a snowball.
    pub(crate) fn throw_snowball(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if used.held != Some(items::SNOWBALL) {
            return false;
        }
        let Some(thrower) = self.players.get(&player) else {
            return false;
        };
        let feet = thrower.pose.position;
        let on_ground = thrower.pose.on_ground;
        let owner = thrower.entity_id;
        let eyes = if used.sneaking && !thrower.flying {
            CROUCHING_EYES
        } else {
            STANDING_EYES
        };
        // `level.playSound(null, x, y, z, SNOWBALL_THROW, NEUTRAL, 0.5, pitch)`, the
        // pitch of the level's random, heard by everyone.
        let mut random = self.next_draw();
        let pitch = 0.4 / (random.next_float() * 0.4 + 0.8);
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_SNOWBALL_THROW,
            source: SOUND_SOURCE_NEUTRAL,
            position: feet,
            volume: 0.5,
            pitch,
            except: None,
        });
        let at = [
            feet.x,
            feet.y + f64::from(eyes) - thrown::BELOW_THE_EYES,
            feet.z,
        ];
        let aim = Aim::Rotation {
            yaw: used.yaw,
            pitch: used.pitch,
            known: self.player_movement(player),
            on_ground,
        };
        let making = Making {
            kind: u16::try_from(entity_types::SNOWBALL).expect("entity type ids fit 16 bits"),
            at,
            data: EntityData::Thrown {
                stack: ItemStack::new(items::SNOWBALL, 1),
                owner: Some(owner),
                left_owner: false,
                has_been_shot: false,
            },
            aim,
            power: thrown::PLAYER_POWER,
            uncertainty: thrown::PLAYER_UNCERTAINTY,
        };
        self.make_projectile(making, |_, _| {}, output);
        true
    }
}
