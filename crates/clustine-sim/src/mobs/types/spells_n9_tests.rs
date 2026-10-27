//! The evoker's spells and the vex from the game's rules (`docs/groundwork/n9-plan.md`,
//! section 8.4; the brief's 1.2, 2.7 and 3.5; n7 `SpellcasterIllager` and its goals,
//! `Evoker` and its goals, `Vex` and its inner classes), not from the code: each spell's
//! timing and draws in the evoker's own stream, where its fangs stand, how many vexes
//! it counts and what the three it summons hold, which sheep its wololo takes; the vex's
//! move, its charge, its drift, its life and its starving, its flight through stone,
//! its targets; and, in a region, the vexes made, the fangs risen and a blue sheep made
//! red.
//!
//! A mob's draws are worked out from its stream in a tick (`Random::of_entity` of its
//! key and `lived`), replayed in the order the game draws. The tests' mob has an even
//! key, so its selectors run whole in the ticks whose `lived` is odd, and in the first.

use std::collections::BTreeMap;

use clustine_data::sounds;
use clustine_data::{BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types, items};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::EvokerState;
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::spells::{BLUE, CastingSpellGoal, RED, Spell, UseSpellGoal};
use crate::mobs::ai::{Ai, Operation, Setup, server_ai_step};
use crate::mobs::body::Event;
use crate::mobs::body::attributes::supplied;
use crate::mobs::tick::{self, Local, MobOutput, MobTick};
use crate::mobs::types::sheep::{COLOURS, SheepState};
use crate::mobs::types::vex::{
    self, VexChargeAttackGoal, VexCopyOwnerTargetGoal, VexRandomMoveGoal, VexState,
};
use crate::mobs::types::{MobType, SpawnReason, Spawning, animal, illagers, of};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::shown::DataShown;

const EVOKER: u16 = entity_types::EVOKER as u16;
const VEX: u16 = entity_types::VEX as u16;
const VILLAGER: u16 = entity_types::VILLAGER as u16;
const SHEEP: u16 = entity_types::SHEEP as u16;

/// The ticked mob's key: even.
const KEY: EntityId = EntityId(40);
const SEED: u64 = 7;

/// Whether `lived` is a tick whose selectors run whole for [`KEY`].
fn full(lived: u32) -> bool {
    let tick_count = lived + 1;
    (tick_count + 40).is_multiple_of(2) || tick_count <= 1
}

// ------------------------------------------------------------------------------------
// A world of stone below y = 0 and air above, with what a test puts in it.

#[derive(Default)]
struct Ground {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    entities: Vec<(Entity, Aabb)>,
    players: Vec<SeenPlayer>,
}

impl Ground {
    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert((x, y, z), state);
    }

    fn add(&mut self, entity: Entity) {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        self.entities.push((entity, bounds));
    }
}

impl World for Ground {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
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

impl Holds for Ground {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Ground {
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
        Some(LightAt { sky: 0, block: 0 })
    }

    fn day(&self) -> u64 {
        18_000
    }
}

/// A mob of the type `kind` of its full health, as its record would decode.
fn healthy(kind: u16) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health =
        supplied(kind, crate::mobs::body::attribute(ids::MAX_HEALTH)).expect("a health") as f32;
    mob.body.vitals.air = MAX_AIR;
    mob
}

/// The record of `mob`, of the type `kind`, standing at `feet`.
fn record(key: u64, kind: u16, feet: Vec3, mob: &Mob) -> Entity {
    Entity {
        key: EntityId(key),
        kind,
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
    }
}

/// A villager standing at `feet`.
fn villager(key: u64, feet: Vec3) -> Entity {
    record(key, VILLAGER, feet, &healthy(VILLAGER))
}

/// A sheep of the colour `colour` standing at `feet`, alive or not.
fn sheep(key: u64, feet: Vec3, colour: u8, alive: bool) -> Entity {
    let mut mob = healthy(SHEEP);
    if !alive {
        mob.body.vitals.health = 0.0;
    }
    let mut own = SheepState::default();
    own.set_colour(colour);
    mob.state = animal::write(&animal::Animal::default(), &own);
    record(key, SHEEP, feet, &mob)
}

/// A vex hovering at `feet`, owned by `owner`.
fn vex_at(key: u64, feet: Vec3, owner: Option<EntityId>, alive: bool) -> Entity {
    let mut mob = healthy(VEX);
    if !alive {
        mob.body.vitals.health = 0.0;
    }
    vex::update(&mut mob, |own| own.owner = owner);
    record(key, VEX, feet, &mob)
}

/// A body of the type `kind` at `position`, its first tick behind it.
fn body(kind: u16, position: Vec3, on_ground: bool) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let moving = Moving {
        position,
        motion: [0.0; 3],
        on_ground,
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
        moving,
        Kind::Mob {
            kind,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living
}

// ------------------------------------------------------------------------------------
// Types of one goal or a few: what a test's draws are made of.

/// An evoker with only the goals `register` adds, its spell's ticks counted down as an
/// evoker's are.
struct EvokerWith(fn(&mut Setup));

impl MobType for EvokerWith {
    fn register_goals(&self, setup: &mut Setup) {
        (self.0)(setup);
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        of(EVOKER).custom_server_ai_step(tick);
    }
}

/// A vex with only the goals `register` adds, its body and controls a vex's.
struct VexWith(fn(&mut Setup));

impl MobType for VexWith {
    fn register_goals(&self, setup: &mut Setup) {
        (self.0)(setup);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        of(VEX).tick_move_control(ai);
    }

    fn before_tick(&self, tick: &mut MobTick<'_>) {
        of(VEX).before_tick(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        of(VEX).after_tick(tick);
    }

    fn no_gravity(&self, mob: &Mob) -> bool {
        of(VEX).no_gravity(mob)
    }
}

fn summoning(setup: &mut Setup) {
    setup.add_goal(1, CastingSpellGoal);
    setup.add_goal(
        4,
        UseSpellGoal {
            spell: Spell::Summon,
        },
    );
}

fn biting(setup: &mut Setup) {
    setup.add_goal(1, CastingSpellGoal);
    setup.add_goal(
        5,
        UseSpellGoal {
            spell: Spell::Fangs,
        },
    );
}

fn turning(setup: &mut Setup) {
    setup.add_goal(1, CastingSpellGoal);
    setup.add_goal(
        6,
        UseSpellGoal {
            spell: Spell::Wololo,
        },
    );
}

fn charging(setup: &mut Setup) {
    setup.add_goal(4, VexChargeAttackGoal);
}

fn copying(setup: &mut Setup) {
    setup.add_target(2, VexCopyOwnerTargetGoal);
}

static SUMMONING: EvokerWith = EvokerWith(summoning);
static BITING: EvokerWith = EvokerWith(biting);
static TURNING: EvokerWith = EvokerWith(turning);
static CHARGING: VexWith = VexWith(charging);
static COPYING: VexWith = VexWith(copying);

// ------------------------------------------------------------------------------------
// One mob ticked again and again.

struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    lived: u32,
    mob_type: &'static dyn MobType,
}

impl Run {
    fn new(kind: u16, mob_type: &'static dyn MobType, mob: Mob, living: Living) -> Self {
        Self {
            kind,
            mob,
            living,
            yaw: 0.0,
            lived: 0,
            mob_type,
        }
    }

    /// An evoker of `mob_type` on the floor at the origin's block, holding `target`.
    fn evoker(mob_type: &'static dyn MobType, target: Option<EntityId>) -> Self {
        let mut mob = healthy(EVOKER);
        mob.body.combat.target = target;
        Self::new(EVOKER, mob_type, mob, body(EVOKER, [0.5, 0.0, 0.5], true))
    }

    /// The stream of its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    fn tick<'w>(&self, world: &'w Ground, draws: &'w mut Random) -> MobTick<'w> {
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
            pitch: 0.0,
            mob_type: self.mob_type,
            world,
            draws,
            out: MobOutput::default(),
        }
    }

    /// One AI step (`serverAiStep`): its draws and its output.
    fn step(&mut self, world: &Ground) -> (u64, MobOutput) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = self.tick(world, &mut draws);
        server_ai_step(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        let out = tick.out;
        self.lived += 1;
        (draws.draws_since(&start), out)
    }

    /// One whole tick of the mob.
    fn whole_tick(&mut self, world: &Ground) -> MobOutput {
        let mut draws = self.stream();
        let mut tick = self.tick(world, &mut draws);
        tick::tick(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        let out = tick.out;
        self.lived += 1;
        out
    }

    /// `f` asked of the mob as its next AI step would have it, with `draws`.
    fn with_ai<R>(
        &mut self,
        world: &Ground,
        draws: &mut Random,
        f: impl FnOnce(&mut Ai<'_, '_>) -> R,
    ) -> (R, MobOutput) {
        let mut tick = self.tick(world, draws);
        let mut setup = Setup::default();
        self.mob_type.register_goals(&mut setup);
        let config = setup.config;
        let found = {
            let mut ai = Ai {
                tick: &mut tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            f(&mut ai)
        };
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        (found, tick.out)
    }

    fn own(&self) -> EvokerState {
        illagers::own_of::<EvokerState>(&self.mob)
    }

    /// The steps until one writes something, with the `lived` it wrote at.
    fn until_written(&mut self, world: &Ground, most: u32) -> (u32, u64, MobOutput) {
        for _ in 0..most {
            let lived = self.lived;
            let (drawn, out) = self.step(world);
            if !out.local.is_empty() {
                return (lived, drawn, out);
            }
        }
        panic!("nothing written in {most} steps");
    }
}

/// The eyes of the entity `key` as a mob reads them (`getEyePosition`).
fn eyes_of(world: &Ground, key: EntityId) -> Vec3 {
    let seen = world.entity(key).expect("in the world");
    let looked = crate::mobs::ai::sensing::Looked::of_seen(&seen);
    [looked.position[0], looked.eye_y, looked.position[2]]
}

fn sounds_of(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// The spells' numbers.

#[test]
fn each_spell_has_the_game_s_warmup_casting_time_interval_id_and_sound() {
    assert_eq!(
        [Spell::Summon, Spell::Fangs, Spell::Wololo].map(|spell| (
            spell.id(),
            spell.warmup(),
            spell.casting(),
            spell.interval()
        )),
        [(1, 20, 100, 340), (2, 20, 40, 100), (3, 40, 60, 140)]
    );
    assert_eq!(
        [Spell::Summon, Spell::Fangs, Spell::Wololo].map(Spell::prepare_sound),
        [
            sounds::ENTITY_EVOKER_PREPARE_SUMMON,
            sounds::ENTITY_EVOKER_PREPARE_ATTACK,
            sounds::ENTITY_EVOKER_PREPARE_WOLOLO
        ]
    );
    assert_eq!(COLOURS[usize::from(BLUE)], "blue");
    assert_eq!(COLOURS[usize::from(RED)], "red");
}

#[test]
fn the_evoker_registers_casting_its_three_spells_and_its_targets_in_the_game_s_order() {
    let mut setup = Setup::default();
    of(EVOKER).register_goals(&mut setup);
    let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    // `Raider`'s five, then the evoker's own.
    assert_eq!(goals, [4, 1, 3, 4, 5, 0, 1, 2, 3, 4, 5, 6, 8, 9, 10]);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [1, 2, 3, 3]);
    // The casting goal takes the move and the look; the spells take nothing.
    assert_eq!(
        setup.goals[6].slot.flags(),
        crate::mobs::ai::selector::flags::MOVE | crate::mobs::ai::selector::flags::LOOK
    );
    for spell in 9..12 {
        assert_eq!(setup.goals[spell].slot.flags(), 0);
    }
}

// ------------------------------------------------------------------------------------
// The summoning.

/// A villager six blocks east of the evoker, its target.
fn villager_east() -> (Ground, EntityId) {
    let mut world = Ground::default();
    world.add(villager(50, [6.5, 0.0, 0.5]));
    (world, EntityId(50))
}

#[test]
fn the_summoning_starts_at_a_living_target_with_its_draw_of_eight_its_sound_and_its_spell() {
    let (world, target) = villager_east();
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    let (drawn, out) = evoker.step(&world);
    assert_eq!(drawn, 1, "nextInt(8), which beats no vexes whatever it is");
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_PREPARE_SUMMON]);
    let own = evoker.own();
    assert_eq!(own.spell, Spell::Summon.id());
    assert_eq!(
        own.spell_ticks, 99,
        "100, counted down once in the same tick"
    );
    let shown = of(EVOKER).shown(&evoker.mob);
    assert!(shown.contains(&(17, DataShown::Byte(1), false)));
}

#[test]
fn the_summoning_casts_after_ten_of_its_goal_s_ticks_drawing_seven_for_each_of_three_vexes() {
    let (world, target) = villager_east();
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    let (lived, drawn, out) = evoker.until_written(&world, 40);
    // Its warmup, `adjustedTickDelay(20)` of a goal ticked every other tick, is 10: one
    // at its start, then the next nine whole ticks (lived 1, 3, ... 17).
    assert_eq!(lived, 17);
    assert_eq!(drawn, 21);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_CAST_SPELL]);
    let mut replay = Random::of_entity(SEED, KEY, lived);
    assert_eq!(out.local.len(), 6);
    for (n, pair) in out.local.chunks(2).enumerate() {
        let dx = -2 + replay.next_int(5) as i32;
        let dz = -2 + replay.next_int(5) as i32;
        let sword = replay.next_float();
        let (a, b) = (replay.next_double(), replay.next_double());
        let hand = replay.next_float();
        let life = 20 * (30 + replay.next_int(90) as i32);
        let _ = (sword, a, b);
        let block = BlockPos::new(dx, 1, dz);
        let Local::MakeMob { kind, mob, at } = &pair[0] else {
            panic!("vex {n} is made first: {:?}", pair[0]);
        };
        assert_eq!(*kind, VEX);
        assert_eq!(*at, [f64::from(dx) + 0.5, 1.0, f64::from(dz) + 0.5]);
        let own = vex::state_of(mob);
        assert_eq!(own.owner, Some(KEY));
        assert_eq!(own.bound_origin, Some(block));
        assert_eq!(own.life, Some(life));
        assert_eq!(
            mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
                .as_ref()
                .map(|stack| stack.item),
            Some(items::IRON_SWORD)
        );
        assert_eq!(
            mob.body
                .equipment
                .drop_chance(usize::from(slots::MAIN_HAND)),
            0.0
        );
        assert_eq!(mob.body.flags.left_handed, hand < 0.05);
        assert_eq!(
            pair[1],
            Local::GameEvent {
                name: "minecraft:entity_place",
                pos: block
            }
        );
    }
}

#[test]
fn a_vex_s_finalize_spawn_holds_a_sword_it_never_drops_and_draws_its_enchantment_then_mob_s() {
    #[derive(Default)]
    struct Calls(Vec<&'static str>);
    impl clustine_world::generation::Draws for Calls {
        fn next_int(&mut self, _: u32) -> u32 {
            self.0.push("int");
            0
        }
        fn next_float(&mut self) -> f32 {
            self.0.push("float");
            0.5
        }
        fn next_double(&mut self) -> f64 {
            self.0.push("double");
            0.5
        }
        fn next_bool(&mut self) -> bool {
            self.0.push("bool");
            false
        }
    }
    let mut level = Calls::default();
    let mut mob = Mob::fresh();
    crate::mobs::making::finalize(
        VEX,
        &mut Spawning {
            mob: &mut mob,
            level: &mut level,
            own: &mut Random::of_entity(SEED, KEY, 0),
            biome: None,
            at: BlockPos::new(0, 0, 0),
            day: 0,
            game_time: 0,
            reason: SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    assert_eq!(level.0, ["float", "double", "double", "float"]);
    assert_eq!(
        mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
            .as_ref()
            .map(|stack| stack.item),
        Some(items::IRON_SWORD)
    );
    assert_eq!(
        mob.body
            .equipment
            .drop_chance(usize::from(slots::MAIN_HAND)),
        0.0
    );
}

#[test]
fn the_summoning_casts_only_where_its_draw_of_eight_beats_the_living_vexes_within_16() {
    let (mut world, target) = villager_east();
    // Three vexes near, and those that do not count: a dead one, one beyond the box.
    for (n, x) in [2.5, 3.5, -4.5].into_iter().enumerate() {
        world.add(vex_at(60 + n as u64, [x, 1.0, 0.5], None, true));
    }
    world.add(vex_at(70, [1.5, 1.0, 1.5], None, false));
    world.add(vex_at(71, [17.5, 1.0, 0.5], None, true));
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    for _ in 0..60 {
        let lived = evoker.lived;
        let (drawn, out) = evoker.step(&world);
        if !full(lived) {
            assert_eq!(drawn, 0);
            continue;
        }
        let draw = Random::of_entity(SEED, KEY, lived).next_int(8);
        assert_eq!(drawn, 1, "one draw an asked tick, lived {lived}");
        let started = sounds_of(&out).contains(&sounds::ENTITY_EVOKER_PREPARE_SUMMON);
        assert_eq!(started, draw + 1 > 3, "lived {lived}, drew {draw}");
        if started {
            return;
        }
    }
    panic!("no draw beat three vexes in 60 ticks");
}

#[test]
fn eight_vexes_near_keep_the_evoker_from_summoning_though_it_draws_every_asked_tick() {
    let (mut world, target) = villager_east();
    for n in 0..8 {
        world.add(vex_at(60 + n, [1.5 + n as f64, 2.0, 3.5], None, true));
    }
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    for _ in 0..30 {
        let lived = evoker.lived;
        let (drawn, out) = evoker.step(&world);
        assert_eq!(drawn, u64::from(full(lived)));
        assert!(out.events.is_empty() && out.local.is_empty());
    }
    assert_eq!(evoker.own().spell, 0);
}

#[test]
fn the_evoker_casts_still_watching_its_target_and_shows_no_spell_once_its_casting_ends() {
    let (world, target) = villager_east();
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    evoker.step(&world);
    evoker.step(&world);
    // The casting goal ran at lived 1: the evoker looks at the villager's eyes.
    assert_eq!(
        evoker.mob.body.controls.look.wanted,
        eyes_of(&world, target)
    );
    let mut ended = None;
    for _ in 0..110 {
        let lived = evoker.lived;
        evoker.step(&world);
        if evoker.own().spell == 0 {
            ended = Some(lived);
            break;
        }
        assert_eq!(evoker.own().spell, Spell::Summon.id(), "lived {lived}");
    }
    // Its 100 ticks set at lived 0 run out at lived 99; the casting goal stops at the
    // next whole tick, 101.
    assert_eq!(ended, Some(101));
    assert_eq!(evoker.own().spell_ticks, 0);
}

#[test]
fn the_summoning_starts_again_at_the_first_asked_tick_340_after_its_start() {
    let (world, target) = villager_east();
    let mut evoker = Run::evoker(&SUMMONING, Some(target));
    let mut starts = Vec::new();
    for _ in 0..400 {
        let lived = evoker.lived;
        let (_, out) = evoker.step(&world);
        if sounds_of(&out).contains(&sounds::ENTITY_EVOKER_PREPARE_SUMMON) {
            starts.push(lived);
        }
    }
    assert_eq!(starts, [0, 341]);
}

#[test]
fn no_spell_at_a_target_starts_without_one() {
    let world = Ground::default();
    for mob_type in [&SUMMONING, &BITING] {
        let mut evoker = Run::evoker(mob_type, None);
        for _ in 0..20 {
            let (drawn, out) = evoker.step(&world);
            assert_eq!(drawn, 0);
            assert!(out.events.is_empty() && out.local.is_empty());
        }
    }
}

// ------------------------------------------------------------------------------------
// The fangs.

/// The fangs an output makes, with where, how turned and after how long.
fn fangs_of(out: &MobOutput) -> Vec<(Vec3, f32, i32)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Fangs { at, yaw, warmup } => Some((*at, *yaw, *warmup)),
            _ => None,
        })
        .collect()
}

#[test]
fn fangs_at_a_far_target_rise_in_a_line_of_sixteen_one_tick_apart_and_draw_nothing() {
    let mut world = Ground::default();
    world.add(villager(50, [8.5, 0.0, 0.5]));
    let mut evoker = Run::evoker(&BITING, Some(EntityId(50)));
    let (_, out) = evoker.step(&world);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_PREPARE_ATTACK]);
    assert_eq!(evoker.own().spell, Spell::Fangs.id());
    assert_eq!(evoker.own().spell_ticks, 39);
    let (lived, drawn, out) = evoker.until_written(&world, 40);
    assert_eq!((lived, drawn), (17, 0));
    let fangs = fangs_of(&out);
    assert_eq!(fangs.len(), 16);
    for (i, (at, yaw, warmup)) in fangs.into_iter().enumerate() {
        // Due east: `atan2(0, 8)` is 0.
        assert_eq!(at, [0.5 + 1.25 * (i as f64 + 1.0), 0.0, 0.5], "fang {i}");
        assert_eq!(yaw, 0.0);
        assert_eq!(warmup, i as i32);
    }
    // Each with `entity_place` where it stands.
    assert_eq!(out.local.len(), 32);
    assert!(matches!(
        out.local[1],
        Local::GameEventAt {
            name: "minecraft:entity_place",
            at: [1.75, 0.0, 0.5],
            affected: None
        }
    ));
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_CAST_SPELL]);
}

#[test]
fn fangs_stand_on_the_first_floor_down_raised_by_what_they_stand_in_and_not_over_a_pit() {
    let mut world = Ground::default();
    world.add(villager(50, [8.5, 0.0, 0.5]));
    // At the second fang (x 3.0) a block on the floor: they stand on it.
    world.set(3, 0, 0, blocks::STONE);
    // At the fourth (x 5.5) a bottom slab: they stand in it, on its top.
    world.set(5, 0, 0, blocks::OAK_SLAB);
    // At the eighth (x 10.5) a pit two deep: nothing to stand on within a block below
    // the lower of the two's feet.
    world.set(10, -1, 0, blocks::AIR);
    world.set(10, -2, 0, blocks::AIR);
    let mut evoker = Run::evoker(&BITING, Some(EntityId(50)));
    let (_, _, out) = evoker.until_written(&world, 40);
    let fangs = fangs_of(&out);
    assert_eq!(fangs.len(), 15);
    let heights: Vec<(f64, f64)> = fangs.iter().map(|(at, _, _)| (at[0], at[1])).collect();
    assert!(heights.contains(&(3.0, 1.0)));
    assert!(heights.contains(&(5.5, 0.5)));
    assert!(!heights.iter().any(|(x, _)| *x == 10.5));
    assert!(heights.contains(&(9.25, 0.0)) && heights.contains(&(11.75, 0.0)));
}

#[test]
fn fangs_at_a_near_target_rise_in_two_rings_five_at_once_and_eight_after_three_ticks() {
    let mut world = Ground::default();
    world.add(villager(50, [2.5, 0.0, 0.5]));
    let mut evoker = Run::evoker(&BITING, Some(EntityId(50)));
    let (_, _, out) = evoker.until_written(&world, 40);
    let fangs = fangs_of(&out);
    assert_eq!(fangs.len(), 13);
    for (n, (at, yaw, warmup)) in fangs.iter().enumerate() {
        let (radius, first) = if n < 5 { (1.5, 0) } else { (2.5, 3) };
        assert_eq!(*warmup, first, "fang {n}");
        let distance = ((at[0] - 0.5).powi(2) + (at[2] - 0.5).powi(2)).sqrt();
        assert!((distance - radius).abs() < 1e-3, "fang {n} at {distance}");
        assert_eq!(at[1], 0.0);
        // Turned to their angle about the evoker.
        let angle = if n < 5 {
            n as f32 * std::f32::consts::PI * 0.4
        } else {
            (n - 5) as f32 * std::f32::consts::PI * 2.0 / 8.0 + 1.256_637_1
        };
        assert_eq!(*yaw, angle * 57.295_776, "fang {n}");
    }
}

// ------------------------------------------------------------------------------------
// The wololo.

/// The sheep around an evoker at the origin: blue at 4 east, white nearer, blue 6
/// south, blue 20 east (out of range), blue behind a wall, a dead blue one.
fn flock() -> Ground {
    let mut world = Ground::default();
    world.add(sheep(51, [4.5, 0.0, 0.5], BLUE, true));
    world.add(sheep(52, [3.5, 0.0, 2.5], 0, true));
    world.add(sheep(53, [0.5, 0.0, 6.5], BLUE, true));
    world.add(sheep(54, [20.5, 0.0, 0.5], BLUE, true));
    world.add(sheep(55, [-5.5, 0.0, 0.5], BLUE, true));
    for y in 0..4 {
        for z in -3..4 {
            world.set(-3, y, z, blocks::STONE);
        }
    }
    world.add(sheep(56, [0.5, 0.0, -8.5], BLUE, false));
    world
}

#[test]
fn the_wololo_takes_one_of_the_blue_sheep_it_sees_within_16_by_its_draw() {
    let world = flock();
    let mut evoker = Run::evoker(&TURNING, None);
    let (drawn, out) = evoker.step(&world);
    assert_eq!(drawn, 1);
    let draw = Random::of_entity(SEED, KEY, 0).next_int(2);
    let chosen = [EntityId(51), EntityId(53)][draw as usize];
    assert_eq!(evoker.own().wololo_target, Some(chosen));
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_PREPARE_WOLOLO]);
    assert_eq!(evoker.own().spell, Spell::Wololo.id());
    assert_eq!(evoker.own().spell_ticks, 59);
    // Twenty of its goal's ticks later (`adjustedTickDelay(40)`) the sheep is made red,
    // and the goal lets it go.
    let (lived, drawn, out) = evoker.until_written(&world, 60);
    assert_eq!((lived, drawn), (37, 0));
    assert_eq!(
        out.local,
        [Local::SheepColour {
            key: chosen,
            colour: RED
        }]
    );
    assert_eq!(sounds_of(&out), [sounds::ENTITY_EVOKER_CAST_SPELL]);
    evoker.step(&world);
    evoker.step(&world);
    assert_eq!(evoker.own().wololo_target, None);
}

#[test]
fn the_wololo_waits_while_the_evoker_has_a_target_and_draws_nothing_without_blue_sheep() {
    let world = flock();
    let mut evoker = Run::evoker(&TURNING, Some(EntityId(52)));
    for _ in 0..10 {
        let (drawn, out) = evoker.step(&world);
        assert_eq!(drawn, 0);
        assert!(out.events.is_empty());
    }
    let mut white = Ground::default();
    white.add(sheep(52, [3.5, 0.0, 2.5], 0, true));
    let mut evoker = Run::evoker(&TURNING, None);
    for _ in 0..10 {
        let (drawn, out) = evoker.step(&white);
        assert_eq!(drawn, 0);
        assert!(out.events.is_empty());
    }
}

#[test]
fn the_casting_evoker_with_no_target_watches_its_wololo_s_sheep() {
    let world = flock();
    let mut evoker = Run::evoker(&TURNING, None);
    evoker.step(&world);
    evoker.step(&world);
    let chosen = evoker.own().wololo_target.expect("a sheep");
    assert_eq!(
        evoker.mob.body.controls.look.wanted,
        eyes_of(&world, chosen)
    );
}

// ------------------------------------------------------------------------------------
// The vex.

/// A vex in the air at `feet`.
fn flying_vex(mob_type: &'static dyn MobType, feet: Vec3, target: Option<EntityId>) -> Run {
    let mut mob = healthy(VEX);
    mob.body.combat.target = target;
    Run::new(VEX, mob_type, mob, body(VEX, feet, false))
}

#[test]
fn a_vex_s_move_control_pushes_its_motion_at_where_it_wants_and_faces_along_it() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    run.mob.body.controls.movement.wanted = [3.5, 9.0, 0.5];
    run.mob.body.controls.movement.speed = 1.0;
    run.mob.body.controls.movement.operation = Operation::MoveTo;
    run.living.body.motion = [0.0, 0.1, 0.0];
    let mut draws = run.stream();
    run.with_ai(&world, &mut draws, vex::tick_move_control);
    // `v = (3, 4, 0)`, of length 5: the motion gains `v · 0.05 / 5`.
    let motion = run.living.body.motion;
    assert!((motion[0] - 0.03).abs() < 1e-12 && (motion[1] - 0.14).abs() < 1e-12);
    assert_eq!(motion[2], 0.0);
    assert_eq!(
        run.mob.body.controls.movement.operation,
        Operation::MoveTo,
        "it keeps on"
    );
    // Facing along its motion, east: `-atan2(x, z)` in degrees.
    let facing = -(crate::mobs::ai::maths::atan2(motion[0], motion[2]) as f32) * 57.295_776;
    assert_eq!(run.yaw, facing);
    assert_eq!(run.mob.body.head.body_yaw, facing);
}

#[test]
fn a_vex_within_its_size_of_where_it_wants_waits_with_its_motion_halved() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    // Its box's size is the mean of 0.4, 0.8 and 0.4.
    run.mob.body.controls.movement.wanted = [0.5, 5.5, 0.5];
    run.mob.body.controls.movement.speed = 1.0;
    run.mob.body.controls.movement.operation = Operation::MoveTo;
    run.living.body.motion = [0.2, -0.4, 0.1];
    let mut draws = run.stream();
    run.with_ai(&world, &mut draws, vex::tick_move_control);
    assert_eq!(run.living.body.motion, [0.1, -0.2, 0.05]);
    assert_eq!(run.mob.body.controls.movement.operation, Operation::Wait);
}

#[test]
fn a_vex_charges_one_asked_tick_in_four_at_its_target_s_eyes_with_its_flag_and_its_sound() {
    let mut world = Ground::default();
    world.add(villager(50, [5.5, 0.0, 0.5]));
    let mut run = flying_vex(&CHARGING, [0.5, 1.0, 0.5], Some(EntityId(50)));
    for _ in 0..40 {
        let lived = run.lived;
        let (drawn, out) = run.step(&world);
        if !full(lived) {
            assert_eq!(drawn, 0);
            continue;
        }
        assert_eq!(drawn, 1, "nextInt(4), lived {lived}");
        let draw = Random::of_entity(SEED, KEY, lived).next_int(4);
        let started = vex::state_of(&run.mob).charging();
        assert_eq!(started, draw == 0, "lived {lived}");
        if started {
            assert_eq!(sounds_of(&out), [sounds::ENTITY_VEX_CHARGE]);
            assert_eq!(
                run.mob.body.controls.movement.wanted,
                eyes_of(&world, EntityId(50))
            );
            assert_eq!(run.mob.body.controls.movement.speed, 1.0);
            assert!(
                of(VEX)
                    .shown(&run.mob)
                    .contains(&(16, DataShown::Byte(1), false))
            );
            // On its way it draws nothing more, and flies at the target.
            let x = run.living.body.position[0];
            let (drawn, _) = run.step(&world);
            assert_eq!(drawn, 0);
            assert!(run.living.body.motion[0] > 0.0);
            assert!(vex::state_of(&run.mob).charging());
            let _ = x;
            return;
        }
    }
    panic!("no charge in 40 ticks");
}

#[test]
fn a_charging_vex_whose_box_meets_its_target_s_hits_it_for_its_attack_and_stops_charging() {
    let mut world = Ground::default();
    world.add(villager(50, [0.9, 0.0, 0.5]));
    let mut run = flying_vex(&CHARGING, [0.5, 1.0, 0.5], Some(EntityId(50)));
    vex::update(&mut run.mob, |own| own.set_charging(true));
    let mut draws = run.stream();
    let ((), out) = run.with_ai(&world, &mut draws, |ai| {
        VexChargeAttackGoal.tick(&mut (), ai);
    });
    let [Local::Strike { key, hurt }] = &out.local[..] else {
        panic!("one hit: {:?}", out.local);
    };
    assert_eq!(*key, EntityId(50));
    assert_eq!(hurt.amount.0, 4.0);
    assert_eq!(hurt.damage_type, "minecraft:mob_attack");
    assert!(!vex::state_of(&run.mob).charging());
}

#[test]
fn a_charging_vex_within_3_of_its_target_aims_again_at_its_eyes() {
    let mut world = Ground::default();
    world.add(villager(50, [2.5, 0.0, 0.5]));
    // Two across and two up: within 3, its box not meeting the villager's.
    let mut run = flying_vex(&CHARGING, [0.5, 2.0, 0.5], Some(EntityId(50)));
    vex::update(&mut run.mob, |own| own.set_charging(true));
    let mut draws = run.stream();
    let ((), out) = run.with_ai(&world, &mut draws, |ai| {
        VexChargeAttackGoal.tick(&mut (), ai);
    });
    assert!(out.local.is_empty());
    assert_eq!(
        run.mob.body.controls.movement.wanted,
        eyes_of(&world, EntityId(50))
    );
    assert_eq!(run.mob.body.controls.movement.operation, Operation::MoveTo);
}

#[test]
fn a_drifting_vex_goes_at_a_quarter_to_the_first_air_of_three_tries_about_its_origin() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    let origin = BlockPos::new(0, 3, 0);
    vex::update(&mut run.mob, |own| own.bound_origin = Some(origin));
    // Find a stream whose first try falls in the ground and second in the air.
    let mut draws = (0..)
        .map(|n| Random::of_entity(SEED, EntityId(n), 0))
        .find(|stream| {
            let mut probe = stream.clone();
            let first = [probe.next_int(15), probe.next_int(11), probe.next_int(15)];
            let second = [probe.next_int(15), probe.next_int(11), probe.next_int(15)];
            3 + first[1] as i32 - 5 < 0 && 3 + second[1] as i32 - 5 >= 0
        })
        .expect("a stream");
    let mut replay = draws.clone();
    run.with_ai(&world, &mut draws, |ai| VexRandomMoveGoal.tick(&mut (), ai));
    let _first = [
        replay.next_int(15),
        replay.next_int(11),
        replay.next_int(15),
    ];
    let [x, y, z] = [
        replay.next_int(15),
        replay.next_int(11),
        replay.next_int(15),
    ];
    let wanted = [
        f64::from(x as i32 - 7) + 0.5,
        f64::from(3 + y as i32 - 5) + 0.5,
        f64::from(z as i32 - 7) + 0.5,
    ];
    assert_eq!(draws.draws_since(&replay), 0, "two tries drawn, three each");
    let movement = &run.mob.body.controls.movement;
    assert_eq!((movement.wanted, movement.speed), (wanted, 0.25));
    assert_eq!(movement.operation, Operation::MoveTo);
    assert_eq!(run.mob.body.controls.look.wanted, wanted, "it looks there");
}

#[test]
fn a_vex_bound_deep_in_stone_tries_three_times_and_goes_nowhere() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    vex::update(&mut run.mob, |own| {
        own.bound_origin = Some(BlockPos::new(0, -30, 0))
    });
    let mut draws = run.stream();
    let start = draws.clone();
    run.with_ai(&world, &mut draws, |ai| VexRandomMoveGoal.tick(&mut (), ai));
    assert_eq!(draws.draws_since(&start), 9);
    assert_eq!(run.mob.body.controls.movement.operation, Operation::Wait);
}

#[test]
fn a_vex_drifts_only_where_it_wants_nothing_and_its_draw_of_four_is_nothing() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    let mut draws = run.stream();
    let mut replay = draws.clone();
    let (can, _) = run.with_ai(&world, &mut draws, |ai| {
        VexRandomMoveGoal.can_use(&mut (), ai)
    });
    assert_eq!(can, replay.next_int(4) == 0);
    run.mob.body.controls.movement.operation = Operation::MoveTo;
    let mut draws = run.stream();
    let start = draws.clone();
    let (can, _) = run.with_ai(&world, &mut draws, |ai| {
        VexRandomMoveGoal.can_use(&mut (), ai)
    });
    assert!(!can);
    assert_eq!(draws.draws_since(&start), 0);
}

#[test]
fn a_summoned_vex_counts_its_life_down_and_starves_by_one_at_nothing_then_every_20() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    run.mob.body.flags.no_ai = true;
    vex::update(&mut run.mob, |own| own.life = Some(2));
    run.whole_tick(&world);
    assert_eq!(vex::state_of(&run.mob).life, Some(1));
    assert_eq!(run.mob.body.vitals.health, 14.0);
    run.whole_tick(&world);
    assert_eq!(vex::state_of(&run.mob).life, Some(20));
    assert_eq!(run.mob.body.vitals.health, 13.0, "starve 1");
    for _ in 0..19 {
        run.whole_tick(&world);
    }
    assert_eq!(run.mob.body.vitals.health, 13.0);
    assert_eq!(vex::state_of(&run.mob).life, Some(1));
    run.whole_tick(&world);
    assert_eq!(run.mob.body.vitals.health, 12.0);
}

#[test]
fn a_vex_of_an_egg_has_no_limited_life_and_never_starves() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, 5.0, 0.5], None);
    run.mob.body.flags.no_ai = true;
    for _ in 0..50 {
        run.whole_tick(&world);
    }
    assert_eq!(vex::state_of(&run.mob).life, None);
    assert_eq!(run.mob.body.vitals.health, 14.0);
}

#[test]
fn a_vex_flies_through_stone_and_falls_only_in_its_first_tick() {
    let world = Ground::default();
    let mut run = flying_vex(of(VEX), [0.5, -10.0, 0.5], None);
    run.mob.body.flags.no_ai = true;
    run.living.body.motion = [0.25, 0.0, 0.0];
    run.whole_tick(&world);
    // Moved by its motion through the stone, then pulled by gravity once.
    assert_eq!(run.living.body.position, [0.75, -10.0, 0.5]);
    assert!(vex::state_of(&run.mob).no_gravity);
    assert!(
        of(VEX)
            .shown(&run.mob)
            .contains(&(5, DataShown::Boolean(true), false))
    );
    let fallen = run.living.body.motion[1];
    assert!((fallen - (-0.08 * 0.98)).abs() < 1e-6, "{fallen}");
    run.whole_tick(&world);
    let position = run.living.body.position;
    assert!((position[1] - (-10.0 + fallen)).abs() < 1e-12);
    assert!(
        (run.living.body.motion[1] - fallen * 0.98).abs() < 1e-6,
        "no gravity since"
    );
    assert!(position[0] > 0.75, "on through the stone");
    assert_eq!(
        run.mob.body.vitals.health, 14.0,
        "nor does the stone choke it"
    );
}

#[test]
fn a_vex_takes_its_owner_s_target_seen_or_not_and_draws_nothing_for_it() {
    let mut world = Ground::default();
    let mut owner = healthy(EVOKER);
    owner.body.combat.target = Some(EntityId(50));
    world.add(record(45, EVOKER, [0.5, 0.0, 0.5], &owner));
    world.add(villager(50, [-6.5, 0.0, 0.5]));
    // A wall between the vex and the villager: the copy needs no sight.
    for y in 0..6 {
        for z in -3..4 {
            world.set(-3, y, z, blocks::STONE);
        }
    }
    let mut run = flying_vex(&COPYING, [0.5, 2.0, 0.5], None);
    vex::update(&mut run.mob, |own| own.owner = Some(EntityId(45)));
    let (drawn, _) = run.step(&world);
    assert_eq!(drawn, 0);
    assert_eq!(run.mob.body.combat.target, Some(EntityId(50)));
    // With no owner, or an owner without a target, nothing.
    let mut run = flying_vex(&COPYING, [0.5, 2.0, 0.5], None);
    run.step(&world);
    assert_eq!(run.mob.body.combat.target, None);
}

#[test]
fn the_vex_registers_its_goals_and_targets_in_the_game_s_order() {
    let mut setup = Setup::default();
    of(VEX).register_goals(&mut setup);
    let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(goals, [0, 4, 8, 9, 10]);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [1, 2, 3]);
    let charge = &setup.goals[1].slot;
    assert!(charge.every_tick());
    assert_eq!(charge.flags(), crate::mobs::ai::selector::flags::MOVE);
    assert!(!setup.goals[2].slot.every_tick());
}

#[test]
fn a_vex_s_state_survives_its_record_and_a_fresh_one_writes_nothing() {
    let own = VexState {
        owner: Some(EntityId(9)),
        bound_origin: Some(BlockPos::new(1, 2, 3)),
        life: Some(700),
        flags: 1,
        no_gravity: true,
    };
    assert_eq!(vex::read(&vex::write(&own)), own);
    assert!(vex::write(&VexState::default()).bytes.is_empty());
    let entity = vex_at(3, [0.0; 3], Some(EntityId(9)), true);
    assert_eq!(vex::owner_of(&entity), Some(EntityId(9)));
    assert_eq!(vex::owner_of(&villager(4, [0.0; 3])), None);
}

#[test]
fn an_evoker_s_experience_is_ten_and_a_vex_s_three_and_more_for_its_sword() {
    let evoker = healthy(EVOKER);
    let mut draws = Random::of_entity(SEED, KEY, 0);
    assert_eq!(
        of(EVOKER).experience_reward(EVOKER, &evoker, &mut draws),
        10
    );
    let mut vex = healthy(VEX);
    vex.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(clustine_world::ItemStack::new(items::IRON_SWORD, 1));
    let mut chances = [0.085; 8];
    chances[usize::from(slots::MAIN_HAND)] = 0.0;
    vex.body.equipment.drop_chances = Some(chances);
    let mut draws = Random::of_entity(SEED, KEY, 0);
    let mut replay = draws.clone();
    let reward = of(VEX).experience_reward(VEX, &vex, &mut draws);
    // A slot it never drops counts as one it may not keep (`<= 1`).
    let expected = 3 + 1 + replay.next_int(3) as i32;
    assert_eq!(reward, expected);
}

// ------------------------------------------------------------------------------------
// In a region: what the spells' writes become.

mod in_a_region {
    use clustine_world::projectiles::Fangs;
    use clustine_world::{Biome, Chunk, ChunkArea, EntityData, EntityIds, Section};

    use super::*;
    use crate::api::{HOTBAR_SLOTS, Inventory};
    use crate::{Holdings, Region, RegionConfig, RegionState, TickInputs, Ticket};

    /// The floor's top: feet stand at y = -60.
    const FEET: f64 = -60.0;

    fn chunks() -> Vec<ChunkPos> {
        (-2..=2)
            .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
            .collect()
    }

    /// One region over the land near the origin, a stone floor at y = -61, `entities`
    /// in their chunks.
    fn region_with(entities: &[Entity]) -> Region {
        let config = RegionConfig {
            spawn: clustine_world::Vec3::new(0.5, FEET, 0.5),
            starting_inventory: Inventory::with_hotbar([const { None }; HOTBAR_SLOTS]),
            return_after: 0,
            lowest_y: -64,
            seed: SEED,
            game_seed: 0,
            light: Default::default(),
            features: Default::default(),
        };
        let state = RegionState::new(EntityIds::block(2).expect("a block"));
        let holdings = Holdings {
            held: (-3..=3)
                .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(x, z)))
                .collect(),
            pinned: vec![ChunkArea::EVERYWHERE],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut region = Region::restore(config, state, holdings);
        region.tick(&TickInputs {
            tickets_added: chunks()
                .into_iter()
                .map(|chunk| (chunk, Ticket::Viewer))
                .collect(),
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: chunks()
                .into_iter()
                .map(|position| {
                    let sections = (0..24)
                        .map(|_| Section::filled(blocks::AIR, Biome(0)))
                        .collect();
                    let mut chunk = Chunk::from_sections(-64, sections);
                    for z in 0..16 {
                        for x in 0..16 {
                            chunk.set(x, -61, z, blocks::STONE);
                        }
                    }
                    chunk.set_entities(
                        entities
                            .iter()
                            .filter(|entity| entity.chunk() == position)
                            .cloned()
                            .collect(),
                    );
                    (position, chunk)
                })
                .collect(),
            ..TickInputs::default()
        });
        region
    }

    /// Every entity record of the region's loaded chunks.
    fn records(region: &Region) -> Vec<Entity> {
        chunks()
            .into_iter()
            .filter_map(|position| region.chunk(position))
            .flat_map(|chunk| chunk.entities().to_vec())
            .collect()
    }

    fn mob_of(entity: &Entity) -> Mob {
        Mob::decode(&entity.data).expect("a mob's record")
    }

    /// The record of `mob` at `feet`, that does not think.
    fn still(key: u64, kind: u16, feet: Vec3, mut mob: Mob) -> Entity {
        mob.body.flags.no_ai = true;
        record(key, kind, feet, &mob)
    }

    #[test]
    fn an_evoker_at_a_villager_summons_three_vexes_of_its_own_and_then_raises_fangs() {
        let mut region = region_with(&[
            record(10, EVOKER, [0.5, FEET, 0.5], &healthy(EVOKER)),
            still(11, VILLAGER, [7.5, FEET, 0.5], healthy(VILLAGER)),
        ]);
        let mut vexes = Vec::new();
        for _ in 0..200 {
            region.tick(&TickInputs::default());
            vexes = records(&region)
                .into_iter()
                .filter(|entity| entity.kind == VEX)
                .collect();
            if !vexes.is_empty() {
                break;
            }
        }
        assert_eq!(vexes.len(), 3, "three at once");
        let evoker = records(&region)
            .into_iter()
            .find(|entity| entity.kind == EVOKER)
            .expect("the evoker");
        let block = crate::physics::containing(evoker.position);
        for made in &vexes {
            let mob = mob_of(made);
            let own = vex::state_of(&mob);
            assert_eq!(own.owner, Some(EntityId(10)));
            let life = own.life.expect("a limited life");
            assert!((600..=2_380).contains(&life), "{life}");
            let origin = own.bound_origin.expect("bound");
            assert!((origin.x - block.x).abs() <= 2 && (origin.z - block.z).abs() <= 2);
            assert_eq!(origin.y, block.y + 1);
            assert_eq!(
                mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
                    .as_ref()
                    .map(|stack| stack.item),
                Some(items::IRON_SWORD)
            );
            assert!(made.key.0 >= EntityIds::block(2).expect("a block").first.0);
        }
        // Once its casting is over, its fangs.
        for _ in 0..300 {
            region.tick(&TickInputs::default());
            let fangs = records(&region)
                .into_iter()
                .filter(|entity| i32::from(entity.kind) == entity_types::EVOKER_FANGS)
                .count();
            if fangs > 0 {
                return;
            }
        }
        panic!("no fangs in 300 ticks");
    }

    #[test]
    fn an_evoker_with_no_target_turns_a_blue_sheep_red() {
        let lamb = sheep(12, [4.5, FEET, 0.5], BLUE, true);
        let mut region = region_with(&[
            record(10, EVOKER, [0.5, FEET, 0.5], &healthy(EVOKER)),
            still(12, SHEEP, lamb.position, mob_of(&lamb)),
        ]);
        for _ in 0..200 {
            region.tick(&TickInputs::default());
            let colour = records(&region)
                .into_iter()
                .find(|entity| entity.key == EntityId(12))
                .map(|entity| crate::mobs::ai::spells::colour_of(&entity.data));
            if colour == Some(RED) {
                return;
            }
            assert_eq!(colour, Some(BLUE));
        }
        panic!("still blue after 200 ticks");
    }

    #[test]
    fn an_evoker_s_fangs_spare_its_own_vexes_and_bite_another_s() {
        // A vex after its first tick, at rest on the floor, owned by `owner`.
        let vex_of = |key: u64, x: f64, owner: u64| {
            let mut mob = healthy(VEX);
            vex::update(&mut mob, |own| {
                own.owner = Some(EntityId(owner));
                own.no_gravity = true;
            });
            still(key, VEX, [x, FEET, 0.5], mob)
        };
        let fangs = |key: u64, x: f64| {
            let mut entity = record(key, VEX, [x, FEET, 0.5], &Mob::fresh());
            entity.kind = entity_types::EVOKER_FANGS as u16;
            entity.data = EntityData::Fangs(Fangs {
                owner: Some(EntityId(10)),
                warmup: 0,
            });
            entity
        };
        let mut region = region_with(&[
            still(10, EVOKER, [0.5, FEET, 0.5], healthy(EVOKER)),
            vex_of(13, 3.5, 10),
            vex_of(14, 6.5, 99),
            fangs(15, 3.5),
            fangs(16, 6.5),
        ]);
        let health = |region: &Region, key: u64| {
            records(region)
                .into_iter()
                .find(|entity| entity.key == EntityId(key))
                .map(|entity| mob_of(&entity).body.vitals.health)
        };
        for _ in 0..20 {
            region.tick(&TickInputs::default());
        }
        assert_eq!(health(&region, 13), Some(14.0), "its own vex is spared");
        assert_eq!(
            health(&region, 14),
            Some(8.0),
            "the other's is bitten for 6"
        );
    }
}
