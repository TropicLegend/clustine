//! Villagers and trading in a region, tested from the specification by someone who did
//! not write the code (N10's X1, part B1): `docs/groundwork/n10-brief.md` (cited as V:
//! sections 0, 1, 3, 4, 5 and 6), `docs/groundwork/n10-dumps.md` (D10: part A's schedule,
//! part B) and the bytecode in `/home/claude/reference/n10-javap/` where the two leave a
//! number open (`Villager-v`'s `shouldRestock`, `catchUpDemand`, `updateSpecialPrices`,
//! `rewardTradeXp`).
//!
//! The worlds are the TNT tests' (`tnt/tests.rs`): a stone floor at y = -61, its top at
//! -60, in the 5 × 5 chunks about the origin's, the middle 3 × 3 ticking; or, where a
//! player's input is needed, the region tests' floor with player 1 standing on it.

use clustine_data::villagers::professions;
use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::trading::{ExactPredicate, ItemCost, Offer, Offers};
use clustine_world::{Entity, EntityChange, EntityId, InChunk, ItemStack, PlayerId};

use super::*;
use crate::api::ClickKind;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::brain::{self, Activity, MemoryKind, Value};
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::gossip::GossipType;
use crate::mobs::types::villager::{self, PROVIDER, VillagerState};
use crate::region::tnt::tests::{add_player, records, world};

const SEED: u64 = 0x0B1_0010;

/// The height villagers stand at: the floor's top.
const Y: i32 = -60;

fn at(x: i32, z: i32) -> BlockPos {
    BlockPos::new(x, Y, z)
}

/// The floor with `placed` on it, everywhere in the 5 × 5 chunks.
fn ground(placed: &[(BlockPos, BlockState)]) -> Region {
    let placed = placed.to_vec();
    world(SEED, move |pos| {
        placed
            .iter()
            .find(|(at, _)| *at == pos)
            .map(|(_, state)| *state)
    })
}

/// A pen of stone three blocks high round x, z = `from` to `to`, with `inside` laid in it.
fn pen(from: i32, to: i32, inside: &[(BlockPos, BlockState)]) -> Vec<(BlockPos, BlockState)> {
    let mut placed = inside.to_vec();
    for x in from..=to {
        for z in from..=to {
            if x == from || x == to || z == from || z == to {
                for y in Y..=Y + 2 {
                    placed.push((BlockPos::new(x, y, z), blocks::STONE));
                }
            }
        }
    }
    placed
}

fn red_bed(part: &str) -> BlockState {
    blocks::RED_BED
        .with_property("part", part)
        .expect("a bed has a part")
}

/// A bed whose head is at `head`, facing north, so its foot is south of it.
fn bed(head: BlockPos) -> [(BlockPos, BlockState); 2] {
    [
        (head, red_bed("head")),
        (BlockPos::new(head.x, head.y, head.z + 1), red_bed("foot")),
    ]
}

fn floor_bell() -> BlockState {
    BlockState::parse("minecraft:bell[attachment=floor,facing=north,powered=false]")
        .expect("a bell's state")
}

fn in_chunk(pos: BlockPos) -> InChunk {
    let (x, z) = pos.in_chunk();
    InChunk::new(x, pos.y, z)
}

/// The free tickets of the point of interest at `pos`, none where there is none.
fn free_tickets(region: &Region, pos: BlockPos) -> Option<u8> {
    region
        .chunk(pos.chunk())
        .expect("a loaded chunk")
        .free_tickets(in_chunk(pos))
}

/// Takes a ticket of the point of interest at `pos`, as a villager's claim would.
fn claim(region: &mut Region, pos: BlockPos) {
    assert!(
        region.acquire_poi(pos, &mut TickOutput::default()),
        "{pos:?} has a free ticket"
    );
}

/// Sets the world's clock: the overworld's time of day `day` and the game time `time`.
fn set_clock(region: &mut Region, day: u64, time: u64) {
    region.clock.day = day;
    region.clock.game_time = time;
}

/// What a villager is made of in these tests.
#[derive(Clone, Default)]
struct Villager {
    shared: Animal,
    own: VillagerState,
    memories: Vec<(MemoryKind, Value)>,
}

impl Villager {
    fn adult() -> Self {
        Self::default()
    }

    fn baby() -> Self {
        let mut shared = Animal::default();
        shared.set_age(animal::BABY_START_AGE, 0);
        Self {
            shared,
            ..Self::default()
        }
    }

    fn of(mut self, profession: i32, level: i32) -> Self {
        self.own.data.profession = profession as u8;
        self.own.data.level = level;
        self
    }

    fn remembering(mut self, kind: MemoryKind, value: Value) -> Self {
        self.memories.push((kind, value));
        self
    }

    fn with(mut self, change: impl FnOnce(&mut VillagerState)) -> Self {
        change(&mut self.own);
        self
    }
}

/// `made` standing at `position` in its chunk, under a key of the region's; its key.
fn put(region: &mut Region, position: [f64; 3], made: &Villager) -> EntityId {
    let chunk = ChunkPos::containing(position[0], position[2]);
    let key = region.take_entity_key(chunk).expect("room and a key");
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.picks_up_loot = true;
    mob.state = animal::write(&made.shared, &made.own);
    let mut remembered = brain::memories_of(&PROVIDER, &mob, 0);
    for (kind, value) in &made.memories {
        remembered.set(*kind, value.clone());
    }
    brain::write_memories(&PROVIDER, &mut mob, remembered);
    let entity = Entity {
        portal: Default::default(),
        key,
        kind: entity_types::VILLAGER as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 0xB100),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
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
    lay(region, &entity);
    key
}

/// A zombie with no mind of its own (`NoAI`) at `position`: it frightens villagers that
/// see it and does nothing else.
fn still_zombie(region: &mut Region, position: [f64; 3]) -> EntityId {
    let chunk = ChunkPos::containing(position[0], position[2]);
    let key = region.take_entity_key(chunk).expect("room and a key");
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.no_ai = true;
    let entity = Entity {
        portal: Default::default(),
        key,
        kind: entity_types::ZOMBIE as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 0xB100),
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
    };
    lay(region, &entity);
    key
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("a loaded chunk")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .map(|(_, entity)| entity.clone())
        .expect("the villager is here")
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).data).expect("a mob")
}

fn own(region: &Region, key: EntityId) -> VillagerState {
    villager::state_of(&mob(region, key)).1
}

fn is_baby(region: &Region, key: EntityId) -> bool {
    villager::state_of(&mob(region, key)).0.is_baby()
}

/// Changes the villager's own state between ticks, as a trade or the like would.
fn change(region: &mut Region, key: EntityId, change: impl FnOnce(&mut VillagerState)) {
    let (chunk, before) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .expect("the villager");
    let mut changed = Mob::decode(&before.data).expect("a mob");
    villager::update(&mut changed, change);
    region.commit_mob(&before, chunk, &changed, &mut TickOutput::default());
}

/// Its memory of `kind` as its last tick left it.
fn memory(region: &Region, key: EntityId, kind: MemoryKind) -> Option<Value> {
    let entity = record(region, key);
    let mob = Mob::decode(&entity.data).expect("a mob");
    brain::memories_of(&PROVIDER, &mob, entity.lived).get(kind)
}

fn remembered_block(region: &Region, key: EntityId, kind: MemoryKind) -> Option<BlockPos> {
    match memory(region, key, kind) {
        Some(Value::Block(pos)) => Some(pos),
        _ => None,
    }
}

fn remembered_time(region: &Region, key: EntityId, kind: MemoryKind) -> Option<i64> {
    match memory(region, key, kind) {
        Some(Value::Long(time)) => Some(time),
        _ => None,
    }
}

/// The activity beside the core its brain has active.
fn active(region: &Region, key: EntityId) -> Option<Activity> {
    let entity = record(region, key);
    let mob = Mob::decode(&entity.data).expect("a mob");
    brain::Brain::read(
        &PROVIDER,
        &mob.mind.brain,
        Some(&Default::default()),
        entity.lived,
    )
    .state
    .active
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

/// Ticks with no inputs until `done` holds, at most `bound` ticks; whether it held.
fn until(region: &mut Region, bound: usize, mut done: impl FnMut(&Region) -> bool) -> bool {
    for _ in 0..bound {
        idle(region);
        if done(region) {
            return true;
        }
    }
    false
}

fn iron_golems(region: &Region) -> usize {
    records(region)
        .iter()
        .filter(|entity| i32::from(entity.kind) == entity_types::IRON_GOLEM)
        .count()
}

// ------------------------------------------------------------------------------------
// The schedule (V 1.2; D10 part A, section 2: each keyframe's value holds from its tick
// until the next keyframe's, on the overworld clock's time of day).

/// A librarian at (7.5, 7.5) whose lectern and bell are beside it and claimed, made
/// fresh so that its first tick reads the schedule (a new brain's last update is -9999).
fn librarian_with_a_lectern_and_a_bell() -> (Region, EntityId) {
    let lectern = at(9, 7);
    let bell = at(5, 7);
    let mut region = ground(&[(lectern, blocks::LECTERN), (bell, floor_bell())]);
    claim(&mut region, lectern);
    claim(&mut region, bell);
    let made = Villager::adult()
        .of(professions::LIBRARIAN, 1)
        .remembering(MemoryKind::JobSite, Value::Block(lectern))
        .remembering(MemoryKind::MeetingPoint, Value::Block(bell));
    let key = put(&mut region, [7.5, -60.0, 7.5], &made);
    (region, key)
}

#[test]
fn an_adult_s_schedule_switches_on_the_very_tick_of_each_keyframe() {
    // The tick a villager sees is the clock's after the tick's step: a region whose
    // clock says `seen - 1` ticks it at `seen`.
    for (seen, expected) in [
        (9, Activity::Rest),
        (10, Activity::Idle),
        (1999, Activity::Idle),
        (2000, Activity::Work),
        (8999, Activity::Work),
        (9000, Activity::Meet),
        (10_999, Activity::Meet),
        (11_000, Activity::Idle),
        (11_999, Activity::Idle),
        (12_000, Activity::Rest),
        (23_999, Activity::Rest),
        (24_009, Activity::Rest),
        (24_010, Activity::Idle),
        (24_000 * 3 + 2000, Activity::Work),
    ] {
        let (mut region, key) = librarian_with_a_lectern_and_a_bell();
        set_clock(&mut region, seen - 1, 100_000 + seen - 1);
        idle(&mut region);
        assert_eq!(active(&region, key), Some(expected), "at {seen}");
    }
}

#[test]
fn a_baby_s_schedule_switches_on_the_very_tick_of_each_keyframe() {
    for (seen, expected) in [
        (9, Activity::Rest),
        (10, Activity::Idle),
        (2999, Activity::Idle),
        (3000, Activity::Play),
        (5999, Activity::Play),
        (6000, Activity::Idle),
        (9999, Activity::Idle),
        (10_000, Activity::Play),
        (11_999, Activity::Play),
        (12_000, Activity::Rest),
        (24_010, Activity::Idle),
    ] {
        let mut region = ground(&[]);
        let key = put(&mut region, [7.5, -60.0, 7.5], &Villager::baby());
        set_clock(&mut region, seen - 1, 100_000 + seen - 1);
        idle(&mut region);
        assert_eq!(active(&region, key), Some(expected), "at {seen}");
    }
}

#[test]
fn the_schedule_is_read_again_only_more_than_twenty_ticks_after_its_last_reading() {
    // Read at 1990 (idle); the work keyframe at 2000 is seen only at the first reading
    // more than 20 ticks on, at 2011 (`Brain.updateActivityFromSchedule`).
    let (mut region, key) = librarian_with_a_lectern_and_a_bell();
    set_clock(&mut region, 1989, 50_000);
    idle(&mut region);
    assert_eq!(active(&region, key), Some(Activity::Idle));
    for seen in 1991..=2010 {
        idle(&mut region);
        assert_eq!(active(&region, key), Some(Activity::Idle), "at {seen}");
    }
    idle(&mut region);
    assert_eq!(active(&region, key), Some(Activity::Work), "at 2011");
}

#[test]
fn a_baby_growing_up_leaves_play_for_the_adult_s_schedule_in_the_tick_it_grows() {
    // `ageBoundaryReached` calls `refreshBrain`, whose new brain reads the schedule at
    // once: at 3500 an adult without a job site idles, where a baby plays.
    let mut region = ground(&[]);
    let mut made = Villager::baby();
    made.shared.set_age(-3, 0);
    let key = put(&mut region, [7.5, -60.0, 7.5], &made);
    set_clock(&mut region, 3499, 3499);
    idle(&mut region);
    assert!(is_baby(&region, key));
    assert_eq!(active(&region, key), Some(Activity::Play));
    assert!(until(&mut region, 5, |region| !is_baby(region, key)));
    assert_eq!(
        active(&region, key),
        Some(Activity::Idle),
        "the adult's idle"
    );
    for _ in 0..40 {
        idle(&mut region);
        assert_eq!(active(&region, key), Some(Activity::Idle));
    }
}

// ------------------------------------------------------------------------------------
// Job sites and professions (V 3.1, 3.2, 3.5; finding 13).

#[test]
fn each_job_site_gives_an_unemployed_villager_its_own_profession() {
    // VillagerProfession's held job sites, by the blocks of their point-of-interest types.
    for (block, profession) in [
        (blocks::BLAST_FURNACE, professions::ARMORER),
        (blocks::SMOKER, professions::BUTCHER),
        (blocks::CARTOGRAPHY_TABLE, professions::CARTOGRAPHER),
        (blocks::BREWING_STAND, professions::CLERIC),
        (blocks::COMPOSTER, professions::FARMER),
        (blocks::BARREL, professions::FISHERMAN),
        (blocks::FLETCHING_TABLE, professions::FLETCHER),
        (blocks::CAULDRON, professions::LEATHERWORKER),
        (blocks::LECTERN, professions::LIBRARIAN),
        (blocks::STONECUTTER, professions::MASON),
        (blocks::LOOM, professions::SHEPHERD),
        (blocks::SMITHING_TABLE, professions::TOOLSMITH),
        (blocks::GRINDSTONE, professions::WEAPONSMITH),
    ] {
        let site = at(11, 7);
        let mut region = ground(&[(site, block)]);
        set_clock(&mut region, 2500, 2500);
        let key = put(&mut region, [7.5, -60.0, 7.5], &Villager::adult());
        let taken = until(&mut region, 1200, |region| {
            i32::from(own(region, key).data.profession) != professions::NONE
        });
        assert!(taken, "{block:?} is never taken");
        assert_eq!(
            i32::from(own(&region, key).data.profession),
            profession,
            "{block:?}"
        );
        assert_eq!(
            remembered_block(&region, key, MemoryKind::JobSite),
            Some(site)
        );
        assert_eq!(free_tickets(&region, site), Some(0), "{block:?} is claimed");
    }
}

#[test]
fn a_nitwit_never_takes_a_job_site() {
    let site = at(9, 7);
    let mut region = ground(&[(site, blocks::LECTERN)]);
    set_clock(&mut region, 2500, 2500);
    let key = put(
        &mut region,
        [7.5, -60.0, 7.5],
        &Villager::adult().of(professions::NITWIT, 1),
    );
    for _ in 0..600 {
        idle(&mut region);
        assert_eq!(
            i32::from(own(&region, key).data.profession),
            professions::NITWIT
        );
        assert_eq!(memory(&region, key, MemoryKind::PotentialJobSite), None);
    }
    assert_eq!(free_tickets(&region, site), Some(1), "the lectern is free");
}

fn paper_for_an_emerald(max_uses: i32) -> Offer {
    Offer::new(
        ItemCost {
            item: items::PAPER,
            count: 24,
            components: ExactPredicate::EMPTY,
        },
        None,
        ItemStack::new(items::EMERALD, 1),
        max_uses,
        2,
        0.05,
    )
}

#[test]
fn a_new_librarian_whose_lectern_is_broken_loses_its_offers_and_takes_the_next_site() {
    // The player's reroll: a librarian that never traded (experience 0, level 1) loses
    // its profession with its site, and with it the offers it had drawn (a change of
    // profession clears them); then a composter placed by it makes it a farmer.
    let lectern = at(9, 7);
    let mut region = ground(&[(lectern, blocks::LECTERN)]);
    claim(&mut region, lectern);
    set_clock(&mut region, 2500, 2500);
    let made = Villager::adult()
        .of(professions::LIBRARIAN, 1)
        .remembering(MemoryKind::JobSite, Value::Block(lectern))
        .with(|own| own.offers = Some(Offers(vec![paper_for_an_emerald(16)])));
    let key = put(&mut region, [7.5, -60.0, 7.5], &made);
    for _ in 0..20 {
        idle(&mut region);
    }
    assert_eq!(
        i32::from(own(&region, key).data.profession),
        professions::LIBRARIAN
    );
    region.set_block(lectern, blocks::AIR, &mut TickOutput::default());
    assert!(until(&mut region, 40, |region| {
        i32::from(own(region, key).data.profession) == professions::NONE
    }));
    assert_eq!(memory(&region, key, MemoryKind::JobSite), None);
    assert_eq!(
        own(&region, key).offers,
        None,
        "its offers go with its profession"
    );

    let composter = at(9, 8);
    region.set_block(composter, blocks::COMPOSTER, &mut TickOutput::default());
    assert!(until(&mut region, 1200, |region| {
        i32::from(own(region, key).data.profession) != professions::NONE
    }));
    assert_eq!(
        i32::from(own(&region, key).data.profession),
        professions::FARMER
    );
    assert_eq!(
        remembered_block(&region, key, MemoryKind::JobSite),
        Some(composter)
    );
}

#[test]
fn a_librarian_with_experience_that_lost_its_lectern_takes_only_another_lectern() {
    // An employed villager's `acquirableJobSite` is its own profession's site
    // (VillagerProfession's `register(name, poi, sound)`): a librarian that keeps its
    // profession (it has traded) never reserves a composter, and takes a lectern.
    let composter = at(9, 7);
    let mut region = ground(&[(composter, blocks::COMPOSTER)]);
    set_clock(&mut region, 2500, 2500);
    let made = Villager::adult()
        .of(professions::LIBRARIAN, 2)
        .with(|own| own.xp = 20);
    let key = put(&mut region, [7.5, -60.0, 7.5], &made);
    for _ in 0..600 {
        idle(&mut region);
        assert_eq!(memory(&region, key, MemoryKind::PotentialJobSite), None);
        assert_eq!(memory(&region, key, MemoryKind::JobSite), None);
    }
    assert_eq!(free_tickets(&region, composter), Some(1));
    let lectern = at(9, 9);
    region.set_block(lectern, blocks::LECTERN, &mut TickOutput::default());
    assert!(until(&mut region, 1200, |region| {
        remembered_block(region, key, MemoryKind::JobSite) == Some(lectern)
    }));
    assert_eq!(
        i32::from(own(&region, key).data.profession),
        professions::LIBRARIAN
    );
}

// ------------------------------------------------------------------------------------
// Restock at the job site (finding 22, V 5.5; Villager-v `shouldRestock` 2941–3054,
// `allowedToRestock` 2905–2940, `catchUpDemand` 3055–3110, `restock` 2796–2862).

/// A librarian of level 2 with experience, at work beside its claimed lectern in a pen,
/// whose one offer was used `used` times and whose restock record `restock` sets; the
/// clock at `day` and `time`.
fn librarian_at_work(
    day: u64,
    time: u64,
    used: i32,
    restock: impl FnOnce(&mut VillagerState),
) -> (Region, EntityId) {
    let lectern = at(9, 7);
    let mut region = ground(&pen(4, 14, &[(lectern, blocks::LECTERN)]));
    claim(&mut region, lectern);
    set_clock(&mut region, day, time);
    let mut offer = paper_for_an_emerald(12);
    offer.uses = used;
    let made = Villager::adult()
        .of(professions::LIBRARIAN, 2)
        .remembering(MemoryKind::JobSite, Value::Block(lectern))
        .with(|own| {
            own.xp = 20;
            own.offers = Some(Offers(vec![offer]));
            restock(own);
        });
    let key = put(&mut region, [8.5, -60.0, 7.5], &made);
    (region, key)
}

fn first_offer(region: &Region, key: EntityId) -> Offer {
    own(region, key).offers.expect("offers").0[0].clone()
}

#[test]
fn a_villager_restocks_at_its_job_site_at_most_twice_a_day_and_2400_ticks_apart() {
    let (mut region, key) = librarian_at_work(1990, 1990, 12, |_| {});
    // The first restock comes at work: its demand updated before its uses are reset.
    assert!(
        until(&mut region, 4000, |region| first_offer(region, key).uses
            == 0),
        "it never restocks at its lectern"
    );
    let first = region.clock.game_time as i64;
    let after = own(&region, key);
    assert_eq!(after.restocks_today, 1);
    assert_eq!(after.last_restock, first);
    assert_eq!(first_offer(&region, key).demand, 12);

    // Used up again: the second comes only more than 2400 ticks after the first.
    change(&mut region, key, |own| {
        own.offers.as_mut().expect("offers").0[0].uses = 12;
    });
    assert!(until(&mut region, 5000, |region| {
        first_offer(region, key).uses == 0
    }));
    let second = region.clock.game_time as i64;
    assert!(second > first + 2400, "{first} then {second}");
    assert_eq!(own(&region, key).restocks_today, 2);
    assert_eq!(first_offer(&region, key).demand, 24);

    // A third never comes the same half day, however long it works.
    change(&mut region, key, |own| {
        own.offers.as_mut().expect("offers").0[0].uses = 12;
    });
    while region.clock.day < 9000 {
        idle(&mut region);
        assert_eq!(first_offer(&region, key).uses, 12, "a third restock");
    }
}

#[test]
fn half_a_day_since_the_last_restock_makes_up_the_missed_ones_resetting_uses_before_demand() {
    // 12000 ticks after its last restock `shouldRestock` resets the day's count, and
    // `catchUpDemand` first resets every offer's uses, then updates the demand once for
    // each of the two restocks missed: with no use left, each lowers the demand by the
    // offer's most uses. Nothing is then used, so no restock follows.
    let (mut region, key) = librarian_at_work(1990, 30_000, 12, |own| {
        own.last_restock = 10_000;
        own.restocks_today = 0;
    });
    assert!(
        until(&mut region, 4000, |region| own(region, key).last_restock
            != 10_000),
        "its first check at the lectern never comes"
    );
    let checked = region.clock.game_time as i64;
    let after = own(&region, key);
    assert_eq!(after.last_restock, checked);
    assert_eq!(after.restocks_today, 0);
    let offer = first_offer(&region, key);
    assert_eq!((offer.uses, offer.demand), (0, -24));
}

// ------------------------------------------------------------------------------------
// Beds (V 3.4, finding 14; `AcquirePoi` for homes with `validateBedPoi`, D10 B §9).

#[test]
fn at_dusk_a_homeless_villager_claims_a_free_bed_and_sleeps_in_it() {
    let head = at(5, 5);
    let mut region = ground(&pen(2, 13, &bed(head)));
    set_clock(&mut region, 12_500, 12_500);
    let key = put(&mut region, [10.5, -60.0, 10.5], &Villager::adult());
    assert!(until(&mut region, 600, |region| {
        remembered_block(region, key, MemoryKind::Home) == Some(head)
    }));
    assert_eq!(free_tickets(&region, head), Some(0), "the bed is claimed");
    assert!(until(&mut region, 3000, |region| {
        own(region, key).sleeping == Some(head)
    }));
}

#[test]
fn a_baby_claims_a_bed_too() {
    let head = at(5, 5);
    let mut region = ground(&pen(2, 13, &bed(head)));
    set_clock(&mut region, 12_500, 12_500);
    let key = put(&mut region, [10.5, -60.0, 10.5], &Villager::baby());
    assert!(until(&mut region, 600, |region| {
        remembered_block(region, key, MemoryKind::Home) == Some(head)
    }));
    assert_eq!(free_tickets(&region, head), Some(0));
}

/// The head of the bed of [`bedroom`].
const HEAD: BlockPos = BlockPos::new(2, -60, 0);

/// The region tests' floor with a bed whose head is at [`HEAD`], player 1 standing two
/// blocks east of it, at `day`.
fn bedroom(day: u64) -> Region {
    let mut region = on_ticking_floor_with(&bed(HEAD));
    region.clock.day = day;
    let user = region.players.get_mut(&player(1)).expect("player 1");
    user.pose.position = Vec3::new(4.5, -60.0, 0.5);
    region
}

fn use_the_bed(region: &mut Region) -> TickOutput {
    region.tick(&moves(vec![place(1, HEAD.x, HEAD.y, HEAD.z, Face::Top)]))
}

#[test]
fn a_bed_a_player_sleeps_in_is_not_claimed() {
    let mut region = bedroom(13_000);
    use_the_bed(&mut region);
    assert!(
        region.players[&player(1)].sleeping.is_some(),
        "the player lies down"
    );
    let key = put(&mut region, [8.5, -60.0, 6.5], &Villager::adult());
    for _ in 0..90 {
        idle(&mut region);
        assert!(region.players[&player(1)].sleeping.is_some());
        assert_eq!(memory(&region, key, MemoryKind::Home), None);
    }
    assert_eq!(free_tickets(&region, HEAD), Some(1));
}

#[test]
fn a_player_sleeping_in_a_villager_s_bed_costs_the_villager_its_claim() {
    // `ValidateNearbyPoi(HOME)` (rest 3): its bed occupied while it is awake, it forgets
    // the bed and gives its ticket back, no villager sleeping there.
    let mut region = bedroom(13_000);
    claim(&mut region, HEAD);
    let now = region.clock.game_time as i64;
    let made = Villager::adult()
        .remembering(MemoryKind::Home, Value::Block(HEAD))
        .remembering(MemoryKind::LastWoken, Value::Long(now));
    let key = put(&mut region, [8.5, -60.0, 6.5], &made);
    use_the_bed(&mut region);
    assert!(
        region.players[&player(1)].sleeping.is_some(),
        "the player lies down"
    );
    assert!(until(&mut region, 5, |region| {
        memory(region, key, MemoryKind::Home).is_none()
    }));
    assert_eq!(
        free_tickets(&region, HEAD),
        Some(1),
        "its ticket is given back"
    );
}

#[test]
fn a_villager_got_out_of_bed_lies_down_again_only_a_hundred_ticks_after_it_woke() {
    let mut region = bedroom(13_000);
    claim(&mut region, HEAD);
    let made = Villager::adult().remembering(MemoryKind::Home, Value::Block(HEAD));
    let key = put(&mut region, [1.5, -60.0, 0.5], &made);
    assert!(until(&mut region, 20, |region| own(region, key)
        .sleeping
        .is_some()));
    use_the_bed(&mut region);
    assert!(until(&mut region, 2, |region| own(region, key)
        .sleeping
        .is_none()));
    let woke = remembered_time(&region, key, MemoryKind::LastWoken).expect("it woke");
    assert!(
        until(&mut region, 400, |region| own(region, key)
            .sleeping
            .is_some()),
        "it never lies down again"
    );
    let slept = remembered_time(&region, key, MemoryKind::LastSlept).expect("it slept");
    assert!(slept >= woke + 100, "woke at {woke}, lay down at {slept}");
}

// ------------------------------------------------------------------------------------
// Golems, bells and gossip (V 4.1, 4.4, 4.5; D10 B §1, §2).

#[test]
fn three_villagers_that_slept_summon_one_golem_when_a_zombie_frightens_them() {
    let mut region = ground(&pen(2, 13, &[]));
    set_clock(&mut region, 14_000, 50_000);
    let keys: Vec<EntityId> = [[3.5, -60.0, 3.5], [11.5, -60.0, 3.5], [3.5, -60.0, 11.5]]
        .into_iter()
        .map(|position| {
            let made = Villager::adult().remembering(MemoryKind::LastSlept, Value::Long(49_000));
            put(&mut region, position, &made)
        })
        .collect();
    still_zombie(&mut region, [7.5, -60.0, 7.5]);
    assert!(
        until(&mut region, 400, |region| iron_golems(region) > 0),
        "no golem comes"
    );
    for key in &keys {
        assert!(
            memory(&region, *key, MemoryKind::GolemDetectedRecently).is_some(),
            "every villager in the box knows of the golem"
        );
    }
    for _ in 0..300 {
        idle(&mut region);
    }
    assert_eq!(iron_golems(&region), 1);
}

/// The bell at the origin's block.
const BELL: BlockPos = BlockPos::new(0, -60, 0);

/// The bell rung, then the tick that runs its block event.
fn ring(region: &mut Region) {
    assert!(region.ring_bell(BELL, None, &mut TickOutput::default()));
    idle(region);
}

#[test]
fn a_bell_rung_sends_the_villagers_closer_than_thirty_two_to_hide_and_not_those_beyond() {
    let mut region = ground(&[(BELL, floor_bell())]);
    assert!(region.bell(BELL).is_some(), "the bell's block entity");
    set_clock(&mut region, 6000, 6000);
    // 31.5 blocks from the bell's centre, and 32.7.
    let near = put(&mut region, [22.8, -60.0, 22.8], &Villager::adult());
    let far = put(&mut region, [-15.5, -60.0, 29.0], &Villager::adult());
    idle(&mut region);
    ring(&mut region);
    let rung = region.clock.game_time as i64;
    assert_eq!(
        remembered_time(&region, near, MemoryKind::HeardBellTime),
        Some(rung)
    );
    assert_eq!(memory(&region, far, MemoryKind::HeardBellTime), None);
    assert!(until(&mut region, 2, |region| active(region, near)
        == Some(Activity::Hide)));
    assert_ne!(active(&region, far), Some(Activity::Hide));
}

#[test]
fn a_ring_within_sixty_ticks_of_the_last_search_reaches_only_those_it_found_then() {
    let mut region = ground(&[(BELL, floor_bell())]);
    set_clock(&mut region, 6000, 6000);
    let first = put(&mut region, [5.5, -60.0, 5.5], &Villager::adult());
    ring(&mut region);
    let searched = region.clock.game_time as i64;
    let newcomer = put(&mut region, [6.5, -60.0, 3.5], &Villager::adult());
    for _ in 0..28 {
        idle(&mut region);
    }
    // Thirty ticks on, the old list: the first hears it again, the newcomer does not.
    ring(&mut region);
    let again = region.clock.game_time as i64;
    assert!(again <= searched + 60);
    assert_eq!(
        remembered_time(&region, first, MemoryKind::HeardBellTime),
        Some(again)
    );
    assert_eq!(memory(&region, newcomer, MemoryKind::HeardBellTime), None);
    // More than sixty ticks after the search, a new list.
    while (region.clock.game_time as i64) < searched + 61 {
        idle(&mut region);
    }
    ring(&mut region);
    let late = region.clock.game_time as i64;
    assert_eq!(
        remembered_time(&region, newcomer, MemoryKind::HeardBellTime),
        Some(late)
    );
}

#[test]
fn gossip_about_a_player_passes_to_a_villager_it_meets() {
    // `TradeWithVillager` in idle: the receiver draws ten times from what the other
    // knows; a single entry of minor_negative 100 arrives less its 20 a transfer.
    let disliked = uuid::Uuid::from_u128(0xBAD);
    let mut region = ground(&pen(2, 13, &[]));
    set_clock(&mut region, 6000, 6000);
    let teller = Villager::adult().with(|own| {
        own.gossips.add(disliked, GossipType::MinorNegative, 100);
    });
    let teller = put(&mut region, [5.5, -60.0, 7.5], &teller);
    let hearer = put(&mut region, [9.5, -60.0, 7.5], &Villager::adult());
    assert!(
        until(&mut region, 6000, |region| {
            own(region, hearer).gossips.reputation(disliked) != 0
        }),
        "the gossip never passes"
    );
    assert_eq!(own(&region, hearer).gossips.reputation(disliked), -80);
    assert_eq!(own(&region, teller).gossips.reputation(disliked), -100);
}

// ------------------------------------------------------------------------------------
// Trading (V 5; D10 B §4 and §6; Villager-v `updateSpecialPrices` 3170–3275,
// `rewardTradeXp` 3813–, `shouldIncreaseLevel` 4412–4432).

/// The window's result slot.
const RESULT: i16 = 2;

/// `count` of `item` for an emerald, `max_uses` times, giving `xp`, at `multiplier`.
fn for_an_emerald(item: i32, count: i32, max_uses: i32, xp: i32, multiplier: f32) -> Offer {
    Offer::new(
        ItemCost {
            item,
            count,
            components: ExactPredicate::EMPTY,
        },
        None,
        ItemStack::new(items::EMERALD, 1),
        max_uses,
        xp,
        multiplier,
    )
}

/// A merchant's click by the player `id`, whose entity is `player` standing at `feet`,
/// as the input numbered `serial` applies it.
fn use_merchant(
    region: &mut Region,
    (id, player, feet): (PlayerId, EntityId, [f64; 3]),
    key: EntityId,
    serial: u64,
) -> TickOutput {
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .expect("the merchant");
    let mut output = TickOutput::default();
    region.applying = Some((id, serial));
    region.mob_used_by(&entity, chunk, player, None, true, false, feet, &mut output);
    region.applying = None;
    output
}

/// The offers the window was sent in `output`.
fn offers_sent(output: &TickOutput) -> Vec<Offers> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::MerchantOffers { offers, .. } => Some(offers.clone()),
            _ => None,
        })
        .collect()
}

fn click(region: &mut Region, id: PlayerId, slot: i16, kind: ClickKind) {
    region.menu_click(id, 41, slot, 0, kind, &mut TickOutput::default());
}

/// The window's payments and result.
fn window(region: &Region, id: PlayerId) -> [Option<ItemStack>; 3] {
    let trade = region.players[&id]
        .menu
        .as_ref()
        .and_then(|menu| menu.trade.as_deref())
        .expect("a merchant's window");
    [
        trade.payments[0].clone(),
        trade.payments[1].clone(),
        trade.result.clone(),
    ]
}

/// A farmer of `level` with `xp` and `offers` two blocks from player 1, standing in the
/// TNT tests' world, whose inventory `fill` lays out.
fn farmer_and_player(
    level: i32,
    xp: i32,
    offers: Vec<Offer>,
    fill: impl FnOnce(&mut clustine_world::Inventory),
) -> (Region, (PlayerId, EntityId, [f64; 3]), EntityId) {
    let feet = [0.5, -60.0, 0.5];
    let mut region = ground(&[]);
    let (id, player) = add_player(&mut region, 1, feet, false);
    fill(&mut region.players.get_mut(&id).expect("player 1").inventory);
    let made = Villager::adult()
        .of(professions::FARMER, level)
        .with(|own| {
            own.xp = xp;
            own.offers = Some(Offers(offers));
        });
    let key = put(&mut region, [2.5, -60.0, 0.5], &made);
    (region, (id, player, feet), key)
}

#[test]
fn a_player_s_hit_raises_the_prices_by_the_floor_of_the_reputation_s_share() {
    // A hit is minor_negative 25 about the player: a reputation of -25. Each offer's
    // special price is `-floor(-25 × multiplier)` in floats: 0.05 gives
    // floor(-1.25) = -2, two emeralds' worth of wheat more; 0.2 gives five more.
    let mut region = on_ticking_floor_with(&[]);
    // Of level 2 with experience, so that it keeps its profession without a job site.
    let made = Villager::adult().of(professions::FARMER, 2).with(|own| {
        own.xp = 20;
        own.offers = Some(Offers(vec![
            for_an_emerald(items::WHEAT, 10, 16, 2, 0.05),
            for_an_emerald(items::WHEAT, 10, 16, 2, 0.2),
        ]));
    });
    let key = put(&mut region, [2.5, -60.0, 1.5], &made);
    checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: key })]),
    );
    assert_eq!(own(&region, key).gossips.reputation(player(1).0), -25);
    let opened = checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: key,
                location: [0.0, 0.1, 0.0],
                sneaking: false,
            },
        )]),
    );
    let sent = offers_sent(&opened);
    let [offers] = &sent[..] else {
        panic!("one list of offers: {sent:?}");
    };
    let prices: Vec<(i32, i32)> = offers
        .0
        .iter()
        .map(|offer| (offer.special_price, offer.cost_a().count))
        .collect();
    assert_eq!(prices, [(2, 12), (5, 15)]);
}

#[test]
fn a_trade_raises_the_level_by_one_however_many_thresholds_its_experience_passes() {
    // `rewardTradeXp` adds the offer's experience and asks `shouldIncreaseLevel` once:
    // from level 1 and 8, a trade of 70 reaches 78, past 10 and 70, and makes level 2
    // only; the next makes 3 (148), the next 4 (218).
    let (mut region, who, key) = farmer_and_player(
        1,
        8,
        vec![for_an_emerald(items::WHEAT, 1, 16, 70, 0.05)],
        |inventory| {
            inventory.set(0, Some(ItemStack::new(items::WHEAT, 64)));
        },
    );
    let id = who.0;
    use_merchant(&mut region, who, key, 41);
    region.select_trade(id, 41, 0, &mut TickOutput::default());
    let mut levels = Vec::new();
    for _ in 0..3 {
        click(&mut region, id, RESULT, ClickKind::Pickup);
        let after = own(&region, key);
        levels.push((after.data.level, after.xp));
    }
    assert_eq!(levels, [(2, 78), (3, 148), (4, 218)]);
}

#[test]
fn a_master_s_trade_adds_experience_and_leaves_its_level_at_five() {
    let (mut region, who, key) = farmer_and_player(
        5,
        300,
        vec![for_an_emerald(items::WHEAT, 1, 16, 30, 0.05)],
        |inventory| {
            inventory.set(0, Some(ItemStack::new(items::WHEAT, 64)));
        },
    );
    let id = who.0;
    use_merchant(&mut region, who, key, 41);
    region.select_trade(id, 41, 0, &mut TickOutput::default());
    click(&mut region, id, RESULT, ClickKind::Pickup);
    let after = own(&region, key);
    assert_eq!((after.data.level, after.xp), (5, 330));
}

#[test]
fn with_the_first_payment_empty_the_second_pays_as_the_first() {
    // `updateSellItem`: slot 0 empty, slot 1 is taken as the first payment; the take
    // tries (a, b) then (b, a), so the price comes out of slot 1.
    let (mut region, who, key) = farmer_and_player(
        1,
        0,
        vec![for_an_emerald(items::WHEAT, 20, 16, 0, 0.05)],
        |inventory| {
            inventory.set(0, Some(ItemStack::new(items::WHEAT, 64)));
        },
    );
    let id = who.0;
    use_merchant(&mut region, who, key, 41);
    // The hotbar's first slot is the window's 30th: its wheat carried into slot 1.
    click(&mut region, id, 30, ClickKind::Pickup);
    click(&mut region, id, 1, ClickKind::Pickup);
    assert_eq!(
        window(&region, id),
        [
            None,
            Some(ItemStack::new(items::WHEAT, 64)),
            Some(ItemStack::new(items::EMERALD, 1)),
        ]
    );
    click(&mut region, id, RESULT, ClickKind::Pickup);
    assert_eq!(
        window(&region, id),
        [
            None,
            Some(ItemStack::new(items::WHEAT, 44)),
            Some(ItemStack::new(items::EMERALD, 1)),
        ]
    );
    assert_eq!(first_offer(&region, key).uses, 1);
}

#[test]
fn picking_a_trade_whose_payment_cannot_go_back_moves_nothing() {
    // `tryMoveItems`: a payment that does not fit back into the inventory whole stops
    // the pick at once, before any filling.
    let (mut region, who, key) = farmer_and_player(
        1,
        0,
        vec![for_an_emerald(items::WHEAT, 20, 16, 0, 0.05)],
        |inventory| {
            for slot in 0..36 {
                inventory.set(slot, Some(ItemStack::new(items::STONE, 64)));
            }
            inventory.set(4, Some(ItemStack::new(items::DIRT, 5)));
            inventory.set(9, Some(ItemStack::new(items::WHEAT, 64)));
        },
    );
    let id = who.0;
    use_merchant(&mut region, who, key, 41);
    // The dirt into the first payment, and its slot filled again with stone.
    click(&mut region, id, 34, ClickKind::Pickup);
    click(&mut region, id, 0, ClickKind::Pickup);
    region
        .players
        .get_mut(&id)
        .expect("player 1")
        .inventory
        .set(4, Some(ItemStack::new(items::STONE, 64)));
    region.select_trade(id, 41, 0, &mut TickOutput::default());
    assert_eq!(
        window(&region, id),
        [Some(ItemStack::new(items::DIRT, 5)), None, None]
    );
    assert_eq!(
        region.players[&id].inventory.0[9],
        Some(ItemStack::new(items::WHEAT, 64))
    );
}
