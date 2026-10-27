//! What a mob senses (the brief's 2.4): `Sensing`'s cache of lines of sight for one AI
//! step, `TargetingConditions`, and the queries goals make of the players and entities
//! around (`ServerEntityGetter`'s), over what the view gives
//! ([`crate::mobs::view::MobWorld`]).
//!
//! Every player is a creative one, whom no mob takes as an enemy (the brief's 2.0.1):
//! combat conditions refuse them, as the game's do.

use std::collections::BTreeMap;

use clustine_blocks::data::entity_in_tag;
use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{EntityData, EntityId, ItemStack};

use super::Ai;
use crate::mobs::body::Vitals;
use crate::mobs::part;
use crate::mobs::view::{Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, collision_shape};
use crate::physics::{self, Vec3};

/// A player's box: as wide as `Player.STANDING_DIMENSIONS`, and as high standing or
/// crouching.
const PLAYER_WIDTH: f32 = 0.6;
const PLAYER_HEIGHT: f32 = 1.8;
const PLAYER_CROUCHING_HEIGHT: f32 = 1.5;

/// The lines of sight a mob has cast in this AI step (`Sensing.seen` and `unseen`),
/// emptied at every step.
#[derive(Debug, Default)]
pub(crate) struct Sensing {
    seen: BTreeMap<EntityId, bool>,
}

/// A living thing a mob looks at: an entity of a chunk or a player.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Looked {
    pub(crate) key: EntityId,
    /// Its entity type, none for a player.
    pub(crate) kind: Option<u16>,
    /// Whether it is a mob (a `Mob`, as the goals' classes ask).
    pub(crate) mob: bool,
    pub(crate) position: Vec3,
    pub(crate) bounds: Aabb,
    pub(crate) eye_y: f64,
    pub(crate) alive: bool,
    pub(crate) sneaking: bool,
    pub(crate) creative: bool,
    pub(crate) spectator: bool,
    pub(crate) vehicle: Option<EntityId>,
    pub(crate) main_hand: Option<ItemStack>,
    pub(crate) off_hand: Option<ItemStack>,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    /// Whether it is in a copy, another region's.
    pub(crate) copied: bool,
}

impl Looked {
    /// An entity of a chunk, as a mob reads it.
    pub(crate) fn of_seen(seen: &Seen<'_>) -> Self {
        let entity = seen.entity;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        // A baby animal's eyes are its own size's.
        let eye_height =
            crate::mobs::types::own_size(entity).map_or(info.eye_height, |size| size.eye_height);
        let (mob, alive) = match &entity.data {
            EntityData::Mob { body, .. } => {
                let alive = part::decode::<Vitals>(body).is_ok_and(|vitals| !vitals.dying());
                (true, alive)
            }
            _ => (false, true),
        };
        Self {
            key: entity.key,
            kind: Some(entity.kind),
            mob,
            position: entity.position,
            bounds: seen.bounds,
            eye_y: entity.position[1] + f64::from(eye_height),
            alive,
            sneaking: false,
            creative: false,
            spectator: false,
            vehicle: entity.riding.vehicle,
            main_hand: None,
            off_hand: None,
            yaw: entity.yaw,
            pitch: entity.pitch,
            copied: seen.copied,
        }
    }

    /// A player, as a mob reads one.
    pub(crate) fn of_player(player: &SeenPlayer) -> Self {
        let height = if player.sneaking {
            PLAYER_CROUCHING_HEIGHT
        } else {
            PLAYER_HEIGHT
        };
        Self {
            key: player.key,
            kind: None,
            mob: false,
            position: player.position,
            bounds: Aabb::of_entity(player.position, PLAYER_WIDTH, height),
            eye_y: player.position[1] + player.eye_height,
            alive: true,
            sneaking: player.sneaking,
            creative: player.creative,
            spectator: player.spectator,
            vehicle: player.vehicle,
            main_hand: player.main_hand.clone(),
            off_hand: player.off_hand.clone(),
            yaw: player.yaw,
            pitch: player.pitch,
            copied: false,
        }
    }

    pub(crate) fn is_player(&self) -> bool {
        self.kind.is_none()
    }

    /// `canBeSeenByAnyone`: alive and not a spectator.
    pub(crate) fn seen_by_anyone(&self) -> bool {
        self.alive && !self.spectator
    }

    /// `canBeSeenAsEnemy`: and not invulnerable, which a creative player is.
    pub(crate) fn seen_as_enemy(&self) -> bool {
        !self.creative && self.seen_by_anyone()
    }

    /// `distanceToSqr(x, y, z)` from its feet.
    pub(crate) fn distance_sqr(&self, point: Vec3) -> f64 {
        physics::distance_sqr(point, self.position)
    }

    /// `blockPosition()`.
    pub(crate) fn block_position(&self) -> clustine_world::BlockPos {
        physics::containing(self.position)
    }

    /// `getVisibilityPercent(looker)`: a crouching player is seen at four fifths of the
    /// range. **Named**: no mob is invisible and nothing worn changes how it is seen
    /// (no effects, no skulls' visibility) until a later step.
    fn visibility(&self) -> f64 {
        let mut visibility = 1.0_f64;
        if self.sneaking {
            visibility *= 0.8;
        }
        visibility.clamp(0.0, 10.0)
    }
}

/// Which things a query takes (the class a goal names).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Class {
    /// `Player`.
    Player,
    /// Any `Mob`.
    Mob,
    /// The mobs of these types.
    Kinds(&'static [u16]),
    /// The living things of an entity type tag.
    Tag(&'static str),
    /// `Monster`: the mobs of the monsters' category but the ghast, the phantom, the
    /// slimes and the shulker, which are not of the class.
    Monster,
}

impl Class {
    fn takes(self, looked: &Looked) -> bool {
        match self {
            Class::Player => looked.is_player(),
            Class::Mob => looked.mob,
            Class::Kinds(kinds) => {
                looked.mob && looked.kind.is_some_and(|kind| kinds.contains(&kind))
            }
            Class::Tag(tag) => looked
                .kind
                .is_some_and(|kind| entity_in_tag(i32::from(kind), tag)),
            Class::Monster => looked.mob && looked.kind.is_some_and(is_monster),
        }
    }
}

/// Whether the mob type `kind` is of the class `Monster`.
pub(crate) fn is_monster(kind: u16) -> bool {
    let id = i32::from(kind);
    EntityTypeInfo::of(id).category == clustine_data::MobCategory::Monster
        && ![
            entity_types::GHAST,
            entity_types::PHANTOM,
            entity_types::SLIME,
            entity_types::MAGMA_CUBE,
            entity_types::SHULKER,
        ]
        .contains(&id)
}

/// `TargetingConditions`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Conditions {
    pub(crate) combat: bool,
    /// -1 for no range.
    pub(crate) range: f64,
    pub(crate) line_of_sight: bool,
    pub(crate) invisibility: bool,
}

impl Conditions {
    /// `forCombat()`.
    pub(crate) const fn combat() -> Self {
        Self {
            combat: true,
            range: -1.0,
            line_of_sight: true,
            invisibility: true,
        }
    }

    /// `forNonCombat()`.
    pub(crate) const fn non_combat() -> Self {
        Self {
            combat: false,
            range: -1.0,
            line_of_sight: true,
            invisibility: true,
        }
    }

    pub(crate) const fn range(self, range: f64) -> Self {
        Self { range, ..self }
    }

    pub(crate) const fn ignore_line_of_sight(self) -> Self {
        Self {
            line_of_sight: false,
            ..self
        }
    }

    pub(crate) const fn ignore_invisibility(self) -> Self {
        Self {
            invisibility: false,
            ..self
        }
    }

    /// `test(level, source, target)` with the mob as the source and `selector` as the
    /// conditions' selector.
    pub(crate) fn test(
        &self,
        ai: &mut Ai<'_, '_>,
        target: &Looked,
        selector: &dyn Fn(&Looked) -> bool,
    ) -> bool {
        if target.key == ai.tick.key {
            return false;
        }
        if !target.seen_by_anyone() {
            return false;
        }
        if !selector(target) {
            return false;
        }
        // No two mobs are of one team: nothing is allied.
        if self.combat && !can_attack(target) {
            return false;
        }
        if self.range > 0.0 {
            let visibility = if self.invisibility {
                target.visibility()
            } else {
                1.0
            };
            let reach = (self.range * visibility).max(2.0);
            if physics::distance_sqr(ai.position(), target.position) > reach * reach {
                return false;
            }
        }
        if self.line_of_sight && !has_line_of_sight(ai, target) {
            return false;
        }
        true
    }
}

/// `Mob.canAttack(target)`: not a ghast, and seen as an enemy.
pub(crate) fn can_attack(target: &Looked) -> bool {
    target.kind != Some(entity_types::GHAST as u16) && target.seen_as_enemy()
}

/// `Sensing.hasLineOfSight(target)`: cast once a step for each thing.
pub(crate) fn has_line_of_sight(ai: &mut Ai<'_, '_>, target: &Looked) -> bool {
    if let Some(&seen) = ai.sensing.seen.get(&target.key) {
        return seen;
    }
    let seen = line_of_sight(ai, target);
    ai.sensing.seen.insert(target.key, seen);
    seen
}

/// `LivingEntity.hasLineOfSight(target)`: from the eyes to its eyes, within 128 blocks,
/// with no block's collision shape (as the mob's own context has it) in the way; water
/// is seen through.
fn line_of_sight(ai: &Ai<'_, '_>, target: &Looked) -> bool {
    let position = ai.position();
    let from = [position[0], ai.eye_y(), position[2]];
    let to = [target.position[0], target.eye_y, target.position[2]];
    if physics::distance_sqr(from, to).sqrt() > 128.0 {
        return false;
    }
    let context = Context {
        bottom: position[1],
        fall_distance: ai.tick.living.body.fall_distance,
        falling_block: false,
        noclip: None,
    };
    let world = ai.tick.world;
    !physics::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        collision_shape(state, pos, context).is_some_and(|(shape, _)| {
            shape
                .boxes()
                .iter()
                .any(|bounds| physics::body::clips_shape(bounds, from, to))
        })
    })
}

/// The players a mob reads, as it reads them, in the view's order.
pub(crate) fn players(ai: &Ai<'_, '_>) -> Vec<Looked> {
    ai.tick
        .world
        .players()
        .iter()
        .map(Looked::of_player)
        .collect()
}

/// `getEntitiesOfClass(class, box)`: the things of the class whose boxes meet `bounds`,
/// in the view's order (players, which are no entities of a chunk, by the box too).
pub(crate) fn things_in(ai: &Ai<'_, '_>, bounds: Aabb, class: Class) -> Vec<Looked> {
    if class == Class::Player {
        return players(ai)
            .into_iter()
            .filter(|player| player.bounds.intersects(bounds))
            .collect();
    }
    ai.tick
        .world
        .entities_in(&bounds)
        .iter()
        .map(Looked::of_seen)
        .filter(|looked| class.takes(looked))
        .collect()
}

/// `getNearestPlayer(conditions, source, x, y, z)`: of the players that pass, the one
/// nearest `from`; the earlier of two as near.
pub(crate) fn nearest_player(
    ai: &mut Ai<'_, '_>,
    conditions: &Conditions,
    selector: &dyn Fn(&Looked) -> bool,
    from: Vec3,
) -> Option<Looked> {
    let candidates = players(ai);
    nearest(ai, candidates, conditions, selector, from)
}

/// `getNearestEntity(list, conditions, source, x, y, z)`: the earlier of two as near.
pub(crate) fn nearest(
    ai: &mut Ai<'_, '_>,
    candidates: Vec<Looked>,
    conditions: &Conditions,
    selector: &dyn Fn(&Looked) -> bool,
    from: Vec3,
) -> Option<Looked> {
    let mut best: Option<(f64, Looked)> = None;
    for candidate in candidates {
        if !conditions.test(ai, &candidate, selector) {
            continue;
        }
        let distance = candidate.distance_sqr(from);
        if best.as_ref().is_none_or(|(nearest, _)| distance < *nearest) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, looked)| looked)
}

/// `getNearbyEntities(class, conditions, source, box)` and `getNearbyPlayers`: those of
/// the box that pass, in order (players by `AABB.contains` of their feet).
pub(crate) fn nearby(
    ai: &mut Ai<'_, '_>,
    class: Class,
    conditions: &Conditions,
    bounds: Aabb,
) -> Vec<Looked> {
    let candidates = if class == Class::Player {
        players(ai)
            .into_iter()
            .filter(|player| bounds.contains(player.position))
            .collect()
    } else {
        things_in(ai, bounds, class)
    };
    candidates
        .into_iter()
        .filter(|candidate| conditions.test(ai, candidate, &|_| true))
        .collect()
}

/// The thing the key names, an entity or a player, wherever the view has it.
pub(crate) fn find(ai: &Ai<'_, '_>, key: EntityId) -> Option<Looked> {
    if let Some(seen) = ai.tick.world.entity(key) {
        return Some(Looked::of_seen(&seen));
    }
    ai.tick
        .world
        .players()
        .iter()
        .find(|player| player.key == key)
        .map(Looked::of_player)
}

/// `Mob.getTarget()`: the target held, as long as it is still a valid one
/// (`asValidTarget`: no creative or spectator player, nothing it cannot attack).
pub(crate) fn target(ai: &Ai<'_, '_>) -> Option<Looked> {
    let key = ai.tick.mob.body.combat.target?;
    find(ai, key).filter(valid_target)
}

/// `Mob.asValidTarget(target)`.
pub(crate) fn valid_target(target: &Looked) -> bool {
    !(target.is_player() && (target.creative || target.spectator)) && can_attack(target)
}

/// `Mob.setTarget(target)`: kept only if valid.
pub(crate) fn set_target(ai: &mut Ai<'_, '_>, target: Option<&Looked>) {
    ai.tick.mob.body.combat.target = target
        .filter(|target| valid_target(target))
        .map(|target| target.key);
}
