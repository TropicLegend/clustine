//! The ravager from the game's rules (n7 Ravager.txt; `docs/groundwork/n10-plan.md`,
//! decision 10 and section 9.7): its goals and body, its hit with its knockback and the
//! ten ticks it holds it still, the stun a shield gives it and the roar after with its
//! targets and draws, the leaves it breaks in its way, its egg and its saved state.

use std::collections::BTreeMap;

use clustine_data::{
    BlockState, EntityTypeInfo, ItemComponents, PathType, attributes, blocks,
    entity_types as types, items, sounds,
};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, TypeState};

use super::{
    ATTACK_DURATION, Blocked, ROAR_AT, ROAR_DAMAGE, ROAR_DURATION, Ravager, RavagerAttackGoal,
    RavagerState, STUN_DURATION, blocked_by_item, state_of, strong_knockback,
};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing;
use crate::mobs::ai::{Ai, Setup, rotate_body, server_ai_step};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::GameEvent;
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::{MobType, SpawnReason, Spawning, illagers, is_built, template};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{self, Vec3, World};
use crate::region::Random;

/// The ravager ticked, its key even.
const KEY: EntityId = EntityId(40);

/// A survival player, whom it takes as a target.
const PLAYER: EntityId = EntityId(9);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// Flat land: stone below y = 0, air above, what a test sets, and the players and
/// entities it puts there.
struct Land {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
}

impl Land {
    fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
            players: Vec::new(),
            entities: Vec::new(),
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
        None
    }
}

/// A survival player standing at `feet`.
fn survivor(feet: Vec3) -> SeenPlayer {
    SeenPlayer {
        worn: Default::default(),
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

/// A mob of the type `id` at `feet` of `health`, its box its type's.
fn other(key: EntityId, id: i32, feet: Vec3, health: f32) -> (Entity, Aabb) {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health;
    let info = EntityTypeInfo::of(id);
    let entity = Entity {
        portal: Default::default(),
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

/// The ravager's body at `feet`, on the ground and at rest.
fn body_of(land: &Land, feet: Vec3) -> Living {
    let info = EntityTypeInfo::of(types::RAVAGER);
    let body = Body {
        stands_on_lava: false,
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
            kind: kind(types::RAVAGER),
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living.fluids = physics::fluids::update(land, &living.body.bounds());
    living
}

/// The base of the ravager's attribute `attribute`.
fn supplied(attribute: i32) -> f64 {
    crate::mobs::body::attributes::supplied(
        kind(types::RAVAGER),
        crate::mobs::body::attribute(attribute),
    )
    .expect("the ravager has the attribute")
}

/// Its base speed as its record keeps it (its type's until it changes it).
fn base_speed(mob: &Mob) -> f64 {
    mob.body
        .attributes
        .made(crate::mobs::body::attribute(attributes::MOVEMENT_SPEED))
        .map_or(supplied(attributes::MOVEMENT_SPEED), |speed| speed.base)
}

/// A ravager ticked again and again on the land, as a region would tick it.
struct Run {
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    controlled_by_mob: bool,
}

impl Run {
    fn new(land: &Land, feet: Vec3) -> Self {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = supplied(attributes::MAX_HEALTH) as f32;
        mob.body.vitals.air = MAX_AIR;
        Self {
            mob,
            living: body_of(land, feet),
            yaw: 0.0,
            pitch: 0.0,
            lived: 0,
            controlled_by_mob: false,
        }
    }

    fn stream(&self) -> Random {
        Random::of_entity(7, KEY, self.lived)
    }

    fn tick<'w>(&self, land: &'w Land, draws: &'w mut Random) -> MobTick<'w> {
        MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: kind(types::RAVAGER),
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: self.controlled_by_mob,
            leashed: false,
            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: &Ravager,
            world: land,
            draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: self.controlled_by_mob,
        }
    }

    /// One AI step and its body's turn: what it wrote and showed.
    fn step(&mut self, land: &Land) -> MobOutput {
        let mut draws = self.stream();
        let mut tick = self.tick(land, &mut draws);
        server_ai_step(&mut tick);
        rotate_body(&mut tick);
        self.keep(tick.mob, tick.living, tick.yaw, tick.pitch);
        tick.out
    }

    /// One whole tick, as the region runs it.
    fn whole_tick(&mut self, land: &Land) -> MobOutput {
        let mut draws = self.stream();
        let mut tick = self.tick(land, &mut draws);
        crate::mobs::tick::tick(&mut tick);
        self.keep(tick.mob, tick.living, tick.yaw, tick.pitch);
        tick.out
    }

    /// Its `aiStep`'s own part alone (after `LivingEntity`'s): the draws it made and
    /// what it wrote and showed.
    fn after_ai_step(&mut self, land: &Land) -> (u64, MobOutput) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(land, &mut draws);
        Ravager.after_ai_step(&mut tick);
        self.keep(tick.mob, tick.living, tick.yaw, tick.pitch);
        let out = tick.out;
        (draws.draws_since(&start), out)
    }

    fn keep(&mut self, mob: Mob, living: Living, yaw: f32, pitch: f32) {
        self.mob = mob;
        self.living = living;
        self.living.body.old_position = self.living.body.position;
        self.yaw = yaw;
        self.pitch = pitch;
        self.lived += 1;
    }

    /// `f` run on the ravager as its next AI step would have it.
    fn with_ai<R>(&self, land: &Land, f: impl FnOnce(&mut Ai<'_, '_>) -> R) -> R {
        let mut draws = self.stream();
        let mut tick = self.tick(land, &mut draws);
        let mut setup = Setup::default();
        Ravager.register_goals(&mut setup);
        let config = setup.config;
        let mut ai = Ai {
            tick: &mut tick,
            config: &config,
            sensing: Default::default(),
            navigation_ticked: false,
        };
        f(&mut ai)
    }

    fn set(&mut self, change: impl FnOnce(&mut RavagerState)) {
        illagers::update_own(&mut self.mob, change);
    }
}

/// The hits a tick wrote: whom, of what damage type and amount, and its knockback.
fn strikes(out: &MobOutput) -> Vec<(EntityId, String, f32, Option<[f64; 3]>)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Strike { key, hurt } => Some((
                *key,
                hurt.damage_type.clone(),
                hurt.amount.0,
                hurt.knockback.map(|push| push.map(|value| value.0)),
            )),
            _ => None,
        })
        .collect()
}

fn swung(out: &MobOutput) -> bool {
    out.local.iter().any(|local| matches!(local, Local::Swing))
}

fn sound(sound: i32) -> Event {
    Event::Sound {
        sound,
        volume: 1.0,
        pitch: 1.0,
    }
}

const MOVE_LOOK: u8 = flags::MOVE | flags::LOOK;

#[test]
fn its_goals_and_targets_are_the_game_s_at_their_priorities_and_leaves_cost_it_nothing() {
    let mut setup = Setup::default();
    Ravager.register_goals(&mut setup);
    let goals: Vec<(i32, u8, bool)> = setup
        .goals
        .iter()
        .map(|goal| (goal.priority, goal.slot.flags(), goal.slot.every_tick()))
        .collect();
    // The raider's five (the patrol and the raid's four), then float, the attack, the
    // stroll away from water and the two looks.
    let priorities: Vec<i32> = goals.iter().map(|goal| goal.0).collect();
    assert_eq!(priorities, [4, 1, 3, 4, 5, 0, 4, 5, 6, 10]);
    assert_eq!(goals[5], (0, flags::JUMP, true));
    assert_eq!(goals[6], (4, MOVE_LOOK, true));
    assert_eq!(goals[7], (5, flags::MOVE, false));
    // Who hurt it, players, villagers, iron golems.
    let targets: Vec<(i32, u8)> = setup
        .targets
        .iter()
        .map(|goal| (goal.priority, goal.slot.flags()))
        .collect();
    assert_eq!(
        targets,
        [
            (2, flags::TARGET),
            (3, flags::TARGET),
            (4, flags::TARGET),
            (4, flags::TARGET),
        ]
    );
    assert_eq!(setup.config.malus[PathType::Leaves as usize], Some(0.0));
}

#[test]
fn its_body_is_the_game_s() {
    assert_eq!(supplied(attributes::MAX_HEALTH), 100.0);
    assert_eq!(supplied(attributes::MOVEMENT_SPEED), 0.3);
    assert_eq!(supplied(attributes::KNOCKBACK_RESISTANCE), 0.75);
    assert_eq!(supplied(attributes::ATTACK_DAMAGE), 12.0);
    assert_eq!(supplied(attributes::ATTACK_KNOCKBACK), 1.5);
    assert_eq!(supplied(attributes::FOLLOW_RANGE), 32.0);
    assert_eq!(supplied(attributes::STEP_HEIGHT), 1.0);
    let info = EntityTypeInfo::of(types::RAVAGER);
    assert_eq!((info.width, info.height), (1.95, 2.2));
    assert_eq!(Ravager.head().max_y_rot, 45);
    assert_eq!(
        Ravager.step_sound(&Mob::fresh()),
        Some(sounds::ENTITY_RAVAGER_STEP)
    );
    assert_eq!(Ravager.step_volume(), 0.15);
}

#[test]
fn its_reach_is_the_raider_s_less_a_twentieth_across() {
    let reach = f64::from(2.04_f32).sqrt() - f64::from(0.6_f32);
    // Its box's east side, then the reach a raider's box would have, then its own.
    let side = 0.5 + 1.95 / 2.0;
    for (east, within) in [
        (side + reach - 0.05 - 0.01, true),
        (side + reach - 0.05 + 0.01, false),
        (side + reach - 0.01, false),
    ] {
        let mut land = Land::new();
        // The player's box begins at `east`.
        land.players.push(survivor([east + 0.3, 0.0, 0.5]));
        let run = Run::new(&land, [0.5, 0.0, 0.5]);
        let found = run.with_ai(&land, |ai| {
            let target = sensing::find(ai, PLAYER).expect("the player is seen");
            RavagerAttackGoal::within_range(ai, &target)
        });
        assert_eq!(found, within, "{east}");
    }
}

#[test]
fn its_hit_is_of_twelve_with_its_knockback_shown_and_heard_and_holds_it_still_ten_ticks() {
    let mut land = Land::new();
    land.players.push(survivor([2.5, 0.0, 0.5]));
    let mut run = Run::new(&land, [0.5, 0.0, 0.5]);
    run.mob.body.combat.target = Some(PLAYER);
    let out = run.step(&land);
    assert!(swung(&out));
    let hits = strikes(&out);
    assert_eq!(hits.len(), 1);
    let (whom, damage_type, amount, knockback) = &hits[0];
    assert_eq!(*whom, PLAYER);
    assert_eq!(damage_type, "minecraft:mob_attack");
    assert_eq!(*amount, 12.0);
    // `knockback(1.5 · 0.5, sin(yaw), -cos(yaw))`.
    assert_eq!(knockback.expect("its hit knocks back")[0], 0.75);
    let shown: Vec<&Event> = out
        .events
        .iter()
        .filter(|event| matches!(event, Event::Entity(_) | Event::Sound { .. }))
        .collect();
    assert_eq!(
        shown,
        [&Event::Entity(4), &sound(sounds::ENTITY_RAVAGER_ATTACK)]
    );
    assert_eq!(state_of(&run.mob).attack_tick, ATTACK_DURATION);

    // Ten ticks it thinks nothing and stands, its base speed nothing; its counter goes
    // down by one a tick.
    for left in (0..ATTACK_DURATION).rev() {
        let out = run.whole_tick(&land);
        assert!(!swung(&out), "{left}");
        assert!(strikes(&out).is_empty(), "{left}");
        assert_eq!(state_of(&run.mob).attack_tick, left);
        assert_eq!(base_speed(&run.mob), 0.0, "{left}");
    }
    // Free again, its base speed is drawn a tenth of the way to its speed with a target.
    run.whole_tick(&land);
    assert_eq!(base_speed(&run.mob), 0.0 + 0.1 * (0.35 - 0.0));
}

#[test]
fn its_base_speed_is_drawn_a_tenth_of_the_way_to_its_speed_with_or_without_a_target() {
    let land = Land::new();
    let mut run = Run::new(&land, [0.5, 0.0, 0.5]);
    run.after_ai_step(&land);
    assert_eq!(base_speed(&run.mob), 0.3 + 0.1 * (0.3 - 0.3));
    run.mob.body.combat.target = Some(PLAYER);
    run.after_ai_step(&land);
    assert_eq!(base_speed(&run.mob), 0.3 + 0.1 * (0.35 - 0.3));
}

#[test]
fn a_shield_stuns_it_one_time_in_two_and_else_throws_the_blocker_back() {
    let at = [0.5, 0.0, 0.5];
    let blocker = [2.5, 0.0, 1.5];
    let (mut stunned, mut knocked) = (0, 0);
    for seed in 0..40 {
        let mut mob = Mob::fresh();
        let mut own = Random::of_entity(seed, KEY, 0);
        let mut replay = own.clone();
        let stuns = replay.next_double() < 0.5;
        let mut events = Vec::new();
        let blocked = blocked_by_item(&mut mob, &mut own, at, blocker, &mut events);
        assert_eq!(own, replay, "one draw");
        if stuns {
            stunned += 1;
            assert_eq!(blocked, Some(Blocked::Stunned));
            assert_eq!(state_of(&mob).stunned_tick, STUN_DURATION);
            assert_eq!(
                events,
                [sound(sounds::ENTITY_RAVAGER_STUNNED), Event::Entity(39)]
            );
        } else {
            knocked += 1;
            // dx = 2, dz = 1, d = 5: (2 / 5 · 4, 0.2, 1 / 5 · 4).
            assert_eq!(blocked, Some(Blocked::Knocked([1.6, 0.2, 0.8])));
            assert_eq!(state_of(&mob), RavagerState::default());
            assert!(events.is_empty());
        }
    }
    assert!(stunned > 0 && knocked > 0);

    // While it roars nothing happens, and nothing is drawn.
    let mut mob = Mob::fresh();
    illagers::update_own(&mut mob, |own: &mut RavagerState| own.roar_tick = 3);
    let mut own = Random::of_entity(1, KEY, 0);
    let before = own.clone();
    let mut events = Vec::new();
    assert_eq!(
        blocked_by_item(&mut mob, &mut own, at, blocker, &mut events),
        None
    );
    assert_eq!(own, before);
    assert!(events.is_empty());
}

#[test]
fn the_strong_knockback_divides_by_the_square_of_the_distance_and_never_by_nothing() {
    assert_eq!(
        strong_knockback([0.0, 5.0, 0.0], [3.0, 0.0, 4.0]),
        [3.0 / 25.0 * 4.0, 0.2, 4.0 / 25.0 * 4.0]
    );
    assert_eq!(
        strong_knockback([1.0, 0.0, 1.0], [1.0, 9.0, 1.0]),
        [0.0, 0.2, 0.0]
    );
}

/// The land round a ravager at the origin's block for its roar: a villager beside it,
/// a pillager, a witch, another ravager, a dead zombie, and a cow out of reach.
fn roar_land() -> Land {
    let mut land = Land::new();
    land.players.push(survivor([1.5, 0.0, 3.5]));
    land.entities
        .push(other(EntityId(101), types::VILLAGER, [3.5, 0.0, 0.5], 20.0));
    land.entities
        .push(other(EntityId(102), types::PILLAGER, [0.5, 0.0, 3.5], 24.0));
    land.entities
        .push(other(EntityId(103), types::WITCH, [-2.5, 0.0, 0.5], 26.0));
    land.entities.push(other(
        EntityId(104),
        types::RAVAGER,
        [0.5, 0.0, -3.0],
        100.0,
    ));
    land.entities
        .push(other(EntityId(105), types::ZOMBIE, [-1.5, 0.0, -1.5], 0.0));
    land.entities
        .push(other(EntityId(106), types::COW, [8.5, 0.0, 0.5], 10.0));
    land
}

#[test]
fn a_stun_counts_down_drawing_for_its_particle_then_it_roars_ten_ticks_after_its_roar_sound() {
    let land = roar_land();
    let feet = [0.5, 0.0, 0.5];
    let mut run = Run::new(&land, feet);
    run.set(|own| own.stunned_tick = STUN_DURATION);
    // Forty ticks of the stun: each draws `nextInt(6)`, and on a 0 two doubles; the
    // last plays its roar and begins the roar's counter.
    for left in (0..STUN_DURATION).rev() {
        let mut replay = run.stream();
        let start = replay.clone();
        if replay.next_int(6) == 0 {
            replay.next_double();
            replay.next_double();
        }
        let (draws, out) = run.after_ai_step(&land);
        assert_eq!(draws, replay.draws_since(&start), "{left}");
        let own = state_of(&run.mob);
        assert_eq!(own.stunned_tick, left);
        assert_eq!(base_speed(&run.mob), 0.0);
        if left == 0 {
            assert_eq!(own.roar_tick, ROAR_DURATION);
            assert_eq!(out.events, [sound(sounds::ENTITY_RAVAGER_ROAR)]);
        } else {
            assert_eq!(own.roar_tick, 0);
            assert!(out.events.is_empty());
        }
        assert!(out.local.is_empty());
    }
    // The roar's counter down to 10, drawing nothing; at 10 it roars.
    for left in (ROAR_AT..ROAR_DURATION).rev() {
        let (draws, out) = run.after_ai_step(&land);
        assert_eq!(draws, 0, "{left}");
        assert_eq!(state_of(&run.mob).roar_tick, left);
        if left != ROAR_AT {
            assert!(out.local.is_empty() && out.events.is_empty(), "{left}");
            continue;
        }
        // The villager and the witch are hurt and thrown back; the pillager, an
        // illager, only thrown back; the other ravager, the dead zombie, the cow out
        // of reach and the player (creative, pushed by their own client) nothing.
        let push = |to: Vec3| strong_knockback(feet, to);
        let hurt = |key: EntityId| Local::Hurt {
            key,
            kind: kind(types::RAVAGER),
            from: feet,
            amount: ROAR_DAMAGE,
            sound: None,
            knockback: None,
            fling: None,
        };
        assert_eq!(
            out.local,
            [
                hurt(EntityId(101)),
                Local::Push {
                    key: EntityId(101),
                    by: push([3.5, 0.0, 0.5]),
                },
                Local::Push {
                    key: EntityId(102),
                    by: push([0.5, 0.0, 3.5]),
                },
                hurt(EntityId(103)),
                Local::Push {
                    key: EntityId(103),
                    by: push([-2.5, 0.0, 0.5]),
                },
            ]
        );
        assert_eq!(
            out.events,
            [Event::GameEvent(GameEvent::EntityAction), Event::Entity(69)]
        );
    }
    // Then it stands ten ticks more, and roars no more.
    for left in (0..ROAR_AT).rev() {
        let (_, out) = run.after_ai_step(&land);
        assert_eq!(state_of(&run.mob).roar_tick, left);
        assert!(out.local.is_empty() && out.events.is_empty());
    }
    assert_eq!(state_of(&run.mob), RavagerState::default());
}

#[test]
fn a_ravager_stunned_or_roaring_thinks_nothing() {
    let mut land = Land::new();
    land.players.push(survivor([2.5, 0.0, 0.5]));
    for change in [
        (|own: &mut RavagerState| own.stunned_tick = 5) as fn(&mut RavagerState),
        |own: &mut RavagerState| own.roar_tick = 15,
    ] {
        let mut run = Run::new(&land, [0.5, 0.0, 0.5]);
        run.mob.body.combat.target = Some(PLAYER);
        run.set(change);
        let out = run.whole_tick(&land);
        assert!(!swung(&out) && strikes(&out).is_empty());
    }
}

#[test]
fn stopped_by_leaves_it_breaks_every_leaves_block_in_its_box_grown_and_else_jumps() {
    let feet = [0.5, 0.0, 0.5];
    // Leaves within 0.2 of its box (x from -0.475 to 1.475, y to 2.2): at x = 1 and
    // x = -1, y = 0 and 2; stone at y = 1, which it does not break; leaves farther,
    // at x = 2, which it does not reach.
    let mut land = Land::new();
    for (x, y, state) in [
        (1, 0, blocks::OAK_LEAVES),
        (1, 1, blocks::STONE),
        (1, 2, blocks::BIRCH_LEAVES),
        (-1, 0, blocks::OAK_LEAVES),
        (2, 0, blocks::OAK_LEAVES),
    ] {
        land.set(x, y, 0, state);
    }
    let mut run = Run::new(&land, feet);
    run.living.body.horizontal_collision = true;
    let (_, out) = run.after_ai_step(&land);
    assert_eq!(
        out.local,
        [
            Local::BreakBlock {
                pos: BlockPos::new(-1, 0, 0),
            },
            Local::BreakBlock {
                pos: BlockPos::new(1, 0, 0),
            },
            Local::BreakBlock {
                pos: BlockPos::new(1, 2, 0),
            },
        ]
    );
    assert_eq!(run.living.body.motion, [0.0; 3], "it broke some: no jump");

    // Against stone alone it jumps, on the ground.
    let mut land = Land::new();
    land.set(1, 0, 0, blocks::STONE);
    let mut run = Run::new(&land, feet);
    run.living.body.horizontal_collision = true;
    let (_, out) = run.after_ai_step(&land);
    assert!(out.local.is_empty());
    let jump = supplied(attributes::JUMP_STRENGTH) as f32;
    assert_eq!(run.living.body.motion[1], f64::from(jump));

    // Not stopped, it breaks nothing and does not jump, leaves or not.
    let mut land = Land::new();
    land.set(1, 0, 0, blocks::OAK_LEAVES);
    let mut run = Run::new(&land, feet);
    let (_, out) = run.after_ai_step(&land);
    assert!(out.local.is_empty());
    assert_eq!(run.living.body.motion, [0.0; 3]);
}

#[test]
fn a_raider_steering_it_leaves_its_goals_their_moves_looks_and_jumps() {
    let land = Land::new();
    let mut run = Run::new(&land, [0.5, 0.0, 0.5]);
    run.controlled_by_mob = true;
    run.mob.mind.goals.0.disabled = flags::MOVE | flags::LOOK | flags::JUMP;
    let mut draws = run.stream();
    let mut tick = run.tick(&land, &mut draws);
    Ravager.before_ai_step(&mut tick);
    assert_eq!(tick.mob.mind.goals.0.disabled, 0);
    // With no mob steering it, `Mob`'s flags are left as they are.
    run.controlled_by_mob = false;
    let mut draws = run.stream();
    let mut tick = run.tick(&land, &mut draws);
    tick.mob.mind.goals.0.disabled = flags::JUMP;
    Ravager.before_ai_step(&mut tick);
    assert_eq!(tick.mob.mind.goals.0.disabled, flags::JUMP);
}

#[test]
fn its_egg_makes_one_that_draws_for_a_leader_but_never_leads() {
    assert!(is_built(kind(types::RAVAGER)));
    assert_eq!(
        ItemComponents::of(items::RAVAGER_SPAWN_EGG).entity,
        Some(kind(types::RAVAGER))
    );
    let mut low = false;
    for seed in 0..200 {
        let mut level = Random::of_entity(seed, EntityId(1), 0);
        let mut replay = level.clone();
        low |= replay.next_float() < 0.06;
        let mut own = Random::of_entity(seed, KEY, 0);
        let mut mob = Mob::fresh();
        Ravager.finalize_spawn(&mut Spawning {
            reason: SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
            game_time: 0,
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome: None,
            at: BlockPos::new(0, 0, 0),
            day: 0,
        });
        assert_eq!(level, replay, "the leader's one draw");
        let shared = illagers::state_of(&mob);
        assert!(!shared.patrol_leader);
        assert!(shared.can_join_raid);
        assert!(mob.body.equipment.slots.iter().all(Option::is_none));
    }
    assert!(
        low,
        "some draw would have made a leader of one that could lead"
    );
}

#[test]
fn its_counters_are_kept_beside_the_raider_s_state_and_read_back_from_its_record_and_a_template() {
    // A fresh one keeps nothing.
    assert_eq!(Mob::fresh().state, TypeState::default());
    let mut mob = Mob::fresh();
    illagers::update(&mut mob, |shared| shared.patrolling = true);
    illagers::update_own(&mut mob, |own: &mut RavagerState| {
        *own = RavagerState {
            attack_tick: 7,
            stunned_tick: 31,
            roar_tick: 12,
        };
    });
    let read = Mob::decode(&mob.encode()).expect("its record reads back");
    assert_eq!(
        state_of(&read),
        RavagerState {
            attack_tick: 7,
            stunned_tick: 31,
            roar_tick: 12,
        }
    );
    assert!(illagers::state_of(&read).patrolling);

    let tag = template::build::compound(&[
        template::build::int("AttackTick", 3),
        template::build::int("StunTick", 20),
        template::build::int("RoarTick", 15),
        template::build::byte("Patrolling", 1),
    ]);
    let mut from = Mob::fresh();
    Ravager.from_template(&mut from, &tag);
    assert_eq!(
        state_of(&from),
        RavagerState {
            attack_tick: 3,
            stunned_tick: 20,
            roar_tick: 15,
        }
    );
    assert!(illagers::state_of(&from).patrolling);
}
