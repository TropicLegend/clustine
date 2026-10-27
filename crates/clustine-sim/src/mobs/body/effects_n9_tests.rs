//! A mob's effects from `docs/groundwork/n9-plan.md`, section 5.2 and the game's dumps
//! (n9 `PoisonMobEffect`, `WitherMobEffect`, `RegenerationMobEffect`,
//! `HealOrHarmMobEffect`, `HungerMobEffect`, `MobEffect`, `MobEffectInstance.tickServer`;
//! n9 `LivingEntity`'s `tickEffects`, `canBeAffected`, `getDamageAfterMagicAbsorb`,
//! `actuallyHurt`, `getJumpBoostPower`, `getEffectiveGravity`, `travelInAir`,
//! `updateSynchronizedMobEffectParticles`; n7k `MobEffects` for the modifiers, colours
//! and particles): each rule on a mob with its period and its numbers, the refusals by
//! type, and what clients are shown.

use clustine_data::{BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, EntityId, Entry};

use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::body::effects::{self, Effect, Effects, Layer};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{Body, attribute};
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
use crate::shown::{DataShown, ShownParticle, index, mob_shared_flags, mob_values};

const COW: u16 = entity_types::COW as u16;
const ZOMBIE: u16 = entity_types::ZOMBIE as u16;
const KEY: EntityId = EntityId(40);
/// The moment of the first tick the tests run.
const START: u32 = 100;

/// A floor of stone at y below 0 and air above, lit and at night.
struct Field;

impl World for Field {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(if pos.y < 0 {
            blocks::STONE
        } else {
            blocks::AIR
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Field {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Field {
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

/// A body of `kind` at rest at `y` (on the floor at 0), its first tick behind it.
fn living(kind: u16, y: f64) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let position = [0.5, y, 0.5];
    let body = Moving {
        position,
        motion: [0.0; 3],
        on_ground: y == 0.0,
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
    crate::mobs::body::attributes::supplied(kind, attribute(ids::MAX_HEALTH))
        .expect("a mob has health") as f32
}

/// A mob without AI ticking in the field, its body carried from one tick to the next as a
/// region carries it in its record (its body still travels here, as the region alone
/// holds a mob without AI still).
struct Run {
    world: Field,
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    stream: Random,
}

impl Run {
    fn new(kind: u16) -> Self {
        Self::at(kind, 0.0)
    }

    fn at(kind: u16, y: f64) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = max_health(kind);
        mob.body.vitals.air = MAX_AIR;
        mob.body.flags.no_ai = true;
        Self {
            world: Field,
            kind,
            mob,
            living: living(kind, y),
            lived: START,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

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

    fn tick(&mut self) {
        self.living.air = self.mob.body.vitals.air;
        self.with(false, tick::tick);
        self.mob.body.vitals.air = self.living.air;
        self.lived += 1;
    }

    fn add(&mut self, effect: Effect, amplifier: u8, duration: i32) -> bool {
        let (kind, now) = (self.kind, self.lived);
        effects::add(&mut self.mob, kind, effect, amplifier, duration, now)
    }

    fn health(&self) -> f32 {
        self.mob.body.vitals.health
    }

    fn value(&self, id: i32) -> f64 {
        self.mob.body.attributes.value(self.kind, attribute(id))
    }

    /// The ticks, counted from 1, at which the health changed over `ticks` ticks.
    fn changes(&mut self, ticks: u32) -> Vec<(u32, f32)> {
        let mut changed = Vec::new();
        for at in 1..=ticks {
            let before = self.health();
            self.tick();
            if self.health().to_bits() != before.to_bits() {
                changed.push((at, self.health() - before));
            }
        }
        changed
    }

    /// `hurtServer` between ticks.
    fn hurt(&mut self, kind: DamageType, amount: f32) -> bool {
        let source = DamageSource::of(kind);
        self.with(false, |tick| {
            crate::mobs::body::hurt_by(tick, &source, amount)
        })
    }
}

#[test]
fn poison_hurts_one_of_magic_every_twenty_five_ticks_counted_from_the_duration_left() {
    let mut run = Run::new(COW);
    run.add(Effect::POISON, 0, 100);
    // Durations 100, 75, 50 and 25 before the count down: the first, 26th, 51st and
    // 76th ticks.
    assert_eq!(
        run.changes(100),
        [(1, -1.0), (26, -1.0), (51, -1.0), (76, -1.0)]
    );
    assert!(!run.mob.body.effects.has(Effect::POISON));
}

#[test]
fn poison_of_amplifier_one_hurts_every_twelve_ticks() {
    let mut run = Run::new(COW);
    run.add(Effect::POISON, 1, 48);
    // `25 >> 1` is 12: durations 48, 36, 24 and 12.
    assert_eq!(
        run.changes(48),
        [(1, -1.0), (13, -1.0), (25, -1.0), (37, -1.0)]
    );
}

#[test]
fn poison_leaves_a_mob_at_one_health() {
    let mut run = Run::new(COW);
    run.mob.body.vitals.health = 2.0;
    run.add(Effect::POISON, 0, 200);
    for _ in 0..200 {
        run.tick();
    }
    assert_eq!(run.health(), 1.0, "no hurt at one health");
}

#[test]
fn wither_hurts_every_forty_ticks_at_any_health_and_can_kill() {
    let mut run = Run::new(COW);
    run.add(Effect::WITHER, 0, 80);
    assert_eq!(run.changes(80), [(1, -1.0), (41, -1.0)]);

    let mut run = Run::new(COW);
    run.mob.body.vitals.health = 1.0;
    run.add(Effect::WITHER, 0, 40);
    run.tick();
    assert!(run.mob.body.vitals.dying(), "wither hurts below one health");
}

#[test]
fn regeneration_heals_one_every_fifty_ticks_below_the_most() {
    let mut run = Run::new(COW);
    run.mob.body.vitals.health = 5.0;
    run.add(Effect::REGENERATION, 0, 100);
    assert_eq!(run.changes(100), [(1, 1.0), (51, 1.0)]);

    // Amplifier 1 every 25, and nothing past the most.
    let mut run = Run::new(COW);
    run.mob.body.vitals.health = max_health(COW) - 1.0;
    run.add(Effect::REGENERATION, 1, 50);
    assert_eq!(run.changes(50), [(1, 1.0)]);
    assert_eq!(run.health(), max_health(COW));
}

#[test]
fn an_endless_effect_s_period_runs_on_the_mob_s_tick_count() {
    // The game's `tickCount` in the first tick is 101 (`lived` 100, counted at the
    // start): poison next hurts at 125, the 25th tick.
    let mut run = Run::new(COW);
    run.add(Effect::POISON, 0, -1);
    let changed = run.changes(60);
    assert_eq!(changed, [(25, -1.0), (50, -1.0)]);
}

#[test]
fn the_periods_are_as_the_dumps_shift_them() {
    let applies = effects::applies_this_tick;
    assert!(applies(Effect::POISON, 25, 0));
    assert!(!applies(Effect::POISON, 24, 0));
    assert!(applies(Effect::POISON, 12, 1));
    assert!(applies(Effect::POISON, 7, 5), "25 >> 5 is 0: every tick");
    assert!(applies(Effect::WITHER, 40, 0));
    assert!(applies(Effect::WITHER, 20, 1));
    assert!(!applies(Effect::WITHER, 25, 0));
    assert!(applies(Effect::REGENERATION, 50, 0));
    assert!(applies(Effect::REGENERATION, 25, 1));
    assert!(!applies(Effect::REGENERATION, 49, 0));
    assert!(applies(Effect::HUNGER, 3, 0));
    assert!(applies(Effect::INSTANT_HEALTH, 1, 0));
    assert!(!applies(Effect::INSTANT_HEALTH, 0, 0));
    assert!(!applies(Effect::SPEED, 20, 0), "most effects have no tick");
}

#[test]
fn hunger_does_nothing_to_a_mob() {
    let mut run = Run::new(COW);
    run.add(Effect::HUNGER, 1, 40);
    assert!(run.changes(40).is_empty());
}

#[test]
fn harming_hurts_six_and_heals_an_undead_and_instant_health_the_other_way() {
    let mut run = Run::new(COW);
    run.add(Effect::INSTANT_DAMAGE, 0, 1);
    run.tick();
    assert_eq!(run.health(), max_health(COW) - 6.0);

    let mut run = Run::new(ZOMBIE);
    run.mob.body.vitals.health = 4.0;
    run.add(Effect::INSTANT_DAMAGE, 1, 1);
    run.tick();
    assert_eq!(run.health(), 12.0, "an undead is healed `4 << 1`");
}

#[test]
fn absorption_gives_four_a_level_which_a_hurt_spends_first_and_the_effect_ends_with_it() {
    let mut run = Run::new(COW);
    run.add(Effect::ABSORPTION, 0, 200);
    assert_eq!(run.mob.body.effects.absorption, 4.0);
    assert_eq!(run.value(ids::MAX_ABSORPTION), 4.0);
    let full = run.health();
    assert!(run.hurt(DamageType::MAGIC, 3.0));
    assert_eq!(run.health(), full, "the absorption took it");
    assert_eq!(run.mob.body.effects.absorption, 1.0);
    // Past the cooldown, a hurt of 3 takes the last 1 and 2 of health.
    for _ in 0..20 {
        run.tick();
    }
    assert!(run.hurt(DamageType::MAGIC, 3.0));
    assert_eq!(run.health(), full - 2.0);
    assert_eq!(run.mob.body.effects.absorption, 0.0);
    // Spent, the effect ends at the next tick, with its most.
    run.tick();
    assert!(!run.mob.body.effects.has(Effect::ABSORPTION));
    assert_eq!(run.value(ids::MAX_ABSORPTION), 0.0);
}

#[test]
fn a_second_absorption_tops_it_up_but_never_lowers_it() {
    let mut run = Run::new(COW);
    run.add(Effect::ABSORPTION, 1, 200);
    assert_eq!(run.mob.body.effects.absorption, 8.0);
    // A weaker one changes nothing in force, and gives no less.
    run.add(Effect::ABSORPTION, 0, 400);
    assert_eq!(run.mob.body.effects.absorption, 8.0);
}

#[test]
fn resistance_takes_a_fifth_a_level_after_the_armour_and_not_from_what_bypasses_it() {
    let mut run = Run::new(COW);
    run.add(Effect::RESISTANCE, 0, 200);
    let full = run.health();
    assert!(run.hurt(DamageType::MAGIC, 5.0));
    assert_eq!(run.health(), full - 4.0, "a fifth taken");

    // Five levels take all of it: no hurt reaches the health.
    let mut run = Run::new(COW);
    run.add(Effect::RESISTANCE, 4, 200);
    run.hurt(DamageType::MAGIC, 5.0);
    assert_eq!(run.health(), full);

    // Out of the world bypasses resistance.
    assert!(DamageType::OUT_OF_WORLD.is("minecraft:bypasses_resistance"));
    let source = DamageSource::of(DamageType::OUT_OF_WORLD);
    let mut held = Effects::default();
    held.add(Effect::RESISTANCE, Layer::new(0, 200, 0), 0);
    assert_eq!(effects::magic_absorbed(&held, &source, 5.0), 5.0);
    let magic = DamageSource::of(DamageType::MAGIC);
    assert_eq!(effects::magic_absorbed(&held, &magic, 5.0), 4.0);
}

#[test]
fn strength_and_weakness_change_the_attack_damage_by_three_and_four_a_level() {
    let mut run = Run::new(ZOMBIE);
    let base = run.value(ids::ATTACK_DAMAGE);
    run.add(Effect::STRENGTH, 1, 100);
    assert_eq!(run.value(ids::ATTACK_DAMAGE), base + 6.0);
    run.add(Effect::WEAKNESS, 0, 100);
    assert_eq!(run.value(ids::ATTACK_DAMAGE), base + 6.0 - 4.0);
    for _ in 0..100 {
        run.tick();
    }
    assert_eq!(
        run.value(ids::ATTACK_DAMAGE),
        base,
        "both taken off at their ends"
    );
}

#[test]
fn slowness_slows_by_fifteen_hundredths_a_level_of_the_total() {
    let mut run = Run::new(COW);
    let base = run.value(ids::MOVEMENT_SPEED);
    run.add(Effect::SLOWNESS, 1, 100);
    let expected = base * (1.0 + -0.150_000_005_960_464_48 * 2.0);
    assert_eq!(run.value(ids::MOVEMENT_SPEED), expected);
}

#[test]
fn health_boost_raises_the_most_and_its_end_lowers_the_health_to_it() {
    let mut run = Run::new(COW);
    run.add(Effect::HEALTH_BOOST, 0, 2);
    assert_eq!(run.value(ids::MAX_HEALTH), f64::from(max_health(COW)) + 4.0);
    assert_eq!(run.health(), max_health(COW), "adding it heals nothing");
    run.mob.body.vitals.health = max_health(COW) + 3.0;
    run.tick();
    run.tick();
    assert!(!run.mob.body.effects.has(Effect::HEALTH_BOOST));
    assert_eq!(run.health(), max_health(COW), "lowered to the most");
}

#[test]
fn jump_boost_adds_a_tenth_a_level_to_the_jump_and_a_block_a_level_to_the_safe_fall() {
    let mut run = Run::new(COW);
    let safe = run.value(ids::SAFE_FALL_DISTANCE);
    assert_eq!(effects::jump_boost_power(&run.mob.body.effects), 0.0);
    run.add(Effect::JUMP_BOOST, 2, 100);
    assert_eq!(effects::jump_boost_power(&run.mob.body.effects), 0.1 * 3.0);
    assert_eq!(run.value(ids::SAFE_FALL_DISTANCE), safe + 3.0);
}

#[test]
fn slow_falling_falls_by_at_most_a_hundredth_and_forgets_the_fall() {
    let mut falling = Run::at(COW, 10.0);
    falling.living.body.fall_distance = 5.0;
    falling.tick();
    let plain = falling.living.body.motion[1];

    let mut slow = Run::at(COW, 10.0);
    slow.living.body.fall_distance = 5.0;
    slow.add(Effect::SLOW_FALLING, 0, 100);
    slow.tick();
    let gravity = |run: &Run| run.value(ids::GRAVITY);
    assert_eq!(plain, -gravity(&falling) * f64::from(0.98_f32));
    assert_eq!(slow.living.body.motion[1], -0.01 * f64::from(0.98_f32));
    assert!(slow.living.body.fall_distance < 1.0, "the fall forgotten");
}

#[test]
fn levitation_lifts_a_mob_towards_a_twentieth_a_level() {
    let mut run = Run::at(COW, 10.0);
    run.add(Effect::LEVITATION, 1, 100);
    run.tick();
    let lift = (0.05 * 2.0 - 0.0) * 0.2;
    assert_eq!(run.living.body.motion[1], lift * f64::from(0.98_f32));
    run.tick();
    assert!(run.living.body.position[1] > 10.0, "it rises");
}

#[test]
fn glowing_sets_the_shared_flag_and_is_the_twenty_fourth_effect() {
    assert_eq!(Effect::GLOWING.name(), Some("minecraft:glowing"));
    assert_eq!(Effect::LEVITATION.name(), Some("minecraft:levitation"));
    let mut run = Run::new(COW);
    run.add(Effect::GLOWING, 0, 1);
    assert_eq!(mob_shared_flags(&run.mob.encode()), 0x40);
    run.tick();
    assert_eq!(mob_shared_flags(&run.mob.encode()), 0);
}

#[test]
fn each_visible_effect_shows_its_particle_and_the_ambience_only_where_all_are_ambient() {
    let shown = |mob: &Mob| {
        let values = mob_values(COW, &mob.encode());
        let particles = values
            .iter()
            .find(|(at, ..)| *at == index::EFFECT_PARTICLES)
            .cloned();
        let ambience = values
            .iter()
            .find(|(at, ..)| *at == index::EFFECT_AMBIENCE)
            .cloned();
        (particles, ambience)
    };
    let mut run = Run::new(COW);
    assert_eq!(
        shown(&run.mob),
        (
            Some((
                index::EFFECT_PARTICLES,
                DataShown::Particles(Vec::new()),
                true
            )),
            Some((index::EFFECT_AMBIENCE, DataShown::Boolean(false), true))
        )
    );
    run.add(Effect::POISON, 0, 100);
    run.add(Effect::OOZING, 0, 100);
    let poison = ShownParticle::EntityEffect {
        colour: (255 << 24) | 8_889_187,
    };
    assert_eq!(
        shown(&run.mob).0,
        Some((
            index::EFFECT_PARTICLES,
            DataShown::Particles(vec![poison, ShownParticle::Simple(62)]),
            false
        ))
    );
    assert_eq!(
        shown(&run.mob).1,
        Some((index::EFFECT_AMBIENCE, DataShown::Boolean(false), true))
    );

    // Ambient levels are fainter, an invisible one shows nothing, and all visible
    // ones ambient set the ambience.
    let mut held = Effects::default();
    let ambient = Layer {
        ambient: true,
        ..Layer::new(0, 100, 0)
    };
    let hidden = Layer {
        visible: false,
        ..Layer::new(0, 100, 0)
    };
    held.add(Effect::SPEED, ambient, 0);
    held.add(Effect::WEAVING, hidden, 0);
    held.add(
        Effect::WIND_CHARGED,
        Layer {
            ambient: true,
            ..hidden
        },
        0,
    );
    assert_eq!(
        effects::particles(&held),
        (
            vec![ShownParticle::EntityEffect {
                colour: (38 << 24) | 3_402_751
            }],
            true
        )
    );
    // The four of N9 and the omens show their own particles.
    assert_eq!(
        Effect::WEAVING.particle(false),
        Some(ShownParticle::Simple(63))
    );
    assert_eq!(
        Effect::WIND_CHARGED.particle(false),
        Some(ShownParticle::Simple(32))
    );
    assert_eq!(
        Effect::INFESTED.particle(true),
        Some(ShownParticle::Simple(40))
    );
    assert_eq!(
        Effect(33).particle(false),
        Some(ShownParticle::Simple(124)),
        "the trial omen's"
    );
}

#[test]
fn each_type_refuses_its_own_effects() {
    use effects::can_be_affected;
    let id = |kind: i32| u16::try_from(kind).expect("a type id");
    for kind in [
        entity_types::SPIDER,
        entity_types::CAVE_SPIDER,
        entity_types::NAUTILUS,
        entity_types::ZOMBIE_NAUTILUS,
    ] {
        assert!(
            !can_be_affected(id(kind), Effect::POISON),
            "{kind} and poison"
        );
        assert!(can_be_affected(id(kind), Effect::WEAKNESS));
    }
    assert!(!can_be_affected(
        id(entity_types::PARCHED),
        Effect::WEAKNESS
    ));
    assert!(!can_be_affected(
        id(entity_types::WITHER_SKELETON),
        Effect::WITHER
    ));
    assert!(can_be_affected(id(entity_types::SKELETON), Effect::WITHER));
    assert!(!can_be_affected(
        id(entity_types::SILVERFISH),
        Effect::INFESTED
    ));
    assert!(!can_be_affected(id(entity_types::SLIME), Effect::OOZING));
    assert!(can_be_affected(id(entity_types::SLIME), Effect::INFESTED));
    for undead in [entity_types::ZOMBIE, entity_types::SKELETON] {
        assert!(!can_be_affected(id(undead), Effect::POISON));
        assert!(!can_be_affected(id(undead), Effect::REGENERATION));
        assert!(can_be_affected(id(undead), Effect::WEAKNESS));
    }
    assert!(can_be_affected(COW, Effect::POISON));
    assert!(can_be_affected(id(entity_types::PLAYER), Effect::POISON));

    // Refused, nothing is added.
    let mut run = Run::new(id(entity_types::SPIDER));
    assert!(!run.add(Effect::POISON, 0, 100));
    assert!(run.mob.body.effects.active.is_empty());
}

#[test]
fn an_effects_part_of_version_two_reads_with_no_absorption_and_the_third_keeps_it() {
    #[derive(serde::Serialize)]
    struct V2 {
        active: Vec<effects::Active>,
    }
    let mut held = Effects::default();
    held.add(Effect::POISON, Layer::new(1, 50, 0), 0);
    let old = Entry {
        id: body_ids::EFFECTS,
        version: 2,
        bytes: postcard::to_allocvec(&V2 {
            active: held.active.clone(),
        })
        .expect("encoded"),
    };
    let read = effects::decode(&[old]).expect("readable");
    assert_eq!(read.active, held.active);
    assert_eq!(read.absorption, 0.0);

    held.absorption = 2.5;
    let body = Body {
        effects: held,
        ..Body::default()
    };
    let entries = body.encode();
    assert_eq!(effects::decode(&entries).expect("readable"), body.effects);
    let as_part: Effects = part::decode(&entries).expect("readable");
    assert_eq!(as_part.absorption, 2.5);
}
