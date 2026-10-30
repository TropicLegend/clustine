//! A mob's effects, from the game's rules (`LivingEntity.addEffect`, `tickEffects`,
//! `hurtServer`'s fire test, `baseTick`'s air, `MobEffectInstance.update` and
//! `tickServer`; `docs/groundwork/n7k-plan.md`, section 10), not from the code: what
//! each built effect does, how long an effect lasts, and how a second one of the same
//! effect is taken.

use clustine_data::{BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, EntityId};

use super::effects::{self, Effect};
use super::{Body, attribute};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::part::{self, body_ids};
use crate::mobs::tick::{self, MobOutput, MobTick};
use crate::mobs::types::of;
use crate::mobs::view::{LightAt, MobWorld, Seen};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;

const COW: u16 = entity_types::COW as u16;
const ZOMBIE: u16 = entity_types::ZOMBIE as u16;
const KEY: EntityId = EntityId(40);
/// The moment of the first tick the tests run.
const START: u32 = 100;

/// A floor of stone with three blocks of `fluid` on it (air for none), lit and at night,
/// so that nothing burns of the sun.
struct Pool {
    fluid: BlockState,
}

impl World for Pool {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(match pos.y {
            ..0 => blocks::STONE,
            0..3 => self.fluid,
            _ => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Pool {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Pool {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn entity(&self, _: EntityId) -> Option<Seen<'_>> {
        None
    }

    fn players(&self) -> Vec<crate::mobs::view::SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        Some(LightAt { sky: 15, block: 0 })
    }

    fn day(&self) -> u64 {
        18_000
    }
}

/// A body of `kind` at rest on the floor, its first tick behind it.
fn living(kind: u16) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let position = [0.5, 0.0, 0.5];
    let body = Moving {
        stands_on_lava: false,
        position,
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: position,
        horizontal_collision: false,
        vertical_collision: false,
        vertical_collision_below: false,
        supporting: None,
        on_ground_no_blocks: false,
        stuck_speed: [0.0; 3],
        movements: Vec::new(),
        in_water: false,
        sync_position: false,
        falling_block: false,
        item: false,
        fell_on: None,
        noclip: None,
        mover: Mover {
            key: KEY,
            root: KEY,
            like_a_vehicle: false,
            happy_ghast: false,
        },
        steps: None,
    };
    let mut living = Living::new(
        body,
        Kind::Mob {
            kind,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living
}

fn max_health(kind: u16) -> f32 {
    super::attributes::supplied(kind, attribute(ids::MAX_HEALTH)).expect("a mob has health") as f32
}

/// A mob of `kind` of full health and air, without AI, so that it stays where it is put.
fn mob(kind: u16) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = max_health(kind);
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.no_ai = true;
    mob
}

/// A mob ticking in a world, its body carried from one tick to the next as a region
/// carries it in its record.
struct Run {
    world: Pool,
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    stream: Random,
}

impl Run {
    fn new(kind: u16, fluid: BlockState) -> Self {
        Self {
            world: Pool { fluid },
            kind,
            mob: mob(kind),
            living: living(kind),
            lived: START,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

    /// `f` on the mob's tick as it stands between ticks (at its `lived`), or as
    /// `counted` says.
    fn with<R>(&mut self, counted: bool, f: impl FnOnce(&mut MobTick<'_>) -> R) -> R {
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted,
            mob: std::mem::replace(&mut self.mob, Mob::fresh()),
            living: self.living.clone(),
            yaw: 0.0,
            pitch: 0.0,
            mob_type: of(self.kind),
            world: &self.world,
            draws: &mut self.stream,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let result = f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        result
    }

    /// One whole tick of the mob.
    fn tick(&mut self) {
        self.living.air = self.mob.body.vitals.air;
        self.with(false, tick::tick);
        self.mob.body.vitals.air = self.living.air;
        self.lived += 1;
    }

    /// `addEffect` between ticks.
    fn add(&mut self, effect: Effect, amplifier: u8, duration: i32) -> bool {
        let (kind, now) = (self.kind, self.lived);
        effects::add(&mut self.mob, kind, effect, amplifier, duration, now)
    }

    fn speed(&self) -> f64 {
        self.mob
            .body
            .attributes
            .value(self.kind, attribute(ids::MOVEMENT_SPEED))
    }

    fn health(&self) -> f32 {
        self.mob.body.vitals.health
    }
}

fn base_speed(kind: u16) -> f64 {
    super::attributes::supplied(kind, attribute(ids::MOVEMENT_SPEED)).expect("a mob has speed")
}

#[test]
fn an_effect_added_between_ticks_lasts_through_as_many_ticks_as_its_duration() {
    let mut run = Run::new(COW, blocks::AIR);
    assert!(run.add(Effect::FIRE_RESISTANCE, 0, 3));
    run.tick();
    run.tick();
    assert!(run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
    run.tick();
    assert!(
        !run.mob.body.effects.has(Effect::FIRE_RESISTANCE),
        "three ticks"
    );
    assert_eq!(run.mob.body.effects, Default::default(), "nothing is left");
}

#[test]
fn an_effect_added_in_the_ai_step_counts_from_the_next_tick() {
    let mut run = Run::new(COW, blocks::AIR);
    // At the moment of `aiStep`, after this tick's `tickEffects`.
    let (kind, now) = (run.kind, run.lived + 1);
    effects::add(&mut run.mob, kind, Effect::FIRE_RESISTANCE, 0, 2, now);
    run.lived += 1;
    run.tick();
    assert!(run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
    run.tick();
    assert!(!run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
}

#[test]
fn a_fire_resistant_mob_in_lava_loses_no_health_and_does_once_the_effect_ends() {
    let mut run = Run::new(COW, blocks::LAVA);
    let full = run.health();
    run.add(Effect::FIRE_RESISTANCE, 0, 60);
    for _ in 0..59 {
        run.tick();
        assert_eq!(run.health(), full, "neither the lava nor the burning hurts");
    }
    assert!(run.living.fire_ticks > 0, "it burns all the same");
    // The sixtieth `tickEffects` ends it in `baseTick`, and the lava of that tick's
    // move hurts.
    run.tick();
    assert!(!run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
    assert_eq!(run.health(), full - 4.0, "the lava hurts it now");
}

#[test]
fn a_burning_mob_out_of_lava_takes_no_fire_hurt_while_fire_resistant() {
    let mut run = Run::new(COW, blocks::AIR);
    let full = run.health();
    run.living.fire_ticks = 200;
    run.add(Effect::FIRE_RESISTANCE, 0, 40);
    for _ in 0..40 {
        run.tick();
    }
    assert_eq!(run.health(), full);
    for _ in 0..20 {
        run.tick();
    }
    assert_eq!(
        run.health(),
        full - 1.0,
        "one `on_fire` hurt in twenty ticks"
    );
}

#[test]
fn water_breathing_keeps_a_mob_s_air_under_water_and_gives_back_what_it_lacks() {
    let mut run = Run::new(COW, blocks::WATER);
    run.tick();
    assert_eq!(
        run.mob.body.vitals.air,
        MAX_AIR - 1,
        "without it, air is lost"
    );

    let mut run = Run::new(COW, blocks::WATER);
    run.add(Effect::WATER_BREATHING, 0, 100);
    for _ in 0..50 {
        run.tick();
    }
    assert_eq!(run.mob.body.vitals.air, MAX_AIR);

    // `shouldEffectsRefillAirsupply`: back four a tick under water.
    let mut run = Run::new(COW, blocks::WATER);
    run.mob.body.vitals.air = 200;
    run.add(Effect::WATER_BREATHING, 0, 100);
    run.tick();
    assert_eq!(run.mob.body.vitals.air, 204);
}

#[test]
fn swiftness_raises_the_speed_a_fifth_a_level_and_its_end_takes_it_off() {
    let base = base_speed(COW);
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 0, 2);
    let amount = f64::from(0.2_f32);
    assert_eq!(run.speed(), base * (1.0 + amount));
    run.tick();
    run.tick();
    assert_eq!(run.speed(), base, "its end takes the modifier off");

    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 1, 20);
    assert_eq!(run.speed(), base * (1.0 + amount * 2.0));
    let instance = run
        .mob
        .body
        .attributes
        .made(attribute(ids::MOVEMENT_SPEED))
        .expect("made by the effect");
    assert_eq!(instance.modifiers.len(), 1);
    assert_eq!(instance.modifiers[0].id, "minecraft:effect.speed");
    assert_eq!(instance.modifiers[0].operation, 2, "add_multiplied_total");
}

#[test]
fn invisibility_sets_the_shared_flag_and_its_end_clears_it() {
    let flags = |mob: &Mob| crate::shown::mob_shared_flags(&mob.encode());
    let mut run = Run::new(COW, blocks::AIR);
    assert_eq!(flags(&run.mob), 0);
    run.add(Effect::INVISIBILITY, 0, 2);
    assert_eq!(flags(&run.mob), 0x20);
    run.tick();
    assert_eq!(flags(&run.mob), 0x20);
    run.tick();
    assert_eq!(flags(&run.mob), 0, "cleared at its end");

    // Another effect leaves the flag clear.
    run.add(Effect::FIRE_RESISTANCE, 0, 20);
    assert_eq!(flags(&run.mob), 0);
}

#[test]
fn instant_health_heals_four_eight_at_amplifier_one_and_harms_an_undead() {
    let mut run = Run::new(COW, blocks::AIR);
    run.mob.body.vitals.health = 1.0;
    run.with(false, |tick| effects::instant_health(tick, 0));
    assert_eq!(run.health(), 5.0);
    run.mob.body.vitals.health = 1.0;
    run.with(false, |tick| effects::instant_health(tick, 1));
    assert_eq!(run.health(), 9.0);
    run.with(false, |tick| effects::instant_health(tick, 1));
    assert_eq!(run.health(), max_health(COW), "never past its most");

    // An undead is hurt `6 << amplifier` of magic instead.
    let mut run = Run::new(ZOMBIE, blocks::AIR);
    let full = run.health();
    run.with(false, |tick| effects::instant_health(tick, 0));
    assert_eq!(run.health(), full - 6.0);
}

#[test]
fn a_drunk_potion_of_healing_heals_at_the_next_tick_and_leaves_nothing() {
    let mut run = Run::new(COW, blocks::AIR);
    run.mob.body.vitals.health = 2.0;
    // As the witch drinks: `addEffect` in `aiStep`, at the next moment.
    let (kind, now) = (run.kind, run.lived + 1);
    assert!(effects::add(
        &mut run.mob,
        kind,
        Effect::INSTANT_HEALTH,
        0,
        1,
        now
    ));
    assert_eq!(run.health(), 2.0, "not at once");
    run.lived += 1;
    run.tick();
    assert_eq!(run.health(), 6.0);
    assert!(!run.mob.body.effects.has(Effect::INSTANT_HEALTH));
}

#[test]
fn a_longer_effect_of_the_same_level_replaces_a_shorter_and_a_shorter_does_not_cut_it() {
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::FIRE_RESISTANCE, 0, 2);
    assert!(run.add(Effect::FIRE_RESISTANCE, 0, 5), "lengthened");
    assert!(!run.add(Effect::FIRE_RESISTANCE, 0, 1), "no change");
    for _ in 0..4 {
        run.tick();
    }
    assert!(run.mob.body.effects.has(Effect::FIRE_RESISTANCE));
    run.tick();
    assert!(
        !run.mob.body.effects.has(Effect::FIRE_RESISTANCE),
        "five ticks"
    );
}

#[test]
fn a_weaker_effect_does_not_shorten_or_weaken_a_stronger_and_waits_hidden_if_longer() {
    let base = base_speed(COW);
    let amount = f64::from(0.2_f32);
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 1, 3);
    assert!(!run.add(Effect::SPEED, 0, 1), "weaker and shorter: nothing");
    assert!(!run.add(Effect::SPEED, 0, 6), "weaker and longer: hidden");
    assert_eq!(run.mob.body.effects.amplifier(Effect::SPEED), Some(1));
    assert_eq!(run.speed(), base * (1.0 + amount * 2.0));
    for _ in 0..3 {
        run.tick();
    }
    // The stronger ended; the weaker, which counted down below it, comes back.
    assert_eq!(run.mob.body.effects.amplifier(Effect::SPEED), Some(0));
    assert_eq!(run.speed(), base * (1.0 + amount));
    for _ in 0..2 {
        run.tick();
    }
    assert!(run.mob.body.effects.has(Effect::SPEED));
    run.tick();
    assert!(!run.mob.body.effects.has(Effect::SPEED), "six ticks in all");
    assert_eq!(run.speed(), base);
}

#[test]
fn a_stronger_shorter_effect_takes_over_and_the_weaker_comes_back_after_it() {
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 0, 5);
    assert!(run.add(Effect::SPEED, 2, 2));
    assert_eq!(run.mob.body.effects.amplifier(Effect::SPEED), Some(2));
    run.tick();
    run.tick();
    assert_eq!(run.mob.body.effects.amplifier(Effect::SPEED), Some(0));
    for _ in 0..3 {
        run.tick();
    }
    assert!(
        !run.mob.body.effects.has(Effect::SPEED),
        "five ticks from its start"
    );
}

#[test]
fn an_infinite_effect_does_not_end_and_a_longer_one_does_not_replace_it() {
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::WATER_BREATHING, 0, -1);
    assert!(!run.add(Effect::WATER_BREATHING, 0, 1_000));
    for _ in 0..50 {
        run.tick();
    }
    assert!(run.mob.body.effects.has(Effect::WATER_BREATHING));
}

#[test]
fn a_type_of_a_tag_is_spared_its_effect() {
    // The skeletons ignore poison and regeneration (`#ignores_poison_and_regen`).
    let mut run = Run::new(entity_types::SKELETON as u16, blocks::AIR);
    assert!(!run.add(Effect::POISON, 0, 100));
    assert!(!run.mob.body.effects.has(Effect::POISON));
    assert!(run.add(Effect::FIRE_RESISTANCE, 0, 100));
}

#[test]
fn a_record_written_before_effects_reads_with_none() {
    let mut body = Body::default();
    body.vitals.health = 4.0;
    let entries = body.encode();
    assert!(entries.iter().all(|entry| entry.id != body_ids::EFFECTS));
    let read = Body::decode(&entries).expect("readable");
    assert!(read.effects.active.is_empty());
}

#[test]
fn an_effect_s_end_survives_encoding_and_decoding() {
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 1, 3);
    run.add(Effect::SPEED, 0, 7);
    run.add(Effect::INVISIBILITY, 0, -1);
    let entries = run.mob.body.encode();
    let effects: effects::Effects = part::decode(&entries).expect("readable");
    assert_eq!(effects, run.mob.body.effects);
    let read = Body::decode(&entries).expect("readable");
    assert_eq!(read, run.mob.body);
    // Read back, it ends where it would have.
    run.mob.body = read;
    for _ in 0..7 {
        assert!(run.mob.body.effects.has(Effect::SPEED));
        run.tick();
    }
    assert!(!run.mob.body.effects.has(Effect::SPEED));
}

#[test]
fn milk_takes_every_effect_off_with_its_modifier() {
    let mut run = Run::new(COW, blocks::AIR);
    run.add(Effect::SPEED, 0, 100);
    run.add(Effect::INVISIBILITY, 0, 100);
    let kind = run.kind;
    assert!(effects::remove_all(&mut run.mob, kind));
    assert!(run.mob.body.effects.active.is_empty());
    assert_eq!(run.speed(), base_speed(COW));
    assert!(
        !effects::remove_all(&mut run.mob, kind),
        "nothing left to take"
    );
}

#[test]
fn effects_are_named_by_the_game_s_registry_ids() {
    assert_eq!(Effect::named("minecraft:speed"), Some(Effect::SPEED));
    assert_eq!(
        Effect::named("minecraft:fire_resistance"),
        Some(Effect::FIRE_RESISTANCE)
    );
    assert_eq!(Effect::INVISIBILITY.name(), Some("minecraft:invisibility"));
    assert_eq!(
        Effect::named("minecraft:breath_of_the_nautilus"),
        Some(Effect(39))
    );
    assert_eq!(Effect(40).name(), None);
}
