//! Burning in daylight and a monster's idle time, from `docs/groundwork/n7k-brief.md`
//! sections 3.1 and 3.3, and the families of `ladder.rs` that N7k adds.

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items};
use clustine_world::{BlockPos, ChunkPos, EntityId, ItemStack};

use super::burning::{burn_undead, monster_idles, monsters_burn};
use super::ladder;
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::types::of;
use crate::mobs::view::{LightAt, MobWorld, Seen};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;

const ZOMBIE_HORSE: u16 = entity_types::ZOMBIE_HORSE as u16;
const HORSE: u16 = entity_types::HORSE as u16;
const KEY: EntityId = EntityId(40);
/// Noon of the overworld clock: monsters burn.
const NOON: u64 = 6_000;

/// Open land at a time of day, every block lit as `light` says.
struct Sunny {
    day: u64,
    light: Option<LightAt>,
}

impl World for Sunny {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(if pos.y < 0 {
            blocks::STONE
        } else {
            blocks::AIR
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Sunny {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Sunny {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn entity(&self, _: EntityId) -> Option<Seen<'_>> {
        None
    }

    fn players(&self) -> Vec<crate::mobs::view::SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        self.light
    }

    fn day(&self) -> u64 {
        self.day
    }
}

fn open_sky(day: u64) -> Sunny {
    Sunny {
        day,
        light: Some(LightAt { sky: 15, block: 0 }),
    }
}

fn living(kind: u16) -> Living {
    let info = EntityTypeInfo::of(i32::from(kind));
    let position = [0.5, 0.0, 0.5];
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
    living.air = MAX_AIR;
    living
}

fn healthy(kind: u16) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = super::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob
}

/// One `burnUndead` of `mob` on `world` with the stream `stream`: the mob and its body's
/// fire after it.
fn burnt(world: &Sunny, kind: u16, mob: Mob, in_water: bool, stream: &mut Random) -> (Mob, i32) {
    let mut living = living(kind);
    living.body.in_water = in_water;
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: KEY,
        kind,
        lived: 100,
        tick_count: 100,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob,
        living,
        yaw: 0.0,
        pitch: 0.0,
        mob_type: of(kind),
        world,
        draws: stream,
        out: MobOutput::default(),
    };
    burn_undead(&mut tick);
    (tick.mob, tick.living.fire_ticks)
}

/// The chance at full light is `(1 - 0.4) * 2 / 30`: a stream's first float below it
/// sets the mob alight for 160 ticks, one above it does nothing.
fn stream_whose_first_float(below: bool) -> Random {
    let chance = (1.0_f32 - 0.4) * 2.0;
    (0..)
        .map(|seed| Random::of_entity(seed, KEY, 1))
        .find(|stream| {
            let first = stream.clone().next_float() * 30.0;
            (first < chance) == below
        })
        .expect("some stream draws either way")
}

#[test]
fn monsters_burn_from_tick_23460_of_the_day_to_12542_of_the_next() {
    assert!(monsters_burn(0));
    assert!(monsters_burn(12_541));
    assert!(!monsters_burn(12_542));
    assert!(!monsters_burn(23_459));
    assert!(monsters_burn(23_460));
    assert!(monsters_burn(24_000 + 6_000));
}

#[test]
fn an_undead_horse_under_the_noon_sun_catches_fire_for_eight_seconds_at_the_game_s_chance() {
    let world = open_sky(NOON);
    let mut stream = stream_whose_first_float(true);
    let mut expected = stream.clone();
    let (_, fire) = burnt(
        &world,
        ZOMBIE_HORSE,
        healthy(ZOMBIE_HORSE),
        false,
        &mut stream,
    );
    assert_eq!(fire, 160);
    expected.next_float();
    assert_eq!(stream, expected, "one draw, no more");

    let mut stream = stream_whose_first_float(false);
    let (_, fire) = burnt(
        &world,
        ZOMBIE_HORSE,
        healthy(ZOMBIE_HORSE),
        false,
        &mut stream,
    );
    assert_eq!(fire, 0, "the draw failed");
}

#[test]
fn nothing_burns_at_night_in_the_dark_or_a_living_horse_and_no_draw_is_made() {
    let cases = [
        (open_sky(18_000), ZOMBIE_HORSE),
        (
            Sunny {
                day: NOON,
                light: Some(LightAt { sky: 6, block: 6 }),
            },
            ZOMBIE_HORSE,
        ),
        (open_sky(NOON), HORSE),
    ];
    for (world, kind) in cases {
        let mut stream = stream_whose_first_float(true);
        let before = stream.clone();
        let (_, fire) = burnt(&world, kind, healthy(kind), false, &mut stream);
        assert_eq!(fire, 0);
        assert_eq!(stream, before, "no draw");
    }
}

#[test]
fn a_wet_undead_or_one_under_a_roof_draws_and_does_not_burn() {
    let mut stream = stream_whose_first_float(true);
    let mut expected = stream.clone();
    let (_, fire) = burnt(
        &open_sky(NOON),
        ZOMBIE_HORSE,
        healthy(ZOMBIE_HORSE),
        true,
        &mut stream,
    );
    assert_eq!(fire, 0);
    expected.next_float();
    assert_eq!(stream, expected, "drawn before the wet test");

    // A block's light of 15 under a roof: bright, and no sky.
    let roofed = Sunny {
        day: NOON,
        light: Some(LightAt { sky: 0, block: 15 }),
    };
    let mut stream = stream_whose_first_float(true);
    let (_, fire) = burnt(
        &roofed,
        ZOMBIE_HORSE,
        healthy(ZOMBIE_HORSE),
        false,
        &mut stream,
    );
    assert_eq!(fire, 0);
}

#[test]
fn an_undead_horse_s_body_armour_keeps_the_sun_off_and_its_head_does_not() {
    let world = open_sky(NOON);
    let mut armoured = healthy(ZOMBIE_HORSE);
    armoured.body.equipment.slots[usize::from(slots::BODY)] = Some(ItemStack {
        item: items::IRON_HORSE_ARMOR,
        count: 1,
    });
    let mut stream = stream_whose_first_float(true);
    let (_, fire) = burnt(&world, ZOMBIE_HORSE, armoured, false, &mut stream);
    assert_eq!(fire, 0, "the body slot protects");

    let mut hatted = healthy(ZOMBIE_HORSE);
    hatted.body.equipment.slots[usize::from(slots::HEAD)] = Some(ItemStack {
        item: items::IRON_HELMET,
        count: 1,
    });
    let mut stream = stream_whose_first_float(true);
    let (_, fire) = burnt(&world, ZOMBIE_HORSE, hatted, false, &mut stream);
    assert_eq!(fire, 160, "not the head");
}

#[test]
fn a_wearing_protection_draws_its_wear_and_a_carved_pumpkin_none() {
    let world = open_sky(NOON);
    let mut armoured = healthy(ZOMBIE_HORSE);
    // Horse armour does not wear; a damageable item in the slot would.
    armoured.body.equipment.slots[usize::from(slots::BODY)] = Some(ItemStack {
        item: items::IRON_CHESTPLATE,
        count: 1,
    });
    let mut stream = stream_whose_first_float(true);
    let mut expected = stream.clone();
    burnt(&world, ZOMBIE_HORSE, armoured, false, &mut stream);
    expected.next_float();
    expected.next_int(2);
    assert_eq!(stream, expected, "the sun's draw, then the wear's");

    let mut pumpkin = healthy(ZOMBIE_HORSE);
    pumpkin.body.equipment.slots[usize::from(slots::BODY)] = Some(ItemStack {
        item: items::CARVED_PUMPKIN,
        count: 1,
    });
    let mut stream = stream_whose_first_float(true);
    let mut expected = stream.clone();
    burnt(&world, ZOMBIE_HORSE, pumpkin, false, &mut stream);
    expected.next_float();
    assert_eq!(stream, expected, "no wear drawn");
}

#[test]
fn a_monster_s_idle_time_gains_two_in_the_light_and_an_animal_s_none() {
    let world = open_sky(NOON);
    let tick_of = |kind: u16| {
        let mut mob = healthy(kind);
        mob.body.idle.no_action_since = Some(90);
        let mut stream = Random::of_entity(1, KEY, 1);
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind,
            lived: 100,
            tick_count: 100,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob,
            living: living(kind),
            yaw: 0.0,
            pitch: 0.0,
            mob_type: of(kind),
            world: &world,
            draws: &mut stream,
            out: MobOutput::default(),
        };
        monster_idles(&mut tick);
        tick.mob.body.no_action_time(100)
    };
    let zombie = entity_types::ZOMBIE as u16;
    assert_eq!(tick_of(zombie), 11 + 2);
    assert_eq!(tick_of(HORSE), 11);
}

#[test]
fn monsters_are_hostile_enemies_on_no_lead_and_villagers_take_no_lead_either() {
    for kind in [
        entity_types::ZOMBIE,
        entity_types::CREEPER,
        entity_types::GUARDIAN,
    ] {
        let kind = kind as u16;
        assert_eq!(ladder::sound_source(kind), 5, "hostile");
        assert!(ladder::is_enemy(kind));
        assert!(!ladder::can_be_leashed(kind));
    }
    for kind in [
        entity_types::ZOMBIE_HORSE,
        entity_types::COW,
        entity_types::IRON_GOLEM,
    ] {
        let kind = kind as u16;
        assert_eq!(ladder::sound_source(kind), 6, "neutral");
        assert!(!ladder::is_enemy(kind));
    }
    assert!(!ladder::can_be_leashed(entity_types::VILLAGER as u16));
    assert!(!ladder::can_be_leashed(
        entity_types::WANDERING_TRADER as u16
    ));
    assert!(ladder::can_be_leashed(entity_types::IRON_GOLEM as u16));
}
