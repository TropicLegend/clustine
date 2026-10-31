//! The Nether portal's blocks and the portal clock (step P8 of
//! `docs/groundwork/p-plan.md`; the brief's sections 2.1, 2.7, 4.1 and 4.2), as the 26.3
//! jar's `BaseFireBlock`, `NetherPortalBlock`, `PortalShape`, `Entity` and
//! `PortalProcessor` have them.
//!
//! - **Lighting.** Fire or soul fire set where that fire was not, by any means, in the
//!   overworld or the Nether, fills the empty frame about it with portal along X, else
//!   along Z, in place of the fire (`BaseFireBlock.onPlace`, 448–484). A flint and steel,
//!   a fire charge and a dispenser may set fire where it could not stay when it would
//!   light a frame (`canBePlacedAt`, `isPortal`, 523–610). The frame's search is
//!   `clustine_blocks::portal`.
//! - **Breaking.** A portal block whose neighbour changes becomes air unless the change
//!   is in front of it or behind it, the neighbour is portal, or its frame is whole and
//!   full (`NetherPortalBlock.updateShape`, 103–148), so a frame broken drops the whole
//!   portal through the neighbours' updates.
//! - **The clock.** An entity in a portal block that may use a portal is marked inside
//!   it (`entityInside`, `setAsInsidePortal`, Entity.txt 7388–7427), and each tick its
//!   cooldown counts down and its clock counts up while it is marked, or down by four
//!   while it is not, until it fires or runs out (`handlePortal`, 7428–7486;
//!   `PortalProcessor`). What firing does is step P9's ([`Region::portal_fired`]).
//!
//! What the region sees of these differs from the game's in these ways, named:
//!
//! - At a line: a frame is lit only where every block of the portal is the region's
//!   own, as a golem is built (`golems.rs`); elsewhere the fire stays where it can, as
//!   it would beside no frame. A portal block keeps its place where its frame reaches
//!   a block the region cannot read, which it cannot judge.
//! - An entity other than a player is marked by the blocks its box passes through from
//!   where its tick began to where it ended, in one sweep: the game sweeps each of its
//!   moves of the tick. A player is marked by where they stand at the end of the tick's
//!   inputs, as they press plates (`pressed.rs`).
//! - The game rules `players_nether_portal_creative_delay` and
//!   `players_nether_portal_default_delay` keep their defaults, 0 and 80; and every
//!   Clustine player is creative, so invulnerable, until survival comes.
//! - A player's cooldown counts down whatever they do: they are never changing
//!   dimension (`isChangingDimension`) in a tick of the region's.

use clustine_blocks::data::{is_block, relative};
use clustine_blocks::portal::{self, PortalShape};
use clustine_data::{BlockState, Face, blocks, entity_types};
use clustine_world::{
    BlockPos, Dimension, Entity, EntityData, EntityField, EntityId, PlayerId, PortalAxis,
    PortalClock, PortalKind, PortalState,
};

use super::Region;
use super::random_ticks::Random;
use super::updates::{UPDATE_CLIENTS, UPDATE_KNOWN_SHAPE, UPDATE_LIMIT};
use crate::api::TickOutput;
use crate::mobs::Mob;
use crate::mobs::types::{fox, villager};
use crate::physics::aabb::Aabb;
use crate::physics::{self, walk};

/// The game rule `players_nether_portal_creative_delay`: how long an invulnerable
/// player stands in a Nether portal before it fires, by default.
pub(crate) const PLAYERS_NETHER_PORTAL_CREATIVE_DELAY: u32 = 0;

/// The game rule `players_nether_portal_default_delay`: how long any other player
/// stands in it, by default.
pub(crate) const PLAYERS_NETHER_PORTAL_DEFAULT_DELAY: u32 = 80;

/// `Entity.getDimensionChangingDelay`: the cooldown an entity is given as its portal
/// fires and while it stands in a portal on cooldown.
pub(crate) const DIMENSION_CHANGING_DELAY: u32 = 300;

/// `Player.getDimensionChangingDelay`.
pub(crate) const PLAYER_DIMENSION_CHANGING_DELAY: u32 = 10;

/// `createPortalBlocks`' flags (18): the clients are told, and no neighbour's shape is
/// asked, so that the portal does not judge itself half made.
const PORTAL_FLAGS: u32 = UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE;

/// How far the blocks an entity's box passes through in a tick are looked at
/// (`checkInsideBlocks`' sixteen steps of a move).
const MAX_STEPS: i32 = 16;

/// Who a portal's clock fired for: a player of the region's, or an entity of its chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortalUser {
    Player(PlayerId),
    Entity(EntityId),
}

/// `BaseFireBlock.inPortalDimension`: the overworld and the Nether light portals, the
/// End does not.
pub(crate) fn in_portal_dimension(dimension: Dimension) -> bool {
    matches!(dimension, Dimension::Overworld | Dimension::Nether)
}

/// `Portal.getPortalTransitionTime` of a portal of `kind` for an entity, or for a
/// player who is invulnerable or not: a Nether portal makes a player wait the game
/// rule's delay (`NetherPortalBlock.getPortalTransitionTime`, 162–186), and everything
/// else none; the End's portal and gateways make nobody wait (`Portal`'s default).
pub(crate) fn transition_time(kind: PortalKind, player: Option<bool>) -> u32 {
    match (kind, player) {
        (PortalKind::Nether, Some(true)) => PLAYERS_NETHER_PORTAL_CREATIVE_DELAY,
        (PortalKind::Nether, Some(false)) => PLAYERS_NETHER_PORTAL_DEFAULT_DELAY,
        _ => 0,
    }
}

/// `Entity.setAsInsidePortal(portal, pos)`: on cooldown, the cooldown is set full again
/// (`cooldown`), so that one standing in the portal they came out of stays on it;
/// otherwise a clock of another kind, or none, is begun at `entry`, marked; and a clock
/// of this kind not yet marked in this tick takes `entry` and is marked.
pub(crate) fn set_as_inside_portal(
    portal: &mut PortalState,
    kind: PortalKind,
    entry: BlockPos,
    cooldown: u32,
) {
    if portal.cooldown > 0 {
        portal.cooldown = cooldown;
        return;
    }
    match portal.clock.as_mut() {
        Some(clock) if clock.kind == kind => {
            if !clock.inside {
                clock.entry = entry;
                clock.inside = true;
            }
        }
        _ => {
            portal.clock = Some(PortalClock {
                kind,
                entry,
                time: 0,
                inside: true,
            });
        }
    }
}

/// `Entity.handlePortal`, without what firing does: the cooldown counts down
/// (`processPortalCooldown`); a clock marked inside is unmarked and, where the entity
/// may use a portal (`allowed`), counts up, firing where the time before it counted
/// was the portal's transition time or more (`processPortalTeleportation`); a clock not
/// marked counts down by four (`decayTick`). One that fired sets the cooldown
/// (`setPortalCooldown`, to `cooldown`) and is kept; one that did not and has run down
/// to nothing goes (`hasExpired`). The clock that fired, if one did.
pub(crate) fn handle_portal(
    portal: &mut PortalState,
    allowed: impl FnOnce() -> bool,
    transition: impl FnOnce(PortalKind) -> u32,
    cooldown: u32,
) -> Option<PortalClock> {
    portal.cooldown = portal.cooldown.saturating_sub(1);
    let clock = portal.clock.as_mut()?;
    let fired = if clock.inside {
        clock.inside = false;
        if allowed() {
            let before = clock.time;
            clock.time = clock.time.saturating_add(1);
            before >= transition(clock.kind)
        } else {
            false
        }
    } else {
        clock.time = clock.time.saturating_sub(4);
        false
    };
    if fired {
        let clock = *clock;
        portal.cooldown = cooldown;
        return Some(clock);
    }
    if clock.time == 0 {
        portal.clock = None;
    }
    None
}

/// `canUsePortal(false)` of an entity other than a player: never the ender dragon
/// (EnderDragon.txt 2954–2958) or the wither (WitherBoss.txt 1596–1599); always a thrown
/// projectile (`ThrowableProjectile.canUsePortal`); otherwise one that rides nothing and
/// is alive (Entity.txt 9279–9293), and, if it is living, not asleep (LivingEntity.txt
/// 10349–10360): a sleeping villager or fox.
pub(crate) fn entity_can_use_portal(entity: &Entity) -> bool {
    let kind = i32::from(entity.kind);
    if kind == entity_types::ENDER_DRAGON || kind == entity_types::WITHER {
        return false;
    }
    if matches!(entity.data, EntityData::Thrown { .. }) {
        return true;
    }
    if entity.riding.vehicle.is_some() {
        return false;
    }
    if !matches!(entity.data, EntityData::Mob { .. }) {
        return true;
    }
    Mob::decode(&entity.data).is_ok_and(|mob| {
        let asleep = if kind == entity_types::VILLAGER {
            villager::state_of(&mob).1.sleeping.is_some()
        } else if kind == entity_types::FOX {
            fox::state_of(&mob).has(fox::flag_bits::SLEEPING)
        } else {
            false
        };
        mob.body.vitals.health > 0.0 && !asleep
    })
}

/// `getDimensionChangingDelay` of an entity other than a player: the delay of its first
/// passenger where that is a player, else 300 (Entity.txt 7512–7525).
pub(crate) fn dimension_changing_delay(entity: &Entity) -> u32 {
    if entity
        .riding
        .passengers
        .first()
        .is_some_and(|seat| seat.player)
    {
        PLAYER_DIMENSION_CHANGING_DELAY
    } else {
        DIMENSION_CHANGING_DELAY
    }
}

/// The box a portal block of `axis` at `pos` is met in: four sixteenths thick across
/// its axis, the whole block along it and up (`NetherPortalBlock.SHAPES`,
/// `Block.column(4, 16, 0, 16)` turned to each axis).
fn portal_box(pos: BlockPos, axis: PortalAxis) -> Aabb {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    let (low, high) = (6.0 / 16.0, 10.0 / 16.0);
    match axis {
        PortalAxis::X => Aabb::new([x, y, z + low], [x + 1.0, y + 1.0, z + high]),
        PortalAxis::Z => Aabb::new([x + low, y, z], [x + high, y + 1.0, z + 1.0]),
    }
}

/// The End portal's shape as a body meets it (`EndPortalBlock.SHAPE`,
/// `Block.column(16, 6, 12)`): the whole block across, from 6 to 12 sixteenths up.
fn end_portal_box(pos: BlockPos) -> Aabb {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    Aabb::new([x, y + 6.0 / 16.0, z], [x + 1.0, y + 12.0 / 16.0, z + 1.0])
}

/// The axis `getCounterClockWise().getAxis()` of a horizontal direction: north and
/// south turn to west and east, along X; east and west to north and south, along Z.
fn counter_clockwise_axis(direction: Face) -> Option<PortalAxis> {
    match direction {
        Face::North | Face::South => Some(PortalAxis::X),
        Face::East | Face::West => Some(PortalAxis::Z),
        Face::Up | Face::Down => None,
    }
}

impl Region {
    /// The level's floor (`getMinY`), where a frame's search stops going down.
    fn portal_floor(&self) -> i32 {
        self.config.dimension.rules().min_y
    }

    /// The empty frame about `pos`, along `axis` first, as the region reads it; a block
    /// it cannot read is air, which is never frame, so a frame it cannot see whole is
    /// not found.
    fn empty_portal_shape(&self, pos: BlockPos, axis: PortalAxis) -> Option<PortalShape> {
        let mut read = |at: BlockPos| self.block(at).unwrap_or(blocks::AIR);
        portal::find_empty_portal_shape(&mut read, self.portal_floor(), pos, axis)
    }

    /// `BaseFireBlock.onPlace`'s portal: fire set at `pos` where that fire was not, in
    /// the overworld or the Nether, fills the empty frame about it, along X else Z,
    /// with portal (`createPortalBlocks`, its flags 18, in its order), which replaces the
    /// fire. Whether it did; where it did not, the fire's own rules go on.
    pub(crate) fn fire_lights_portal(&mut self, pos: BlockPos, output: &mut TickOutput) -> bool {
        if !in_portal_dimension(self.config.dimension) {
            return false;
        }
        let Some(shape) = self.empty_portal_shape(pos, PortalAxis::X) else {
            return false;
        };
        let cells = shape.cells();
        if !cells.iter().all(|cell| self.own_block(*cell).is_some()) {
            return false;
        }
        let state = shape.portal_state();
        for cell in cells {
            self.set_block_updated(cell, state, PORTAL_FLAGS, UPDATE_LIMIT, output);
        }
        true
    }

    /// `BaseFireBlock.isPortal(level, pos, direction)`: in the overworld or the Nether,
    /// at the first neighbour of `pos` that is frame (in the order of the directions),
    /// whether an empty frame is about `pos`. The axis tried first is the one counter-
    /// clockwise of a horizontal `direction`, or for an upward or downward one a random
    /// horizontal axis (`Direction.Plane.getRandomAxis`) drawn from `random`, the
    /// level's stream of the use. The search tries the other axis after, so only the
    /// draw depends on which.
    pub(crate) fn fire_would_light_portal(
        &self,
        pos: BlockPos,
        direction: Face,
        random: Option<&mut Random>,
    ) -> bool {
        if !in_portal_dimension(self.config.dimension) {
            return false;
        }
        let framed = Face::ALL.into_iter().any(|face| {
            self.block(relative(pos, face))
                .is_some_and(portal::is_frame)
        });
        if !framed {
            return false;
        }
        let axis = match (counter_clockwise_axis(direction), random) {
            (Some(axis), _) => axis,
            (None, Some(random)) => {
                if random.next_int(2) == 0 {
                    PortalAxis::X
                } else {
                    PortalAxis::Z
                }
            }
            // A use with no stream of its own faces a horizontal direction (a player's,
            // `getHorizontalDirection`), so this is never asked.
            (None, None) => PortalAxis::X,
        };
        self.empty_portal_shape(pos, axis).is_some()
    }

    /// `NetherPortalBlock.updateShape` of the portal block `state` at `pos` whose
    /// neighbour towards `direction` became `neighbour`: the state it takes, the same
    /// or air; `None` where `state` is no portal. A portal whose frame reaches a block
    /// the region cannot read stays.
    pub(crate) fn nether_portal_shape(
        &self,
        pos: BlockPos,
        state: BlockState,
        direction: Face,
        neighbour: BlockState,
    ) -> Option<BlockState> {
        if !is_block(state, blocks::NETHER_PORTAL) {
            return None;
        }
        let mut unseen = false;
        let mut read = |at: BlockPos| {
            self.block(at).unwrap_or_else(|| {
                unseen = true;
                blocks::AIR
            })
        };
        let stays = portal::portal_stays(
            &mut read,
            self.portal_floor(),
            state,
            pos,
            direction,
            neighbour,
        );
        Some(if stays || unseen { state } else { blocks::AIR })
    }

    /// The first portal block, in the order of the walk, that a box `bounds` (where it
    /// ends, at `to`) meets on its way from `from` (`checkInsideBlocks`, with the
    /// portal's own shape as `getEntityInsideCollisionShape`), with its kind: a Nether
    /// portal or the End's.
    fn portal_met(
        &self,
        from: physics::Vec3,
        to: physics::Vec3,
        bounds: Aabb,
    ) -> Option<(BlockPos, PortalKind)> {
        let inside = bounds.deflate(f64::from(1.0e-5_f32));
        let delta = physics::sub(to, from);
        let moving = bounds.moved(physics::sub(from, to));
        let mut met = None;
        walk::for_each_block_between(from, to, &inside, |pos, step| {
            if step >= MAX_STEPS {
                return false;
            }
            let Some(state) = self.block(pos) else {
                return true;
            };
            let (shape, kind) = if is_block(state, blocks::NETHER_PORTAL) {
                (portal_box(pos, portal::axis_of(state)), PortalKind::Nether)
            } else if is_block(state, blocks::END_PORTAL) {
                (end_portal_box(pos), PortalKind::End)
            } else {
                return true;
            };
            if moving.collided_along_vector(delta, &[shape]) {
                met = Some((pos, kind));
                return false;
            }
            true
        });
        met
    }

    /// `baseTick`'s `handlePortal` of the entity `key`, other than a player, as its tick
    /// begins: its portal state counted on and committed where it changed; a clock that
    /// fires goes to [`Region::portal_fired`].
    pub(crate) fn entity_portal_tick(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, entity)) = self.find_entity(key) else {
            return;
        };
        if entity.portal.is_idle() {
            return;
        }
        let before = entity.portal;
        let mut portal = before;
        let fired = handle_portal(
            &mut portal,
            || entity_can_use_portal(entity),
            |kind| transition_time(kind, None),
            dimension_changing_delay(entity),
        );
        if portal != before {
            self.commit_entity(chunk, key, vec![EntityField::Portal(portal)], output);
        }
        if let Some(clock) = fired {
            self.portal_fired(PortalUser::Entity(key), clock.kind, clock.entry, output);
        }
    }

    /// `entityInside` of the portal blocks the entity `key`, other than a player, met in
    /// its tick, which began with it at `from`: where it may use a portal, it is marked
    /// inside the first, committed where its portal state changed.
    pub(crate) fn entity_portal_inside(
        &mut self,
        key: EntityId,
        from: physics::Vec3,
        output: &mut TickOutput,
    ) {
        let Some((chunk, entity)) = self.find_entity(key) else {
            return;
        };
        let bounds = self.entity_box(entity);
        let Some((entry, kind)) = self.portal_met(from, entity.position, bounds) else {
            return;
        };
        if !entity_can_use_portal(entity) {
            return;
        }
        let before = entity.portal;
        let mut portal = before;
        set_as_inside_portal(&mut portal, kind, entry, dimension_changing_delay(entity));
        if portal != before {
            self.commit_entity(chunk, key, vec![EntityField::Portal(portal)], output);
        }
    }

    /// The players' portals, in the order of their ids: each one's `handlePortal`, then
    /// the portal blocks where they stand (`entityInside`) where they may use a portal
    /// (`canUsePortal(false)`: riding nothing and not asleep; a player is always alive
    /// here). Their state is journalled where it changed.
    pub(crate) fn players_portal_tick(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(player) = self.players.get(&id) else {
                continue;
            };
            let before = player.portal;
            let allowed = player.riding.is_none() && player.sleeping.is_none();
            let feet = player.pose.position;
            let mut portal = before;
            // Every Clustine player is creative, and so invulnerable.
            let invulnerable = true;
            let fired = handle_portal(
                &mut portal,
                || allowed,
                |kind| transition_time(kind, Some(invulnerable)),
                PLAYER_DIMENSION_CHANGING_DELAY,
            );
            let at = [feet.x, feet.y, feet.z];
            let body = crate::placement::body(feet, false);
            let bounds = Aabb::new([body[0], body[1], body[2]], [body[3], body[4], body[5]]);
            let met = self.portal_met(at, at, bounds).filter(|_| allowed);
            // `EndPortalBlock.entityInside`: in the End, a player who has not seen the
            // credits is shown them instead of the portal (the plan, section 5.3), which
            // is P10c's; until it is built, nothing.
            let to_credits =
                self.config.dimension == clustine_world::Dimension::End && !player.seen_credits;
            let credits = |kind| kind == PortalKind::End && to_credits;
            if let Some((entry, kind)) = met
                && !credits(kind)
            {
                set_as_inside_portal(&mut portal, kind, entry, PLAYER_DIMENSION_CHANGING_DELAY);
            }
            if portal != before {
                if let Some(player) = self.players.get_mut(&id) {
                    player.portal = portal;
                }
                self.journal.players.insert(id);
            }
            if let Some(clock) = fired {
                self.portal_fired(PortalUser::Player(id), clock.kind, clock.entry, output);
            }
        }
    }

    /// What a portal does as its clock fires for `who`, in the portal of `kind` it
    /// entered at `entry`: the cooldown is set already. The traveller leaves on its
    /// travel (`travel.rs`).
    pub(crate) fn portal_fired(
        &mut self,
        who: PortalUser,
        kind: PortalKind,
        entry: BlockPos,
        output: &mut TickOutput,
    ) {
        self.travel_fired(who, kind, entry, output);
    }
}

#[cfg(test)]
#[path = "portals_tests.rs"]
pub(super) mod tests;
