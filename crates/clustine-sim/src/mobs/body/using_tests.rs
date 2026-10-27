//! A mob's use of an item over time, from `docs/groundwork/n9-plan.md` section 5.5 ("On
//! a mob") and the game's `LivingEntity` (`startUsingItem`, `updatingUsingItem`,
//! `updateUsingItem`, `completeUsingItem`, `stopUsingItem`) and `Consumable`
//! (`emitParticlesAndSounds`, `onConsume`): the part kept and read again, a use counted
//! down in the mob's tick before its AI and ended at its last tick, stopped when its
//! hand holds another item, and the living flags shown while it lasts.

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityId, ItemStack};

use super::{
    InUse, ItemUse, OFF_HAND, USING, is_using, release_using, start_using, stop_using, updating,
};
use crate::api::Hand;
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::tick::{Local, MobOutput, MobTick, tick as mob_tick};
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::of;
use crate::mobs::types::wandering_trader::{self, TraderState};
use crate::mobs::view::{LightAt, MobWorld, Seen};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::region::Random;
use crate::shown::{DataShown, index, mob_values};

const COW: u16 = entity_types::COW as u16;
const TRADER: u16 = entity_types::WANDERING_TRADER as u16;
const WITCH: u16 = entity_types::WITCH as u16;
const KEY: EntityId = EntityId(40);
/// A potion's and milk's use: 1.6 seconds.
const DRINK_TICKS: i32 = 32;

/// Open land of stone below 0 at a time of day, lit by the sky.
struct Field {
    day: u64,
}

impl World for Field {
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
        Some(LightAt { sky: 15, block: 0 })
    }

    fn day(&self) -> u64 {
        self.day
    }
}

/// Morning: bright outside.
const MORNING: u64 = 1_000;

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

/// A mob of the type `kind` at its full health, holding `main` and `off`.
fn holding(kind: u16, main: Option<ItemStack>, off: Option<ItemStack>) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = super::super::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = main;
    mob.body.equipment.slots[usize::from(slots::OFF_HAND)] = off;
    mob
}

/// A mob's tick of its own, kept across the ticks of a test.
struct Ticking {
    kind: u16,
    mob: Mob,
    living: Living,
    lived: u32,
    stream: Random,
}

impl Ticking {
    fn new(kind: u16, mob: Mob) -> Self {
        Self {
            kind,
            mob,
            living: living(kind),
            lived: 100,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

    /// Runs `step` on the mob as a tick has it, and gives what it put out.
    fn with(&mut self, world: &Field, step: impl FnOnce(&mut MobTick<'_>)) -> MobOutput {
        let mob = std::mem::replace(&mut self.mob, Mob::fresh());
        let living = std::mem::replace(&mut self.living, living(self.kind));
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: KEY,
            kind: self.kind,
            lived: self.lived,
            tick_count: self.lived,
            vehicle: None,
            carries: false,
            leashed: false,
            counted: false,
            mob,
            living,
            yaw: 0.0,
            pitch: 0.0,
            mob_type: of(self.kind),
            world,
            draws: &mut self.stream,
            out: MobOutput::default(),
        };
        step(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        tick.out
    }

    /// The mob's whole tick.
    fn tick(&mut self, world: &Field) -> MobOutput {
        let out = self.with(world, mob_tick);
        self.lived += 1;
        out
    }

    fn using(&self) -> Option<InUse> {
        self.mob.body.using.using
    }

    fn hand(&self, slot: u8) -> Option<i32> {
        self.mob.body.equipment.slots[usize::from(slot)]
            .as_ref()
            .map(|stack| stack.item)
    }
}

/// The game events a tick's output names, in order.
fn game_events(out: &MobOutput) -> Vec<&'static str> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::GameEventAt { name, .. } => Some(*name),
            _ => None,
        })
        .collect()
}

/// The living flags `mob_values` shows of `mob` of the type `kind`, with whether they
/// are the default; none where it shows none.
fn living_flags(kind: u16, mob: &Mob) -> Option<(DataShown, bool)> {
    mob_values(kind, &mob.encode())
        .into_iter()
        .find(|(at, _, _)| *at == index::LIVING_FLAGS)
        .map(|(_, value, default)| (value, default))
}

#[test]
fn the_use_part_round_trips_through_the_mob_s_record_and_a_fresh_one_is_left_out() {
    let mut mob = holding(COW, None, Some(ItemStack::new(items::MILK_BUCKET, 1)));
    let fresh = mob.encode();
    mob.body.using = ItemUse {
        using: Some(InUse {
            hand: Hand::Off,
            item: items::MILK_BUCKET,
            remaining: 17,
        }),
        flags: USING | OFF_HAND,
    };
    let kept = mob.encode();
    assert_ne!(kept, fresh);
    assert_eq!(Mob::decode(&kept).unwrap(), mob);
    // An off hand's bit left up after its use is kept too.
    mob.body.using = ItemUse {
        using: None,
        flags: OFF_HAND,
    };
    assert_eq!(Mob::decode(&mob.encode()).unwrap(), mob);
    // A mob that never used anything has no entry of it.
    mob.body.using = ItemUse::default();
    assert_eq!(mob.encode(), fresh);
    assert_eq!(Mob::decode(&fresh).unwrap().body.using, ItemUse::default());
}

#[test]
fn a_trader_s_record_written_with_its_drink_in_its_own_state_reads_with_the_use_dropped() {
    /// The trader's own state as it was kept before its use moved to the body.
    #[derive(serde::Serialize)]
    struct Before {
        despawn_delay: i32,
        wander_target: Option<BlockPos>,
        using: Option<(ItemStack, i32)>,
    }
    let animal = Animal::default();
    let mut bytes = postcard::to_allocvec(&animal).unwrap();
    bytes.extend(
        postcard::to_allocvec(&Before {
            despawn_delay: 4_000,
            wander_target: Some(BlockPos::new(3, -60, 9)),
            using: Some((ItemStack::new(items::POTION, 1), 12)),
        })
        .unwrap(),
    );
    let written = animal::write(
        &animal,
        &TraderState {
            despawn_delay: 1,
            wander_target: None,
        },
    );
    let mut mob = Mob::fresh();
    mob.state = clustine_world::TypeState {
        version: written.version,
        bytes,
    };
    let (_, own) = wandering_trader::state_of(&mob);
    assert_eq!(
        own,
        TraderState {
            despawn_delay: 4_000,
            wander_target: Some(BlockPos::new(3, -60, 9)),
        }
    );
    assert!(!is_using(&mob));
}

#[test]
fn a_drink_begun_counts_down_in_the_mob_s_tick_and_is_drunk_at_its_last() {
    let world = Field { day: MORNING };
    let mut cow = Ticking::new(
        COW,
        holding(COW, Some(ItemStack::new(items::MILK_BUCKET, 1)), None),
    );
    cow.mob.body.effects = Default::default();
    let added = effects::add(&mut cow.mob, COW, Effect::INVISIBILITY, 0, 600, 100);
    assert!(added);
    let out = cow.with(&world, |tick| start_using(tick, Hand::Main));
    assert_eq!(game_events(&out), ["minecraft:item_interact_start"]);
    assert_eq!(
        cow.using(),
        Some(InUse {
            hand: Hand::Main,
            item: items::MILK_BUCKET,
            remaining: DRINK_TICKS,
        })
    );
    // A second start while it lasts does nothing.
    let out = cow.with(&world, |tick| start_using(tick, Hand::Main));
    assert!(game_events(&out).is_empty());
    for left in (1..DRINK_TICKS).rev() {
        let out = cow.tick(&world);
        assert_eq!(cow.using().map(|using| using.remaining), Some(left));
        assert!(!game_events(&out).contains(&"minecraft:drink"));
        assert!(cow.mob.body.effects.has(Effect::INVISIBILITY));
    }
    let out = cow.tick(&world);
    assert_eq!(cow.using(), None);
    assert!(
        !cow.mob.body.effects.has(Effect::INVISIBILITY),
        "milk clears"
    );
    assert_eq!(cow.hand(slots::MAIN_HAND), Some(items::BUCKET));
    let events = game_events(&out);
    let drink = events.iter().position(|name| *name == "minecraft:drink");
    let finish = events
        .iter()
        .position(|name| *name == "minecraft:item_interact_finish");
    assert!(drink.is_some() && finish > drink, "{events:?}");
    // Nothing more happens of it.
    let out = cow.tick(&world);
    assert!(!game_events(&out).contains(&"minecraft:item_interact_finish"));
}

#[test]
fn a_drink_s_mouthful_draws_four_times_and_sounds_at_half_volume_at_the_last_draw_s_pitch() {
    let world = Field { day: MORNING };
    let mut cow = Ticking::new(
        COW,
        holding(COW, Some(ItemStack::new(items::MILK_BUCKET, 1)), None),
    );
    cow.with(&world, |tick| start_using(tick, Hand::Main));
    // 32 left: used 0, not past the first 7. At 24 left, 8 used: the first mouthful.
    cow.mob.body.using.using.as_mut().unwrap().remaining = 24;
    let mut expected = cow.stream.clone();
    let out = cow.with(&world, updating);
    let _volume = expected.next_bool();
    let _triangle = (expected.next_float(), expected.next_float());
    let pitch = expected.next_float() * (1.0 - 0.9) + 0.9;
    assert_eq!(
        out.events,
        [Event::Sound {
            sound: sounds::ENTITY_GENERIC_DRINK,
            volume: 0.5,
            pitch,
        }]
    );
    assert_eq!(cow.stream.next_float(), expected.next_float());
    // 23 left: not a fourth; nothing heard, nothing drawn.
    let mut expected = cow.stream.clone();
    let out = cow.with(&world, updating);
    assert!(out.events.is_empty());
    assert_eq!(cow.stream.next_float(), expected.next_float());
}

#[test]
fn a_trader_s_mouthfuls_are_its_own_sound() {
    let world = Field { day: MORNING };
    let mut trader = Ticking::new(
        TRADER,
        holding(TRADER, Some(ItemStack::new(items::MILK_BUCKET, 1)), None),
    );
    trader.with(&world, |tick| start_using(tick, Hand::Main));
    trader.mob.body.using.using.as_mut().unwrap().remaining = 24;
    let out = trader.with(&world, updating);
    assert!(matches!(
        out.events[..],
        [Event::Sound { sound, volume, .. }]
            if sound == sounds::ENTITY_WANDERING_TRADER_DRINK_MILK && volume == 0.5
    ));
}

#[test]
fn a_use_whose_hand_holds_another_item_or_none_stops_at_the_next_update() {
    let world = Field { day: MORNING };
    for other in [Some(ItemStack::new(items::POTION, 1)), None] {
        let mut cow = Ticking::new(
            COW,
            holding(COW, Some(ItemStack::new(items::MILK_BUCKET, 1)), None),
        );
        cow.with(&world, |tick| start_using(tick, Hand::Main));
        cow.tick(&world);
        assert_eq!(cow.using().map(|using| using.remaining), Some(31));
        cow.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = other.clone();
        let out = cow.tick(&world);
        assert_eq!(cow.using(), None);
        assert_eq!(cow.mob.body.using.flags, 0);
        assert_eq!(
            game_events(&out),
            ["minecraft:item_interact_finish"],
            "{other:?}"
        );
        // What its hand held is left as it is.
        assert_eq!(cow.hand(slots::MAIN_HAND), other.map(|stack| stack.item));
    }
}

#[test]
fn a_use_let_go_ends_with_its_flag_down_and_item_interact_finish_heard() {
    let world = Field { day: MORNING };
    let mut cow = Ticking::new(
        COW,
        holding(COW, Some(ItemStack::new(items::MILK_BUCKET, 1)), None),
    );
    cow.with(&world, |tick| start_using(tick, Hand::Main));
    cow.tick(&world);
    let out = cow.with(&world, release_using);
    assert_eq!(cow.using(), None);
    assert_eq!(cow.mob.body.using.flags, 0);
    assert_eq!(game_events(&out), ["minecraft:item_interact_finish"]);
    // Not drunk: still milk in its hand.
    assert_eq!(cow.hand(slots::MAIN_HAND), Some(items::MILK_BUCKET));
    // Let go again, with nothing in use: nothing heard.
    let out = cow.with(&world, release_using);
    assert!(game_events(&out).is_empty());
}

#[test]
fn an_empty_hand_begins_no_use() {
    let world = Field { day: MORNING };
    let mut cow = Ticking::new(COW, holding(COW, None, None));
    let out = cow.with(&world, |tick| start_using(tick, Hand::Off));
    assert!(out.local.is_empty());
    assert_eq!(cow.mob.body.using, ItemUse::default());
}

#[test]
fn the_living_flags_show_1_while_a_use_lasts_3_with_the_off_hand_and_the_using_bit_alone_goes_after()
 {
    let world = Field { day: MORNING };
    let milk = || Some(ItemStack::new(items::MILK_BUCKET, 1));
    let mut cow = Ticking::new(COW, holding(COW, milk(), milk()));
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(0), true))
    );

    cow.with(&world, |tick| start_using(tick, Hand::Main));
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(1), false))
    );
    cow.with(&world, stop_using);
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(0), true))
    );

    cow.with(&world, |tick| start_using(tick, Hand::Off));
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(3), false))
    );
    // `stopUsingItem` takes down the using bit only (`setLivingEntityFlag(1, false)`):
    // the off hand's stays until a use with the main hand begins.
    cow.with(&world, stop_using);
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(2), false))
    );
    cow.with(&world, |tick| start_using(tick, Hand::Main));
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(1), false))
    );
    // The whole drink, to its end: 0.
    for _ in 0..DRINK_TICKS {
        cow.tick(&world);
    }
    assert_eq!(cow.using(), None);
    assert_eq!(
        living_flags(COW, &cow.mob),
        Some((DataShown::Byte(0), true))
    );
}

#[test]
fn a_witch_shows_no_living_flags_and_its_values_are_as_before() {
    let mut witch = holding(WITCH, Some(ItemStack::new(items::POTION, 1)), None);
    let before = mob_values(WITCH, &witch.encode());
    assert!(before.iter().all(|(at, _, _)| *at != index::LIVING_FLAGS));
    // The body's six and the witch's own: the raider's two and its drinking.
    let indices: Vec<u8> = before.iter().map(|(at, _, _)| *at).collect();
    let mut expected = vec![
        index::AIR_SUPPLY,
        index::TICKS_FROZEN,
        index::HEALTH,
        index::EFFECT_PARTICLES,
        index::EFFECT_AMBIENCE,
        index::MOB_FLAGS,
    ];
    expected.extend(of(WITCH).shown(&witch).iter().map(|(at, _, _)| *at));
    expected.sort_unstable();
    assert_eq!(indices, expected);
    // Even a part that says otherwise is not shown for it.
    witch.body.using.flags = USING;
    assert_eq!(mob_values(WITCH, &witch.encode()), before);
}

#[test]
fn a_trader_s_drink_begun_by_its_ai_is_counted_first_in_the_next_tick_s_update_before_its_ai() {
    // Bright outside and invisible: it drinks milk.
    let world = Field { day: MORNING };
    let mut trader = Ticking::new(TRADER, holding(TRADER, None, None));
    effects::add(&mut trader.mob, TRADER, Effect::INVISIBILITY, 0, 6_000, 100);
    let mut began = None;
    for at in 0..10 {
        let out = trader.tick(&world);
        if trader.hand(slots::MAIN_HAND) == Some(items::MILK_BUCKET) {
            assert_eq!(game_events(&out), ["minecraft:item_interact_start"]);
            began = Some(at);
            break;
        }
    }
    assert!(began.is_some(), "it takes out its milk");
    // Begun in the AI, after this tick's update: all of it is left.
    assert_eq!(
        trader.using().map(|using| using.remaining),
        Some(DRINK_TICKS)
    );
    assert_eq!(
        living_flags(TRADER, &trader.mob),
        Some((DataShown::Byte(1), false))
    );
    trader.tick(&world);
    assert_eq!(
        trader.using().map(|using| using.remaining),
        Some(DRINK_TICKS - 1)
    );
    // Drunk at its last tick's update, and its goal, seeing no use in the same tick's AI,
    // empties its hand at once.
    let mut last = MobOutput::default();
    for _ in 1..DRINK_TICKS {
        last = trader.tick(&world);
    }
    assert!(!trader.mob.body.effects.has(Effect::INVISIBILITY));
    assert_eq!(trader.using(), None);
    assert_eq!(trader.hand(slots::MAIN_HAND), None);
    assert!(last.events.iter().any(|event| matches!(
        event,
        Event::Sound { sound, .. } if *sound == sounds::ENTITY_WANDERING_TRADER_REAPPEARED
    )));
}
