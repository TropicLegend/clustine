//! The eye of ender (`docs/groundwork/p-brief.md`, section 4.3; `docs/groundwork/p-plan.md`,
//! sections 19.2 P10b, 19.5 items 8, 12 and 15): its item used on an end portal frame
//! (`EnderEyeItem.useOn`, d-javap EnderEyeItem 10–235), its item used in the air
//! (`EnderEyeItem.use`, 0–249), and the thrown eye's flight (`EyeOfEnder`, d-javap
//! EyeOfEnder). The rules that read only blocks are `placement/ender_eye.rs`.
//!
//! **On a frame without its eye** the eye is set with flags 2 (the clients told, no
//! neighbour told), what stands in the eye's box is lifted onto it
//! (`Block.pushEntitiesUp`, n6 Block 102–121), the comparators beside it are told, and
//! level event 1503 is played; the stack is shrunk, but a creative player's count is put
//! back (`ServerPlayerGameMode.useItemOn` 350–377), and every player is creative. Then
//! the 5 × 5 ring is looked for from the frame; where it is whole, each of the inner 3 ×
//! 3 is destroyed with its drops and made `end_portal` with flags 2, and the global level
//! event 1038 at its centre goes into the region's report (`VillageReport::
//! global_events`): the store publishes it to every region, and each region tells its
//! own players, this one among them, from what is published (19.5 item 8, 19.6), which
//! is the step that tells the published events, not this one.
//!
//! **In the air**, aimed at a frame, the use passes. Otherwise the nearest of the
//! world's strongholds is looked for as `ChunkGenerator.findNearestMapStructure` looks
//! for a structure placed in rings (n7 ChunkGenerator 153–172 and 358–436): every
//! chunk of the rings in their order, by the square of the distance from the player's
//! block to the chunk's (8, 32, 8), the first nearest kept; the radius of 100 the item
//! passes is read only for structures spread at random, so a ring's stronghold is found
//! however far. It is the chunk's lowest corner at y 0 (`getLocatePos`, the set's
//! `locate_offset` none). The strongholds are the overworld's: another level's
//! generator places none (`#has_structure/stronghold` is of overworld biomes), and the
//! flat world has none, so no eye is thrown there (CONSUME). Found, an eye is made at
//! the player's x and z and half their height (`getY(0.5)`), its UUID then its
//! survival (`nextInt(5) > 0`) drawn from its own stream (`signalTo`), heard
//! (`projectile_shoot`), added, the launch heard at the player's feet with a pitch of
//! the level's random (`lerp(nextFloat, 0.33, 0.5)`), the stack used up but by a
//! creative player (`ItemStack.consume`), and the arm swung for everyone
//! (`SUCCESS_SERVER`, `swingAndResetAttackStrength`).
//!
//! **The eye's tick** (`EyeOfEnder.tick` 0–200): it moves by its motion, its motion is
//! steered towards its target (`updateDeltaMovement`), and one more tick of `life` is
//! counted; past 80 it plays `ender_eye_death` where it is, is gone, and drops its
//! item there (an item with no pick-up delay, `new ItemEntity`) if it survives, else
//! plays level event 2003. Its particles are the client's. The edges send its motion
//! as for any entity of its type (an interval of 4 with its deltas).
//!
//! Named differences: the game's `startUsingItem` at the start of the use in the air
//! is left out (an eye's use lasts no tick and shows nothing, the living flag of using
//! aside, and its `item_interact_start` is not heard); `pushEntitiesUp` lifts the
//! region's players (a teleport their client is told) and the entities of its chunks
//! that ride nothing, carry nobody and are held by no leash, where the game lifts any;
//! an eye's `life` is its `lived`, and its target is kept with it, where the game saves
//! neither and an eye reloaded flies on without steering for another 80 ticks; an eye
//! does not burn, is not pushed by water nor by explosions; an eye exactly above its
//! target (no horizontal distance) holds its course, where the game's divides by
//! nothing.

use clustine_blocks::data::{flag, is_block, set_flag};
use clustine_data::{ItemComponents, blocks, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Dimension, Double, Entity, EntityData, EyeOfEnder, ItemStack, PlayerId,
};

use super::Region;
use super::entities::insecure_uuid;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{GlobalLevelEvent, Hand, PlayerEvent, RegionEvent, TickOutput, VillageReport};
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::placement::ender_eye::{aims_at_frame, find_portal_ring, portal_inside};
use crate::placement::{Click, ItemUse};

/// The level event of an eye set in a frame (`levelEvent(1503)`).
const EYE_SET: i32 = 1503;
/// The global level event of an End portal opened (`globalLevelEvent(1038)`).
const PORTAL_OPENED: i32 = 1038;
/// The level event of an eye that shatters at the end of its flight (`levelEvent(2003)`).
const EYE_SHATTERS: i32 = 2003;

/// `EyeOfEnder`'s `TOO_FAR_DISTANCE` and `TOO_FAR_SIGNAL_HEIGHT`: a target further than
/// this across is replaced by one this far towards it and this high above the eye.
const TOO_FAR_DISTANCE: f64 = 12.0;
const TOO_FAR_SIGNAL_HEIGHT: f64 = 8.0;

/// The ticks of `life` an eye flies for: it ends in the tick its `life` passes it.
const LIFE: u32 = 80;

/// A player's box, standing and crouching (`Player.STANDING_DIMENSIONS`, `CROUCHING`).
const PLAYER_WIDTH: f32 = 0.6;
const STANDING_HEIGHT: f32 = 1.8;
const CROUCHING_HEIGHT: f32 = 1.5;

/// The frame's eye, the part of its collision shape that an eye adds
/// (`EndPortalFrameBlock.SHAPE_FULL` less `SHAPE_EMPTY`: `column(8, 13, 16)`), as
/// offsets within the block.
const EYE_SHAPE: Aabb = Aabb {
    min: [0.25, 0.8125, 0.25],
    max: [0.75, 1.0, 0.75],
};

/// The point a ring's stronghold is found at, from the chunks `strongholds` of the rings
/// in their order, for a player whose block is `from` (`getNearestGeneratedStructure` of
/// a `ConcentricRingsStructurePlacement`): the chunk whose (8, 32, 8) is nearest, the
/// first of equals, as its lowest corner at y 0.
pub(crate) fn nearest_stronghold(strongholds: &[ChunkPos], from: BlockPos) -> Option<BlockPos> {
    let mut best: Option<(ChunkPos, f64)> = None;
    for chunk in strongholds {
        // `SectionPos.sectionToBlockCoord(x, 8)`, 32, and `Vec3i.distSqr` in doubles.
        let dx = f64::from(chunk.x) * 16.0 + 8.0 - f64::from(from.x);
        let dy = 32.0 - f64::from(from.y);
        let dz = f64::from(chunk.z) * 16.0 + 8.0 - f64::from(from.z);
        let distance = dx * dx + dy * dy + dz * dz;
        if best.is_none_or(|(_, nearest)| distance < nearest) {
            best = Some((*chunk, distance));
        }
    }
    best.map(|(chunk, _)| BlockPos::new(chunk.x * 16, 0, chunk.z * 16))
}

/// `EyeOfEnder.signalTo(point)` of an eye at `at`: a point further than 12 across is
/// replaced by the one 12 towards it and 8 above the eye.
pub(crate) fn signal_to(at: Vec3, point: Vec3) -> Vec3 {
    let delta = [point[0] - at[0], point[1] - at[1], point[2] - at[2]];
    let across = (delta[0] * delta[0] + delta[2] * delta[2]).sqrt();
    if across > TOO_FAR_DISTANCE {
        [
            at[0] + delta[0] / across * TOO_FAR_DISTANCE,
            at[1] + TOO_FAR_SIGNAL_HEIGHT,
            at[2] + delta[2] / across * TOO_FAR_DISTANCE,
        ]
    } else {
        point
    }
}

/// `EyeOfEnder.updateDeltaMovement(motion, position, target)`: the speed across eases
/// towards the distance across, by 0.0025 of the difference, both and the rise slowed
/// to 0.8 within a block across; across it heads straight at the target, and up or
/// down it eases by 0.015 towards 1 or -1 by whether the eye was below the target
/// before its last move.
pub(crate) fn steer(motion: Vec3, position: Vec3, target: Vec3) -> Vec3 {
    let towards = [target[0] - position[0], 0.0, target[2] - position[2]];
    let length =
        (towards[0] * towards[0] + towards[1] * towards[1] + towards[2] * towards[2]).sqrt();
    let speed = (motion[0] * motion[0] + motion[2] * motion[2]).sqrt();
    // `Mth.lerp(0.0025, speed, length)`.
    let mut wanted = speed + 0.0025 * (length - speed);
    let mut rise = motion[1];
    if length < 1.0 {
        wanted *= 0.8;
        rise *= 0.8;
    }
    let up = if position[1] - motion[1] < target[1] {
        1.0
    } else {
        -1.0
    };
    // The game divides by the length; at none it would have no course (the module's
    // note).
    let scale = if length > 0.0 { wanted / length } else { 0.0 };
    [
        towards[0] * scale + 0.0,
        towards[1] * scale + (rise + (up - rise) * 0.015),
        towards[2] * scale + 0.0,
    ]
}

impl Region {
    /// `EnderEyeItem.useOn` of the eye in the hand of `click` on its block, which the
    /// region holds, by the player `player`: whether it did anything (a frame without
    /// its eye), as the module's comment says.
    pub(crate) fn ender_eye_used_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held != Some(items::ENDER_EYE) {
            return false;
        }
        let pos = click.position;
        let Some(old) = self.block(pos) else {
            return false;
        };
        if !is_block(old, blocks::END_PORTAL_FRAME) || flag(old, "eye") {
            return false;
        }
        let new = set_flag(old, "eye", true);
        self.push_up_onto_the_eye(pos, output);
        self.set_block_updated(pos, new, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        self.update_neighbour_for_output_signal(pos, new, output);
        // `getItemInHand().shrink(1)`, put back for a creative player, as every player
        // is: nothing is taken.
        let _ = player;
        output.events.push(RegionEvent::LevelEvent {
            kind: EYE_SET,
            position: pos,
            data: 0,
        });
        let Some(front_top_left) = find_portal_ring(pos, |at| self.block(at)) else {
            return true;
        };
        let (inside, centre) = portal_inside(front_top_left);
        for cell in inside {
            // `destroyBlock(cell, true, null)`, then the portal with flags 2.
            self.destroy_block(cell, true, UPDATE_LIMIT, output);
            self.set_block_updated(
                cell,
                blocks::END_PORTAL,
                UPDATE_CLIENTS,
                UPDATE_LIMIT,
                output,
            );
        }
        let opened = GlobalLevelEvent {
            event: PORTAL_OPENED,
            dimension: self.config.dimension,
            position: centre,
            data: 0,
        };
        output
            .village
            .get_or_insert_with(VillageReport::default)
            .global_events
            .push(opened);
        true
    }

    /// `Block.pushEntitiesUp` of the eye set in the frame at `pos`: what stands in the
    /// eye's box is lifted to stand on it (`teleportRelative(0, 1 + collide, 0)`, the
    /// box raised a block and let down onto the eye), the region's players by a
    /// teleport, the free entities of its chunks by their records.
    fn push_up_onto_the_eye(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let origin = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        let eye = EYE_SHAPE.moved(origin);
        let top = eye.max[1];
        // `Shapes.collide(Y, box.move(0, 1, 0), [eye], -1)`: the eye stops the box only
        // where its top is not above the box's raised bottom.
        let lift = |bottom: f64| {
            let raised = bottom + 1.0;
            let collide = if top - 1.0e-7 <= raised {
                (top - raised).max(-1.0)
            } else {
                -1.0
            };
            1.0 + collide
        };
        let players: Vec<(PlayerId, f64)> = self
            .players
            .iter()
            .filter_map(|(id, player)| {
                let height = if player.keys.sneak && !player.flying {
                    CROUCHING_HEIGHT
                } else {
                    STANDING_HEIGHT
                };
                let feet = player.pose.position;
                let bounds = Aabb::of_entity([feet.x, feet.y, feet.z], PLAYER_WIDTH, height);
                bounds.intersects(eye).then(|| (*id, lift(feet.y)))
            })
            .collect();
        for (id, by) in players {
            if by == 0.0 {
                continue;
            }
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let previous_chunk = player.chunk();
            player.pose.position.y += by;
            let (entity, pose) = (player.entity_id, player.pose);
            self.journal.players.insert(id);
            output.events.push(RegionEvent::EntityMoved {
                entity,
                pose,
                previous_chunk,
            });
            output
                .player_events
                .push((id, PlayerEvent::Teleported { pose }));
        }
        for (key, chunk) in self.entities_in(&eye) {
            if !self.ticks_chunk(chunk) {
                continue;
            }
            let Some(before) = self.entity_record(key, chunk).cloned() else {
                continue;
            };
            let by = lift(before.position[1]);
            if !before.riding.is_free() || by == 0.0 {
                continue;
            }
            let mut after = before.clone();
            after.position[1] += by;
            self.write_projectile(&before, after, chunk, output);
        }
    }

    /// `EnderEyeItem.use` of the eye in the player `player`'s hand in the air: whether it
    /// did anything, as the module's comment says (a use aimed at a frame passes).
    pub(crate) fn ender_eye_used(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if used.held != Some(items::ENDER_EYE) {
            return false;
        }
        if aims_at_frame(used, &super::Readable(self)) {
            return false;
        }
        // The hand the client used it with: the main hand where it holds an eye, as it
        // uses the off hand only where the main hand's use did nothing.
        let hand = [Hand::Main, Hand::Off].into_iter().find(|hand| {
            self.in_hand(player, *hand)
                .is_some_and(|stack| stack.item == items::ENDER_EYE)
        });
        let (Some(hand), Some(own)) = (hand, self.players.get(&player)) else {
            return false;
        };
        let Some(stack) = self.in_hand(player, hand) else {
            return false;
        };
        let feet = own.pose.position;
        let height = if used.sneaking && !own.flying {
            CROUCHING_HEIGHT
        } else {
            STANDING_HEIGHT
        };
        let entity = own.entity_id;
        let block = BlockPos::new(
            feet.x.floor() as i32,
            feet.y.floor() as i32,
            feet.z.floor() as i32,
        );
        let found = if self.config.dimension == Dimension::Overworld {
            nearest_stronghold(&self.config.strongholds, block)
        } else {
            None
        };
        // `CONSUME`: nothing found, nothing thrown, no swing.
        let Some(found) = found else {
            return true;
        };
        // `getY(0.5)`: half the player's height above their feet.
        let at = [feet.x, feet.y + f64::from(height) * 0.5, feet.z];
        let point = [f64::from(found.x), f64::from(found.y), f64::from(found.z)];
        self.make_eye(at, point, &stack, entity, output);
        // `level.playSound(null, x, y, z, ENDER_EYE_LAUNCH, NEUTRAL, 1, lerp(r, 0.33,
        // 0.5))`, heard by everyone.
        let r = self.next_draw().next_float();
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_ENDER_EYE_LAUNCH,
            source: SOUND_SOURCE_NEUTRAL,
            position: feet,
            volume: 1.0,
            pitch: 0.33 + r * (0.5 - 0.33),
            except: None,
        });
        // `stack.consume(1, player)`: a creative player keeps it, as every player does.
        let swing = ItemComponents::of(items::ENDER_EYE).attack_animation;
        self.swing_arm(player, hand == Hand::Off, swing, true, output);
        self.reset_attack_strength(player);
        true
    }

    /// `new EyeOfEnder(level, x, y, z)` at `at`, `setItem` one of `stack`,
    /// `signalTo(point)`, the `projectile_shoot` of the thrower `thrower` and
    /// `addFreshEntity`: made in the region's own land, sent to the holder of a chunk it
    /// copies, and not made where it can read nothing (`projectiles.rs`' rule).
    fn make_eye(
        &mut self,
        at: Vec3,
        point: Vec3,
        stack: &ItemStack,
        thrower: clustine_world::EntityId,
        output: &mut TickOutput,
    ) {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return;
        };
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let target = signal_to(at, point);
        let survives = own.next_int(5) > 0;
        let entity = Entity {
            portal: Default::default(),
            key,
            kind: u16::try_from(entity_types::EYE_OF_ENDER).expect("entity type ids fit 16 bits"),
            uuid,
            position: at,
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::EyeOfEnder(Box::new(EyeOfEnder {
                // `setItem`: one of the stack, the default eye for an empty one.
                item: if stack.count > 0 {
                    stack.with_count(1)
                } else {
                    ItemStack::new(items::ENDER_EYE, 1)
                },
                target: target.map(Double),
                survives,
            })),
        };
        self.vibrate(
            Vibration::PROJECTILE_SHOOT,
            at,
            Context::of(Some(Source::of(thrower)), None),
        );
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// The tick of a thrown eye of ender (`EyeOfEnder.tick`), as the module's comment
    /// says; it commits through `write_projectile`.
    pub(crate) fn tick_eye_of_ender(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::EyeOfEnder(eye) = &entity.data else {
            return;
        };
        let target = eye.target.map(|value| value.0);
        let moved = [
            entity.position[0] + entity.motion[0],
            entity.position[1] + entity.motion[1],
            entity.position[2] + entity.motion[2],
        ];
        let motion = steer(entity.motion, moved, target);
        // `++life > 80`: `life` counted from 0 at its making, as `lived` is.
        if entity.lived.saturating_add(1) > LIFE {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_ENDER_EYE_DEATH,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(moved[0], moved[1], moved[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
            let (item, survives) = (eye.item.clone(), eye.survives);
            self.remove_entity(entity.key, entity.hops, position, output);
            if survives {
                // `new ItemEntity(level, x, y, z, item)`: no pick-up delay.
                self.spawn_item_as(moved, item, 0, None, output);
            } else {
                output.events.push(RegionEvent::LevelEvent {
                    kind: EYE_SHATTERS,
                    position: BlockPos::new(
                        moved[0].floor() as i32,
                        moved[1].floor() as i32,
                        moved[2].floor() as i32,
                    ),
                    data: 0,
                });
            }
            return;
        }
        let after = Entity {
            position: moved,
            motion,
            lived: entity.lived.saturating_add(1),
            ..entity.clone()
        };
        self.write_projectile(&entity, after, position, output);
    }
}

#[cfg(test)]
#[path = "eye_of_ender_tests.rs"]
mod tests;
