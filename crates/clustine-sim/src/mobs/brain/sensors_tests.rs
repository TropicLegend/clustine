//! Tests of the sensors from the game's rules (the brief's 7.1.3 and 7.2; the plan,
//! sections 2.3 and 8 item 1): what each scan writes, their phases from the delays their
//! creation draws, and their sensing again after a take-in, which writes only what the
//! region keeps.

use clustine_data::{blocks, items};
use clustine_world::{BlockPos, EntityId};

use super::behaviours_tests::{COW, Field, KEY, Run, SEED, healthy, player, record};
use super::sensors::{
    HURT_BY, IS_IN_WATER, NEAREST_ADULT, NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS, TemptingSensor,
    hurt_by, visible_mobs,
};
use super::*;
use crate::mobs::body::hurt::DamageType;
use crate::mobs::types::animal;
use crate::region::Random;

static LIVING: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_LIVING_ENTITIES],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The living things within the follow range (16) of the mob's box, nearest first,
/// players among them, the mob itself and things not living left out; with none, `mobs`
/// is erased and `visible_mobs` is still there, empty, as the game's object is.
#[test]
fn the_living_sensor_lists_the_living_nearest_first() {
    let mut field = Field::default();
    field.put(
        record(EntityId(50), COW, [6.5, 0.0, 0.5], &healthy()),
        false,
    );
    field.put(record(EntityId(51), COW, [3.5, 0.0, 0.5], &healthy()), true);
    field.put(
        record(EntityId(52), COW, [40.5, 0.0, 0.5], &healthy()),
        false,
    );
    field.players.push(player(77, [0.5, 0.0, 5.0], None));
    let mut run = Run::on(field, &LIVING, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::Mobs),
        Some(Value::Entities(vec![
            EntityId(51),
            EntityId(77),
            EntityId(50)
        ]))
    );
    assert_eq!(
        visible_mobs(&ticked.brain.memories),
        Some(vec![EntityId(51), EntityId(77), EntityId(50)])
    );
    assert_eq!(ticked.draws, 0, "a scan draws nothing");

    let mut alone = Run::new(&LIVING, [0.5, 0.0, 0.5]);
    let ticked = alone.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::Mobs, Status::Absent)
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::VisibleMobs, Status::Present)
    );
    assert_eq!(visible_mobs(&ticked.brain.memories), Some(Vec::new()));
}

static PLAYERS: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_PLAYERS],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The players within 16 (a sphere about the feet), nearest first, spectators left
/// out; the nearest seen; none a creative player that it may attack.
#[test]
fn the_player_sensor_finds_the_near_and_never_an_attackable_creative() {
    let mut field = Field::default();
    field.players.push(player(77, [10.5, 0.0, 0.5], None));
    field.players.push(player(78, [4.5, 0.0, 0.5], None));
    let mut spectator = player(79, [2.5, 0.0, 0.5], None);
    spectator.spectator = true;
    field.players.push(spectator);
    field.players.push(player(80, [0.5, 0.0, 16.6], None));
    let mut run = Run::on(field, &PLAYERS, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    let memories = &ticked.brain.memories;
    assert_eq!(
        memories.get(MemoryKind::NearestPlayers),
        Some(Value::Entities(vec![EntityId(78), EntityId(77)]))
    );
    assert_eq!(
        memories.get(MemoryKind::NearestVisiblePlayer),
        Some(Value::Entity(EntityId(78)))
    );
    assert!(memories.check(MemoryKind::NearestVisibleTargetablePlayer, Status::Absent));
    assert!(memories.check(MemoryKind::NearestVisibleTargetablePlayers, Status::Absent));
}

static HURTING: Provider = Provider {
    memories: &[],
    sensors: &[&HURT_BY],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A player's hit: `hurt_by` is the player's attack and `hurt_by_entity` the player;
/// past 40 ticks `hurt_by` goes and `hurt_by_entity` stays; a hurter the view does not
/// have is kept ("unknown" is not dead); a dead one is forgotten.
#[test]
fn the_hurt_sensor_remembers_who_hurt_and_forgets_only_the_dead() {
    let mut field = Field::default();
    field.players.push(player(77, [3.5, 0.0, 0.5], None));
    let mut run = Run::on(field, &HURTING, [0.5, 0.0, 0.5]);
    run.mob.body.combat.last_hurt_by = Some((EntityId(77), 0));
    run.mob.body.last_damage.last = Some((player_attack(), Some(EntityId(77)), 0));
    run.mob.body.combat.last_hurt_by_player = Some((77, 0));
    let ticked = run.tick();
    assert_eq!(
        hurt_by(&ticked.brain.memories),
        Some(DamageType::PLAYER_ATTACK)
    );
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::HurtByEntity),
        Some(Value::Entity(EntityId(77)))
    );
    run.lived = 41;
    run.field.players.clear();
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::HurtBy, Status::Absent)
    );
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::HurtByEntity),
        Some(Value::Entity(EntityId(77))),
        "unknown, kept"
    );
    run.lived = 61;
    let mut dead = healthy();
    dead.body.vitals.health = 0.0;
    run.field
        .put(record(EntityId(77), COW, [3.5, 0.0, 0.5], &dead), false);
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::HurtByEntity, Status::Absent)
    );
}

/// Sensing again after a take-in writes only `hurt_by`, never `hurt_by_entity`, and
/// moves no phase.
#[test]
fn sensing_again_after_a_take_in_writes_only_what_the_region_keeps() {
    let mut field = Field::default();
    field.players.push(player(77, [3.5, 0.0, 0.5], None));
    let mut run = Run::on(field, &HURTING, [0.5, 0.0, 0.5]);
    // Its scans at 0, 20, 40...: a take-in at 5 senses again.
    run.tick();
    for _ in 1..5 {
        run.tick();
    }
    run.mob.body.combat.last_hurt_by = Some((EntityId(77), 5));
    run.mob.body.last_damage.last = Some((player_attack(), Some(EntityId(77)), 5));
    run.mob.body.combat.last_hurt_by_player = Some((77, 5));
    run.sensed = None;
    let ticked = run.tick();
    assert_eq!(
        hurt_by(&ticked.brain.memories),
        Some(DamageType::PLAYER_ATTACK)
    );
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::HurtByEntity, Status::Absent)
    );
    assert_eq!(ticked.brain.state.next_scans, [20]);
}

static ADULTS: Provider = Provider {
    memories: &[],
    sensors: &[&NEAREST_LIVING_ENTITIES, &NEAREST_ADULT],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The nearest seen adult of the mob's own type: a nearer baby of it, and a nearer
/// player, passed over.
#[test]
fn the_adult_sensor_finds_the_nearest_adult_of_its_kind() {
    let mut field = Field::default();
    let mut calf = healthy();
    animal::update(&mut calf, |animal| {
        animal.set_age(animal::BABY_START_AGE, 0)
    });
    field.put(record(EntityId(50), COW, [2.5, 0.0, 0.5], &calf), false);
    field.put(
        record(EntityId(51), COW, [5.5, 0.0, 0.5], &healthy()),
        false,
    );
    field.players.push(player(77, [1.5, 0.0, 1.5], None));
    let mut run = Run::on(field, &ADULTS, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::NearestVisibleAdult),
        Some(Value::Entity(EntityId(51)))
    );
}

static TEMPTS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| stack.item == items::WHEAT,
};

static TEMPTING: Provider = Provider {
    memories: &[],
    sensors: &[&TEMPTS],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// The nearest player within the tempt range (10) holding its food in either hand, not
/// riding it; beyond the range, or riding it, none.
#[test]
fn the_tempting_sensor_finds_the_nearest_player_holding_food() {
    let mut field = Field::default();
    field
        .players
        .push(player(77, [8.5, 0.0, 0.5], Some(items::WHEAT)));
    let mut off_hand = player(78, [5.5, 0.0, 0.5], None);
    off_hand.off_hand = Some(clustine_world::ItemStack::new(items::WHEAT, 1));
    field.players.push(off_hand);
    field
        .players
        .push(player(79, [1.5, 0.0, 0.5], Some(items::CARROT)));
    let mut riding = player(80, [0.5, 1.0, 0.5], Some(items::WHEAT));
    riding.vehicle = Some(KEY);
    field.players.push(riding);
    field
        .players
        .push(player(81, [11.0, 0.0, 0.5], Some(items::WHEAT)));
    let mut run = Run::on(field, &TEMPTING, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert_eq!(
        ticked.brain.memories.get(MemoryKind::TemptingPlayer),
        Some(Value::Entity(EntityId(78)))
    );
}

static WATER: Provider = Provider {
    memories: &[],
    sensors: &[&IS_IN_WATER],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `is_in_water` is there in water and erased out of it.
#[test]
fn the_water_sensor_follows_the_water() {
    let mut field = Field::default();
    field.set(BlockPos::new(0, 0, 0), blocks::WATER);
    field.set(BlockPos::new(0, 1, 0), blocks::WATER);
    let mut run = Run::on(field, &WATER, [0.5, 0.0, 0.5]);
    assert!(
        run.tick()
            .brain
            .memories
            .check(MemoryKind::IsInWater, Status::Present)
    );
    run.move_to([5.5, 0.0, 0.5]);
    for _ in 0..19 {
        run.tick();
    }
    assert!(
        run.tick()
            .brain
            .memories
            .check(MemoryKind::IsInWater, Status::Absent)
    );
}

static SCARE: super::sensors::MobSensor = super::sensors::MobSensor {
    rate: 5,
    mob_test: |_, looked| looked.is_player(),
    ready: |cx| cx.ai.on_ground(),
    to_set: MemoryKind::DangerDetectedRecently,
    ttl: 80,
};

static SCARED: Provider = Provider {
    memories: &[MemoryKind::DangerDetectedRecently],
    sensors: &[&NEAREST_LIVING_ENTITIES, &SCARE],
    activities: Vec::new,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// A mob sensor sets its memory with its expiry where a thing of `mobs` passes its test,
/// and erases it where the mob is not ready; sensing again after a take-in it writes
/// nothing, its memory being the record's.
#[test]
fn the_mob_sensor_sets_its_memory_with_its_expiry_and_senses_nothing_again() {
    let mut field = Field::default();
    field.players.push(player(77, [3.5, 0.0, 0.5], None));
    let mut run = Run::on(field, &SCARED, [0.5, 0.0, 0.5]);
    let ticked = run.tick();
    assert_eq!(
        ticked
            .brain
            .memories
            .expires_in(MemoryKind::DangerDetectedRecently),
        80
    );
    run.field.players.clear();
    run.living.body.on_ground = false;
    run.sensed = None;
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::DangerDetectedRecently, Status::Present),
        "sensed again, nothing written of it"
    );
    for _ in 2..5 {
        run.tick();
    }
    let ticked = run.tick();
    assert!(
        ticked
            .brain
            .memories
            .check(MemoryKind::DangerDetectedRecently, Status::Absent),
        "not ready at its next scan"
    );
}

/// A key whose stream's first `nextInt(20)` is `delay`.
fn key_with_delay(delay: u32) -> (EntityId, Random) {
    (1..10_000)
        .map(EntityId)
        .find_map(|key| {
            let stream = Random::of_entity(SEED, key, 0);
            (stream.clone().next_int(20) == delay).then_some((key, stream))
        })
        .expect("a key of each delay")
}

/// A delay of 0 and one of 1 both scan at the brain's first tick, and every 20 after; a
/// delay of k scans at its k-th tick.
#[test]
fn delays_of_nothing_and_one_both_scan_at_the_first_tick() {
    for (delay, first) in [(0, 0), (1, 0), (7, 6)] {
        let (_, mut stream) = key_with_delay(delay);
        let state = made(&HURTING, 0, &mut stream);
        assert_eq!(state.next_scans, [first], "delay {delay}");
        let mut run = Run::new(&HURTING, [0.5, 0.0, 0.5]);
        part::put(&mut run.entries, &state);
        run.mob.body.combat.last_hurt_by = Some((EntityId(77), 0));
        run.mob.body.last_damage.last = Some((player_attack(), Some(EntityId(77)), 0));
        let mut scanned = Vec::new();
        for _ in 0..=first + 20 {
            let now = run.lived;
            let ticked = run.tick();
            if ticked.brain.state.next_scans != [first] && scanned.is_empty() {
                scanned.push(now);
            }
        }
        assert_eq!(scanned, [first], "delay {delay}");
    }
}

/// The registry id of a player's attack.
fn player_attack() -> i32 {
    crate::mobs::body::hurt::DamageType::PLAYER_ATTACK
        .id()
        .expect("a damage type")
}
