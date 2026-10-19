//! `applyEffectsFromBlocks` of a living body: what it stands on (slime slows it,
//! redstone ore lights), and what the blocks and fluids its moves went through do to
//! it, in the game's order of effects (`InsideBlockEffectApplier.StepBasedCollector`,
//! as `physics::item` has it for items): fire lights it, water puts it out, lava lights
//! it, powder snow holds and freezes it, a cobweb and a berry bush hold it, a lit
//! campfire burns it.

use clustine_blocks::data::{flag, has_lava, has_water, is_class};
use clustine_data::{BlockClass, BlockState, sounds};
use clustine_world::BlockPos;

use super::{Damage, Living};
use crate::physics::body::Body;
use crate::physics::item::{Draws, Inside, check_inside_blocks, handed_back, tick_movements};
use crate::physics::{Vec3, World};

/// The effects of blocks a body is in, in the game's order of applying them
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
    Hurt(Damage, f32),
    LavaHurt,
}

/// Each effect once in a step however many blocks of the step applied it, with what
/// the blocks queued before and after it.
#[derive(Debug, Default)]
struct Collector {
    in_step: [bool; 5],
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

/// A body's part in the walk through the blocks its moves touched.
struct LivingInside<'a, W: World + ?Sized> {
    living: &'a mut Living,
    collector: &'a mut Collector,
    world: &'a W,
}

impl<W: World + ?Sized> Inside for LivingInside<'_, W> {
    fn body(&self) -> &Body {
        &self.living.body
    }

    fn removed(&self) -> bool {
        !self.living.alive()
    }

    fn met(&mut self, state: BlockState, pos: BlockPos, step: i32, block: bool, fluid: bool) {
        if block {
            self.collector.advance_step(step);
            let in_block = self.world.state(self.living.body.block_position());
            self.living.entity_inside(state, in_block, self.collector);
            if handed_back(state) {
                self.living.inside.push(pos);
            }
        }
        if fluid {
            self.collector.advance_step(step);
            if has_water(state) {
                self.collector.apply(Effect::Extinguish);
            } else if has_lava(state) {
                self.collector.apply(Effect::ClearFreeze);
                self.collector.apply(Effect::LavaIgnite);
                self.collector
                    .run_after(Effect::LavaIgnite, Action::LavaHurt);
            }
        }
    }
}

impl Living {
    /// `applyEffectsFromBlocks()` of this tick's moves.
    pub(super) fn apply_effects_from_blocks<W: World + ?Sized>(
        &mut self,
        world: &W,
        draws: &mut dyn Draws,
    ) {
        let movements = tick_movements(&mut self.body);
        self.last_movements = movements.clone();
        // `isAffectedByBlocks`.
        if !self.alive() || self.no_physics {
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
            &mut LivingInside {
                living: self,
                collector: &mut collector,
                world,
            },
            &movements,
        );
        collector.flush_step();
        for action in std::mem::take(&mut collector.queued) {
            if !self.alive() {
                break;
            }
            self.run(action);
        }
        if was_on_fire && !self.is_on_fire() {
            // `playEntityOnFireExtinguishedSound`, its pitch of two of its own draws.
            let pitch = 1.6 + (draws.next_float() - draws.next_float()) * 0.4;
            self.play(sounds::ENTITY_GENERIC_EXTINGUISH_FIRE, 0.7, pitch);
        }
        let ignited = self.fire_ticks > fire_before;
        if !self.is_on_fire() && !ignited {
            self.fire_ticks = 0;
        }
    }

    /// `Block.stepOn` of what it stands on.
    fn step_on(&mut self, pos: BlockPos, state: BlockState) {
        if is_class(state, BlockClass::SlimeBlock) {
            // A body that does not step carefully is slowed on slime.
            let up = self.body.motion[1].abs();
            if up < 0.1 {
                let factor = 0.4 + up * 0.2;
                let [x, y, z] = self.body.motion;
                self.body.motion = [x * factor, y * 1.0, z * factor];
            }
        } else if is_class(state, BlockClass::RedStoneOreBlock) {
            self.ore.push(pos);
        }
        // A magma block's `hotFloor` hit breaks no stand.
    }

    /// `BlockState.entityInside` of what a living body meets, `in_block` the block at
    /// its feet (`getInBlockState`). A cactus's and a berry bush's hits break no stand,
    /// and are not asked.
    fn entity_inside(
        &mut self,
        state: BlockState,
        in_block: BlockState,
        collector: &mut Collector,
    ) {
        if is_class(state, BlockClass::BaseFireBlock) {
            let damage = if is_class(state, BlockClass::SoulFireBlock) {
                2.0
            } else {
                1.0
            };
            collector.apply(Effect::ClearFreeze);
            collector.apply(Effect::FireIgnite);
            collector.run_after(Effect::FireIgnite, Action::Hurt(Damage::InFire, damage));
        } else if is_class(state, BlockClass::CampfireBlock) {
            // **Inferred**: `CampfireBlock.entityInside` is not in the dump; a lit
            // campfire hurts a living body in it at once with its fire damage (1, a
            // soul campfire's 2), lighting nothing, as the reference servers have it.
            if flag(state, "lit") {
                let damage =
                    if clustine_blocks::data::is_block(state, clustine_data::blocks::SOUL_CAMPFIRE)
                    {
                        2.0
                    } else {
                        1.0
                    };
                self.hurt(Damage::Campfire, damage);
            }
        } else if is_class(state, BlockClass::WebBlock) {
            self.make_stuck([0.25, f64::from(0.05_f32), 0.25]);
        } else if is_class(state, BlockClass::SweetBerryBushBlock) {
            self.make_stuck([f64::from(0.8_f32), 0.75, f64::from(0.8_f32)]);
        } else if is_class(state, BlockClass::PowderSnowBlock) {
            // **Inferred**: `PowderSnowBlock` is not in the dump. A living body is held
            // only where the powder snow is the block at its feet, and is put in it
            // (`FREEZE`) and put out wherever it meets it.
            if is_class(in_block, BlockClass::PowderSnowBlock) {
                let side = f64::from(0.9_f32);
                self.make_stuck([side, 1.5, side]);
            }
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

    /// One queued action of the effects of blocks.
    fn run(&mut self, action: Action) {
        match action {
            // **Inferred** (`InsideBlockEffectType` is not in the dump): `FREEZE` puts
            // the body in powder snow, whose freezing `aiStep` counts, and
            // `CLEAR_FREEZE` thaws it at once.
            Action::Effect(Effect::Freeze) => self.in_powder_snow = true,
            Action::Effect(Effect::ClearFreeze) => self.frozen = 0,
            Action::Effect(Effect::FireIgnite) => {
                // `BaseFireBlock.fireIgnite`.
                if self.fire_ticks < 0 {
                    self.fire_ticks += 1;
                }
                if self.fire_ticks >= 0 {
                    self.ignite_for_ticks(160);
                }
            }
            Action::Effect(Effect::LavaIgnite) => self.ignite_for_ticks(300),
            Action::Effect(Effect::Extinguish) => self.clear_fire(),
            Action::Hurt(damage, amount) => {
                self.hurt(damage, amount);
            }
            Action::LavaHurt => {
                // `lavaHurt`: the burning sound only where lava hurt it, which no
                // stand is.
                self.hurt(Damage::Lava, 4.0);
            }
        }
    }
}
