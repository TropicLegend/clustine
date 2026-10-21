//! A living body without a mind: `LivingEntity` as far as an armour stand meets it
//! (`docs/groundwork/n5-brief.md`, finding 7 and section 5.5; `docs/groundwork/n5-plan.md`,
//! decision 4). It has no AI, no input, no effects and no damage path but its kind's
//! own; what it keeps is the game's tick of such a body: `LivingEntity.baseTick` (the
//! fluids it is in, its fire, its air under water), `aiStep` (the zeroing of small
//! motion, `travel` in air, water and lava with the gravity, friction and drag of its
//! attributes, the effects of the blocks it is in and powder snow's freezing), and the
//! fall's sounds of `checkFallDamage`. The region does the rest around it: what it
//! wears, its pushing of carts, its record (`region/stands.rs`). Living entities of N7
//! extend this rather than replace it.
//!
//! As the physics are (`crate::physics`), the arithmetic is the game's to the bit and
//! nothing here reads anything but the blocks and entities a [`World`] gives.
//!
//! Left out, and named: the particles of a landing, of bubbles and of breaking (the
//! repo sends no particles yet); the splash of entering water (as for items, N4);
//! bubble columns' push and a honey block's slide, which items leave out too; turtle
//! eggs trampled; the enchantments of worn items (Clustine's stacks have none); and
//! the attribute modifiers of worn items, which no stack Clustine has carries.

mod inside;
mod travel;

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, sounds};
use clustine_world::BlockPos;

use crate::physics::body::{Body, Movement};
use crate::physics::fluids::{self, InFluids};
use crate::physics::item::{Draws, Region as Holds, Sound};
use crate::physics::{self, World};

/// The game's `EquipmentSlot`s by their ordinals, which a living body's record keeps
/// what it wears by and `SetEquipment` names.
pub(crate) mod slots {
    pub(crate) const MAIN_HAND: u8 = 0;
    pub(crate) const OFF_HAND: u8 = 1;
    pub(crate) const FEET: u8 = 2;
    pub(crate) const LEGS: u8 = 3;
    pub(crate) const CHEST: u8 = 4;
    pub(crate) const HEAD: u8 = 5;
    pub(crate) const BODY: u8 = 6;
    pub(crate) const SADDLE: u8 = 7;

    /// `getFilterBit(0)`: the slot's id, which is not its ordinal (the off hand's is 5,
    /// after the armour's).
    pub(crate) fn filter_bit(slot: u8) -> u32 {
        match slot {
            MAIN_HAND => 0,
            OFF_HAND => 5,
            FEET => 1,
            LEGS => 2,
            CHEST => 3,
            HEAD => 4,
            other => u32::from(other),
        }
    }

    /// Whether the slot is a hand (`EquipmentSlot.Type.HAND`).
    pub(crate) fn is_hand(slot: u8) -> bool {
        slot == MAIN_HAND || slot == OFF_HAND
    }
}

/// The most air a body holds (`getMaxAirSupply`).
pub(crate) const MAX_AIR: i32 = 300;

/// The ticks in powder snow that freeze a body through (`getTicksRequiredToFreeze`).
pub(crate) const TICKS_TO_FREEZE: i32 = 140;

/// The values of a body's attributes that its tick reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Attributes {
    pub(crate) gravity: f64,
    pub(crate) safe_fall_distance: f64,
    pub(crate) fall_damage_multiplier: f64,
    pub(crate) friction_modifier: f64,
    pub(crate) air_drag_modifier: f64,
    pub(crate) water_movement_efficiency: f64,
    pub(crate) bounciness: f64,
    pub(crate) scale: f32,
}

impl Attributes {
    /// `createLivingAttributes()`'s defaults, which an armour stand keeps but for its
    /// step height of 0 (`ArmorStand.createAttributes`, which the physics never step
    /// with anyway). **Inferred**: the values are in `Attributes`, which is not in the
    /// dump; these are the game's as its wiki and the reference servers give them
    /// (gravity 0.08, a safe fall of 3, modifiers of 1, no water efficiency, no
    /// bounciness, a scale of 1). No stack Clustine has changes them.
    pub(crate) const LIVING: Self = Self {
        gravity: 0.08,
        safe_fall_distance: 3.0,
        fall_damage_multiplier: 1.0,
        friction_modifier: 1.0,
        air_drag_modifier: 1.0,
        water_movement_efficiency: 0.0,
        bounciness: 0.0,
        scale: 1.0,
    };
}

/// What hurts a body in its tick, as the game's damage types that a living body
/// without a mind meets of itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Damage {
    /// `inFire`: standing in fire or soul fire.
    InFire,
    /// `campfire`: standing in a lit campfire.
    Campfire,
    /// `onFire`: burning.
    OnFire,
    /// `lava`.
    Lava,
    /// `fellOutOfWorld`: far below the world.
    OutOfWorld,
    /// `inWall`: its eyes in a block that suffocates (a mob's alone).
    InWall,
    /// `drown`: out of air under water (a mob's alone).
    Drown,
    /// `fall`: a fall beyond its safe distance, its damage counted (a mob's alone).
    Fall,
    /// `freeze`: frozen through in powder snow (a mob's alone).
    Freeze,
    /// `dryOut`: a dolphin out of water too long.
    DryOut,
}

/// The kind of a body, for what only its kind decides: how it takes damage
/// (`hurtServer`) and the sound of its fall.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// An armour stand, with its health and whether nothing but the void hurts it (its
    /// own field of being invisible, or being a marker).
    ArmorStand { health: f32, invulnerable: bool },
    /// A mob of the entity type `kind`, whose hurts its body takes
    /// (`crate::mobs::body::hurt_server`); what it meets of itself in its tick waits in
    /// `hurts` for its tick to take, in the order met.
    Mob {
        kind: u16,
        hurts: Vec<(Damage, f32)>,
    },
}

/// How a body's tick ended it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    /// It lives on.
    Lives,
    /// It broke as by anything (`ArmorStand.brokenByAnything`): its kind drops what it
    /// wears and plays its breaking sound.
    Broken,
    /// It was killed and drops nothing (`kill`, the void's).
    Killed,
}

/// How a body travels where its type has a way of its own (`travel` or `travelInWater`
/// overridden), chosen by its type for each tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Travel {
    /// `LivingEntity.travel`.
    Living,
    /// A `travelInWater` of its own in water (a fish's, a dolphin's, a turtle's):
    /// `moveRelative(speed, input)`, its move, its motion times 0.9, then a sink of 0.005
    /// as `sinks` says, after the move; in the air and in lava the living body's.
    Swims { speed: f32, sinks: Sinks },
    /// `travel` that is `move(SELF, motion)` and nothing more (a squid's).
    Bare,
    /// `travelFlying(input, water, lava, air)` (an allay's, a happy ghast's): no gravity,
    /// `moveRelative` at the speed of where it is, its move, and its motion slowed to
    /// 0.8 in water, a half in lava and 0.91 in the air.
    Flying { water: f32, lava: f32, air: f32 },
}

/// Whether a swimmer's own `travelInWater` sinks it after its move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Sinks {
    Never,
    Always,
    /// Unless its feet are within `distance` of the middle of the block `centre` (a
    /// turtle near the home it goes to).
    UnlessNear {
        centre: BlockPos,
        distance: f64,
    },
}

/// A living body in its tick: the fields the game keeps on a `LivingEntity` that its
/// tick reads and writes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Living {
    pub(crate) body: Body,
    pub(crate) kind: Kind,
    pub(crate) attributes: Attributes,
    pub(crate) eye_height: f32,
    /// Whether it travels at all (`isEffectiveAi`, false for a marker).
    pub(crate) travels: bool,
    /// `noPhysics`: it moves through everything.
    pub(crate) no_physics: bool,
    pub(crate) fire_ticks: i32,
    /// `DATA_AIR_SUPPLY_ID`.
    pub(crate) air: i32,
    /// `DATA_TICKS_FROZEN`.
    pub(crate) frozen: i32,
    /// `canFreeze`: it wears nothing that keeps the cold off.
    pub(crate) can_freeze: bool,
    /// `PowderSnowBlock.canEntityWalkOnPowderSnow`: it wears leather boots.
    pub(crate) walks_on_powder_snow: bool,
    /// `firstTick`: lava does not count before the end of its first `baseTick`.
    pub(crate) first_tick: bool,
    pub(crate) in_powder_snow: bool,
    pub(crate) was_in_powder_snow: bool,
    /// What the fluids were at the last update.
    pub(crate) fluids: InFluids,
    /// `finalMovementsThisTick`: what its last effects of blocks walked along.
    pub(crate) last_movements: Vec<Movement>,
    pub(crate) end: End,
    /// The shared flag of being on fire, as `baseTick` set it.
    pub(crate) on_fire_flag: bool,
    pub(crate) sounds: Vec<Sound>,
    /// Redstone ore it stood on, to light, and sculk sensors, to hear it.
    pub(crate) ore: Vec<BlockPos>,
    /// Where its fall ended in this tick, if one did, for the landing's vibration.
    pub(crate) landed_on: Option<BlockPos>,
    /// The plates, tripwire and detector rails its walk met, in the order met, whose
    /// `entityInside` the region runs itself ([`crate::physics::item::handed_back`]).
    pub(crate) inside: Vec<BlockPos>,
    /// What steers it (`travelInput`: `xxa`, `yya`, `zza` widened), zero for a body
    /// without a mind, and its `getSpeed()`.
    pub(crate) input: physics::Vec3,
    pub(crate) speed: f32,
    /// `yRot`, which its input is turned by.
    pub(crate) yaw: f32,
    /// `maxUpStep()`: the `step_height` it steps up as it moves; a stand's is 0.
    pub(crate) step_height: f32,
    /// Of `#fall_damage_immune`: a fall costs it nothing and sounds of nothing.
    pub(crate) fall_damage_immune: bool,
    /// Whose `causeFallDamage` a fall it ends takes.
    pub(crate) landing: Landing,
    /// `canBreatheUnderwater`: under water it keeps its air.
    pub(crate) breathes_under_water: bool,
    /// Its air ran out under water in this tick (entity event 67's bubbles).
    pub(crate) drowned: bool,
    /// How it travels in this tick, its type's choice.
    pub(crate) travel_kind: Travel,
    /// `isPushedByFluid`: false for the water creatures and the turtle.
    pub(crate) pushed_by_fluid: bool,
    /// `getMaxAirSupply`: a dolphin's is 4800.
    pub(crate) max_air: i32,
    /// `increaseAirSupply` gives it all its air at once (a dolphin's), not four a tick.
    pub(crate) refills_at_once: bool,
    /// `omnidirectionalAirMover()`: the air slows its fall as it slows it sideways.
    pub(crate) omnidirectional_air: bool,
    /// `getWaterSlowDown()`, 0.8 but for the types that swim better.
    pub(crate) water_slow_down: f32,
}

impl Living {
    /// A body at rest of `kind` in `body`, as a fresh one is: full air, nothing frozen,
    /// not burning, in its first tick.
    pub(crate) fn new(body: Body, kind: Kind, eye_height: f32) -> Self {
        Self {
            body,
            kind,
            attributes: Attributes::LIVING,
            eye_height,
            travels: true,
            no_physics: false,
            fire_ticks: 0,
            air: MAX_AIR,
            frozen: 0,
            can_freeze: true,
            walks_on_powder_snow: false,
            first_tick: true,
            in_powder_snow: false,
            was_in_powder_snow: false,
            fluids: InFluids::default(),
            last_movements: Vec::new(),
            end: End::Lives,
            on_fire_flag: false,
            sounds: Vec::new(),
            ore: Vec::new(),
            landed_on: None,
            inside: Vec::new(),
            input: [0.0; 3],
            speed: 0.0,
            yaw: 0.0,
            step_height: 0.0,
            fall_damage_immune: false,
            landing: Landing::Living,
            breathes_under_water: false,
            drowned: false,
            travel_kind: Travel::Living,
            pushed_by_fluid: true,
            max_air: MAX_AIR,
            refills_at_once: false,
            omnidirectional_air: false,
            water_slow_down: 0.8,
        }
    }

    /// Whether it is a mob's body, whose hurts its mind's body takes.
    fn is_mob(&self) -> bool {
        matches!(self.kind, Kind::Mob { .. })
    }

    /// Whether it lives (`isAlive`).
    pub(crate) fn alive(&self) -> bool {
        self.end == End::Lives
    }

    /// `isOnFire` (nothing living here is immune to fire).
    pub(crate) fn is_on_fire(&self) -> bool {
        self.fire_ticks > 0
    }

    /// `isInLava`.
    pub(crate) fn in_lava(&self) -> bool {
        !self.first_tick && self.fluids.in_lava()
    }

    /// `getAirDrag`: `0.98f` changed by the air drag modifier.
    pub(crate) fn air_drag(&self) -> f32 {
        let drag = if self.omnidirectional_air { 0.91 } else { 0.98 };
        modified_friction(drag, self.attributes.air_drag_modifier as f32)
    }

    fn play(&mut self, sound: i32, volume: f32, pitch: f32) {
        self.sounds.push(Sound {
            sound,
            position: self.body.position,
            volume,
            pitch,
        });
    }

    /// `igniteForTicks`.
    fn ignite_for_ticks(&mut self, ticks: i32) {
        if self.fire_ticks < ticks {
            self.fire_ticks = ticks;
        }
    }

    /// `clearFire`.
    fn clear_fire(&mut self) {
        self.fire_ticks = self.fire_ticks.min(0);
    }

    /// `hurtServer(source, amount)` of the kind, for what the body meets of itself:
    /// whether it took the hit.
    pub(crate) fn hurt(&mut self, damage: Damage, amount: f32) -> bool {
        let _ = amount;
        if !self.alive() {
            return false;
        }
        match &mut self.kind {
            Kind::Mob { hurts, .. } => {
                // The mob's body takes it in its tick (`mobs::tick`), after the step that
                // met it and before anything else reads its health: noted here, in
                // order. Taken as given here; where the game reads the answer, the
                // mob's body does what the answer decides when it takes it (lava's
                // burn sound plays only for a hurt that counted:
                // `mobs::body::take_hurt`).
                hurts.push((damage, amount));
                true
            }
            &mut Kind::ArmorStand {
                health,
                invulnerable,
            } => {
                // `ArmorStand.hurtServer`: the void kills it, dropping nothing; an
                // invisible stand or a marker takes nothing else; fire lights it and
                // then burns it, a little at a time; nothing else of these is in its
                // tags of what breaks it (`#can_break_armor_stand`).
                if damage == Damage::OutOfWorld {
                    self.end = End::Killed;
                    return false;
                }
                if invulnerable {
                    return false;
                }
                match damage {
                    Damage::InFire | Damage::Campfire => {
                        if self.is_on_fire() {
                            self.cause_damage(health, 0.15);
                        } else {
                            self.ignite_for_ticks(100);
                        }
                    }
                    Damage::OnFire if health > 0.5 => self.cause_damage(health, 4.0),
                    _ => {}
                }
                false
            }
        }
    }

    /// `ArmorStand.causeDamage(amount)`: health down to half a heart breaks it.
    fn cause_damage(&mut self, health: f32, amount: f32) {
        let left = health - amount;
        if left <= 0.5 {
            self.end = End::Broken;
        } else {
            if let Kind::ArmorStand { health, .. } = &mut self.kind {
                *health = left;
            }
        }
    }

    /// `getFallDamageSound(damage)` of the kind.
    fn fall_sound(&self, damage: i32) -> i32 {
        match self.kind {
            Kind::ArmorStand { .. } => sounds::ENTITY_ARMOR_STAND_FALL,
            // `LivingEntity.getFallSounds`, which no mob of N7 changes.
            Kind::Mob { .. } if damage > 4 => sounds::ENTITY_GENERIC_BIG_FALL,
            Kind::Mob { .. } => sounds::ENTITY_GENERIC_SMALL_FALL,
        }
    }

    /// The tick of the body: `LivingEntity.tick()` without what its kind does around
    /// it (what it wears, its pushing).
    pub(crate) fn tick<R: Holds + ?Sized>(&mut self, world: &R, draws: &mut dyn Draws) {
        self.base_tick(world);
        if !self.alive() {
            return;
        }
        self.ai_step(world, draws);
    }

    /// `updateFluidInteraction`: how deep it is in water and lava, the fall ended in
    /// water, and the currents' push (a living body is pushed as anything but a player
    /// is).
    fn update_fluids<W: World + ?Sized>(&mut self, world: &W) {
        self.fluids = refresh_fluids(&mut self.body, world, self.pushed_by_fluid);
    }

    /// `LivingEntity.baseTick()`.
    pub(crate) fn base_tick<R: Holds + ?Sized>(&mut self, world: &R) {
        // `Entity.baseTick`.
        self.was_in_powder_snow = self.in_powder_snow;
        self.in_powder_snow = false;
        self.update_fluids(world);
        if self.fire_ticks > 0 {
            if self.fire_ticks % 20 == 0 && !self.in_lava() {
                self.hurt(Damage::OnFire, 1.0);
            }
            self.fire_ticks -= 1;
        }
        if self.in_lava() {
            self.body.fall_distance *= 0.5;
        }
        // `checkBelowWorld`, and `LivingEntity.onBelowWorld`'s hit of the void.
        if self.body.position[1] < f64::from(world.min_y() - 64) {
            self.hurt(Damage::OutOfWorld, 4.0);
        }
        self.on_fire_flag = self.fire_ticks > 0;
        self.first_tick = false;
        if !self.alive() {
            return;
        }
        // What is in a wall takes `inWall`, and what drowns `drown`: neither breaks a
        // stand, which is not asked, but both hurt a mob. The air goes down a tick at a
        // time with the eyes in water out of a bubble column, and back up four a tick
        // elsewhere.
        if self.is_mob() && self.is_in_wall(world) {
            self.hurt(Damage::InWall, 1.0);
        }
        let eyes = [
            self.body.position[0],
            self.body.position[1] + f64::from(self.eye_height),
            self.body.position[2],
        ];
        let at_eyes = world.state(physics::containing(eyes));
        if self.eye_in_water(world) && !is_class(at_eyes, BlockClass::BubbleColumnBlock) {
            if !self.breathes_under_water {
                // `decreaseAirSupply`, with no oxygen bonus, which nothing in Clustine
                // gives and which alone would draw.
                self.air -= 1;
                if self.air <= -20 {
                    // `shouldTakeDrowningDamage`: the bubbles (entity event 67) and a
                    // drowning hit, which only a mob takes.
                    self.air = 0;
                    if self.is_mob() {
                        self.drowned = true;
                        self.hurt(Damage::Drown, 2.0);
                    }
                }
            }
            // A body that breathes under water keeps its air there: it is refilled
            // only by water breathing or a conduit's power, which no body has.
        } else if self.air < self.max_air {
            self.air = if self.refills_at_once {
                self.max_air
            } else {
                (self.air + 4).min(self.max_air)
            };
        }
    }

    /// `isInWall()`: not moving through everything, and a block that suffocates in a
    /// thin box at its eyes, of four fifths of its width each way. **Inferred**, as
    /// `region/cushions.rs` has it: a block suffocates where its collision shape is the
    /// full block; the blocks whose own predicates the dump does not have are not told
    /// apart.
    fn is_in_wall<W: World + ?Sized>(&self, world: &W) -> bool {
        if self.no_physics {
            return false;
        }
        let half = f64::from(self.body.width * 0.8) / 2.0;
        let eye = self.body.position[1] + f64::from(self.eye_height);
        let thin = 1.0e-6 / 2.0;
        let low = [
            physics::floor(self.body.position[0] - half),
            physics::floor(eye - thin),
            physics::floor(self.body.position[2] - half),
        ];
        let high = [
            physics::floor(self.body.position[0] + half),
            physics::floor(eye + thin),
            physics::floor(self.body.position[2] + half),
        ];
        (low[0]..=high[0]).any(|x| {
            (low[1]..=high[1]).any(|y| {
                (low[2]..=high[2]).any(|z| {
                    let state = world.state(BlockPos::new(x, y, z));
                    !state.props().is_air() && state.props().collision_is_full_block()
                })
            })
        })
    }

    /// `isEyeInFluid(#water)`: water in the column of its block reaches its eyes.
    /// **Inferred**, as `Region::eye_in_water` has it: the game keeps this from its last
    /// fluid update (`EntityFluidInteraction`, not in the dump) with the fluid's camera
    /// height, taken here as the fluid's height.
    pub(crate) fn eye_in_water<W: World + ?Sized>(&self, world: &W) -> bool {
        let bounds = self.body.bounds().deflate(0.001);
        let eye = self.body.position[1] + f64::from(self.eye_height);
        let block = self.body.block_position();
        let y0 = physics::floor(bounds.min[1]);
        let y1 = bounds.max[1].ceil() as i32 - 1;
        (y0..=y1).any(|y| {
            let pos = BlockPos::new(block.x, y, block.z);
            let state = world.state(pos);
            if !clustine_blocks::data::has_water(state) {
                return false;
            }
            let fluid = clustine_blocks::fluid::FluidState::of(state);
            let height = clustine_blocks::fluid::height(&mut physics::Rules(world), fluid, pos);
            eye >= f64::from(y) && eye <= f64::from(y) + f64::from(height)
        })
    }

    /// `LivingEntity.aiStep()` of a body without a mind.
    fn ai_step<R: Holds + ?Sized>(&mut self, world: &R, draws: &mut dyn Draws) {
        self.zero_small_motion();
        if self.travels {
            self.travel(world);
        }
        self.after_travel(world, draws);
    }

    /// The start of `aiStep`: small motion is zeroed, each axis apart, as for every type
    /// but the player. A mob's AI runs after it (`crate::mobs::tick`).
    pub(crate) fn zero_small_motion(&mut self) {
        let [mut x, mut y, mut z] = self.body.motion;
        if x.abs() < 0.003 {
            x = 0.0;
        }
        if z.abs() < 0.003 {
            z = 0.0;
        }
        if y.abs() < 0.003 {
            y = 0.0;
        }
        self.body.motion = [x, y, z];
    }

    /// What `aiStep` does after `travel`: the effects of the blocks walked through, and
    /// powder snow's freezing.
    pub(crate) fn after_travel<R: Holds + ?Sized>(&mut self, world: &R, draws: &mut dyn Draws) {
        self.apply_effects_from_blocks(world, draws);
        // Powder snow freezes it a tick at a time and thaws it two; a fully frozen
        // body's `freeze` hit every 40 ticks breaks no stand, and is not asked.
        if self.in_powder_snow && self.can_freeze {
            self.frozen = (self.frozen + 1).min(TICKS_TO_FREEZE);
        } else {
            self.frozen = (self.frozen - 2).max(0);
        }
    }

    /// It ended a fall of `distance` on the block at `on` (`Block.fallOn`, then
    /// `LivingEntity.causeFallDamage`): with damage above 0 the kind's fall sound and
    /// the sound of the block under it, and no damage, which a stand ignores.
    /// **Inferred**: the blocks' own `fallOn` are not in the dump; the multipliers are
    /// the game's as the reference servers have them (a hay bale and honey a fifth, a
    /// bed half the fall, slime and powder snow none); any other block is the default.
    pub(crate) fn landed<W: World + ?Sized>(&mut self, world: &W, distance: f64, on: BlockPos) {
        let state = world.state(on);
        // A slime block's `fallOn` asks for the fall with a multiplier of nothing, which
        // hurts nobody but still sounds a horse's landing.
        let (distance, multiplier) =
            if is_class(state, BlockClass::HayBlock) || is_class(state, BlockClass::HoneyBlock) {
                (distance, 0.2_f32)
            } else if is_class(state, BlockClass::BedBlock) {
                (distance * 0.5, 1.0)
            } else if is_class(state, BlockClass::SlimeBlock) {
                (distance, 0.0)
            } else if is_class(state, BlockClass::PowderSnowBlock) {
                return;
            } else {
                (distance, 1.0)
            };
        // `AbstractHorse.causeFallDamage`: above a block, its landing sound first,
        // whatever it costs.
        if let Landing::Horse { baby } = self.landing
            && distance > 1.0
        {
            let sound = if baby {
                sounds::ENTITY_BABY_HORSE_LAND
            } else {
                sounds::ENTITY_HORSE_LAND
            };
            self.play(sound, 0.4, 1.0);
        }
        // `calculateFallDamage`: the fall beyond the safe distance, floored; nothing for
        // a type of `#fall_damage_immune`.
        if self.fall_damage_immune {
            return;
        }
        let power = distance + 1.0e-6 - self.attributes.safe_fall_distance;
        let mut damage =
            physics::floor(power * f64::from(multiplier) * self.attributes.fall_damage_multiplier);
        if self.landing == Landing::Frog {
            damage -= 5;
        }
        if damage <= 0 {
            return;
        }
        // A horse and a llama sound no fall of their own, only the block's; a llama is
        // hurt only by a fall of six or more (`Llama.causeFallDamage`).
        let hurt = match self.landing {
            Landing::Living | Landing::Frog => {
                let sound = self.fall_sound(damage);
                self.play(sound, 1.0, 1.0);
                true
            }
            Landing::Horse { .. } => true,
            Landing::Llama => distance >= 6.0,
        };
        // `playBlockFallSound`: the block a fifth below its feet.
        let below = BlockPos::new(
            physics::floor(self.body.position[0]),
            physics::floor(self.body.position[1] - 0.200_000_002_980_232_24),
            physics::floor(self.body.position[2]),
        );
        let block = world.state(below);
        if !block.props().is_air()
            && let Some(sound_type) = block.sound_type()
        {
            self.play(
                sound_type.fall,
                sound_type.volume * 0.5,
                sound_type.pitch * 0.75,
            );
        }
        // `hurt(fall, damage)`, which a stand ignores. The game's horse is hurt before its
        // block's sound, which comes out the same: both are this tick's.
        if self.is_mob() && hurt {
            self.hurt(Damage::Fall, damage as f32);
        }
    }
}

/// Whose `causeFallDamage` a body's landing takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landing {
    /// `LivingEntity`'s: the kind's fall sound, the block's, the hurt.
    Living,
    /// `AbstractHorse`'s: its landing sound above a block (a foal's own), the hurt and
    /// the block's sound, no fall sound of the kind.
    Horse { baby: bool },
    /// `Llama`'s: no landing sound; hurt only by a fall of six or more, the block's
    /// sound at any fall that would hurt.
    Llama,
    /// `Frog`'s: `LivingEntity`'s, its damage five less (`calculateFallDamage`).
    Frog,
}

/// `computeModifiedFriction(friction, modifier)`, in floats.
pub(crate) fn modified_friction(friction: f32, modifier: f32) -> f32 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

/// `updateFluidInteraction` of `body`: what it found, with the body's being in water,
/// its fall ended in water, and the currents' push where it is `pushed` by fluids.
fn refresh_fluids<W: World + ?Sized>(body: &mut Body, world: &W, pushed: bool) -> InFluids {
    let found = fluids::update(world, &body.bounds());
    body.in_water = found.in_water();
    if found.in_water() {
        body.fall_distance = 0.0;
    }
    if pushed {
        body.motion = fluids::push(&found, body.motion);
    }
    found
}

#[cfg(test)]
mod tests;
