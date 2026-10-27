//! Part V3's social life in a region (`docs/groundwork/n10-plan.md`, section 7, decision
//! 17; `n10-dumps.md`, part B, sections 1 and 2), each number from the game's rules: a
//! player's hit and a zombie's are gossiped about by the attacker's UUID, a murder by
//! the villagers that saw it and not one behind a wall, a trade by the villager after its
//! brain, and a day's gossip decays; a bell rung tells a villager across a line it was
//! heard, and its resonance makes raiders glow on its forty-fifth tick, the region's own
//! and one across the line.

use clustine_data::{BlockState, blocks, entity_types, sounds};
use clustine_world::{
    Causer, Double, Effected, Entity, EntityChange, EntityId, EntityUse, Float, HurtBy, RaiderUse,
    VillagerUse,
};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::gossip::GossipType;
use crate::mobs::types::villager::{self, VillagerState};
use crate::region::tnt::tests::west_of_a_line;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);
const THREE: EntityId = EntityId(44);
const ZOMBIE: EntityId = EntityId(46);
const SEED: u64 = 0x0050_C1A1;

/// A mob of the type `kind` at `position`, standing, of `health`, with the type state
/// `state`.
fn mob_at(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    state: clustine_world::TypeState,
    health: f32,
) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.picks_up_loot = kind == entity_types::VILLAGER;
    mob.state = state;
    Entity {
        key,
        kind: kind as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1000),
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
    }
}

fn villager_at(key: EntityId, position: [f64; 3], own: &VillagerState, health: f32) -> Entity {
    mob_at(
        key,
        entity_types::VILLAGER,
        position,
        animal::write(&Animal::default(), own),
        health,
    )
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// `entity` laid in the region's copy of its chunk.
fn lay_copied(region: &mut Region, entity: &Entity) {
    region
        .copies
        .get_mut(&entity.chunk())
        .expect("the chunk is copied")
        .apply_entity(&EntityChange::Whole(entity.clone()));
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&region.find_entity(key).expect("the mob is here").1.data).unwrap()
}

fn own(region: &Region, key: EntityId) -> VillagerState {
    villager::state_of(&mob(region, key)).1
}

fn idle(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

fn player_uuid(number: u128) -> uuid::Uuid {
    player(number).0
}

#[test]
fn a_player_s_hit_is_gossiped_about_by_the_villager_hit() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(ONE, [2.5, -60.0, 1.5], &VillagerState::default(), 20.0),
    );
    checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: ONE })]),
    );
    assert_eq!(
        own(&region, ONE).gossips.entries,
        [(player_uuid(1), GossipType::MinorNegative, 25)]
    );
}

#[test]
fn a_zombie_s_hit_is_gossiped_about_by_its_uuid_which_the_region_finds() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &villager_at(ONE, [2.5, -60.0, 1.5], &VillagerState::default(), 20.0),
    );
    let zombie = mob_at(
        ZOMBIE,
        entity_types::ZOMBIE,
        [4.5, -60.0, 1.5],
        Default::default(),
        20.0,
    );
    lay(&mut region, &zombie);
    let cause = Causer {
        key: ZOMBIE,
        kind: entity_types::ZOMBIE as u16,
        position: [Double(4.5), Double(-60.0), Double(1.5)],
        player: None,
        living: true,
    };
    let hurt = HurtBy {
        damage_type: "minecraft:mob_attack".to_owned(),
        cause: Some(cause.clone()),
        direct: Some(cause),
        position: None,
        projectile_motion: None,
        amount: Float(2.0),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    };
    let mut output = TickOutput::default();
    region.strike(ONE, Box::new(hurt), &mut output);
    assert_eq!(
        own(&region, ONE).gossips.entries,
        [(zombie.uuid, GossipType::MinorNegative, 25)]
    );
}

#[test]
fn a_murder_is_gossiped_about_by_the_villagers_that_saw_it_and_not_one_behind_a_wall() {
    // A wall of stone at z = 4 between the dying villager and the third.
    let wall: Vec<(BlockPos, BlockState)> = (0..=8)
        .flat_map(|x| (-60..=-56).map(move |y| (BlockPos::new(x, y, 4), blocks::STONE)))
        .collect();
    let mut region = on_ticking_floor_with(&wall);
    region.clock.day = 6000;
    lay(
        &mut region,
        &villager_at(ONE, [2.5, -60.0, 1.5], &VillagerState::default(), 1.0),
    );
    lay(
        &mut region,
        &villager_at(TWO, [5.5, -60.0, 1.5], &VillagerState::default(), 20.0),
    );
    lay(
        &mut region,
        &villager_at(THREE, [2.5, -60.0, 7.5], &VillagerState::default(), 20.0),
    );
    // A tick for each to see the others (its sensor's first scan).
    idle(&mut region);
    checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: ONE })]),
    );
    assert!(mob(&region, ONE).body.vitals.dying(), "it died");
    assert_eq!(
        own(&region, TWO).gossips.entries,
        [(player_uuid(1), GossipType::MajorNegative, 25)]
    );
    assert!(own(&region, THREE).gossips.entries.is_empty());
}

#[test]
fn a_trade_is_gossiped_about_after_the_villager_s_brain_with_entity_event_14() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 6000;
    let traded = VillagerState {
        last_traded_player: Some(player(1)),
        ..VillagerState::default()
    };
    lay(
        &mut region,
        &villager_at(ONE, [2.5, -60.0, 1.5], &traded, 20.0),
    );
    let output = idle(&mut region);
    let own = own(&region, ONE);
    assert_eq!(
        own.gossips.entries,
        [(player_uuid(1), GossipType::Trading, 2)]
    );
    assert_eq!(own.last_traded_player, None);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityEvent {
            entity: ONE,
            event: 14,
            ..
        }
    )));
}

#[test]
fn gossip_decays_once_a_day_of_game_time_after_its_last_decay() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 6000;
    region.clock.game_time = 30_000;
    let someone = uuid::Uuid::from_u128(77);
    let mut gossiping = VillagerState::default();
    gossiping
        .gossips
        .add(someone, GossipType::MinorPositive, 10);
    gossiping
        .gossips
        .add(someone, GossipType::MajorPositive, 20);
    gossiping
        .gossips
        .add(someone, GossipType::MinorNegative, 21);
    gossiping.last_gossip_decay = 6_000;
    lay(
        &mut region,
        &villager_at(ONE, [2.5, -60.0, 1.5], &gossiping, 20.0),
    );
    idle(&mut region);
    let after = own(&region, ONE);
    assert_eq!(
        after.gossips.entries,
        [
            (someone, GossipType::MinorPositive, 9),
            (someone, GossipType::MajorPositive, 20),
        ]
    );
    let decayed_at = after.last_gossip_decay;
    assert!(
        decayed_at >= 30_000,
        "stamped with the game time: {decayed_at}"
    );
    // Not again within the day.
    idle(&mut region);
    assert_eq!(own(&region, ONE).gossips, after.gossips);
}

/// The bell of these tests, west of the line, on the floor facing north.
const BELL: BlockPos = BlockPos::new(14, -60, 2);

fn floor_bell() -> BlockState {
    BlockState::parse("minecraft:bell[attachment=floor,facing=north,powered=false]")
        .expect("a bell's state")
}

#[test]
fn a_bell_tells_a_villager_across_the_line_and_its_resonance_makes_raiders_glow() {
    let bell = floor_bell();
    let mut region = west_of_a_line(SEED, move |pos| (pos == BELL).then_some(bell));
    assert!(region.bell(BELL).is_some(), "the bell's block entity");
    // A villager across the line within 32 of the bell, one beyond 32 but in its box,
    // a pillager across the line and one of the region's own.
    let near = EntityId(70_001);
    let far = EntityId(70_002);
    let across = EntityId(70_003);
    lay_copied(
        &mut region,
        &villager_at(near, [20.5, -60.0, 2.5], &VillagerState::default(), 20.0),
    );
    lay_copied(
        &mut region,
        &villager_at(far, [20.5, -60.0, 40.5], &VillagerState::default(), 20.0),
    );
    let pillager = |key, at| mob_at(key, entity_types::PILLAGER, at, Default::default(), 24.0);
    lay_copied(&mut region, &pillager(across, [24.5, -60.0, 2.5]));
    let ours = region.take_entity_key(ChunkPos::new(0, 0)).expect("a key");
    lay(&mut region, &pillager(ours, [10.5, -60.0, 2.5]));
    let mut output = TickOutput::default();
    assert!(region.ring_bell(BELL, None, &mut output));
    // The ring's block event runs in the next tick: the near villager hears it, by its
    // holder, at that tick's game time.
    let output = region.tick(&TickInputs::default());
    let heard: Vec<(EntityId, u64)> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity {
                key,
                by: EntityUse::Villager(VillagerUse::Heard { at }),
            } => Some((*key, *at)),
            _ => None,
        })
        .collect();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].0, near);
    assert!(heard[0].1 > 0);
    // Its fifth tick of shaking starts the resonance, heard once.
    let mut resonances = 0;
    let mut glowing_sent = false;
    for tick in 2..=45 {
        let output = region.tick(&TickInputs::default());
        let resonated = output.events.iter().any(|event| {
            matches!(event, RegionEvent::Sound { sound, .. } if *sound == sounds::BLOCK_BELL_RESONATE)
        });
        if resonated {
            assert_eq!(tick, 5, "the resonance starts on the fifth tick");
            resonances += 1;
        }
        let glow = output.effects.iter().any(|effect| {
            matches!(
                &effect.what,
                Effected::OnEntity { key, by: EntityUse::Raider(RaiderUse::Glow) } if *key == across
            )
        });
        if glow {
            assert_eq!(tick, 45, "the raiders glow on the forty-fifth tick");
            glowing_sent = true;
        }
        let ours_glows = mob(&region, ours).body.effects.has(MobEffect::GLOWING);
        assert_eq!(ours_glows, tick == 45, "tick {tick}");
    }
    assert_eq!(resonances, 1);
    assert!(glowing_sent);
}
