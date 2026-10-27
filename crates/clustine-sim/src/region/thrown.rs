//! Thrown items (`ThrowableItemProjectile`; `docs/groundwork/n9-plan.md`, section 4.4, as
//! D9 2 settles it): the snowball, the egg, the splash and the lingering potion, the
//! bottle o' enchanting and the ender pearl. Each is thrown by a player
//! (`region/throws.rs`), a mob or a dispenser (`Projectile.spawnProjectile`), flies as
//! `ThrowableProjectile.tick` has it (the arithmetic in `crate::physics::thrown`) with
//! its kind's gravity, and hits as its kind's `onHitEntity`, `onHitBlock` and `onHit`
//! have it:
//!
//! - the snowball: a hurt of nothing (3 to a blaze), whose knockback pushes along its
//!   motion; its particles (entity event 3); gone;
//! - the egg: a hurt of nothing; one chick in eight, and four in 256, at the egg with
//!   its yaw, of the stack's variant (`ThrownEgg.onHit`, n9 ThrownEgg 113–195); its
//!   particles; gone;
//! - the splash potion: water's behaviours, then each living entity near given the
//!   effects by its distance, box to box (`ThrownSplashPotion.onHitAsPotion`, n9 38–180);
//!   the level events of its colour; gone;
//! - the lingering potion: water's behaviours, then a cloud (`clouds.rs`) at the entity
//!   hit or the potion (n9 ThrownLingeringPotion 38–114); the level events; gone;
//! - the bottle: its level events, then `3 + nextInt(5) + nextInt(5)` experience in orbs
//!   along the side hit or back against its motion (n9 ThrownExperienceBottle 43–112,
//!   `orbs.rs`); gone;
//! - the pearl: a hurt of nothing; 96 draws of its particles; its owner moved to where
//!   it was as the tick began, an endermite one time in twenty (n9 ThrownEnderpearl
//!   137–318); gone.
//!
//! The hit test reads the blocks and entities of the region's loaded and copied chunks,
//! the region's players and the faces of other regions' players (`projectiles.rs`'
//! `targets`). An entity the region shows only in a copy is hit by its holder, through
//! an effect on it, as a mob's hit is; a face refuses every hurt; the projectile is gone
//! in its own region. A projectile that crosses a line goes as any entity does
//! (`riding.rs`' `relocate`) and flies on there. Effects go to the region's own entity
//! and player, to a copied entity's holder (`EntityUse::Effects`) and to another
//! region's player by their stay (`send_on_player`), and a pearl's landing to its owner
//! the same way (`EntityUse::PearlLanded`).
//!
//! What the game keeps of it in memory only (`isInWater`, which its next tick's drag
//! reads) the region keeps by key, as primed TNT's (`items.rs`'s `CartMemory`); after a
//! take-in it starts dry, as at the game's load.
//!
//! Named differences:
//! - The blocks a thrown item passes through (`applyEffectsFromBlocks`: a tripwire,
//!   fire), the first tick's bubble column, its burning in lava and the world border
//!   are left out.
//! - The owner is known while the region has it, a player or an entity of its own
//!   chunks, or for a pearl a player of another region by their face or the world's
//!   players; one it does not have is none, as the game's `getOwner()` is null for an
//!   owner its level does not have loaded.
//! - No blaze exists yet: every snowball hurts by nothing.
//! - Only mobs are the living entities a splash or a cloud reaches: an armour stand is
//!   never affected by potions anyway. Water put out on a burning entity of a copy is
//!   not (`EntityUse::Effects` carries no fire); axolotls are not rehydrated (none dries
//!   out in Clustine).
//! - A splash's proximity crosses a line as a float (`GivenEffects`' `instant_factor`),
//!   the game's being a double.
//! - A pearl thrown by a mob teleports nobody (no mob throws one); an endermite is
//!   always allowed (the server spawns monsters, and the difficulty is easy).
//! - Where the pearl lands is the position it had as its tick began (`oldPosition()`,
//!   which the level sets before each tick: **Inferred**, D9 2.8 calls it ambiguous).

use clustine_blocks::data::{flag, in_tag, set_flag};
use clustine_data::{EntityTypeInfo, Face, ItemComponents, entity_types, sounds};
use clustine_world::components::{Component, EffectInstance, Kind, PotionContents};
use clustine_world::{
    BlockPos, ChunkPos, Double, Entity, EntityData, EntityField, EntityId, EntityUse, Float,
    GivenEffects, ItemStack, PlayerId, Thrower, projectiles::Cloud,
};

use super::Region;
use super::attached::Breaker;
use super::items::{CartMemory, Memory, View, tracked};
use super::projectiles::{Target, TargetKind};
use super::random_ticks::Random;
use super::updates::{UPDATE_ALL, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::mobs::body::hurt::{Causer, DamageSource};
use crate::physics::aabb::Aabb;
use crate::physics::collide::Context as Collision;
use crate::physics::thrown::{self, BlockHit};
use crate::physics::{self, Vec3, fluids};

/// The entity event that shows a snowball's or an egg's particles where it hit.
const HIT_PARTICLES: u8 = 3;

/// How far below the world's floor an entity is gone (`checkBelowWorld`).
const BELOW_THE_WORLD: f64 = 64.0;

/// `PotionContents.BASE_POTION_COLOR`: the colour of a potion of no visible effect,
/// and of the bottle's splash.
pub(crate) const BASE_POTION_COLOUR: i32 = -13_083_194;

/// The level events of a potion's splash: of one whose potion is instant, of any
/// other, and the sounds of each (`LevelEvent.PARTICLES_INSTANT_POTION_SPLASH`,
/// `PARTICLES_SPELL_POTION_SPLASH`, `SOUND_SPLASH_POTION…`).
const INSTANT_SPLASH: i32 = 2007;
const SPLASH: i32 = 2002;
const INSTANT_SPLASH_SOUND: i32 = 1054;
const SPLASH_SOUND: i32 = 1053;

/// `LevelEvent.SOUND_EXTINGUISH_FIRE`, of a campfire put out.
const CAMPFIRE_PUT_OUT: i32 = 1009;

/// The entity types `isSensitiveToWater`, whom a splash of water hurts: the enderman,
/// the blaze, the snow golem and the strider (as `entity_effects.rs` has them).
const SENSITIVE_TO_WATER: [i32; 4] = [
    entity_types::ENDERMAN,
    entity_types::BLAZE,
    entity_types::SNOW_GOLEM,
    entity_types::STRIDER,
];

/// The colour of each effect (`MobEffect.getColor`), from `MobEffects`' static
/// initialiser (n7k MobEffects).
const EFFECT_COLOURS: [(&str, i32); 40] = [
    ("minecraft:speed", 3_402_751),
    ("minecraft:slowness", 9_154_528),
    ("minecraft:haste", 14_270_531),
    ("minecraft:mining_fatigue", 4_866_583),
    ("minecraft:strength", 16_762_624),
    ("minecraft:instant_health", 16_262_179),
    ("minecraft:instant_damage", 11_101_546),
    ("minecraft:jump_boost", 16_646_020),
    ("minecraft:nausea", 5_578_058),
    ("minecraft:regeneration", 13_458_603),
    ("minecraft:resistance", 9_520_880),
    ("minecraft:fire_resistance", 16_750_848),
    ("minecraft:water_breathing", 10_017_472),
    ("minecraft:invisibility", 16_185_078),
    ("minecraft:blindness", 2_039_587),
    ("minecraft:night_vision", 12_779_366),
    ("minecraft:hunger", 5_797_459),
    ("minecraft:weakness", 4_738_376),
    ("minecraft:poison", 8_889_187),
    ("minecraft:wither", 7_561_558),
    ("minecraft:health_boost", 16_284_963),
    ("minecraft:absorption", 2_445_989),
    ("minecraft:saturation", 16_262_179),
    ("minecraft:glowing", 9_740_385),
    ("minecraft:levitation", 13_565_951),
    ("minecraft:luck", 5_882_118),
    ("minecraft:unluck", 12_624_973),
    ("minecraft:slow_falling", 15_978_425),
    ("minecraft:conduit_power", 1_950_417),
    ("minecraft:dolphins_grace", 8_954_814),
    ("minecraft:bad_omen", 745_784),
    ("minecraft:hero_of_the_village", 4_521_796),
    ("minecraft:darkness", 2_696_993),
    ("minecraft:trial_omen", 1_484_454),
    ("minecraft:raid_omen", 14_565_464),
    ("minecraft:wind_charged", 12_438_015),
    ("minecraft:weaving", 7_891_290),
    ("minecraft:oozing", 10_092_451),
    ("minecraft:infested", 9_214_860),
    ("minecraft:breath_of_the_nautilus", 65_518),
];

/// A projectile's hit on an entity, as the projectile's region works it out, and as an
/// effect brings it to the entity's holder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hit {
    pub(crate) projectile: EntityId,
    pub(crate) kind: u16,
    pub(crate) at: Vec3,
    pub(crate) motion: Vec3,
    pub(crate) damage: f32,
    pub(crate) owner: Option<Thrower>,
}

impl Hit {
    fn as_use(self) -> EntityUse {
        EntityUse::Thrown {
            projectile: self.projectile,
            kind: self.kind,
            at: self.at.map(Double),
            motion: self.motion.map(Double),
            damage: Float(self.damage),
            owner: self.owner,
        }
    }

    /// The hit an effect brings, if it is one.
    pub(crate) fn of_use(by: &EntityUse) -> Option<Self> {
        match *by {
            EntityUse::Thrown {
                projectile,
                kind,
                at,
                motion,
                damage,
                owner,
            } => Some(Self {
                projectile,
                kind,
                at: at.map(|value| value.0),
                motion: motion.map(|value| value.0),
                damage: damage.0,
                owner,
            }),
            _ => None,
        }
    }

    /// `damageSources().thrown(projectile, owner)`.
    fn source(&self) -> DamageSource {
        let direct = Causer {
            key: self.projectile,
            kind: self.kind,
            position: self.at,
            player: None,
            living: false,
        };
        let owner = self.owner.map(|owner| Causer {
            key: owner.key,
            kind: owner.kind,
            position: owner.from.map(|value| value.0),
            player: owner.player.map(|uuid| uuid.as_u128()),
            living: true,
        });
        DamageSource::thrown(direct, self.motion, owner)
    }

    /// Whether a player threw it: every player is a creative one.
    fn by_creative(&self) -> bool {
        self.owner.is_some_and(|owner| owner.player.is_some())
    }
}

/// What a projectile's move met (`ProjectileUtil.getHitResultOnMoveVector`): an entity
/// where its box was entered, or a block, the point on it and the side met.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Struck {
    Entity {
        target: EntityId,
        kind: TargetKind,
        at: Vec3,
    },
    Block {
        hit: BlockHit,
        face: Face,
    },
}

impl Struck {
    /// `HitResult.getLocation()`.
    pub(crate) fn location(&self) -> Vec3 {
        match *self {
            Struck::Entity { at, .. } => at,
            Struck::Block { hit, .. } => hit.location,
        }
    }
}

/// A projectile as its hit test sees it: its key and box, and who it may not hit yet
/// (`canHitEntity`: not its owner's unit before it has left it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Flying {
    pub(crate) key: EntityId,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) owner_root: Option<EntityId>,
    pub(crate) left_owner: bool,
    /// The game's `tickCount` in this tick, which the margin grows by.
    pub(crate) tick_count: u32,
    pub(crate) fall_distance: f64,
}

/// A living entity a splash, a cloud, a rocket or fangs reach: the target as the hit
/// test lists it, its type (none for a player), its feet and whether it burns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Living {
    pub(crate) target: Target,
    pub(crate) kind: Option<u16>,
    pub(crate) position: Vec3,
    pub(crate) burning: bool,
}

/// `stack.getOrDefault(POTION_CONTENTS, EMPTY)`: the patch's, else the item's default,
/// else none.
pub(crate) fn potion_of(stack: &ItemStack) -> PotionContents {
    if stack.components.removes(Kind::PotionContents) {
        return PotionContents::default();
    }
    if let Some(Component::PotionContents(contents)) = stack.components.get(Kind::PotionContents) {
        return contents.clone();
    }
    ItemComponents::of(stack.item)
        .potion_contents
        .map_or_else(PotionContents::default, |default| PotionContents {
            potion: default.potion.map(str::to_owned),
            custom_colour: default.custom_colour,
            custom_effects: default.custom_effects.iter().map(world_instance).collect(),
            custom_name: default.custom_name.map(str::to_owned),
        })
}

/// `stack.getOrDefault(POTION_DURATION_SCALE, 1.0f)`.
pub(crate) fn duration_scale_of(stack: &ItemStack) -> f32 {
    if stack.components.removes(Kind::PotionDurationScale) {
        return 1.0;
    }
    if let Some(Component::PotionDurationScale(scale)) =
        stack.components.get(Kind::PotionDurationScale)
    {
        return scale.0;
    }
    ItemComponents::of(stack.item)
        .potion_duration_scale
        .unwrap_or(1.0)
}

/// An effect of the data's table as an item carries it.
fn world_instance(instance: &clustine_data::EffectInstance) -> EffectInstance {
    EffectInstance {
        effect: instance.effect.to_owned(),
        amplifier: instance.amplifier,
        duration: instance.duration,
        ambient: instance.ambient,
        visible: instance.visible,
        show_icon: instance.show_icon,
    }
}

/// The effects of the base potion of `contents`, if it has one Clustine knows.
fn base_effects(contents: &PotionContents) -> &'static [clustine_data::EffectInstance] {
    contents
        .potion
        .as_deref()
        .and_then(clustine_data::potions::Potion::named)
        .map_or(&[], |potion| potion.effects)
}

/// `PotionContents.getAllEffects()`: the base potion's effects, then its own.
pub(crate) fn all_effects(contents: &PotionContents) -> Vec<EffectInstance> {
    base_effects(contents)
        .iter()
        .map(world_instance)
        .chain(contents.custom_effects.iter().cloned())
        .collect()
}

/// `PotionContents.hasEffects()`.
pub(crate) fn has_effects(contents: &PotionContents) -> bool {
    !contents.custom_effects.is_empty() || !base_effects(contents).is_empty()
}

/// `MobEffect.isInstantaneous()`: instant health, instant damage and saturation.
pub(crate) fn is_instantaneous(effect: &str) -> bool {
    matches!(
        effect,
        "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation"
    )
}

/// `PotionContents.getColor()`: its own colour, else the mix of its visible effects'
/// colours weighted by their levels (`getColorOptional`, opaque), else the base colour.
pub(crate) fn potion_colour(contents: &PotionContents) -> i32 {
    if let Some(colour) = contents.custom_colour {
        return colour;
    }
    let (mut red, mut green, mut blue, mut weight) = (0_i32, 0_i32, 0_i32, 0_i32);
    for instance in all_effects(contents) {
        if !instance.visible {
            continue;
        }
        let Some(&(_, colour)) = EFFECT_COLOURS
            .iter()
            .find(|(name, _)| *name == instance.effect)
        else {
            continue;
        };
        let level = instance.amplifier.wrapping_add(1);
        red = red.wrapping_add(level.wrapping_mul((colour >> 16) & 255));
        green = green.wrapping_add(level.wrapping_mul((colour >> 8) & 255));
        blue = blue.wrapping_add(level.wrapping_mul(colour & 255));
        weight = weight.wrapping_add(level);
    }
    if weight == 0 {
        return BASE_POTION_COLOUR;
    }
    // `ARGB.color(r, g, b)`: opaque.
    let (r, g, b) = (red / weight, green / weight, blue / weight);
    (0xFF00_0000_u32 as i32) | ((r & 255) << 16) | ((g & 255) << 8) | (b & 255)
}

/// `MobEffectInstance.mapDuration(f)`: an infinite or spent duration is kept.
pub(crate) fn map_duration(duration: i32, f: impl FnOnce(i32) -> i32) -> i32 {
    if duration == -1 || duration == 0 {
        duration
    } else {
        f(duration)
    }
}

/// `MobEffectInstance.withScaledDuration(scale)`: `max(floor(d · scale), 1)`.
pub(crate) fn scaled(instance: &EffectInstance, scale: f32) -> EffectInstance {
    EffectInstance {
        duration: map_duration(instance.duration, |duration| {
            ((duration as f32 * scale).floor() as i32).max(1)
        }),
        ..instance.clone()
    }
}

/// The face of the player whose stay is the entity `key`, of another region, and where
/// the region last saw them: by their face, or the world's players.
pub(crate) fn far_player_of(region: &Region, key: EntityId) -> Option<(PlayerId, Vec3, bool)> {
    if let Some((id, kept)) = region
        .faces
        .kept
        .iter()
        .find(|(_, kept)| kept.entity == key)
    {
        return Some((*id, kept.face.position, kept.face.sleeping.is_some()));
    }
    region
        .faces
        .world
        .iter()
        .flatten()
        .find(|player| player.entity == key)
        .map(|player| (player.player, player.position, player.sleeping.is_some()))
}

impl Region {
    /// The tick of the thrown item `entity` of the loaded chunk `position`
    /// (`ThrowableProjectile.tick`): gravity, then drag, then the hit test along its
    /// move, which puts it where it hit or where its move takes it; its turn towards
    /// its motion; `Projectile.tick` (its first tick's `projectile_shoot`, whether it
    /// has left its owner, the fluids it is in and their push, the floor of the world);
    /// and then the hit.
    pub(crate) fn tick_thrown(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Thrown {
            ref stack,
            owner,
            left_owner,
            has_been_shot,
        } = entity.data
        else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let in_water = self
            .entity_memory
            .get(&key)
            .is_some_and(|memory| memory.cart.in_water);
        // `applyGravity`, then `applyInertia`.
        let mut motion = [
            entity.motion[0] + 0.0,
            entity.motion[1] + -thrown::gravity_of(entity.kind),
            entity.motion[2] + 0.0,
        ];
        let drag = if in_water {
            thrown::WATER_DRAG
        } else {
            thrown::AIR_DRAG
        };
        motion = physics::scale(motion, f64::from(drag));
        // `ProjectileUtil.getHitResultOnMoveVector(this, canHitEntity)`.
        let owner_root = owner.and_then(|owner| self.unit_root(owner));
        let flying = Flying {
            key,
            width: info.width,
            height: info.height,
            owner_root,
            left_owner,
            tick_count: entity.tick_count().saturating_add(1),
            fall_distance: entity.fall_distance,
        };
        let struck = self.hit_on_move(&flying, position, entity.position, motion);
        let location = struck.map_or_else(
            || physics::add(entity.position, motion),
            |struck| struck.location(),
        );
        let [yaw, pitch] = thrown::update_rotation(entity.yaw, entity.pitch, motion);
        // `Projectile.tick`: the first tick's `projectile_shoot`, of the owner, where the
        // projectile is now.
        if !has_been_shot {
            self.shot_vibration(owner, owner_root, location);
        }
        // `checkLeftOwner`, from where it is now.
        let left_owner = left_owner || self.has_left_owner(key, owner_root, location, info, motion);
        // `baseTick`: the fluids it is in, and their push; below the world it is gone.
        let view = View {
            region: self,
            home: position,
        };
        let found = fluids::update(&view, &Aabb::of_entity(location, info.width, info.height));
        motion = fluids::push(&found, motion);
        if self.below_the_world(position, location) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let after = Entity {
            position: location,
            motion,
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            data: EntityData::Thrown {
                stack: stack.clone(),
                owner,
                left_owner,
                has_been_shot: true,
            },
            ..entity.clone()
        };
        if let Some(struck) = struck {
            // `hitTargetOrDeflectSelf`: no entity of Clustine's deflects, and a block hit
            // on air is none (a hit is never on air: the clip finds collision shapes).
            self.thrown_struck(&entity, &after, struck, flying.tick_count, output);
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        self.entity_memory.insert(
            key,
            Memory {
                cart: CartMemory {
                    in_water: found.in_water(),
                    ..CartMemory::default()
                },
                ..Memory::default()
            },
        );
        self.write_projectile(&entity, after, position, output);
    }

    /// Whether `at` lies far enough below the floor of the world for an entity there to
    /// be gone (`checkBelowWorld`).
    pub(crate) fn below_the_world(&self, position: ChunkPos, at: Vec3) -> bool {
        let min_y = self
            .chunks
            .get(&position)
            .map_or(-64, |chunk| chunk.min_y());
        at[1] < f64::from(min_y) - BELOW_THE_WORLD
    }

    /// `Projectile.tick`'s first `gameEvent(PROJECTILE_SHOOT, getOwner())` where the
    /// projectile is.
    pub(crate) fn shot_vibration(
        &mut self,
        owner: Option<EntityId>,
        owner_root: Option<EntityId>,
        at: Vec3,
    ) {
        let source = owner.filter(|_| owner_root.is_some()).map(Source::of);
        self.vibrate(Vibration::PROJECTILE_SHOOT, at, Context::of(source, None));
    }

    /// `Projectile.checkLeftOwner()` of the projectile `key` at `at`, moving by
    /// `motion`: nothing of its owner's unit is pickable within a block of its move.
    pub(crate) fn has_left_owner(
        &self,
        key: EntityId,
        owner_root: Option<EntityId>,
        at: Vec3,
        info: &EntityTypeInfo,
        motion: Vec3,
    ) -> bool {
        let reach = Aabb::of_entity(at, info.width, info.height)
            .expand_towards(motion)
            .inflate([1.0; 3]);
        owner_root.is_none_or(|root| {
            !self
                .targets(&reach, key)
                .iter()
                .any(|target| target.pickable && target.root == root)
        })
    }

    /// `ProjectileUtil.getHitResultOnMoveVector(projectile, canHitEntity)` of `flying`
    /// at `from` moving by `motion`: the block its move meets (`Level.clip`), and the
    /// nearest entity it may hit whose box, grown by the margin, the move meets before
    /// that; the entity where there is one.
    pub(crate) fn hit_on_move(
        &self,
        flying: &Flying,
        position: ChunkPos,
        from: Vec3,
        motion: Vec3,
    ) -> Option<Struck> {
        let can_hit = |target: &Target| {
            target.pickable
                && target.alive
                && (flying.left_owner || flying.owner_root.is_none_or(|root| target.root != root))
        };
        let to = physics::add(from, motion);
        let collision = Collision {
            bottom: from[1],
            fall_distance: flying.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let view = View {
            region: self,
            home: position,
        };
        let block = thrown::clip_blocks_faced(&view, collision, from, to);
        let end = block.map_or(to, |(hit, _)| hit.location);
        let search = Aabb::of_entity(from, flying.width, flying.height)
            .expand_towards(motion)
            .inflate([1.0; 3]);
        let margin = f64::from(thrown::margin(flying.tick_count));
        let mut nearest = f64::MAX;
        let mut struck = None;
        for target in self.targets(&search, flying.key) {
            if !can_hit(&target) {
                continue;
            }
            let Some(at) = target.bounds.inflate([margin; 3]).clip(from, end) else {
                continue;
            };
            let distance = physics::distance_sqr(from, at);
            if distance < nearest {
                nearest = distance;
                struck = Some(Struck::Entity {
                    target: target.key,
                    kind: target.kind,
                    at,
                });
            }
        }
        struck.or(block.map(|(hit, face)| Struck::Block { hit, face }))
    }

    /// `Projectile.onHit` of the thrown item `before`, which is `after` once it moved,
    /// at what it struck: its kind's `onHitEntity` or `onHitBlock`, the hook of the block
    /// hit and `projectile_land`, then its kind's own `onHit`, whose draws are its own.
    /// It is gone after.
    fn thrown_struck(
        &mut self,
        before: &Entity,
        after: &Entity,
        struck: Struck,
        tick_count: u32,
        output: &mut TickOutput,
    ) {
        let key = after.key;
        let kind = i32::from(after.kind);
        let EntityData::Thrown { stack, owner, .. } = &after.data else {
            return;
        };
        let (stack, owner) = (stack.clone(), *owner);
        let hurts = kind == entity_types::SNOWBALL
            || kind == entity_types::EGG
            || kind == entity_types::ENDER_PEARL;
        match struck {
            Struck::Entity {
                target,
                kind: target_kind,
                at,
            } => {
                // `onHitEntity`: the snowball's, the egg's and the pearl's hurt of
                // nothing; a potion's and a bottle's do nothing. Another region's player,
                // a face here, is creative and takes nothing (section 4.1).
                if hurts && target_kind != TargetKind::Face {
                    let hit = Hit {
                        projectile: key,
                        kind: after.kind,
                        at: after.position,
                        motion: after.motion,
                        damage: 0.0,
                        owner: owner.and_then(|owner| self.thrower(owner)),
                    };
                    self.thrown_hit(target, hit, output);
                }
                self.vibrate(
                    Vibration::PROJECTILE_LAND,
                    at,
                    Context::of(Some(Source::of(key)), None),
                );
            }
            Struck::Block { hit, face } => {
                let state = self.block(hit.pos);
                if let Some(state) = state {
                    self.projectile_hit(after, state, &hit, output);
                }
                // `AbstractThrownPotion.onHitBlock`: water puts out fire about the side
                // hit.
                let potion =
                    kind == entity_types::SPLASH_POTION || kind == entity_types::LINGERING_POTION;
                if potion && douses_fire(&potion_of(&stack)) {
                    self.douse_about(hit.pos, face, output);
                }
                self.vibrate_at(
                    Vibration::PROJECTILE_LAND,
                    hit.pos,
                    Context::of(Some(Source::of(key)), state),
                );
            }
        }
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        if kind == entity_types::EGG {
            self.hatch(after, &stack, &mut own, output);
        } else if kind == entity_types::SPLASH_POTION || kind == entity_types::LINGERING_POTION {
            self.potion_breaks(after, &stack, owner, struck, tick_count, output);
        } else if kind == entity_types::EXPERIENCE_BOTTLE {
            self.bottle_breaks(after, struck, &mut own, output);
        } else if kind == entity_types::ENDER_PEARL {
            self.pearl_lands(before.position, owner, &mut own, output);
        }
        self.entity_drew(key, own.draws_since(&start));
        if kind == entity_types::SNOWBALL || kind == entity_types::EGG {
            // Its particles, before it is gone.
            output.events.push(RegionEvent::EntityEvent {
                entity: key,
                chunk: before.chunk(),
                event: HIT_PARTICLES,
            });
        }
    }

    /// `onHitEntity` of a projectile's hit `hit` on the entity or player `target`: done
    /// here where the region has the entity; a player is creative and takes nothing; an
    /// entity the region sees only in a copy is hit by its holder.
    fn thrown_hit(&mut self, target: EntityId, hit: Hit, output: &mut TickOutput) {
        if self.find_entity(target).is_some() {
            self.thrown_hit_here(target, hit, output);
            return;
        }
        if self.player_with_entity(target).is_some() {
            // A creative player is invulnerable to everything that a snowball deals.
            return;
        }
        self.send_on_entity(target, hit.as_use());
    }

    /// `target.hurt(thrown(projectile, owner), damage)` of the region's entity `target`,
    /// as its kind takes it: a mob by its body's hurt, pushed along the projectile's
    /// motion; a vehicle wobbles, or a creative player's throw takes it away; a hanging
    /// thing breaks, or a frame gives up its item; a falling block is only marked hurt;
    /// an armour stand is not hurt by a throw (`#can_break_armor_stand`), nor primed TNT,
    /// an item or an orb (none of which a projectile hits).
    pub(crate) fn thrown_hit_here(&mut self, target: EntityId, hit: Hit, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(target) else {
            return;
        };
        let record = record.clone();
        let creative = hit.by_creative();
        match &record.data {
            EntityData::Mob { .. } => {
                self.hurt_mob(target, &hit.source(), hit.damage, output);
            }
            EntityData::Minecart { .. } | EntityData::Boat { .. } => {
                self.hurt_vehicle(target, hit.damage, creative, output);
            }
            EntityData::ItemFrame { .. } => self.hurt_frame(&record, creative, output),
            EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. } => {
                let player = hit.owner.and_then(|owner| owner.player);
                self.hurt_attached(target, Breaker::of_owner(player), output);
            }
            EntityData::FallingBlock { .. } => {
                // `markHurt`: its motion is sent.
                let mut report = tracked(&record, Tracked::SYNC_VELOCITY, false);
                report.chunk = chunk;
                output.events.push(RegionEvent::EntityTracked(report));
            }
            EntityData::ArmorStand { .. }
            | EntityData::PrimedTnt { .. }
            | EntityData::Item { .. }
            | EntityData::ExperienceOrb { .. }
            | EntityData::Thrown { .. }
            | EntityData::Arrow(_)
            | EntityData::Spit(_)
            | EntityData::Firework(_)
            | EntityData::Cloud(_)
            | EntityData::Fangs(_) => {}
        }
    }

    /// The living entities whose boxes meet `bounds`, but `except`, in the hit test's
    /// order: the region's mobs and those of its copies, its players, and the faces of
    /// other regions' players (`getEntitiesOfClass(LivingEntity, bounds)`).
    pub(crate) fn living_in(&self, bounds: &Aabb, except: EntityId) -> Vec<Living> {
        let mut found = Vec::new();
        for target in self.targets(bounds, except) {
            let living = match target.kind {
                TargetKind::Here | TargetKind::Copy => {
                    let record = if target.kind == TargetKind::Here {
                        self.find_entity(target.key)
                            .map(|(_, record)| record.clone())
                    } else {
                        self.copied_entity(target.key).map(|(_, record)| record)
                    };
                    let Some(record) = record else {
                        continue;
                    };
                    if !matches!(record.data, EntityData::Mob { .. }) {
                        continue;
                    }
                    Living {
                        target,
                        kind: Some(record.kind),
                        position: record.position,
                        burning: record.fire_ticks > 0,
                    }
                }
                TargetKind::Player => {
                    let Some(player) = self
                        .player_with_entity(target.key)
                        .and_then(|id| self.players.get(&id))
                    else {
                        continue;
                    };
                    let at = player.pose.position;
                    Living {
                        target,
                        kind: None,
                        position: [at.x, at.y, at.z],
                        burning: false,
                    }
                }
                TargetKind::Face => {
                    let Some((_, at, _)) = far_player_of(self, target.key) else {
                        continue;
                    };
                    Living {
                        target,
                        kind: None,
                        position: at,
                        burning: false,
                    }
                }
            };
            found.push(living);
        }
        found
    }

    /// Effects `given` to the living entity `living`, where it is: taken here for the
    /// region's own mob or player, sent to a copied mob's holder, and to another
    /// region's player by their stay.
    pub(crate) fn give_effects(
        &mut self,
        living: &Living,
        given: GivenEffects,
        output: &mut TickOutput,
    ) {
        let key = living.target.key;
        let by = EntityUse::Effects(Box::new(given));
        match living.target.kind {
            TargetKind::Here => {
                if let Some((_, record)) = self.find_entity(key) {
                    let record = record.clone();
                    let EntityUse::Effects(given) = &by else {
                        return;
                    };
                    self.take_effects(&record, given, output);
                }
            }
            TargetKind::Copy => {
                self.send_on_entity(key, by);
            }
            TargetKind::Player => self.player_used(key, by, output),
            TargetKind::Face => {
                if let Some((player, at, _)) = far_player_of(self, key) {
                    self.send_on_player(player, key, physics::containing(at), by);
                }
            }
        }
    }

    /// The hurt `hurt` of the living entity `living`, where it is: taken here for the
    /// region's own mob, sent to a copied mob's holder; a player, the region's or
    /// another's, is creative and refuses it.
    pub(crate) fn give_hurt(
        &mut self,
        living: &Living,
        hurt: clustine_world::HurtBy,
        output: &mut TickOutput,
    ) {
        self.hurt_target(living.target.key, living.target.kind, hurt, output);
    }

    /// The hurt `hurt` of the entity `key` the hit test listed as `kind`: taken here by
    /// the region's own entity as its kind takes it (`entity_effects.rs`' `take_hurt`),
    /// sent to the holder of one in a copy; a player is creative and refuses it.
    pub(crate) fn hurt_target(
        &mut self,
        key: EntityId,
        kind: TargetKind,
        hurt: clustine_world::HurtBy,
        output: &mut TickOutput,
    ) {
        match kind {
            TargetKind::Here => {
                if let Some((_, record)) = self.find_entity(key) {
                    let record = record.clone();
                    self.take_hurt(&record, &hurt, output);
                }
            }
            TargetKind::Copy => {
                self.send_on_entity(key, EntityUse::Hurt(Box::new(hurt)));
            }
            TargetKind::Player | TargetKind::Face => {}
        }
    }

    /// Who caused a hurt or gave effects, as the record of the hurt names them.
    pub(crate) fn causer_of(&self, key: EntityId) -> Option<clustine_world::Causer> {
        let thrower = self.thrower(key)?;
        Some(clustine_world::Causer {
            key,
            kind: thrower.kind,
            position: thrower.from,
            player: thrower.player,
            living: true,
        })
    }

    /// `ThrownEgg.onHit`: own `nextInt(8)`, and on 0 own `nextInt(32)`, for one chick or
    /// four; each made at the egg with its yaw, a baby of the stack's variant
    /// (`chicken/variant`, the item's default), no `finalizeSpawn`; put clear of the
    /// blocks (`fudgePositionAfterSizeChange` from no size), and where that finds no
    /// room, no more are made.
    fn hatch(
        &mut self,
        egg: &Entity,
        stack: &ItemStack,
        own: &mut Random,
        output: &mut TickOutput,
    ) {
        if own.next_int(8) != 0 {
            return;
        }
        let count = if own.next_int(32) == 0 { 4 } else { 1 };
        let kind = u16::try_from(entity_types::CHICKEN).expect("entity type ids fit 16 bits");
        for _ in 0..count {
            let chick = chick_of(stack);
            let size = crate::mobs::types::of(kind).dimensions(&chick);
            let info = EntityTypeInfo::of(entity_types::CHICKEN);
            let (width, height) =
                size.map_or((info.width, info.height), |size| (size.width, size.height));
            let Some(at) = self.fudged(egg.position, width, height) else {
                break;
            };
            if let Some(made) = self.new_mob(kind, chick, at, egg.yaw, 0.0) {
                self.add_made_mob(made, output);
            }
        }
    }

    /// `Entity.fudgePositionAfterSizeChange(ZERO_SIZED_DIMENSIONS)` of an entity of
    /// `width` and `height` at `at`: the nearest place about it whose box the blocks
    /// leave free, else the nearest about it at its height; none where neither is.
    fn fudged(&self, at: Vec3, width: f32, height: f32) -> Option<Vec3> {
        let dw = f64::from(0.0_f32.max(width - 0.0)) + 1.0e-6;
        let dh = f64::from(0.0_f32.max(height - 0.0)) + 1.0e-6;
        let (w, h) = (f64::from(width), f64::from(height));
        let view = View {
            region: self,
            home: ChunkPos::containing(at[0], at[2]),
        };
        let context = Collision {
            bottom: at[1],
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        let shape = thrown::of_size(at, [dw, dh, dw]);
        if let Some(free) = thrown::find_free_position(&view, context, shape, at, [w, h, w]) {
            return Some([free[0], free[1] - h / 2.0, free[2]]);
        }
        if width > 0.0 && height > 0.0 {
            let flat = thrown::of_size(at, [dw, 1.0e-6, dw]);
            if let Some(free) = thrown::find_free_position(&view, context, flat, at, [w, 0.0, w]) {
                return Some([free[0], free[1] + 1.0e-6, free[2]]);
            }
        }
        None
    }

    /// `AbstractThrownPotion.onHit` of the potion `potion`: water's behaviours about
    /// it, then where it has effects the splash's or the cloud's
    /// (`onHitAsPotion`), then its level events: the instant splash for a base potion
    /// that is instant, the other for any other, each with its sound.
    fn potion_breaks(
        &mut self,
        potion: &Entity,
        stack: &ItemStack,
        owner: Option<EntityId>,
        struck: Struck,
        tick_count: u32,
        output: &mut TickOutput,
    ) {
        let contents = potion_of(stack);
        if is_water(&contents) {
            self.water_about(potion, owner, output);
        }
        if has_effects(&contents) {
            if i32::from(potion.kind) == entity_types::SPLASH_POTION {
                self.splash(potion, stack, &contents, owner, tick_count, output);
            } else {
                self.linger(potion, stack, &contents, owner, struck, output);
            }
        }
        let instant = base_effects(&contents)
            .iter()
            .any(|instance| is_instantaneous(instance.effect));
        let (event, sound) = if contents.potion.is_some() && instant {
            (INSTANT_SPLASH, INSTANT_SPLASH_SOUND)
        } else {
            (SPLASH, SPLASH_SOUND)
        };
        let at = physics::containing(potion.position);
        output.events.push(RegionEvent::LevelEvent {
            kind: event,
            position: at,
            data: potion_colour(&contents),
        });
        output.events.push(RegionEvent::LevelEvent {
            kind: sound,
            position: at,
            data: 0,
        });
    }

    /// The potion or cloud `key` of `kind` at `at` as the cause of what it gives.
    fn giver(key: EntityId, kind: u16, at: Vec3) -> clustine_world::Causer {
        clustine_world::Causer {
            key,
            kind,
            position: at.map(Double),
            player: None,
            living: false,
        }
    }

    /// `affectEntitiesAround` of a potion of water: each mob within four blocks, centre
    /// to centre, in its box grown by (4, 2, 4), hurt by `indirectMagic` 1 where water
    /// hurts it (the holder's rule, `entity_effects.rs`) and put out where it burns.
    fn water_about(&mut self, potion: &Entity, owner: Option<EntityId>, output: &mut TickOutput) {
        let info = EntityTypeInfo::of(i32::from(potion.kind));
        let area =
            Aabb::of_entity(potion.position, info.width, info.height).inflate([4.0, 2.0, 4.0]);
        let source = Self::giver(potion.key, potion.kind, potion.position);
        let owner = owner.and_then(|owner| self.causer_of(owner));
        for living in self.living_in(&area, potion.key) {
            let Some(kind) = living.kind else {
                continue;
            };
            let sensitive = SENSITIVE_TO_WATER.contains(&i32::from(kind));
            if !(sensitive || living.burning)
                || physics::distance_sqr(potion.position, living.position) >= 16.0
            {
                continue;
            }
            if sensitive {
                let water = GivenEffects {
                    instances: Vec::new(),
                    instant_factor: Float(1.0),
                    source: Some(source.clone()),
                    owner: owner.clone(),
                    water: true,
                };
                self.give_effects(&living, water, output);
            }
            if living.burning && living.target.kind == TargetKind::Here {
                let alive = self
                    .find_entity(living.target.key)
                    .is_some_and(|(_, record)| super::projectiles::pickable_and_alive(record).1);
                if let (true, Some((chunk, _))) = (alive, self.find_entity(living.target.key)) {
                    self.commit_entity(
                        chunk,
                        living.target.key,
                        vec![EntityField::FireTicks(0)],
                        output,
                    );
                }
            }
        }
    }

    /// `ThrownSplashPotion.onHitAsPotion`: every living entity whose box, grown by the
    /// margin, lies within four blocks of the potion's box (box to box) is given each
    /// effect by its proximity `1 - d / 4`: an instant one with that factor, any other
    /// for `(int) (prox · duration · scale + 0.5)` ticks unless that ends within 20.
    fn splash(
        &mut self,
        potion: &Entity,
        stack: &ItemStack,
        contents: &PotionContents,
        owner: Option<EntityId>,
        tick_count: u32,
        output: &mut TickOutput,
    ) {
        let info = EntityTypeInfo::of(i32::from(potion.kind));
        let scale = duration_scale_of(stack);
        let effects = all_effects(contents);
        // At the hit the potion is where it hit: its box moved to the hit is its own.
        let moved = Aabb::of_entity(potion.position, info.width, info.height);
        let area = moved.inflate([4.0, 2.0, 4.0]);
        let margin = f64::from(thrown::margin(tick_count));
        let source = Self::giver(potion.key, potion.kind, potion.position);
        let owner = owner.and_then(|owner| self.causer_of(owner));
        for living in self.living_in(&area, potion.key) {
            // `isAffectedByPotions`: alive.
            if !living.target.alive {
                continue;
            }
            let d2 = thrown::box_distance_sqr(&moved, &living.target.bounds.inflate([margin; 3]));
            if d2 >= 16.0 {
                continue;
            }
            let proximity = 1.0 - d2.sqrt() / 4.0;
            let mut instances = Vec::new();
            for effect in &effects {
                if is_instantaneous(&effect.effect) {
                    instances.push(effect.clone());
                    continue;
                }
                let duration = map_duration(effect.duration, |duration| {
                    (proximity * f64::from(duration) * f64::from(scale) + 0.5) as i32
                });
                // `endsWithin(20)`: a finite duration of 20 or fewer.
                if duration != -1 && duration <= 20 {
                    continue;
                }
                instances.push(EffectInstance {
                    duration,
                    show_icon: effect.visible,
                    ..effect.clone()
                });
            }
            if instances.is_empty() {
                continue;
            }
            let given = GivenEffects {
                instances,
                instant_factor: Float(proximity as f32),
                source: Some(source.clone()),
                owner: owner.clone(),
                water: false,
            };
            self.give_effects(&living, given, output);
        }
    }

    /// `ThrownLingeringPotion.onHitAsPotion`: a cloud of radius 3 at the entity hit or
    /// at the potion, shrinking by `radius / 600` a tick and by half a block at each
    /// use, lasting 600 ticks after waiting 10, of the stack's contents and duration
    /// scale, owned by the thrower; made by section 4.1's rule (`make_cloud`).
    fn linger(
        &mut self,
        potion: &Entity,
        stack: &ItemStack,
        contents: &PotionContents,
        owner: Option<EntityId>,
        struck: Struck,
        output: &mut TickOutput,
    ) {
        let at = match struck {
            Struck::Entity { target, kind, .. } => self.position_of(target, kind),
            Struck::Block { .. } => None,
        }
        .unwrap_or(potion.position);
        let radius = 3.0_f32;
        let duration = 600;
        let cloud = Cloud {
            duration,
            wait_time: 10,
            reapplication_delay: 20,
            radius: Float(radius),
            radius_at: 0,
            radius_on_use: Float(-0.5),
            radius_per_tick: Float(-radius / duration as f32),
            duration_on_use: 0,
            potion: contents.clone(),
            duration_scale: Float(duration_scale_of(stack)),
            owner,
            victims: Vec::new(),
        };
        self.make_cloud(at, cloud, output);
    }

    /// The feet of the entity or player `key` the hit test listed as `kind`.
    fn position_of(&self, key: EntityId, kind: TargetKind) -> Option<Vec3> {
        match kind {
            TargetKind::Here => self.find_entity(key).map(|(_, record)| record.position),
            TargetKind::Copy => self.copied_entity(key).map(|(_, record)| record.position),
            TargetKind::Player => {
                let at = self
                    .players
                    .get(&self.player_with_entity(key)?)?
                    .pose
                    .position;
                Some([at.x, at.y, at.z])
            }
            TargetKind::Face => far_player_of(self, key).map(|(_, at, _)| at),
        }
    }

    /// `ThrownExperienceBottle.onHit`: its splash of the base colour and its sound, then
    /// `3 + nextInt(5) + nextInt(5)` experience, its own draws, awarded in orbs at the
    /// hit along the side hit or back against its motion (`awardWithDirection`).
    fn bottle_breaks(
        &mut self,
        bottle: &Entity,
        struck: Struck,
        own: &mut Random,
        output: &mut TickOutput,
    ) {
        let at = physics::containing(bottle.position);
        output.events.push(RegionEvent::LevelEvent {
            kind: SPLASH,
            position: at,
            data: BASE_POTION_COLOUR,
        });
        output.events.push(RegionEvent::LevelEvent {
            kind: SPLASH_SOUND,
            position: at,
            data: 0,
        });
        let first = own.next_int(5) as i32;
        let experience = 3 + first + own.next_int(5) as i32;
        let direction = match struck {
            Struck::Block { face, .. } => thrown::unit_of(face),
            Struck::Entity { .. } => physics::scale(bottle.motion, -1.0),
        };
        self.award_with_direction(struck.location(), direction, experience, output);
    }

    /// `ThrownEnderpearl.onHit` after its hurt: its 32 particles' 96 draws, then its
    /// owner, where allowed (not asleep), a player of the region or of another: one own
    /// `nextFloat() < 0.05` for an endermite, and the landing at `from`, where the pearl
    /// was as its tick began, sent to the player's stay.
    fn pearl_lands(
        &mut self,
        from: Vec3,
        owner: Option<EntityId>,
        own: &mut Random,
        output: &mut TickOutput,
    ) {
        for _ in 0..32 {
            own.next_double();
            own.next_gaussian();
            own.next_gaussian();
        }
        let Some(owner) = owner else {
            return;
        };
        let landed = |endermite| EntityUse::PearlLanded {
            at: from.map(Double),
            endermite,
        };
        if let Some(id) = self.player_with_entity(owner) {
            if self
                .players
                .get(&id)
                .is_none_or(|player| player.sleeping.is_some())
            {
                return;
            }
            let endermite = own.next_float() < 0.05;
            self.player_used(owner, landed(endermite), output);
            return;
        }
        if let Some((player, at, sleeping)) = far_player_of(self, owner) {
            if sleeping {
                return;
            }
            let endermite = own.next_float() < 0.05;
            self.send_on_player(player, owner, physics::containing(at), landed(endermite));
        }
    }

    /// `douseFire` about the block `pos` hit on its side `face`
    /// (`AbstractThrownPotion.onHitBlock`): the block beside it on that side, the block
    /// itself, and the four beside the first.
    fn douse_about(&mut self, pos: BlockPos, face: Face, output: &mut TickOutput) {
        let [x, y, z] = thrown::unit_of(face).map(|value| value as i32);
        let beside = pos.offset(x, y, z);
        self.douse(beside, output);
        self.douse(pos, output);
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            self.douse(beside.offset(dx, 0, dz), output);
        }
    }

    /// `AbstractThrownPotion.douseFire(pos)`: fire goes; a lit candle is put out with its
    /// sound; a lit campfire is put out with its level event. Where the region cannot
    /// set the block, it is set by the effect of a set, as any rule's.
    fn douse(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if in_tag(state, "minecraft:fire") {
            self.destroy_block(pos, false, UPDATE_LIMIT, output);
            return;
        }
        let candle = in_tag(state, "minecraft:candles") || in_tag(state, "minecraft:candle_cakes");
        if candle && flag(state, "lit") {
            self.set_block_updated(
                pos,
                set_flag(state, "lit", false),
                UPDATE_ALL,
                UPDATE_LIMIT,
                output,
            );
            output.events.push(RegionEvent::Sound {
                sound: sounds::BLOCK_CANDLE_EXTINGUISH,
                source: crate::api::SOUND_SOURCE_BLOCKS,
                position: clustine_world::Vec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
            self.vibrate_at(Vibration::BLOCK_CHANGE, pos, Context::of(None, Some(state)));
            return;
        }
        if in_tag(state, "minecraft:campfires") && flag(state, "lit") {
            output.events.push(RegionEvent::LevelEvent {
                kind: CAMPFIRE_PUT_OUT,
                position: pos,
                data: 0,
            });
            self.vibrate_at(Vibration::BLOCK_CHANGE, pos, Context::of(None, Some(state)));
            self.set_block_updated(
                pos,
                set_flag(state, "lit", false),
                UPDATE_ALL,
                UPDATE_LIMIT,
                output,
            );
        }
    }
}

/// Whether the potion is water (`#douses_fire`, `#hurts_water_sensitive_entities`,
/// `#extinguishes_entities`: each holds water alone).
fn is_water(contents: &PotionContents) -> bool {
    contents.potion.as_deref() == Some("minecraft:water")
}

/// `#douses_fire`.
fn douses_fire(contents: &PotionContents) -> bool {
    is_water(contents)
}

/// A chick as an egg of `stack` makes it: a baby of the stack's `chicken/variant`, or
/// the item's default, or the chicken's own; its other state the chicken's defaults.
fn chick_of(stack: &ItemStack) -> crate::mobs::Mob {
    use crate::mobs::types::animal::{self, Animal, BABY_START_AGE};
    use crate::mobs::types::chicken::ChickenState;
    let named = if stack.components.removes(Kind::ChickenVariant) {
        None
    } else if let Some(Component::ChickenVariant(name)) = stack.components.get(Kind::ChickenVariant)
    {
        Some(name.clone())
    } else {
        ItemComponents::of(stack.item)
            .chicken_variant
            .map(str::to_owned)
    };
    let mut own = ChickenState::default();
    if let Some(variant) = named
        .and_then(|name| clustine_data::synced_registry("minecraft:chicken_variant")?.id_of(&name))
        .and_then(|id| u8::try_from(id).ok())
    {
        own.variant = variant;
    }
    let mut animal = Animal::default();
    animal.set_age(BABY_START_AGE, 0);
    let mut chick = crate::mobs::Mob::fresh();
    chick.state = animal::write(&animal, &own);
    chick
}

#[cfg(test)]
#[path = "thrown_n9_tests.rs"]
mod thrown_n9_tests;
