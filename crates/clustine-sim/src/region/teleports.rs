//! `randomTeleport` and what comes of a teleport (`docs/groundwork/n9-plan.md`, sections
//! 4.4, 5.5 and 8.4; n9 LivingEntity 10092–10250, TeleportRandomlyConsumeEffect,
//! ThrownEnderpearl): the enderman's, the chorus fruit's, and the endermite of an ender
//! pearl.
//!
//! `randomTeleport(x, y, z, broadcast, refuse)` draws nothing: from the block at the place
//! asked it goes down, while above the world's floor, to the first block of
//! `#entities_can_teleport_to`, its feet as many blocks lower as it went; that block must
//! not be one `refuse` holds, its box there must meet no block and no entity that
//! collides, hold no liquid and no block `refuse` holds, and its type must take the place
//! (`canRandomlyTeleportTo`: an enderman no water below). A place whose chunk the region
//! can read nothing of fails, as the game's fails at an unloaded one (named: an
//! enderman's 32 blocks reach past the copies, so it teleports less often near a line).
//!
//! Named: the world border is Clustine's none; the level events 2018 and 2017 of a
//! teleport's trail, whose data (`BlockUtil.clampedPackDifferenceInPosition`) is not
//! dumped, are not sent, the entity event 46 at the place reached being; a player's
//! `resetCurrentImpulseContext` is the client's.

use clustine_blocks::data::in_tag;
use clustine_data::{BlockState, EntityTypeInfo, entity_types, sounds};
use clustine_world::{ChunkPos, PlayerId};

use super::items::View;
use super::{Random, Region};
use crate::api::{PlayerEvent, RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{self, Vec3, World};

/// What a teleport is of: the box it moves, who moves (whose own box does not stop it),
/// the tag of blocks it refuses, and what its type asks of the block below the place
/// (`canRandomlyTeleportTo`).
pub(crate) struct Teleporter<'a> {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) mover: &'a Mover,
    /// The tag `refuse` holds (`#enderman_does_not_teleport_to`,
    /// `#consumable_does_not_teleport_to`).
    pub(crate) refuse: &'static str,
    /// `canRandomlyTeleportTo(x, y, z)` of its type, of the block below the place:
    /// `LivingEntity`'s takes any place.
    pub(crate) takes: fn(BlockState) -> bool,
}

/// `LivingEntity.canRandomlyTeleportTo`: any place.
pub(crate) fn any_place(_: BlockState) -> bool {
    true
}

/// `LivingEntity.randomTeleport(x, y, z, broadcast, refuse)`'s search: the place reached,
/// or `None` where it fails (see the module's comment). It draws nothing; the teleport
/// itself is the caller's (`teleportTo`, the entity event 46 where it broadcasts, a
/// mob's navigation stopped).
pub(crate) fn random_teleport<W: Holds + ?Sized>(
    world: &W,
    to: Vec3,
    who: &Teleporter<'_>,
) -> Option<Vec3> {
    let [x, mut y, z] = to;
    let mut pos = physics::containing(to);
    if !world.has_chunk(ChunkPos::containing(x, z)) {
        return None;
    }
    let floor = world.min_y();
    while pos.y > floor {
        pos.y -= 1;
        let below = world.state(pos);
        if in_tag(below, "minecraft:entities_can_teleport_to") {
            return check_position(world, [x, y, z], who, below);
        }
        y -= 1.0;
    }
    None
}

/// `checkPositionAndTeleport(x, y, z, broadcast, refuse, below, level)` as far as it
/// judges: the block below not refused, the box free of collisions, liquids and refused
/// blocks, and the type taking the place.
fn check_position<W: Holds + ?Sized>(
    world: &W,
    at: Vec3,
    who: &Teleporter<'_>,
    below: BlockState,
) -> Option<Vec3> {
    if in_tag(below, who.refuse) {
        return None;
    }
    let bounds = Aabb::of_entity(at, who.width, who.height);
    let context = Context {
        bottom: at[1],
        fall_distance: 0.0,
        falling_block: false,
        noclip: None,
    };
    if !no_collision(world, context, who.mover, &bounds)
        || blocks_in(world, &bounds, |state| {
            clustine_blocks::fluid::FluidState::of(state)
                .kind()
                .is_some()
        })
    {
        return None;
    }
    if blocks_in(world, &bounds, |state| in_tag(state, who.refuse)) {
        return None;
    }
    let below = physics::containing(at);
    let below = world.state(clustine_world::BlockPos::new(below.x, below.y - 1, below.z));
    (who.takes)(below).then_some(at)
}

/// Whether a block the box reaches into holds `test` (`containsAnyLiquid` and
/// `findBlocksIn`, read as the blocks from the floor of the box's least corner to the
/// ceiling of its greatest less one).
fn blocks_in<W: World + ?Sized>(
    world: &W,
    bounds: &Aabb,
    test: impl Fn(BlockState) -> bool,
) -> bool {
    let low = physics::containing(bounds.min);
    let high = [
        bounds.max[0].ceil() as i32 - 1,
        bounds.max[1].ceil() as i32 - 1,
        bounds.max[2].ceil() as i32 - 1,
    ];
    for x in low.x..=high[0] {
        for y in low.y..=high[1] {
            for z in low.z..=high[2] {
                if test(world.state(clustine_world::BlockPos::new(x, y, z))) {
                    return true;
                }
            }
        }
    }
    false
}

/// The player's box: as wide as `Player.STANDING_DIMENSIONS`, as high standing or
/// crouching.
fn player_box(sneaking: bool) -> (f32, f32) {
    if sneaking { (0.6, 1.5) } else { (0.6, 1.8) }
}

impl Region {
    /// The endermite an ender pearl of the player `id` made, at their feet and with their
    /// look, before they are moved (`ThrownEnderpearl.onHit`): constructed of its own
    /// stream (`EntityType.create`, `TRIGGERED`, no `finalizeSpawn`), by section 4.1's
    /// rule.
    pub(crate) fn pearl_endermite(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let pose = player.pose;
        let at = [pose.position.x, pose.position.y, pose.position.z];
        let kind = entity_types::ENDERMITE as u16;
        let mut prepare = |_: &mut Region, _: &mut Mob, _: &mut Random| {};
        self.make_mob(kind, at, (pose.yaw, pose.pitch), &mut prepare, output);
    }

    /// The chorus fruit's `teleport_randomly` of the player `id`
    /// (`TeleportRandomlyConsumeEffect.apply`): up to 16 tries of `randomTeleport` within
    /// `diameter` of their feet, each try's draws from `random` (the player's stream
    /// aside), its height held to the level's logical height ([`Region::chorus_heights`]),
    /// the first that lands moving
    /// them with the teleport's sound and the player's fall reset. Whether they were moved.
    /// Part S9's; S1's eating calls it.
    ///
    /// A rider is taken off before each try, as the game's `stopRiding` comes before it.
    pub(crate) fn chorus_teleport(
        &mut self,
        id: PlayerId,
        diameter: f64,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let diameter = f64::from(diameter as f32);
        for _ in 0..16 {
            let Some(player) = self.players.get(&id) else {
                return false;
            };
            let from = player.pose.position;
            let x = from.x + (random.next_double() - 0.5) * diameter;
            let (low, high) = self.chorus_heights();
            let y = (from.y + (random.next_double() - 0.5) * diameter).clamp(low, high);
            let z = from.z + (random.next_double() - 0.5) * diameter;
            if player.riding.is_some() {
                self.dismount_player(id, output);
            }
            let Some(player) = self.players.get(&id) else {
                return false;
            };
            let mover = Mover {
                key: player.entity_id,
                root: player.entity_id,
                like_a_vehicle: false,
                happy_ghast: false,
            };
            let (width, height) = player_box(player.keys.sneak);
            let home = player.chunk();
            let who = Teleporter {
                width,
                height,
                mover: &mover,
                refuse: "minecraft:consumable_does_not_teleport_to",
                takes: any_place,
            };
            let reached = {
                let view = View { region: self, home };
                random_teleport(&view, [x, y, z], &who)
            };
            let Some(reached) = reached else {
                continue;
            };
            let from = [from.x, from.y, from.z];
            self.teleport_player(id, reached, output);
            if let Some(vibration) = super::vibrations::Vibration::named("minecraft:teleport") {
                let entity = mover.key;
                let context = super::vibrations::Context::of(
                    Some(super::vibrations::Source::of(entity)),
                    None,
                );
                self.vibrate(vibration, from, context);
            }
            output.events.push(RegionEvent::Sound {
                sound: sounds::ITEM_CHORUS_FRUIT_TELEPORT,
                source: crate::api::SOUND_SOURCE_PLAYERS,
                position: clustine_world::Vec3::new(reached[0], reached[1], reached[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
            return true;
        }
        false
    }

    /// The heights a chorus fruit's try is held between: `getMinY()` and `getMinY() +
    /// getLogicalHeight() − 1` of the level (d TeleportRandomlyConsumeEffect, 61–80), so
    /// that in the Nether it never aims above 127, under the roof: the overworld −64 to
    /// 319, the Nether 0 to 127, the End 0 to 255.
    pub(crate) fn chorus_heights(&self) -> (f64, f64) {
        let rules = self.config.dimension.rules();
        let logical = i32::try_from(rules.logical_height).expect("a level's height fits an i32");
        (f64::from(rules.min_y), f64::from(rules.min_y + logical - 1))
    }

    /// `teleportTo(x, y, z)` of the region's player `id`, with `broadcastEntityEvent(46)`:
    /// moved there, their client told, and the portal's particles shown where they are.
    fn teleport_player(&mut self, id: PlayerId, to: Vec3, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let previous_chunk = player.chunk();
        player.pose.position = clustine_world::Vec3::new(to[0], to[1], to[2]);
        let (entity, pose) = (player.entity_id, player.pose);
        let chunk = player.chunk();
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityMoved {
            entity,
            pose,
            previous_chunk,
        });
        output
            .player_events
            .push((id, PlayerEvent::Teleported { pose }));
        output.events.push(RegionEvent::EntityEvent {
            entity,
            chunk,
            event: 46,
        });
    }
}

/// The enderman's box and its own take of a place (`Enderman.canRandomlyTeleportTo`: no
/// water in the block below), with the mover `mover`.
pub(crate) fn enderman(mover: &Mover) -> Teleporter<'_> {
    let info = EntityTypeInfo::of(entity_types::ENDERMAN);
    Teleporter {
        width: info.width,
        height: info.height,
        mover,
        refuse: "minecraft:enderman_does_not_teleport_to",
        takes: no_water_below,
    }
}

/// `Enderman.canRandomlyTeleportTo(x, y, z)`: the block below the place holds no water.
fn no_water_below(below: BlockState) -> bool {
    !clustine_blocks::data::has_water(below)
}
