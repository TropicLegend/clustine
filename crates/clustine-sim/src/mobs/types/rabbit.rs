//! Rabbits (`Rabbit` and its nested classes; `docs/groundwork/n7-brief.md`, section
//! 4.12): an animal of carrots and dandelions that hops: its own move and jump controls
//! start a hop wherever it wants to go once its landing's delay is over, and its jump is
//! as high as where it goes asks. It flees wolves and monsters (never a creative
//! player), raids ripe carrots, and is of a variant its biome draws.
//!
//! Named differences: no rabbit is the killer bunny (only commands and data make one);
//! `mobGriefing` is the game's default, on; the idle head tilt's timer is drawn and
//! read by nothing, as on the game's server.

use clustine_blocks::data::{in_tag, is_class, number, set};
use clustine_data::{BlockClass, BlockState, attributes as ids, blocks, entity_types, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, biome_in_tag};
use super::{Breeding, Dimensions, Interaction, MobType, Spawning, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, ClimbOnTopOfPowderSnowGoal, FloatGoal, Items, LookAtPlayerGoal,
    MoveToBlockGoal, MovingToBlock, Panic, PanicGoal, RandomStrollGoal, TemptGoal,
};
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Operation, Setup, controls};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// `#rabbit_food`.
const FOOD: Items = Items::Tag("minecraft:rabbit_food");

/// A kit's size.
const BABY: Dimensions = Dimensions {
    width: 0.24,
    height: 0.4,
    eye_height: 0.39,
};

/// The synced data's index of its variant (`DATA_TYPE_ID`).
const TYPE_INDEX: u8 = 18;

/// `Rabbit$Variant`'s ids.
pub(crate) mod variant {
    pub(crate) const BROWN: i32 = 0;
    pub(crate) const WHITE: i32 = 1;
    pub(crate) const BLACK: i32 = 2;
    pub(crate) const WHITE_SPLOTCHED: i32 = 3;
    pub(crate) const GOLD: i32 = 4;
    pub(crate) const SALT: i32 = 5;
}

/// What a rabbit keeps beyond an animal: its variant; the carrot it last ate's ticks
/// (`moreCarrotTicks`); its hop (`jumpTicks`, `jumpDuration`, `wasOnGround`,
/// `jumpDelayTicks`); its jump control's `canJump` and its move control's
/// `nextJumpSpeed`; whether its garden raid wants to and can raid; the idle timer its
/// constructor draws.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RabbitState {
    pub(crate) variant: i32,
    pub(crate) more_carrot_ticks: i32,
    pub(crate) jump_ticks: i32,
    pub(crate) jump_duration: i32,
    pub(crate) was_on_ground: bool,
    pub(crate) jump_delay_ticks: i32,
    pub(crate) can_jump: bool,
    pub(crate) next_jump_speed: f64,
    pub(crate) wants_to_raid: bool,
    pub(crate) can_raid: bool,
    pub(crate) idle_timeout: i32,
}

impl Own for RabbitState {}

/// The rabbit.
pub(crate) struct Rabbit;

fn state_of(mob: &Mob) -> RabbitState {
    animal::read::<RabbitState>(&mob.state).1
}

fn put_state(mob: &mut Mob, own: &RabbitState) {
    let animal = animal::animal_of(&mob.state);
    mob.state = animal::write(&animal, own);
}

/// `getRandomRabbitVariant(level, pos)`: one draw of the level's always; white or
/// splotched in snow, gold in deserts, else brown, salt and pepper or black.
pub(crate) fn random_variant(
    biome: Option<&str>,
    level: &mut dyn clustine_world::generation::Draws,
) -> i32 {
    let drawn = level.next_int(100);
    if biome_in_tag(biome, "minecraft:spawns_white_rabbits") {
        if drawn < 80 {
            variant::WHITE
        } else {
            variant::WHITE_SPLOTCHED
        }
    } else if biome_in_tag(biome, "minecraft:spawns_gold_rabbits") {
        variant::GOLD
    } else if drawn < 50 {
        variant::BROWN
    } else if drawn < 90 {
        variant::SALT
    } else {
        variant::BLACK
    }
}

/// `RabbitMoveControl.setWantedPosition(x, y, z, speed)`'s own part: in water a rabbit
/// goes at 1.5, and a speed above nothing is its next hop's.
fn wanted_speed(mob: &mut Mob, in_water: bool, speed: f64, own: &mut RabbitState) -> f64 {
    let speed = if in_water { 1.5 } else { speed };
    if speed > 0.0 {
        own.next_jump_speed = speed;
    }
    let _ = mob;
    speed
}

/// `Rabbit.setSpeedModifier(speed)`: its navigation's speed, and the move control
/// wanting where it wanted at that speed.
fn set_speed_modifier(mob: &mut Mob, in_water: bool, speed: f64, own: &mut RabbitState) {
    mob.body.progress.speed = speed;
    let speed = wanted_speed(mob, in_water, speed, own);
    let movement = &mut mob.body.controls.movement;
    movement.speed = speed;
    if movement.operation != Operation::Jumping {
        movement.operation = Operation::MoveTo;
    }
}

/// `setJumping(jumping)`: the input, and a hop's sound.
fn set_jumping(tick: &mut MobTick<'_>, jumping: bool) {
    tick.mob.body.inputs.jumping = jumping;
    if jumping {
        let pitch = ((tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0) * 0.8;
        tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_RABBIT_JUMP,
            volume: 1.0,
            pitch,
        });
    }
}

/// `startJumping()`.
fn start_jumping(tick: &mut MobTick<'_>, own: &mut RabbitState) {
    set_jumping(tick, true);
    own.jump_duration = 15;
    own.jump_ticks = 0;
}

/// `Path.getNextEntityPos(entity)` of the path the mob follows, where it is not done.
fn next_path_pos(mob: &Mob, width: f32) -> Option<Vec3> {
    let path = mob.body.path.followed.as_ref()?;
    let node = path.nodes.get(mob.body.progress.next as usize)?;
    let offset = f64::from((width + 1.0) as i32) * 0.5;
    Some([
        f64::from(node.x) + offset,
        f64::from(node.y),
        f64::from(node.z) + offset,
    ])
}

/// `facePoint(x, z)`.
fn face_point(tick: &mut MobTick<'_>, x: f64, z: f64) {
    let [px, _, pz] = tick.living.body.position;
    tick.yaw = crate::mobs::ai::maths::yaw_towards(x - px, z - pz);
}

/// `RabbitPanicGoal(rabbit, speed)`: a panic that hops at its speed every tick of it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RabbitPanicGoal {
    pub(crate) speed: f64,
}

impl Goal for RabbitPanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        self.panic().flags()
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        self.panic().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        self.panic().can_continue(state, ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        self.panic().start(state, ai);
    }

    fn stop(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        self.panic().stop(state, ai);
    }

    fn tick(&self, _: &mut Panic, ai: &mut Ai<'_, '_>) {
        let in_water = ai.in_water();
        let mut own = state_of(&ai.tick.mob);
        set_speed_modifier(&mut ai.tick.mob, in_water, self.speed, &mut own);
        put_state(&mut ai.tick.mob, &own);
    }
}

impl RabbitPanicGoal {
    fn panic(&self) -> PanicGoal {
        PanicGoal { speed: self.speed }
    }
}

/// `RaidGardenGoal`'s state: its `MoveToBlockGoal`'s.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Raiding {
    pub(crate) moving: MovingToBlock,
}

/// `RaidGardenGoal(rabbit)`: a rabbit that wants more food goes to a ripe carrot within
/// 16 blocks and eats a stage of it. Its target counts only while it has not found one
/// (`canRaid`), so the game's goal goes on only as long as its first ticks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RaidGardenGoal;

impl RaidGardenGoal {
    fn moving() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: f64::from(0.7_f32),
            search_range: 16,
            vertical_search_range: 1,
            vertical_search_start: 0,
            valid: raid_target,
            next_start: None,
        }
    }
}

/// `RaidGardenGoal.isValidTarget(level, pos)`: a block that carries crops, with a ripe
/// carrot above it, while the rabbit wants to raid and has found none.
fn raid_target(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let own = state_of(&ai.tick.mob);
    if !own.wants_to_raid || own.can_raid {
        return false;
    }
    let world = ai.tick.world;
    if !in_tag(world.state(pos), "minecraft:supports_crops") {
        return false;
    }
    let above = world.state(BlockPos::new(pos.x, pos.y + 1, pos.z));
    is_carrot(above) && number(above, "age").is_some_and(|age| age >= 7)
}

fn is_carrot(state: BlockState) -> bool {
    is_class(state, BlockClass::CarrotBlock)
}

impl Goal for RaidGardenGoal {
    type State = Raiding;

    fn flags(&self) -> u8 {
        Self::moving().flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Raiding, ai: &mut Ai<'_, '_>) -> bool {
        if state.moving.next_start_tick <= 0 {
            let mut own = state_of(&ai.tick.mob);
            own.can_raid = false;
            own.wants_to_raid = own.more_carrot_ticks <= 0;
            put_state(&mut ai.tick.mob, &own);
        }
        let found = Self::moving().can_use(&mut state.moving, ai);
        if found {
            // The first valid target the search met set `canRaid`.
            let mut own = state_of(&ai.tick.mob);
            own.can_raid = true;
            put_state(&mut ai.tick.mob, &own);
        }
        found
    }

    fn can_continue(&self, state: &mut Raiding, ai: &mut Ai<'_, '_>) -> bool {
        state_of(&ai.tick.mob).can_raid && Self::moving().can_continue(&mut state.moving, ai)
    }

    fn start(&self, state: &mut Raiding, ai: &mut Ai<'_, '_>) {
        Self::moving().start(&mut state.moving, ai);
    }

    fn stop(&self, state: &mut Raiding, ai: &mut Ai<'_, '_>) {
        Self::moving().stop(&mut state.moving, ai);
    }

    fn tick(&self, state: &mut Raiding, ai: &mut Ai<'_, '_>) {
        Self::moving().tick(&mut state.moving, ai);
        let block = state.moving.block.unwrap_or(BlockPos::new(0, 0, 0));
        let max_pitch = ai.head().max_x_rot as f32;
        controls::set_look_at(
            ai,
            [
                f64::from(block.x) + 0.5,
                f64::from(block.y + 1),
                f64::from(block.z) + 0.5,
            ],
            10.0,
            max_pitch,
        );
        if !state.moving.reached {
            return;
        }
        let above = BlockPos::new(block.x, block.y + 1, block.z);
        let carrot = ai.tick.world.state(above);
        let mut own = state_of(&ai.tick.mob);
        if own.can_raid && is_carrot(carrot) {
            let age = number(carrot, "age").unwrap_or(0);
            if age == 0 {
                // Air first, then `destroyBlock` of the air, which does nothing.
                ai.tick.out.local.push(Local::SetBlock {
                    pos: above,
                    state: blocks::AIR,
                    flags: 2,
                });
            } else {
                let eaten = set(carrot, "age", &(age - 1).to_string());
                ai.tick.out.local.push(Local::SetBlock {
                    pos: above,
                    state: eaten,
                    flags: 2,
                });
                ai.tick.out.local.push(Local::GameEvent {
                    name: "minecraft:block_change",
                    pos: above,
                });
                ai.tick.out.local.push(Local::LevelEvent {
                    kind: 2001,
                    pos: above,
                    data: i32::from(carrot.0),
                });
            }
            own.more_carrot_ticks = 40;
        }
        own.can_raid = false;
        put_state(&mut ai.tick.mob, &own);
        state.moving.next_start_tick = 10;
    }
}

impl MobType for Rabbit {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        let wolf = &[entity_types::WOLF as u16][..];
        setup.add_goal(1, FloatGoal);
        setup.add_goal(1, ClimbOnTopOfPowderSnowGoal);
        setup.add_goal(1, RabbitPanicGoal { speed: 2.2 });
        setup.add_goal(2, animal::BreedGoal { speed: 0.8 });
        setup.add_goal(3, TemptGoal::new(1.0, FOOD, false));
        for (class, distance) in [
            (Class::Player, 8.0),
            (Class::Kinds(wolf), 10.0),
            (Class::Monster, 4.0),
        ] {
            setup.add_goal(
                4,
                AvoidEntityGoal {
                    class,
                    max_distance: distance,
                    walk_speed: 2.2,
                    sprint_speed: 2.2,
                },
            );
        }
        setup.add_goal(5, RaidGardenGoal);
        setup.add_goal(6, RandomStrollGoal::avoiding_water(0.6));
        setup.add_goal(11, LookAtPlayerGoal::new(Class::Player, 10.0));
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK)
    }

    /// The constructor: its idle timer drawn, and `setSpeedModifier(0)`: it wants the
    /// world's origin at no speed, and so is always going somewhere (the brief's
    /// finding 14).
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let mut state = state_of(mob);
        state.idle_timeout = own.next_int(40) as i32 + 180;
        set_speed_modifier(mob, false, 0.0, &mut state);
        put_state(mob, &state);
    }

    /// `Rabbit.customServerAiStep` (it does not call `Animal`'s): the landing's delay
    /// counted down, the carrot's ticks drawn down, and on the ground a landing's delay
    /// begun, then a hop towards where it wants to go once the delay is over.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let mut own = state_of(&tick.mob);
        let in_water = tick.living.fluids.in_water();
        // What its navigation wanted in this step went through its move control's
        // `setWantedPosition`, which the rabbit's sees first.
        let movement = &tick.mob.body.controls.movement;
        if movement.operation == Operation::MoveTo {
            let speed = movement.speed;
            let speed = wanted_speed(&mut tick.mob, in_water, speed, &mut own);
            tick.mob.body.controls.movement.speed = speed;
        }
        if own.jump_delay_ticks > 0 {
            own.jump_delay_ticks -= 1;
        }
        if own.more_carrot_ticks > 0 {
            own.more_carrot_ticks -= tick.draws.next_int(3) as i32;
            if own.more_carrot_ticks < 0 {
                own.more_carrot_ticks = 0;
            }
        }
        let on_ground = tick.living.body.on_ground;
        if on_ground {
            if !own.was_on_ground {
                set_jumping(tick, false);
                // `checkLandingDelay()`.
                own.jump_delay_ticks = if tick.mob.body.controls.movement.speed < 2.2 {
                    10
                } else {
                    3
                };
                own.can_jump = false;
            }
            if !tick.mob.body.controls.jump {
                let wanted = tick.mob.body.controls.movement.operation == Operation::MoveTo;
                if wanted && own.jump_delay_ticks == 0 {
                    let width = tick.living.body.width;
                    let to = next_path_pos(&tick.mob, width)
                        .unwrap_or(tick.mob.body.controls.movement.wanted);
                    face_point(tick, to[0], to[2]);
                    start_jumping(tick, &mut own);
                }
            } else if !own.can_jump {
                own.can_jump = true;
            }
        }
        own.was_on_ground = on_ground;
        put_state(&mut tick.mob, &own);
    }

    /// `RabbitMoveControl.tick()`: on the ground and not hopping it wants to go nowhere
    /// (its speed nothing), else at its next hop's speed; then a move control's.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        let in_water = ai.in_water();
        let mut own = state_of(&ai.tick.mob);
        let mob = &mut ai.tick.mob;
        if ai.tick.living.body.on_ground && !mob.body.inputs.jumping && !mob.body.controls.jump {
            set_speed_modifier(mob, in_water, 0.0, &mut own);
        } else if matches!(
            mob.body.controls.movement.operation,
            Operation::MoveTo | Operation::Jumping
        ) {
            let speed = own.next_jump_speed;
            set_speed_modifier(mob, in_water, speed, &mut own);
        }
        put_state(&mut ai.tick.mob, &own);
        controls::tick_move(ai);
    }

    /// `RabbitJumpControl.tick()`: a jump asked for starts a hop.
    fn tick_jump_control(&self, ai: &mut Ai<'_, '_>) {
        if std::mem::take(&mut ai.tick.mob.body.controls.jump) {
            let mut own = state_of(&ai.tick.mob);
            start_jumping(ai.tick, &mut own);
            put_state(&mut ai.tick.mob, &own);
        }
    }

    /// `Rabbit.getJumpPower()`: 0.3, 0.2 for a slow hop, 0.5 up to a higher node or
    /// against a wall, as `LivingEntity.getJumpPower(f / 0.42f)`.
    fn jump_power(&self, tick: &MobTick<'_>, block_factor: f32) -> Option<f32> {
        let movement = &tick.mob.body.controls.movement;
        let mut power = 0.3_f32;
        if movement.speed <= 0.6 {
            power = 0.2;
        }
        let y = tick.living.body.position[1];
        if next_path_pos(&tick.mob, tick.living.body.width).is_some_and(|next| next[1] > y + 0.5) {
            power = 0.5;
        }
        if tick.living.body.horizontal_collision
            || (tick.mob.body.inputs.jumping && movement.wanted[1] > y + 0.5)
        {
            power = 0.5;
        }
        let strength = tick
            .mob
            .body
            .attributes
            .value(tick.kind, crate::mobs::body::attribute(ids::JUMP_STRENGTH))
            as f32;
        Some(power / 0.42 * strength * block_factor)
    }

    /// `Rabbit.jumpFromGround()` after `LivingEntity`'s: a hop from standing pushes it on,
    /// and clients see it hop (entity event 1).
    fn jumped_from_ground(&self, tick: &mut MobTick<'_>) {
        let motion = tick.living.body.motion;
        if tick.mob.body.controls.movement.speed > 0.0
            && motion[0] * motion[0] + motion[2] * motion[2] < 0.01
        {
            let up = if animal::is_baby(&tick.mob) { 0.5 } else { 1.5 };
            let push = crate::physics::minecart::input_vector([0.0, up, 1.0], 0.1, tick.yaw);
            tick.living.body.motion = physics::add(motion, push);
        }
        tick.out.events.push(Event::Entity(1));
    }

    /// `Rabbit.aiStep` after `Animal`'s: a hop ends when its ticks are its duration.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let mut own = state_of(&tick.mob);
        if own.jump_ticks != own.jump_duration {
            own.jump_ticks += 1;
        } else if own.jump_duration != 0 {
            own.jump_ticks = 0;
            own.jump_duration = 0;
            set_jumping(tick, false);
        }
        put_state(&mut tick.mob, &own);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        animal::mob_interact(used, FOOD, None)
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let mut own = state_of(spawn.mob);
        own.variant = random_variant(spawn.biome, spawn.level);
        put_state(spawn.mob, &own);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this = state_of(breeding.this);
        let partner = state_of(breeding.partner);
        let mut variant = random_variant(breeding.biome, breeding.draws);
        if breeding.draws.next_int(20) != 0 {
            variant = if breeding.draws.next_bool() {
                partner.variant
            } else {
                this.variant
            };
        }
        let mut child = Mob::fresh();
        let own = RabbitState {
            variant,
            ..RabbitState::default()
        };
        child.state = animal::write(&animal::Animal::default(), &own);
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_RABBIT_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_RABBIT_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_RABBIT_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            TYPE_INDEX,
            DataShown::Int(own.variant),
            own.variant == variant::BROWN,
        ));
        values
    }
}
