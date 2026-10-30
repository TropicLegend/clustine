//! Family E1 of P12 from the game's rules (`docs/groundwork/p-plan.md`, section 18.3;
//! the brief's sections 5.8 and 5.9; d ZombifiedPiglin and MagmaCube): the zombified
//! piglin's making, state, goals, anger, alert and sounds, and the magma cube's size,
//! jumps in the air and in lava, harm and sounds; both types' spawn rules.

use std::collections::BTreeMap;

use clustine_data::sounds;
use clustine_data::{BlockState, EntityTypeInfo, attributes as ids, blocks, entity_types, items};
use clustine_world::generation::Draws;
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId};

use super::{KIND as PIGLIN, PiglinOwn, own_of, write_own};
use crate::living::{Kind, Living, slots};
use crate::mobs::Mob;
use crate::mobs::ai::{Setup, server_ai_step};
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::types::{SpawnReason, Spawning, cube_mob, magma_cube, of, zombie_family};
use crate::mobs::view::{LightAt, MobWorld, Seen};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::spawning::rules::{Light, SpawnWorld};

const KEY: EntityId = EntityId(40);

/// The cow a zombified piglin of a test is angry at.
const COW: EntityId = EntityId(5);

const MAGMA: u16 = entity_types::MAGMA_CUBE as u16;

// ---------------------------------------------------------------------------------
// A field of stone below y 0 and air above, as the monsters' tests of N9 have it.

struct Field {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    entities: Vec<(Entity, Aabb)>,
    time: i64,
}

impl Field {
    fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
            entities: Vec::new(),
            time: 1_000,
        }
    }
}

impl World for Field {
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

    fn players(&self) -> Vec<crate::mobs::view::SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        Some(LightAt { sky: 0, block: 0 })
    }

    fn sky_darken(&self) -> i32 {
        11
    }

    fn day(&self) -> u64 {
        18_000
    }

    fn game_time(&self) -> i64 {
        self.time
    }
}

/// A mob of the type `id` standing at `feet` as a record and its box, of `mob`.
fn record(key: u64, id: u16, feet: Vec3, mob: &Mob) -> (Entity, Aabb) {
    let info = EntityTypeInfo::of(i32::from(id));
    let entity = crate::mobs::making::fresh_record(
        EntityId(key),
        id,
        uuid::Uuid::from_u128(u128::from(key)),
        feet,
        0.0,
        mob,
    );
    let size = crate::mobs::types::own_size(&entity)
        .map_or((info.width, info.height), |own| (own.width, own.height));
    (entity, Aabb::of_entity(feet, size.0, size.1))
}

/// A living mob of some health.
fn alive() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob
}

/// A body of the type `kind` of `mob`'s size at `feet`, on the ground where `on_ground`.
fn living(kind: u16, mob: &Mob, feet: Vec3, on_ground: bool) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let size = of(kind).dimensions(mob);
    let (width, height, eye) = size.map_or((info.width, info.height, info.eye_height), |own| {
        (own.width, own.height, own.eye_height)
    });
    let body = Moving {
        stands_on_lava: false,
        position: feet,
        motion: [0.0; 3],
        on_ground,
        fall_distance: 0.0,
        width,
        height,
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
            kind,
            hurts: Vec::new(),
        },
        eye,
    );
    living.first_tick = false;
    living.air = crate::living::MAX_AIR;
    living
}

/// One mob ticked step by step in a field, as a region would.
struct Run {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
}

impl Run {
    fn new(kind: u16, mob: Mob, feet: Vec3, on_ground: bool) -> Self {
        let living = living(kind, &mob, feet, on_ground);
        Self {
            kind,
            mob,
            living,
            yaw: 0.0,
            pitch: 0.0,
            lived: 0,
        }
    }

    /// Its stream in its next tick.
    fn stream(&self) -> Random {
        Random::of_entity(7, KEY, self.lived)
    }

    /// `f` on its next tick, kept; the draws `f` made and what the tick put out.
    fn with_tick(&mut self, world: &Field, f: impl FnOnce(&mut MobTick<'_>)) -> (u64, MobOutput) {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            yaw: self.yaw,
            pitch: self.pitch,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: true,
            mob: self.mob.clone(),
            living: self.living.clone(),
            mob_type: of(self.kind),
            world,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        let out = tick.out;
        self.lived += 1;
        (draws.draws_since(&start), out)
    }

    /// The type's `customServerAiStep` alone.
    fn custom_step(&mut self, world: &Field) -> (u64, MobOutput) {
        self.with_tick(world, |tick| tick.mob_type.custom_server_ai_step(tick))
    }
}

/// A level's random that records what is drawn of it, and answers a `nextInt(20)` from
/// its script where it has one.
struct Script {
    random: Random,
    twenty: Option<u32>,
    calls: Vec<&'static str>,
}

impl Script {
    fn new(twenty: Option<u32>) -> Self {
        Self {
            random: Random::of_entity(11, EntityId(1), 0),
            twenty,
            calls: Vec::new(),
        }
    }
}

impl Draws for Script {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push("int");
        let drawn = self.random.next_int(bound);
        match (bound, self.twenty) {
            (20, Some(scripted)) => scripted,
            _ => drawn,
        }
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push("float");
        self.random.next_float()
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push("double");
        self.random.next_double()
    }

    fn next_bool(&mut self) -> bool {
        self.calls.push("bool");
        self.random.next_bool()
    }
}

/// A mob of the type `kind` made by an egg's `finalizeSpawn` from `level`.
fn made_by_egg(kind: u16, level: &mut dyn Draws) -> Mob {
    let mut own = Random::of_entity(3, KEY, 0);
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
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
    mob
}

/// The sounds a tick put out, by id.
fn sounds_of(out: &MobOutput) -> Vec<i32> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

/// A field for the spawn rules: blocks over air.
struct Rules(BTreeMap<BlockPos, BlockState>);

impl SpawnWorld for Rules {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(self.0.get(&pos).copied().unwrap_or(blocks::AIR))
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        Some(Light { sky: 15, block: 15 })
    }

    fn sky_darken(&self) -> i32 {
        0
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        None
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(64)
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        6_000
    }

    fn dimension(&self) -> clustine_world::Dimension {
        clustine_world::Dimension::Nether
    }
}

fn on(ground: BlockState) -> Rules {
    Rules([(BlockPos::new(0, 63, 0), ground)].into_iter().collect())
}

// ---------------------------------------------------------------------------------
// The zombified piglin.

/// `checkZombifiedPiglinSpawnRules`: not on a nether wart block, whatever the light,
/// drawing nothing.
#[test]
fn a_zombified_piglin_spawns_on_anything_but_a_nether_wart_block_in_any_light() {
    let at = BlockPos::new(0, 64, 0);
    let mut draws = Random::of_entity(1, KEY, 0);
    let before = draws.clone();
    assert!(super::check_spawn_rules(
        &on(blocks::NETHERRACK),
        at,
        &mut draws
    ));
    assert!(!super::check_spawn_rules(
        &on(blocks::NETHER_WART_BLOCK),
        at,
        &mut draws
    ));
    assert!(super::check_spawn_rules(
        &on(blocks::WARPED_WART_BLOCK),
        at,
        &mut draws
    ));
    assert_eq!(draws, before, "the rule draws nothing");
}

/// `populateDefaultEquipmentSlots`: one `nextInt(20)` of the level's random, a golden
/// spear at 0 and a golden sword otherwise, and no armour drawn for.
#[test]
fn an_egg_s_zombified_piglin_holds_a_golden_sword_or_one_time_in_twenty_a_spear() {
    for (scripted, item) in [
        (Some(0), items::GOLDEN_SPEAR),
        (Some(7), items::GOLDEN_SWORD),
    ] {
        let mut level = Script::new(scripted);
        let mob = made_by_egg(PIGLIN, &mut level);
        let held = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
            .as_ref()
            .map(|stack| stack.item);
        assert_eq!(held, Some(item));
        for slot in [slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD] {
            assert!(mob.body.equipment.slots[usize::from(slot)].is_none());
        }
        assert_eq!(
            level.calls.iter().filter(|call| **call == "int").count(),
            1,
            "one int, the weapon's: {:?}",
            level.calls
        );
    }
}

/// `randomizeReinforcementsChance()` is the zombified piglin's own: none, and nothing
/// of its own stream drawn for it, where a zombie's draws a double.
#[test]
fn a_zombified_piglin_draws_no_reinforcements_chance_and_has_none() {
    let zombie = entity_types::ZOMBIE as u16;
    let mut drawn = Vec::new();
    for kind in [zombie, PIGLIN] {
        let mut mob = Mob::fresh();
        let mut own = Random::of_entity(9, KEY, 0);
        let start = own.clone();
        zombie_family::handle_attributes(kind, &mut mob, &mut own, 0.0, SpawnReason::SpawnEgg);
        drawn.push(own.draws_since(&start));
        if kind == PIGLIN {
            let reinforcements = mob
                .body
                .attributes
                .value(kind, attribute(ids::SPAWN_REINFORCEMENTS));
            assert_eq!(reinforcements, 0.0);
        }
    }
    assert_eq!(drawn[0], drawn[1] + 1, "one double fewer: {drawn:?}");
    // Its attributes, the data's.
    let mob = Mob::fresh();
    let value = |id| mob.body.attributes.value(PIGLIN, attribute(id));
    assert_eq!(value(ids::MOVEMENT_SPEED), f64::from(0.23_f32));
    assert_eq!(value(ids::ATTACK_DAMAGE), 5.0);
}

/// Its own part follows the family's in its type state: the family reads its part as a
/// zombie's, and a write through the family keeps its own.
#[test]
fn a_zombified_piglin_s_state_is_the_family_s_followed_by_its_own() {
    let mut mob = Mob::fresh();
    let own = PiglinOwn {
        first_anger_sound_in: 3,
        until_next_alert: 90,
        had_target: true,
        ..PiglinOwn::default()
    };
    write_own(&mut mob, &own);
    assert_eq!(own_of(&mob), own);
    assert!(!zombie_family::is_baby(&mob));
    of(PIGLIN).set_baby(&mut mob, true);
    assert!(zombie_family::is_baby(&mob), "the family's part changed");
    assert_eq!(own_of(&mob), own, "its own kept");
    // A fresh one writes nothing; a family's state alone reads as a fresh own.
    write_own(&mut mob, &PiglinOwn::default());
    assert_eq!(own_of(&mob), PiglinOwn::default());
    assert!(zombie_family::is_baby(&mob));
    // A baby's eyes are at 0.78.
    let size = of(PIGLIN).dimensions(&mob).expect("a baby's own size");
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.49, 0.98, 0.78)
    );
}

/// `addBehaviourGoals` after `Zombie.registerGoals`' own: the turtle egg (4), the looks
/// (8, 8), the spear's slot (1), the zombie's attack (2), the stroll (7); its targets:
/// who hurt it (1), the players it is angry at (2), the universal anger's reset (3).
#[test]
fn the_zombified_piglin_s_goals_and_targets_are_its_own_on_the_zombie_s() {
    let mut setup = Setup::default();
    of(PIGLIN).register_goals(&mut setup);
    let goals: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(goals, [4, 8, 8, 1, 2, 7]);
    let targets: Vec<i32> = setup.targets.iter().map(|goal| goal.priority).collect();
    assert_eq!(targets, [1, 2, 3]);
}

/// Its own kind's alert reaches it, a zombie's does not.
#[test]
fn a_zombified_piglin_s_alert_reaches_its_kind_and_a_zombie_s_passes_it_over() {
    use crate::mobs::ai::targets::comes_to_alert;
    let zombie = entity_types::ZOMBIE as u16;
    assert!(comes_to_alert(PIGLIN, PIGLIN));
    assert!(!comes_to_alert(zombie, PIGLIN));
    assert!(comes_to_alert(zombie, entity_types::HUSK as u16));
    assert!(!comes_to_alert(PIGLIN, zombie));
}

/// A field with a cow at `(4.5, 0, 0.5)` and zombified piglins of no target: one near,
/// one more than 10 above, one near that holds a target of its own.
fn field_with_others() -> Field {
    let mut world = Field::new();
    let cow = entity_types::COW as u16;
    world
        .entities
        .push(record(5, cow, [4.5, 0.0, 0.5], &alive()));
    world
        .entities
        .push(record(61, PIGLIN, [-3.5, 0.0, 2.5], &alive()));
    world
        .entities
        .push(record(62, PIGLIN, [0.5, 12.0, 0.5], &alive()));
    let mut busy = alive();
    busy.body.combat.target = Some(EntityId(5));
    world
        .entities
        .push(record(63, PIGLIN, [2.5, 0.0, -2.5], &busy));
    world
}

/// `setTarget`'s two draws, the anger's timer, and then the countdowns: its haste once
/// angry, its angry sound when the first countdown ends, and at the alert's end the one
/// near of its kind with no target given its target.
#[test]
fn a_zombified_piglin_with_a_target_grows_angry_hastes_sounds_and_alerts_its_kind() {
    let world = field_with_others();
    let mut mob = alive();
    mob.body.combat.target = Some(COW);
    let mut run = Run::new(PIGLIN, mob, [0.5, 0.0, 0.5], true);
    let mut replay = run.stream();
    let (drawn, out) = run.custom_step(&world);
    let first = replay.next_int(21) as i32;
    let alert = 80 + replay.next_int(41) as i32;
    let timer = 400 + i64::from(replay.next_int(381));
    assert_eq!(drawn, 3, "two of `setTarget` and the anger's timer");
    let own = own_of(&run.mob);
    assert_eq!(
        own.first_anger_sound_in, first,
        "not angry before the update"
    );
    assert_eq!(own.until_next_alert, alert - 1, "counted down at once");
    assert_eq!(own.anger.end, world.time + timer);
    assert_eq!(own.anger.target, Some(COW));
    assert!(own.had_target);
    assert!(sounds_of(&out).is_empty());
    let speed = |run: &Run| {
        run.mob
            .body
            .attributes
            .value(PIGLIN, attribute(ids::MOVEMENT_SPEED))
    };
    assert_eq!(speed(&run), f64::from(0.23_f32), "not yet hasty");
    let mut heard = None;
    let mut alerted = Vec::new();
    for step in 1..=200 {
        let (_, out) = run.custom_step(&world);
        if step == 1 {
            assert_eq!(speed(&run), f64::from(0.23_f32) + 0.05, "hasty while angry");
        }
        if sounds_of(&out).contains(&sounds::ENTITY_ZOMBIFIED_PIGLIN_ANGRY) {
            assert!(heard.is_none(), "the first angry sound plays once");
            heard = Some(step);
        }
        for local in &out.local {
            if let Local::Alert {
                key,
                attacker,
                class,
                ..
            } = local
            {
                assert_eq!((*attacker, *class), (COW, PIGLIN));
                alerted.push((step, *key));
            }
        }
    }
    if first > 0 {
        assert_eq!(heard, Some(first), "after its countdown");
    } else {
        assert_eq!(heard, None, "a countdown of 0 plays nothing");
    }
    let first_alert = alerted.first().map(|(step, _)| *step);
    assert_eq!(first_alert, Some(alert), "at the alert's end");
    let keys: Vec<EntityId> = alerted
        .iter()
        .filter(|(step, _)| Some(*step) == first_alert)
        .map(|(_, key)| *key)
        .collect();
    assert_eq!(keys, [EntityId(61)], "only the near one of no target");
    // The next alert comes 80 to 120 steps after.
    if let Some((next, _)) = alerted.get(1) {
        assert!((alert + 81..=alert + 121).contains(next), "{next}");
    }
}

/// A baby is not hasty when angry; one calm again is not hasty, and its ambient sound
/// is the angry one only while angry.
#[test]
fn a_baby_does_not_haste_and_the_calm_neither_haste_nor_sound_angry() {
    let world = field_with_others();
    let mut baby = alive();
    of(PIGLIN).set_baby(&mut baby, true);
    baby.body.combat.target = Some(COW);
    let mut run = Run::new(PIGLIN, baby, [0.5, 0.0, 0.5], true);
    run.custom_step(&world);
    run.custom_step(&world);
    let speed = run
        .mob
        .body
        .attributes
        .value(PIGLIN, attribute(ids::MOVEMENT_SPEED));
    assert_eq!(speed, f64::from(0.23_f32) * 1.5, "a baby's own speed alone");
    let mut draws = Random::of_entity(1, KEY, 0);
    let ambient = |mob: &Mob, now, draws: &mut Random| {
        of(PIGLIN).ambient_sound_drawn(mob, draws, now, 0, None)
    };
    let own = own_of(&run.mob);
    assert_eq!(
        ambient(&run.mob, world.time, &mut draws),
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_ANGRY)
    );
    assert_eq!(
        ambient(&run.mob, own.anger.end, &mut draws),
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_AMBIENT),
        "its anger over"
    );
    // A grown one whose anger is over and who holds no target is not hasty.
    let mut calm = alive();
    write_own(
        &mut calm,
        &PiglinOwn {
            anger: crate::mobs::types::neutral::Anger {
                end: world.time - 1,
                target: None,
            },
            ..PiglinOwn::default()
        },
    );
    let mut run = Run::new(PIGLIN, calm, [0.5, 0.0, 0.5], true);
    let (drawn, _) = run.custom_step(&world);
    assert_eq!(drawn, 0, "nothing to draw without a target");
    let speed = run
        .mob
        .body
        .attributes
        .value(PIGLIN, attribute(ids::MOVEMENT_SPEED));
    assert_eq!(speed, f64::from(0.23_f32));
}

/// It is immune to fire and does not burn by day; its hurt, death and step sounds.
#[test]
fn a_zombified_piglin_is_immune_to_fire_and_has_its_own_sounds() {
    let info = EntityTypeInfo::of(entity_types::ZOMBIFIED_PIGLIN);
    assert!(info.fire_immune);
    assert!(!clustine_blocks::data::entity_in_tag(
        entity_types::ZOMBIFIED_PIGLIN,
        "minecraft:burn_in_daylight"
    ));
    let mob = Mob::fresh();
    let source =
        crate::mobs::body::hurt::DamageSource::of(crate::mobs::body::hurt::DamageType::FALL);
    assert_eq!(
        of(PIGLIN).hurt_sound(&mob, &source),
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_HURT)
    );
    assert_eq!(
        of(PIGLIN).death_sound(&mob),
        Some(sounds::ENTITY_ZOMBIFIED_PIGLIN_DEATH)
    );
    assert_eq!(
        of(PIGLIN).step_sound(&mob),
        Some(sounds::ENTITY_ZOMBIE_STEP)
    );
}

// ---------------------------------------------------------------------------------
// The magma cube.

fn magma(size: i32) -> Mob {
    let mut mob = Mob::fresh();
    cube_mob::set_size(MAGMA, &mut mob, size, true);
    mob
}

/// `checkMagmaCubeSpawnRules`: anything but peaceful, wherever, drawing nothing.
#[test]
fn a_magma_cube_spawns_anywhere_but_in_peace() {
    let at = BlockPos::new(0, 64, 0);
    let mut draws = Random::of_entity(1, KEY, 0);
    let before = draws.clone();
    for ground in [blocks::NETHER_WART_BLOCK, blocks::AIR, blocks::LAVA] {
        assert!(magma_cube::check_spawn_rules(&on(ground), at, &mut draws));
    }
    assert_eq!(draws, before);
}

/// `setSize`: the cube's health, speed and attack, and its own armour of three times
/// its size; its experience its size.
#[test]
fn a_magma_cube_s_size_sets_its_armour_to_three_times_it() {
    for size in [1, 2, 4] {
        let mob = magma(size);
        let value = |id| mob.body.attributes.value(MAGMA, attribute(id));
        assert_eq!(value(ids::ARMOR), f64::from(size * 3), "size {size}");
        assert_eq!(value(ids::ATTACK_DAMAGE), f64::from(size));
        assert_eq!(value(ids::MAX_HEALTH), f64::from(size * size));
        assert_eq!(
            value(ids::MOVEMENT_SPEED),
            f64::from(0.2_f32 + 0.1_f32 * size as f32)
        );
        let mut draws = Random::of_entity(1, KEY, 0);
        assert_eq!(of(MAGMA).experience_reward(MAGMA, &mob, &mut draws), size);
    }
    // A slime's armour is untouched.
    let slime = entity_types::SLIME as u16;
    let mut mob = Mob::fresh();
    cube_mob::set_size(slime, &mut mob, 4, true);
    assert_eq!(mob.body.attributes.value(slime, attribute(ids::ARMOR)), 0.0);
}

/// On the ground its move control jumps once its delay is over, with its own jump
/// sound; the next delay four times the slime's.
#[test]
fn a_magma_cube_jumps_with_four_times_the_slime_s_delay_and_its_own_sound() {
    let world = Field::new();
    let mut run = Run::new(MAGMA, magma(2), [0.5, 0.0, 0.5], true);
    let mut replay = run.stream();
    let (_, out) = run.with_tick(&world, server_ai_step);
    // The target goals' intervals, the random way's delay and way, then the jump's
    // delay.
    replay.next_int(5);
    replay.next_int(5);
    let _delay = replay.next_int(60);
    let _way = replay.next_int(360);
    let jump_delay = (replay.next_int(20) as i32 + 10) * 4;
    assert!(run.mob.body.inputs.jumping, "it jumps");
    assert_eq!(cube_mob::state_of(&run.mob).jump_delay, jump_delay);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_MAGMA_CUBE_JUMP]);
    // A tiny one's jump sound is the same.
    let mut run = Run::new(MAGMA, magma(1), [0.5, 0.0, 0.5], true);
    let (_, out) = run.with_tick(&world, server_ai_step);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_MAGMA_CUBE_JUMP]);
}

/// `jumpFromGround`: the jump strength's 0.42 and a tenth of its size.
#[test]
fn a_magma_cube_s_jump_is_the_strength_and_a_tenth_of_its_size() {
    let world = Field::new();
    for size in [1, 2, 4] {
        let mut run = Run::new(MAGMA, magma(size), [0.5, 0.0, 0.5], true);
        let mut power = None;
        run.with_tick(&world, |tick| power = tick.mob_type.jump_power(tick, 1.0));
        assert_eq!(power, Some(0.42_f32 + size as f32 * 0.1_f32), "size {size}");
    }
}

/// `jumpInLiquid(LAVA)`: jumping in lava, its upward motion `0.22 + 0.05 · size`, sent;
/// in water the body's own jump stands.
#[test]
fn a_magma_cube_leaps_up_through_lava_and_not_through_water() {
    let world = Field::new();
    for size in [1, 4] {
        let mut run = Run::new(MAGMA, magma(size), [0.5, 0.0, 0.5], false);
        run.mob.body.inputs.jumping = true;
        run.living.fluids.lava_height = 0.5;
        run.living.body.motion = [0.0, 0.04, 0.0];
        let (_, out) = run.with_tick(&world, |tick| tick.mob_type.before_travel(tick));
        assert_eq!(
            run.living.body.motion[1],
            f64::from(0.22_f32 + 0.05_f32 * size as f32),
            "size {size}"
        );
        assert!(out.needs_sync);
    }
    let mut run = Run::new(MAGMA, magma(2), [0.5, 0.0, 0.5], false);
    run.mob.body.inputs.jumping = true;
    run.living.fluids.water_height = 0.5;
    run.living.fluids.lava_height = 0.5;
    run.living.body.in_water = true;
    run.living.body.motion = [0.0, 0.04, 0.0];
    run.with_tick(&world, |tick| tick.mob_type.before_travel(tick));
    assert_eq!(run.living.body.motion[1], 0.04, "water's jump comes first");
    // Not jumping: nothing.
    let mut run = Run::new(MAGMA, magma(2), [0.5, 0.0, 0.5], false);
    run.living.fluids.lava_height = 0.5;
    run.with_tick(&world, |tick| tick.mob_type.before_travel(tick));
    assert_eq!(run.living.body.motion[1], 0.0);
}

/// `canDealDamage`: a magma cube means harm at any size, where a tiny slime does not.
#[test]
fn a_tiny_magma_cube_means_harm() {
    let mut world = Field::new();
    let mut golem = Mob::fresh();
    golem.body.vitals.health = 100.0;
    world.entities.push(record(
        5,
        entity_types::IRON_GOLEM as u16,
        [4.5, 0.0, 0.5],
        &golem,
    ));
    let mut mob = magma(1);
    mob.body.combat.target = Some(EntityId(5));
    let mut run = Run::new(MAGMA, mob, [0.5, 0.0, 0.5], true);
    run.with_tick(&world, server_ai_step);
    assert!(cube_mob::state_of(&run.mob).aggressive);
}

/// Its hurt, death and squish sounds by its size, and its loot by its size.
#[test]
fn a_magma_cube_s_sounds_and_loot_follow_its_size() {
    let source =
        crate::mobs::body::hurt::DamageSource::of(crate::mobs::body::hurt::DamageType::FALL);
    let tiny = magma(1);
    let large = magma(4);
    assert_eq!(
        of(MAGMA).hurt_sound(&tiny, &source),
        Some(sounds::ENTITY_MAGMA_CUBE_HURT_SMALL)
    );
    assert_eq!(
        of(MAGMA).hurt_sound(&large, &source),
        Some(sounds::ENTITY_MAGMA_CUBE_HURT)
    );
    assert_eq!(
        of(MAGMA).death_sound(&tiny),
        Some(sounds::ENTITY_MAGMA_CUBE_DEATH_SMALL)
    );
    assert_eq!(
        of(MAGMA).death_sound(&large),
        Some(sounds::ENTITY_MAGMA_CUBE_DEATH)
    );
    assert_eq!(of(MAGMA).loot_facts(&large).cube_size, Some(4));
    assert!(EntityTypeInfo::of(entity_types::MAGMA_CUBE).fire_immune);
    // Its landing's squish.
    let world = Field::new();
    let mut run = Run::new(MAGMA, magma(1), [0.5, 0.0, 0.5], true);
    let (_, out) = run.with_tick(&world, cube_mob::after_tick);
    assert_eq!(sounds_of(&out), [sounds::ENTITY_MAGMA_CUBE_SQUISH_SMALL]);
}
