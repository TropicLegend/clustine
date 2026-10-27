//! The attacks of part S6 from the game's rules (`docs/groundwork/n9-plan.md`, section
//! 8.4; n7 `ZombieAttackGoal`, `MeleeAttackGoal`, `MoveThroughVillageGoal`,
//! `Vindicator`; n7k `Drowned` and its goals, `Creeper`, `Spider$SpiderAttackGoal`,
//! `CaveSpider`): each type's goals by priority, the zombie's raised arms, a zombie
//! chasing and hitting a villager, the drowned's trident raised and thrown, the
//! creeper's chase of what hurt it, the spider's light test and its draw of a hundred,
//! the vindicator's hold of its ground and its hit, and the draws each makes.

use std::collections::BTreeMap;

use clustine_data::{BlockState, EntityTypeInfo, attributes, blocks, entity_types as types};
use clustine_data::{items, sounds};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack};

use super::super::MobType;
use super::super::drowned::{self, DrownedAttackGoal, TridentAttackGoal};
use super::super::illagers;
use super::super::spider::SpiderAttackGoal;
use super::super::vindicator::BreakDoorGoal;
use super::{MoveThroughVillageGoal, ZombieAttackGoal, ZombieAttacking};
use crate::api::Hand;
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Attacking;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::{Ai, Setup, rotate_body, server_ai_step};
use crate::mobs::body::Event;
use crate::mobs::body::using::USING;
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{self, Vec3, World};
use crate::region::Random;

/// The key of the mob ticked, even: its selectors' full ticks are its even tick counts.
const KEY: EntityId = EntityId(40);

/// A survival player, whom the monsters take as a target.
const PLAYER: EntityId = EntityId(9);

/// Another mob: a villager, or what hurt a creeper.
const OTHER: EntityId = EntityId(101);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// Whether the selectors' tick at `lived` is a full one: `(tickCount + id) % 2 == 0`,
/// the id even, or the first tick.
fn full(lived: u32) -> bool {
    let tick_count = lived + 1;
    tick_count.is_multiple_of(2) || tick_count <= 1
}

/// Flat land: stone below y = 0, air above, what a test sets, the players and entities
/// it puts there, one light everywhere and a sky darkened by `darken` (by day, 0).
struct Land {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
    light: Option<LightAt>,
    darken: i32,
}

impl Land {
    fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
            players: Vec::new(),
            entities: Vec::new(),
            light: None,
            darken: 0,
        }
    }

    /// At night: the sky darkened by 11.
    fn at_night() -> Self {
        Self {
            darken: 11,
            ..Self::new()
        }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert((x, y, z), state);
    }
}

impl World for Land {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < 0 => blocks::STONE,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Land {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Land {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        self.entities
            .iter()
            .filter(|(_, own)| own.intersects(*bounds))
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
            .collect()
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        self.entities
            .iter()
            .find(|(entity, _)| entity.key == key)
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.players.clone()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        self.light
    }

    fn sky_darken(&self) -> i32 {
        self.darken
    }
}

/// A survival player standing at `feet`.
fn survivor(feet: Vec3) -> SeenPlayer {
    SeenPlayer {
        key: PLAYER,
        uuid: 9,
        position: feet,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: None,
        off_hand: None,
        sneaking: false,
        creative: false,
        spectator: false,
        vehicle: None,
        moved_horizontally: false,
        sprinting: false,
        alive: true,
        sleeping: None,
        last_hurt_mob: None,
        last_hurt_by: None,
    }
}

/// A living mob of the type `id` standing at `feet`, its box its type's.
fn other(key: EntityId, id: i32, feet: Vec3) -> (Entity, Aabb) {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    let info = EntityTypeInfo::of(id);
    let entity = Entity {
        key,
        kind: kind(id),
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
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
    (entity, Aabb::of_entity(feet, info.width, info.height))
}

/// The body of a mob of the type `id` at `feet`, on the ground, at rest, its fluids as
/// the land has them.
fn body_of(land: &Land, id: i32, feet: Vec3) -> Living {
    let info = EntityTypeInfo::of(id);
    let body = Body {
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
            kind: kind(id),
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living.fluids = physics::fluids::update(land, &living.body.bounds());
    living.body.in_water = living.fluids.in_water();
    living
}

/// A type whose goals a test gives, for counting one goal's draws alone.
struct Only(fn(&mut Setup));

impl MobType for Only {
    fn register_goals(&self, setup: &mut Setup) {
        (self.0)(setup);
    }
}

/// One mob ticked again and again on the land, as a region would tick it.
struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    mob_type: &'static dyn MobType,
}

impl Run {
    /// A mob of the type `id` at `feet`, of its type's goals, at its full health.
    fn new(land: &Land, id: i32, feet: Vec3) -> Self {
        Self::of(land, id, feet, crate::mobs::types::of(kind(id)))
    }

    /// A mob of the type `id` at `feet` of the goals of `mob_type`.
    fn of(land: &Land, id: i32, feet: Vec3, mob_type: &'static dyn MobType) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = supplied(id, attributes::MAX_HEALTH) as f32;
        mob.body.vitals.air = MAX_AIR;
        Self {
            kind: kind(id),
            mob,
            living: body_of(land, id, feet),
            yaw: 0.0,
            pitch: 0.0,
            lived: 0,
            mob_type,
        }
    }

    /// Its stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(7, KEY, self.lived)
    }

    fn tick<'w>(&self, land: &'w Land, draws: &'w mut Random) -> MobTick<'w> {
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
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: self.mob_type,
            world: land,
            draws,
            out: MobOutput::default(),
        }
    }

    /// One AI step and its body's turn, standing where it is: the draws it made and
    /// what it wrote and showed.
    fn step(&mut self, land: &Land) -> (u64, MobOutput) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(land, &mut draws);
        server_ai_step(&mut tick);
        rotate_body(&mut tick);
        self.keep(tick.mob, tick.living, tick.yaw, tick.pitch);
        let out = tick.out;
        (draws.draws_since(&start), out)
    }

    /// One whole tick of the mob, as the region runs it: it walks.
    fn whole_tick(&mut self, land: &Land) -> MobOutput {
        let mut draws = self.stream();
        let mut tick = self.tick(land, &mut draws);
        crate::mobs::tick::tick(&mut tick);
        self.keep(tick.mob, tick.living, tick.yaw, tick.pitch);
        tick.out
    }

    fn keep(&mut self, mob: Mob, living: Living, yaw: f32, pitch: f32) {
        self.mob = mob;
        self.living = living;
        self.living.body.old_position = self.living.body.position;
        self.yaw = yaw;
        self.pitch = pitch;
        self.lived += 1;
    }

    /// `f` run on the mob as its next AI step would have it, `carries` whether something
    /// rides it; the draws `f` made with what it gave.
    fn with_ai<R>(
        &self,
        land: &Land,
        carries: bool,
        f: impl FnOnce(&mut Ai<'_, '_>) -> R,
    ) -> (u64, R, Mob) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(land, &mut draws);
        tick.carries = carries;
        let mut setup = Setup::default();
        self.mob_type.register_goals(&mut setup);
        let config = setup.config;
        let mut ai = Ai {
            tick: &mut tick,
            config: &config,
            sensing: Default::default(),
            navigation_ticked: false,
        };
        let given = f(&mut ai);
        let mob = tick.mob;
        (draws.draws_since(&start), given, mob)
    }

    fn position(&self) -> Vec3 {
        self.living.body.position
    }
}

/// The base of the attribute `attribute` of the type `id`.
fn supplied(id: i32, attribute: i32) -> f64 {
    crate::mobs::body::attributes::supplied(kind(id), crate::mobs::body::attribute(attribute))
        .expect("the type has the attribute")
}

/// The hits a tick wrote: whom, of what damage type and amount, and the hit's stamp.
/// A hit as [`strikes`] reads it.
type Struck = (EntityId, String, f32, Option<(EntityId, u32)>);

fn strikes(out: &MobOutput) -> Vec<Struck> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Strike { key, hurt } => {
                Some((*key, hurt.damage_type.clone(), hurt.amount.0, hurt.hit))
            }
            _ => None,
        })
        .collect()
}

fn swung(out: &MobOutput) -> bool {
    out.local.iter().any(|local| matches!(local, Local::Swing))
}

/// The goals of the type `id`: each one's priority, flags and whether it is ticked every
/// tick, in the order added.
fn goals_of(id: i32) -> Vec<(i32, u8, bool)> {
    let mut setup = Setup::default();
    crate::mobs::types::of(kind(id)).register_goals(&mut setup);
    setup
        .goals
        .iter()
        .map(|goal| (goal.priority, goal.slot.flags(), goal.slot.every_tick()))
        .collect()
}

const MOVE_LOOK: u8 = flags::MOVE | flags::LOOK;

#[test]
fn each_type_registers_its_goals_at_the_game_s_priorities_with_the_attacks_real() {
    let priorities = |id: i32| -> Vec<i32> { goals_of(id).iter().map(|goal| goal.0).collect() };
    // `Zombie.registerGoals`, then `addBehaviourGoals`: the turtle egg, the looks, the
    // spear, the attack, the village, the stroll.
    for id in [types::ZOMBIE, types::HUSK, types::ZOMBIE_VILLAGER] {
        assert_eq!(priorities(id), [4, 8, 8, 2, 3, 6, 7], "{id}");
        let goals = goals_of(id);
        // The spear's slot holds its flags; the attack is the melee goal, every tick.
        assert_eq!(goals[3], (2, MOVE_LOOK, false));
        assert_eq!(goals[4], (3, MOVE_LOOK, true));
        assert_eq!(goals[5], (6, flags::MOVE, false));
    }
    // The drowned's `addBehaviourGoals`: to water, the trident, the attack, the beach,
    // up, the stroll.
    assert_eq!(priorities(types::DROWNED), [4, 8, 8, 1, 2, 2, 5, 6, 7]);
    let drowned = goals_of(types::DROWNED);
    assert_eq!(drowned[4], (2, MOVE_LOOK, true));
    assert_eq!(drowned[5], (2, MOVE_LOOK, true));
    // The creeper: float, swell, two of avoiding, the attack, the stroll, the looks.
    assert_eq!(priorities(types::CREEPER), [1, 2, 3, 3, 4, 5, 6, 6]);
    assert_eq!(goals_of(types::CREEPER)[4], (4, MOVE_LOOK, true));
    // The spiders: float, the armadillo, the leap, the attack, the stroll, the looks.
    for id in [types::SPIDER, types::CAVE_SPIDER] {
        assert_eq!(priorities(id), [1, 2, 3, 4, 5, 6, 6], "{id}");
        assert_eq!(goals_of(id)[3], (4, MOVE_LOOK, true));
    }
    // The vindicator: the raider's five, then float, the creaking, the doors, its hold
    // of its ground, the attack, the stroll, the looks.
    assert_eq!(
        priorities(types::VINDICATOR),
        [4, 1, 3, 4, 5, 0, 1, 2, 3, 4, 5, 8, 9, 10]
    );
    let vindicator = goals_of(types::VINDICATOR);
    assert_eq!(vindicator[7], (2, flags::MOVE, false));
    assert_eq!(vindicator[9], (4, MOVE_LOOK, true));
    assert_eq!(vindicator[10], (5, MOVE_LOOK, true));
}

#[test]
fn a_zombie_s_arms_are_raised_from_the_fifth_tick_of_its_attack_while_no_hit_is_near() {
    // A player out of reach, and the zombie standing still: no hit comes, its cooldown
    // stays at nothing.
    let mut land = Land::at_night();
    land.players.push(survivor([6.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::ZOMBIE, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let mut raised = Vec::new();
    for _ in 0..8 {
        let (_, out) = run.step(&land);
        assert!(strikes(&out).is_empty());
        raised.push(run.mob.body.combat.aggressive);
    }
    // `start` raises them and the same tick's `tick` lowers them: down for four ticks.
    assert_eq!(raised, [false, false, false, false, true, true, true, true]);
}

#[test]
fn a_zombie_lowers_its_arms_at_a_hit_and_raises_them_once_the_next_is_under_ten_ticks_away() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::ZOMBIE, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let mut seen = Vec::new();
    for _ in 0..24 {
        let lived = run.lived;
        let (_, out) = run.step(&land);
        let hit = !strikes(&out).is_empty();
        seen.push((lived, hit, run.mob.body.combat.aggressive));
    }
    let hits: Vec<u32> = seen.iter().filter(|s| s.1).map(|s| s.0).collect();
    // A hit at once, and the next 20 ticks on.
    assert_eq!(hits, [0, 20]);
    for (lived, _, aggressive) in seen {
        // The cooldown left after the tick: 20 at a hit, a tick less each tick after.
        let cooldown = 20 - (lived % 20) as i32;
        let expected = lived + 1 >= 5 && cooldown < 10;
        assert_eq!(aggressive, expected, "at {lived}");
    }
}

#[test]
fn a_zombie_chases_a_villager_and_hits_it_swinging_its_arm() {
    let mut land = Land::at_night();
    land.entities
        .push(other(OTHER, types::VILLAGER, [8.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::ZOMBIE, [0.5, 0.0, 0.5]);
    let start = run.position();
    let mut hit = None;
    for _ in 0..200 {
        let lived = run.lived;
        let out = run.whole_tick(&land);
        if !strikes(&out).is_empty() {
            hit = Some((lived, out));
            break;
        }
    }
    let (lived, out) = hit.expect("the zombie reaches the villager and hits it");
    assert_eq!(run.mob.body.combat.target, Some(OTHER));
    assert!(run.position()[0] - start[0] > 5.0, "it walked to it");
    // The swing comes before the hurt, as `swingForAttack` before `doHurtTarget`.
    let swing = out
        .local
        .iter()
        .position(|local| matches!(local, Local::Swing))
        .expect("its arm swings");
    let strike = out
        .local
        .iter()
        .position(|local| matches!(local, Local::Strike { .. }))
        .expect("its hit");
    assert!(swing < strike);
    // A mob's attack of the zombie's damage, stamped with the zombie and its tick.
    let damage = supplied(types::ZOMBIE, attributes::ATTACK_DAMAGE) as f32;
    assert_eq!(
        strikes(&out),
        [(
            OTHER,
            "minecraft:mob_attack".to_owned(),
            damage,
            Some((KEY, lived))
        )]
    );
}

#[test]
fn move_through_village_and_the_vindicator_s_door_goal_never_start_and_draw_nothing() {
    for land in [Land::new(), Land::at_night()] {
        let run = Run::new(&land, types::ZOMBIE, [0.5, 0.0, 0.5]);
        let (drawn, used, _) = run.with_ai(&land, false, |ai| {
            MoveThroughVillageGoal.can_use(&mut (), ai)
        });
        assert_eq!((drawn, used), (0, false));
        let run = Run::new(&land, types::VINDICATOR, [0.5, 0.0, 0.5]);
        let (drawn, used, _) = run.with_ai(&land, false, |ai| BreakDoorGoal.can_use(&mut (), ai));
        assert_eq!((drawn, used), (0, false));
    }
}

#[test]
fn a_zombie_holding_a_spear_attacks_as_one_with_a_sword() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::ZOMBIE, [0.5, 0.0, 0.5]);
    run.mob.body.equipment.slots[0] = Some(ItemStack::new(items::IRON_SPEAR, 1));
    run.mob.body.combat.target = Some(PLAYER);
    let (_, out) = run.step(&land);
    assert!(swung(&out));
    assert_eq!(strikes(&out).len(), 1);
}

/// The drowned's trident goal alone.
static TRIDENT_ONLY: Only = Only(|setup: &mut Setup| {
    setup.add_goal(2, TridentAttackGoal);
});

#[test]
fn a_drowned_raises_its_trident_and_throws_one_every_forty_ticks_drawing_only_its_sound() {
    let mut land = Land::at_night();
    let target = [6.5, 0.0, 0.5];
    land.players.push(survivor(target));
    let mut run = Run::of(&land, types::DROWNED, [0.5, 0.0, 0.5], &TRIDENT_ONLY);
    let trident = ItemStack::new(items::TRIDENT, 1);
    run.mob.body.equipment.slots[0] = Some(trident.clone());
    run.mob.body.combat.target = Some(PLAYER);
    let mut thrown = Vec::new();
    for _ in 0..90 {
        let lived = run.lived;
        let mut replay = run.stream();
        let (drawn, out) = run.step(&land);
        // Raised from its start, and never let go.
        let using = run.mob.body.using.using.expect("the trident is raised");
        assert_eq!((using.hand, using.item), (Hand::Main, items::TRIDENT));
        assert_eq!(run.mob.body.using.flags & USING, USING);
        assert!(run.mob.body.combat.aggressive);
        let shots: Vec<_> = out
            .local
            .iter()
            .filter_map(|local| match local {
                Local::Shoot(shot) => Some(shot.clone()),
                _ => None,
            })
            .collect();
        if shots.is_empty() {
            assert_eq!(drawn, 0, "nothing drawn between throws, at {lived}");
            continue;
        }
        thrown.push(lived);
        let [shot] = shots.as_slice() else {
            panic!("one trident a throw");
        };
        assert_eq!(shot.kind, kind(types::TRIDENT));
        assert_eq!(shot.stack, trident);
        assert_eq!(shot.weapon, None);
        assert_eq!(
            (shot.power, shot.uncertainty, shot.from_mob),
            (1.6, 10.0, None)
        );
        // From its eyes less a tenth, at a third of the player's height and a fifth of
        // the way across above it.
        let eye = f64::from(EntityTypeInfo::of(types::DROWNED).eye_height);
        assert_eq!(shot.at, [0.5, eye - 0.1, 0.5]);
        let up = f64::from(1.8_f32) * 0.333_333_333_333_333_3 - (eye - 0.1);
        let lift = 6.0 * 0.200_000_002_980_232_24;
        assert_eq!(shot.direction[0], 6.0);
        assert_eq!(shot.direction[1], up + lift);
        assert_eq!(shot.direction[2], 0.0);
        // Its sound, of its one draw.
        assert_eq!(drawn, 1);
        let pitch = 1.0 / (replay.next_float() * 0.4 + 0.8);
        assert!(out.events.contains(&Event::Sound {
            sound: sounds::ENTITY_DROWNED_SHOOT,
            volume: 1.0,
            pitch,
        }));
        // The trident in its hand stays.
        assert_eq!(run.mob.body.equipment.slots[0], Some(trident.clone()));
    }
    // Its first interval is set at its first tick (40, the lerp of 40 and 40), and
    // counted down from the next.
    assert_eq!(thrown, [40, 80]);
}

#[test]
fn a_drowned_lowers_its_trident_when_its_target_is_gone() {
    let mut land = Land::at_night();
    land.players.push(survivor([6.5, 0.0, 0.5]));
    let mut run = Run::of(&land, types::DROWNED, [0.5, 0.0, 0.5], &TRIDENT_ONLY);
    run.mob.body.equipment.slots[0] = Some(ItemStack::new(items::TRIDENT, 1));
    run.mob.body.combat.target = Some(PLAYER);
    run.step(&land);
    assert!(drowned_using(&run));
    // The player leaves; the next full tick stops the goal.
    land.players.clear();
    run.step(&land);
    run.step(&land);
    assert!(!drowned_using(&run));
    assert_eq!(run.mob.body.using.flags & USING, 0);
    assert!(!run.mob.body.combat.aggressive);
}

fn drowned_using(run: &Run) -> bool {
    crate::mobs::body::using::is_using(&run.mob)
}

#[test]
fn a_drowned_without_a_trident_chases_and_hits_by_its_zombie_s_attack_and_never_raises_one() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::DROWNED, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let (_, out) = run.step(&land);
    assert!(swung(&out));
    assert_eq!(strikes(&out).len(), 1);
    assert!(!drowned_using(&run));
    // With a trident the trident's goal, first of the two at 2, takes the target.
    let mut run = Run::new(&land, types::DROWNED, [0.5, 0.0, 0.5]);
    run.mob.body.equipment.slots[0] = Some(ItemStack::new(items::TRIDENT, 1));
    run.mob.body.combat.target = Some(PLAYER);
    let (_, out) = run.step(&land);
    assert!(strikes(&out).is_empty());
    assert!(drowned_using(&run));
}

#[test]
fn a_drowned_attacks_by_day_only_a_target_in_water() {
    let mut land = Land::new();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let attack = DrownedAttackGoal(ZombieAttackGoal::new(1.0, false));
    let asked = |land: &Land| {
        let mut run = Run::new(land, types::DROWNED, [0.5, 0.0, 0.5]);
        run.mob.body.combat.target = Some(PLAYER);
        let (_, used, _) = run.with_ai(land, false, |ai| {
            attack.can_use(&mut ZombieAttacking::default(), ai)
        });
        used
    };
    assert!(!asked(&land), "by day, on land");
    land.set(1, 0, 0, blocks::WATER);
    assert!(asked(&land), "by day, in water");
    land.blocks.clear();
    land.darken = 11;
    assert!(asked(&land), "by night, anything");
}

#[test]
fn a_drowned_in_water_swims_after_a_target_in_water_and_not_after_one_on_land() {
    let mut land = Land::new();
    for x in -3..=8 {
        for y in 0..4 {
            land.set(x, y, 0, blocks::WATER);
        }
    }
    let swims = |land: &Land, target: Vec3| {
        let mut land_with = Land {
            blocks: land.blocks.clone(),
            ..Land::new()
        };
        land_with.players.push(survivor(target));
        let mut run = Run::new(&land_with, types::DROWNED, [0.5, 0.5, 0.5]);
        run.mob.body.combat.target = Some(PLAYER);
        let mut draws = run.stream();
        let mut tick = run.tick(&land_with, &mut draws);
        drowned::Drowned.after_base_tick(&mut tick, MAX_AIR);
        tick.mob.body.swimming.swimming
    };
    assert!(swims(&land, [6.5, 0.5, 0.5]), "its target in water");
    assert!(!swims(&land, [6.5, 0.0, 9.5]), "its target on land");
}

#[test]
fn a_creeper_hurt_by_a_mob_chases_it() {
    let mut land = Land::at_night();
    land.entities
        .push(other(OTHER, types::SKELETON, [8.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::CREEPER, [0.5, 0.0, 0.5]);
    run.mob.body.combat.last_hurt_by = Some((OTHER, 0));
    run.step(&land);
    assert_eq!(run.mob.body.combat.target, Some(OTHER));
    assert!(run.mob.body.combat.aggressive, "its melee goal runs");
    let path = run.mob.body.path.followed.as_ref().expect("a path to it");
    assert_eq!(path.target, BlockPos::new(8, 0, 0));
    // Walking, it comes to it.
    let start = run.position();
    for _ in 0..60 {
        run.whole_tick(&land);
    }
    assert!(run.position()[0] - start[0] > 4.0, "{:?}", run.position());
}

#[test]
fn a_creeper_s_hit_does_nothing_but_its_arm_swings() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::of(&land, types::CREEPER, [0.5, 0.0, 0.5], &CREEPER_MELEE_ONLY);
    run.mob.body.combat.target = Some(PLAYER);
    let (_, out) = run.step(&land);
    assert!(swung(&out));
    assert!(strikes(&out).is_empty());
}

/// A creeper's melee goal alone, its type's hit the creeper's.
static CREEPER_MELEE_ONLY: CreeperMelee = CreeperMelee;

struct CreeperMelee;

impl MobType for CreeperMelee {
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(
            4,
            crate::mobs::ai::goals::MeleeAttackGoal {
                speed: 1.0,
                following_even_if_not_seen: false,
            },
        );
    }

    fn do_hurt_target(&self, ai: &mut Ai<'_, '_>, target: &crate::mobs::ai::sensing::Looked) {
        super::super::creeper::Creeper.do_hurt_target(ai, target);
    }
}

#[test]
fn a_spider_in_light_lets_its_target_go_one_time_in_a_hundred_by_a_draw_asked_first() {
    let goal = SpiderAttackGoal;
    let mut land = Land::new();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    // Full daylight at its eyes: worth a half and more.
    land.light = Some(LightAt { sky: 15, block: 0 });
    let mut run = Run::new(&land, types::SPIDER, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let mut kept = 0;
    let mut dropped = None;
    for lived in 0..2000 {
        run.lived = lived;
        let draw = run.stream().next_int(100);
        let (drawn, goes_on, mob) = run.with_ai(&land, false, |ai| {
            goal.can_continue(&mut Attacking::default(), ai)
        });
        // The light's draw first; the melee goal's own test draws nothing.
        assert_eq!(drawn, 1);
        if draw == 0 {
            assert!(!goes_on);
            assert_eq!(mob.body.combat.target, None, "the target let go");
            dropped = Some(lived);
            break;
        }
        assert!(goes_on, "at {lived}");
        assert_eq!(mob.body.combat.target, Some(PLAYER));
        kept += 1;
    }
    assert!(dropped.is_some() && kept > 0, "{dropped:?} after {kept}");
    // In light worth less than a half it draws nothing, and goes on.
    land.light = Some(LightAt { sky: 7, block: 0 });
    let (drawn, goes_on, _) = run.with_ai(&land, false, |ai| {
        goal.can_continue(&mut Attacking::default(), ai)
    });
    assert_eq!((drawn, goes_on), (0, true));
}

#[test]
fn a_spider_s_own_goals_let_its_target_go_in_light_but_never_in_the_dark() {
    // The spider's registered attack, by its type: in light, at a full tick in which
    // its draw of a hundred is 0, its target is gone; in the dark, never.
    let mut lit = Land::new();
    lit.players.push(survivor([1.5, 0.0, 0.5]));
    lit.light = Some(LightAt { sky: 15, block: 0 });
    let mut dark = Land::at_night();
    dark.players.push(survivor([1.5, 0.0, 0.5]));
    let mut dropped = None;
    for (name, land) in [("lit", &lit), ("dark", &dark)] {
        let mut run = Run::new(land, types::SPIDER, [0.5, 0.0, 0.5]);
        run.mob.body.combat.target = Some(PLAYER);
        run.step(land);
        assert!(run.mob.body.combat.aggressive, "{name}: it attacks");
        for _ in 0..1000 {
            let lived = run.lived;
            run.step(land);
            if run.mob.body.combat.target.is_none() {
                dropped = Some((name, lived));
                break;
            }
        }
        if name == "lit" {
            let (_, lived) = dropped.expect("let go in light");
            assert!(full(lived), "asked at a full tick");
            assert!(!run.mob.body.combat.aggressive, "its attack stopped");
            dropped = None;
        }
    }
    assert_eq!(dropped, None, "never in the dark");
}

#[test]
fn a_spider_with_a_rider_does_not_attack_but_leaps_and_bites_without_one() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let run = Run::new(&land, types::SPIDER, [0.5, 0.0, 0.5]);
    let mut run = run;
    run.mob.body.combat.target = Some(PLAYER);
    let (_, used, _) = run.with_ai(&land, true, |ai| {
        SpiderAttackGoal.can_use(&mut Attacking::default(), ai)
    });
    assert!(!used, "a vehicle");
    let (_, used, _) = run.with_ai(&land, false, |ai| {
        SpiderAttackGoal.can_use(&mut Attacking::default(), ai)
    });
    assert!(used);
    // Its bite, of a spider's damage; the cave spider's poisons nothing at easy.
    for id in [types::SPIDER, types::CAVE_SPIDER] {
        let mut run = Run::new(&land, id, [0.5, 0.0, 0.5]);
        run.mob.body.combat.target = Some(PLAYER);
        let (_, out) = run.step(&land);
        assert!(swung(&out), "{id}");
        let hurts: Vec<_> = out
            .local
            .iter()
            .filter_map(|local| match local {
                Local::Strike { hurt, .. } => Some(hurt.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(hurts.len(), 1, "{id}");
        assert!(hurts[0].effects.is_empty() && hurts[0].extras.is_empty());
    }
}

#[test]
fn a_spider_s_chase_follows_a_target_it_cannot_see() {
    assert!(SpiderAttackGoal.flags() == MOVE_LOOK && SpiderAttackGoal.every_tick());
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let run = Run::new(&land, types::SPIDER, [0.5, 0.0, 0.5]);
    let mut run = run;
    run.mob.body.combat.target = Some(PLAYER);
    // `followingTargetEvenIfNotSeen`: it goes on while the player is within its home,
    // with no path at all, where a zombie's attack, which follows only its path, stops.
    let (_, goes_on, _) = run.with_ai(&land, false, |ai| {
        SpiderAttackGoal.can_continue(&mut Attacking::default(), ai)
    });
    assert!(goes_on);
}

#[test]
fn a_patrolling_vindicator_holds_its_ground_then_attacks() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::VINDICATOR, [0.5, 0.0, 0.5]);
    run.mob.body.equipment.slots[0] = Some(ItemStack::new(items::IRON_AXE, 1));
    illagers::update(&mut run.mob, |own| own.patrolling = true);
    run.mob.body.combat.target = Some(PLAYER);
    // Its hold of its ground first, at 4: the player within its radius of 10, it is
    // aggressive and hits nothing.
    let (_, out) = run.step(&land);
    assert!(strikes(&out).is_empty());
    assert!(run.mob.body.combat.aggressive);
    // Aggressive, the hold stops at the next full tick and the attack starts.
    let mut hit = None;
    for _ in 0..6 {
        let lived = run.lived;
        let (_, out) = run.step(&land);
        if !strikes(&out).is_empty() {
            hit = Some((lived, out));
            break;
        }
    }
    let (lived, out) = hit.expect("the vindicator hits the player");
    assert!(full(lived));
    assert!(swung(&out));
    let damage = supplied(types::VINDICATOR, attributes::ATTACK_DAMAGE) as f32;
    assert_eq!(strikes(&out)[0].2, damage);
}

#[test]
fn a_vindicator_not_patrolling_attacks_at_once() {
    let mut land = Land::at_night();
    land.players.push(survivor([1.5, 0.0, 0.5]));
    let mut run = Run::new(&land, types::VINDICATOR, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let (_, out) = run.step(&land);
    assert!(swung(&out));
    assert_eq!(strikes(&out).len(), 1);
}
