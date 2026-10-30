//! The skeletons, the pillager, the witch and the guardians from the game's rules
//! (`docs/groundwork/n9-plan.md`, sections 4.6, 5.3 and 8.4; n7 `RangedBowAttackGoal`,
//! `RangedCrossbowAttackGoal`, `Witch`, `Guardian$GuardianAttackGoal`; n7k
//! `AbstractSkeleton`, `Stray`, `Bogged`, `Parched`; n9 `CrossbowItem`,
//! `CrossbowAttackMob`, `LivingEntity.releaseUsingItem`), not from the code: each goal's
//! rhythm, its draws in their order and tick, and what it shoots; and the tipped arrows
//! of the variants' loot.

use clustine_data::{EntityTypeInfo, attributes as ids, entity_types, items, sounds};
use clustine_world::components::{Component, EffectInstance, Kind as Of, PotionContents};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack};

use super::guardians::{self, BEAM_EVENT, GuardianAttackGoal};
use super::illagers;
use super::pillager::{self, PillagerState};
use super::witch::{self, Potion, WitchState};
use super::{of, skeletons};
use crate::living::{Kind, Living, MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::ai::ranged_weapons::{
    CrossbowAttacking, CrossbowState, RangedBowAttackGoal, RangedCrossbowAttackGoal,
};
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::{self, Looked};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::loot::{self, Facts, Subject};
use crate::mobs::body::{Event, attribute, using};
use crate::mobs::tick::{Local, MobOutput, MobTick, Shot};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body as Moving;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::thrown::BELOW_THE_EYES;
use crate::physics::{Vec3, World};
use crate::region::Random;
use crate::shown::DataShown;

const KEY: EntityId = EntityId(40);
const TARGET: EntityId = EntityId(77);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// Open stone ground below y = 0 and air above, with the entities a test puts on it.
#[derive(Default)]
struct Field {
    entities: Vec<(Entity, Aabb)>,
}

impl World for Field {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        Some(if pos.y < 0 {
            clustine_data::blocks::STONE
        } else {
            clustine_data::blocks::AIR
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
}

impl Field {
    /// A field with one target of the type `id` standing at `feet`, of `health`, moving
    /// by `motion`, with `mob`'s effects.
    fn with_target(id: i32, feet: Vec3, health: f32, motion: Vec3, mob: Option<Mob>) -> Self {
        let mut mob = mob.unwrap_or_else(Mob::fresh);
        mob.body.vitals.health = health;
        let info = EntityTypeInfo::of(id);
        let entity = Entity {
            portal: Default::default(),
            key: TARGET,
            kind: kind(id),
            uuid: uuid::Uuid::from_u128(77),
            position: feet,
            motion,
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
        Self {
            entities: vec![(entity, Aabb::of_entity(feet, info.width, info.height))],
        }
    }

    /// A zombie of 20 health standing still at `feet`.
    fn zombie_at(feet: Vec3) -> Self {
        Self::with_target(entity_types::ZOMBIE, feet, 20.0, [0.0; 3], None)
    }
}

/// A body of the type `id` at rest at `at`, its first tick behind it.
fn body(id: i32, at: Vec3) -> Living {
    let info = EntityTypeInfo::of(id);
    let body = Moving {
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
            kind: kind(id),
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living
}

/// A mob of the type `id` holding `main` and targeting the field's target.
fn armed(main: Option<ItemStack>) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = MAX_AIR;
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = main;
    mob.body.combat.target = Some(TARGET);
    mob
}

/// A mob ticked again and again, its stream carried from one tick to the next.
struct Shooter {
    kind: u16,
    mob: Mob,
    living: Living,
    yaw: f32,
    pitch: f32,
    lived: u32,
    stream: Random,
}

impl Shooter {
    fn new(id: i32, mob: Mob, at: Vec3) -> Self {
        Self {
            kind: kind(id),
            mob,
            living: body(id, at),
            yaw: 0.0,
            pitch: 0.0,
            lived: 100,
            stream: Random::of_entity(7, KEY, 1),
        }
    }

    /// `f` on the mob's tick; what it gave, what the tick made, and the draws it took.
    fn with<R>(
        &mut self,
        world: &Field,
        f: impl FnOnce(&mut MobTick<'_>) -> R,
    ) -> (R, MobOutput, u64) {
        let start = self.stream.clone();
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
            mob: std::mem::replace(&mut self.mob, Mob::fresh()),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: of(self.kind),
            world,
            draws: &mut self.stream,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let given = f(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        let out = tick.out;
        (given, out, self.stream.draws_since(&start))
    }

    /// `f` in the mob's AI step.
    fn ai<R>(
        &mut self,
        world: &Field,
        f: impl FnOnce(&mut Ai<'_, '_>) -> R,
    ) -> (R, MobOutput, u64) {
        let mob_type = of(self.kind);
        let mut setup = Setup::default();
        mob_type.register_goals_of(&self.mob, &mut setup);
        let config = setup.config;
        self.with(world, |tick| {
            let mut ai = Ai {
                tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            f(&mut ai)
        })
    }

    /// One tick of `goal` as the mob's tick runs it: its use updated before its AI step
    /// (`updatingUsingItem`), then the goal's tick; the tick's count after.
    fn goal_tick<G: Goal>(
        &mut self,
        world: &Field,
        goal: &G,
        state: &mut G::State,
    ) -> (MobOutput, u64) {
        let ((), used, drawn_by_use) = self.with(world, using::updating);
        let ((), mut out, drawn) = self.ai(world, |ai| goal.tick(state, ai));
        let mut all = used;
        all.local.append(&mut out.local);
        all.events.append(&mut out.events);
        self.lived += 1;
        (all, drawn_by_use + drawn)
    }

    /// The field's target as the mob reads it.
    fn target(&mut self, world: &Field) -> Looked {
        self.ai(world, |ai| sensing::target(ai))
            .0
            .expect("the target is seen")
    }
}

fn shots(out: &MobOutput) -> Vec<Shot> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Shoot(shot) => Some(shot.clone()),
            _ => None,
        })
        .collect()
}

fn sounds_of(out: &MobOutput) -> Vec<(i32, f32, f32)> {
    out.events
        .iter()
        .filter_map(|event| match event {
            Event::Sound {
                sound,
                volume,
                pitch,
            } => Some((*sound, *volume, *pitch)),
            _ => None,
        })
        .collect()
}

fn effects_of(stack: &ItemStack) -> Vec<EffectInstance> {
    match stack.components.get(Of::PotionContents) {
        Some(Component::PotionContents(contents)) => contents.custom_effects.clone(),
        _ => Vec::new(),
    }
}

fn potion_of(stack: &ItemStack) -> Option<String> {
    match stack.components.get(Of::PotionContents) {
        Some(Component::PotionContents(contents)) => contents.potion.clone(),
        _ => None,
    }
}

fn bow_goal(interval: i32) -> RangedBowAttackGoal {
    RangedBowAttackGoal {
        speed: 1.0,
        attack_interval_min: interval,
        radius: 15.0,
        perform: skeletons::perform_ranged_attack,
    }
}

/// The ticks at which a skeleton of the type `id` with its goal at `interval` shoots at a
/// zombie 10 blocks off within `ticks` ticks of its goal.
fn shot_ticks(id: i32, interval: i32, ticks: u32) -> Vec<u32> {
    let world = Field::zombie_at([10.5, 0.0, 0.5]);
    let mut skeleton = Shooter::new(
        id,
        armed(Some(ItemStack::new(items::BOW, 1))),
        [0.5, 0.0, 0.5],
    );
    let goal = bow_goal(interval);
    let mut state = Default::default();
    assert!(skeleton.ai(&world, |ai| goal.can_use(&mut state, ai)).0);
    skeleton.ai(&world, |ai| goal.start(&mut state, ai));
    (0..ticks)
        .filter(|_| !shots(&skeleton.goal_tick(&world, &goal, &mut state).0).is_empty())
        .collect()
}

#[test]
fn a_skeleton_draws_at_its_goal_s_first_tick_looses_twenty_ticks_on_and_again_every_sixty_at_easy()
{
    // Drawn at tick 0 (its attack time −1 counted to −2), loosed at 20 ticks of use;
    // then 40 ticks of waiting and 20 of drawing.
    assert_eq!(shot_ticks(entity_types::SKELETON, 40, 150), [20, 80, 140]);
    assert_eq!(shot_ticks(entity_types::STRAY, 40, 150), [20, 80, 140]);
}

#[test]
fn the_bogged_and_the_parched_wait_seventy_ticks_between_their_draws() {
    assert_eq!(shot_ticks(entity_types::BOGGED, 70, 201), [20, 110, 200]);
    assert_eq!(shot_ticks(entity_types::PARCHED, 70, 201), [20, 110, 200]);
}

#[test]
fn each_skeleton_type_registers_its_bow_goal_at_4_last_or_the_melee_goal_with_no_bow() {
    for id in [
        entity_types::SKELETON,
        entity_types::STRAY,
        entity_types::BOGGED,
        entity_types::PARCHED,
    ] {
        let mut with_bow = Setup::default();
        of(kind(id)).register_goals_of(&armed(Some(ItemStack::new(items::BOW, 1))), &mut with_bow);
        let mut without = Setup::default();
        of(kind(id)).register_goals_of(&armed(None), &mut without);
        let priorities = |setup: &Setup| -> Vec<i32> {
            setup
                .goals
                .iter()
                .map(|registered| registered.priority)
                .collect()
        };
        assert_eq!(priorities(&with_bow), [2, 3, 3, 5, 6, 6, 4], "{id}");
        assert_eq!(priorities(&without), priorities(&with_bow));
    }
    // With a bow the skeleton's goal draws it; without, its melee goal takes the target
    // within reach.
    let world = Field::zombie_at([1.5, 0.0, 0.5]);
    let steps = |shooter: &mut Shooter| {
        for _ in 0..2 {
            shooter.with(&world, crate::mobs::ai::server_ai_step);
            shooter.lived += 1;
        }
    };
    let mut unarmed = Shooter::new(entity_types::SKELETON, armed(None), [0.5, 0.0, 0.5]);
    steps(&mut unarmed);
    assert!(unarmed.mob.body.combat.aggressive);
    assert!(!using::is_using(&unarmed.mob));
    let mut archer = Shooter::new(
        entity_types::SKELETON,
        armed(Some(ItemStack::new(items::BOW, 1))),
        [0.5, 0.0, 0.5],
    );
    steps(&mut archer);
    assert!(archer.mob.body.combat.aggressive);
    assert!(
        using::is_using(&archer.mob),
        "its goal's first tick draws the bow"
    );
    assert_eq!(archer.mob.body.using.flags & using::USING, using::USING);
}

#[test]
fn a_skeleton_s_arrow_is_aimed_at_the_target_s_third_up_from_a_tenth_below_its_eyes_at_1_6_and_10()
{
    let world = Field::zombie_at([7.5, 0.0, -3.25]);
    let bow = ItemStack::new(items::BOW, 1);
    let mut skeleton = Shooter::new(
        entity_types::SKELETON,
        armed(Some(bow.clone())),
        [0.5, 0.0, 0.5],
    );
    let target = skeleton.target(&world);
    let mut replay = skeleton.stream.clone();
    let ((), out, drawn) = skeleton.ai(&world, |ai| {
        skeletons::perform_ranged_attack(ai, &target, 0.75)
    });
    assert_eq!(drawn, 1, "the sound's pitch alone: the arrow draws its own");
    let eyes = f64::from(EntityTypeInfo::of(entity_types::SKELETON).eye_height);
    let at = [0.5, eyes - BELOW_THE_EYES, 0.5];
    let zombie = f64::from(EntityTypeInfo::of(entity_types::ZOMBIE).height);
    let (dx, dz) = (7.0, -3.75);
    let dy = zombie * 0.333_333_333_333_333_3 - at[1];
    let across = f64::sqrt(dx * dx + dz * dz);
    assert_eq!(
        shots(&out),
        [Shot {
            kind: kind(entity_types::ARROW),
            stack: ItemStack::new(items::ARROW, 1),
            weapon: Some(bow),
            at,
            direction: [dx, dy + across * 0.200_000_002_980_232_24, dz],
            power: 1.6,
            uncertainty: 10.0,
            from_mob: Some(0.75),
        }]
    );
    let pitch = 1.0 / (replay.next_float() * 0.4 + 0.8);
    assert_eq!(
        sounds_of(&out),
        [(sounds::ENTITY_SKELETON_SHOOT, 1.0, pitch)]
    );
}

#[test]
fn a_skeleton_shoots_the_arrows_it_holds_in_its_off_hand_and_takes_none_of_them() {
    let world = Field::zombie_at([5.5, 0.0, 0.5]);
    let mut mob = armed(Some(ItemStack::new(items::BOW, 1)));
    mob.body.equipment.slots[usize::from(slots::OFF_HAND)] =
        Some(ItemStack::new(items::SPECTRAL_ARROW, 3));
    let mut skeleton = Shooter::new(entity_types::STRAY, mob, [0.5, 0.0, 0.5]);
    let target = skeleton.target(&world);
    let ((), out, _) = skeleton.ai(&world, |ai| {
        skeletons::perform_ranged_attack(ai, &target, 1.0)
    });
    let shot = &shots(&out)[0];
    assert_eq!(shot.kind, kind(entity_types::SPECTRAL_ARROW));
    assert_eq!(shot.stack, ItemStack::new(items::SPECTRAL_ARROW, 1));
    assert!(
        effects_of(&shot.stack).is_empty(),
        "a spectral arrow is no `Arrow`"
    );
    // `getProjectile` copies nothing out of the hand: a mob's bow uses no ammunition.
    assert_eq!(
        skeleton.mob.body.equipment.slots[usize::from(slots::OFF_HAND)],
        Some(ItemStack::new(items::SPECTRAL_ARROW, 3))
    );
}

#[test]
fn the_stray_s_bogged_s_and_parched_s_arrows_carry_slowness_poison_and_weakness() {
    let world = Field::zombie_at([5.5, 0.0, 0.5]);
    let effect = |name: &str, duration| EffectInstance {
        effect: name.to_owned(),
        amplifier: 0,
        duration,
        ambient: false,
        visible: true,
        show_icon: true,
    };
    for (id, expected) in [
        (entity_types::SKELETON, vec![]),
        (entity_types::STRAY, vec![effect("minecraft:slowness", 600)]),
        (entity_types::BOGGED, vec![effect("minecraft:poison", 100)]),
        (
            entity_types::PARCHED,
            vec![effect("minecraft:weakness", 600)],
        ),
    ] {
        let mut skeleton = Shooter::new(
            id,
            armed(Some(ItemStack::new(items::BOW, 1))),
            [0.5, 0.0, 0.5],
        );
        let target = skeleton.target(&world);
        let ((), out, _) = skeleton.ai(&world, |ai| {
            skeletons::perform_ranged_attack(ai, &target, 1.0)
        });
        let shot = &shots(&out)[0];
        assert_eq!(shot.stack.item, items::ARROW, "{id}");
        assert_eq!(effects_of(&shot.stack), expected, "{id}");
    }
    // A tipped arrow held keeps its potion and gains the type's effect after its own.
    let mut mob = armed(Some(ItemStack::new(items::BOW, 1)));
    let mut tipped = ItemStack::new(items::TIPPED_ARROW, 2);
    tipped
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:healing".to_owned()),
            ..PotionContents::default()
        }));
    mob.body.equipment.slots[usize::from(slots::OFF_HAND)] = Some(tipped);
    let mut stray = Shooter::new(entity_types::STRAY, mob, [0.5, 0.0, 0.5]);
    let target = stray.target(&world);
    let ((), out, _) = stray.ai(&world, |ai| {
        skeletons::perform_ranged_attack(ai, &target, 1.0)
    });
    let shot = &shots(&out)[0];
    assert_eq!(shot.stack.item, items::TIPPED_ARROW);
    assert_eq!(potion_of(&shot.stack).as_deref(), Some("minecraft:healing"));
    assert_eq!(effects_of(&shot.stack), [effect("minecraft:slowness", 600)]);
}

#[test]
fn a_strafing_skeleton_draws_two_floats_every_twentieth_tick_of_it_and_none_between() {
    let world = Field::zombie_at([8.5, 0.0, 0.5]);
    let mut skeleton = Shooter::new(
        entity_types::SKELETON,
        armed(Some(ItemStack::new(items::BOW, 1))),
        [0.5, 0.0, 0.5],
    );
    // Its next shot far off, so that only the strafe draws.
    let goal = bow_goal(1000);
    let mut state = crate::mobs::ai::ranged_weapons::BowAttacking {
        attack_time: 1000,
        see_time: 25,
        strafing_time: 0,
        ..Default::default()
    };
    let mut drawn = Vec::new();
    for _ in 0..45 {
        let replay = skeleton.stream.clone();
        let before = (state.strafing_clockwise, state.strafing_backwards);
        let (_, draws) = skeleton.goal_tick(&world, &goal, &mut state);
        drawn.push(draws);
        if draws == 2 {
            // Each turns its way below 0.3; between a quarter and three quarters of the
            // radius nothing else sets its backing.
            let mut replay = replay;
            let turn = f64::from(replay.next_float()) < 0.3;
            let back = f64::from(replay.next_float()) < 0.3;
            assert_eq!(state.strafing_clockwise, before.0 ^ turn);
            assert_eq!(state.strafing_backwards, before.1 ^ back);
        }
    }
    let strafing: Vec<usize> = (0..45).filter(|&tick| drawn[tick] == 2).collect();
    assert_eq!(strafing, [19, 39]);
    assert!(drawn.iter().all(|&draws| draws == 0 || draws == 2));
    // It strafes, its move control told to.
    assert_eq!(
        skeleton.mob.body.controls.movement.operation,
        crate::mobs::ai::Operation::Strafe
    );
}

fn crossbow_goal() -> RangedCrossbowAttackGoal {
    RangedCrossbowAttackGoal {
        speed: 1.0,
        radius: 8.0,
        perform: pillager::perform_ranged_attack,
        set_charging: pillager::set_charging,
    }
}

fn charging(mob: &Mob) -> bool {
    illagers::own_of::<PillagerState>(mob).charging
}

#[test]
fn a_pillager_charges_its_crossbow_over_25_ticks_loads_it_on_release_waits_then_shoots() {
    let world = Field::zombie_at([5.5, 0.0, 0.5]);
    let mut pillager = Shooter::new(
        entity_types::PILLAGER,
        armed(Some(ItemStack::new(items::CROSSBOW, 1))),
        [0.5, 0.0, 0.5],
    );
    let goal = crossbow_goal();
    let mut state = CrossbowAttacking::default();
    assert!(pillager.ai(&world, |ai| goal.can_use(&mut state, ai)).0);
    // Ticks 0 to 3: seen for less than five ticks, it walks, a new path's delay drawn at
    // the first (`nextInt(21)`).
    let replay = pillager.stream.clone();
    let (_, drawn) = pillager.goal_tick(&world, &goal, &mut state);
    assert_eq!(drawn, 1);
    let mut expected = replay;
    assert_eq!(state.update_path_delay, 20 + expected.next_int(21) as i32);
    for _ in 1..4 {
        assert_eq!(pillager.goal_tick(&world, &goal, &mut state).1, 0);
        assert_eq!(state.state, CrossbowState::Uncharged);
    }
    // Tick 4: seen for five, it stands and begins to charge.
    let (out, drawn) = pillager.goal_tick(&world, &goal, &mut state);
    assert_eq!(drawn, 0);
    assert_eq!(state.state, CrossbowState::Charging);
    assert!(charging(&pillager.mob));
    assert!(out.local.iter().any(|local| matches!(
        local,
        Local::GameEventAt {
            name: "minecraft:item_interact_start",
            ..
        }
    )));
    assert!(
        of(kind(entity_types::PILLAGER))
            .shown(&pillager.mob)
            .contains(&(17, DataShown::Boolean(true), false))
    );
    // Its charge's sounds at a fifth and at a half of it, of the players' source.
    let mut heard = Vec::new();
    for tick in 5..29 {
        let (out, drawn) = pillager.goal_tick(&world, &goal, &mut state);
        assert_eq!(drawn, 0, "tick {tick}");
        for local in out.local {
            if let Local::Sound {
                sound,
                source,
                volume,
                ..
            } = local
            {
                heard.push((tick, sound, source, volume));
            }
        }
        assert_eq!(state.state, CrossbowState::Charging, "tick {tick}");
    }
    assert_eq!(
        heard,
        [
            (10, sounds::ITEM_CROSSBOW_LOADING_START, 7, 0.5),
            (18, sounds::ITEM_CROSSBOW_LOADING_MIDDLE, 7, 0.5),
        ]
    );
    assert!(!crate::items::crossbow::is_charged(
        pillager.mob.body.equipment.slots[0]
            .as_ref()
            .expect("its crossbow")
    ));
    // Tick 29: 25 ticks of use; released, the extra use tick loads it and draws the end
    // sound's pitch, then the wait's `nextInt(20)`.
    let mut replay = pillager.stream.clone();
    let (out, drawn) = pillager.goal_tick(&world, &goal, &mut state);
    assert_eq!(drawn, 2);
    let pitch = crate::items::crossbow::loaded_pitch(replay.next_float());
    let delay = 20 + replay.next_int(20) as i32;
    assert_eq!(state.state, CrossbowState::Charged);
    assert_eq!(state.attack_delay, delay);
    assert!(!charging(&pillager.mob));
    assert!(!using::is_using(&pillager.mob));
    assert_eq!(
        sounds_of(&out),
        [(sounds::ITEM_CROSSBOW_LOADING_END, 1.0, pitch)]
    );
    let crossbow = pillager.mob.body.equipment.slots[0]
        .clone()
        .expect("its crossbow");
    assert_eq!(
        crate::items::crossbow::charged(&crossbow),
        [ItemStack::new(items::ARROW, 1)],
        "a fresh arrow of its own, split off whole: no infinite materials"
    );
    // It waits `delay` ticks, then shoots at the next it sees the target.
    for _ in 0..delay - 1 {
        assert!(shots(&pillager.goal_tick(&world, &goal, &mut state).0).is_empty());
    }
    let (_, drawn) = pillager.goal_tick(&world, &goal, &mut state);
    assert_eq!(drawn, 0);
    assert_eq!(state.state, CrossbowState::ReadyToAttack);
    let (out, drawn) = pillager.goal_tick(&world, &goal, &mut state);
    assert_eq!(drawn, 0, "one projectile: no pitch drawn, no unbreaking");
    assert_eq!(state.state, CrossbowState::Uncharged);
    let shot = &shots(&out)[0];
    assert_eq!(shot.kind, kind(entity_types::ARROW));
    assert_eq!(shot.power, 1.6);
    assert_eq!(shot.uncertainty, 10.0);
    assert_eq!(shot.from_mob, None, "no damage from the mob: the arrow's 2");
    let weapon = shot.weapon.clone().expect("its crossbow");
    assert!(!crate::items::crossbow::is_charged(&weapon));
    assert_eq!(sounds_of(&out), [(sounds::ITEM_CROSSBOW_SHOOT, 1.0, 1.0)]);
    let held = pillager.mob.body.equipment.slots[0]
        .clone()
        .expect("its crossbow");
    assert_eq!(crate::items::enchantments::damage(&held), 1, "worn by one");
    // Its aim: a tenth below the eyes towards the target's third up, lifted by a fifth,
    // normalised as floats.
    let eyes = f64::from(EntityTypeInfo::of(entity_types::PILLAGER).eye_height);
    assert_eq!(shot.at, [0.5, eyes - BELOW_THE_EYES, 0.5]);
    let zombie = f64::from(EntityTypeInfo::of(entity_types::ZOMBIE).height);
    let dy = zombie * 0.333_333_333_333_333_3 - shot.at[1] + 5.0 * 0.200_000_002_980_232_24;
    let (x, y) = (5.0_f32, dy as f32);
    let scale = 1.0 / x.mul_add(x, y.mul_add(y, 0.0)).sqrt();
    assert_eq!(
        shot.direction,
        [f64::from(x * scale), f64::from(y * scale), 0.0]
    );
}

#[test]
fn a_pillager_s_goal_stopped_mid_charge_drops_its_target_and_its_charge_and_its_flag() {
    let world = Field::zombie_at([5.5, 0.0, 0.5]);
    let mut pillager = Shooter::new(
        entity_types::PILLAGER,
        armed(Some(ItemStack::new(items::CROSSBOW, 1))),
        [0.5, 0.0, 0.5],
    );
    let goal = crossbow_goal();
    let mut state = CrossbowAttacking {
        see_time: 10,
        ..CrossbowAttacking::default()
    };
    pillager.goal_tick(&world, &goal, &mut state);
    assert!(charging(&pillager.mob) && using::is_using(&pillager.mob));
    pillager.ai(&world, |ai| goal.stop(&mut state, ai));
    assert!(!charging(&pillager.mob));
    assert!(!using::is_using(&pillager.mob));
    assert_eq!(pillager.mob.body.combat.target, None);
    assert_eq!(
        state.state,
        CrossbowState::Charging,
        "kept, as the game's field"
    );
}

#[test]
fn a_pillager_holds_its_ground_at_2_and_charges_at_3() {
    let mut setup = Setup::default();
    of(kind(entity_types::PILLAGER)).register_goals(&mut setup);
    let priorities: Vec<i32> = setup.goals.iter().map(|goal| goal.priority).collect();
    assert_eq!(&priorities[priorities.len() - 5..], [2, 3, 8, 9, 10]);
}

/// The witch aiming at the field's target, its draws and what it threw.
fn witch_throws(world: &Field, stream: Random) -> (Option<Shot>, MobOutput, Mob, u64) {
    let mut witch = Shooter::new(entity_types::WITCH, armed(None), [0.5, 0.0, 0.5]);
    witch.stream = stream;
    let target = witch.target(world);
    let ((), out, drawn) = witch.ai(world, |ai| witch::perform_ranged_attack(ai, &target, 1.0));
    (shots(&out).first().cloned(), out, witch.mob, drawn)
}

#[test]
fn a_witch_slows_a_far_target_poisons_a_healthy_one_and_weakens_or_harms_a_near_weak_one() {
    let stream = Random::of_entity(3, KEY, 1);
    // Ten blocks off: slowness, and only the throw's pitch drawn.
    let (shot, out, _, drawn) = witch_throws(&Field::zombie_at([10.5, 0.0, 0.5]), stream.clone());
    let shot = shot.expect("a throw");
    assert_eq!(
        potion_of(&shot.stack).as_deref(),
        Some("minecraft:slowness")
    );
    assert_eq!(shot.stack.item, items::SPLASH_POTION);
    assert_eq!(shot.kind, kind(entity_types::SPLASH_POTION));
    assert_eq!((shot.power, shot.uncertainty), (0.75, 8.0));
    assert_eq!(drawn, 1);
    let pitch = 0.8 + stream.clone().next_float() * 0.4;
    assert_eq!(sounds_of(&out), [(sounds::ENTITY_WITCH_THROW, 1.0, pitch)]);

    // Five blocks off, healthy: poison.
    let (shot, _, _, drawn) = witch_throws(&Field::zombie_at([5.5, 0.0, 0.5]), stream.clone());
    assert_eq!(
        potion_of(&shot.expect("a throw").stack).as_deref(),
        Some("minecraft:poison")
    );
    assert_eq!(drawn, 1);

    // Two and a half off, weak: weakness one time in four, drawn before the pitch.
    let weak = |stream: Random| {
        witch_throws(
            &Field::with_target(entity_types::ZOMBIE, [3.0, 0.0, 0.5], 5.0, [0.0; 3], None),
            stream,
        )
    };
    let lucky = (0..)
        .map(|seed| Random::of_entity(seed, KEY, 1))
        .find(|stream| stream.clone().next_float() < 0.25)
        .expect("some stream");
    let (shot, _, _, drawn) = weak(lucky);
    assert_eq!(
        potion_of(&shot.expect("a throw").stack).as_deref(),
        Some("minecraft:weakness")
    );
    assert_eq!(drawn, 2);
    let unlucky = (0..)
        .map(|seed| Random::of_entity(seed, KEY, 1))
        .find(|stream| stream.clone().next_float() >= 0.25)
        .expect("some stream");
    let (shot, _, _, drawn) = weak(unlucky);
    let shot = shot.expect("a throw");
    assert_eq!(potion_of(&shot.stack).as_deref(), Some("minecraft:harming"));
    assert_eq!(drawn, 2);
    assert_eq!(shot.power, 0.75, "beyond 2 blocks");

    // Four blocks off, weak: no weakness's draw beyond 3.
    let (shot, _, _, drawn) = witch_throws(
        &Field::with_target(entity_types::ZOMBIE, [4.5, 0.0, 0.5], 5.0, [0.0; 3], None),
        stream,
    );
    assert_eq!(
        potion_of(&shot.expect("a throw").stack).as_deref(),
        Some("minecraft:harming")
    );
    assert_eq!(drawn, 1);
}

#[test]
fn a_witch_throws_at_where_its_target_will_be_softly_within_two_blocks() {
    let stream = Random::of_entity(3, KEY, 1);
    let world = Field::with_target(
        entity_types::ZOMBIE,
        [2.0, 0.0, 0.5],
        20.0,
        [0.25, 0.5, -0.5],
        None,
    );
    let (shot, _, _, _) = witch_throws(&world, stream);
    let shot = shot.expect("a throw");
    let witch_eyes = f64::from(EntityTypeInfo::of(entity_types::WITCH).eye_height);
    let zombie_eyes = f64::from(EntityTypeInfo::of(entity_types::ZOMBIE).eye_height);
    let (dx, dz) = (2.0 + 0.25 - 0.5, 0.5 - 0.5 - 0.5);
    let dy = zombie_eyes - 1.100_000_023_841_858;
    let across = f64::sqrt(dx * dx + dz * dz);
    assert_eq!(shot.at, [0.5, witch_eyes - BELOW_THE_EYES, 0.5]);
    assert_eq!(shot.direction, [dx, dy + across * 0.2, dz]);
    assert_eq!(shot.power, 0.45);
}

#[test]
fn a_witch_heals_a_raider_and_drops_it_and_throws_nothing_while_it_drinks() {
    let stream = Random::of_entity(3, KEY, 1);
    let hurt = Field::with_target(entity_types::PILLAGER, [5.5, 0.0, 0.5], 4.0, [0.0; 3], None);
    let (shot, _, mob, _) = witch_throws(&hurt, stream.clone());
    assert_eq!(
        potion_of(&shot.expect("a throw").stack).as_deref(),
        Some("minecraft:healing")
    );
    assert_eq!(mob.body.combat.target, None);
    let well = Field::with_target(
        entity_types::VINDICATOR,
        [5.5, 0.0, 0.5],
        20.0,
        [0.0; 3],
        None,
    );
    let (shot, _, _, _) = witch_throws(&well, stream.clone());
    assert_eq!(
        potion_of(&shot.expect("a throw").stack).as_deref(),
        Some("minecraft:regeneration")
    );

    let world = Field::zombie_at([5.5, 0.0, 0.5]);
    let mut drinking = Shooter::new(entity_types::WITCH, armed(None), [0.5, 0.0, 0.5]);
    illagers::update_own(&mut drinking.mob, |own: &mut WitchState| {
        own.drinking = Some(Potion::Healing);
        own.using_time = 10;
    });
    let target = drinking.target(&world);
    let ((), out, drawn) = drinking.ai(&world, |ai| witch::perform_ranged_attack(ai, &target, 1.0));
    assert!(out.local.is_empty() && out.events.is_empty());
    assert_eq!(drawn, 0);
}

#[test]
fn a_witch_whose_target_is_beyond_eleven_blocks_takes_swiftness_on_its_fourth_draw() {
    let max = crate::mobs::body::attributes::supplied(
        kind(entity_types::WITCH),
        attribute(ids::MAX_HEALTH),
    )
    .expect("a witch's health") as f32;
    let stream = (0..)
        .map(|seed| Random::of_entity(seed, KEY, 1))
        .find(|stream| {
            let mut r = stream.clone();
            r.next_float();
            r.next_float();
            r.next_float();
            r.next_float() < 0.5
        })
        .expect("some stream");
    let drink = |at: f64| {
        let world = Field::zombie_at([at, 0.0, 0.5]);
        let mut mob = armed(None);
        mob.body.vitals.health = max;
        let mut witch = Shooter::new(entity_types::WITCH, mob, [0.5, 0.0, 0.5]);
        witch.stream = stream.clone();
        witch.with(&world, |tick| {
            of(kind(entity_types::WITCH)).before_ai_step(tick)
        });
        illagers::own_of::<WitchState>(&witch.mob).drinking
    };
    assert_eq!(drink(12.0), Some(Potion::Swiftness), "132.25 beyond 121");
    assert_eq!(drink(11.5), None, "121 is not beyond 121");
}

fn guardian(elder: bool) -> Shooter {
    let id = if elder {
        entity_types::ELDER_GUARDIAN
    } else {
        entity_types::GUARDIAN
    };
    Shooter::new(id, armed(None), [0.5, 0.0, 0.5])
}

fn strikes(out: &MobOutput) -> Vec<(EntityId, String, f32)> {
    out.local
        .iter()
        .filter_map(|local| match local {
            Local::Strike { key, hurt } => Some((*key, hurt.damage_type.clone(), hurt.amount.0)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_guardian_s_beam_shows_ten_ticks_on_and_strikes_with_magic_then_its_bite_at_80() {
    for (elder, duration, magic, bite) in [(false, 80, 1.0, 6.0), (true, 60, 3.0, 8.0)] {
        let world = Field::zombie_at([5.5, 0.0, 0.5]);
        let mut guardian = guardian(elder);
        let goal = GuardianAttackGoal { elder };
        let mut state = Default::default();
        assert!(guardian.ai(&world, |ai| goal.can_use(&mut state, ai)).0);
        guardian.ai(&world, |ai| goal.start(&mut state, ai));
        assert_eq!(state.attack_time, -10);
        for tick in 0..10 {
            let (out, drawn) = guardian.goal_tick(&world, &goal, &mut state);
            assert_eq!(drawn, 0);
            let beam = guardians::state_of(&guardian.mob).beam;
            if tick < 9 {
                assert_eq!(beam, None);
                assert!(out.events.is_empty());
            } else {
                assert_eq!(beam, Some(TARGET));
                assert_eq!(out.events, [Event::Entity(BEAM_EVENT)]);
            }
        }
        let kind = guardian.kind;
        assert!(of(kind).shown(&guardian.mob).contains(&(
            17,
            DataShown::Entity(Some(TARGET)),
            false
        )));
        for _ in 1..duration {
            let (out, _) = guardian.goal_tick(&world, &goal, &mut state);
            assert!(strikes(&out).is_empty());
        }
        let (out, drawn) = guardian.goal_tick(&world, &goal, &mut state);
        assert_eq!(drawn, 0);
        assert_eq!(state.attack_time, duration);
        assert_eq!(
            strikes(&out),
            [
                (TARGET, "minecraft:indirect_magic".to_owned(), magic),
                (TARGET, "minecraft:mob_attack".to_owned(), bite),
            ],
            "elder {elder}"
        );
        assert_eq!(guardian.mob.body.combat.target, None, "the target dropped");
        // The goal ends: its beam gone and its stroll triggered.
        assert!(
            !guardian
                .ai(&world, |ai| goal.can_continue(&mut state, ai))
                .0
        );
        guardian.ai(&world, |ai| goal.stop(&mut state, ai));
        let own = guardians::state_of(&guardian.mob);
        assert_eq!(own.beam, None);
        assert!(own.stroll_triggered);
    }
}

#[test]
fn a_guardian_lets_a_target_within_three_blocks_go_and_the_elder_keeps_it() {
    let near = Field::zombie_at([2.0, 0.0, 0.5]);
    let goal = GuardianAttackGoal { elder: false };
    let mut state = Default::default();
    let mut young = guardian(false);
    assert!(!young.ai(&near, |ai| goal.can_continue(&mut state, ai)).0);
    let elder_goal = GuardianAttackGoal { elder: true };
    let mut elder = guardian(true);
    assert!(
        elder
            .ai(&near, |ai| elder_goal.can_continue(&mut state, ai))
            .0
    );
}

#[test]
fn a_guardian_with_a_beam_turns_its_body_to_its_head_and_sinks_not_while_it_has_a_target() {
    let world = Field::default();
    let mut guardian = guardian(false);
    guardian.mob.body.head.head_yaw = 123.0;
    guardians::update(&mut guardian.mob, |own| own.beam = Some(TARGET));
    guardian.living.body.in_water = true;
    guardian.with(&world, |tick| of(tick.kind).before_ai_step(tick));
    assert_eq!(guardian.yaw, 123.0);
    let (travel, _, _) = guardian.with(&world, |tick| guardians::travel(tick));
    assert_eq!(
        travel,
        crate::living::Travel::Swims {
            speed: 0.1,
            sinks: crate::living::Sinks::Never
        }
    );
    guardian.mob.body.combat.target = None;
    let (travel, _, _) = guardian.with(&world, |tick| guardians::travel(tick));
    assert_eq!(
        travel,
        crate::living::Travel::Swims {
            speed: 0.1,
            sinks: crate::living::Sinks::Always
        }
    );
}

#[test]
fn the_stray_s_bogged_s_and_parched_s_tipped_arrows_carry_their_potions() {
    for (id, potion) in [
        (entity_types::STRAY, "minecraft:slowness"),
        (entity_types::BOGGED, "minecraft:poison"),
        (entity_types::PARCHED, "minecraft:weakness"),
    ] {
        let subject = Subject {
            kind: kind(id),
            on_fire: false,
            baby: false,
            vehicle: None,
            facts: Facts::default(),
        };
        let source = DamageSource::of(DamageType::MAGIC);
        let tipped = (0..64)
            .flat_map(|seed| {
                let mut draws = Random::of_entity(seed, KEY, 1);
                loot::entity_drops(&subject, &source, true, &mut draws)
            })
            .find(|stack| stack.item == items::TIPPED_ARROW && stack.count > 0)
            .expect("a tipped arrow within 64 deaths");
        assert_eq!(tipped.count, 1);
        assert_eq!(potion_of(&tipped).as_deref(), Some(potion), "{id}");
        // Not killed by a player: none.
        assert!((0..64).all(|seed| {
            let mut draws = Random::of_entity(seed, KEY, 1);
            loot::entity_drops(&subject, &source, false, &mut draws)
                .iter()
                .all(|stack| stack.item != items::TIPPED_ARROW)
        }));
    }
}
