//! Tests of the villager's work and professions (`docs/groundwork/n10-plan.md`, section 6;
//! `n10-brief.md`, section 3), on brains of their own: the villager's packages are part
//! V1's, so each test gives a villager-shaped mob a provider of the behaviours it tests,
//! on a little village of its own whose points of interest follow its blocks and whose
//! tickets the test takes and gives back as a region applies its villagers' locals.

use std::collections::BTreeMap;

use clustine_blocks::data::{number, set};
use clustine_data::{BlockState, EntityTypeInfo, PathType, blocks, entity_types};
use clustine_world::poi::poi_type_of;
use clustine_world::{ChunkPos, Entity};
use uuid::Uuid;

use super::*;
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::brain::{self as brains, ActivityData, MemoryWrite, Provider, Sensed, sensors};
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::types::{MobType, animal};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;

/// The world's seed the villagers' streams are keyed by.
const SEED: u64 = 7;
/// The game time the tests begin at: past 0, which `AcquirePoi` takes for "never".
const START: i64 = 1000;

// ------------------------------------------------------------------------------------
// A little village: stone below y = 0, air above, the blocks a test puts, and the
// tickets taken of its points of interest.

#[derive(Default)]
struct Village {
    blocks: BTreeMap<BlockPos, BlockState>,
    taken: BTreeMap<BlockPos, u8>,
    entities: Vec<(Entity, Aabb)>,
    time: i64,
}

impl Village {
    fn set(&mut self, pos: BlockPos, state: BlockState) {
        self.blocks.insert(pos, state);
    }

    fn records(&self, wanted: &dyn Fn(PoiType) -> bool, occupancy: Occupancy) -> Vec<PoiRecord> {
        self.blocks
            .iter()
            .filter_map(|(pos, state)| Some((*pos, poi_type_of(*state)?)))
            .filter(|(_, kind)| wanted(*kind))
            .map(|(pos, kind)| PoiRecord {
                pos,
                kind,
                free: kind
                    .max_tickets()
                    .saturating_sub(self.taken.get(&pos).copied().unwrap_or(0)),
                own: true,
            })
            .filter(|record| match occupancy {
                Occupancy::HasSpace => record.free > 0,
                Occupancy::IsOccupied => record.free != record.kind.max_tickets(),
                Occupancy::Any => true,
            })
            .collect()
    }
}

impl World for Village {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&pos) {
            Some(state) => *state,
            None if pos.y < 0 => blocks::STONE,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Village {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Village {
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

    fn pois_in_square(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: &dyn Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        self.records(wanted, occupancy)
            .into_iter()
            .filter(|record| (record.pos.x - centre.x).abs() <= r)
            .filter(|record| (record.pos.z - centre.z).abs() <= r)
            .collect()
    }

    fn pois_in_range(
        &self,
        centre: BlockPos,
        r: i32,
        wanted: &dyn Fn(PoiType) -> bool,
        occupancy: Occupancy,
    ) -> Vec<PoiRecord> {
        let most = f64::from(r) * f64::from(r);
        self.pois_in_square(centre, r, wanted, occupancy)
            .into_iter()
            .filter(|record| record.dist_sqr(centre) <= most)
            .collect()
    }

    fn poi_type_at(&self, pos: BlockPos) -> Option<PoiType> {
        poi_type_of(self.blocks.get(&pos).copied()?)
    }

    fn game_time(&self) -> i64 {
        self.time
    }
}

// ------------------------------------------------------------------------------------
// Villagers of a provider of the test's.

/// A villager's body, size and navigation, its brain the provider's.
struct Shaped(&'static Provider);

impl MobType for Shaped {
    /// As `Villager`'s (`mobs/types/villager.rs`): doors open, it floats, it looks 48 ahead.
    fn register_goals(&self, setup: &mut Setup) {
        setup.set_malus(PathType::FireInNeighbor, 16.0);
        setup.set_malus(PathType::Fire, -1.0);
        let navigation = &mut setup.config.navigation;
        navigation.flags.can_open_doors = true;
        navigation.set_can_float(true);
        navigation.set_required_path_length(48.0);
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(self.0)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }
}

/// The core package's point-of-interest and profession behaviours (V 1.4, CORE), each
/// at its priority, with the sensor that lists the villagers about.
fn professions_core() -> Vec<ActivityData> {
    vec![ActivityData {
        activity: Activity::Core,
        behaviours: vec![
            (0, validate_job_site(PROFESSION_NOW)),
            (0, validate_potential_job_site()),
            (2, poi_competitor_scan()),
            (6, acquire_job_site()),
            (7, go_to_potential_job_site(0.5)),
            (8, yield_job_site(0.5)),
            (10, assign_profession_from_job_site()),
            (10, reset_profession()),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }]
}

static PROFESSIONS: Provider = Provider {
    memories: &[],
    sensors: &[&sensors::NEAREST_LIVING_ENTITIES],
    activities: professions_core,
    core: &[Activity::Core],
    default: Activity::Idle,
};
static PROFESSIONAL: Shaped = Shaped(&PROFESSIONS);

/// A farmer's work, as the work package has it (V 1.4, WORK), in the core so that no
/// schedule is needed.
fn farm_work() -> Vec<ActivityData> {
    vec![ActivityData {
        activity: Activity::Core,
        behaviours: vec![
            (5, harvest_farmland()),
            (5, work_at_composter()),
            (6, use_bonemeal()),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }]
}

static FARMING: Provider = Provider {
    memories: &[],
    sensors: &[],
    activities: farm_work,
    core: &[Activity::Core],
    default: Activity::Idle,
};
static FARMER: Shaped = Shaped(&FARMING);

/// A villager on the test's village, as a region keeps it between ticks.
struct Placed {
    key: EntityId,
    mob: Mob,
    living: Living,
    lived: u32,
    kind: &'static Shaped,
    sensed: Option<Sensed>,
}

impl Placed {
    /// An adult villager of `profession` with its feet at `at`, on the ground.
    fn new(key: u64, kind: &'static Shaped, at: Vec3, profession: i32) -> Self {
        let info = EntityTypeInfo::of(entity_types::VILLAGER);
        let body = Body {
            position: at,
            motion: [0.0; 3],
            on_ground: true,
            fall_distance: 0.0,
            width: info.width,
            height: info.height,
            old_position: at,
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
                key: EntityId(key),
                root: EntityId(key),
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
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 20.0;
        mob.body.vitals.air = MAX_AIR;
        villager::update(&mut mob, |own| {
            own.data.profession = u8::try_from(profession).expect("a profession's id");
            own.finalized = true;
        });
        Self {
            key: EntityId(key),
            mob,
            living,
            lived: 0,
            kind,
            sensed: None,
        }
    }

    fn position(&self) -> Vec3 {
        self.living.body.position
    }

    /// Its record as the village shows it to the others.
    fn record(&self) -> (Entity, Aabb) {
        let body = &self.living.body;
        let entity = Entity {
            portal: Default::default(),
            key: self.key,
            kind: entity_types::VILLAGER as u16,
            uuid: Uuid::from_u128(u128::from(self.key.0)),
            position: body.position,
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: body.on_ground,
            hops: 0,
            lived: self.lived,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: self.mob.encode(),
        };
        (
            entity,
            Aabb::of_entity(body.position, body.width, body.height),
        )
    }

    fn own(&self) -> villager::VillagerState {
        villager::state_of(&self.mob).1
    }

    fn memory(&self, kind: MemoryKind) -> Option<Value> {
        recorded_memory(&self.mob.mind.brain, kind)
    }

    /// Memories written as another's tick or the test writes them, by the brain's rules.
    fn write(&mut self, writes: Vec<MemoryWrite>) {
        let provider = self.kind.0;
        let mut memories = brains::memories_of(provider, &self.mob, self.lived);
        for write in writes {
            write.apply(&mut memories);
        }
        brains::write_memories(provider, &mut self.mob, memories);
    }
}

/// What one tick of a villager did: its output, and whether its brain was made again.
struct Ticked {
    out: MobOutput,
    remade: bool,
}

/// One tick of the villager `index` at the village's time, its brain's as a region runs
/// it; the others' records as they are.
fn tick(village: &mut Village, villagers: &mut [Placed], index: usize) -> Ticked {
    village.entities = villagers.iter().map(Placed::record).collect();
    let placed = &mut villagers[index];
    let mut draws = Random::of_entity(SEED, placed.key, placed.lived);
    let mut tick = MobTick {
        sensed: placed.sensed.take(),
        ridden: Default::default(),
        key: placed.key,
        kind: entity_types::VILLAGER as u16,
        lived: placed.lived,
        tick_count: placed.lived,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob: placed.mob.clone(),
        living: placed.living.clone(),
        yaw: 0.0,
        pitch: 0.0,
        mob_type: placed.kind,
        world: &*village,
        draws: &mut draws,
        out: MobOutput::default(),
        controlled: None,
        controlled_by_mob: false,
    };
    let remade = brains::run(&mut tick, |_, _| {});
    placed.mob = tick.mob;
    placed.sensed = tick.sensed;
    placed.lived += 1;
    Ticked {
        out: tick.out,
        remade,
    }
}

/// What the region does with a villager's locals that these tests read: tickets taken
/// and given back, blocks set and broken, and writes into another villager as its holder
/// applies them (`region/villagers_across.rs`, without the re-check of a yield, which is
/// the holder's).
fn apply(village: &mut Village, villagers: &mut [Placed], out: &MobOutput) {
    for local in &out.local {
        match local {
            Local::TakePoi { pos } => {
                let kind = village.poi_type_at(*pos).expect("a point taken is there");
                let taken = village.taken.entry(*pos).or_insert(0);
                assert!(
                    *taken < kind.max_tickets(),
                    "a ticket taken twice at {pos:?}"
                );
                *taken += 1;
            }
            Local::ReleasePoi { pos, .. } => {
                if let Some(taken) = village.taken.get_mut(pos) {
                    *taken = taken.saturating_sub(1);
                }
            }
            Local::SetBlock { pos, state, .. } => village.set(*pos, *state),
            Local::BreakBlock { pos } => {
                village.blocks.remove(pos);
                village.taken.remove(pos);
            }
            Local::Villager { key, used } => {
                let other = villagers
                    .iter_mut()
                    .find(|placed| placed.key == *key)
                    .expect("the villager written into is here");
                match *used {
                    VillagerUse::LoseJobSite { site } => {
                        if other.memory(MemoryKind::JobSite) == Some(Value::Block(site)) {
                            other.write(vec![MemoryWrite::Erase(MemoryKind::JobSite)]);
                        }
                    }
                    VillagerUse::Yield { site, speed, .. } => other.write(vec![
                        MemoryWrite::Set(MemoryKind::PotentialJobSite, Value::Block(site)),
                        MemoryWrite::Set(
                            MemoryKind::WalkTarget,
                            walk(Tracker::Block(site), speed.0, 1),
                        ),
                    ]),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Ticks every villager in turn, `ticks` times, the village's clock running from
/// `START`, until `done` holds after a round; the round it held at.
fn run_until(
    village: &mut Village,
    villagers: &mut [Placed],
    ticks: usize,
    mut done: impl FnMut(&Village, &[Placed]) -> bool,
) -> Option<usize> {
    for round in 0..ticks {
        village.time = START + round as i64;
        for index in 0..villagers.len() {
            let ticked = tick(village, villagers, index);
            apply(village, villagers, &ticked.out);
        }
        if done(village, villagers) {
            return Some(round);
        }
    }
    None
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The lectern the tests' librarians work at, east of where they stand.
const LECTERN: BlockPos = BlockPos::new(3, 0, 0);

fn profession_of(placed: &Placed) -> i32 {
    i32::from(placed.own().data.profession)
}

fn behaviour_parts(placed: &Placed) -> usize {
    placed
        .mob
        .mind
        .brain
        .iter()
        .filter(|entry| entry.id >= ids::FIRST_BEHAVIOUR)
        .count()
}

// ------------------------------------------------------------------------------------
// Professions.

/// V 3.5: "place a lectern by an unemployed villager in the morning and watch it become a
/// librarian within a minute". Its `AcquirePoi` scans after its first wait of up to 19
/// ticks, finds the lectern within 48, paths to it and takes its ticket; within 2 of it,
/// `AssignProfessionFromJobSite` makes it the job site in the same tick, with entity
/// event 14, the first profession that holds a lectern (the librarian's), and the brain
/// made again.
#[test]
fn an_unemployed_villager_beside_a_placed_lectern_becomes_a_librarian() {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    let mut villagers = [Placed::new(
        1,
        &PROFESSIONAL,
        [2.5, 0.0, 0.5],
        professions::NONE,
    )];
    let mut seen = Vec::new();
    let mut took = None;
    for round in 0..60 {
        village.time = START + round;
        let ticked = tick(&mut village, &mut villagers, 0);
        apply(&mut village, &mut villagers, &ticked.out);
        if ticked.out.local.contains(&Local::TakePoi { pos: LECTERN }) {
            took = Some(round);
            seen.push(ticked);
            break;
        }
        assert!(!ticked.remade, "nothing remade before the lectern is taken");
    }
    let round = took.expect("the lectern is taken within a minute");
    assert!(round <= 40, "the first scan comes within 40 ticks: {round}");
    let ticked = &seen[0];
    let placed = &villagers[0];
    assert!(
        ticked.out.events.contains(&Event::Entity(14)),
        "the green sparks"
    );
    assert!(ticked.remade, "the brain is made again");
    assert_eq!(profession_of(placed), professions::LIBRARIAN);
    assert_eq!(
        placed.memory(MemoryKind::JobSite),
        Some(Value::Block(LECTERN))
    );
    assert_eq!(placed.memory(MemoryKind::PotentialJobSite), None);
    assert_eq!(
        village.taken.get(&LECTERN),
        Some(&1),
        "its one ticket is taken"
    );
    // Of the old brain only its saved memories go on: no behaviour keeps a part.
    assert_eq!(behaviour_parts(placed), 0);
}

/// A librarian made as above.
fn librarian() -> (Village, [Placed; 1]) {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    let mut villagers = [Placed::new(
        1,
        &PROFESSIONAL,
        [2.5, 0.0, 0.5],
        professions::NONE,
    )];
    run_until(&mut village, &mut villagers, 60, |_, placed| {
        profession_of(&placed[0]) == professions::LIBRARIAN
    })
    .expect("it becomes a librarian");
    (village, villagers)
}

/// V 3.5: "break it and watch it lose the job". `ValidateNearbyPoi` finds no point at
/// its job site and forgets it (giving back nothing: the record went with the block);
/// `ResetProfession`, with no job site, no experience and at level 1, makes it unemployed
/// in the same tick and its brain made again.
#[test]
fn a_librarian_whose_lectern_is_broken_is_unemployed_again() {
    let (mut village, mut villagers) = librarian();
    village.blocks.remove(&LECTERN);
    village.taken.remove(&LECTERN);
    village.time += 1;
    let ticked = tick(&mut village, &mut villagers, 0);
    assert!(
        !ticked
            .out
            .local
            .iter()
            .any(|local| matches!(local, Local::ReleasePoi { .. })),
        "nothing to give back"
    );
    assert!(ticked.remade);
    assert_eq!(villagers[0].memory(MemoryKind::JobSite), None);
    assert_eq!(profession_of(&villagers[0]), professions::NONE);
}

/// A librarian that has traded keeps its profession when its lectern goes: only one with
/// no experience at level 1 is reset.
#[test]
fn a_librarian_with_experience_keeps_its_profession_when_its_lectern_is_broken() {
    let (mut village, mut villagers) = librarian();
    villager::update(&mut villagers[0].mob, |own| own.xp = 10);
    village.blocks.remove(&LECTERN);
    village.time += 1;
    let ticked = tick(&mut village, &mut villagers, 0);
    assert!(!ticked.remade);
    assert_eq!(villagers[0].memory(MemoryKind::JobSite), None);
    assert_eq!(profession_of(&villagers[0]), professions::LIBRARIAN);
}

/// Two unemployed villagers beside one lectern: its one ticket goes to the first whose
/// scan comes; the other's scans find no point with space, so one librarian is made.
#[test]
fn two_villagers_and_one_lectern_make_one_librarian() {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    let mut villagers = [
        Placed::new(1, &PROFESSIONAL, [2.5, 0.0, 0.5], professions::NONE),
        Placed::new(2, &PROFESSIONAL, [4.5, 0.0, 0.5], professions::NONE),
    ];
    run_until(&mut village, &mut villagers, 200, |_, _| false);
    let librarians: Vec<&Placed> = villagers
        .iter()
        .filter(|placed| profession_of(placed) == professions::LIBRARIAN)
        .collect();
    assert_eq!(librarians.len(), 1);
    assert_eq!(village.taken.get(&LECTERN), Some(&1));
    let other = villagers
        .iter()
        .find(|placed| profession_of(placed) == professions::NONE)
        .expect("one is left unemployed");
    assert_eq!(other.memory(MemoryKind::PotentialJobSite), None);
    assert_eq!(other.memory(MemoryKind::JobSite), None);
}

/// Two librarians whose job site is one lectern, with `xp` each.
fn rivals(xp: [i32; 2]) -> (Village, [Placed; 2]) {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    village.taken.insert(LECTERN, 1);
    village.time = START;
    let mut villagers = [
        Placed::new(1, &PROFESSIONAL, [2.5, 0.0, 0.5], professions::LIBRARIAN),
        Placed::new(2, &PROFESSIONAL, [4.5, 0.0, 0.5], professions::LIBRARIAN),
    ];
    for (placed, xp) in villagers.iter_mut().zip(xp) {
        villager::update(&mut placed.mob, |own| own.xp = xp);
        placed.write(vec![MemoryWrite::Set(
            MemoryKind::JobSite,
            Value::Block(LECTERN),
        )]);
    }
    (village, villagers)
}

/// `PoiCompetitorScan`: of two villagers holding one job site, the one with less
/// experience loses it, whichever of them scans; on a tie the one scanning loses (the
/// later in the reduction wins). A loser that is another villager is written into
/// (`VillagerUse::LoseJobSite`), and no ticket is given back.
#[test]
fn of_two_villagers_holding_one_job_site_the_one_with_less_experience_loses_it() {
    // The one with less experience scans: it loses its own.
    let (mut village, mut villagers) = rivals([2, 5]);
    let ticked = tick(&mut village, &mut villagers, 0);
    assert_eq!(villagers[0].memory(MemoryKind::JobSite), None);
    assert!(
        !ticked
            .out
            .local
            .iter()
            .any(|local| matches!(local, Local::Villager { .. } | Local::ReleasePoi { .. }))
    );

    // The one with more scans: the other is written into.
    let (mut village, mut villagers) = rivals([5, 2]);
    let ticked = tick(&mut village, &mut villagers, 0);
    assert_eq!(
        villagers[0].memory(MemoryKind::JobSite),
        Some(Value::Block(LECTERN))
    );
    assert!(ticked.out.local.contains(&Local::Villager {
        key: EntityId(2),
        used: VillagerUse::LoseJobSite { site: LECTERN },
    }));
    apply(&mut village, &mut villagers, &ticked.out);
    assert_eq!(villagers[1].memory(MemoryKind::JobSite), None);
    assert_eq!(village.taken.get(&LECTERN), Some(&1), "the winner holds it");

    // A tie: the one scanning loses.
    let (mut village, mut villagers) = rivals([3, 3]);
    tick(&mut village, &mut villagers, 0);
    assert_eq!(villagers[0].memory(MemoryKind::JobSite), None);
}

/// A villager of another profession at the same site is no competitor.
#[test]
fn a_villager_of_another_profession_at_the_site_is_no_competitor() {
    let (mut village, mut villagers) = rivals([2, 5]);
    villager::update(&mut villagers[1].mob, |own| {
        own.data.profession = professions::FARMER as u8;
    });
    tick(&mut village, &mut villagers, 0);
    assert_eq!(
        villagers[0].memory(MemoryKind::JobSite),
        Some(Value::Block(LECTERN))
    );
}

/// `YieldJobSite`: an unemployed villager that reserved a lectern hands it to a nearby
/// librarian with no job site that can reach it: it forgets its walk, look and
/// reservation, the librarian is given the site (`VillagerUse::Yield`), and the ticket
/// passes with it, given back by nobody.
#[test]
fn an_unemployed_villager_yields_its_reserved_site_to_a_librarian_without_one() {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    village.taken.insert(LECTERN, 1);
    village.time = START;
    let mut villagers = [
        Placed::new(1, &PROFESSIONAL, [9.5, 0.0, 0.5], professions::NONE),
        Placed::new(2, &PROFESSIONAL, [5.5, 0.0, 0.5], professions::LIBRARIAN),
    ];
    villager::update(&mut villagers[1].mob, |own| own.xp = 3);
    villagers[0].write(vec![MemoryWrite::Set(
        MemoryKind::PotentialJobSite,
        Value::Block(LECTERN),
    )]);
    let ticked = tick(&mut village, &mut villagers, 0);
    assert!(ticked.out.local.contains(&Local::Villager {
        key: EntityId(2),
        used: VillagerUse::Yield {
            site: LECTERN,
            kind: PoiType::Librarian,
            speed: clustine_world::Float(0.5),
        },
    }));
    assert!(
        !ticked
            .out
            .local
            .iter()
            .any(|local| matches!(local, Local::ReleasePoi { .. })),
        "the ticket passes"
    );
    let yielder = &villagers[0];
    for kind in [
        MemoryKind::PotentialJobSite,
        MemoryKind::WalkTarget,
        MemoryKind::LookTarget,
    ] {
        assert_eq!(yielder.memory(kind), None, "{kind:?} forgotten");
    }
    apply(&mut village, &mut villagers, &ticked.out);
    assert_eq!(
        villagers[1].memory(MemoryKind::PotentialJobSite),
        Some(Value::Block(LECTERN))
    );
    assert_eq!(village.taken.get(&LECTERN), Some(&1));
}

/// A villager with no one to yield to keeps its reservation and walks to it
/// (`GoToPotentialJobSite`), and a librarian with experience that already has a job site
/// elsewhere is no taker.
#[test]
fn a_reservation_with_no_taker_is_kept_and_walked_to() {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    village.set(at(7, 0, 4), blocks::LECTERN);
    village.taken.insert(LECTERN, 1);
    village.taken.insert(at(7, 0, 4), 1);
    village.time = START;
    let mut villagers = [
        Placed::new(1, &PROFESSIONAL, [9.5, 0.0, 0.5], professions::NONE),
        Placed::new(2, &PROFESSIONAL, [5.5, 0.0, 0.5], professions::LIBRARIAN),
    ];
    villager::update(&mut villagers[1].mob, |own| own.xp = 3);
    villagers[1].write(vec![MemoryWrite::Set(
        MemoryKind::JobSite,
        Value::Block(at(7, 0, 4)),
    )]);
    villagers[0].write(vec![MemoryWrite::Set(
        MemoryKind::PotentialJobSite,
        Value::Block(LECTERN),
    )]);
    let ticked = tick(&mut village, &mut villagers, 0);
    assert!(
        !ticked
            .out
            .local
            .iter()
            .any(|local| matches!(local, Local::Villager { .. }))
    );
    let placed = &villagers[0];
    assert_eq!(
        placed.memory(MemoryKind::PotentialJobSite),
        Some(Value::Block(LECTERN))
    );
    assert_eq!(
        placed.memory(MemoryKind::WalkTarget),
        Some(walk(Tracker::Block(LECTERN), 0.5, 1))
    );
}

// ------------------------------------------------------------------------------------
// `AcquirePoi`'s retries.

/// `AcquirePoi$JitteredLinearRetry`: each mark draws one `nextInt(40)` and puts the next
/// try 40 to 79 ticks further than the last delay, at most 400 from the mark; it is still
/// valid for 400 ticks after its last try.
#[test]
fn a_retry_backs_off_by_40_to_79_ticks_a_try_up_to_400() {
    let mut draws = Random::of_entity(SEED, EntityId(5), 0);
    let mut expected = draws.clone();
    let mut time = 100;
    let mut retry = Retry::new(&mut draws, time);
    let mut delay = 40 + expected.next_int(40) as i32;
    assert_eq!(retry.delay, delay);
    assert_eq!(retry.next, time + i64::from(delay));
    assert!(!retry.should_retry(retry.next - 1));
    assert!(retry.should_retry(retry.next));
    for _ in 0..20 {
        time = retry.next;
        retry.mark(&mut draws, time);
        delay = (delay + 40 + expected.next_int(40) as i32).min(400);
        assert_eq!(retry.delay, delay);
        assert_eq!(retry.previous, time);
        assert_eq!(retry.next, time + i64::from(delay));
    }
    assert_eq!(retry.delay, 400, "capped");
    assert!(retry.still_valid(time + 399));
    assert!(!retry.still_valid(time + 400));
}

/// Runs `f` with the villager's next tick's step as a behaviour gets it, its memories
/// the record's, at the game time `time`; the memories after.
fn with_cx<R>(
    village: &Village,
    placed: &mut Placed,
    time: i64,
    registered: &[MemoryKind],
    f: impl FnOnce(&mut Cx<'_, '_, '_>) -> R,
) -> (R, MobOutput) {
    let mut draws = Random::of_entity(SEED, placed.key, placed.lived);
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: placed.key,
        kind: entity_types::VILLAGER as u16,
        lived: placed.lived,
        tick_count: placed.lived,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob: placed.mob.clone(),
        living: placed.living.clone(),
        yaw: 0.0,
        pitch: 0.0,
        mob_type: placed.kind,
        world: village,
        draws: &mut draws,
        out: MobOutput::default(),
        controlled: None,
        controlled_by_mob: false,
    };
    let mut setup = Setup::default();
    placed.kind.register_goals(&mut setup);
    let config = setup.config;
    let mut memories = brains::Memories {
        registered: registered.iter().copied().collect(),
        now: placed.lived,
        ..brains::Memories::default()
    };
    let result = {
        let mut ai = crate::mobs::ai::Ai {
            tick: &mut tick,
            config: &config,
            sensing: crate::mobs::ai::sensing::Sensing::default(),
            navigation_ticked: true,
        };
        let mut cx = Cx {
            ai: &mut ai,
            memories: &mut memories,
            time,
        };
        f(&mut cx)
    };
    placed.mob = tick.mob;
    placed.lived += 1;
    (result, tick.out)
}

/// `AcquirePoi` with a lectern no path reaches (in the air, five blocks up): its first
/// call only sets its first scan's time; the scan finds the lectern, its path does not
/// reach it, and the lectern goes on a retry (one `nextInt(40)`); later scans before the
/// retry's time leave it out and search no path; at the retry's time it is marked again
/// (another `nextInt(40)`), later and later up to 400 ticks, until a scan 400 ticks after
/// its last try forgets it and it starts again at 40 to 79. Nothing is reserved.
#[test]
fn a_site_no_path_reaches_is_tried_again_later_and_later() {
    let high = at(3, 5, 0);
    let mut village = Village::default();
    village.set(high, blocks::LECTERN);
    let mut placed = Placed::new(1, &PROFESSIONAL, [0.5, 0.0, 0.5], professions::NONE);
    let behaviour = AcquirePoi::new(
        |_, kind: PoiType| kind.is_job_site(),
        MemoryKind::PotentialJobSite,
        Some(MemoryKind::JobSite),
        true,
        None,
        any_block,
    );
    let registered = [MemoryKind::PotentialJobSite, MemoryKind::JobSite];
    let mut state = Acquiring::default();
    let mut time = START;
    let (fired, _) = with_cx(&village, &mut placed, time, &registered, |cx| {
        behaviour.trigger(&mut state, cx)
    });
    assert!(!fired, "the first call only waits");
    assert!((START..START + 20).contains(&state.next));
    assert!(state.retries.is_empty());

    // The delay of each try the scans made, in order.
    let mut delays = Vec::new();
    while delays.len() < 12 {
        time = state.next.max(time + 1);
        let (fired, out) = with_cx(&village, &mut placed, time, &registered, |cx| {
            let fired = behaviour.trigger(&mut state, cx);
            (fired, cx.memories.get(MemoryKind::PotentialJobSite))
        });
        assert!(fired.0, "a scan fires whether it finds or not");
        assert_eq!(fired.1, None, "nothing reserved");
        assert!(out.local.is_empty(), "nothing taken");
        assert!((20..40).contains(&(state.next - time)), "the next scan");
        let retry = *state.retries.get(&high).expect("the lectern waits");
        if retry.previous == time {
            delays.push(retry.delay);
        }
        assert_eq!(retry.next, retry.previous + i64::from(retry.delay));
    }
    // 40 to 79 first, then each 40 to 79 more, never past 400, and after a try at 400
    // forgotten and begun again.
    assert!((40..80).contains(&delays[0]), "{delays:?}");
    let mut restarted = false;
    for pair in delays.windows(2) {
        let grown = pair[1] - pair[0];
        if pair[0] == 400 {
            assert!((40..80).contains(&pair[1]), "{delays:?}");
            restarted = true;
        } else {
            assert!(pair[1] == 400 || (40..80).contains(&grown), "{delays:?}");
        }
    }
    assert!(delays.contains(&400), "{delays:?}");
    assert!(restarted, "{delays:?}");
}

/// A baby takes no job site (`onlyIfAdult`), and draws nothing for it.
#[test]
fn a_baby_takes_no_job_site_and_draws_nothing_for_it() {
    let mut village = Village::default();
    village.set(LECTERN, blocks::LECTERN);
    let mut placed = Placed::new(1, &PROFESSIONAL, [2.5, 0.0, 0.5], professions::NONE);
    animal::made_baby(&mut placed.mob);
    let behaviour = AcquirePoi::new(
        |_, kind: PoiType| kind.is_job_site(),
        MemoryKind::PotentialJobSite,
        Some(MemoryKind::JobSite),
        true,
        None,
        any_block,
    );
    let registered = [MemoryKind::PotentialJobSite, MemoryKind::JobSite];
    let mut state = Acquiring::default();
    let start = Random::of_entity(SEED, placed.key, placed.lived);
    let ((fired, drawn), _) = with_cx(&village, &mut placed, START, &registered, |cx| {
        let fired = behaviour.trigger(&mut state, cx);
        (fired, cx.ai.tick.draws.draws_since(&start))
    });
    assert!(!fired);
    assert_eq!(drawn, 0);
    assert_eq!(state, Acquiring::default());
}

// ------------------------------------------------------------------------------------
// The farmer.

const FARMLAND_AT: BlockPos = BlockPos::new(1, -1, 0);
const CROP_AT: BlockPos = BlockPos::new(1, 0, 0);

/// A farmer standing in its crop's block, over farmland, its secondary job site known.
fn farm(crop: BlockState, inventory: &[ItemStack]) -> (Village, [Placed; 1]) {
    let mut village = Village::default();
    village.set(FARMLAND_AT, blocks::FARMLAND);
    village.set(CROP_AT, crop);
    let mut placed = Placed::new(1, &FARMER, [1.5, 0.0, 0.5], professions::FARMER);
    villager::update(&mut placed.mob, |own| {
        for (slot, stack) in own.inventory.iter_mut().zip(inventory) {
            *slot = Some(stack.clone());
        }
    });
    placed.write(vec![MemoryWrite::Set(
        MemoryKind::SecondaryJobSite,
        Value::Blocks(vec![FARMLAND_AT]),
    )]);
    (village, [placed])
}

fn wheat(age: u8) -> BlockState {
    set(blocks::WHEAT, "age", &age.to_string())
}

fn count(placed: &Placed, item: i32) -> i32 {
    count_item(&placed.own().inventory, item)
}

/// `HarvestFarmland` (V 3.3, finding 15): a farmer by ripe wheat breaks it with its drops;
/// at its next tick the block is air over farmland and it plants the first of its seeds
/// there (the block, its game event, `crop_planted`'s sound) with one seed less; the
/// young wheat then makes it look for another place, and there is none.
#[test]
fn a_farmer_harvests_ripe_wheat_and_replants_it() {
    let (mut village, mut villagers) = farm(wheat(7), &[ItemStack::new(items::WHEAT_SEEDS, 5)]);
    village.time = START;
    let first = tick(&mut village, &mut villagers, 0);
    assert!(
        first
            .out
            .local
            .contains(&Local::BreakBlock { pos: CROP_AT })
    );
    apply(&mut village, &mut villagers, &first.out);
    assert_eq!(village.blocks.get(&CROP_AT), None, "harvested");

    village.time += 1;
    let second = tick(&mut village, &mut villagers, 0);
    assert!(second.out.local.contains(&Local::SetBlock {
        pos: CROP_AT,
        state: blocks::WHEAT,
        flags: composting::UPDATE_ALL,
    }));
    assert!(second.out.local.iter().any(|local| matches!(
        local,
        Local::GameEventAt {
            name: "minecraft:block_place",
            affected: Some(state),
            ..
        } if *state == blocks::WHEAT
    )));
    assert!(second.out.local.iter().any(|local| matches!(
        local,
        Local::Sound { sound, at, .. }
            if *sound == sounds::ITEM_CROP_PLANT && *at == [1.0, 0.0, 0.0]
    )));
    apply(&mut village, &mut villagers, &second.out);
    assert_eq!(village.blocks.get(&CROP_AT), Some(&blocks::WHEAT));
    assert_eq!(count(&villagers[0], items::WHEAT_SEEDS), 4);

    // Young wheat: another place is looked for, there is none, nothing more is done.
    for _ in 0..5 {
        village.time += 1;
        let later = tick(&mut village, &mut villagers, 0);
        assert!(
            !later
                .out
                .local
                .iter()
                .any(|local| matches!(local, Local::SetBlock { .. } | Local::BreakBlock { .. })),
            "{:?}",
            later.out.local
        );
    }
}

/// A farmer with no seeds harvests and leaves the farmland bare.
#[test]
fn a_farmer_with_no_seeds_harvests_and_plants_nothing() {
    let (mut village, mut villagers) = farm(wheat(7), &[ItemStack::new(items::BREAD, 3)]);
    for round in 0..3 {
        village.time = START + round;
        let ticked = tick(&mut village, &mut villagers, 0);
        assert!(
            !ticked
                .out
                .local
                .iter()
                .any(|local| matches!(local, Local::SetBlock { .. }))
        );
        apply(&mut village, &mut villagers, &ticked.out);
    }
    assert_eq!(village.blocks.get(&CROP_AT), None);
}

/// `UseBonemeal`: at a tenth tick, a farmer with bone meal beside young wheat holds bone
/// meal up, and within 1 of it grows it (`growCrops`: 2 to 5 ages, with flags 2), the
/// sparks of level event 1505 with 15, one bone meal less.
#[test]
fn a_farmer_with_bone_meal_grows_young_wheat() {
    let (mut village, mut villagers) = farm(wheat(1), &[ItemStack::new(items::BONE_MEAL, 3)]);
    // Its tenth tick, so that it may start.
    villagers[0].lived = 10;
    village.time = START;
    let ticked = tick(&mut village, &mut villagers, 0);
    let grown = ticked
        .out
        .local
        .iter()
        .find_map(|local| match local {
            Local::SetBlock { pos, state, flags } if *pos == CROP_AT => Some((*state, *flags)),
            _ => None,
        })
        .expect("the wheat grows");
    let age = number(grown.0, "age").expect("wheat has an age");
    assert!((3..=6).contains(&age), "1 and 2 to 5: {age}");
    assert_eq!(grown.1, composting::UPDATE_CLIENTS);
    assert!(ticked.out.local.contains(&Local::LevelEvent {
        kind: composting::PLANT_GROWTH,
        pos: CROP_AT,
        data: composting::PLANT_GROWTH_DATA,
    }));
    assert_eq!(count(&villagers[0], items::BONE_MEAL), 2);
    assert_eq!(
        villagers[0].mob.body.equipment.slots[usize::from(slots::MAIN_HAND)],
        Some(ItemStack::new(items::BONE_MEAL, 1)),
        "bone meal in hand"
    );
}

/// `WorkAtComposter` (V 3.3): 300 ticks after its last check and half the time, a farmer
/// within 1.73 of its composter bakes its wheat by threes into bread and composts its
/// seeds beyond 10 of each, one by one: the first layer of an empty composter is sure, a
/// later one 30 in 100; level event 1500 with 1 where the level rose; the farmer's work
/// sound.
#[test]
fn a_farmer_at_its_composter_bakes_and_composts() {
    let composter = at(2, 0, 0);
    let mut village = Village::default();
    village.set(composter, blocks::COMPOSTER);
    let mut placed = Placed::new(1, &FARMER, [1.5, 0.0, 0.5], professions::FARMER);
    villager::update(&mut placed.mob, |own| {
        own.inventory[0] = Some(ItemStack::new(items::WHEAT, 7));
        own.inventory[1] = Some(ItemStack::new(items::WHEAT_SEEDS, 15));
    });
    placed.write(vec![MemoryWrite::Set(
        MemoryKind::JobSite,
        Value::Block(composter),
    )]);
    let mut villagers = [placed];
    let mut worked = None;
    for round in 0..40 {
        village.time = START + round;
        let ticked = tick(&mut village, &mut villagers, 0);
        if ticked.out.events.iter().any(|event| {
            matches!(event, Event::Sound { sound, .. } if *sound == sounds::ENTITY_VILLAGER_WORK_FARMER)
        }) {
            worked = Some(ticked);
            break;
        }
    }
    let ticked = worked.expect("it works within a few tries");
    let placed = &villagers[0];
    assert_eq!(count(placed, items::BREAD), 2);
    assert_eq!(count(placed, items::WHEAT), 1);
    assert_eq!(
        count(placed, items::WHEAT_SEEDS),
        10,
        "ten kept, five put in"
    );
    let levels: Vec<u8> = ticked
        .out
        .local
        .iter()
        .filter_map(|local| match local {
            Local::SetBlock { pos, state, .. } if *pos == composter => composting::level(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        levels.first(),
        Some(&1),
        "the first layer is sure: {levels:?}"
    );
    assert!(
        levels.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{levels:?}"
    );
    assert!(levels.len() <= 5);
    assert!(ticked.out.local.contains(&Local::LevelEvent {
        kind: composting::COMPOSTER_FILL,
        pos: composter,
        data: 1,
    }));
}

/// A farmer at a ready composter takes its bone meal out first (dropped above it, the
/// composter emptied with its sound) and then composts into the empty one.
#[test]
fn a_farmer_at_a_ready_composter_takes_the_bone_meal_out_first() {
    let composter = at(2, 0, 0);
    let mut village = Village::default();
    village.set(composter, set(blocks::COMPOSTER, "level", "8"));
    let mut placed = Placed::new(1, &FARMER, [1.5, 0.0, 0.5], professions::FARMER);
    villager::update(&mut placed.mob, |own| {
        own.inventory[0] = Some(ItemStack::new(items::BEETROOT_SEEDS, 11));
    });
    placed.write(vec![MemoryWrite::Set(
        MemoryKind::JobSite,
        Value::Block(composter),
    )]);
    let mut villagers = [placed];
    let mut worked = None;
    for round in 0..40 {
        village.time = START + round;
        let ticked = tick(&mut village, &mut villagers, 0);
        if ticked
            .out
            .local
            .iter()
            .any(|local| matches!(local, Local::Drop { .. }))
        {
            worked = Some(ticked);
            break;
        }
    }
    let ticked = worked.expect("it works within a few tries");
    let dropped = ticked
        .out
        .local
        .iter()
        .find_map(|local| match local {
            Local::Drop { stack, at, .. } => Some((stack.clone(), *at)),
            _ => None,
        })
        .expect("the bone meal");
    assert_eq!(dropped.0, ItemStack::new(items::BONE_MEAL, 1));
    assert!((2.15..=2.85).contains(&dropped.1[0]) && dropped.1[1] == 1.01);
    let levels: Vec<u8> = ticked
        .out
        .local
        .iter()
        .filter_map(|local| match local {
            Local::SetBlock { pos, state, .. } if *pos == composter => composting::level(*state),
            _ => None,
        })
        .collect();
    // Emptied, then the one seed beyond ten makes the first, sure layer.
    assert_eq!(levels, [0, 1]);
    assert!(ticked.out.local.iter().any(|local| matches!(
        local,
        Local::Sound { sound, .. } if *sound == composting::EMPTY_SOUND
    )));
    assert_eq!(count(&villagers[0], items::BEETROOT_SEEDS), 10);
}
