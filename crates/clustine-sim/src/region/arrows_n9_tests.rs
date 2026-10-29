//! Tests of arrows and tridents from the game's rules (`docs/groundwork/n9-plan.md`,
//! sections 4.2 and 4.3, read against n9 AbstractArrow, Arrow, ThrownTrident, n7k
//! ProjectileDeflection and ButtonBlock): an arrow's flight worked out by hand tick by
//! tick, in air and in water; its stick in a block, its rest there, its fall when the
//! block goes, its despawn and a tipped arrow's fading; its hits on a cow of the region,
//! piercing, across a line, and its turn back from a creative player and a face; a
//! burning arrow and TNT; a wooden button; pick-up; a trident's return; and every draw
//! from the arrow's own stream in the game's order.
//!
//! The worlds are those of the tests of TNT (`tnt/tests.rs`): a stone floor at y = -61,
//! the blocks at -60. An arrow's own stream in its tick is `Random::of_entity(seed, key,
//! lived)` for the `lived` its tick began at.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::components::{Component, PotionContents};
use clustine_world::{
    Arrow, ArrowKind, BlockPos, ChunkPos, Double, Effected, Entity, EntityChange, EntityData,
    EntityId, EntityUse, Float, InGround, ItemStack, Pickup,
};

use super::super::Region;
use super::super::projectiles::{Aim, Making};
use super::super::random_ticks::Random;
use super::super::tnt::tests::{
    add_player, idle, nothing, primed_tnt, state, west_of_a_line, world,
};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::physics::arrows as maths;
use crate::physics::thrown;

const SEED: u64 = 0x00A7_70E5;

/// An owner no region has: an arrow of it has none.
const NOBODY: EntityId = EntityId(999_999);

fn arrow_kind() -> u16 {
    u16::try_from(entity_types::ARROW).unwrap()
}

fn trident_kind() -> u16 {
    u16::try_from(entity_types::TRIDENT).unwrap()
}

/// A plain arrow of `owner`, as a bow of nobody's would make it.
fn record(owner: EntityId) -> Arrow {
    Arrow {
        pickup: Pickup::Allowed,
        pickup_stack: ItemStack::new(items::ARROW, 1),
        weapon: None,
        owner: Some(owner),
        left_owner: false,
        has_been_shot: false,
        base_damage: Double(2.0),
        crit: false,
        pierce_level: 0,
        pierced: Vec::new(),
        hit_sound: "minecraft:entity.arrow.hit".to_owned(),
        in_ground: None,
        shake_until: 0,
        of_kind: ArrowKind::Arrow,
    }
}

/// An arrow `arrow` at `at`, shot along `direction` at `power` with no uncertainty, so
/// that its motion is the direction made of length `power`.
fn shoot(
    region: &mut Region,
    at: [f64; 3],
    direction: [f64; 3],
    power: f32,
    arrow: Arrow,
) -> EntityId {
    shoot_kind(region, arrow_kind(), at, direction, power, arrow, 0)
}

fn shoot_kind(
    region: &mut Region,
    kind: u16,
    at: [f64; 3],
    direction: [f64; 3],
    power: f32,
    arrow: Arrow,
    fire: i32,
) -> EntityId {
    let making = Making {
        kind,
        at,
        data: EntityData::Arrow(Box::new(arrow)),
        aim: Aim::Direction(direction),
        power,
        uncertainty: 0.0,
    };
    let key = region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("an arrow is made");
    if fire > 0 {
        let (chunk, made) = region.find_entity(key).expect("made here");
        let lit = Entity {
            fire_ticks: fire,
            ..made.clone()
        };
        region
            .chunks
            .get_mut(&chunk)
            .unwrap()
            .apply_entity(&EntityChange::Whole(lit));
    }
    key
}

/// Lays `entity` in the region's loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// An arrow's whole record at `position`, lying still, under the region's next key.
fn lying(region: &mut Region, kind: u16, position: [f64; 3], lived: u32, arrow: Arrow) -> Entity {
    let key = region
        .take_entity_key(ChunkPos::containing(position[0], position[2]))
        .expect("a key");
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 11),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Arrow(Box::new(arrow)),
    }
}

/// A cow resting on the floor at `feet`, of full health, with no AI.
fn cow(key: EntityId, feet: [f64; 3]) -> Entity {
    let kind = entity_types::COW as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a cow has a health") as f32;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
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
    }
}

fn a_cow_at(region: &mut Region, feet: [f64; 3]) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    lay(region, &cow(key, feet));
    key
}

fn mob(region: &Region, key: EntityId) -> Mob {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    Mob::decode(&record.data).expect("a mob's record")
}

fn health(region: &Region, key: EntityId) -> f32 {
    mob(region, key).body.vitals.health
}

fn the(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .expect("the arrow is here")
        .1
        .clone()
}

fn arrow_of(entity: &Entity) -> Arrow {
    match &entity.data {
        EntityData::Arrow(arrow) => (**arrow).clone(),
        other => panic!("not an arrow: {other:?}"),
    }
}

/// The pitches of the sound `sound` in `output`.
fn pitches(output: &TickOutput, sound: i32) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: heard,
                pitch,
                ..
            } if *heard == sound => Some(*pitch),
            _ => None,
        })
        .collect()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

/// One tick of an arrow's flight in air by hand: it moves by its motion, then keeps
/// `0.99f` of it and falls by 0.05.
fn fly(position: [f64; 3], motion: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let drag = f64::from(0.99_f32);
    let moved = [
        position[0] + motion[0],
        position[1] + motion[1],
        position[2] + motion[2],
    ];
    (
        moved,
        [
            motion[0] * drag + 0.0,
            motion[1] * drag + -0.05,
            motion[2] * drag + 0.0,
        ],
    )
}

fn bits(values: [f64; 3]) -> [u64; 3] {
    values.map(f64::to_bits)
}

#[test]
fn an_arrow_flies_by_its_motion_then_keeps_ninety_nine_hundredths_and_falls_by_a_twentieth() {
    let mut region = world(SEED, nothing);
    let start = [0.5, -50.0, 0.5];
    let key = shoot(&mut region, start, [0.0, 0.0, 1.0], 1.5, record(NOBODY));
    assert_eq!(the(&region, key).motion, [0.0, 0.0, 1.5]);
    let (mut position, mut motion) = (start, [0.0, 0.0, 1.5]);
    for tick in 1..=5 {
        idle(&mut region);
        (position, motion) = fly(position, motion);
        let now = the(&region, key);
        assert_eq!(
            bits(now.position),
            bits(position),
            "tick {tick}: {:?}",
            now.position
        );
        assert_eq!(
            bits(now.motion),
            bits(motion),
            "tick {tick}: {:?}",
            now.motion
        );
    }
    // The move comes before the drag: 0.5, then 1.5 on, then 1.485, 1.47015, 1.4554485.
    let z = 0.5 + 1.5 + 1.5 * 0.99 + 1.5 * 0.99 * 0.99 + 1.5 * 0.99 * 0.99 * 0.99;
    assert!(
        (position[2] - (z + 1.5 * 0.99_f64.powi(4))).abs() < 1.0e-4,
        "{position:?}"
    );
    let now = the(&region, key);
    assert!(arrow_of(&now).has_been_shot);
    assert!(
        now.pitch < 0.0,
        "it turns down towards its motion: {}",
        now.pitch
    );
}

#[test]
fn in_water_an_arrow_moves_by_the_motion_it_had_and_keeps_six_tenths_of_it() {
    let water = state("minecraft:water[level=0]");
    let mut region = world(SEED, move |pos: BlockPos| {
        (pos.y >= -60 && pos.y <= -40).then_some(water)
    });
    let start = [0.5, -50.0, 0.5];
    let key = shoot(&mut region, start, [0.0, 0.0, 1.0], 1.0, record(NOBODY));
    // Its first tick knows no water yet (`wasTouchingWater` from a base tick before):
    // it flies as in air, and its base tick finds the water.
    idle(&mut region);
    let (mut position, mut motion) = fly(start, [0.0, 0.0, 1.0]);
    assert_eq!(bits(the(&region, key).position), bits(position));
    for tick in 2..=4 {
        idle(&mut region);
        // The move along the motion as the tick began, then six tenths kept and no air
        // drag, then gravity.
        position = [
            position[0] + motion[0],
            position[1] + motion[1],
            position[2] + motion[2],
        ];
        let inertia = f64::from(0.6_f32);
        motion = [
            motion[0] * inertia + 0.0,
            motion[1] * inertia + -0.05,
            motion[2] * inertia + 0.0,
        ];
        let now = the(&region, key);
        assert_eq!(bits(now.position), bits(position), "tick {tick}");
        assert_eq!(bits(now.motion), bits(motion), "tick {tick}");
    }
}

/// A wall of stone across z = 5 from the floor up.
fn wall(pos: BlockPos) -> Option<BlockState> {
    (pos.z == 5 && pos.y > -61).then_some(blocks::STONE)
}

#[test]
fn an_arrow_sticks_in_a_wall_moved_back_along_each_axis_of_its_motion() {
    let mut region = world(SEED, wall);
    let start = [0.5, -50.0, 0.5];
    let mut arrow = record(NOBODY);
    arrow.crit = true;
    arrow.pierce_level = 2;
    let key = shoot(&mut region, start, [0.0, 0.0, 1.0], 1.5, arrow);
    let (mut position, mut motion) = (start, [0.0, 0.0, 1.5]);
    for _ in 0..3 {
        idle(&mut region);
        (position, motion) = fly(position, motion);
    }
    assert!(
        position[2] < 5.0 && position[2] + motion[2] > 5.0,
        "{position:?}"
    );
    let lived = the(&region, key).lived;
    let output = idle(&mut region);
    let now = the(&region, key);
    let stuck = arrow_of(&now);
    // Where the clip met the wall's face, less `signum · 0.05` along each axis: back
    // along z, up along y (it was falling), and nothing along x, where it moved not.
    let to = [
        position[0] + motion[0],
        position[1] + motion[1],
        position[2] + motion[2],
    ];
    let delta = [
        to[0] - position[0],
        to[1] - position[1],
        to[2] - position[2],
    ];
    let along = (5.0 - position[2]) / delta[2];
    let met = [
        position[0] + along * delta[0],
        position[1] + along * delta[1],
        position[2] + along * delta[2],
    ];
    assert_eq!(
        bits(now.position),
        bits([met[0], met[1] + maths::MOVE_BACK, met[2] - maths::MOVE_BACK])
    );
    assert_eq!(now.motion, [0.0; 3]);
    assert_eq!(
        stuck.in_ground,
        Some(InGround {
            state: blocks::STONE.0,
            since: lived + 1,
        })
    );
    assert_eq!(stuck.shake_until, lived + 1 + 7);
    assert!(!stuck.crit);
    assert_eq!(stuck.pierce_level, 0);
    // Its sound's pitch is the first draw of its own stream in the tick.
    let mut own = Random::of_entity(SEED, key, lived);
    let pitch = maths::hit_pitch(own.next_float());
    assert_eq!(pitches(&output, sounds::ENTITY_ARROW_HIT), [pitch]);
    // In the ground it commits nothing, tick after tick.
    for _ in 0..3 {
        let output = idle(&mut region);
        assert!(
            !output.entity_changes.iter().any(|(_, change)| matches!(
                change,
                EntityChange::Set(changed, _) if *changed == key
            )),
            "{:?}",
            output.entity_changes
        );
    }
    assert_eq!(the(&region, key).position, now.position);
}

#[test]
fn an_arrow_falls_with_three_draws_when_the_block_it_stuck_in_goes() {
    let mut region = world(SEED, wall);
    let key = shoot(
        &mut region,
        [0.5, -50.0, 0.5],
        [0.0, 0.0, 1.0],
        1.5,
        record(NOBODY),
    );
    for _ in 0..4 {
        idle(&mut region);
    }
    let stuck = the(&region, key);
    assert!(arrow_of(&stuck).in_ground.is_some());
    // The wall goes about it.
    let mut output = TickOutput::default();
    let y = crate::physics::floor(stuck.position[1]);
    for dy in -1..=1 {
        for x in -1..=1 {
            region.set_block(BlockPos::new(x, y + dy, 5), blocks::AIR, &mut output);
        }
    }
    idle(&mut region);
    let fell = the(&region, key);
    assert_eq!(arrow_of(&fell).in_ground, None);
    // Its motion was nothing: three draws times nothing, and no gravity in that tick.
    assert_eq!(fell.motion, [0.0; 3]);
    idle(&mut region);
    assert_eq!(the(&region, key).motion, [0.0, -0.05, 0.0]);
}

#[test]
fn a_falling_arrow_s_motion_takes_three_of_its_floats_in_the_order_x_y_z() {
    let mut region = world(SEED, nothing);
    // An arrow in the ground with a motion, in air, where it stuck in stone: it falls.
    let mut arrow = record(NOBODY);
    arrow.in_ground = Some(InGround {
        state: blocks::STONE.0,
        since: 10,
    });
    let mut entity = lying(&mut region, arrow_kind(), [0.5, -55.0, 0.5], 20, arrow);
    entity.motion = [1.0, -2.0, 3.0];
    lay(&mut region, &entity);
    idle(&mut region);
    let mut own = Random::of_entity(SEED, entity.key, 20);
    let draws = [own.next_float(), own.next_float(), own.next_float()];
    let expected = [
        1.0 * f64::from(draws[0] * 0.2),
        -2.0 * f64::from(draws[1] * 0.2),
        3.0 * f64::from(draws[2] * 0.2),
    ];
    let fell = the(&region, entity.key);
    assert_eq!(bits(fell.motion), bits(expected));
    assert_eq!(arrow_of(&fell).in_ground, None);
    assert_eq!(
        fell.position, entity.position,
        "no move in the tick it falls"
    );
}

/// An arrow stuck in the floor's stone, of `lived`, that landed at `since`.
fn in_the_floor(region: &mut Region, lived: u32, since: u32, arrow: Arrow) -> Entity {
    let mut arrow = arrow;
    arrow.in_ground = Some(InGround {
        state: blocks::STONE.0,
        since,
    });
    arrow.has_been_shot = true;
    let entity = lying(region, arrow_kind(), [2.5, -60.95, 2.5], lived, arrow);
    lay(region, &entity);
    entity
}

#[test]
fn an_arrow_is_gone_after_twelve_hundred_ticks_in_the_ground() {
    let mut region = world(SEED, nothing);
    // Its tick ends at `lived` 5001: 1199 and 1200 ticks after landing.
    let stays = in_the_floor(&mut region, 5000, 5001 - 1199, record(NOBODY));
    let goes = in_the_floor(&mut region, 5000, 5001 - 1200, record(NOBODY));
    let output = idle(&mut region);
    assert!(region.find_entity(stays.key).is_some());
    assert!(region.find_entity(goes.key).is_none());
    assert!(gone(&output, goes.key));
}

fn tipped() -> ItemStack {
    let mut stack = ItemStack::new(items::TIPPED_ARROW, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:poison".to_owned()),
            ..PotionContents::default()
        }));
    stack
}

#[test]
fn a_tipped_arrow_is_a_plain_one_after_six_hundred_ticks_in_the_ground() {
    let mut region = world(SEED, nothing);
    let mut arrow = record(NOBODY);
    arrow.pickup_stack = tipped();
    let early = in_the_floor(&mut region, 1000, 1001 - 599, arrow.clone());
    let late = in_the_floor(&mut region, 1000, 1001 - 600, arrow);
    let output = idle(&mut region);
    assert_eq!(arrow_of(&the(&region, early.key)).pickup_stack, tipped());
    assert_eq!(
        arrow_of(&the(&region, late.key)).pickup_stack,
        ItemStack::new(items::ARROW, 1)
    );
    let events: Vec<(EntityId, u8)> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } => Some((*entity, *event)),
            _ => None,
        })
        .collect();
    assert!(events.contains(&(late.key, 0)));
    assert!(!events.iter().any(|(entity, _)| *entity == early.key));
}

#[test]
fn an_arrow_hurts_a_cow_by_its_speed_and_a_crit_s_draw_and_gives_its_potion() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let full = health(&region, target);
    let mut arrow = record(NOBODY);
    arrow.crit = true;
    arrow.pickup_stack = tipped();
    // At 2 along x: from 1.5 into the cow's box at 3.05 in one step.
    let key = shoot(&mut region, [1.5, -59.5, 0.5], [1.0, 0.0, 0.0], 2.0, arrow);
    let lived = the(&region, key).lived;
    let output = idle(&mut region);
    // `ceil(2 · 2) = 4`, then a crit's `nextInt(4 / 2 + 2)` added; then the sound's
    // pitch, the next draw.
    let mut own = Random::of_entity(SEED, key, lived);
    let crit = own.next_int(maths::crit_bound(4));
    let pitch = maths::hit_pitch(own.next_float());
    assert_eq!(health(&region, target), full - (4 + crit) as f32);
    assert_eq!(pitches(&output, sounds::ENTITY_ARROW_HIT), [pitch]);
    // Not piercing: it is gone.
    assert!(region.find_entity(key).is_none());
    assert!(gone(&output, key));
    // Its hurt pushes along its motion, and its potion is given at an eighth of its time.
    let pushed = region.find_entity(target).expect("the cow").1.motion;
    assert!(pushed[0] > 0.0, "{pushed:?}");
    let poison = MobEffect::named("minecraft:poison").expect("poison");
    assert!(mob(&region, target).body.effects.has(poison));
    assert_eq!(super::potion_effects(&tipped())[0].duration, 900 / 8);
}

#[test]
fn a_piercing_arrow_hurts_two_cows_in_a_step_and_is_gone_silently_at_the_third() {
    let mut region = world(SEED, nothing);
    let cows = [2.0, 3.0, 4.0].map(|x| a_cow_at(&mut region, [x, -60.0, 0.5]));
    let full = health(&region, cows[0]);
    let mut arrow = record(NOBODY);
    arrow.pierce_level = 1;
    let key = shoot(&mut region, [0.5, -59.5, 0.5], [1.0, 0.0, 0.0], 4.0, arrow);
    let output = idle(&mut region);
    // `ceil(4 · 2) = 8` each to the first two; the third finds the pierced set full.
    assert_eq!(health(&region, cows[0]), full - 8.0);
    assert_eq!(health(&region, cows[1]), full - 8.0);
    assert_eq!(health(&region, cows[2]), full);
    assert_eq!(pitches(&output, sounds::ENTITY_ARROW_HIT).len(), 2);
    assert!(region.find_entity(key).is_none());
}

#[test]
fn an_arrow_at_a_cow_across_a_line_is_sent_as_a_hurt_and_is_gone() {
    let mut region = west_of_a_line(SEED, nothing);
    // A cow in the copy of the chunk east of the line.
    let copied = cow(EntityId(77_777), [16.5, -60.0, 0.5]);
    region
        .copies
        .get_mut(&ChunkPos::new(1, 0))
        .expect("the copy")
        .apply_entity(&EntityChange::Whole(copied.clone()));
    let mut arrow = record(NOBODY);
    arrow.pickup_stack = tipped();
    let key = shoot(&mut region, [14.5, -59.5, 0.5], [1.0, 0.0, 0.0], 2.0, arrow);
    let lived = the(&region, key).lived;
    let output = idle(&mut region);
    let hurts: Vec<&clustine_world::HurtBy> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity {
                key: target,
                by: EntityUse::Hurt(hurt),
            } if *target == copied.key => Some(&**hurt),
            _ => None,
        })
        .collect();
    let [hurt] = &hurts[..] else {
        panic!("one hurt: {:?}", output.effects);
    };
    assert_eq!(hurt.damage_type, "minecraft:arrow");
    assert_eq!(hurt.amount, Float(4.0));
    assert!(hurt.arrow);
    assert_eq!(hurt.effects[0].effect, "minecraft:poison");
    // Its sound's pitch drawn now, the arrow's first draw of the tick.
    let pitch = maths::hit_pitch(Random::of_entity(SEED, key, lived).next_float());
    assert_eq!(
        hurt.sound,
        Some((sounds::ENTITY_ARROW_HIT, Float(1.0), Float(pitch)))
    );
    assert!(region.find_entity(key).is_none());
}

/// What a refused hit leaves of the arrow `shot`, shot at 2 along x: a tenth of its
/// motion turned back, its yaw, turned towards its motion, turned by `170 + 20 · f`,
/// then the air's drag and gravity.
fn turned_back(region: &Region, shot: &Entity) {
    let key = shot.key;
    let draw = Random::of_entity(SEED, key, shot.lived).next_float();
    let [turned, _] = maths::turn(shot.yaw, shot.pitch, [2.0, 0.0, 0.0], false);
    let (motion, yaw) = maths::reverse([2.0, 0.0, 0.0], maths::ARROW_BOUNCE, turned, draw);
    let drag = f64::from(0.99_f32);
    let expected = [
        motion[0] * drag + 0.0,
        motion[1] * drag + -0.05,
        motion[2] * drag + 0.0,
    ];
    let now = the(region, key);
    assert_eq!(bits(now.motion), bits(expected));
    assert_eq!(now.yaw, yaw);
    assert!(
        now.motion[0] < 0.0,
        "back the way it came: {:?}",
        now.motion
    );
}

#[test]
fn an_arrow_turns_back_from_a_creative_player_of_the_region() {
    let mut region = world(SEED, nothing);
    add_player(&mut region, 1, [3.5, -60.0, 0.5], false);
    let key = shoot(
        &mut region,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        record(NOBODY),
    );
    let shot = the(&region, key);
    idle(&mut region);
    turned_back(&region, &shot);
}

/// Whether the report of the projectile `key` in `output` asks for it to be sent at once
/// (`needsSync`); none where it was not reported.
fn synced(output: &TickOutput, key: EntityId) -> Option<bool> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::EntityTracked(report) if report.entity == key => {
            Some(report.flags & crate::api::Tracked::NEEDS_SYNC != 0)
        }
        _ => None,
    })
}

/// `stepMoveAndHit` sets `needsSync` after the entities' hits (n9 AbstractArrow 169-171):
/// the turn back from a creative player is sent in the tick it happens, not at the
/// arrow's next update twenty ticks on, which a client sees fly on through the player.
/// The comparison with the official server found Clustine's turn never shown.
#[test]
fn an_arrow_turned_back_by_a_creative_player_is_sent_at_once() {
    let mut region = world(SEED, nothing);
    add_player(&mut region, 1, [3.5, -60.0, 0.5], false);
    let key = shoot(
        &mut region,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        record(NOBODY),
    );
    let shot = the(&region, key);
    let output = idle(&mut region);
    turned_back(&region, &shot);
    assert_eq!(synced(&output, key), Some(true));
    // Flying back, it hits nothing and waits for its updates again.
    let output = idle(&mut region);
    assert_eq!(synced(&output, key), Some(false));
}

/// `stepMoveAndHit` sets `needsSync` after a block's hit too (n9 AbstractArrow 134-142):
/// an arrow is sent where it sticks in the tick it sticks, and not in the ticks of its
/// flight before.
#[test]
fn an_arrow_sticking_in_a_wall_is_sent_at_once_and_one_in_flight_is_not() {
    let mut region = world(SEED, wall);
    let key = shoot(
        &mut region,
        [0.5, -50.0, 0.5],
        [0.0, 0.0, 1.0],
        1.5,
        record(NOBODY),
    );
    for _ in 0..3 {
        let output = idle(&mut region);
        assert_eq!(synced(&output, key), Some(false));
    }
    let output = idle(&mut region);
    assert!(arrow_of(&the(&region, key)).in_ground.is_some());
    assert_eq!(synced(&output, key), Some(true));
}

#[test]
fn an_arrow_turns_back_from_another_region_s_player_s_face() {
    let mut region = world(SEED, nothing);
    let pose = crate::api::Pose::at(clustine_world::Vec3::new(3.5, -60.0, 0.5));
    let face = crate::api::PlayerFace::of(
        &pose,
        false,
        false,
        &crate::api::Inventory::default(),
        0,
        None,
    );
    let player = clustine_world::PlayerId(uuid::Uuid::from_u128(42));
    region
        .faces
        .departed(player, EntityId(424_242), 0, face, region.tick);
    let key = shoot(
        &mut region,
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.0,
        record(NOBODY),
    );
    let shot = the(&region, key);
    let output = idle(&mut region);
    turned_back(&region, &shot);
    assert!(output.effects.is_empty(), "nothing is sent for a face");
}

#[test]
fn a_burning_arrow_lights_the_tnt_it_hits() {
    let tnt_at = BlockPos::new(0, -50, 5);
    let mut region = world(SEED, move |pos| (pos == tnt_at).then_some(blocks::TNT));
    shoot_kind(
        &mut region,
        arrow_kind(),
        [0.5, -49.5, 2.5],
        [0.0, 0.0, 1.0],
        1.5,
        record(NOBODY),
        100,
    );
    let mut primed = Vec::new();
    for _ in 0..4 {
        idle(&mut region);
        primed = primed_tnt(&region);
        if !primed.is_empty() {
            break;
        }
    }
    assert_eq!(primed.len(), 1, "primed by the arrow");
    assert_eq!(region.block(tnt_at), Some(blocks::AIR));
}

#[test]
fn a_cold_arrow_leaves_tnt_be() {
    let tnt_at = BlockPos::new(0, -50, 5);
    let mut region = world(SEED, move |pos| (pos == tnt_at).then_some(blocks::TNT));
    shoot(
        &mut region,
        [0.5, -49.5, 2.5],
        [0.0, 0.0, 1.0],
        1.5,
        record(NOBODY),
    );
    for _ in 0..4 {
        idle(&mut region);
    }
    assert!(primed_tnt(&region).is_empty());
    assert_eq!(region.block(tnt_at), Some(blocks::TNT));
}

fn button() -> BlockState {
    state("minecraft:oak_button[face=floor,facing=north,powered=false]")
}

#[test]
fn an_arrow_on_a_wooden_button_keeps_it_pressed_and_it_lets_go_when_the_arrow_goes() {
    let at = BlockPos::new(2, -60, 2);
    let mut region = world(SEED, move |pos| (pos == at).then_some(button()));
    // An arrow lying in the button's cell, stuck in the button (its block is the one
    // it stuck in, so it does not fall).
    let mut arrow = record(NOBODY);
    arrow.in_ground = Some(InGround {
        state: button().0,
        since: 0,
    });
    let entity = lying(&mut region, arrow_kind(), [2.5, -59.95, 2.5], 10, arrow);
    lay(&mut region, &entity);
    let output = idle(&mut region);
    let pressed = region.block(at).expect("loaded");
    assert_eq!(pressed.property("powered"), Some("true"));
    assert_eq!(
        pitches(&output, sounds::BLOCK_WOODEN_BUTTON_CLICK_ON),
        [1.0]
    );
    // While the arrow lies there the button's own look finds it.
    assert!(region.arrow_on_button(at, pressed));
    // The arrow goes; the button's look 30 ticks after the press lets it go.
    let mut output = TickOutput::default();
    region.remove_entity(entity.key, 0, ChunkPos::new(0, 0), &mut output);
    for tick in 1..30 {
        idle(&mut region);
        assert_eq!(
            region.block(at).unwrap().property("powered"),
            Some("true"),
            "tick {tick}"
        );
    }
    idle(&mut region);
    assert_eq!(region.block(at).unwrap().property("powered"), Some("false"));
}

#[test]
fn a_player_picks_up_an_arrow_done_shaking_and_one_only_a_creative_player_takes_gives_nothing() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, [2.5, -60.0, 2.5], false);
    let mut shaking = record(NOBODY);
    shaking.shake_until = 200;
    let shaking = in_the_floor(&mut region, 100, 90, shaking);
    let mut output = TickOutput::default();
    region.players_touch_arrows(&mut output);
    assert!(region.find_entity(shaking.key).is_some(), "still shaking");
    region.remove_entity(shaking.key, 0, ChunkPos::new(0, 0), &mut output);

    let allowed = in_the_floor(&mut region, 100, 90, record(NOBODY));
    let mut output = TickOutput::default();
    region.players_touch_arrows(&mut output);
    assert!(region.find_entity(allowed.key).is_none());
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::ItemTaken { item, collector, count: 1, .. }
            if *item == allowed.key && *collector == player
    )));
    let held = region.players.get(&id).expect("the player");
    assert!(
        held.inventory
            .0
            .iter()
            .flatten()
            .any(|stack| stack.item == items::ARROW)
    );

    let mut creative = record(NOBODY);
    creative.pickup = Pickup::CreativeOnly;
    creative.pickup_stack = ItemStack::new(items::SPECTRAL_ARROW, 1);
    let creative = in_the_floor(&mut region, 100, 90, creative);
    let mut output = TickOutput::default();
    region.players_touch_arrows(&mut output);
    assert!(region.find_entity(creative.key).is_none());
    let held = region.players.get(&id).expect("the player");
    assert!(
        !held
            .inventory
            .0
            .iter()
            .flatten()
            .any(|stack| stack.item == items::SPECTRAL_ARROW)
    );
}

fn loyal_trident() -> ItemStack {
    let mut stack = ItemStack::new(items::TRIDENT, 1);
    stack.components.set(Component::Enchantments(vec![(
        "minecraft:loyalty".to_owned(),
        3,
    )]));
    stack
}

#[test]
fn a_loyal_trident_that_dealt_its_damage_comes_back_to_its_owner_through_blocks() {
    let mut region = world(SEED, nothing);
    let (_, player) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    let mut trident = record(player);
    trident.pickup_stack = loyal_trident();
    trident.of_kind = ArrowKind::Trident {
        dealt_damage: true,
        returning: 0,
    };
    trident.hit_sound = "minecraft:item.trident.hit_ground".to_owned();
    trident.in_ground = Some(InGround {
        state: blocks::STONE.0,
        since: 40,
    });
    let entity = lying(&mut region, trident_kind(), [5.5, -60.95, 0.5], 50, trident);
    lay(&mut region, &entity);
    let output = idle(&mut region);
    // The pull towards the eyes, 1.62 above the feet, at loyalty 3; then, passing
    // through blocks, a move by that motion and the drag, with no gravity.
    let [raised, pulled] =
        maths::trident_return(entity.position, [0.0; 3], [0.5, -60.0 + 1.62, 0.5], 3);
    let moved = [
        raised[0] + pulled[0],
        raised[1] + pulled[1],
        raised[2] + pulled[2],
    ];
    let drag = f64::from(0.99_f32);
    let now = the(&region, entity.key);
    assert_eq!(bits(now.position), bits(moved));
    assert_eq!(bits(now.motion), bits(pulled.map(|value| value * drag)));
    assert!(now.motion[0] < 0.0, "towards its owner: {:?}", now.motion);
    assert!(matches!(
        arrow_of(&now).of_kind,
        ArrowKind::Trident { returning: 1, .. }
    ));
    assert_eq!(pitches(&output, sounds::ITEM_TRIDENT_RETURN), [1.0]);
    // Once only.
    let output = idle(&mut region);
    assert!(pitches(&output, sounds::ITEM_TRIDENT_RETURN).is_empty());
}

#[test]
fn a_trident_hits_a_cow_by_eight_and_bounces_whether_or_not_it_took() {
    let mut region = world(SEED, nothing);
    let target = a_cow_at(&mut region, [3.5, -60.0, 0.5]);
    let full = health(&region, target);
    let mut trident = record(NOBODY);
    trident.pickup_stack = ItemStack::new(items::TRIDENT, 1);
    trident.of_kind = ArrowKind::Trident {
        dealt_damage: false,
        returning: 0,
    };
    let key = shoot_kind(
        &mut region,
        trident_kind(),
        [1.5, -59.5, 0.5],
        [1.0, 0.0, 0.0],
        2.5,
        trident,
        0,
    );
    let lived = the(&region, key).lived;
    let output = idle(&mut region);
    assert_eq!(health(&region, target), full - 8.0);
    assert_eq!(pitches(&output, sounds::ITEM_TRIDENT_HIT), [1.0]);
    let draw = Random::of_entity(SEED, key, lived).next_float();
    let (motion, _) = maths::reverse([2.5, 0.0, 0.0], maths::TRIDENT_BOUNCE, 0.0, draw);
    let now = the(&region, key);
    assert!(matches!(
        arrow_of(&now).of_kind,
        ArrowKind::Trident {
            dealt_damage: true,
            ..
        }
    ));
    // Gravity adds `(0, -0.05, 0)`: a turned back `-0.0` along z is `0.0` after it.
    let drag = f64::from(0.99_f32);
    assert_eq!(
        bits(now.motion),
        bits([
            motion[0] * drag + 0.0,
            motion[1] * drag + -0.05,
            motion[2] * drag + 0.0
        ])
    );
}

#[test]
fn the_arithmetic_of_an_arrow_is_the_game_s() {
    // `Math.signum(0)` is 0: no move back along an axis it does not move along.
    assert_eq!(
        maths::move_back([1.0, 2.0, 3.0], [0.0, -1.0, 2.0]),
        [1.0, 2.0 + maths::MOVE_BACK, 3.0 - maths::MOVE_BACK]
    );
    assert_eq!(maths::damage_by_speed(1.5, 2.0), 3);
    assert_eq!(maths::damage_by_speed(1.51, 2.0), 4);
    assert_eq!(maths::crit_bound(9), 6);
    assert_eq!(maths::crit_damage(i32::MAX, 5), i32::MAX);
    assert_eq!(maths::power_damage(2.0, 0), 2.0);
    assert_eq!(maths::power_damage(2.0, 1), 3.0);
    assert_eq!(maths::power_damage(2.0, 5), 5.0);
    assert_eq!(maths::punch_knockback(2), 2.0);
    assert_eq!(maths::arrow_goes_after(1), 580);
    // A refusal keeps a tenth of the motion, turned back.
    let (motion, yaw) = maths::reverse([1.0, -2.0, 0.5], maths::ARROW_BOUNCE, 10.0, 0.5);
    assert_eq!(motion, [-0.1, 0.2, -0.05]);
    assert_eq!(yaw, 10.0 + 180.0);
    // Within a step each effect once, fire before lava before water; steps in turn.
    use maths::InsideEffect as E;
    assert_eq!(
        maths::inside_effects(&[
            (0, E::Extinguish),
            (0, E::FireIgnite),
            (0, E::FireIgnite),
            (1, E::LavaIgnite)
        ]),
        [E::FireIgnite, E::Extinguish, E::LavaIgnite]
    );
    assert_eq!(maths::after_inside(0, E::FireIgnite), 160);
    assert_eq!(maths::after_inside(-1, E::FireIgnite), 160);
    assert_eq!(maths::after_inside(-3, E::FireIgnite), -2);
    assert_eq!(maths::after_inside(50, E::Extinguish), 0);
    assert_eq!(maths::after_inside(50, E::LavaIgnite), 300);
    // The margin of the hit test is the thrown items'.
    assert_eq!(thrown::margin(1), 0.0);
}

#[test]
fn a_target_block_gives_more_power_the_nearer_the_middle_it_is_hit() {
    use super::super::projectile_hits::target_strength;
    use clustine_data::Face;
    assert_eq!(target_strength([0.5, 0.5, 0.0], Face::North), 15);
    assert_eq!(target_strength([0.99, 0.5, 0.0], Face::North), 1);
    assert_eq!(target_strength([0.75, 0.5, 0.0], Face::North), 8);
}
