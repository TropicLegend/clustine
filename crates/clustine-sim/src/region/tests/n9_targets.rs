//! The monsters' target goals, tested from the plan (`docs/groundwork/n9-plan.md`,
//! sections 8.1 and 14, finding 29) and the brief's 1.1 and 1.2 by someone who did not
//! write them: what each type's target goals draw of its own stream on each tick the
//! target selector asks them (the interval goals one `nextInt(5)` before any search, the
//! enderman's `EndermanLookForPlayerGoal`, the phantom's, the vex's copy of its owner's
//! and `HurtByTargetGoal` nothing); what they find (no creative player; a villager, an
//! iron golem, a baby turtle on land and not in water, as the table says); and
//! `alertOthers`, by the exact class of the hurt mob, also across a pinned line.

use clustine_data::{EntityTypeInfo, blocks, entity_types};
use clustine_world::{Causer, Double, Effected, Entity, EntityChange, EntityUse, Float, HurtBy};

use super::n9_world::*;
use super::thrown::lay;
use super::*;
use crate::api::WorldClock;
use crate::living::{Kind as LivingKind, Living};
use crate::mobs::Mob;
use crate::mobs::ai::tests::Plot;
use crate::mobs::ai::{Setup, server_ai_step};
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::types::MobType;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::region::random_ticks::Random;

// --- What the goals draw ------------------------------------------------------------

/// A type with only the target goals of the type `kind`, as its `registerGoals` adds
/// them, and none of its other goals: so that what a step draws is theirs alone.
struct TargetsOf(u16);

impl MobType for TargetsOf {
    fn register_goals(&self, setup: &mut Setup) {
        let mut whole = Setup::default();
        crate::mobs::types::of(self.0).register_goals(&mut whole);
        setup.targets = whole.targets;
        setup.config = whole.config;
    }
}

const fn of(kind: i32) -> u16 {
    kind as u16
}

static ZOMBIE: TargetsOf = TargetsOf(of(entity_types::ZOMBIE));
static HUSK: TargetsOf = TargetsOf(of(entity_types::HUSK));
static ZOMBIE_VILLAGER: TargetsOf = TargetsOf(of(entity_types::ZOMBIE_VILLAGER));
static DROWNED: TargetsOf = TargetsOf(of(entity_types::DROWNED));
static SKELETON: TargetsOf = TargetsOf(of(entity_types::SKELETON));
static CREEPER: TargetsOf = TargetsOf(of(entity_types::CREEPER));
static WITCH: TargetsOf = TargetsOf(of(entity_types::WITCH));
static VINDICATOR: TargetsOf = TargetsOf(of(entity_types::VINDICATOR));
static PILLAGER: TargetsOf = TargetsOf(of(entity_types::PILLAGER));
static EVOKER: TargetsOf = TargetsOf(of(entity_types::EVOKER));
static GUARDIAN: TargetsOf = TargetsOf(of(entity_types::GUARDIAN));
static ELDER_GUARDIAN: TargetsOf = TargetsOf(of(entity_types::ELDER_GUARDIAN));
static ENDERMAN: TargetsOf = TargetsOf(of(entity_types::ENDERMAN));
static PHANTOM: TargetsOf = TargetsOf(of(entity_types::PHANTOM));
static VEX: TargetsOf = TargetsOf(of(entity_types::VEX));
static SLIME: TargetsOf = TargetsOf(of(entity_types::SLIME));
static SILVERFISH: TargetsOf = TargetsOf(of(entity_types::SILVERFISH));
static ENDERMITE: TargetsOf = TargetsOf(of(entity_types::ENDERMITE));

/// The key of the mob stepped: even, so that the selectors' full ticks are those of an
/// even tick count.
const STEPPED: EntityId = EntityId(42);

/// A body of the type `kind` standing at `feet` on the plot's ground.
fn body(kind: u16, feet: [f64; 3]) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let body = Moving {
        position: feet,
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: feet,
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
            key: STEPPED,
            root: STEPPED,
            like_a_vehicle: false,
            happy_ghast: false,
        },
        steps: None,
    };
    let mut living = Living::new(
        body,
        LivingKind::Mob {
            kind,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = crate::living::MAX_AIR;
    living
}

/// The draws of each of `steps` AI steps of a mob of `kind` with only its target goals
/// (`types`), alone on an empty plot: nobody and nothing to find.
fn draws_alone(kind: i32, types: &'static TargetsOf, steps: u32) -> Vec<u64> {
    let plot = Plot::flat();
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    let mut living = body(of(kind), [0.5, 0.0, 0.5]);
    let mut drawn = Vec::new();
    for lived in 0..steps {
        let mut draws = Random::of_entity(SEED, STEPPED, lived);
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: None,
            controlled: None,
            controlled_by_mob: false,
            ridden: Default::default(),
            key: STEPPED,
            kind: of(kind),
            lived,
            tick_count: lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: mob.clone(),
            living: living.clone(),
            yaw: 0.0,
            pitch: 0.0,
            mob_type: types,
            world: &plot,
            draws: &mut draws,
            out: MobOutput::default(),
        };
        server_ai_step(&mut tick);
        assert_eq!(tick.mob.body.combat.target, None, "nothing to find");
        mob = tick.mob;
        living = tick.living;
        drawn.push(draws.draws_since(&start));
    }
    drawn
}

/// Holds that a mob of `kind` draws `each` on every tick its target selector asks its
/// goals and nothing between, the selector asking every other tick (`Mob.serverAiStep`:
/// `(tickCount + id) % 2 == 0`, and the first ticks).
fn draws_each_asked_tick(kind: i32, types: &'static TargetsOf, each: u64) {
    let drawn = draws_alone(kind, types, 12);
    let asked = drawn.iter().filter(|draws| **draws > 0).count();
    if each == 0 {
        assert!(drawn.iter().all(|draws| *draws == 0), "{kind}: {drawn:?}");
        return;
    }
    assert!(
        drawn.iter().all(|draws| *draws == 0 || *draws == each),
        "{kind}: {each} on each asked tick: {drawn:?}"
    );
    assert!(
        (6..=7).contains(&asked),
        "{kind}: asked every other tick: {drawn:?}"
    );
    assert!(
        drawn.windows(2).all(|pair| pair[0] > 0 || pair[1] > 0),
        "{kind}: never two ticks unasked: {drawn:?}"
    );
}

/// Section 8.1 and M9 0.4: the five types whose placeholders drew nothing (the zombie
/// family, the drowned, the illagers, the witch, the guardians) now draw, each
/// `NearestAttackableTargetGoal` with an interval one `nextInt(5)` on each asked tick,
/// before its search; `HurtByTargetGoal` and the vindicator's Johnny goal nothing; the
/// witch's healer a `nextBoolean()` outside a raid. Counted from the brief's table 1.2.
#[test]
fn the_interval_goals_of_the_five_types_draw_once_each_on_every_asked_tick() {
    // Player, villager, golem, baby turtle.
    draws_each_asked_tick(entity_types::ZOMBIE, &ZOMBIE, 4);
    draws_each_asked_tick(entity_types::HUSK, &HUSK, 4);
    draws_each_asked_tick(entity_types::ZOMBIE_VILLAGER, &ZOMBIE_VILLAGER, 4);
    // Player, villager, golem, axolotl, baby turtle.
    draws_each_asked_tick(entity_types::DROWNED, &DROWNED, 5);
    // Player, villager, golem; the Johnny goal draws nothing.
    draws_each_asked_tick(entity_types::VINDICATOR, &VINDICATOR, 3);
    draws_each_asked_tick(entity_types::PILLAGER, &PILLAGER, 3);
    draws_each_asked_tick(entity_types::EVOKER, &EVOKER, 3);
    // The healer's boolean, then the player goal's int.
    draws_each_asked_tick(entity_types::WITCH, &WITCH, 2);
    // `NearestAttackableTargetGoal(LivingEntity, 10, ...)`.
    draws_each_asked_tick(entity_types::GUARDIAN, &GUARDIAN, 1);
    draws_each_asked_tick(entity_types::ELDER_GUARDIAN, &ELDER_GUARDIAN, 1);
}

/// The types that drew before N9 draw as before: the skeletons' player, golem and baby
/// turtle goals, the creeper's player goal.
#[test]
fn the_skeleton_s_and_the_creeper_s_interval_goals_draw_on_every_asked_tick() {
    draws_each_asked_tick(entity_types::SKELETON, &SKELETON, 3);
    draws_each_asked_tick(entity_types::CREEPER, &CREEPER, 1);
}

/// Finding 29 and the N9f table: not every target goal draws. The enderman's
/// `EndermanLookForPlayerGoal` searches without a draw (n9 `Enderman$
/// EndermanLookForPlayerGoal.canUse`), so of its goals only the endermite's
/// `NearestAttackableTargetGoal` draws; the phantom's goal counts down and draws
/// nothing; the vex's `VexCopyOwnerTargetGoal` draws nothing, its player goal one; the
/// slime's player and golem goals, the silverfish's and the endermite's player goals one
/// each.
#[test]
fn the_enderman_s_the_phantom_s_and_the_vex_s_own_goals_draw_nothing() {
    draws_each_asked_tick(entity_types::ENDERMAN, &ENDERMAN, 1);
    draws_each_asked_tick(entity_types::PHANTOM, &PHANTOM, 0);
    draws_each_asked_tick(entity_types::VEX, &VEX, 1);
    draws_each_asked_tick(entity_types::SLIME, &SLIME, 2);
    draws_each_asked_tick(entity_types::SILVERFISH, &SILVERFISH, 1);
    draws_each_asked_tick(entity_types::ENDERMITE, &ENDERMITE, 1);
}

// --- What they find, in a region ----------------------------------------------------

/// The night: the undead do not burn, and the drowned's `okTarget` takes anything.
const NIGHT: WorldClock = WorldClock {
    game_time: 18_000,
    day: 18_000,
};

/// A mob of the type `kind` at `feet`, of full health, with AI where `ai`.
fn mob_record(key: EntityId, kind: i32, feet: [f64; 3], ai: bool) -> Entity {
    let mut entity = super::thrown::cow(key, feet);
    entity.kind = of(kind);
    let mut mob = Mob::decode(&entity.data).expect("a mob's record");
    mob.body.flags.no_ai = !ai;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        of(kind),
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .unwrap_or(20.0) as f32;
    entity.data = mob.encode();
    entity
}

/// A baby turtle at `feet`, with no AI.
fn baby_turtle(key: EntityId, feet: [f64; 3]) -> Entity {
    let mut entity = mob_record(key, entity_types::TURTLE, feet, false);
    let mut mob = Mob::decode(&entity.data).expect("a mob's record");
    let animal = crate::mobs::types::animal::Animal {
        age: -24_000,
        ..Default::default()
    };
    mob.state = crate::mobs::types::animal::write(
        &animal,
        &crate::mobs::types::turtle::TurtleState::default(),
    );
    entity.data = mob.encode();
    entity
}

fn target_of(region: &Region, key: EntityId) -> Option<EntityId> {
    super::thrown::mob(region, key).body.combat.target
}

const HUNTER: EntityId = EntityId(0x7_0000_0001);
const PREY: EntityId = EntityId(0x7_0000_0002);

/// The target a zombie with AI at (0.5, -60, 0.5) holds after `ticks` ticks of a night
/// with `prey` laid six blocks east of it, and player 1 standing beside it where
/// `player`.
fn zombie_hunts(
    prey: Option<Entity>,
    player: bool,
    water_at: Option<BlockPos>,
) -> Option<EntityId> {
    let mut region = alone(0, NIGHT);
    if player {
        enter_with(
            &mut region,
            1,
            entering_place(Vec3::new(2.5, -60.0, 0.5), Vec::new(), 0),
        );
    }
    if let Some(at) = water_at {
        let (x, z) = at.in_chunk();
        region
            .chunks
            .get_mut(&at.chunk())
            .expect("loaded")
            .set(x, at.y, z, blocks::WATER);
    }
    lay(
        &mut region,
        &mob_record(HUNTER, entity_types::ZOMBIE, [0.5, -60.0, 0.5], true),
    );
    if let Some(prey) = prey {
        lay(&mut region, &prey);
    }
    for _ in 0..200 {
        region.tick(&TickInputs::default());
        if target_of(&region, HUNTER).is_some() {
            break;
        }
    }
    target_of(&region, HUNTER)
}

/// Section 8.1 and the brief's 1.2: a zombie's goals find no creative player (finding
/// 1's three locks) however long it stands beside one, and find a villager (no need to
/// see it), an iron golem and a baby turtle on land; not a baby turtle in water, nor a
/// grown one (`BABY_ON_LAND_SELECTOR`).
#[test]
fn a_zombie_finds_a_villager_a_golem_and_a_baby_turtle_on_land_and_no_creative_player() {
    let feet = [6.5, -60.0, 0.5];
    assert_eq!(zombie_hunts(None, true, None), None, "no creative player");
    assert_eq!(
        zombie_hunts(
            Some(mob_record(PREY, entity_types::VILLAGER, feet, false)),
            true,
            None
        ),
        Some(PREY),
        "a villager, the player beside it notwithstanding"
    );
    assert_eq!(
        zombie_hunts(
            Some(mob_record(PREY, entity_types::IRON_GOLEM, feet, false)),
            false,
            None
        ),
        Some(PREY),
        "an iron golem"
    );
    assert_eq!(
        zombie_hunts(Some(baby_turtle(PREY, feet)), false, None),
        Some(PREY),
        "a baby turtle on land"
    );
    assert_eq!(
        zombie_hunts(
            Some(baby_turtle(PREY, feet)),
            false,
            Some(BlockPos::new(6, -60, 0))
        ),
        None,
        "a baby turtle in water"
    );
    assert_eq!(
        zombie_hunts(
            Some(mob_record(PREY, entity_types::TURTLE, feet, false)),
            false,
            None
        ),
        None,
        "a grown turtle"
    );
}

// --- alertOthers ----------------------------------------------------------------------

/// The skeleton that hurts the alerting mob.
const ATTACKER: EntityId = EntityId(0x7_0000_0100);

/// A skeleton's arrow hurt of 1 on the mob, which it takes, setting the skeleton as
/// what last hurt it.
fn hurt_by_the_skeleton() -> EntityUse {
    let skeleton = Causer {
        key: ATTACKER,
        kind: of(entity_types::SKELETON),
        position: [Double(10.5), Double(-60.0), Double(6.5)],
        player: None,
        living: true,
    };
    EntityUse::Hurt(Box::new(HurtBy {
        damage_type: "minecraft:mob_attack".to_owned(),
        cause: Some(skeleton.clone()),
        direct: Some(skeleton),
        position: None,
        projectile_motion: None,
        amount: Float(1.0),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    }))
}

/// Section 8.1 (finding 28) and the brief's 1.1: a hurt zombie alerts every mob of its
/// class (`instanceof Zombie`: zombies, husks, zombie villagers, drowned) in
/// `unitCube(position).inflate(follow, 10, follow)` that has no target; not a skeleton.
/// Those in the region are given the target at once; one in a copy across the pinned
/// line is sent the word, which its holder applies by the same tests. A hurt husk alerts
/// husks alone, here and across the line.
#[test]
fn a_hurt_zombie_alerts_its_class_by_exact_class_here_and_across_a_pinned_line() {
    for (alerter, comes, stays) in [
        (
            entity_types::ZOMBIE,
            vec![
                entity_types::HUSK,
                entity_types::DROWNED,
                entity_types::ZOMBIE_VILLAGER,
                entity_types::ZOMBIE,
            ],
            vec![entity_types::SKELETON, entity_types::SPIDER],
        ),
        (
            entity_types::HUSK,
            vec![entity_types::HUSK],
            vec![
                entity_types::ZOMBIE,
                entity_types::DROWNED,
                entity_types::ZOMBIE_VILLAGER,
            ],
        ),
    ] {
        let mut west = west(100, NIGHT);
        let mut east = east(100, NIGHT);
        let hurt = EntityId(0x7_0000_0200);
        lay(
            &mut west,
            &mob_record(hurt, alerter, [10.5, -60.0, 0.5], true),
        );
        lay(
            &mut west,
            &mob_record(ATTACKER, entity_types::SKELETON, [10.5, -60.0, 6.5], false),
        );
        // One of each kind on either side of the line, eight blocks or less from the
        // hurt mob, standing still.
        let mut here = Vec::new();
        let mut there = Vec::new();
        for (index, kind) in comes.iter().chain(&stays).enumerate() {
            let z = -6.5 + index as f64 * 2.0;
            let west_key = EntityId(0x7_0000_0300 + index as u64);
            lay(
                &mut west,
                &mob_record(west_key, *kind, [6.5, -60.0, z], false),
            );
            here.push((west_key, *kind));
            let east_key = EntityId(0x7_0000_0400 + index as u64);
            let record = mob_record(east_key, *kind, [18.5, -60.0, z], false);
            lay(&mut east, &record);
            west.copies
                .get_mut(&record.chunk())
                .expect("the column east of the line is copied")
                .apply_entity(&EntityChange::Whole(record));
            there.push((east_key, *kind));
        }
        let mut output = TickOutput::default();
        west.entity_used(hurt, hurt_by_the_skeleton(), &mut output);
        let mut outputs = Vec::new();
        for _ in 0..3 {
            outputs.push(west.tick(&TickInputs::default()));
        }
        assert_eq!(target_of(&west, hurt), Some(ATTACKER), "{alerter}");
        for (key, kind) in &here {
            let expected = comes.contains(kind).then_some(ATTACKER);
            assert_eq!(
                target_of(&west, *key),
                expected,
                "{alerter} alerting {kind} here"
            );
        }
        let sent: Vec<clustine_world::Effect> = outputs
            .iter()
            .flat_map(|output| output.effects.iter().cloned())
            .filter(|effect| {
                matches!(
                    &effect.what,
                    Effected::OnEntity {
                        by: EntityUse::Alert { .. },
                        ..
                    }
                )
            })
            .collect();
        east.tick(&TickInputs {
            effects: filed(sent, 1),
            ..TickInputs::default()
        });
        for (key, kind) in &there {
            let expected = comes.contains(kind).then_some(ATTACKER);
            assert_eq!(
                target_of(&east, *key),
                expected,
                "{alerter} alerting {kind} across the line"
            );
        }
    }
}
