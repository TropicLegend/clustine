//! The axolotl's, the frog's and the tadpole's brains from the game's rules
//! (`AxolotlAi`, `FrogAi`, `TadpoleAi` and their own behaviours and sensors; the brief's
//! 7.4.6 to 7.5.8 and 10.7.2): the memories each registers, the draws of its making,
//! each activity and its switching, the play-dead count to the tick, the hunt's
//! cooldown, the sensors of what they hunt and eat, the croak, the laying, and the
//! frog's landing spots. Each mob is ticked on a pond of its own, its record carried
//! between ticks as a region would carry it.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, Entry, ItemStack};

use super::axolotl_ai;
use super::frog::{self, Pose};
use super::frog_ai::{self, Croak, Croaking, TryLaySpawnOnFluidNearLand};
use super::tadpole_ai;
use super::{axolotl, of};
use crate::living::{Kind, Living};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::brain::{
    self, Activity, ActivityData, Brain, BrainState, Cx, HeldShot, HeldTimed, Memories, MemoryKind,
    Provider, Sensed, Status, Tracker, Value,
};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;

const AXOLOTL: u16 = entity_types::AXOLOTL as u16;
const FROG: u16 = entity_types::FROG as u16;
const TADPOLE: u16 = entity_types::TADPOLE as u16;

/// The key of the mob ticked.
const KEY: EntityId = EntityId(40);
const SEED: u64 = 11;

/// Land of stone below y = 0 and air above, with what a test puts on it.
#[derive(Default)]
struct Pond {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    players: Vec<SeenPlayer>,
    entities: Vec<(Entity, Aabb)>,
}

impl Pond {
    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert((x, y, z), state);
    }

    /// Water from x, z = -8 to 8 and y = -4 to -1, over the stone.
    fn with_water() -> Self {
        let mut pond = Self::default();
        for x in -8..=8 {
            for z in -8..=8 {
                for y in -4..0 {
                    pond.set(x, y, z, blocks::WATER);
                }
            }
        }
        pond
    }

    /// A mob of `kind` at `position` with `health`.
    fn put_mob(&mut self, key: EntityId, kind: u16, position: Vec3, health: f32) {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = health;
        let info = EntityTypeInfo::of(i32::from(kind));
        let bounds = Aabb::of_entity(position, info.width, info.height);
        self.entities.push((
            Entity {
                key,
                kind,
                uuid: uuid::Uuid::from_u128(u128::from(key.0)),
                position,
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
            },
            bounds,
        ));
    }
}

impl World for Pond {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < -4 => blocks::STONE,
            None if pos.y < 0 => blocks::STONE,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Pond {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Pond {
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

/// A creative player at `position` holding `held`.
fn player(key: u64, position: Vec3, held: Option<i32>) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: held.map(|item| ItemStack::new(item, 1)),
        off_hand: None,
        sneaking: false,
        creative: true,
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

/// The body of a mob of `kind` at `position`, on the ground where it is not in water,
/// with its fluids as the pond has them.
fn body_at(pond: &Pond, kind: u16, position: Vec3) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let body = Body {
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
    living.fluids = crate::physics::fluids::update(pond, &living.body.bounds());
    living.body.in_water = living.fluids.in_water();
    living.body.on_ground = !living.body.in_water;
    living
}

/// One amphibian ticked on a pond: its record carried between ticks, the game time its
/// `lived`.
struct Pet {
    pond: Pond,
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    sensed: Option<Sensed>,
}

/// What a tick left: what the mob put out, how many draws it made, and the brain's
/// parts that changed.
struct Ticked {
    out: MobOutput,
    draws: u64,
    changed: Vec<u16>,
}

impl Pet {
    fn new(pond: Pond, kind: u16, at: Vec3) -> Self {
        let living = body_at(&pond, kind, at);
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 10.0;
        Self {
            pond,
            kind,
            mob,
            living,
            lived: 0,
            sensed: Some(Sensed::new()),
        }
    }

    fn stream(&self) -> Random {
        Random::of_entity(SEED, KEY, self.lived)
    }

    /// The tick with `f` run on it, as the region builds it.
    fn with_tick<R>(&mut self, f: impl FnOnce(&mut MobTick<'_>) -> R) -> (R, Ticked) {
        let before = self.mob.mind.brain.clone();
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: self.sensed.take(),
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
            mob_type: of(self.kind),
            world: &self.pond,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let answer = f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.sensed = tick.sensed;
        let out = tick.out;
        self.lived += 1;
        let changed = changed_parts(&before, &self.mob.mind.brain);
        (
            answer,
            Ticked {
                out,
                draws: draws.draws_since(&start),
                changed,
            },
        )
    }

    /// Its type's `customServerAiStep`: its brain's tick and its activities.
    fn step(&mut self) -> Ticked {
        self.with_tick(|tick| tick.mob_type.custom_server_ai_step(tick))
            .1
    }

    /// A tick of `provider`'s brain alone on the mob.
    fn brain_tick(&mut self, provider: &'static Provider) -> (Brain, Ticked) {
        let now = self.lived;
        let entries = self.mob.mind.brain.clone();
        let sensed = self.sensed.clone();
        self.with_tick(|tick| {
            let mut brain = Brain::read(provider, &entries, sensed.as_ref(), now);
            let mut setup = Setup::default();
            tick.mob_type.register_goals(&mut setup);
            let config = setup.config;
            let (written, sensed) = {
                let mut ai = Ai {
                    tick,
                    config: &config,
                    sensing: Default::default(),
                    navigation_ticked: true,
                };
                brain.tick(&mut ai, now, i64::from(now));
                brain.write()
            };
            tick.mob.mind.brain = written;
            tick.sensed = Some(sensed);
            brain
        })
    }

    /// The mob's kept memories as code outside its tick reads them, after its last tick.
    fn memories(&self) -> Memories {
        let provider = of(self.kind).brain().expect("a brain");
        brain::memories_of(provider, &self.mob, self.lived)
    }

    /// Its brain's own state.
    fn state(&self) -> BrainState {
        crate::mobs::part::decode(&self.mob.mind.brain).unwrap_or_default()
    }

    /// A sensed memory as the region keeps it.
    fn sensed(&self, kind: MemoryKind) -> Option<Value> {
        match self.sensed.as_ref()?.get(&kind)? {
            brain::Slot::Kept { value, .. } => Some(value.clone()),
            brain::Slot::Count(_) => None,
        }
    }

    /// Kept memories changed outside a tick, as a hurt or a use changes them.
    fn write(&mut self, f: impl FnOnce(&mut Memories)) {
        let provider = of(self.kind).brain().expect("a brain");
        let mut memories = brain::memories_of(provider, &self.mob, self.lived);
        f(&mut memories);
        brain::write_memories(provider, &mut self.mob, memories);
    }
}

/// The ids of the parts that differ between two writes of a brain.
fn changed_parts(before: &[Entry], after: &[Entry]) -> Vec<u16> {
    let mut ids: Vec<u16> = before.iter().chain(after).map(|entry| entry.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter(|id| {
            before.iter().find(|entry| entry.id == *id)
                != after.iter().find(|entry| entry.id == *id)
        })
        .collect()
}

fn memory_part(kind: MemoryKind) -> u16 {
    brain::ids::FIRST_MEMORY + kind.id()
}

/// `f` run on a `Cx` of a frog at `at` on `pond`.
fn with_cx<R>(pond: Pond, at: Vec3, f: impl FnOnce(&mut Cx<'_, '_, '_>) -> R) -> R {
    let mut pet = Pet::new(pond, FROG, at);
    pet.with_tick(|tick| {
        let mut setup = Setup::default();
        tick.mob_type.register_goals(&mut setup);
        let config = setup.config;
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: Default::default(),
            navigation_ticked: true,
        };
        let mut memories = Memories::default();
        let mut cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time: 0,
        };
        f(&mut cx)
    })
    .0
}

// ------------------------------------------------------------------------------------
// What each brain is made of.

/// The memories a brain has are those its sensors require and its behaviours' entry
/// conditions name (`Brain.<init>`, the brief's 7.1.1 finding 11), from the dumps'
/// providers and activities: an unregistered memory takes no write, so each of these
/// must be there for the game's rules to work (the hunting cooldown the fight's end
/// sets, the pregnancy a breeding sets).
#[test]
fn each_brain_registers_the_memories_its_sensors_and_its_behaviours_name() {
    use MemoryKind as M;
    let registered = |provider: &'static Provider| -> BTreeSet<MemoryKind> {
        Brain::read(provider, &[], None, 0).memories.registered
    };
    let shared = [
        M::Mobs,
        M::VisibleMobs,
        M::HurtBy,
        M::HurtByEntity,
        M::TemptingPlayer,
        M::LookTarget,
        M::WalkTarget,
        M::Path,
        M::CantReachWalkTargetSince,
        M::TemptationCooldownTicks,
        M::BreedTarget,
        M::IsPanicking,
    ];
    let with = |more: &[MemoryKind]| -> BTreeSet<MemoryKind> {
        shared.iter().chain(more).copied().collect()
    };
    assert_eq!(
        registered(&axolotl_ai::PROVIDER),
        with(&[
            M::NearestVisibleAdult,
            M::NearestAttackable,
            M::HasHuntingCooldown,
            M::PlayDeadTicks,
            M::AttackTarget,
            M::AttackCoolingDown,
        ])
    );
    assert_eq!(
        registered(&frog_ai::PROVIDER),
        with(&[
            M::NearestAttackable,
            M::UnreachableTongueTargets,
            M::IsInWater,
            M::LongJumpCoolingDown,
            M::LongJumpMidJump,
            M::AttackTarget,
            M::IsPregnant,
        ])
    );
    assert_eq!(
        registered(&tadpole_ai::PROVIDER),
        with(&[
            M::NearestPlayers,
            M::NearestVisiblePlayer,
            M::NearestVisibleTargetablePlayer,
            M::NearestVisibleTargetablePlayers,
        ])
    );
}

/// `LivingEntity.<init>`'s `makeBrain`: one `nextInt(20)` of the mob's own random per
/// sensor, in the provider's order (the axolotl's and the frog's five, the tadpole's
/// four, the brief's 10.7.2), each sensor scanning at the brain's `max(delay, 1)`-th
/// tick; nothing else is drawn of the type's own.
#[test]
fn each_is_made_with_one_draw_of_its_own_for_each_of_its_sensors_in_order() {
    for (kind, sensors) in [(AXOLOTL, 5), (FROG, 5), (TADPOLE, 4)] {
        let mut own = Random::of_entity(SEED, KEY, 0);
        let mut replay = own.clone();
        let mut mob = Mob::fresh();
        brain::construct(of(kind), &mut mob, &mut own);
        let state: BrainState = crate::mobs::part::decode(&mob.mind.brain).unwrap();
        let expected: Vec<u32> = (0..sensors)
            .map(|_| replay.next_int(20).max(1) - 1)
            .collect();
        assert_eq!(state.next_scans, expected, "{kind}");
        assert_eq!(own, replay, "{kind}: nothing more drawn");
        assert_eq!(state.active, None);
    }
}

// ------------------------------------------------------------------------------------
// The axolotl.

/// The brief's 7.4.7: `PLAY_DEAD_TICKS` of 200 is lowered by `ValidatePlayDead` each
/// tick from the next brain tick, so that the synced flag (`> 0`, after the brain) shows
/// for 199 ticks (199 to 1), is down at 0, and the memory is gone in the 201st brain
/// tick, which also leaves playing dead (`useDefaultActivity`). The count is written as
/// it is set and as its lowering begins, and not in between.
#[test]
fn a_play_dead_count_shows_for_199_ticks_and_leaves_playing_dead_as_it_goes() {
    let mut pet = Pet::new(Pond::with_water(), AXOLOTL, [0.5, -3.0, 0.5]);
    pet.write(|memories| memories.set(MemoryKind::PlayDeadTicks, Value::Int(200)));
    let part = memory_part(MemoryKind::PlayDeadTicks);
    let mut flags = Vec::new();
    for tick in 0..=200 {
        let ticked = pet.step();
        flags.push(axolotl::read(&pet.mob).playing_dead);
        let state = pet.state();
        let memories = pet.memories();
        if tick < 200 {
            assert_eq!(state.active, Some(Activity::PlayDead), "{tick}");
            assert_eq!(
                memories.int(MemoryKind::PlayDeadTicks),
                Some(199 - tick),
                "{tick}"
            );
            assert_eq!(ticked.changed.contains(&part), tick == 0, "{tick}");
        } else {
            assert_eq!(state.active, Some(Activity::Idle));
            assert!(memories.check(MemoryKind::PlayDeadTicks, Status::Absent));
        }
    }
    assert!(flags[..199].iter().all(|flag| *flag));
    assert!(!flags[199] && !flags[200]);
}

/// `EraseMemoryIf(isBreeding, PLAY_DEAD_TICKS)` comes before `ValidatePlayDead`, so a
/// breeding forgets the count and nothing leaves playing dead: the game's axolotl stays in
/// it, its flag down, until a hurt sets a count again.
#[test]
fn a_breeding_forgets_playing_dead_and_leaves_the_axolotl_in_it_as_the_game_does() {
    let mut pet = Pet::new(Pond::with_water(), AXOLOTL, [0.5, -3.0, 0.5]);
    pet.pond
        .put_mob(EntityId(41), AXOLOTL, [2.5, -3.0, 0.5], 14.0);
    pet.write(|memories| {
        memories.set(MemoryKind::PlayDeadTicks, Value::Int(200));
        memories.set(MemoryKind::BreedTarget, Value::Entity(EntityId(41)));
    });
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::PlayDead));
    for _ in 0..30 {
        pet.step();
        assert_eq!(pet.state().active, Some(Activity::PlayDead));
        assert!(!axolotl::read(&pet.mob).playing_dead);
        assert!(
            pet.memories()
                .check(MemoryKind::PlayDeadTicks, Status::Absent)
        );
    }
}

/// `updateActivity`: an attack target makes it fight; once the fight's target is
/// forgotten (here one dead) it idles again with a hunting cooldown of 2400 ticks, and
/// the fight's end forgets the target.
#[test]
fn a_fight_that_ends_leaves_a_hunting_cooldown_of_2400_ticks() {
    let mut pet = Pet::new(Pond::with_water(), AXOLOTL, [0.5, -3.0, 0.5]);
    let fish = EntityId(43);
    pet.pond.put_mob(
        fish,
        entity_types::TROPICAL_FISH as u16,
        [3.5, -3.0, 0.5],
        3.0,
    );
    pet.write(|memories| memories.set(MemoryKind::AttackTarget, Value::Entity(fish)));
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Fight));
    assert!(
        pet.memories()
            .check(MemoryKind::HasHuntingCooldown, Status::Absent)
    );
    // The fish dies: `StopAttackingIfTargetInvalid` forgets it, and the fight ends.
    pet.pond.entities.clear();
    pet.pond.put_mob(
        fish,
        entity_types::TROPICAL_FISH as u16,
        [3.5, -3.0, 0.5],
        0.0,
    );
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Idle));
    let memories = pet.memories();
    assert!(memories.check(MemoryKind::AttackTarget, Status::Absent));
    assert!(memories.check(MemoryKind::HasHuntingCooldown, Status::Present));
    // Set at its tick for 2400 more: read after it, 2400 of them are left.
    assert_eq!(memories.expires_in(MemoryKind::HasHuntingCooldown), 2400);
}

/// `AxolotlAttackablesSensor`: what it hunts within 8 blocks and in water, and not while
/// it has a hunting cooldown; nothing out of water or farther.
#[test]
fn an_axolotl_senses_what_it_hunts_within_eight_blocks_in_water_unless_it_has_hunted_lately() {
    let fish = EntityId(43);
    let sensed = |at: Vec3, cooling: bool| {
        let mut pond = Pond::with_water();
        pond.put_mob(fish, entity_types::TROPICAL_FISH as u16, at, 3.0);
        let mut pet = Pet::new(pond, AXOLOTL, [0.5, -3.0, 0.5]);
        if cooling {
            pet.write(|memories| {
                memories.set_with_expiry(MemoryKind::HasHuntingCooldown, Value::Bool(true), 2400);
            });
        }
        pet.step();
        pet.sensed(MemoryKind::NearestAttackable)
    };
    assert_eq!(sensed([6.5, -3.0, 0.5], false), Some(Value::Entity(fish)));
    assert_eq!(sensed([6.5, -3.0, 0.5], true), None, "it hunted lately");
    assert_eq!(sensed([0.5, -3.0, 8.7], false), None, "beyond 8 blocks");
    assert_eq!(sensed([6.5, 0.0, 0.5], false), None, "out of water");
    // Whom it hunts, in the game's `#axolotl_hunt_targets`.
    for kind in [
        entity_types::TROPICAL_FISH,
        entity_types::PUFFERFISH,
        entity_types::SALMON,
        entity_types::COD,
        entity_types::SQUID,
        entity_types::GLOW_SQUID,
        entity_types::TADPOLE,
    ] {
        assert!(
            clustine_blocks::data::entity_in_tag(kind, "minecraft:axolotl_hunt_targets"),
            "{kind}"
        );
    }
}

/// `updateActivity` while it plays dead: nothing changes its activity but the end of the
/// count; a fight's target does not take it out of playing dead.
#[test]
fn playing_dead_is_left_only_by_its_count_s_end() {
    let mut pet = Pet::new(Pond::with_water(), AXOLOTL, [0.5, -3.0, 0.5]);
    let fish = EntityId(43);
    pet.pond.put_mob(
        fish,
        entity_types::TROPICAL_FISH as u16,
        [3.5, -3.0, 0.5],
        3.0,
    );
    pet.write(|memories| memories.set(MemoryKind::PlayDeadTicks, Value::Int(3)));
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::PlayDead));
    pet.write(|memories| memories.set(MemoryKind::AttackTarget, Value::Entity(fish)));
    // Lowered to 1 and to 0 in the next two ticks.
    for _ in 0..2 {
        pet.step();
        assert_eq!(pet.state().active, Some(Activity::PlayDead));
    }
    // The count reads nothing: it is forgotten and the default taken, then the fight.
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Fight));
}

// ------------------------------------------------------------------------------------
// The frog.

/// `FrogAi.updateActivity`: the first of tongue, laying, jumping, swimming and idling
/// whose requirements hold (the brief's 7.5.4): in water it swims; on land with its jump
/// cooled it jumps, else it idles; pregnant it lays.
#[test]
fn a_frog_swims_in_water_jumps_when_cooled_idles_else_and_lays_when_pregnant() {
    let mut swimmer = Pet::new(Pond::with_water(), FROG, [0.5, -3.0, 0.5]);
    swimmer.step();
    assert_eq!(swimmer.state().active, Some(Activity::Swim));

    let mut jumper = Pet::new(Pond::default(), FROG, [0.5, 0.0, 0.5]);
    jumper.step();
    assert_eq!(jumper.state().active, Some(Activity::LongJump));

    let mut idler = Pet::new(Pond::default(), FROG, [0.5, 0.0, 0.5]);
    frog_ai::set_long_jump_cooldown(&mut idler.mob, 120);
    idler.step();
    assert_eq!(idler.state().active, Some(Activity::Idle));
    // Its cooldown is lowered from its first brain tick.
    assert_eq!(
        idler.memories().int(MemoryKind::LongJumpCoolingDown),
        Some(119)
    );

    let mut layer = Pet::new(Pond::default(), FROG, [0.5, 0.0, 0.5]);
    frog_ai::set_long_jump_cooldown(&mut layer.mob, 120);
    frog::update(&mut layer.mob, |frog| frog.pregnant = true);
    layer.step();
    assert_eq!(layer.state().active, Some(Activity::LaySpawn));
    // The breeding's mark is taken into the brain, and the state keeps it no more.
    assert!(!frog::read(&layer.mob).pregnant);
    assert!(
        layer
            .memories()
            .check(MemoryKind::IsPregnant, Status::Present)
    );
}

/// A brain of the frog's croak alone, in its core.
static CROAKER: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldTimed::new(Croak))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `Croak`: a standing frog croaks (`CROAKING`) for 60 ticks of its counter and stands
/// again in the tick after, then may start again at once; each start draws one number.
/// Its counter is written as it starts and as it stops, and not in between.
#[test]
fn a_frog_croaks_for_sixty_ticks_writing_its_counter_only_as_it_starts_and_stops() {
    let mut pet = Pet::new(Pond::default(), FROG, [0.5, 0.0, 0.5]);
    let part = brain::ids::FIRST_BEHAVIOUR;
    let mut poses = Vec::new();
    for tick in 0..=61 {
        let (_, ticked) = pet.brain_tick(&CROAKER);
        poses.push(frog::read(&pet.mob).pose);
        assert_eq!(
            ticked.changed.contains(&part),
            matches!(tick, 0 | 60 | 61),
            "{tick}"
        );
        assert_eq!(ticked.draws, u64::from(matches!(tick, 0 | 61)), "{tick}");
    }
    assert!(poses[..60].iter().all(|pose| *pose == Pose::Croaking));
    assert_eq!(poses[60], Pose::Standing);
    assert_eq!(poses[61], Pose::Croaking);
}

/// `Croak.start` in a liquid takes no pose and keeps its counter: a frog that has croaked
/// to the end stops in the tick it starts; a frog that never croaked counts 60 ticks with
/// no pose.
#[test]
fn a_croak_started_in_water_shows_nothing_and_goes_on_from_the_last_count() {
    let mut pet = Pet::new(Pond::with_water(), FROG, [0.5, -3.0, 0.5]);
    let mut running = 0;
    for _ in 0..70 {
        pet.brain_tick(&CROAKER);
        assert_eq!(frog::read(&pet.mob).pose, Pose::Standing);
        let counter: Croaking =
            crate::mobs::brain::behaviours::read_entry::<(bool, i64, Croaking)>(
                brain::ids::FIRST_BEHAVIOUR,
                1,
                &parts(&pet.mob),
            )
            .2;
        if counter.since.is_some() {
            running += 1;
        }
    }
    assert_eq!(running, 60, "a fresh counter runs its 60 ticks");
    // Now at 60: each start stops at once.
    let counter: (bool, i64, Croaking) = crate::mobs::brain::behaviours::read_entry(
        brain::ids::FIRST_BEHAVIOUR,
        1,
        &parts(&pet.mob),
    );
    assert!(!counter.0);
    for _ in 0..5 {
        pet.brain_tick(&CROAKER);
        let counter: (bool, i64, Croaking) = crate::mobs::brain::behaviours::read_entry(
            brain::ids::FIRST_BEHAVIOUR,
            1,
            &parts(&pet.mob),
        );
        assert!(!counter.0, "stopped in its start's tick");
        assert_eq!(counter.2.value, 60);
    }
}

/// The brain's parts of a mob by id.
fn parts(mob: &Mob) -> BTreeMap<u16, Entry> {
    mob.mind
        .brain
        .iter()
        .map(|entry| (entry.id, entry.clone()))
        .collect()
}

/// A brain of the frog's laying alone, in its core.
static LAYER: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldShot::new(TryLaySpawnOnFluidNearLand))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `TryLaySpawnOnFluidNearLand`: a pregnant frog on its way lays frogspawn over the first
/// water, north, east, south then west of the block under it, with air over it, the
/// block's placing heard and felt; no longer pregnant. With no such water it fires all the
/// same and stays pregnant.
#[test]
fn a_pregnant_frog_lays_frogspawn_on_the_first_water_beside_the_block_under_it() {
    let lay = |water: &[(i32, i32)]| {
        let mut pond = Pond::default();
        for (x, z) in water {
            pond.set(*x, -1, *z, blocks::WATER);
        }
        let mut pet = Pet::new(pond, FROG, [0.5, 0.0, 0.5]);
        pet.write(|memories| {
            memories.set(MemoryKind::IsPregnant, Value::Unit);
            memories.set(
                MemoryKind::WalkTarget,
                Value::Walk {
                    to: Tracker::Block(BlockPos::new(5, 0, 5)),
                    speed: 1.0,
                    close_enough: 0,
                },
            );
        });
        let (_, ticked) = pet.brain_tick(&LAYER);
        (ticked.out.local, pet.memories())
    };
    // East and south: east is the first of them.
    let (local, memories) = lay(&[(1, 0), (0, 1)]);
    let at = BlockPos::new(1, 0, 0);
    assert_eq!(
        local[0],
        Local::SetBlock {
            pos: at,
            state: blocks::FROGSPAWN,
            flags: 3
        }
    );
    assert!(matches!(
        local[1],
        Local::GameEventAt { name: "minecraft:block_place", at: [1.5, 0.5, 0.5], affected: Some(state) }
            if state == blocks::FROGSPAWN
    ));
    assert!(matches!(
        local[2],
        Local::Sound { sound, volume, pitch, .. }
            if sound == clustine_data::sounds::ENTITY_FROG_LAY_SPAWN && volume == 1.0 && pitch == 1.0
    ));
    assert!(memories.check(MemoryKind::IsPregnant, Status::Absent));
    // West alone: west.
    let (local, _) = lay(&[(-1, 0)]);
    assert!(matches!(local[0], Local::SetBlock { pos, .. } if pos == BlockPos::new(-1, 0, 0)));
    // None: nothing laid, still pregnant.
    let (local, memories) = lay(&[]);
    assert!(local.is_empty());
    assert!(memories.check(MemoryKind::IsPregnant, Status::Present));
}

/// `FrogAi.isAcceptableLandingSpot`: never into a fluid or by one; always onto a lily pad
/// or big dripleaf; onto a trapdoor or the air over one; else the default (a block solid
/// to render under it, and a place that costs nothing).
#[test]
fn a_frog_lands_on_lily_pads_and_trapdoors_never_by_water_and_else_on_solid_ground() {
    let at = BlockPos::new(3, 0, 0);
    let landing = |set: &[((i32, i32, i32), BlockState)]| {
        let mut pond = Pond::default();
        for ((x, y, z), state) in set {
            pond.set(*x, *y, *z, *state);
        }
        with_cx(pond, [0.5, 0.0, 0.5], |cx| {
            frog_ai::acceptable_landing(cx, at)
        })
    };
    // On the stone floor.
    assert!(landing(&[]));
    // Over air: nothing solid to land on.
    assert!(!landing(&[((3, -1, 0), blocks::AIR)]));
    // Water under it, at it, or over it.
    assert!(!landing(&[((3, -1, 0), blocks::WATER)]));
    assert!(!landing(&[((3, 0, 0), blocks::WATER)]));
    assert!(!landing(&[((3, 1, 0), blocks::WATER)]));
    // A lily pad at it, over air.
    assert!(landing(&[
        ((3, 0, 0), blocks::LILY_PAD),
        ((3, -1, 0), blocks::AIR)
    ]));
    // A trapdoor at it, and the air over one.
    let trapdoor = BlockState::parse("minecraft:oak_trapdoor[half=bottom,open=false]").unwrap();
    assert!(landing(&[((3, 0, 0), trapdoor)]));
    assert!(landing(&[((3, -1, 0), trapdoor)]));
}

/// A frog tempted by a player holding a slime ball (`FROG_TEMPTATIONS`) is not idle in
/// its jump: `LONG_JUMP` needs no tempting player.
#[test]
fn a_tempted_frog_does_not_long_jump() {
    let mut pond = Pond::default();
    pond.players
        .push(player(9, [4.5, 0.0, 0.5], Some(items::SLIME_BALL)));
    let mut pet = Pet::new(pond, FROG, [0.5, 0.0, 0.5]);
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Idle));
    assert_eq!(
        pet.sensed(MemoryKind::TemptingPlayer),
        Some(Value::Entity(EntityId(9)))
    );
}

// ------------------------------------------------------------------------------------
// The tadpole.

/// `TadpoleAi`: idle always beside its core; a player holding a slime ball within its
/// tempt range is followed at 1.25 and walked to within 2.
#[test]
fn a_tadpole_idles_and_follows_a_player_holding_a_slime_ball() {
    let mut pond = Pond::with_water();
    pond.players
        .push(player(9, [5.5, -3.0, 0.5], Some(items::SLIME_BALL)));
    let mut pet = Pet::new(pond, TADPOLE, [0.5, -3.0, 0.5]);
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Idle));
    let memories = pet.memories();
    let walk = memories.get(MemoryKind::WalkTarget);
    assert!(
        matches!(
            walk,
            Some(Value::Walk { to: Tracker::Entity { key: EntityId(9), .. }, speed, close_enough: 2 })
                if speed == 1.25
        ),
        "{walk:?}"
    );
}

/// `TadpoleAi.updateActivity` sets idle, which needs nothing.
#[test]
fn a_tadpole_s_one_activity_is_idle() {
    let mut pet = Pet::new(Pond::default(), TADPOLE, [0.5, 0.0, 0.5]);
    for _ in 0..3 {
        pet.step();
        assert_eq!(pet.state().active, Some(Activity::Idle));
    }
}

/// `PlayDead`: with a count and the one that hurt it, in water, it starts (one draw, its
/// 200 ticks drawn as `nextInt(1)`), forgetting its walk and look targets; out of water
/// it does not.
#[test]
fn playing_dead_in_water_forgets_where_it_walked_and_looked() {
    let hurter = EntityId(9);
    let set = |memories: &mut Memories| {
        memories.set(MemoryKind::PlayDeadTicks, Value::Int(200));
        memories.set(MemoryKind::HurtByEntity, Value::Entity(hurter));
        memories.set(
            MemoryKind::WalkTarget,
            Value::Walk {
                to: Tracker::Block(BlockPos::new(5, -3, 5)),
                speed: 0.5,
                close_enough: 0,
            },
        );
        memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(Tracker::Block(BlockPos::new(5, -3, 5))),
        );
    };
    let mut wet = Pet::new(Pond::with_water(), AXOLOTL, [0.5, -3.0, 0.5]);
    wet.write(set);
    let (_, first) = wet.brain_tick(&PLAYER_DEAD);
    assert_eq!(first.draws, 1);
    let memories = wet.memories();
    assert!(memories.check(MemoryKind::WalkTarget, Status::Absent));
    assert!(memories.check(MemoryKind::LookTarget, Status::Absent));

    let mut dry = Pet::new(Pond::default(), AXOLOTL, [0.5, 0.0, 0.5]);
    dry.write(set);
    let (_, first) = dry.brain_tick(&PLAYER_DEAD);
    assert_eq!(first.draws, 0);
    assert!(
        dry.memories()
            .check(MemoryKind::WalkTarget, Status::Present)
    );
}

/// A brain of the axolotl's `PlayDead` alone, in its core.
static PLAYER_DEAD: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldTimed::new(axolotl_ai::PlayDead))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `FrogAttackablesSensor`: of the seen, the closest it may eat (`#frog_food`) within 10
/// blocks that its tongue has not lately found out of reach; with one, it starts
/// attacking and takes to its tongue.
#[test]
fn a_frog_senses_what_it_may_eat_within_ten_blocks_and_takes_to_its_tongue() {
    let slime = EntityId(45);
    let slime_kind = entity_types::SLIME as u16;
    let sensed = |at: Vec3, unreachable: bool| {
        let mut pond = Pond::default();
        pond.put_mob(slime, slime_kind, at, 1.0);
        let mut pet = Pet::new(pond, FROG, [0.5, 0.0, 0.5]);
        frog_ai::set_long_jump_cooldown(&mut pet.mob, 1000);
        if unreachable {
            pet.write(|memories| {
                memories.set_with_expiry(
                    MemoryKind::UnreachableTongueTargets,
                    Value::Entities(vec![slime]),
                    100,
                );
            });
        }
        pet.step();
        (pet.sensed(MemoryKind::NearestAttackable), pet)
    };
    let (found, mut pet) = sensed([6.5, 0.0, 0.5], false);
    assert_eq!(found, Some(Value::Entity(slime)));
    assert_eq!(sensed([6.5, 0.0, 0.5], true).0, None, "out of reach lately");
    assert_eq!(sensed([0.5, 0.0, 10.6], false).0, None, "beyond 10 blocks");
    // The idle `StartAttacking` takes it, and the next activity is the tongue.
    assert_eq!(
        pet.memories().get(MemoryKind::AttackTarget),
        Some(Value::Entity(slime))
    );
    pet.step();
    assert_eq!(pet.state().active, Some(Activity::Tongue));
}

/// A brain of the frog's tongue alone, in its core, with the memory of what is out of
/// its reach (which the frog's sensor registers).
static TONGUE: Provider = Provider {
    memories: &[MemoryKind::UnreachableTongueTargets],
    sensors: &[],
    activities: || {
        vec![ActivityData::numbered(
            Activity::Core,
            0,
            vec![Box::new(HeldTimed::new(frog_ai::ShootTongue))],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `ShootTongue` (the brief's 7.5.5): at a target within 1.75 its tongue is shot at once
/// (`FROG_TONGUE` at 2, `USING_TONGUE`), the catch takes seven ticks of its timer (the
/// post-increment) before the eat (`FROG_EAT` at 2, the hit of its attack damage, 10),
/// the eating four more, and in the tick after it stops: the target forgotten and the
/// frog standing.
#[test]
fn a_frog_s_tongue_catches_for_seven_ticks_then_eats_and_stops_after_four() {
    let slime = EntityId(45);
    let mut pond = Pond::default();
    pond.put_mob(slime, entity_types::SLIME as u16, [1.5, 0.0, 0.5], 1.0);
    let mut pet = Pet::new(pond, FROG, [0.5, 0.0, 0.5]);
    pet.write(|memories| memories.set(MemoryKind::AttackTarget, Value::Entity(slime)));
    let sound = |ticked: &Ticked, sound: i32| {
        ticked.out.events.iter().any(|event| {
            matches!(event, crate::mobs::body::Event::Sound { sound: heard, volume, .. }
                if *heard == sound && *volume == 2.0)
        })
    };
    let mut hits = Vec::new();
    for tick in 0..=12 {
        let (_, ticked) = pet.brain_tick(&TONGUE);
        assert_eq!(
            sound(&ticked, clustine_data::sounds::ENTITY_FROG_TONGUE),
            tick == 0,
            "{tick}"
        );
        assert_eq!(
            sound(&ticked, clustine_data::sounds::ENTITY_FROG_EAT),
            tick == 7,
            "{tick}"
        );
        for local in &ticked.out.local {
            // `doHurtTarget` (N9 part S9).
            if let Local::Strike { key, hurt } = local {
                hits.push((tick, *key, hurt.amount.0));
            }
        }
        let pose = frog::read(&pet.mob).pose;
        if tick < 12 {
            assert_eq!(pose, Pose::UsingTongue, "{tick}");
            assert_eq!(frog::read(&pet.mob).tongue_target, Some(slime), "{tick}");
        } else {
            assert_eq!(pose, Pose::Standing);
            assert_eq!(frog::read(&pet.mob).tongue_target, None);
            assert!(
                pet.memories()
                    .check(MemoryKind::AttackTarget, Status::Absent)
            );
        }
    }
    assert_eq!(hits, [(7, slime, 10.0)]);
}

/// `ShootTongue.checkExtraStartConditions`: a target its path ends farther than 1.75 from
/// is forgotten and kept out of reach for 100 ticks, of the five it keeps.
#[test]
fn a_tongue_s_target_out_of_reach_is_forgotten_and_kept_out_of_reach_for_100_ticks() {
    let slime = EntityId(45);
    let mut pond = Pond::default();
    // Walled off: stone round it, two blocks high.
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx != 0 || dz != 0 {
                for y in [0, 1] {
                    pond.set(6 + dx, y, dz, blocks::STONE);
                }
            }
        }
    }
    pond.put_mob(slime, entity_types::SLIME as u16, [6.5, 0.0, 0.5], 1.0);
    let mut pet = Pet::new(pond, FROG, [0.5, 0.0, 0.5]);
    pet.write(|memories| memories.set(MemoryKind::AttackTarget, Value::Entity(slime)));
    pet.brain_tick(&TONGUE);
    let memories = pet.memories();
    assert!(memories.check(MemoryKind::AttackTarget, Status::Absent));
    assert_eq!(
        memories.get(MemoryKind::UnreachableTongueTargets),
        Some(Value::Entities(vec![slime]))
    );
    assert_eq!(
        memories.expires_in(MemoryKind::UnreachableTongueTargets),
        100
    );
}
