//! The villager's brain of N10 (part V1), from the game's rules
//! (`docs/groundwork/n10-brief.md`, cited as V, sections 1.1 to 1.6; D10 B section 9 and
//! C.9): its activities as the game adds them and their conditions, the draws of a calm
//! villager's tick in the game's order, and its walks to and in a village, on a world of
//! the test's own whose village is where the test says.

use std::collections::BTreeMap;

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId};

use super::MobType;
use super::villager::{self, PROVIDER};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::brain::behaviours::rest;
use crate::mobs::brain::{self, Activity, MemoryKind, Provider, Status, Tracker, Value};
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;

const KEY: EntityId = EntityId(40);
const SEED: u64 = 7;

/// Flat land, stone below y = 0, its sky seen everywhere, with blocks the test sets, a
/// time of day, and a village: the sections within the village's reach of `village`,
/// by the Chebyshev distance in sections (`sectionsToVillage`, D10 A.1).
struct Field {
    blocks: BTreeMap<(i32, i32, i32), BlockState>,
    day: u64,
    village: Option<[i32; 3]>,
    entities: Vec<(Entity, Aabb)>,
}

impl Field {
    fn new(day: u64) -> Self {
        Self {
            blocks: BTreeMap::new(),
            day,
            village: None,
            entities: Vec::new(),
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

    fn players(&self) -> Vec<SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }

    fn sections_to_village(&self, section: [i32; 3]) -> i32 {
        let Some(village) = self.village else {
            return crate::region::pois::NO_VILLAGE;
        };
        let distance = (0..3)
            .map(|axis| (section[axis] - village[axis]).abs())
            .max()
            .unwrap_or(0);
        if distance <= 5 {
            distance
        } else {
            crate::region::pois::NO_VILLAGE
        }
    }

    fn day(&self) -> u64 {
        self.day
    }

    fn game_time(&self) -> i64 {
        self.day as i64
    }

    fn motion_blocking_height(&self, _: i32, _: i32) -> Option<i32> {
        Some(0)
    }
}

/// A villager's body standing at `position`, at rest.
fn body(position: Vec3) -> Living {
    let info = EntityTypeInfo::of(entity_types::VILLAGER);
    let body = Body {
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
            kind: entity_types::VILLAGER as u16,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living
}

/// A villager as its construction makes it (its brain's nine sensors' delays drawn).
fn made_villager() -> Mob {
    let mut own = Random::of_generated(SEED, KEY);
    crate::mobs::making::construct(entity_types::VILLAGER as u16, &mut own, 0.0).1
}

/// One `customServerAiStep` of a villager of type `mob_type` at `lived` on `field`: its
/// mob, its body and its stream after it.
fn step(
    field: &Field,
    mob_type: &'static dyn MobType,
    mob: Mob,
    living: Living,
    lived: u32,
) -> (Mob, Living, Random) {
    let mut draws = Random::of_entity(SEED, KEY, lived);
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: KEY,
        kind: entity_types::VILLAGER as u16,
        lived,
        tick_count: lived,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob,
        living,
        yaw: 0.0,
        pitch: 0.0,
        mob_type,
        world: field,
        draws: &mut draws,
        out: MobOutput::default(),
        controlled: None,
        controlled_by_mob: false,
    };
    mob_type.custom_server_ai_step(&mut tick);
    (tick.mob, tick.living, draws)
}

/// The activity beside the core the villager's brain has active.
fn active(mob: &Mob, lived: u32) -> Option<Activity> {
    brain::Brain::read(&PROVIDER, &mob.mind.brain, Some(&Default::default()), lived)
        .state
        .active
}

/// `ShufflingList.shuffle` of children of `weights` by the floats `drawn`: their indices
/// in the order a gate tries them (as `mobs/brain.rs`'s gate sorts them).
fn shuffled(weights: &[u32], drawn: &[f32]) -> Vec<usize> {
    let mut keyed: Vec<(f64, usize)> = weights
        .iter()
        .zip(drawn)
        .enumerate()
        .map(|(index, (weight, r))| {
            let exponent = f64::from(1.0_f32 / *weight as f32);
            (-libm::pow(f64::from(*r), exponent), index)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    keyed.into_iter().map(|(_, index)| index).collect()
}

#[test]
fn the_activities_are_added_as_the_game_adds_them_work_with_a_job_site_and_the_meeting_with_a_meeting_point()
 {
    let activities = (PROVIDER.activities)();
    let order: Vec<Activity> = activities.iter().map(|data| data.activity).collect();
    assert_eq!(
        order,
        [
            Activity::Play,
            Activity::Work,
            Activity::Core,
            Activity::Meet,
            Activity::Rest,
            Activity::Idle,
            Activity::Panic,
            Activity::PreRaid,
            Activity::Raid,
            Activity::Hide,
        ]
    );
    for data in &activities {
        let conditions = match data.activity {
            Activity::Work => vec![(MemoryKind::JobSite, Status::Present)],
            Activity::Meet => vec![(MemoryKind::MeetingPoint, Status::Present)],
            _ => Vec::new(),
        };
        assert_eq!(data.conditions, conditions, "{:?}", data.activity);
        assert!(data.erase.is_empty(), "nothing is erased as one stops");
    }
    // The packages' priorities in the order of their lists (VillagerGoalPackages,
    // V 1.4): what orders the behaviours of one priority and one activity.
    let priorities = |activity: Activity| -> Vec<i32> {
        activities
            .iter()
            .find(|data| data.activity == activity)
            .unwrap()
            .behaviours
            .iter()
            .map(|(priority, _)| *priority)
            .collect()
    };
    assert_eq!(
        priorities(Activity::Core),
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 5, 6, 7, 8, 10, 10, 10, 10
        ]
    );
    assert_eq!(priorities(Activity::Work), [5, 5, 10, 10, 2, 3, 99]);
    assert_eq!(priorities(Activity::Play), [0, 5, 5, 5, 99]);
    assert_eq!(priorities(Activity::Rest), [2, 3, 3, 5, 5, 99]);
    assert_eq!(priorities(Activity::Meet), [2, 10, 10, 2, 3, 3, 3, 5, 99]);
    assert_eq!(priorities(Activity::Idle), [2, 3, 3, 3, 3, 3, 5, 99]);
    assert_eq!(priorities(Activity::Panic), [0, 1, 1, 3, 5]);
    assert_eq!(priorities(Activity::PreRaid), [0, 0, 5, 99]);
    assert_eq!(priorities(Activity::Raid), [0, 0, 2, 5, 99]);
    assert_eq!(priorities(Activity::Hide), [0, 1, 5]);
}

#[test]
fn the_memories_of_its_points_of_interest_are_kept_and_its_sensors_are_the_game_s_nine() {
    let mut mob = made_villager();
    let mut memories = brain::memories_of(&PROVIDER, &mob, 0);
    for kind in [
        MemoryKind::Home,
        MemoryKind::JobSite,
        MemoryKind::PotentialJobSite,
        MemoryKind::MeetingPoint,
    ] {
        memories.set(kind, Value::Block(BlockPos::new(1, 2, 3)));
    }
    brain::write_memories(&PROVIDER, &mut mob, memories);
    let memories = brain::memories_of(&PROVIDER, &mob, 1);
    for kind in [
        MemoryKind::Home,
        MemoryKind::JobSite,
        MemoryKind::PotentialJobSite,
        MemoryKind::MeetingPoint,
    ] {
        assert_eq!(
            memories.get(kind),
            Some(Value::Block(BlockPos::new(1, 2, 3))),
            "{kind:?}"
        );
    }
    let rates: Vec<u32> = PROVIDER
        .sensors
        .iter()
        .map(|sensor| sensor.rate())
        .collect();
    assert_eq!(rates, [20, 20, 20, 20, 20, 20, 20, 40, 200]);
}

#[test]
fn a_calm_idle_villager_s_tick_draws_the_panic_trigger_s_then_the_raid_status_s_then_its_gates_then_its_own_hundred()
 {
    // V 1.6's steady state, on fresh villagers at many moments: the level's `nextInt(1)`
    // of the panic trigger's start, `SetRaidStatus`'s `nextInt(20)`, the idle `RunOne`'s
    // seven list draws and, where `DoNothing` comes before the stroll, its `nextInt(31)`;
    // the full look's ten list draws and its `DoNothing`'s `nextInt(31)` (nothing else
    // there is to look at); the three `AcquirePoi`'s first `nextInt(20)`; then the
    // villager's own `nextInt(100)`.
    let field = Field::new(6000);
    let villager_type = super::of(entity_types::VILLAGER as u16);
    let mut checked = 0;
    for lived in 0..60 {
        let mut expected = Random::of_entity(SEED, KEY, lived);
        expected.next_int(1);
        expected.next_int(20);
        let idle: Vec<f32> = (0..7).map(|_| expected.next_float()).collect();
        // The idle `RunOne`: villager, love, cat, the stroll (3), walk to the look
        // target, jumping on a bed, nothing (6). Only the stroll and `DoNothing` start
        // here; a stroll's 30 draws of land are not what this test follows.
        let order = shuffled(&[2, 1, 1, 1, 1, 1, 1], &idle);
        let first = order
            .iter()
            .copied()
            .find(|index| *index == 3 || *index == 6)
            .unwrap();
        if first == 3 {
            continue;
        }
        expected.next_int(31);
        for _ in 0..10 {
            expected.next_float();
        }
        expected.next_int(31);
        // The three `AcquirePoi` (job site at 6, home and meeting point at 10) each set
        // their first search `nextInt(20)` ticks ahead (V 3.1).
        let searches: Vec<u32> = (0..3).map(|_| expected.next_int(20)).collect();
        expected.next_int(100);
        let mob = made_villager();
        let (after, _, drawn) = step(&field, villager_type, mob, body([0.5, 0.0, 0.5]), lived);
        assert_eq!(drawn, expected, "at {lived}");
        assert_eq!(active(&after, lived + 1), Some(Activity::Idle));

        // Its next tick, both gates still running their waits: only the two level
        // draws, a search that is due, and its own.
        let mut expected = Random::of_entity(SEED, KEY, lived + 1);
        expected.next_int(1);
        expected.next_int(20);
        // A search due by then (the field's clock stands still) draws its next; with
        // nothing to find, nothing more.
        for due in searches {
            if due == 0 {
                expected.next_int(20);
            }
        }
        expected.next_int(100);
        let (_, _, drawn) = step(
            &field,
            villager_type,
            after,
            body([0.5, 0.0, 0.5]),
            lived + 1,
        );
        assert_eq!(drawn, expected, "at {}", lived + 1);
        checked += 1;
    }
    assert!(checked > 10, "{checked}");
}

/// A type with a brain of one idle activity of the behaviours `IDLE` makes.
struct Strolling(&'static Provider);

impl MobType for Strolling {
    fn brain(&self) -> Option<&'static Provider> {
        Some(self.0)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        brain::run(tick, |_, _| {});
    }
}

static VILLAGE_STROLL: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![brain::ActivityData::numbered(
            Activity::Idle,
            0,
            vec![rest::village_bound_random_stroll(0.5)],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

static TO_THE_VILLAGE: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: || {
        vec![brain::ActivityData::numbered(
            Activity::Idle,
            0,
            vec![rest::go_to_closest_village(0.5, 4)],
        )]
    },
    core: &[Activity::Core],
    default: Activity::Idle,
};

static STROLLING: Strolling = Strolling(&VILLAGE_STROLL);
static GOING: Strolling = Strolling(&TO_THE_VILLAGE);

/// The walk target the brain of `provider` holds, a block's.
fn walk_target(provider: &'static Provider, mob: &Mob, lived: u32) -> Option<(BlockPos, i32)> {
    let memories = brain::memories_of(provider, mob, lived);
    match memories.get(MemoryKind::WalkTarget)? {
        Value::Walk {
            to: Tracker::Block(pos),
            close_enough,
            ..
        } => Some((pos, close_enough)),
        _ => None,
    }
}

#[test]
fn outside_a_village_the_stroll_goes_towards_the_nearer_section_and_in_one_anywhere() {
    // A village two sections east (x 32 to 47): from section 0 the nearest section nearer
    // it within two is the one at x 2 (the first of equals), so every walk goes east,
    // towards its middle (`DefaultRandomPos.getPosTowards`, a right angle about it). On
    // flat land only a place drawn at the level of its feet is stable, so that many of
    // its ten tries find none, and then no walk is set.
    let mut field = Field::new(6000);
    field.village = Some([2, 0, 0]);
    let mut found = 0;
    for lived in 0..200 {
        let (after, _, _) = step(
            &field,
            &STROLLING,
            Mob::fresh(),
            body([8.5, 0.0, 8.5]),
            lived,
        );
        let Some((to, close_enough)) = walk_target(&VILLAGE_STROLL, &after, lived + 1) else {
            continue;
        };
        found += 1;
        assert_eq!(close_enough, 0);
        assert!(to.x >= 8, "towards the village at {lived}: {to:?}");
    }
    assert!(found > 10, "{found}");
    // In the village (its own section a village's), a stroll within 10 anywhere: some
    // go west.
    field.village = Some([0, 0, 0]);
    let mut westward = 0;
    for lived in 0..40 {
        let (after, _, _) = step(
            &field,
            &STROLLING,
            Mob::fresh(),
            body([8.5, 0.0, 8.5]),
            lived,
        );
        let (to, _) = walk_target(&VILLAGE_STROLL, &after, lived + 1).unwrap();
        assert!((to.x - 8).abs() <= 10 && (to.z - 8).abs() <= 10);
        if to.x < 8 {
            westward += 1;
        }
    }
    assert!(westward > 5, "{westward}");
}

#[test]
fn go_to_closest_village_walks_nearer_a_village_and_never_starts_in_one() {
    // A village three sections east: a place nearer it than where the villager stands
    // is taken (`LandRandomPos.getPos(15, 7)` liked the nearer), close enough at 4.
    let mut field = Field::new(18_000);
    field.village = Some([3, 0, 0]);
    for lived in 0..20 {
        let (after, _, _) = step(&field, &GOING, Mob::fresh(), body([8.5, 0.0, 8.5]), lived);
        let (to, close_enough) =
            walk_target(&TO_THE_VILLAGE, &after, lived + 1).expect("a place is found");
        assert_eq!(close_enough, 4);
        assert!(to.x >= 8, "{to:?}");
    }
    // In the village, it never starts: no walk target, and nothing drawn.
    field.village = Some([0, 0, 0]);
    let (after, _, drawn) = step(&field, &GOING, Mob::fresh(), body([8.5, 0.0, 8.5]), 0);
    assert_eq!(walk_target(&TO_THE_VILLAGE, &after, 1), None);
    assert_eq!(drawn, Random::of_entity(SEED, KEY, 0));
}

#[test]
fn a_sleeper_is_small_lies_in_its_pose_and_says_nothing() {
    let villager_type = super::of(entity_types::VILLAGER as u16);
    let mut mob = Mob::fresh();
    villager::update(&mut mob, |own| {
        own.sleeping = Some(BlockPos::new(2, -60, 0))
    });
    let size = villager_type.dimensions(&mob).unwrap();
    assert_eq!((size.width, size.height), (0.2, 0.2));
    assert_eq!(villager_type.ambient_sound(&mob), None);
    let values = villager_type.shown(&mob);
    assert!(values.contains(&(6, crate::shown::DataShown::Pose(2), false)));
    assert!(values.contains(&(
        14,
        crate::shown::DataShown::OptionalBlockPos(Some(BlockPos::new(2, -60, 0))),
        false
    )));
}

#[test]
fn a_sleeper_stands_up_beside_its_bed_on_the_side_away_from_its_look() {
    // A bed whose head is at (0, 0, 0) facing north (its foot to the south), on stone:
    // a villager looking east (yaw −90) stands up west of the head, looking at it.
    let mut field = Field::new(0);
    let head = BlockPos::new(0, 0, 0);
    field.blocks.insert((0, 0, 0), bed("head"));
    field.blocks.insert((0, 0, 1), bed("foot"));
    let up = rest::stand_up_position(&field, head, (0, -1), -90.0, 0.6, 1.95).unwrap();
    assert_eq!((up.x, up.y, up.z), (-0.5, 0.0, 0.5));
    let yaw = rest::yaw_to_bed(head, up);
    assert!((yaw - -90.0).abs() < 0.01, "{yaw}");
    // Walled about but for the far end, it stands beyond the foot, the first of the
    // ring's places there.
    for (x, z) in [(-1, 0), (1, 0), (-1, 1), (1, 1), (-1, -1), (0, -1), (1, -1)] {
        field.blocks.insert((x, 0, z), blocks::STONE);
        field.blocks.insert((x, 1, z), blocks::STONE);
    }
    let up = rest::stand_up_position(&field, head, (0, -1), -90.0, 0.6, 1.95).unwrap();
    assert_eq!((up.x, up.z), (-0.5, 2.5));
}

fn bed(part: &str) -> BlockState {
    BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).unwrap()
}
