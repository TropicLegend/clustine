//! The monsters' target goals from the game's rules (`docs/groundwork/n9-plan.md`,
//! section 8.1; the brief's 1.1 and 1.2): each type's goals in their priority order, what
//! each draws and when, the spider's light test before its draw, the drowned's
//! `okTarget`, and `alertOthers` as the alerting mob's tick makes it.

use clustine_data::{blocks, entity_types as types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::super::tests::{Built, Plot, cow_body, player};
use super::super::{Ai, Setup, server_ai_step};
use super::{Among, Gate, NearestTarget, Target};
use crate::living::Living;
use crate::mobs::Mob;
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::MobType;
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;

/// The key of the mob ticked, even.
const KEY: EntityId = EntityId(40);

/// Whether the selectors' tick of the mob at `lived` is a full one, which asks the goals
/// not running: `(tickCount + id) % 2 == 0`, the id even, and the first tick's.
fn full(lived: u32) -> bool {
    let tick_count = lived + 1;
    tick_count.is_multiple_of(2) || tick_count <= 1
}

/// The survival player who hurts the mobs.
const HITTER: EntityId = EntityId(9);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// A plot whose every block has the light `light` and whose sky is darkened by `darken`.
struct Lit {
    plot: Plot,
    light: Option<LightAt>,
    darken: i32,
}

impl Lit {
    fn new() -> Self {
        Self {
            plot: Plot::flat(),
            light: None,
            darken: 0,
        }
    }
}

impl World for Lit {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        self.plot.block(pos)
    }

    fn has_chunk(&self, chunk: ChunkPos) -> bool {
        self.plot.has_chunk(chunk)
    }
}

impl Holds for Lit {
    fn may_hold(&self, chunk: ChunkPos) -> bool {
        self.plot.may_hold(chunk)
    }

    fn min_y(&self) -> i32 {
        self.plot.min_y()
    }
}

impl MobWorld for Lit {
    fn is_loaded(&self, pos: BlockPos) -> bool {
        self.plot.is_loaded(pos)
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        self.plot.entities_in(bounds)
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        self.plot.entity(key)
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.plot.players()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        self.light
    }

    fn sky_darken(&self) -> i32 {
        self.darken
    }
}

/// A player in survival standing at `feet`.
fn survivor(key: EntityId, feet: Vec3) -> SeenPlayer {
    SeenPlayer {
        creative: false,
        ..player(key.0, feet, None)
    }
}

/// A mob of the type `id` standing at `feet`, holding `target`.
fn monster(key: u64, id: i32, feet: Vec3, target: Option<EntityId>) -> (Entity, Aabb) {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.combat.target = target;
    let entity = Entity {
        key: EntityId(key),
        kind: kind(id),
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position: feet,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    };
    (entity, Aabb::of_entity(feet, 0.6, 1.95))
}

/// One mob of the type `kind` ticked again and again on a plot.
struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    mob_type: &'static dyn MobType,
    seed: u64,
}

impl Run {
    fn new(world: &Lit, id: i32, mob_type: &'static dyn MobType) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 20.0;
        mob.body.vitals.air = crate::living::MAX_AIR;
        Self {
            kind: kind(id),
            mob,
            living: cow_body(world, [0.5, 0.0, 0.5]),
            lived: 0,
            mob_type,
            seed: 7,
        }
    }

    /// Its stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(self.seed, KEY, self.lived)
    }

    fn tick<'w>(&self, world: &'w Lit, draws: &'w mut Random) -> MobTick<'w> {
        MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: 0.0,
            pitch: 0.0,
            mob_type: self.mob_type,
            world,
            draws,
            out: MobOutput::default(),
        }
    }

    /// One AI step: the draws it made and what it wrote to others.
    fn step(&mut self, world: &Lit) -> (u64, Vec<Local>) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(world, &mut draws);
        server_ai_step(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        let locals = tick.out.local;
        self.lived += 1;
        (draws.draws_since(&start), locals)
    }

    /// `f` asked of the mob as its next AI step would have it.
    fn with_ai<R>(&self, world: &Lit, f: impl FnOnce(&mut Ai<'_, '_>) -> R) -> R {
        let mut draws = self.stream();
        let mut tick = self.tick(world, &mut draws);
        let mut setup = Setup::default();
        self.mob_type.register_goals(&mut setup);
        let config = setup.config;
        let mut ai = Ai {
            tick: &mut tick,
            config: &config,
            sensing: Default::default(),
            navigation_ticked: false,
        };
        f(&mut ai)
    }

    /// Hurt by the hitter just before its tick.
    fn hurt_by_hitter(&mut self) {
        self.mob.body.combat.last_hurt_by = Some((HITTER, self.lived));
    }
}

fn priorities(id: i32) -> Vec<i32> {
    let mut setup = Setup::default();
    crate::mobs::types::of(kind(id)).register_goals(&mut setup);
    setup
        .targets
        .iter()
        .map(|registered| registered.priority)
        .collect()
}

fn players_goal(target: &Target, must_see: bool) -> bool {
    matches!(target, Target::Nearest(goal)
        if goal.among == Among::Players && goal.interval == 10 && goal.must_see == must_see)
}

fn mobs_goal(target: &Target, kinds: &'static [u16], must_see: bool) -> bool {
    matches!(target, Target::Nearest(goal)
        if goal.among == Among::Mobs(super::Class::Kinds(kinds))
            && goal.interval == 10
            && goal.must_see == must_see)
}

fn hurt_by(target: &Target, ignore: &[u16], alert: bool) -> bool {
    matches!(target, Target::HurtBy { ignore: own, alert: own_alert }
        if *own == ignore && *own_alert == alert)
}

#[test]
fn each_type_registers_its_target_goals_in_the_game_s_priority_order() {
    for id in [types::ZOMBIE, types::HUSK, types::ZOMBIE_VILLAGER] {
        assert_eq!(priorities(id), [1, 2, 3, 3, 5], "{id}");
    }
    assert_eq!(priorities(types::DROWNED), [1, 2, 3, 3, 3, 5]);
    for id in [types::SKELETON, types::STRAY, types::BOGGED, types::PARCHED] {
        assert_eq!(priorities(id), [1, 2, 3, 3], "{id}");
    }
    assert_eq!(priorities(types::CREEPER), [1, 2]);
    assert_eq!(priorities(types::SPIDER), [1, 2, 3]);
    assert_eq!(priorities(types::CAVE_SPIDER), [1, 2, 3]);
    assert_eq!(priorities(types::WITCH), [1, 2, 3]);
    assert_eq!(priorities(types::VINDICATOR), [1, 2, 3, 3, 4]);
    assert_eq!(priorities(types::PILLAGER), [1, 2, 3, 3]);
    assert_eq!(priorities(types::EVOKER), [1, 2, 3, 3]);
    assert_eq!(priorities(types::GUARDIAN), [1]);
    assert_eq!(priorities(types::ELDER_GUARDIAN), [1]);
}

#[test]
fn each_type_s_goals_are_the_game_s_classes_with_their_selectors() {
    use super::{
        CREEPER, DROWNED, DROWNED_TARGETS, EVOKER, GUARDIAN, IRON_GOLEMS, PILLAGER, RAIDERS,
        SKELETON, SPIDER, TURTLES, VILLAGERS, VINDICATOR, WITCH, ZOMBIE,
    };
    let zombie: Vec<&Target> = ZOMBIE.iter().map(|(_, goal)| goal).collect();
    assert!(hurt_by(zombie[0], &[], true));
    assert!(players_goal(zombie[1], true));
    assert!(mobs_goal(zombie[2], VILLAGERS, false));
    assert!(mobs_goal(zombie[3], IRON_GOLEMS, true));
    assert!(mobs_goal(zombie[4], TURTLES, true));
    assert!(matches!(zombie[4], Target::Nearest(goal) if goal.selector.is_some()));

    let drowned: Vec<&Target> = DROWNED_TARGETS.iter().map(|(_, goal)| goal).collect();
    assert!(hurt_by(drowned[0], DROWNED, true));
    assert!(players_goal(drowned[1], true));
    assert!(matches!(drowned[1], Target::Nearest(goal) if goal.selector.is_some()));
    assert!(mobs_goal(drowned[4], super::AXOLOTLS, true));

    assert!(hurt_by(&SKELETON[0].1, &[], false));
    assert!(mobs_goal(&SKELETON[3].1, TURTLES, true));
    assert!(players_goal(&CREEPER[0].1, true));
    assert!(hurt_by(&CREEPER[1].1, &[], false));

    assert!(hurt_by(&SPIDER[0].1, &[], false));
    for (_, goal) in &SPIDER[1..] {
        assert!(matches!(goal, Target::Nearest(goal) if goal.gate == Gate::InTheDark));
    }

    assert!(hurt_by(&WITCH[0].1, RAIDERS, false));
    assert!(matches!(WITCH[1].1, Target::HealableRaider));
    assert!(players_goal(&WITCH[2].1, true));

    // The pillager alerts too: its dump calls `setAlertOthers()` (Pillager.txt 166-170).
    for table in [VINDICATOR, PILLAGER, EVOKER] {
        assert!(hurt_by(&table[0].1, RAIDERS, true));
        assert!(players_goal(&table[1].1, true));
    }
    assert!(matches!(VINDICATOR[2].1, Target::Nearest(goal) if goal.must_see));
    assert!(matches!(PILLAGER[2].1, Target::Nearest(goal) if !goal.must_see));
    let Target::Nearest(johnny) = VINDICATOR[4].1 else {
        panic!("Johnny's goal");
    };
    assert_eq!(
        (
            johnny.interval,
            johnny.must_see,
            johnny.must_reach,
            johnny.gate
        ),
        (0, true, true, Gate::Johnny)
    );
    for (_, goal) in &EVOKER[1..3] {
        assert!(matches!(goal, Target::Nearest(goal) if goal.unseen_memory == 300));
    }
    assert!(matches!(EVOKER[3].1, Target::Nearest(goal) if !goal.must_see));

    let Target::Nearest(guardian) = GUARDIAN[0].1 else {
        panic!("the guardian's goal");
    };
    assert!(matches!(guardian.among, Among::Living(_)));
    assert_eq!((guardian.interval, guardian.must_see), (10, true));
}

static PLAYERS: Built = Built(|setup: &mut Setup| {
    setup.add_target(1, NearestTarget::players(true));
});

#[test]
fn a_nearest_attackable_target_goal_draws_next_int_five_once_per_asked_tick_before_its_search() {
    let mut world = Lit::new();
    world.plot.players.push(survivor(HITTER, [3.5, 0.0, 0.5]));
    let mut run = Run::new(&world, types::ZOMBIE, &PLAYERS);
    let mut passed_over = 0;
    loop {
        assert!(run.lived < 200, "the draw is 0 one time in five");
        let asked = full(run.lived);
        let draw = run.stream().next_int(5);
        let (drawn, _) = run.step(&world);
        if !asked {
            // Between the selector's full ticks it asks nothing of a goal not running.
            assert_eq!(drawn, 0);
            continue;
        }
        assert_eq!(drawn, 1, "one draw, of five");
        if draw == 0 {
            assert_eq!(run.mob.body.combat.target, Some(HITTER));
            break;
        }
        // A draw but 0 searches for nothing, the player in plain sight.
        assert_eq!(run.mob.body.combat.target, None);
        passed_over += 1;
    }
    assert!(
        passed_over > 0,
        "the seed passes over the player at least once"
    );
    // Running, it draws nothing more.
    assert_eq!(run.step(&world).0, 0);
    assert_eq!(run.step(&world).0, 0);
}

static ZOMBIE_TARGETS: Built = Built(|setup: &mut Setup| {
    super::register(setup, super::ZOMBIE);
});

static HURT_BY_ONLY: Built = Built(|setup: &mut Setup| {
    super::register(setup, &super::ZOMBIE[..1]);
});

static JOHNNY: Built = Built(|setup: &mut Setup| {
    super::register(setup, &super::VINDICATOR[4..]);
});

#[test]
fn hurt_by_target_goal_and_the_johnny_goal_draw_nothing() {
    let mut world = Lit::new();
    world.plot.players.push(survivor(HITTER, [3.5, 0.0, 0.5]));
    let mut run = Run::new(&world, types::ZOMBIE, &HURT_BY_ONLY);
    run.hurt_by_hitter();
    let (drawn, locals) = run.step(&world);
    assert_eq!(drawn, 0);
    assert_eq!(run.mob.body.combat.target, Some(HITTER));
    assert!(locals.is_empty(), "nobody to alert");
    // A vindicator not named Johnny asks nothing more of its goal, of the interval 0.
    let mut vindicator = Run::new(&world, types::VINDICATOR, &JOHNNY);
    for _ in 0..10 {
        assert_eq!(vindicator.step(&world).0, 0);
    }
    assert_eq!(vindicator.mob.body.combat.target, None);
}

static SPIDER_PLAYERS: Built = Built(|setup: &mut Setup| {
    super::register(setup, &super::SPIDER[1..2]);
});

#[test]
fn a_spider_asks_for_a_target_only_in_the_dark_and_draws_after_its_light_test() {
    let mut world = Lit::new();
    // Full daylight at its eyes: worth 1, a half and more.
    world.light = Some(LightAt { sky: 15, block: 0 });
    let mut run = Run::new(&world, types::SPIDER, &SPIDER_PLAYERS);
    for _ in 0..7 {
        assert_eq!(run.step(&world).0, 0);
    }
    // Sky light 8 darkened by 4 is worth 4 / 15, below a half: it draws.
    world.darken = 4;
    world.light = Some(LightAt { sky: 8, block: 0 });
    assert!(full(run.lived));
    assert_eq!(run.step(&world).0, 1);
    assert_eq!(run.step(&world).0, 0);
    assert_eq!(run.step(&world).0, 1);
}

static WITCH_TARGETS: Built = Built(|setup: &mut Setup| {
    super::register(setup, &super::WITCH[1..]);
});

#[test]
fn a_witch_draws_a_boolean_for_its_healer_then_an_int_for_players_each_asked_tick() {
    let mut world = Lit::new();
    world.plot.players.push(survivor(HITTER, [3.5, 0.0, 0.5]));
    let mut run = Run::new(&world, types::WITCH, &WITCH_TARGETS);
    loop {
        assert!(run.lived < 200, "the draw is 0 one time in five");
        let mut replay = run.stream();
        let _healer = replay.next_bool();
        let draw = replay.next_int(5);
        let asked = full(run.lived);
        let (drawn, _) = run.step(&world);
        if !asked {
            assert_eq!(drawn, 0);
            continue;
        }
        // The healer's boolean, whatever it is, and the player goal's int after it.
        assert_eq!(drawn, 2);
        if draw == 0 {
            assert_eq!(run.mob.body.combat.target, Some(HITTER));
            break;
        }
        assert_eq!(run.mob.body.combat.target, None);
    }
}

#[test]
fn a_drowned_takes_by_day_only_what_is_in_water_and_by_night_anything() {
    let mut world = Lit::new();
    let on_land = survivor(EntityId(1), [3.5, 0.0, 0.5]);
    let in_water = survivor(EntityId(2), [6.5, 0.0, 0.5]);
    world.plot.set(6, 0, 0, blocks::WATER);
    let run = Run::new(&world, types::DROWNED, &ZOMBIE_TARGETS);
    let ok = |world: &Lit, seen: &SeenPlayer| {
        run.with_ai(world, |ai| {
            super::drowned_ok_target(ai, &super::Looked::of_player(seen))
        })
    };
    assert!(!ok(&world, &on_land), "by day, not on land");
    assert!(ok(&world, &in_water), "by day, in water");
    world.darken = 4;
    assert!(ok(&world, &on_land), "by night, anything");
}

/// The alerts of `locals`: to whom, of what attacker and type, of what class.
fn alerts(locals: &[Local]) -> Vec<(EntityId, EntityId, u16, u16)> {
    locals
        .iter()
        .filter_map(|local| match *local {
            Local::Alert {
                key,
                attacker,
                kind,
                class,
            } => Some((key, attacker, kind, class)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_zombie_hurt_by_a_player_alerts_its_class_in_the_box_but_one_with_a_target_or_ignored() {
    let mut world = Lit::new();
    world.plot.players.push(survivor(HITTER, [3.5, 0.0, 0.5]));
    let at = |x: f64, y: f64, z: f64| [x, y, z];
    world.plot.entities = vec![
        monster(101, types::ZOMBIE, at(5.5, 0.0, 0.5), None),
        monster(102, types::HUSK, at(-5.5, 0.0, 0.5), None),
        monster(103, types::DROWNED, at(0.5, 0.0, 8.5), None),
        monster(104, types::ZOMBIE_VILLAGER, at(0.5, 0.0, -30.5), None),
        // One with a target of its own.
        monster(105, types::ZOMBIE, at(2.5, 0.0, 2.5), Some(EntityId(77))),
        // A zombified piglin is a zombie, of a class the zombie's alert ignores.
        monster(106, types::ZOMBIFIED_PIGLIN, at(2.5, 0.0, -2.5), None),
        // Not a zombie.
        monster(107, types::SKELETON, at(-2.5, 0.0, 2.5), None),
        // Beyond the follow range of 35 across, and beyond 10 above the unit cube.
        monster(108, types::ZOMBIE, at(37.5, 0.0, 0.5), None),
        monster(109, types::ZOMBIE, at(0.5, 11.5, 0.5), None),
    ];
    let mut run = Run::new(&world, types::ZOMBIE, &ZOMBIE_TARGETS);
    run.hurt_by_hitter();
    let (_, locals) = run.step(&world);
    assert_eq!(run.mob.body.combat.target, Some(HITTER));
    let player = kind(types::PLAYER);
    let zombie = kind(types::ZOMBIE);
    assert_eq!(
        alerts(&locals),
        [101, 102, 103, 104].map(|key| (EntityId(key), HITTER, player, zombie)),
        "in the view's order"
    );
}

#[test]
fn a_husk_alerts_only_husks() {
    let mut world = Lit::new();
    world.plot.players.push(survivor(HITTER, [3.5, 0.0, 0.5]));
    world.plot.entities = vec![
        monster(101, types::ZOMBIE, [5.5, 0.0, 0.5], None),
        monster(102, types::HUSK, [-5.5, 0.0, 0.5], None),
        monster(103, types::DROWNED, [0.5, 0.0, 8.5], None),
    ];
    let mut run = Run::new(&world, types::HUSK, &ZOMBIE_TARGETS);
    run.hurt_by_hitter();
    let (_, locals) = run.step(&world);
    assert_eq!(
        alerts(&locals),
        [(
            EntityId(102),
            HITTER,
            kind(types::PLAYER),
            kind(types::HUSK)
        )]
    );
}

#[test]
fn only_the_alerting_class_comes_and_never_one_it_ignores() {
    let zombie = kind(types::ZOMBIE);
    for id in [
        types::ZOMBIE,
        types::HUSK,
        types::ZOMBIE_VILLAGER,
        types::DROWNED,
    ] {
        assert!(super::comes_to_alert(zombie, kind(id)), "{id}");
    }
    assert!(!super::comes_to_alert(
        zombie,
        kind(types::ZOMBIFIED_PIGLIN)
    ));
    assert!(!super::comes_to_alert(zombie, kind(types::SKELETON)));
    let husk = kind(types::HUSK);
    assert!(super::comes_to_alert(husk, husk));
    assert!(!super::comes_to_alert(husk, zombie));
    let drowned = kind(types::DROWNED);
    assert!(!super::comes_to_alert(drowned, zombie));
    let vindicator = kind(types::VINDICATOR);
    assert!(super::comes_to_alert(vindicator, vindicator));
    assert!(!super::comes_to_alert(vindicator, kind(types::PILLAGER)));
}
