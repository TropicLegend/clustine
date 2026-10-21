//! The sniffer's, the allay's and the happy ghast's own rules on their state, from the
//! brief's sections 7.9 to 7.11 with 9.7.4 and 10.7.4: their type states, their synced
//! data with its indices and defaults, the draws of their `finalizeSpawn` (none of their
//! own), their interactions and sounds, and the parts of their ticks that need no brain.

use clustine_data::{EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityId, ItemStack, TypeState};

use super::allay::{self, AllayState};
use super::animal::{self, Animal};
use super::happy_ghast::{self, HappyGhastState, riding};
use super::sniffer::{self, SnifferState, State};
use super::{Did, Interaction, Spawning, Used, of};
use crate::living::{Kind, Living, slots};
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::tick::{Local, MobOutput, MobTick};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::shown::{DataShown, Serialiser};

mod brains;

const SNIFFER: u16 = entity_types::SNIFFER as u16;
const ALLAY: u16 = entity_types::ALLAY as u16;
const GHAST: u16 = entity_types::HAPPY_GHAST as u16;

fn draws(seed: u64) -> Random {
    Random::of_entity(seed, EntityId(5), 9)
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count: 1 })
}

/// Open air above a stone floor below y = 0.
struct Air;

impl World for Air {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
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

impl Holds for Air {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Air {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn players(&self) -> Vec<SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }
}

/// A body of the type `kind` standing at `position`.
fn body_of(kind: u16, position: Vec3) -> Living {
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
            key: EntityId(5),
            root: EntityId(5),
            like_a_vehicle: false,
        },
        steps: None,
    };
    Living::new(
        body,
        Kind::Mob {
            kind,
            hurts: Vec::new(),
        },
        info.eye_height,
    )
}

/// Runs `f` on a tick of `mob` of the type `kind` at `position`, its `tickCount` before
/// the tick `tick_count`; the mob and what the tick put out after.
fn in_tick(
    kind: u16,
    mob: Mob,
    position: Vec3,
    tick_count: u32,
    f: impl FnOnce(&mut MobTick<'_>),
) -> (Mob, MobOutput) {
    let world = Air;
    let mut stream = draws(1);
    let mut tick = MobTick {
        sensed: None,
        ridden: Default::default(),
        key: EntityId(5),
        kind,
        lived: tick_count,
        tick_count,
        yaw: 0.0,
        pitch: 0.0,
        vehicle: None,
        carries: false,
        leashed: false,
        counted: false,
        mob,
        living: body_of(kind, position),
        mob_type: of(kind),
        world: &world,
        draws: &mut stream,
        out: MobOutput::default(),
    };
    f(&mut tick);
    (tick.mob, tick.out)
}

fn use_with<'a>(
    kind: u16,
    mob: &'a mut Mob,
    held: Option<ItemStack>,
    stream: &'a mut Random,
) -> Used<'a> {
    Used {
        key: EntityId(5),
        kind,
        mob,
        player: EntityId(9),
        player_uuid: Some(77),
        game_time: 0,
        held,
        creative: true,
        sneaking: false,
        carries: false,
        passengers: 0,
        on_ground: true,
        from: [0.0, 0.0, 0.0],
        position: [1.0, 0.0, 1.0],
        moment: 100,
        draws: stream,
        did: Vec::new(),
    }
}

fn interact(kind: u16, mob: &mut Mob, held: Option<ItemStack>) -> (Interaction, Vec<Did>) {
    let mut stream = draws(1);
    let mut used = use_with(kind, mob, held, &mut stream);
    let result = of(kind).mob_interact(&mut used);
    (result, used.did)
}

fn sounds_of(did: &[Did]) -> Vec<(i32, f32, bool)> {
    did.iter()
        .filter_map(|done| match done {
            Did::Sound {
                sound,
                volume,
                but_the_player,
                ..
            } => Some((*sound, *volume, *but_the_player)),
            _ => None,
        })
        .collect()
}

fn sniffer_with(own: &SnifferState) -> Mob {
    let mut mob = Mob::fresh();
    mob.state = animal::write(&Animal::default(), own);
    mob
}

fn ghast_with(age: i32, own: &HappyGhastState) -> Mob {
    let mut shared = Animal::default();
    shared.set_age(age, 0);
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.state = animal::write(&shared, own);
    mob
}

#[test]
fn the_three_are_built_and_the_allay_alone_is_no_animal() {
    for kind in [SNIFFER, ALLAY, GHAST] {
        assert!(super::is_built(kind), "{kind}");
    }
    let mut record = clustine_world::Entity {
        key: EntityId(1),
        kind: ALLAY,
        uuid: uuid::Uuid::from_u128(1),
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: Mob::fresh().encode(),
    };
    assert!(animal::of_record(&record).is_none());
    for kind in [SNIFFER, GHAST] {
        record.kind = kind;
        assert!(animal::of_record(&record).is_some(), "{kind}");
    }
}

#[test]
fn a_fresh_one_of_each_keeps_no_type_state_and_a_changed_one_reads_back() {
    // A fresh mob writes nothing: an idle sniffer, an allay that may duplicate, a ghast
    // that is not still.
    assert_eq!(
        sniffer_with(&SnifferState::default()).state,
        TypeState::default()
    );
    let mut allay = Mob::fresh();
    allay::update(&mut allay, |_| {});
    assert_eq!(allay.state, TypeState::default());
    assert_eq!(
        ghast_with(0, &HappyGhastState::default()).state,
        TypeState::default()
    );
    // Each reads back what it wrote, at its version.
    let digging = SnifferState {
        state: State::Digging,
        drop_seed_at: 130,
    };
    assert_eq!(sniffer::read(&sniffer_with(&digging)), digging);
    let own = AllayState {
        dancing: true,
        jukebox: Some(BlockPos::new(1, 2, 3)),
        cooldown_until: Some(6100),
        inventory: one(items::DIAMOND),
        no_gravity: true,
    };
    allay::update(&mut allay, |kept| *kept = own.clone());
    assert_eq!(allay.state.version, 2);
    assert_eq!(allay::read(&allay), own);
    let still = HappyGhastState {
        still_timeout: 10,
        stays_still: true,
        leash_holder: true,
        impulse_at: 33,
    };
    assert_eq!(happy_ghast::read(&ghast_with(0, &still)), still);
    // A record of another version reads as a fresh one's.
    let mut other = allay.clone();
    other.state.version = 9;
    assert_eq!(allay::read(&other), AllayState::default());
}

#[test]
fn each_shows_its_synced_data_at_the_brief_s_indices_with_their_defaults() {
    // The sniffer: `AgeableMob`'s 16 and 17, its state (18, `SNIFFER_STATE`) and its
    // seed's tick (19).
    let mob = sniffer_with(&SnifferState::default());
    assert_eq!(
        of(SNIFFER).shown(&mob),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Serialised(Serialiser::SnifferState, 0), true),
            (19, DataShown::Int(0), true),
        ]
    );
    let digging = sniffer_with(&SnifferState {
        state: State::Digging,
        drop_seed_at: 130,
    });
    let shown = of(SNIFFER).shown(&digging);
    assert_eq!(
        shown[2],
        (
            18,
            DataShown::Serialised(Serialiser::SnifferState, 5),
            false
        )
    );
    assert_eq!(shown[3], (19, DataShown::Int(130), false));
    // The states' ids are their ordinals.
    let ids: Vec<i32> = [
        State::Idling,
        State::FeelingHappy,
        State::Scenting,
        State::Sniffing,
        State::Searching,
        State::Digging,
        State::Rising,
    ]
    .iter()
    .map(|state| state.id())
    .collect();
    assert_eq!(ids, [0, 1, 2, 3, 4, 5, 6]);
    // The allay: `NoGravity` (5), dancing (16), may duplicate (17, true by default).
    assert_eq!(
        of(ALLAY).shown(&Mob::fresh()),
        [
            (5, DataShown::Boolean(false), true),
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(true), true),
        ]
    );
    let mut waiting = Mob::fresh();
    allay::update(&mut waiting, |own| {
        own.dancing = true;
        own.cooldown_until = Some(6000);
    });
    let shown = of(ALLAY).shown(&waiting);
    assert_eq!(shown[1], (16, DataShown::Boolean(true), false));
    assert_eq!(shown[2], (17, DataShown::Boolean(false), false));
    // The happy ghast: the leash holder (18) and staying still (19).
    let ghast = ghast_with(0, &HappyGhastState::default());
    assert_eq!(
        of(GHAST).shown(&ghast),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Boolean(false), true),
            (19, DataShown::Boolean(false), true),
        ]
    );
    let held = ghast_with(
        -10,
        &HappyGhastState {
            leash_holder: true,
            stays_still: true,
            ..HappyGhastState::default()
        },
    );
    let shown = of(GHAST).shown(&held);
    assert_eq!(shown[0], (16, DataShown::Boolean(true), false));
    assert_eq!(shown[2], (18, DataShown::Boolean(true), false));
    assert_eq!(shown[3], (19, DataShown::Boolean(true), false));
}

#[test]
fn none_of_the_three_draws_anything_of_its_own_in_its_spawn_or_its_constructor() {
    for kind in [SNIFFER, ALLAY, GHAST] {
        let mut mob = Mob::fresh();
        let mut level = draws(3);
        let mut own = draws(4);
        let (level_before, own_before) = (level.clone(), own.clone());
        of(kind).construct(&mut mob, &mut own);
        for after in [false, true] {
            let mut spawn = Spawning {
                game_time: 0,
                mob: &mut mob,
                level: &mut level,
                own: &mut own,
                biome: None,
                at: BlockPos::new(0, -60, 0),
                day: 0,
            };
            if after {
                of(kind).after_finalize_spawn(&mut spawn);
            } else {
                of(kind).finalize_spawn(&mut spawn);
            }
        }
        assert_eq!(level, level_before, "{kind}");
        assert_eq!(own, own_before, "{kind}");
        assert_eq!(mob, Mob::fresh(), "{kind}");
    }
}

#[test]
fn a_sniffer_s_transitions_play_their_sounds_and_digging_sets_its_seed_tick_and_event_63() {
    let at = [0.5, 0.0, 0.5];
    let sounds = |out: &MobOutput| -> Vec<(i32, f32)> {
        out.events
            .iter()
            .filter_map(|event| match event {
                Event::Sound { sound, pitch, .. } => Some((*sound, *pitch)),
                _ => None,
            })
            .collect()
    };
    for (to, expected) in [
        (State::Idling, vec![]),
        (
            State::FeelingHappy,
            vec![(sounds::ENTITY_SNIFFER_HAPPY, 1.0)],
        ),
        (
            State::Scenting,
            vec![(sounds::ENTITY_SNIFFER_SCENTING, 1.0)],
        ),
        (
            State::Sniffing,
            vec![(sounds::ENTITY_SNIFFER_SNIFFING, 1.0)],
        ),
        (State::Searching, vec![]),
        (
            State::Rising,
            vec![(sounds::ENTITY_SNIFFER_DIGGING_STOP, 1.0)],
        ),
    ] {
        let (mob, out) = in_tick(SNIFFER, Mob::fresh(), at, 9, |tick| {
            sniffer::transition_to(tick, to);
        });
        assert_eq!(sniffer::read(&mob).state, to);
        assert_eq!(sounds(&out), expected, "{to:?}");
    }
    // A snifflet scents higher.
    let (_, out) = in_tick(SNIFFER, sniffer::hatchling(), at, 9, |tick| {
        sniffer::transition_to(tick, State::Scenting);
    });
    assert_eq!(sounds(&out), [(sounds::ENTITY_SNIFFER_SCENTING, 1.3)]);
    // Digging: the seed at the game's `tickCount` (10 in this tick) and 120, event 63.
    let (mob, out) = in_tick(SNIFFER, Mob::fresh(), at, 9, |tick| {
        sniffer::transition_to(tick, State::Digging);
    });
    assert_eq!(
        sniffer::read(&mob),
        SnifferState {
            state: State::Digging,
            drop_seed_at: 130,
        }
    );
    assert_eq!(out.events, [Event::Entity(63)]);
}

#[test]
fn a_digging_sniffer_drops_its_seed_at_its_tick_at_its_nose_s_corner_and_is_heard_every_tenth() {
    let digging = sniffer_with(&SnifferState {
        state: State::Digging,
        drop_seed_at: 130,
    });
    // Facing south (yaw 0), its nose is 2.25 ahead, its block that of its feet and a
    // fifth.
    let at = [0.5, 0.0, 0.5];
    assert_eq!(sniffer::head_block(at, 0.0, 0.0), BlockPos::new(0, 0, 2));
    let (_, out) = in_tick(SNIFFER, digging.clone(), at, 128, |tick| {
        of(SNIFFER).before_tick(tick);
    });
    assert!(out.local.is_empty() && out.events.is_empty());
    let (_, out) = in_tick(SNIFFER, digging.clone(), at, 129, |tick| {
        of(SNIFFER).before_tick(tick);
    });
    // The 130th tick: the game event of the tenth tick, then the seed.
    let mut seed_draws = draws(1);
    let seed = sniffer::seed(&mut seed_draws);
    assert!(matches!(
        seed.item,
        items::TORCHFLOWER_SEEDS | items::PITCHER_POD
    ));
    assert_eq!(
        out.local,
        [
            Local::GameEvent {
                name: "minecraft:entity_action",
                pos: BlockPos::new(0, 0, 2),
            },
            Local::Drop {
                stack: seed,
                at: [0.0, 0.0, 2.0],
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            },
        ]
    );
    assert_eq!(
        out.events,
        [Event::Sound {
            sound: sounds::ENTITY_SNIFFER_DROP_SEED,
            volume: 1.0,
            pitch: 1.0,
        }]
    );
    // An idle one does neither.
    let (_, out) = in_tick(SNIFFER, Mob::fresh(), at, 129, |tick| {
        of(SNIFFER).before_tick(tick);
    });
    assert!(out.local.is_empty() && out.events.is_empty());
    // The seed is one of two, about as often.
    let mut torch = 0;
    let mut random = draws(8);
    for _ in 0..400 {
        if sniffer::seed(&mut random).item == items::TORCHFLOWER_SEEDS {
            torch += 1;
        }
    }
    assert!((150..=250).contains(&torch), "{torch}");
}

#[test]
fn a_digging_sniffer_is_lower_by_0_4_with_eyes_at_0_81_and_a_snifflet_half_of_either() {
    let info = EntityTypeInfo::of(entity_types::SNIFFER);
    let digging = sniffer_with(&SnifferState {
        state: State::Digging,
        drop_seed_at: 0,
    });
    let size = of(SNIFFER).dimensions(&digging).unwrap();
    assert_eq!(size.width, info.width);
    assert_eq!(size.height, info.height - 0.4);
    assert_eq!(size.eye_height, 0.81);
    assert!(of(SNIFFER).dimensions(&Mob::fresh()).is_none());
    let baby = sniffer::hatchling();
    let size = of(SNIFFER).dimensions(&baby).unwrap();
    assert_eq!(size.width, info.width * 0.5);
    assert_eq!(size.height, info.height * 0.5);
    // A snifflet grows for twice an animal's time, from a hatching or an egg.
    assert_eq!(animal::animal_of(&baby.state).age, -48_000);
    let mut stream = draws(1);
    let mut breeding = super::Breeding {
        this: &Mob::fresh(),
        partner: &Mob::fresh(),
        partner_kind: SNIFFER,
        child_kind: None,
        draws: &mut stream,
        biome: None,
        egg_user: None,
    };
    let mut child = of(SNIFFER).breed_offspring(&mut breeding).unwrap();
    animal::made_baby(&mut child);
    assert_eq!(animal::animal_of(&child.state).age, -48_000);
    // Its breeding lays an egg, with its plop.
    let laid = of(SNIFFER).laid_from_breeding().unwrap();
    assert_eq!(laid.stack, one(items::SNIFFER_EGG).unwrap());
    assert_eq!(laid.sound, sounds::BLOCK_SNIFFER_EGG_PLOP);
    assert_eq!(laid.pitch_base, 0.5);
}

#[test]
fn a_sniffer_fed_torchflower_seeds_falls_in_love_and_eats_twice_each_of_a_level_draw() {
    let mut mob = sniffer_with(&SnifferState::default());
    let (result, did) = interact(SNIFFER, &mut mob, one(items::TORCHFLOWER_SEEDS));
    assert_eq!(result, Interaction::SuccessServer);
    assert!(animal::animal_of(&mob.state).in_love(100));
    let mut replay = draws(1);
    let first = replay.next_float() * (1.2 - 0.8) + 0.8;
    let second = replay.next_float() * (1.2 - 0.8) + 0.8;
    assert_eq!(did[0], Did::Event(18));
    let pitches: Vec<f32> = did
        .iter()
        .filter_map(|done| match done {
            Did::Sound { sound, pitch, .. } if *sound == sounds::ENTITY_SNIFFER_EAT => Some(*pitch),
            _ => None,
        })
        .collect();
    assert_eq!(pitches, [first, second]);
    // In love already, it takes nothing and eats nothing.
    let (result, did) = interact(SNIFFER, &mut mob, one(items::TORCHFLOWER_SEEDS));
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
    // Wheat is no food of a sniffer's.
    let mut other = sniffer_with(&SnifferState::default());
    assert_eq!(
        interact(SNIFFER, &mut other, one(items::WHEAT)).0,
        Interaction::Pass
    );
}

#[test]
fn sniffers_mate_only_while_idle_scenting_or_happy() {
    let record = |kind: u16| clustine_world::Entity {
        key: EntityId(1),
        kind,
        uuid: uuid::Uuid::from_u128(1),
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: Mob::fresh().encode(),
    };
    let (this, partner) = (record(SNIFFER), record(SNIFFER));
    for (state, mates) in [
        (State::Idling, true),
        (State::FeelingHappy, true),
        (State::Scenting, true),
        (State::Sniffing, false),
        (State::Searching, false),
        (State::Digging, false),
        (State::Rising, false),
    ] {
        let mob = sniffer_with(&SnifferState {
            state,
            drop_seed_at: 0,
        });
        let idle = sniffer_with(&SnifferState::default());
        let mate = |mob| super::Mate { entity: &this, mob };
        let other = super::Mate {
            entity: &partner,
            mob: &idle,
        };
        assert_eq!(of(SNIFFER).can_mate(mate(&mob), other), mates, "{state:?}");
    }
    // Its explored places: the newest first, twenty kept behind it.
    let mut explored = Vec::new();
    for i in 0..25 {
        explored = sniffer::store_explored(&explored, BlockPos::new(i, 0, 0));
    }
    assert_eq!(explored.len(), 21);
    assert_eq!(explored[0], BlockPos::new(24, 0, 0));
    assert_eq!(explored[20], BlockPos::new(4, 0, 0));
    assert!(sniffer::diggable(blocks::MOSS_BLOCK) && sniffer::diggable(blocks::GRASS_BLOCK));
    assert!(!sniffer::diggable(blocks::STONE));
}

#[test]
fn a_sniffer_s_sounds_are_its_own_and_it_is_quiet_while_it_digs_or_searches() {
    let sniffer = of(SNIFFER);
    let idle = sniffer_with(&SnifferState::default());
    assert_eq!(
        sniffer.ambient_sound(&idle),
        Some(sounds::ENTITY_SNIFFER_IDLE)
    );
    for state in [State::Digging, State::Searching] {
        let busy = sniffer_with(&SnifferState {
            state,
            drop_seed_at: 0,
        });
        assert_eq!(sniffer.ambient_sound(&busy), None);
    }
    assert_eq!(sniffer.step_sound(&idle), Some(sounds::ENTITY_SNIFFER_STEP));
    let source = DamageSource::of(DamageType("minecraft:generic"));
    assert_eq!(
        sniffer.hurt_sound(&idle, &source),
        Some(sounds::ENTITY_SNIFFER_HURT)
    );
    assert_eq!(
        sniffer.death_sound(&idle),
        Some(sounds::ENTITY_SNIFFER_DEATH)
    );
    assert_eq!(sniffer.head().max_y_rot, 50);
    assert!(sniffer.quad_leash(SNIFFER));
    // Dying, it is idle again.
    let mut digging = sniffer_with(&SnifferState {
        state: State::Digging,
        drop_seed_at: 5,
    });
    sniffer.before_die(&mut digging);
    assert_eq!(sniffer::read(&digging).state, State::Idling);
}

#[test]
fn an_allay_takes_one_of_what_it_is_given_likes_the_giver_and_gives_it_back_with_what_it_gathered()
{
    let mut mob = Mob::fresh();
    let given = ItemStack {
        item: items::DIAMOND,
        count: 5,
    };
    let (result, did) = interact(ALLAY, &mut mob, Some(given));
    assert_eq!(result, Interaction::Success);
    assert_eq!(mob.body.equipment.slots[0], one(items::DIAMOND));
    // The giver is liked, in its brain's memory.
    assert_eq!(allay::liked_player(&mob), Some(77));
    // Its sound is not sent to the giver, whose own client plays it.
    assert_eq!(
        sounds_of(&did),
        [(sounds::ENTITY_ALLAY_ITEM_GIVEN, 2.0, true)]
    );
    // Holding one, it takes nothing more.
    assert_eq!(
        interact(ALLAY, &mut mob, one(items::DIRT)).0,
        Interaction::Pass
    );
    // An empty hand takes it back; what it gathered falls at its eyes with no motion.
    allay::update(&mut mob, |own| {
        own.inventory = Some(ItemStack {
            item: items::DIAMOND,
            count: 3,
        });
    });
    let (result, did) = interact(ALLAY, &mut mob, None);
    assert_eq!(result, Interaction::Success);
    assert_eq!(mob.body.equipment.slots[0], None);
    assert_eq!(allay::read(&mob), AllayState::default());
    assert_eq!(allay::liked_player(&mob), None);
    let eyes = f64::from(EntityTypeInfo::of(entity_types::ALLAY).eye_height);
    assert_eq!(
        did,
        [
            Did::Sound {
                sound: sounds::ENTITY_ALLAY_ITEM_TAKEN,
                source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
                at: [1.0, 0.0, 1.0],
                volume: 2.0,
                pitch: 1.0,
                but_the_player: true,
            },
            Did::Throw {
                stack: ItemStack {
                    item: items::DIAMOND,
                    count: 3,
                },
                at: [1.0, eyes - 0.300_000_011_920_928_96, 1.0],
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                motion: [0.0; 3],
            },
            Did::Give {
                stack: one(items::DIAMOND).unwrap(),
                no_duplicates: false,
            },
        ]
    );
    // With empty hands on both sides, nothing.
    assert_eq!(interact(ALLAY, &mut mob, None).0, Interaction::Pass);
}

#[test]
fn a_dancing_allay_given_an_amethyst_shard_is_duplicated_and_both_wait_6000_ticks() {
    let mut mob = Mob::fresh();
    allay::update(&mut mob, |own| own.dancing = true);
    let (result, did) = interact(ALLAY, &mut mob, one(items::AMETHYST_SHARD));
    assert_eq!(result, Interaction::Success);
    let own = allay::read(&mob);
    assert_eq!(own.cooldown_until, Some(100 + 6000));
    assert!(!own.can_duplicate());
    assert_eq!(own.cooldown(100), 6000);
    assert_eq!(own.cooldown(6099), 1);
    assert_eq!(own.cooldown(6100), 0);
    // Its hand is still empty: the shard is not given.
    assert_eq!(mob.body.equipment.slots[0], None);
    let Did::Spawn { kind, mob: copy } = &did[0] else {
        panic!("a copy first: {did:?}");
    };
    assert_eq!(*kind, ALLAY);
    assert!(copy.body.flags.persistent);
    assert_eq!(allay::read(copy).cooldown_until, Some(6000));
    assert!(!allay::read(copy).dancing);
    assert_eq!(did[1], Did::Event(18));
    assert_eq!(
        sounds_of(&did),
        [(sounds::BLOCK_AMETHYST_BLOCK_CHIME, 2.0, true)]
    );
    // Waiting, a shard is neither a duplication nor, its hand being empty, refused: it
    // takes it as a gift.
    let (_, did) = interact(ALLAY, &mut mob, one(items::AMETHYST_SHARD));
    assert!(matches!(did[0], Did::Sound { sound, .. } if sound == sounds::ENTITY_ALLAY_ITEM_GIVEN));
    // Not dancing, the same.
    let mut still = Mob::fresh();
    let (_, did) = interact(ALLAY, &mut still, one(items::AMETHYST_SHARD));
    assert_eq!(still.body.equipment.slots[0], one(items::AMETHYST_SHARD));
    assert!(!did.iter().any(|done| matches!(done, Did::Spawn { .. })));
}

#[test]
fn an_allay_counts_its_cooldown_down_in_its_ai_step_and_may_duplicate_again_at_nothing() {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    allay::update(&mut mob, |own| own.cooldown_until = Some(50));
    // In the tick at 48, one is left after it.
    let (after, _) = in_tick(ALLAY, mob.clone(), [0.5, 0.0, 0.5], 48, |tick| {
        of(ALLAY).after_ai_step(tick);
    });
    assert!(!allay::read(&after).can_duplicate());
    let (after, _) = in_tick(ALLAY, mob, [0.5, 0.0, 0.5], 49, |tick| {
        of(ALLAY).after_ai_step(tick);
    });
    assert!(allay::read(&after).can_duplicate());
    // Hurt, it heals a point in every tenth tick of the game's count.
    let mut hurt = Mob::fresh();
    hurt.body.vitals.health = 10.0;
    for (tick_count, health) in [(8, 10.0), (9, 11.0)] {
        let (after, _) = in_tick(ALLAY, hurt.clone(), [0.5, 0.0, 0.5], tick_count, |tick| {
            of(ALLAY).after_ai_step(tick);
        });
        assert_eq!(after.body.vitals.health, health, "{tick_count}");
    }
    // A dance with no jukebox ends at a twentieth tick.
    let mut dancing = Mob::fresh();
    allay::update(&mut dancing, |own| own.dancing = true);
    for (tick_count, still) in [(18, true), (19, false)] {
        let (after, _) = in_tick(
            ALLAY,
            dancing.clone(),
            [0.5, 0.0, 0.5],
            tick_count,
            |tick| {
                of(ALLAY).after_ai_step(tick);
            },
        );
        assert_eq!(allay::read(&after).dancing, still, "{tick_count}");
    }
}

#[test]
fn an_allay_wants_only_what_its_hand_holds_while_its_slot_has_room_and_its_liked_player_cannot_hurt_it()
 {
    let mut mob = Mob::fresh();
    let diamonds = ItemStack {
        item: items::DIAMOND,
        count: 10,
    };
    assert!(!allay::wants_to_pick_up(&mob, diamonds));
    mob.body.equipment.slots[0] = one(items::DIAMOND);
    assert!(allay::wants_to_pick_up(&mob, diamonds));
    assert!(!allay::wants_to_pick_up(&mob, one(items::DIRT).unwrap()));
    allay::update(&mut mob, |own| {
        own.inventory = Some(ItemStack {
            item: items::DIAMOND,
            count: 64,
        });
    });
    assert!(!allay::wants_to_pick_up(&mob, diamonds));
    // It dies dropping what it gathered, then what it holds.
    let drops = of(ALLAY).drop_equipment(&mut mob);
    assert_eq!(
        drops,
        [
            ItemStack {
                item: items::DIAMOND,
                count: 64,
            },
            one(items::DIAMOND).unwrap(),
        ]
    );
    assert_eq!(mob.body.equipment.slots[0], None);
    // The player it likes does no harm; anyone else does.
    let mut memories = crate::mobs::brain::memories_of(&allay::PROVIDER, &mob, 1);
    memories.set(
        crate::mobs::brain::MemoryKind::LikedPlayer,
        crate::mobs::brain::Value::Uuid(77),
    );
    crate::mobs::brain::write_memories(&allay::PROVIDER, &mut mob, memories);
    let by = |player| {
        DamageSource::by(
            DamageType::PLAYER_ATTACK,
            Causer {
                key: EntityId(9),
                kind: entity_types::PLAYER as u16,
                position: [0.0; 3],
                player: Some(player),
                living: true,
            },
        )
    };
    assert!(of(ALLAY).refuses_hurt(&mob, &by(77)));
    assert!(!of(ALLAY).refuses_hurt(&mob, &by(78)));
    assert!(!of(ALLAY).refuses_hurt(&Mob::fresh(), &by(77)));
}

#[test]
fn an_allay_sounds_with_its_item_or_without_at_four_tenths() {
    let allay = of(ALLAY);
    let mut mob = Mob::fresh();
    assert_eq!(
        allay.ambient_sound(&mob),
        Some(sounds::ENTITY_ALLAY_AMBIENT_WITHOUT_ITEM)
    );
    mob.body.equipment.slots[0] = one(items::DIAMOND);
    assert_eq!(
        allay.ambient_sound(&mob),
        Some(sounds::ENTITY_ALLAY_AMBIENT_WITH_ITEM)
    );
    let source = DamageSource::of(DamageType("minecraft:generic"));
    assert_eq!(
        allay.hurt_sound(&mob, &source),
        Some(sounds::ENTITY_ALLAY_HURT)
    );
    assert_eq!(allay.death_sound(&mob), Some(sounds::ENTITY_ALLAY_DEATH));
    assert_eq!(allay.sound_volume_of(&mob), 0.4);
    assert!(!allay.remove_when_far_away(ALLAY, &mob, 100_000, 1000.0));
    assert!(clustine_blocks::data::entity_in_tag(
        entity_types::ALLAY,
        "minecraft:fall_damage_immune"
    ));
    // Its flight: `travelFlying` at its speed in the air and 0.02 in fluids.
    let (_, _) = in_tick(ALLAY, Mob::fresh(), [0.5, 0.0, 0.5], 0, |tick| {
        tick.mob.body.inputs.speed = 0.25;
        assert_eq!(
            allay.travel(tick),
            crate::living::Travel::Flying {
                water: 0.02,
                lava: 0.02,
                air: 0.25,
            }
        );
    });
}

#[test]
fn a_grown_happy_ghast_takes_a_harness_and_a_ghastling_does_not_and_is_grown_by_snowballs() {
    let mut adult = ghast_with(0, &HappyGhastState::default());
    let (result, did) = interact(GHAST, &mut adult, one(items::WHITE_HARNESS));
    assert_eq!(result, Interaction::Success);
    let body = usize::from(slots::BODY);
    assert_eq!(adult.body.equipment.slots[body], one(items::WHITE_HARNESS));
    assert_eq!(adult.body.equipment.drop_chances.unwrap()[body], 2.0);
    assert!(happy_ghast::harnessed(&adult));
    assert_eq!(
        sounds_of(&did),
        [(sounds::ENTITY_HAPPY_GHAST_EQUIP, 1.0, false)]
    );
    assert!(did.contains(&Did::GameEvent("minecraft:equip")));
    // A second does not go on: the click, harnessed, takes the player on instead.
    let (result, did) = interact(GHAST, &mut adult, one(items::BLACK_HARNESS));
    assert_eq!(result, Interaction::Success);
    assert_eq!(did, [Did::Mount]);
    // A grown one never falls in love with snowballs.
    let mut bare = ghast_with(0, &HappyGhastState::default());
    assert_eq!(
        interact(GHAST, &mut bare, one(items::SNOWBALL)).0,
        Interaction::Pass
    );
    assert!(!animal::animal_of(&bare.state).in_love(100));
    // A ghastling takes no harness, and a snowball takes a tenth off its time.
    let mut baby = ghast_with(-24_000, &HappyGhastState::default());
    assert_eq!(
        interact(GHAST, &mut baby, one(items::WHITE_HARNESS)).0,
        Interaction::Pass
    );
    assert_eq!(baby.body.equipment.slots[body], None);
    let (result, _) = interact(GHAST, &mut baby, one(items::SNOWBALL));
    assert_eq!(result, Interaction::Success);
    // From -24000 at 0, at 100 it is -23900; a tenth of that in whole seconds, 119.
    assert_eq!(animal::animal_of(&baby.state).age(100), -23_900 + 119 * 20);
}

#[test]
fn a_happy_ghast_stays_still_for_ten_ticks_after_a_player_and_says_when_it_begins() {
    let mut own = HappyGhastState::default();
    assert!(!own.on_still_timeout());
    assert!(own.set_server_still_timeout(10));
    assert!(own.on_still_timeout() && own.stays_still);
    // Again while still: it does not begin again.
    assert!(!own.set_server_still_timeout(10));
    assert!(!own.set_server_still_timeout(0));
    assert!(!own.stays_still && !own.on_still_timeout());
    // Its tick counts it down after its first 60 ticks, never before.
    let still = ghast_with(
        0,
        &HappyGhastState {
            still_timeout: 3,
            stays_still: true,
            ..HappyGhastState::default()
        },
    );
    for (tick_count, left) in [(10, 3), (59, 3), (60, 2)] {
        let (after, _) = in_tick(GHAST, still.clone(), [0.5, 0.0, 0.5], tick_count, |tick| {
            of(GHAST).after_tick(tick);
        });
        assert_eq!(
            happy_ghast::read(&after).still_timeout,
            left,
            "{tick_count}"
        );
    }
    let last = ghast_with(
        0,
        &HappyGhastState {
            still_timeout: 1,
            stays_still: true,
            ..HappyGhastState::default()
        },
    );
    let (after, _) = in_tick(GHAST, last, [0.5, 0.0, 0.5], 100, |tick| {
        of(GHAST).after_tick(tick);
    });
    assert_eq!(happy_ghast::read(&after), HappyGhastState::default());
    // Its yaw snaps to the nearest quarter turn.
    assert_eq!(happy_ghast::wrap_degrees_90(100.0), 10.0);
    assert_eq!(happy_ghast::wrap_degrees_90(50.0), -40.0);
    assert_eq!(happy_ghast::wrap_degrees_90(-50.0), 40.0);
    assert_eq!(happy_ghast::wrap_degrees_90(-45.0), -45.0);
}

#[test]
fn a_happy_ghast_shows_it_holds_a_quad_leash_for_four_of_its_ticks_after_the_last_notice() {
    let ghast = of(GHAST);
    assert!(ghast.quad_leash_holder());
    assert_eq!(ghast.leash_distances(), (10.0, 16.0));
    let mut mob = ghast_with(0, &HappyGhastState::default());
    let mut time = happy_ghast::LEASH_HOLDER_TIME;
    let mut shown = Vec::new();
    for _ in 0..6 {
        ghast.leash_holder_tick(&mut mob, &mut time);
        shown.push(happy_ghast::read(&mob).leash_holder);
    }
    assert_eq!(shown, [true, true, true, true, false, false]);
    // A pull of its leash makes its move control wait.
    mob.body.controls.movement.operation = crate::mobs::ai::Operation::MoveTo;
    ghast.on_elastic_leash_pull(&mut mob);
    assert_eq!(
        mob.body.controls.movement.operation,
        crate::mobs::ai::Operation::Wait
    );
    // Others hold by one, but for the horses that are no llamas, held by four.
    assert!(!of(entity_types::COW as u16).quad_leash_holder());
    assert!(!of(entity_types::COW as u16).quad_leash(entity_types::COW as u16));
    assert!(of(entity_types::HORSE as u16).quad_leash(entity_types::HORSE as u16));
    assert!(!of(entity_types::LLAMA as u16).quad_leash(entity_types::LLAMA as u16));
    assert_eq!(of(entity_types::COW as u16).leash_distances(), (6.0, 12.0));
}

#[test]
fn riding_a_happy_ghast_goes_along_the_rider_s_pitch_whatever_the_key_s_size() {
    let speed = 0.05;
    let scale = 3.900_000_095_367_431_6 * speed;
    // Ahead at a level pitch: straight on.
    let ahead = riding::ridden_input(0.0, 0.3, 0.0, false, speed);
    assert_eq!(ahead, [0.0, -0.0 * scale, scale]);
    // Back: half, and up a half a block while jumping.
    let back = riding::ridden_input(1.0, -1.0, 0.0, true, speed);
    assert_eq!(back, [scale, 0.5 * scale, -0.5 * scale]);
    // Looking down, it goes down.
    let down = riding::ridden_input(0.0, 1.0, 90.0, false, speed);
    assert!(down[1] < -0.99 * scale && down[2].abs() < 1.0e-3);
    assert_eq!(riding::ridden_rotation(40.0, 10.0), (20.0, 10.0));
    assert_eq!(riding::ridden_yaw(0.0, 100.0), 8.0);
    assert!(riding::can_add_passenger(3) && !riding::can_add_passenger(4));
    assert_eq!(riding::seats().len(), 4);
    let mut mob = ghast_with(0, &HappyGhastState::default());
    assert!(!riding::steered(&mob, true));
    mob.body.equipment.slots[usize::from(slots::BODY)] = one(items::RED_HARNESS);
    assert!(riding::steered(&mob, true) && !riding::steered(&mob, false));
    // The first rider's goggles down, the last's up; getting off holds it still.
    assert_eq!(
        riding::passenger_added(&mut mob, false, false),
        Some(sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN)
    );
    assert_eq!(riding::passenger_added(&mut mob, true, false), None);
    assert_eq!(riding::passenger_removed(&mut mob, 1), None);
    assert_eq!(happy_ghast::read(&mob).still_timeout, 10);
    assert!(!riding::steered(&mob, true));
    assert_eq!(
        riding::passenger_removed(&mut mob, 0),
        Some(sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP)
    );
    // Still, it is solid; a ghastling never is.
    assert!(riding::can_be_collided_with(&mob, false, false));
    let baby = ghast_with(-100, &HappyGhastState::default());
    assert!(!riding::can_be_collided_with(&baby, true, true));
    let moving = ghast_with(0, &HappyGhastState::default());
    assert!(!riding::can_be_collided_with(&moving, false, false));
    assert!(riding::can_be_collided_with(&moving, true, true));
    assert_eq!(riding::dismount_at([1.0, 2.0, 3.0], 4.0), [1.0, 6.0, 3.0]);
}

#[test]
fn a_ghastling_is_small_breathes_under_water_and_speaks_softer_than_a_grown_happy_ghast() {
    let ghast = of(GHAST);
    let info = EntityTypeInfo::of(entity_types::HAPPY_GHAST);
    let baby = ghast_with(-100, &HappyGhastState::default());
    let adult = ghast_with(0, &HappyGhastState::default());
    let size = ghast.dimensions(&baby).unwrap();
    assert_eq!(size.width, info.width * 0.2375);
    assert_eq!(size.height, info.height * 0.2375);
    assert_eq!(size.eye_height, 0.468_75);
    assert!(ghast.dimensions(&adult).is_none());
    assert!(ghast.breathes_under_water(&baby) && !ghast.breathes_under_water(&adult));
    assert_eq!(ghast.sound_volume_of(&baby), 1.0);
    assert_eq!(ghast.sound_volume_of(&adult), 4.0);
    assert_eq!(
        ghast.ambient_sound(&baby),
        Some(sounds::ENTITY_GHASTLING_AMBIENT)
    );
    assert_eq!(
        ghast.ambient_sound(&adult),
        Some(sounds::ENTITY_HAPPY_GHAST_AMBIENT)
    );
    let source = DamageSource::of(DamageType("minecraft:generic"));
    assert_eq!(
        ghast.hurt_sound(&baby, &source),
        Some(sounds::ENTITY_GHASTLING_HURT)
    );
    assert_eq!(
        ghast.death_sound(&adult),
        Some(sounds::ENTITY_HAPPY_GHAST_DEATH)
    );
    // In the overworld's clouds (192.33 high) it heals fast.
    assert!(happy_ghast::in_clouds(190.0, 4.0));
    assert!(!happy_ghast::in_clouds(197.0, 4.0));
    assert!(!happy_ghast::in_clouds(100.0, 4.0));
    // It flies at five thirds of its flying speed everywhere.
    let (_, _) = in_tick(GHAST, adult, [0.5, 0.0, 0.5], 0, |tick| {
        let speed = 0.05_f32 * 5.0 / 3.0;
        assert_eq!(
            ghast.travel(tick),
            crate::living::Travel::Flying {
                water: speed,
                lava: speed,
                air: speed,
            }
        );
    });
}
