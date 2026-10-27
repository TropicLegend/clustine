//! The item's tick (`ItemEntity.tick`, with `Entity.baseTick` and
//! `applyEffectsFromBlocks` as items meet them, `docs/groundwork/n4-brief.md`, sections
//! 1.9, 3 and 4), as far as it reads only the item and the blocks: the region does its
//! merging between the two halves, and holds it back only from a loaded chunk of its
//! own that is full ([`Region::may_hold`]); a move into a chunk it has not loaded the
//! region sends on (`region/arrivals.rs`).

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, BlockState, EntityInsideShape, Face, sounds};
use clustine_world::{BlockPos, ChunkPos, ItemStack};

use super::aabb::Aabb;
use super::body::{AIR_DRAG, Body, Movement, physics_of};
use super::collide;
use super::fluids::{self, InFluids};
use super::voxel::{Cells, Placed};
use super::walk;
use super::{Vec3, World};

/// The pick-up delay the game reads as never, which does not count down.
pub(crate) const NEVER_PICKED_UP: u16 = 32_767;

/// The age at which an item despawns (`ItemEntity.LIFETIME`).
pub(crate) const LIFETIME: u32 = 6000;

/// The game's `SoundSource.NEUTRAL`, which an entity's own sounds are of.
pub(crate) const SOUND_SOURCE_NEUTRAL: i32 = 6;

/// What hurts an item in N4 (the game's damage types).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Source {
    /// `inFire`: standing in fire or soul fire.
    InFire,
    /// `onFire`: burning.
    OnFire,
    Lava,
    Cactus,
}

impl Source {
    /// Whether the type is in `#minecraft:is_fire`.
    fn is_fire(self) -> bool {
        matches!(self, Self::InFire | Self::OnFire | Self::Lava)
    }
}

/// The effects of blocks an entity is in, in the game's order of applying them
/// (`InsideBlockEffectType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Freeze = 0,
    ClearFreeze = 1,
    FireIgnite = 2,
    LavaIgnite = 3,
    Extinguish = 4,
}

const APPLY_ORDER: [Effect; 5] = [
    Effect::Freeze,
    Effect::ClearFreeze,
    Effect::FireIgnite,
    Effect::LavaIgnite,
    Effect::Extinguish,
];

/// What a block queues to run around one of the effects.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Effect(Effect),
    Hurt(Source, f32),
    LavaHurt,
}

/// `InsideBlockEffectApplier.StepBasedCollector`: each effect once in a step however
/// many blocks of the step applied it, with what the blocks queued before and after it.
#[derive(Debug, Default)]
struct Collector {
    in_step: [bool; 5],
    before: [Vec<Action>; 5],
    after: [Vec<Action>; 5],
    queued: Vec<Action>,
    last_step: Option<i32>,
}

impl Collector {
    fn advance_step(&mut self, step: i32) {
        if self.last_step != Some(step) {
            self.last_step = Some(step);
            self.flush_step();
        }
    }

    fn flush_step(&mut self) {
        for effect in APPLY_ORDER {
            let index = effect as usize;
            self.queued.append(&mut self.before[index]);
            if std::mem::take(&mut self.in_step[index]) {
                self.queued.push(Action::Effect(effect));
            }
            self.queued.append(&mut self.after[index]);
        }
    }

    fn apply(&mut self, effect: Effect) {
        self.in_step[effect as usize] = true;
    }

    fn run_after(&mut self, effect: Effect, action: Action) {
        self.after[effect as usize].push(action);
    }
}

/// A sound the item played (`Entity.playSound`), at its position then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sound {
    pub(crate) sound: i32,
    pub(crate) position: Vec3,
    pub(crate) volume: f32,
    pub(crate) pitch: f32,
}

/// An item in its tick.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Item {
    pub(crate) body: Body,
    pub(crate) stack: ItemStack,
    pub(crate) health: i16,
    pub(crate) pick_up_delay: u16,
    pub(crate) age: u32,
    pub(crate) fire_ticks: i32,
    /// `fireImmune`: the stack resists fire (netherite).
    pub(crate) fire_immune: bool,
    /// `firstTick`: the item's first tick, in which lava does not count yet.
    pub(crate) first_tick: bool,
    /// `tickCount`, already counted for this tick.
    pub(crate) tick_count: i32,
    /// `getId()`.
    pub(crate) game_id: i32,
    /// `finalMovementsThisTick`: what its last effects of blocks walked along.
    pub(crate) last_movements: Vec<Movement>,
    /// `noPhysics`: it is stuck in a block and being pushed out.
    pub(crate) no_physics: bool,
    pub(crate) removed: bool,
    /// `needsSync` and `syncVelocity`, for the edge.
    pub(crate) needs_sync: bool,
    pub(crate) sync_velocity: bool,
    /// The shared flag of being on fire, as `baseTick` set it.
    pub(crate) on_fire_flag: bool,
    /// What the fluids were at the first update of the tick.
    pub(crate) fluids: InFluids,
    /// Its motion at the start of its own part of the tick.
    pub(crate) start_motion: Vec3,
    /// Redstone ore it stood on, to light, and sculk sensors, to hear it.
    pub(crate) ore: Vec<BlockPos>,
    /// The blocks its walk met whose `entityInside` the region runs itself, because it
    /// reads the region's entities or containers: plates, tripwire, detector rails and
    /// hoppers, in the order they were met (`docs/groundwork/n6-plan.md`, decision 11).
    /// A sculk sensor has no `entityInside`: it hears what stands on it
    /// ([`Item::ore`]).
    pub(crate) inside: Vec<BlockPos>,
    pub(crate) sounds: Vec<Sound>,
}

/// Whether the region runs the `entityInside` of `state` for an entity itself
/// ([`Item::inside`], and the walks of the other entities, which hand back the same).
pub(crate) fn handed_back(state: BlockState) -> bool {
    is_class(state, BlockClass::BasePressurePlateBlock)
        || is_class(state, BlockClass::TripWireBlock)
        || is_class(state, BlockClass::DetectorRailBlock)
        || is_class(state, BlockClass::HopperBlock)
}

/// What draws the item's random numbers (`Entity.random`).
pub(crate) trait Draws {
    fn next_float(&mut self) -> f32;
}

/// What the item's tick asks of the region beyond its blocks.
pub(crate) trait Region: World {
    /// Whether a move may end in the chunk: anywhere but a loaded chunk of the region's
    /// own, other than the entity's, that is full.
    fn may_hold(&self, chunk: ChunkPos) -> bool;

    /// The bottom of the world.
    fn min_y(&self) -> i32;
}

impl Item {
    /// Whether it burns (`isOnFire`).
    pub(crate) fn is_on_fire(&self) -> bool {
        !self.fire_immune && self.fire_ticks > 0
    }

    /// `ItemEntity.hurtServer`: what the stack resists does nothing, and anything else
    /// takes from its health, which at 0 destroys it.
    pub(crate) fn hurt(&mut self, source: Source, amount: f32) -> bool {
        if self.removed || (source.is_fire() && self.fire_immune) {
            return false;
        }
        // `markHurt`.
        self.sync_velocity = true;
        self.health = (f32::from(self.health) - amount) as i32 as i16;
        if self.health <= 0 {
            self.removed = true;
        }
        true
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

    fn play(&mut self, sound: i32, volume: f32, pitch: f32) {
        self.sounds.push(Sound {
            sound,
            position: self.body.position,
            volume,
            pitch,
        });
    }

    /// `updateFluidInteraction`: how deep it is in water and lava, the fall ended in
    /// water, and the currents' push; whether any fluid reaches it.
    fn update_fluids<W: World + ?Sized>(&mut self, world: &W) -> InFluids {
        let found = fluids::update(world, &self.body.bounds());
        self.body.in_water = found.in_water();
        if found.in_water() {
            self.body.fall_distance = 0.0;
        }
        self.body.motion = fluids::push(&found, self.body.motion);
        found
    }

    /// `isInLava`.
    fn in_lava(&self) -> bool {
        !self.first_tick && self.fluids.in_lava()
    }

    /// The first part of `ItemEntity.tick`, up to its merging: false where the item is
    /// gone by then.
    pub(crate) fn tick_before_merge<R: Region + ?Sized>(
        &mut self,
        world: &R,
        draws: &mut dyn Draws,
    ) -> bool {
        if self.stack.count <= 0 {
            self.removed = true;
            return false;
        }
        self.base_tick(world);
        if self.removed {
            return false;
        }
        if self.pick_up_delay > 0 && self.pick_up_delay != NEVER_PICKED_UP {
            self.pick_up_delay -= 1;
        }
        self.body.old_position = self.body.position;
        self.start_motion = self.body.motion;
        let threshold = f64::from(0.1_f32);
        if self.body.in_water && self.fluids.water_height > threshold {
            self.fluid_movement(f64::from(0.99_f32));
        } else if self.in_lava() && self.fluids.lava_height > threshold {
            self.fluid_movement(f64::from(0.95_f32));
        } else {
            self.body.apply_gravity();
        }
        let context = self.body.context();
        self.no_physics = !collide::no_collision(
            world,
            context,
            &self.body.mover,
            &self.body.bounds().deflate(1.0e-7),
        );
        if self.no_physics {
            let bounds = self.body.bounds();
            let centre = [
                self.body.position[0],
                (bounds.min[1] + bounds.max[1]) / 2.0,
                self.body.position[2],
            ];
            self.move_towards_closest_space(world, centre, draws);
        }
        let slow = super::horizontal_sqr(self.body.motion) <= f64::from(1.0e-5_f32);
        if !self.body.on_ground || !slow || self.tick_count.wrapping_add(self.game_id) % 4 == 0 {
            let motion = self.body.motion;
            self.move_guarded(world, motion);
            self.apply_effects_from_blocks(world, draws, None);
            let drag = AIR_DRAG;
            let mut friction = drag;
            if self.body.on_ground {
                let under = world.state(self.body.below_for_movement(world));
                friction = drag * physics_of(under).map_or(0.6, |physics| physics.friction);
            }
            let [x, y, z] = self.body.motion;
            self.body.motion = [
                x * f64::from(friction),
                y * f64::from(drag),
                z * f64::from(friction),
            ];
            if self.body.on_ground && self.body.motion[1] < 0.0 {
                let [x, y, z] = self.body.motion;
                self.body.motion = [x * 1.0, y * -0.5, z * 1.0];
            }
        } else {
            let last = self.last_movements.clone();
            self.apply_effects_from_blocks(world, draws, Some(last));
        }
        !self.removed
    }

    /// Whether the tick's merging runs: every second tick after a move to another
    /// block, every fortieth otherwise.
    pub(crate) fn merges_now(&self) -> bool {
        let moved = (0..3).any(|axis| {
            super::floor(self.body.old_position[axis]) != super::floor(self.body.position[axis])
        });
        let every = if moved { 2 } else { 40 };
        self.tick_count % every == 0 && self.is_mergable()
    }

    /// `isMergable`, of the item itself; the stack's room is the caller's, which knows
    /// its maximum.
    pub(crate) fn is_mergable(&self) -> bool {
        !self.removed
            && self.pick_up_delay != NEVER_PICKED_UP
            && self.age < LIFETIME
            && self.stack.count < max_stack_size(&self.stack)
    }

    /// The rest of `ItemEntity.tick`, after its merging.
    pub(crate) fn tick_after_merge<W: World + ?Sized>(&mut self, world: &W) {
        if self.removed {
            return;
        }
        self.age = self.age.saturating_add(1);
        let found = self.update_fluids(world);
        self.needs_sync |= found.found;
        if super::length_sqr(super::sub(self.body.motion, self.start_motion)) > 0.01 {
            self.needs_sync = true;
        }
        if self.age >= LIFETIME {
            self.removed = true;
        }
    }

    /// `Entity.baseTick` as an item meets it.
    fn base_tick<R: Region + ?Sized>(&mut self, world: &R) {
        self.fluids = self.update_fluids(world);
        if self.fire_ticks > 0 {
            if self.fire_immune {
                self.clear_fire();
            } else {
                if self.fire_ticks % 20 == 0 && !self.in_lava() {
                    self.hurt(Source::OnFire, 1.0);
                }
                self.fire_ticks -= 1;
            }
        }
        if self.in_lava() {
            self.body.fall_distance *= 0.5;
        }
        // `checkBelowWorld`.
        if self.body.position[1] < f64::from(world.min_y() - 64) {
            self.removed = true;
        }
        self.on_fire_flag = self.fire_ticks > 0;
        self.first_tick = false;
    }

    /// `setFluidMovement`: slowed sideways, and rising slowly until near the surface.
    fn fluid_movement(&mut self, factor: f64) {
        let [x, y, z] = self.body.motion;
        let rise = if y < f64::from(0.06_f32) {
            f64::from(5.0e-4_f32)
        } else {
            0.0
        };
        self.body.motion = [x * factor, y + rise, z * factor];
    }

    /// `moveTowardsClosestSpace(x, y, z)`: towards the nearest side of the block the
    /// point is in whose neighbour is not a full block, a little faster than it moved.
    fn move_towards_closest_space<W: World + ?Sized>(
        &mut self,
        world: &W,
        point: Vec3,
        draws: &mut dyn Draws,
    ) {
        let pos = super::containing(point);
        let fraction = [
            point[0] - f64::from(pos.x),
            point[1] - f64::from(pos.y),
            point[2] - f64::from(pos.z),
        ];
        // North, south, west, east and up, each as (axis, step).
        let directions = [(2, -1), (2, 1), (0, -1), (0, 1), (1, 1)];
        let (mut best, mut nearest) = ((1_usize, 1_i32), f64::MAX);
        for (axis, step) in directions {
            let mut beside = [pos.x, pos.y, pos.z];
            beside[axis] += step;
            let state = world.state(BlockPos::new(beside[0], beside[1], beside[2]));
            if !state.props().collision_is_full_block() {
                let along = fraction[axis];
                let distance = if step > 0 { 1.0 - along } else { along };
                if distance < nearest {
                    best = (axis, step);
                    nearest = distance;
                }
            }
        }
        let speed = draws.next_float() * 0.2 + 0.1;
        let step = best.1 as f32;
        let mut motion = super::scale(self.body.motion, 0.75);
        motion[best.0] = f64::from(step * speed);
        self.body.motion = motion;
    }

    /// `move(SELF, vector)`, and the region's guard after it ([`move_guarded`]).
    fn move_guarded<R: Region + ?Sized>(&mut self, world: &R, vector: Vec3) {
        move_guarded(&mut self.body, world, self.no_physics, vector);
    }

    /// `PistonMovingBlockEntity.moveEntityByPiston` of the item: `move(PISTON, vector)`
    /// with the region's guard, given how far pistons have moved it in this tick and
    /// the way the piston pushes (`NOCLIP`), then the effects of the blocks on its way
    /// (`applyEffectsFromBlocks(from, to)`); the move is not kept among the tick's
    /// (`removeLatestMovementRecording`).
    pub(crate) fn pushed_by_piston<R: Region + ?Sized>(
        &mut self,
        world: &R,
        draws: &mut dyn Draws,
        noclip: Face,
        vector: Vec3,
        deltas: &mut [f64; 3],
    ) {
        self.body.noclip = Some(noclip);
        let from = self.body.position;
        move_guarded_as(&mut self.body, world, self.no_physics, vector, Some(deltas));
        let movement = Movement {
            from,
            to: self.body.position,
            asked: None,
        };
        self.apply_effects_from_blocks(world, draws, Some(vec![movement]));
        self.body.movements.clear();
        self.body.noclip = None;
    }

    /// `applyEffectsFromBlocks`, of this tick's movements or, given `last`, of the last
    /// tick's (`applyEffectsFromBlocksForLastMovements`), or of the one movement a
    /// piston made.
    fn apply_effects_from_blocks<W: World + ?Sized>(
        &mut self,
        world: &W,
        draws: &mut dyn Draws,
        last: Option<Vec<Movement>>,
    ) {
        let movements = match last {
            Some(last) if !last.is_empty() => last,
            Some(_) => vec![Movement {
                from: self.body.position,
                to: self.body.position,
                asked: None,
            }],
            None => {
                let movements = tick_movements(&mut self.body);
                self.last_movements = movements.clone();
                movements
            }
        };
        if self.removed || self.no_physics {
            return;
        }
        if self.body.on_ground {
            let pos = self.body.on_pos_legacy(world);
            let state = world.state(pos);
            self.step_on(pos, state);
        }
        let was_on_fire = self.is_on_fire();
        let fire_before = self.fire_ticks;
        let mut collector = Collector::default();
        check_inside_blocks(
            world,
            &mut ItemInside {
                item: self,
                collector: &mut collector,
            },
            &movements,
        );
        // `applyAndClear`.
        collector.flush_step();
        for action in std::mem::take(&mut collector.queued) {
            if self.removed {
                break;
            }
            self.run(action, draws);
        }
        if was_on_fire && !self.is_on_fire() {
            // `playEntityOnFireExtinguishedSound`.
            let pitch = 1.6 + (draws.next_float() - draws.next_float()) * 0.4;
            self.play(sounds::ENTITY_GENERIC_EXTINGUISH_FIRE, 0.7, pitch);
        }
        let ignited = self.fire_ticks > fire_before;
        if !self.is_on_fire() && !ignited {
            self.fire_ticks = 0;
        }
    }

    /// `Block.stepOn` of what the item stands on.
    fn step_on(&mut self, pos: BlockPos, state: BlockState) {
        if is_class(state, BlockClass::SlimeBlock) {
            let up = self.body.motion[1].abs();
            if up < 0.1 {
                let factor = 0.4 + up * 0.2;
                let [x, y, z] = self.body.motion;
                self.body.motion = [x * factor, y * 1.0, z * factor];
            }
        } else if is_class(state, BlockClass::RedStoneOreBlock)
            || is_class(state, BlockClass::SculkSensorBlock)
        {
            self.ore.push(pos);
        }
    }

    /// One queued action of the effects of blocks.
    fn run(&mut self, action: Action, draws: &mut dyn Draws) {
        match action {
            Action::Effect(Effect::Freeze | Effect::ClearFreeze) => {}
            Action::Effect(Effect::FireIgnite) => {
                // `BaseFireBlock.fireIgnite`.
                if !self.fire_immune {
                    if self.fire_ticks < 0 {
                        self.fire_ticks += 1;
                    }
                    if self.fire_ticks >= 0 {
                        self.ignite_for_ticks(160);
                    }
                }
            }
            Action::Effect(Effect::LavaIgnite) => {
                if !self.fire_immune {
                    self.ignite_for_ticks(300);
                }
            }
            Action::Effect(Effect::Extinguish) => self.clear_fire(),
            Action::Hurt(source, amount) => {
                self.hurt(source, amount);
            }
            Action::LavaHurt => {
                if !self.fire_immune
                    && self.hurt(Source::Lava, 4.0)
                    && (self.health <= 0 || self.tick_count % 10 == 0)
                {
                    let pitch = 2.0 + draws.next_float() * 0.4;
                    self.play(sounds::ENTITY_GENERIC_BURN, 0.4, pitch);
                }
            }
        }
    }

    /// `BlockState.entityInside` of what an item meets.
    fn entity_inside(&mut self, state: BlockState, collector: &mut Collector) {
        if is_class(state, BlockClass::BaseFireBlock) {
            let damage = if is_class(state, BlockClass::SoulFireBlock) {
                2.0
            } else {
                1.0
            };
            collector.apply(Effect::ClearFreeze);
            collector.apply(Effect::FireIgnite);
            collector.run_after(Effect::FireIgnite, Action::Hurt(Source::InFire, damage));
        } else if is_class(state, BlockClass::CactusBlock) {
            self.hurt(Source::Cactus, 1.0);
        } else if is_class(state, BlockClass::WebBlock) {
            self.make_stuck([0.25, f64::from(0.05_f32), 0.25]);
        } else if is_class(state, BlockClass::PowderSnowBlock) {
            let side = f64::from(0.9_f32);
            self.make_stuck([side, 1.5, side]);
            collector.apply(Effect::Freeze);
            collector.apply(Effect::Extinguish);
        } else if is_class(state, BlockClass::LayeredCauldronBlock) {
            collector.apply(Effect::Extinguish);
        } else if is_class(state, BlockClass::LavaCauldronBlock) {
            collector.apply(Effect::ClearFreeze);
            collector.apply(Effect::LavaIgnite);
            collector.run_after(Effect::LavaIgnite, Action::LavaHurt);
        }
    }

    /// `makeStuckInBlock`.
    fn make_stuck(&mut self, multiplier: Vec3) {
        self.body.fall_distance = 0.0;
        self.body.stuck_speed = multiplier;
    }
}

/// The movements of the tick that `applyEffectsFromBlocks` walks, taken from the body:
/// those its moves made, closed with the way from the last of them to where it is, or
/// the way from where it began the tick if it made none.
pub(crate) fn tick_movements(body: &mut Body) -> Vec<Movement> {
    let mut movements = std::mem::take(&mut body.movements);
    match movements.last() {
        None => movements.push(Movement {
            from: body.old_position,
            to: body.position,
            asked: None,
        }),
        Some(last) => {
            if super::distance_sqr(last.to, body.position) > 9.999_999_439_624_929e-11 {
                movements.push(Movement {
                    from: last.to,
                    to: body.position,
                    asked: None,
                });
            }
        }
    }
    movements
}

/// What the walk through the blocks a move touches asks of the entity, and tells it
/// of each block it meets (`Entity.checkInsideBlocks`).
pub(crate) trait Inside {
    /// The entity's body as it is now: the walk reads its box and its context.
    fn body(&self) -> &Body;

    /// Whether it is gone, which ends the walk.
    fn removed(&self) -> bool;

    /// It met the block `state` at `pos` in the walk's step `step`: its shape for
    /// entities inside, its fluid, or both.
    fn met(&mut self, state: BlockState, pos: BlockPos, step: i32, block: bool, fluid: bool);
}

/// An item's part in the walk: its effects of blocks go through its collector.
struct ItemInside<'a> {
    item: &'a mut Item,
    collector: &'a mut Collector,
}

impl Inside for ItemInside<'_> {
    fn body(&self) -> &Body {
        &self.item.body
    }

    fn removed(&self) -> bool {
        self.item.removed
    }

    fn met(&mut self, state: BlockState, pos: BlockPos, step: i32, block: bool, fluid: bool) {
        if block {
            self.collector.advance_step(step);
            self.item.entity_inside(state, self.collector);
            if handed_back(state) {
                self.item.inside.push(pos);
            }
        }
        if fluid {
            self.collector.advance_step(step);
            if clustine_blocks::data::has_water(state) {
                self.collector.apply(Effect::Extinguish);
            } else if clustine_blocks::data::has_lava(state) {
                self.collector.apply(Effect::ClearFreeze);
                self.collector.apply(Effect::LavaIgnite);
                self.collector
                    .run_after(Effect::LavaIgnite, Action::LavaHurt);
            }
        }
    }
}

/// `checkInsideBlocks(movements, collector)`.
pub(crate) fn check_inside_blocks<W: World + ?Sized, I: Inside + ?Sized>(
    world: &W,
    entity: &mut I,
    movements: &[Movement],
) {
    let mut visited: std::collections::BTreeSet<(i32, i32, i32)> = Default::default();
    for movement in movements {
        let mut from = movement.from;
        let delta = super::sub(movement.to, movement.from);
        let mut budget = 16;
        match movement.asked {
            Some(asked) if super::length_sqr(delta) > 0.0 => {
                for axis in collide::axis_step_order(asked) {
                    let along = delta[axis];
                    if along != 0.0 {
                        let mut next = from;
                        next[axis] += along;
                        budget -=
                            check_inside_between(world, entity, from, next, &mut visited, budget);
                        from = next;
                    }
                }
            }
            _ => {
                budget -= check_inside_between(
                    world,
                    entity,
                    movement.from,
                    movement.to,
                    &mut visited,
                    16,
                );
            }
        }
        if budget <= 0 {
            check_inside_between(world, entity, movement.to, movement.to, &mut visited, 1);
        }
    }
}

/// `checkInsideBlocks(from, to, collector, visited, maxSteps)`: the blocks the box
/// meets on its way, told to the entity; one more than the last step reached.
fn check_inside_between<W: World + ?Sized, I: Inside + ?Sized>(
    world: &W,
    entity: &mut I,
    from: Vec3,
    to: Vec3,
    visited: &mut std::collections::BTreeSet<(i32, i32, i32)>,
    max_steps: i32,
) -> i32 {
    let bounds = entity.body().bounds_at(to).deflate(f64::from(1.0e-5_f32));
    let mut last = 0;
    let moving = entity.body().bounds_at(from);
    walk::for_each_block_between(from, to, &bounds, |pos, step| {
        if entity.removed() {
            return false;
        }
        if step >= max_steps {
            return false;
        }
        last = step;
        let state = world.state(pos);
        if state.props().is_air() {
            return true;
        }
        let hit_block = match inside_shape(entity.body(), state, pos) {
            None => true,
            Some(boxes) => moving.collided_along_vector(super::sub(to, from), &boxes),
        };
        let hit_fluid = fluid_box(world, state, pos)
            .is_some_and(|fluid| moving.collided_along_vector(super::sub(to, from), &[fluid]));
        if (!hit_block && !hit_fluid) || !visited.insert((pos.x, pos.y, pos.z)) {
            return true;
        }
        entity.met(state, pos, step, hit_block, hit_fluid);
        true
    });
    last + 1
}

/// The boxes of the entity-inside shape of `state` at `pos`, `None` for the whole
/// block (`Shapes.block()`, which the game takes as met without looking).
fn inside_shape(body: &Body, state: BlockState, pos: BlockPos) -> Option<Vec<Aabb>> {
    let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    let shape = match state.entity_inside_shape() {
        EntityInsideShape::Shape(shape) if shape.is_block() => return None,
        EntityInsideShape::Shape(shape) => Placed {
            cells: Cells::Data(shape),
            shift: None,
            offset: at,
        },
        EntityInsideShape::CollisionOfTheEntity => {
            match collide::collision_shape(state, pos, body.context()) {
                Some((shape, false)) => shape,
                _ => return None,
            }
        }
    };
    Some(shape.boxes())
}

/// `move(SELF, vector)`, and the region's guard after it: where the move would take
/// the entity into a chunk the region may not hold it in (one of its own, loaded and
/// full), it keeps its coordinate along that axis and loses its motion along it
/// (`docs/groundwork/n4-plan.md`, section 7, as step N3d narrows it).
pub(crate) fn move_guarded<R: Region + ?Sized>(
    body: &mut Body,
    world: &R,
    no_physics: bool,
    vector: Vec3,
) {
    move_guarded_as(body, world, no_physics, vector, None);
}

/// `move(type, vector)`, by the entity itself or, given how far pistons have moved it
/// in this tick, by a piston ([`Body::move_as`]), and the region's guard after it.
pub(crate) fn move_guarded_as<R: Region + ?Sized>(
    body: &mut Body,
    world: &R,
    no_physics: bool,
    vector: Vec3,
    piston: Option<&mut [f64; 3]>,
) {
    let before = body.position;
    body.move_as(world, no_physics, vector, piston);
    hold_back(body, world, before);
}

/// The region's guard after a move that began at `before` ([`move_guarded`]).
pub(crate) fn hold_back<R: Region + ?Sized>(body: &mut Body, world: &R, before: Vec3) {
    let after = body.position;
    if ChunkPos::containing(after[0], before[2]) != ChunkPos::containing(before[0], before[2])
        && !world.may_hold(ChunkPos::containing(after[0], before[2]))
    {
        body.position[0] = before[0];
        body.motion[0] = 0.0;
    }
    let x = body.position[0];
    if ChunkPos::containing(x, after[2]) != ChunkPos::containing(x, before[2])
        && !world.may_hold(ChunkPos::containing(x, after[2]))
    {
        body.position[2] = before[2];
        body.motion[2] = 0.0;
    }
}

/// `FluidState.getAABB`: the box of the fluid in `state`, as high as its height (the
/// game adds the height to the block's y as floats).
fn fluid_box<W: World + ?Sized>(world: &W, state: BlockState, pos: BlockPos) -> Option<Aabb> {
    let fluid = clustine_blocks::fluid::FluidState::of(state);
    fluid.kind()?;
    let height = clustine_blocks::fluid::height(&mut super::Rules(world), fluid, pos);
    let base = Aabb::of_block(pos);
    Some(Aabb::new(
        base.min,
        [base.max[0], f64::from(pos.y as f32 + height), base.max[2]],
    ))
}

/// The most of the item a stack holds (`minecraft:max_stack_size`).
pub(crate) fn max_stack_size(stack: &ItemStack) -> i32 {
    usize::try_from(stack.item)
        .ok()
        .filter(|item| *item < clustine_data::ITEMS.len())
        .map_or(64, |_| {
            i32::from(clustine_data::ItemComponents::of(stack.item).max_stack_size)
        })
}

/// `ItemStack.isSameItemSameComponents` of two slots, where an empty slot is the game's
/// empty stack: both empty, or the same item with the same components.
pub(crate) fn same_kind_or_both_none(a: Option<&ItemStack>, b: Option<&ItemStack>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_kind(b),
        _ => false,
    }
}

/// Whether the stack's item resists fire (`canBeHurtBy(inFire)` is false).
pub(crate) fn resists_fire(stack: &ItemStack) -> bool {
    usize::try_from(stack.item)
        .ok()
        .filter(|item| *item < clustine_data::ITEMS.len())
        .is_some_and(|_| clustine_data::ItemComponents::of(stack.item).is_fire_resistant())
}
