//! Tests of a survival player's damage, hunger and death (step V4), written from
//! `docs/groundwork/v-plan.md`, sections 2.2 to 2.5, the build design's sections 7 and 8,
//! and the game's classes (`javap -c -p` of the 26.3 jar), by someone who did not write
//! the code, driving regions by their public interface alone: players who join as
//! survival players with what their place says, who load, move, sneak, sprint, use and
//! attack, and what the regions report and keep.
//!
//! What they cover: the hurt (difficulty, armour and toughness, the protections' cap, the
//! cooldown's two halves, absorption, the shield, the totem), the knock told to the
//! client, the sounds of a hurt and a death, falls onto each kind of block, the hurts of a
//! player's own tick (fire, lava, drowning, a wall, cactus, magma, powder snow, starving,
//! Poison and Wither), hunger (exhaustion, regeneration, eating, the rule of `SetHealth`),
//! death (its message, its drops and orbs, the respawn), players fighting, a skeleton's
//! arrow, TNT, and a zombie's hit across a pinned line.
//!
//! The scene is `n6_scene`'s: a stone floor at y = -61, player 1 standing on it at
//! (0.5, -60, 0.5). It is night (the clock at 18000), so that no zombie burns. The world's
//! difficulty is easy (`Difficulty::WORLD`) and its game rules the game's defaults.

mod n6_entities;
mod n6_scene;

use clustine_data::{DAMAGE_TYPES, entity_types, items, sounds};
use clustine_sim::api::{
    Face, Inventory, Keys, OwnData, Place, PlayerEvent, PlayerInput, RegionEvent, StayNote,
};
use clustine_sim::{Durable, FaceChange, FaceSeq, TickInputs, TickOutput, WorldClock};
use clustine_world::components::{Component, EffectInstance};
use clustine_world::{
    ARMOUR_START, Arrival, BlockPos, ChunkPos, Due, EffectId, Entity, EntityChange, EntityData,
    EntityId, FiledEffect, GameMode, ItemStack, OFF_HAND, PlayerId, PlayerMode, Vec3,
};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// The scene.

/// Where player 1 stands unless a test says otherwise.
const FEET: Vec3 = Vec3::new(0.5, -60.0, 0.5);
/// The block of player 1's feet, of their eyes (1.62 above the feet), and under them.
const FEET_BLOCK: BlockPos = BlockPos::new(0, -60, 0);
const EYES_BLOCK: BlockPos = BlockPos::new(0, -59, 0);
const FLOOR: BlockPos = BlockPos::new(0, -61, 0);

/// The night: no undead burns, and daylight plays no part.
const NIGHT: WorldClock = WorldClock {
    game_time: 18_000,
    day: 18_000,
};

/// Makes a place a survival player's, holding nothing, at food 17: below 18 food heals
/// nothing (`FoodData.tick @181-235`), so that a test of a hurt sees the hurt alone.
fn survival(place: &mut Place) {
    place.mode = PlayerMode::new(GameMode::Survival);
    place.inventory = Inventory::default();
    place.vitals.food.food = 17;
}

/// A region alone at night with `world`, and player 1 at `feet`, a survival player whose
/// place `change` changes after [`survival`], whose client has loaded
/// (`ServerPlayer.isInvulnerableTo`: nothing hurts a player before).
fn scene_at(world: &World, feet: Vec3, change: impl FnOnce(&mut Place)) -> Scene {
    loaded_scene(world, feet, change, Vec::new()).0
}

/// As [`scene_at`], with `first` inputs of player 1 given in the tick their client loads
/// in, before the load: the scene and that tick's output, in which a block they stand in
/// first hurts them.
fn loaded_scene(
    world: &World,
    feet: Vec3,
    change: impl FnOnce(&mut Place),
    first: Vec<PlayerInput>,
) -> (Scene, TickOutput) {
    let layout = Layout::alone();
    let region = region_at(&layout, world, Inventory::default(), Some(NIGHT));
    let mut scene = Scene {
        region,
        layout,
        inventory: Inventory::default(),
        actors: Vec::new(),
    };
    scene.join_with(1, feet, |place| {
        survival(place);
        change(place);
    });
    let mut inputs = first;
    inputs.push(PlayerInput::Loaded);
    let loaded = scene.inputs_of(0, inputs);
    (scene, loaded)
}

fn survival_scene(world: &World, change: impl FnOnce(&mut Place)) -> Scene {
    scene_at(world, FEET, change)
}

/// Player `n` joins `scene` at `feet` as a survival player changed by `change`, and loads.
fn join_survival(scene: &mut Scene, n: u128, feet: Vec3, change: impl FnOnce(&mut Place)) -> usize {
    let who = scene.join_with(n, feet, |place| {
        survival(place);
        change(place);
    });
    scene.input_of(who, PlayerInput::Loaded);
    who
}

fn creative(place: &mut Place) {
    place.mode = PlayerMode::new(GameMode::Creative);
}

fn health_of(scene: &Scene, n: u128) -> f32 {
    scene
        .region
        .player_state(player(n))
        .expect("the player is there")
        .health
}

fn health(scene: &Scene) -> f32 {
    health_of(scene, 1)
}

fn vitals_of(scene: &Scene, n: u128) -> clustine_sim::Vitals {
    scene
        .region
        .player_state(player(n))
        .expect("the player is there")
        .vitals
}

fn vitals(scene: &Scene) -> clustine_sim::Vitals {
    vitals_of(scene, 1)
}

fn effect(name: &str, amplifier: i32, duration: i32) -> EffectInstance {
    EffectInstance {
        effect: name.to_owned(),
        amplifier,
        duration,
        ambient: false,
        visible: true,
        show_icon: true,
    }
}

fn enchanted(item: i32, enchantments: &[(&str, i32)]) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack.components.set(Component::Enchantments(
        enchantments
            .iter()
            .map(|(name, level)| ((*name).to_owned(), *level))
            .collect(),
    ));
    stack
}

/// The four pieces of armour `[feet, legs, chest, head]` worn.
fn wearing(place: &mut Place, pieces: [i32; 4]) {
    for (index, item) in pieces.into_iter().enumerate() {
        place
            .inventory
            .set(ARMOUR_START + index, Some(ItemStack::new(item, 1)));
    }
}

// ------------------------------------------------------------------------------------
// What the regions report.

/// The name of a damage type by the id the region sends: the index in the game's
/// registry, sorted by name.
fn type_name(id: i32) -> &'static str {
    DAMAGE_TYPES[usize::try_from(id).expect("an index")].name
}

/// The names of the damage types of the tick's damage events on `entity`, in order.
fn hurts_on(output: &TickOutput, entity: EntityId) -> Vec<&'static str> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent {
                entity: hurt,
                damage_type,
                ..
            } if *hurt == entity => Some(type_name(*damage_type)),
            _ => None,
        })
        .collect()
}

/// The damage events on `entity`, with their causer and direct entity.
fn damage_events_on(
    output: &TickOutput,
    entity: EntityId,
) -> Vec<(&'static str, Option<EntityId>, Option<EntityId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent {
                entity: hurt,
                damage_type,
                cause,
                direct,
                ..
            } if *hurt == entity => Some((type_name(*damage_type), *cause, *direct)),
            _ => None,
        })
        .collect()
}

/// The [`PlayerEvent::Health`] of `n` in the tick, as (health, food, saturation).
fn told_health(output: &TickOutput, n: u128) -> Vec<(f32, i32, f32)> {
    events_of(output, player(n))
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::Health {
                health,
                food,
                saturation,
            } => Some((health, food, saturation)),
            _ => None,
        })
        .collect()
}

fn knocked(output: &TickOutput, n: u128) -> Vec<[f64; 3]> {
    events_of(output, player(n))
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::Knocked { motion } => Some(motion),
            _ => None,
        })
        .collect()
}

fn own_data(output: &TickOutput, n: u128) -> Vec<OwnData> {
    events_of(output, player(n))
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::OwnData(data) => Some(data),
            _ => None,
        })
        .collect()
}

/// The entity events of the tick on `entity`.
fn entity_events_on(output: &TickOutput, entity: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent {
                entity: on, event, ..
            } if *on == entity => Some(*event),
            _ => None,
        })
        .collect()
}

/// The sounds of id `sound` in the tick, with whom each is not played for.
fn sounds_heard(output: &TickOutput, sound: i32) -> Vec<Option<PlayerId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: played,
                except,
                ..
            } if *played == sound => Some(*except),
            _ => None,
        })
        .collect()
}

/// The deaths of the tick: who died and their message's key.
fn deaths(output: &TickOutput) -> Vec<(PlayerId, clustine_sim::api::DeathMessage)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::Died {
                player, message, ..
            } => Some((*player, message.clone())),
            _ => None,
        })
        .collect()
}

fn cooldowns(output: &TickOutput, n: u128) -> Vec<(String, i32)> {
    events_of(output, player(n))
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::Cooldown { group, ticks } => Some((group, ticks)),
            _ => None,
        })
        .collect()
}

/// Idles until `found` finds something in a tick's output, for at most `ticks` ticks;
/// the outputs idled through go to `seen`, the one found last. Panics if nothing came.
fn idle_until_found<T>(
    scene: &mut Scene,
    ticks: u32,
    seen: &mut Vec<TickOutput>,
    found: impl Fn(&TickOutput) -> Option<T>,
) -> T {
    for _ in 0..ticks {
        let output = scene.idle();
        let result = found(&output);
        seen.push(output);
        if let Some(result) = result {
            return result;
        }
    }
    panic!("nothing came in {ticks} ticks");
}

/// The ticks with a damage event of `kind` on player 1: the load's tick `loaded` is
/// tick 0, and `ticks` more are idled.
fn hurt_ticks(scene: &mut Scene, loaded: &TickOutput, kind: &str, ticks: u32) -> Vec<u32> {
    let me = scene.me();
    let mut at = Vec::new();
    if hurts_on(loaded, me).contains(&kind) {
        at.push(0);
    }
    for tick in 1..=ticks {
        let output = scene.idle();
        if hurts_on(&output, me).contains(&kind) {
            at.push(tick);
        }
    }
    at
}

// ------------------------------------------------------------------------------------
// Moves.

fn move_to(position: Vec3, on_ground: bool) -> PlayerInput {
    PlayerInput::Move {
        position: Some(position),
        rotation: None,
        on_ground,
    }
}

fn turn(yaw: f32) -> PlayerInput {
    PlayerInput::Move {
        position: None,
        rotation: Some((yaw, 0.0)),
        on_ground: true,
    }
}

/// Player `who` rises `height` above `feet` in one move, falls half of it in the next and
/// lands on `feet` in the third, as a client tells a fall: the outputs of the three.
fn fall_by(scene: &mut Scene, who: usize, feet: Vec3, height: f64) -> Vec<TickOutput> {
    vec![
        scene.input_of(
            who,
            move_to(Vec3::new(feet.x, feet.y + height, feet.z), false),
        ),
        scene.input_of(
            who,
            move_to(Vec3::new(feet.x, feet.y + height / 2.0, feet.z), false),
        ),
        scene.input_of(who, move_to(feet, true)),
    ]
}

fn fall(scene: &mut Scene, height: f64) -> Vec<TickOutput> {
    fall_by(scene, 0, FEET, height)
}

fn keys(sneak: bool, sprint: bool) -> PlayerInput {
    PlayerInput::Keys(Keys {
        forward: sprint,
        backward: false,
        left: false,
        right: false,
        jump: false,
        sneak,
        sprint,
    })
}

// ------------------------------------------------------------------------------------
// Mobs, made by eggs in a creative player's scratch scene and laid down where a test
// wants them.

/// The record of a mob of `kind` from its `egg`, made at night in a scratch scene.
fn made(egg: i32, kind: i32) -> Entity {
    let layout = Layout::alone();
    let held = inventory(&[(0, stack(egg, 1))]);
    let region = region_at(&layout, &World::default(), held.clone(), Some(NIGHT));
    let mut scratch = Scene {
        region,
        layout,
        inventory: held,
        actors: Vec::new(),
    };
    scratch.join_with(1, FEET, creative);
    scratch.click(bp(3, -61, 3), Face::Top);
    let found = scratch.of_kind(kind);
    assert_eq!(found.len(), 1, "the egg makes one mob of kind {kind}");
    found.into_iter().next().expect("one")
}

/// `record` as entity `n` standing at rest at `at`, facing `yaw`.
fn laid(record: &Entity, n: u32, at: [f64; 3], yaw: f32) -> Entity {
    let mut entity = record.clone();
    entity.key = key(n);
    entity.uuid = uuid::Uuid::from_u128(9_000 + u128::from(n));
    entity.position = at;
    entity.motion = [0.0; 3];
    entity.on_ground = true;
    entity.fall_distance = 0.0;
    entity.fire_ticks = 0;
    entity.yaw = yaw;
    entity
}

fn zombie_at(at: [f64; 3]) -> Entity {
    laid(
        &made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE),
        1,
        at,
        180.0,
    )
}

/// Idles `scene` until a damage event of `kind` on player 1, for at most `ticks` ticks,
/// unless the last output of `seen` has one already: the outputs idled through go to
/// `seen`, the hurt's last.
fn until_hurt(scene: &mut Scene, kind: &'static str, ticks: u32, seen: &mut Vec<TickOutput>) {
    let me = scene.me();
    if seen
        .last()
        .is_some_and(|output| hurts_on(output, me).contains(&kind))
    {
        return;
    }
    idle_until_found(scene, ticks, seen, |output| {
        hurts_on(output, me).contains(&kind).then_some(())
    });
}

/// The health a zombie's hit leaves of 20 on easy: its attack damage of 3
/// (`Zombie.createAttributes`) scaled to `min(3 / 2 + 1, 3)` (`Player.hurtServer
/// @34-83`), then taken through the armour's `armour` points and `toughness`
/// (`CombatRules.getDamageAfterAbsorb`).
fn after_a_zombie(armour: f32, toughness: f32) -> f32 {
    let damage = (3.0_f32 / 2.0 + 1.0).min(3.0);
    20.0 - after_armour(damage, armour, toughness)
}

/// `CombatRules.getDamageAfterAbsorb @0-60`: `t = 2 + toughness / 4`, `r = clamp(armour −
/// d / t, armour · 0.2, 20)`, `d · (1 − r / 25)`, in floats as the game's.
fn after_armour(damage: f32, armour: f32, toughness: f32) -> f32 {
    let t = 2.0 + toughness / 4.0;
    let r = (armour - damage / t).clamp(armour * 0.2, 20.0);
    damage * (1.0 - r / 25.0)
}

// ====================================================================================
// The hurt (plan 2.2).

/// `Mob.doHurtTarget @0-127`: a zombie hits for its attack damage, 3, from the
/// `mob_attack` type (`DamageSources.mobAttack`), which scales `when_caused_by_living_non_
/// player`: on easy `min(d / 2 + 1, d)`, 2.5 (`Player.hurtServer @34-83`). The hit is a
/// damage event naming the zombie as cause and direct entity, costs the type's exhaustion
/// of 0.1 (`Player.actuallyHurt @96-101`), and its knock (`LivingEntity.knockback` of 0.4
/// away from the zombie, from the motion the server keeps, nothing; on the ground its y
/// is `min(0.4, y / 2 + 0.4)`) is told to the client, as `markHurt` makes the tracker
/// send the player its own `SetEntityMotion` (`ServerEntity.sendChanges @960-993`).
#[test]
fn a_zombie_s_hit_on_easy_takes_two_and_a_half_and_the_knock_is_told() {
    let world = World::default().with_entities(&[zombie_at([0.5, -60.0, 3.5])]);
    let mut scene = survival_scene(&world, |_| {});
    let mut seen = Vec::new();
    until_hurt(&mut scene, "minecraft:mob_attack", 400, &mut seen);
    let hit = seen.last().expect("the hit's tick");
    let me = scene.me();
    assert_eq!(
        damage_events_on(hit, me),
        [("minecraft:mob_attack", Some(key(1)), Some(key(1)))]
    );
    assert_eq!(health(&scene), after_a_zombie(0.0, 0.0));
    assert_eq!(health(&scene), 17.5);
    assert!(
        (vitals(&scene).food.exhaustion - 0.1).abs() < 1e-6,
        "the hurt's exhaustion: {:?}",
        vitals(&scene).food
    );
    // The client is told its health, and its knock, in the hit's tick.
    assert_eq!(told_health(hit, 1).last().map(|told| told.0), Some(17.5));
    let knocks = knocked(hit, 1);
    assert_eq!(knocks.len(), 1, "{knocks:?}");
    let [x, y, z] = knocks[0];
    let zombie = scene
        .all()
        .into_iter()
        .find(|entity| entity.key == key(1))
        .expect("the zombie");
    let (away_x, away_z) = (0.5 - zombie.position[0], 0.5 - zombie.position[2]);
    assert!(
        x * away_x + z * away_z > 0.0,
        "knocked away from the zombie: {:?} from {:?}",
        knocks[0],
        zombie.position
    );
    assert!(((x * x + z * z).sqrt() - 0.4).abs() < 1e-6, "{knocks:?}");
    assert!((y - 0.4).abs() < 1e-6, "{knocks:?}");
}

/// `CombatRules.getDamageAfterAbsorb`: a full set of iron (15 points, no toughness)
/// takes a zombie's 2.5 to `2.5 · (1 − 13.75 / 25)`; diamond (20 points, 8 toughness)
/// to `2.5 · (1 − 19.375 / 25)` (`Player.actuallyHurt @0-146`,
/// `LivingEntity.getDamageAfterArmorAbsorb @0-36`).
#[test]
fn armour_and_its_toughness_take_their_part_of_a_zombie_s_hit() {
    for (pieces, armour, toughness) in [
        (
            [
                items::IRON_BOOTS,
                items::IRON_LEGGINGS,
                items::IRON_CHESTPLATE,
                items::IRON_HELMET,
            ],
            15.0,
            0.0,
        ),
        (
            [
                items::DIAMOND_BOOTS,
                items::DIAMOND_LEGGINGS,
                items::DIAMOND_CHESTPLATE,
                items::DIAMOND_HELMET,
            ],
            20.0,
            8.0,
        ),
    ] {
        let world = World::default().with_entities(&[zombie_at([0.5, -60.0, 3.5])]);
        let mut scene = survival_scene(&world, |place| wearing(place, pieces));
        let mut seen = Vec::new();
        until_hurt(&mut scene, "minecraft:mob_attack", 400, &mut seen);
        let expected = after_a_zombie(armour, toughness);
        assert!(
            (health(&scene) - expected).abs() < 1e-5,
            "{armour} points, {toughness} toughness: {} where {expected}",
            health(&scene)
        );
    }
}

/// `LivingEntity.getDamageAfterMagicAbsorb @0-233`: the protections of what is worn add
/// up (Protection 1 a level against everything, Feather Falling 3 a level against falls:
/// the enchantments' `damage_protection`), capped at 20, four fifths
/// (`EnchantmentHelper.getDamageProtection`, `CombatRules.getDamageAfterMagicAbsorb`).
/// Protection IV on all four pieces and Feather Falling IV on the boots make 28, so a fall
/// of 10 blocks (7, `LivingEntity.calculateFallDamage`; falls go through armour,
/// `#bypasses_armor`) takes 1.4, not nothing.
#[test]
fn the_protections_are_capped_at_four_fifths() {
    let mut scene = survival_scene(&World::default(), |place| {
        let protection = ("minecraft:protection", 4);
        let pieces = [
            enchanted(
                items::IRON_BOOTS,
                &[protection, ("minecraft:feather_falling", 4)],
            ),
            enchanted(items::IRON_LEGGINGS, &[protection]),
            enchanted(items::IRON_CHESTPLATE, &[protection]),
            enchanted(items::IRON_HELMET, &[protection]),
        ];
        for (index, piece) in pieces.into_iter().enumerate() {
            place.inventory.set(ARMOUR_START + index, Some(piece));
        }
    });
    fall(&mut scene, 10.0);
    let expected = 20.0 - 7.0 * (1.0 - 20.0_f32 / 25.0);
    assert!(
        (health(&scene) - expected).abs() < 1e-5,
        "{} where {expected}",
        health(&scene)
    );
}

/// `LivingEntity.hurtServer @184-269`: a full hit sets the cooldown of 20 ticks; in its
/// second half (above 10) a hurt is taken only by what it has above the last, with no
/// damage event, no knock and no sound, and the last hurt becomes it; one no greater is
/// refused. A fall of 5 (2), then one of 8 (5) two ticks on: 3 more; then one of 5
/// again: nothing.
#[test]
fn in_the_cooldown_s_second_half_only_the_excess_is_taken_with_no_event() {
    let mut scene = survival_scene(&World::default(), |_| {});
    let me = scene.me();
    let first = fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 18.0);
    assert_eq!(hurts_on(&first[2], me), ["minecraft:fall"]);
    let second = fall(&mut scene, 8.0);
    assert_eq!(health(&scene), 15.0, "5 less the 2 before");
    for output in &second {
        assert!(hurts_on(output, me).is_empty(), "{:?}", output.events);
        assert!(knocked(output, 1).is_empty());
        assert!(sounds_heard(output, sounds::ENTITY_PLAYER_HURT).is_empty());
    }
    assert_eq!(
        told_health(&second[2], 1).last().map(|told| told.0),
        Some(15.0),
        "the health is told"
    );
    let third = fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 15.0, "2 is not above the last hurt of 5");
    assert!(third.iter().all(|output| hurts_on(output, me).is_empty()));
}

/// `Player.actuallyHurt`: absorption takes a hurt before the health
/// (`setAbsorptionAmount(absorption − taken)`), and the client is told the absorption
/// left in its own entity data (`DATA_PLAYER_ABSORPTION_ID`). The amount is the place's:
/// the game saves it (`LivingEntity.addAdditionalSaveData`'s `AbsorptionAmount`) and
/// reads it back with the effects (`readAdditionalSaveData`, `internalSetAbsorptionAmount`
/// and the effects put back without `onEffectAdded`), and `AbsorptionMobEffect.
/// applyEffectTick` keeps the effect while the amount is above 0.
#[test]
fn absorption_kept_in_a_place_takes_a_hurt_before_the_health() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.effects = vec![effect("minecraft:absorption", 0, 1200)];
        place.vitals.absorption = 4.0;
    });
    let outputs = fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 20.0);
    assert_eq!(vitals(&scene).absorption, 2.0);
    let told: Vec<OwnData> = outputs
        .iter()
        .flat_map(|output| own_data(output, 1))
        .collect();
    assert_eq!(
        told.last().map(|data| data.absorption),
        Some(2.0),
        "{told:?}"
    );
}

/// The same with the absorption a totem's save gives (Absorption II: 8,
/// `AbsorptionMobEffect.onEffectAdded`): a fall of 5 twenty ticks on takes 2 of it and
/// none of the health.
#[test]
fn absorption_from_a_totem_takes_a_hurt_before_the_health() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 1.0;
        place
            .inventory
            .set(OFF_HAND, Some(ItemStack::new(items::TOTEM_OF_UNDYING, 1)));
    });
    fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 1.0, "saved by the totem");
    assert_eq!(vitals(&scene).absorption, 8.0);
    scene.idle_for(20);
    let before = health(&scene);
    let outputs = fall(&mut scene, 5.0);
    assert_eq!(vitals(&scene).absorption, 6.0);
    assert!(health(&scene) >= before, "{} from {before}", health(&scene));
    let told: Vec<OwnData> = outputs
        .iter()
        .flat_map(|output| own_data(output, 1))
        .collect();
    assert_eq!(
        told.last().map(|data| data.absorption),
        Some(6.0),
        "{told:?}"
    );
}

/// A shield in use (`blocks_attacks`, `BlocksAttacks.blockDelaySeconds` 0.25: 5 ticks,
/// `LivingEntity.getItemBlockingWith`) takes all of a hit from in front of the player
/// (`horizontalBlockingAngle` 90 of the look and the source's direction,
/// `BlocksAttacks.resolveBlockedDamage`), and nothing of one from behind.
#[test]
fn a_raised_shield_blocks_a_zombie_in_front_and_not_one_behind() {
    for (yaw, blocked) in [(0.0_f32, true), (180.0, false)] {
        // The zombie stands south of the player: a yaw of 0 looks south.
        let world = World::default().with_entities(&[zombie_at([0.5, -60.0, 3.5])]);
        let mut scene = survival_scene(&world, |place| {
            place
                .inventory
                .set(0, Some(ItemStack::new(items::SHIELD, 1)));
            place.pose.yaw = yaw;
        });
        scene.input(turn(yaw));
        scene.input(PlayerInput::UseItem {
            rotation: (yaw, 0.0),
            sneaking: false,
            sequence: 1,
        });
        let me = scene.me();
        let mut seen = Vec::new();
        let mut struck = false;
        for _ in 0..400 {
            let output = scene.idle();
            let attacked = output.events.iter().any(|event| {
                matches!(event, RegionEvent::Sound { sound, .. }
                    if *sound == sounds::ITEM_SHIELD_BLOCK)
            }) || !hurts_on(&output, me).is_empty();
            seen.push(output);
            if attacked {
                struck = true;
                break;
            }
        }
        assert!(struck, "yaw {yaw}: the zombie strikes");
        if blocked {
            assert_eq!(health(&scene), 20.0, "blocked from in front");
        } else {
            assert_eq!(health(&scene), 17.5, "taken from behind");
        }
    }
}

/// Player 2 hits player 1, who raised a shield the tick before: in its first 5 ticks a
/// shield blocks nothing (`blockDelaySeconds` 0.25); raised 10 ticks, it blocks a fist's
/// hit; an axe's hit (`weapon`'s `disable_blocking_for_seconds` 5) disables it for 100
/// ticks (`BlocksAttacks.disable`: the seconds · `disable_cooldown_scale` 1 · 20), a
/// cooldown of the shield told to player 1.
#[test]
fn a_shield_blocks_nothing_in_its_first_five_ticks_and_an_axe_disables_it() {
    let shield = |place: &mut Place| {
        place
            .inventory
            .set(0, Some(ItemStack::new(items::SHIELD, 1)));
        place.pose.yaw = 0.0;
    };
    let raise = |scene: &mut Scene| {
        scene.input(PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence: 1,
        })
    };
    let attacker_feet = Vec3::new(0.5, -60.0, 2.0);

    // Raised a tick before the hit.
    let mut scene = survival_scene(&World::default(), shield);
    let attacker = join_survival(&mut scene, 2, attacker_feet, |_| {});
    scene.input_of(attacker, turn(180.0));
    scene.idle_for(20);
    raise(&mut scene);
    let me = scene.me();
    let hit = scene.input_of(
        attacker,
        PlayerInput::Attack {
            entity: me,
            part: None,
        },
    );
    assert_eq!(health(&scene), 19.0, "{:?}", hit.events);

    // Raised ten ticks before.
    let mut scene = scene_of_two(shield, |_| {}, attacker_feet);
    raise(&mut scene);
    scene.idle_for(10);
    let me = scene.me();
    scene.input_of(
        1,
        PlayerInput::Attack {
            entity: me,
            part: None,
        },
    );
    assert_eq!(health(&scene), 20.0, "the fist's hit is blocked");

    // An axe.
    let mut scene = scene_of_two(
        shield,
        |place| {
            place
                .inventory
                .set(0, Some(ItemStack::new(items::IRON_AXE, 1)));
        },
        attacker_feet,
    );
    raise(&mut scene);
    scene.idle_for(10);
    let me = scene.me();
    let hit = scene.input_of(
        1,
        PlayerInput::Attack {
            entity: me,
            part: None,
        },
    );
    assert_eq!(health(&scene), 20.0, "the axe's hit is blocked");
    assert!(
        cooldowns(&hit, 1).contains(&("minecraft:shield".to_owned(), 100)),
        "{:?}",
        events_of(&hit, player(1))
    );
}

/// Player 1 changed by `first` and player 2 at `second_feet` changed by `second`, both
/// survival and loaded, player 2 facing player 1 and 20 ticks idle so that their attack
/// is charged (`Player.getAttackStrengthScale`).
fn scene_of_two(
    first: impl FnOnce(&mut Place),
    second: impl FnOnce(&mut Place),
    second_feet: Vec3,
) -> Scene {
    let mut scene = survival_scene(&World::default(), first);
    let attacker = join_survival(&mut scene, 2, second_feet, second);
    let yaw = if second_feet.z > FEET.z { 180.0 } else { 0.0 };
    scene.input_of(attacker, turn(yaw));
    scene.idle_for(20);
    scene
}

/// `LivingEntity.checkTotemDeathProtection`: a hurt that would kill a player holding a
/// totem of undying in a hand uses it up instead: health 1, every effect cleared, then
/// the totem's (`DeathProtection.TOTEM_OF_UNDYING`: Regeneration II for 900 ticks,
/// Absorption II for 100, Fire Resistance for 800), and the entity event 35.
#[test]
fn a_totem_in_the_off_hand_saves_a_player_with_one_health_and_its_effects() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 1.0;
        place
            .inventory
            .set(OFF_HAND, Some(ItemStack::new(items::TOTEM_OF_UNDYING, 1)));
        place.effects = vec![effect("minecraft:speed", 0, 1000)];
    });
    let me = scene.me();
    let outputs = fall(&mut scene, 5.0);
    let state = scene.region.player_state(player(1)).expect("there");
    assert!(!state.dead, "saved");
    assert_eq!(state.health, 1.0);
    assert_eq!(state.inventory.0[OFF_HAND], None, "the totem is used up");
    let mut effects: Vec<(String, u8)> = state
        .effects
        .iter()
        .map(|kept| (kept.effect.clone(), kept.amplifier))
        .collect();
    effects.sort();
    assert_eq!(
        effects,
        [
            ("minecraft:absorption".to_owned(), 1),
            ("minecraft:fire_resistance".to_owned(), 0),
            ("minecraft:regeneration".to_owned(), 1),
        ]
    );
    assert!(
        entity_events_on(&outputs[2], me).contains(&35),
        "{:?}",
        outputs[2].events
    );
    assert!(deaths(&outputs[2]).is_empty());
}

// ====================================================================================
// The knock and the sounds (plan 2.2; the design's section 7, M6 and M7).

/// `LivingEntity.hurtServer @348`: `markHurt` unless the type is `#no_impact`, of which
/// drowning is the one (`data/tags/damage_type/no_impact.json`): drowning tells the client
/// no motion. A fall, which has impact, does.
#[test]
fn a_drowning_hurt_tells_no_knock_and_a_fall_does() {
    let world = World::of(&[
        (FEET_BLOCK, state("minecraft:water[level=0]")),
        (EYES_BLOCK, state("minecraft:water[level=0]")),
    ]);
    let mut scene = survival_scene(&world, |place| place.vitals.air = 0);
    let me = scene.me();
    let mut seen = Vec::new();
    until_hurt(&mut scene, "minecraft:drown", 40, &mut seen);
    let drowned = seen.last().expect("the tick");
    assert!(
        knocked(drowned, 1).is_empty(),
        "{:?}",
        drowned.player_events
    );

    let mut scene = survival_scene(&World::default(), |_| {});
    let landed = fall(&mut scene, 5.0).pop().expect("the landing");
    assert_eq!(hurts_on(&landed, me), ["minecraft:fall"]);
    assert_eq!(knocked(&landed, 1).len(), 1, "{:?}", landed.player_events);
}

/// `Player.playSound` plays a player's sounds for everyone near but the player
/// (`level.playSound(this, …)`), whose client plays them itself: the hurt's
/// (`LivingEntity.playHurtSound`, `entity.player.hurt`) and the death's
/// (`entity.player.death`), and a fall's (`entity.player.small_fall` to 4, big beyond,
/// `LivingEntity.playBlockFallSound`'s `getFallDamageSound`).
#[test]
fn the_sounds_of_a_hurt_and_a_death_are_for_everyone_but_the_player() {
    let mut scene = survival_scene(&World::default(), |place| place.health = 6.0);
    let landed = fall(&mut scene, 5.0).pop().expect("the landing");
    assert_eq!(
        sounds_heard(&landed, sounds::ENTITY_PLAYER_HURT),
        [Some(player(1))]
    );
    assert_eq!(
        sounds_heard(&landed, sounds::ENTITY_PLAYER_SMALL_FALL),
        [Some(player(1))]
    );
    // 9 more blocks: 6, a big fall, and a death.
    let landed = {
        scene.idle_for(20);
        fall(&mut scene, 9.0).pop().expect("the landing")
    };
    assert_eq!(
        sounds_heard(&landed, sounds::ENTITY_PLAYER_DEATH),
        [Some(player(1))]
    );
    assert_eq!(
        sounds_heard(&landed, sounds::ENTITY_PLAYER_BIG_FALL),
        [Some(player(1))]
    );
    assert!(sounds_heard(&landed, sounds::ENTITY_PLAYER_HURT).is_empty());
}

// ====================================================================================
// Falls (plan 2.3; the design's section 7, "Falling").

/// `LivingEntity.calculateFallDamage @0-35`: `floor(distance + 1e-6 − 3)` from the safe
/// fall distance of 3: five blocks onto stone hurt 2.
#[test]
fn a_fall_of_five_blocks_onto_stone_hurts_two() {
    let mut scene = survival_scene(&World::default(), |_| {});
    let me = scene.me();
    let outputs = fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 18.0);
    assert_eq!(hurts_on(&outputs[2], me), ["minecraft:fall"]);
    assert!(hurts_on(&outputs[0], me).is_empty() && hurts_on(&outputs[1], me).is_empty());
    assert_eq!(vitals(&scene).fall_distance, 0.0, "a landing ends the fall");
}

/// Three blocks are safe: nothing.
#[test]
fn a_fall_of_three_blocks_hurts_nothing() {
    let mut scene = survival_scene(&World::default(), |_| {});
    fall(&mut scene, 3.0);
    assert_eq!(health(&scene), 20.0);
}

/// `HayBlock.fallOn`: `causeFallDamage(distance, 0.2)`: five blocks hurt `floor(2 · 0.2)`,
/// nothing; thirteen `floor(10 · 0.2)`, 2.
#[test]
fn a_fall_onto_hay_hurts_a_fifth() {
    let world = World::of(&[(FLOOR, state("minecraft:hay_block[axis=y]"))]);
    let mut scene = survival_scene(&world, |_| {});
    fall(&mut scene, 5.0);
    assert_eq!(health(&scene), 20.0);
    fall(&mut scene, 13.0);
    assert_eq!(health(&scene), 18.0);
}

/// `BedBlock.fallOn`: `super.fallOn(…, distance · 0.5)`: eleven blocks hurt as 5.5,
/// 2.
#[test]
fn a_fall_onto_a_bed_hurts_as_half_the_height() {
    let world = World::of(&[(
        FLOOR,
        state("minecraft:red_bed[facing=north,occupied=false,part=foot]"),
    )]);
    let mut scene = survival_scene(&world, |_| {});
    fall(&mut scene, 11.0);
    assert_eq!(health(&scene), 18.0);
}

/// `SlimeBlock.fallOn`: nothing unless the player is `isSuppressingBounce` (sneaking),
/// when it is a fall as onto any block.
#[test]
fn a_fall_onto_slime_hurts_only_a_sneaking_player() {
    let world = World::of(&[(FLOOR, state("minecraft:slime_block"))]);
    let mut scene = survival_scene(&world, |_| {});
    fall(&mut scene, 10.0);
    assert_eq!(health(&scene), 20.0);
    scene.input(keys(true, false));
    fall(&mut scene, 10.0);
    assert_eq!(health(&scene), 13.0);
}

/// `Entity.updateInWaterStateAndDoFluidPushing`: water resets the fall: a player landing
/// in water is hurt by nothing.
#[test]
fn a_fall_into_water_hurts_nothing() {
    let world = World::of(&[(FEET_BLOCK, state("minecraft:water[level=0]"))]);
    let mut scene = survival_scene(&world, |_| {});
    fall(&mut scene, 10.0);
    assert_eq!(health(&scene), 20.0);
}

/// `LivingEntity.aiStep @527-548`: Slow Falling resets the fall distance in each of the
/// player's ticks, so a fall told a block a tick hurts nothing, where without it ten
/// blocks so hurt 7.
#[test]
fn a_fall_with_slow_falling_hurts_nothing() {
    let by_blocks = |scene: &mut Scene| {
        scene.input(move_to(Vec3::new(0.5, -50.0, 0.5), false));
        for down in 1..=10 {
            let y = -50.0 - f64::from(down);
            scene.input(move_to(Vec3::new(0.5, y, 0.5), down == 10));
        }
    };
    let mut scene = survival_scene(&World::default(), |place| {
        place.effects = vec![effect("minecraft:slow_falling", 0, 1200)];
    });
    by_blocks(&mut scene);
    assert_eq!(health(&scene), 20.0);
    let mut scene = survival_scene(&World::default(), |_| {});
    by_blocks(&mut scene);
    assert_eq!(health(&scene), 13.0);
}

/// `Player.causeFallDamage @0-41`: nothing for a player whose abilities let them fly.
#[test]
fn a_creative_players_fall_hurts_nothing() {
    let mut scene = survival_scene(&World::default(), creative);
    fall(&mut scene, 20.0);
    assert_eq!(health(&scene), 20.0);
}

// ====================================================================================
// The hurts of a player's own tick (plan 2.3; the design's section 7, "The tick").

/// `BaseFireBlock.entityInside`: standing in fire hurts `in_fire` 1 and sets the player
/// alight, which their client is told (shared flag 0 of their own entity data); burning
/// (`Entity.baseTick @126-187`), `on_fire` 1 every 20 ticks.
#[test]
fn fire_hurts_and_sets_a_player_alight_who_then_burns() {
    let world = World::of(&[(
        FEET_BLOCK,
        state("minecraft:fire[age=0,east=false,north=false,south=false,up=false,west=false]"),
    )]);
    let (mut scene, loaded) = loaded_scene(&world, FEET, |_| {}, Vec::new());
    let me = scene.me();
    let mut seen = vec![loaded];
    until_hurt(&mut scene, "minecraft:in_fire", 5, &mut seen);
    assert_eq!(health(&scene), 19.0);
    let alight = seen
        .iter()
        .chain(scene.idle_for(40).iter())
        .flat_map(|output| own_data(output, 1))
        .any(|data| data.on_fire);
    assert!(alight, "the client is told the player burns");
    assert!(vitals(&scene).fire > 0, "{:?}", vitals(&scene).fire);
    // Out of the fire, it burns on: `on_fire` 1 at a time, 20 ticks apart.
    scene.walk_to(Vec3::new(3.5, -60.0, 3.5));
    let mut at = Vec::new();
    for tick in 0..60 {
        let output = scene.idle();
        if hurts_on(&output, me).contains(&"minecraft:on_fire") {
            at.push(tick);
        }
    }
    assert!(at.len() >= 2, "{at:?}");
    assert_eq!(at[1] - at[0], 20, "{at:?}");
}

/// `Entity.lavaHurt`: lava hurts 4 and sets the player alight for 15 seconds
/// (`lavaIgnite`: `igniteForSeconds(15)`, 300 ticks).
#[test]
fn lava_hurts_four_and_sets_the_player_alight_for_fifteen_seconds() {
    let world = World::of(&[(FEET_BLOCK, state("minecraft:lava[level=0]"))]);
    let (mut scene, loaded) = loaded_scene(&world, FEET, |_| {}, Vec::new());
    let mut seen = vec![loaded];
    until_hurt(&mut scene, "minecraft:lava", 5, &mut seen);
    assert_eq!(health(&scene), 16.0);
    assert!(
        (290..=300).contains(&vitals(&scene).fire),
        "{}",
        vitals(&scene).fire
    );
}

/// `LivingEntity.baseTick @206-415`: under water the air runs down one a tick; at −20 it
/// is 0 again with a `drown` hurt of 2 and the bubbles' entity event 67; the client is
/// told its air in its own entity data (`DATA_AIR_SUPPLY_ID`). The next hurt is 20 ticks
/// on.
#[test]
fn drowning_hurts_two_at_minus_twenty_air_with_event_sixty_seven_and_the_air_told() {
    let world = World::of(&[
        (FEET_BLOCK, state("minecraft:water[level=0]")),
        (EYES_BLOCK, state("minecraft:water[level=0]")),
    ]);
    let mut scene = survival_scene(&world, |place| place.vitals.air = 0);
    let me = scene.me();
    let mut seen = Vec::new();
    until_hurt(&mut scene, "minecraft:drown", 40, &mut seen);
    let drowned = seen.last().expect("the tick");
    assert_eq!(health(&scene), 18.0);
    assert_eq!(vitals(&scene).air, 0);
    assert!(entity_events_on(drowned, me).contains(&67));
    let airs: Vec<i32> = seen
        .iter()
        .flat_map(|output| own_data(output, 1))
        .map(|data| data.air)
        .collect();
    assert_eq!(airs.last(), Some(&0), "{airs:?}");
    assert!(airs.contains(&-19), "{airs:?}");
    let mut gap = 0;
    loop {
        gap += 1;
        let output = scene.idle();
        if hurts_on(&output, me).contains(&"minecraft:drown") {
            break;
        }
        assert!(gap < 40, "no second hurt");
    }
    assert_eq!(gap, 20);
    assert_eq!(health(&scene), 16.0);
}

/// `LivingEntity.baseTick @96-116`: a player whose eyes are in a solid block takes
/// `in_wall` 1 a tick, held back by the cooldown to one hurt every 10 ticks. Stone at
/// the feet and the eyes: no pose fits (`Player.updatePlayerPose`), and the eyes stay in
/// stone.
#[test]
fn a_player_in_a_wall_is_hurt_every_ten_ticks() {
    let world = World::of(&[
        (FEET_BLOCK, state("minecraft:stone")),
        (EYES_BLOCK, state("minecraft:stone")),
    ]);
    let (mut scene, loaded) = loaded_scene(&world, FEET, |_| {}, Vec::new());
    let at = hurt_ticks(&mut scene, &loaded, "minecraft:in_wall", 35);
    assert!(at.len() >= 3, "{at:?}");
    assert_eq!(at[1] - at[0], 10, "{at:?}");
    assert_eq!(at[2] - at[1], 10, "{at:?}");
}

/// `CactusBlock.entityInside`: `cactus` 1 to a player whose box reaches into it.
#[test]
fn a_cactus_hurts_one() {
    let world = World::of(&[
        (bp(1, -61, 0), state("minecraft:sand")),
        (bp(1, -60, 0), state("minecraft:cactus[age=0]")),
    ]);
    let (mut scene, loaded) = loaded_scene(&world, Vec3::new(0.95, -60.0, 0.5), |_| {}, Vec::new());
    let mut seen = vec![loaded];
    until_hurt(&mut scene, "minecraft:cactus", 5, &mut seen);
    assert_eq!(health(&scene), 19.0);
}

/// `MagmaBlock.stepOn`: `hot_floor` 1 to one standing on it who is not
/// `isSteppingCarefully` (sneaking).
#[test]
fn magma_hurts_a_player_who_does_not_sneak() {
    let world = World::of(&[(FLOOR, state("minecraft:magma_block"))]);
    let (mut scene, loaded) = loaded_scene(&world, FEET, |_| {}, vec![keys(true, false)]);
    let me = scene.me();
    assert!(hurts_on(&loaded, me).is_empty(), "sneaking");
    for output in scene.idle_for(30) {
        assert!(hurts_on(&output, me).is_empty(), "sneaking");
    }
    assert_eq!(health(&scene), 20.0);
    let stood = scene.input(keys(false, false));
    let mut seen = vec![stood];
    until_hurt(&mut scene, "minecraft:hot_floor", 5, &mut seen);
    assert_eq!(health(&scene), 19.0);
}

/// `LivingEntity.aiStep @679-761`: fully frozen (140 ticks in powder snow), `freeze` 1
/// whenever the tick count is a multiple of 40; the client is told its ticks frozen.
#[test]
fn a_frozen_player_in_powder_snow_is_hurt_every_forty_ticks() {
    let world = World::of(&[(FEET_BLOCK, state("minecraft:powder_snow"))]);
    let (mut scene, loaded) =
        loaded_scene(&world, FEET, |place| place.vitals.frozen = 140, Vec::new());
    let at = hurt_ticks(&mut scene, &loaded, "minecraft:freeze", 79);
    assert_eq!(at.len(), 2, "{at:?}");
    assert_eq!(at[1] - at[0], 40);
    assert_eq!(health(&scene), 18.0);
    assert!(vitals(&scene).frozen >= 140);
}

/// `FoodData.tick @239-311`: at food 0, `starve` 1 every 80 ticks while the health is
/// above 10 on easy.
#[test]
fn a_starving_player_on_easy_loses_health_every_eighty_ticks_down_to_ten() {
    let (mut scene, loaded) = loaded_scene(
        &World::default(),
        FEET,
        |place| {
            place.health = 12.0;
            place.vitals.food.food = 0;
            place.vitals.food.saturation = 0.0;
        },
        Vec::new(),
    );
    let at = hurt_ticks(&mut scene, &loaded, "minecraft:starve", 400);
    assert_eq!(at.len(), 2, "{at:?}");
    assert_eq!(at[1] - at[0], 80);
    assert_eq!(health(&scene), 10.0);
}

/// `PoisonMobEffect.applyEffectTick`: `magic` 1 while the health is above 1, every
/// `25 >> amplifier` ticks (`shouldApplyEffectTickThisTick`): it never kills.
#[test]
fn poison_hurts_every_twenty_five_ticks_and_stops_at_one() {
    let (mut scene, loaded) = loaded_scene(
        &World::default(),
        FEET,
        |place| {
            place.health = 3.0;
            place.effects = vec![effect("minecraft:poison", 0, 400)];
        },
        Vec::new(),
    );
    let at = hurt_ticks(&mut scene, &loaded, "minecraft:magic", 150);
    assert_eq!(at.len(), 2, "{at:?}");
    assert_eq!(at[1] - at[0], 25);
    assert_eq!(health(&scene), 1.0);
    assert!(!scene.region.player_state(player(1)).expect("there").dead);
}

/// `WitherMobEffect.applyEffectTick`: `wither` 1 every `40 >> amplifier` ticks, to the
/// death; the message is `death.attack.wither`.
#[test]
fn wither_hurts_every_forty_ticks_and_kills() {
    let (mut scene, loaded) = loaded_scene(
        &World::default(),
        FEET,
        |place| {
            place.health = 2.0;
            place.effects = vec![effect("minecraft:wither", 0, 400)];
        },
        Vec::new(),
    );
    let me = scene.me();
    let mut at = Vec::new();
    if hurts_on(&loaded, me).contains(&"minecraft:wither") {
        at.push(0);
    }
    let mut died = None;
    for tick in 1..120 {
        let output = scene.idle();
        if hurts_on(&output, me).contains(&"minecraft:wither") {
            at.push(tick);
        }
        if let Some((_, message)) = deaths(&output).into_iter().next() {
            died = Some(message.key);
            break;
        }
    }
    assert_eq!(at.len(), 2, "{at:?}");
    assert_eq!(at[1] - at[0], 40);
    assert_eq!(died.as_deref(), Some("death.attack.wither"));
}

// ====================================================================================
// Hunger (plan 2.5).

/// `ServerPlayer.checkMovementStatistics @66-295`: sprinting on the ground costs 0.1 a
/// metre, in whole centimetres a move (`Math.round(distance · 100)`, `0.1 · cm · 0.01`).
/// Walking costs nothing.
#[test]
fn sprinting_costs_a_tenth_a_metre_and_walking_nothing() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.vitals.food.saturation = 0.0;
    });
    for step in 1..=5 {
        scene.walk_to(Vec3::new(0.5, -60.0, 0.5 + f64::from(step)));
    }
    assert_eq!(vitals(&scene).food.exhaustion, 0.0, "walking");
    scene.input(keys(false, true));
    for step in 6..=15 {
        scene.walk_to(Vec3::new(0.5, -60.0, 0.5 + f64::from(step)));
    }
    let exhaustion = vitals(&scene).food.exhaustion;
    assert!((exhaustion - 1.0).abs() < 1e-4, "{exhaustion}");
}

/// `ServerPlayer.jumpFromGround @19, @29`, seen by the server as a move off the ground
/// that rises (`handlePlayerPositionChange @487-511`): 0.05, or 0.2 sprinting.
#[test]
fn a_jump_costs_five_hundredths_and_a_sprinting_jump_a_fifth() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.vitals.food.saturation = 0.0;
    });
    scene.input(move_to(Vec3::new(0.5, -59.58, 0.5), false));
    scene.input(move_to(FEET, true));
    let walking = vitals(&scene).food.exhaustion;
    assert!((walking - 0.05).abs() < 1e-6, "{walking}");
    scene.input(keys(false, true));
    scene.input(move_to(Vec3::new(0.5, -59.58, 0.5), false));
    scene.input(move_to(FEET, true));
    let sprinting = vitals(&scene).food.exhaustion - walking;
    assert!((sprinting - 0.2).abs() < 1e-6, "{sprinting}");
}

/// `FoodData.tick @79-176`: hurt at food 20 with saturation, every 10 ticks the player
/// heals `min(saturation, 6) / 6` and that much saturation goes into exhaustion.
#[test]
fn at_full_food_a_hurt_player_heals_by_their_saturation_every_ten_ticks() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 10.0;
        place.vitals.food.food = 20;
        place.vitals.food.saturation = 3.0;
    });
    let mut heals = Vec::new();
    for tick in 0..25 {
        let output = scene.idle();
        for (health, _, _) in told_health(&output, 1) {
            heals.push((tick, health));
        }
    }
    assert!(heals.len() >= 2, "{heals:?}");
    assert_eq!(heals[1].0 - heals[0].0, 10, "{heals:?}");
    assert!((heals[0].1 - 10.5).abs() < 1e-6, "3 / 6: {heals:?}");
    assert!((heals[1].1 - 11.0).abs() < 1e-6, "{heals:?}");
}

/// `FoodData.tick @181-235`: hurt at food 18 or 19 with no saturation, every 80 ticks
/// the player heals 1 and 6 goes into exhaustion, which costs a point of food.
#[test]
fn at_eighteen_food_a_hurt_player_heals_one_every_eighty_ticks() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 10.0;
        place.vitals.food.food = 19;
        place.vitals.food.saturation = 0.0;
    });
    let mut heals = Vec::new();
    for tick in 0..200 {
        let output = scene.idle();
        for (health, food, _) in told_health(&output, 1) {
            heals.push((tick, health, food));
        }
    }
    let healed: Vec<(i32, f32)> = heals
        .iter()
        .filter(|(_, health, _)| *health > 10.0)
        .scan(10.0, |last, (tick, health, _)| {
            let new = *health > *last;
            *last = *health;
            Some(new.then_some((*tick, *health)))
        })
        .flatten()
        .collect();
    assert_eq!(healed.len(), 2, "{heals:?}");
    assert_eq!(healed[1].0 - healed[0].0, 80, "{heals:?}");
    assert_eq!(health(&scene), 12.0);
    assert_eq!(
        vitals(&scene).food.food,
        17,
        "two heals cost 12: two points of food"
    );
}

/// `Consumable.onConsume` after 1.6 s (32 ticks), `FoodData.eat(FoodProperties) @0-12`:
/// bread (nutrition 5, saturation 6) from food 10 and no saturation gives 15 and 6, and
/// the loaf is gone.
#[test]
fn eating_bread_feeds_five_and_six() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.vitals.food.food = 10;
        place.vitals.food.saturation = 0.0;
        place
            .inventory
            .set(0, Some(ItemStack::new(items::BREAD, 2)));
    });
    scene.input(PlayerInput::UseItem {
        rotation: (0.0, 0.0),
        sneaking: false,
        sequence: 1,
    });
    let mut seen = Vec::new();
    let fed = idle_until_found(&mut scene, 40, &mut seen, |output| {
        told_health(output, 1)
            .into_iter()
            .find(|(_, food, _)| *food == 15)
    });
    assert_eq!(fed, (20.0, 15, 6.0));
    let food = vitals(&scene).food;
    assert_eq!((food.food, food.saturation), (15, 6.0));
    assert_eq!(scene.holds(0), Some(ItemStack::new(items::BREAD, 1)));
}

/// `ServerPlayer.doTick @146-264`: `SetHealth` is sent when the health or the food
/// changed, or the saturation became or stopped being zero, and not when the saturation
/// only fell. Hunger at amplifier 39 costs 0.2 a tick (`HungerMobEffect`: `0.005 · (amp +
/// 1)`): from saturation 2, at food 17, each 4 of exhaustion takes 1 of saturation, then
/// of food. Told: saturation 0 at food 17, then food 16 — not saturation 1.
#[test]
fn the_health_is_told_on_a_change_of_health_or_food_or_saturation_s_being_zero() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.vitals.food.saturation = 2.0;
        place.effects = vec![effect("minecraft:hunger", 39, 1000)];
    });
    let mut told = Vec::new();
    for _ in 0..70 {
        let output = scene.idle();
        told.extend(told_health(&output, 1));
    }
    assert_eq!(told, [(20.0, 17, 0.0), (20.0, 16, 0.0)]);
}

/// `ServerPlayer.resetSentInfo` sets `lastSentHealth` to −1e8, so the first `doTick`
/// after a join sends `SetHealth` whatever the health (`doTick @146-264`): a player who
/// comes back hurt and hungry is told so at once.
#[test]
fn a_joining_player_is_told_their_health_food_and_saturation() {
    let layout = Layout::alone();
    let region = region_at(
        &layout,
        &World::default(),
        Inventory::default(),
        Some(NIGHT),
    );
    let mut scene = Scene {
        region,
        layout,
        inventory: Inventory::default(),
        actors: Vec::new(),
    };
    let (_, entered) = scene.join_seen(1, FEET, |place| {
        survival(place);
        place.health = 7.0;
        place.vitals.food.food = 3;
        place.vitals.food.saturation = 0.0;
    });
    let mut told = told_health(&entered, 1);
    let loaded = scene.input(PlayerInput::Loaded);
    told.extend(told_health(&loaded, 1));
    assert_eq!(told.first(), Some(&(7.0, 3, 0.0)), "{told:?}");
}

/// The same for a newcomer, whom the store has no place for: the game tells every joining
/// player (`resetSentInfo`), so a new one is told 20, 20 and 5.
#[ignore = "named: a newcomer with a new player's health is not told it at the join; their client shows the same (hunger.rs)"]
#[test]
fn a_newcomer_is_told_their_health_food_and_saturation() {
    let layout = Layout::alone();
    let mut region = region_at(
        &layout,
        &World::default(),
        Inventory::default(),
        Some(NIGHT),
    );
    region.tick(&TickInputs {
        player_changes: vec![clustine_sim::PlayerChange::Join(
            EDGE,
            clustine_sim::PlayerJoin {
                appearance: Default::default(),
                player: player(1),
                name: "Newcomer".into(),
                attempt: 7001,
            },
        )],
        ..TickInputs::default()
    });
    let entering = region
        .entering_state(player(1))
        .expect("the join is entering")
        .entity_id;
    let mut told = Vec::new();
    let entered = region.tick(&TickInputs {
        entered: vec![clustine_sim::Entered {
            unit: None,
            player: player(1),
            entity: entering,
            place: None,
            holder: None,
        }],
        ..TickInputs::default()
    });
    told.extend(told_health(&entered, 1));
    for _ in 0..3 {
        told.extend(told_health(&region.tick(&TickInputs::default()), 1));
    }
    assert_eq!(told.first(), Some(&(20.0, 20, 5.0)), "{told:?}");
}

// ====================================================================================
// Death (plan 2.4).

/// `CombatTracker.getDeathMessage`: `death.attack.<msgId>`, the zombie's `mob`, naming
/// the zombie as the killer.
#[test]
fn a_zombie_s_kill_is_told_as_death_attack_mob_naming_the_zombie() {
    let world = World::default().with_entities(&[zombie_at([0.5, -60.0, 3.5])]);
    let mut scene = survival_scene(&world, |place| place.health = 2.0);
    let mut seen = Vec::new();
    let (who, message) = idle_until_found(&mut scene, 400, &mut seen, |output| {
        deaths(output).into_iter().next()
    });
    assert_eq!(who, player(1));
    assert_eq!(message.key, "death.attack.mob");
    let killer = message.killer.expect("the killer is named");
    assert_eq!(i32::from(killer.kind), entity_types::ZOMBIE);
    assert_eq!(killer.entity, key(1));
    assert!(!killer.player);
}

/// `CombatTracker.getDeathMessage @75-96` and `getFallMessage`: a death by a fall of more
/// than 5 blocks with no killer is `death.fell.accident.generic`
/// (`FallLocation.GENERIC`); the void's is `death.attack.outOfWorld`.
#[test]
fn a_fall_s_death_and_the_void_s_have_their_messages() {
    let mut scene = survival_scene(&World::default(), |place| place.health = 5.0);
    let landed = fall(&mut scene, 10.0).pop().expect("the landing");
    let died: Vec<String> = deaths(&landed)
        .into_iter()
        .map(|(_, message)| message.key)
        .collect();
    assert_eq!(died, ["death.fell.accident.generic"]);

    let mut scene = survival_scene(&World::default(), |_| {});
    scene.input(move_to(Vec3::new(0.5, -140.0, 0.5), false));
    let mut seen = Vec::new();
    let (_, message) = idle_until_found(&mut scene, 200, &mut seen, |output| {
        deaths(output).into_iter().next()
    });
    assert_eq!(message.key, "death.attack.outOfWorld");
    assert_eq!(message.killer, None);
}

/// `Player.dropEquipment @0-35`, `Inventory.dropAll`, `Player.drop(stack, true, false)`
/// (`createItemStackToDrop`): every stack thrown from the eyes less 0.3, in a random
/// direction (`f = r · 0.5`, `a = r · 2π`, motion `(−sin a · f, 0.2, cos a · f)`), with
/// no thrower and a pick-up delay of 40; one with Curse of Vanishing is destroyed
/// (`prevent_equipment_drop`). The experience lies as orbs of `min(level · 7, 100)`
/// points (`Player.getBaseExperienceReward @0-40`). The dead body picks nothing up.
#[test]
fn a_death_drops_the_inventory_spread_and_unowned_and_the_experience_as_orbs() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 5.0;
        place
            .inventory
            .set(0, Some(ItemStack::new(items::STONE, 5)));
        place.inventory.set(9, Some(ItemStack::new(items::DIRT, 3)));
        place
            .inventory
            .set(ARMOUR_START + 3, Some(enchanted(items::IRON_HELMET, &[])));
        place.inventory.set(
            10,
            Some(enchanted(
                items::DIAMOND_SWORD,
                &[("minecraft:vanishing_curse", 1)],
            )),
        );
        place.experience.level = 3;
    });
    let landed = fall(&mut scene, 10.0).pop().expect("the landing");
    assert_eq!(deaths(&landed).len(), 1);
    let state = scene
        .region
        .player_state(player(1))
        .expect("the body stays");
    assert!(state.dead);
    assert!(state.inventory.0.iter().all(Option::is_none));

    let lying: Vec<Entity> = scene
        .all()
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::Item { .. }))
        .collect();
    let mut kinds: Vec<i32> = lying
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, .. } => Some(stack.item),
            _ => None,
        })
        .collect();
    kinds.sort_unstable();
    let mut expected = vec![items::STONE, items::DIRT, items::IRON_HELMET];
    expected.sort_unstable();
    assert_eq!(
        kinds, expected,
        "the sword with Curse of Vanishing is destroyed"
    );
    for entity in &lying {
        let EntityData::Item {
            thrower,
            pick_up_delay,
            ..
        } = &entity.data
        else {
            unreachable!()
        };
        assert_eq!(*thrower, None);
        assert!(
            (38..=40).contains(pick_up_delay),
            "a pick-up delay of 40: {pick_up_delay}"
        );
        let [dx, dz] = [entity.position[0] - 0.5, entity.position[2] - 0.5];
        assert!(
            (dx * dx + dz * dz).sqrt() < 1.2,
            "thrown about the body: {:?}",
            entity.position
        );
        assert!(
            entity.position[1] > -60.0,
            "from above the feet: {:?}",
            entity.position
        );
    }
    let directions: Vec<[f64; 2]> = lying
        .iter()
        .map(|entity| [entity.motion[0], entity.motion[2]])
        .collect();
    assert!(
        directions.windows(2).any(|pair| pair[0] != pair[1]),
        "each thrown its own way: {directions:?}"
    );
    let points: i32 = scene
        .all()
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::ExperienceOrb { value, count, .. } => Some(value * count),
            _ => None,
        })
        .sum();
    assert_eq!(points, 21, "3 · 7");

    // Beyond the pick-up delay, nothing is taken up by the dead body.
    scene.idle_for(60);
    let still = scene
        .all()
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::Item { .. }))
        .count();
    assert_eq!(still, 3);
}

/// `Player.getBaseExperienceReward`: at most 100 points, whatever the level.
#[test]
fn a_death_drops_at_most_a_hundred_points_of_experience() {
    let mut scene = survival_scene(&World::default(), |place| {
        place.health = 5.0;
        place.experience.level = 30;
    });
    fall(&mut scene, 10.0);
    let points: i32 = scene
        .all()
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::ExperienceOrb { value, count, .. } => Some(value * count),
            _ => None,
        })
        .sum();
    assert_eq!(points, 100);
}

/// `ServerPlayer.restoreFrom @0-335` after a death: the new body has full health, a new
/// `FoodData` (20 and 5), full air and no fire, and the client is told so.
#[test]
fn a_respawn_gives_a_new_body_with_full_health_food_and_air() {
    let world = World::of(&[
        (FEET_BLOCK, state("minecraft:water[level=0]")),
        (EYES_BLOCK, state("minecraft:water[level=0]")),
    ]);
    let mut scene = survival_scene(&world, |place| {
        place.health = 1.0;
        place.vitals.air = 0;
        place.vitals.food.food = 6;
        place.vitals.food.saturation = 0.0;
    });
    let mut seen = Vec::new();
    idle_until_found(&mut scene, 40, &mut seen, |output| {
        deaths(output).into_iter().next()
    });
    let respawned = scene.input(PlayerInput::PerformRespawn);
    let mut outputs = vec![respawned];
    outputs.extend(scene.idle_for(3));
    let state = scene.region.player_state(player(1)).expect("there");
    assert!(!state.dead);
    assert_eq!(state.health, 20.0);
    assert_eq!(state.vitals.food.food, 20);
    assert_eq!(state.vitals.food.saturation, 5.0);
    assert_eq!(state.vitals.air, 300);
    assert!(state.vitals.fire <= 0);
    let told: Vec<(f32, i32, f32)> = outputs
        .iter()
        .flat_map(|output| told_health(output, 1))
        .collect();
    assert!(told.contains(&(20.0, 20, 5.0)), "{told:?}");
}

// ====================================================================================
// Players, mobs and blasts (plan 2.2, 2.3).

/// `Player.attack`: a fist hits another survival player for 1 (`player_attack`, which
/// does not scale with the difficulty: its cause is a player), and
/// `Player.causeExtraKnockback @124-166` tells the hit player's client its motion at once,
/// in the tick of the hit.
#[test]
fn a_players_hit_on_another_takes_one_and_the_knock_is_told_at_once() {
    let mut scene = scene_of_two(|_| {}, |_| {}, Vec3::new(0.5, -60.0, 2.0));
    let me = scene.me();
    let hit = scene.input_of(
        1,
        PlayerInput::Attack {
            entity: me,
            part: None,
        },
    );
    assert_eq!(health(&scene), 19.0);
    let attacker = scene.actors[1].entity;
    assert_eq!(
        damage_events_on(&hit, me),
        [("minecraft:player_attack", Some(attacker), Some(attacker))]
    );
    let knocks = knocked(&hit, 1);
    assert_eq!(knocks.len(), 1, "{:?}", hit.player_events);
    assert!(knocks[0][2] < 0.0, "pushed north, away: {knocks:?}");
}

/// `Player.hurtServer @0-30`: a creative player (`abilities.invulnerable`) refuses the hit.
#[test]
fn a_creative_player_refuses_another_players_hit() {
    let mut scene = scene_of_two(creative, |_| {}, Vec3::new(0.5, -60.0, 2.0));
    let me = scene.me();
    let hit = scene.input_of(
        1,
        PlayerInput::Attack {
            entity: me,
            part: None,
        },
    );
    assert_eq!(health(&scene), 20.0);
    assert!(hurts_on(&hit, me).is_empty());
    assert!(knocked(&hit, 1).is_empty());
}

/// `AbstractArrow.onHitEntity`: a skeleton's arrow hits a survival player as `arrow`,
/// caused by the skeleton, and sticks in them (`DATA_ARROW_COUNT_ID`, told in their own
/// entity data).
#[test]
fn a_skeleton_s_arrow_hurts_a_survival_player_and_sticks_in_them() {
    let skeleton = laid(
        &made(items::SKELETON_SPAWN_EGG, entity_types::SKELETON),
        1,
        [0.5, -60.0, 9.5],
        180.0,
    );
    let world = World::default().with_entities(&[skeleton]);
    let mut scene = survival_scene(&world, |_| {});
    let me = scene.me();
    let mut seen = Vec::new();
    until_hurt(&mut scene, "minecraft:arrow", 600, &mut seen);
    let hit = seen.last().expect("the hit");
    let (_, cause, _) = damage_events_on(hit, me)[0];
    assert_eq!(cause, Some(key(1)), "the skeleton shot it");
    assert!(health(&scene) < 20.0);
    let arrows: Vec<i32> = seen
        .iter()
        .chain(scene.idle_for(2).iter())
        .flat_map(|output| own_data(output, 1))
        .map(|data| data.arrows)
        .collect();
    assert!(arrows.contains(&1), "{arrows:?}");
}

/// `ServerExplosion.hurtEntities` and `ExplosionDamageCalculator.getEntityDamageAmount
/// @62-63`: TNT of power 4 (`PrimedTnt.explode`, at `getY(0.0625)`), three blocks east,
/// unhidden: `(i² + i) / 2 · 7 · 8 + 1` with `i = 1 − distance / 8`, which `explosion`
/// scales always: on easy `min(d / 2 + 1, d)`.
#[test]
fn tnt_hurts_a_survival_player_by_the_game_s_formula_on_easy() {
    let tnt = n6_entities::primed(1, [3.5, -60.0, 0.5], 1);
    let world = World::default().with_entities(&[tnt]);
    let layout = Layout::alone();
    // The TNT goes off in the first tick its chunk is loaded in: the player is in place
    // first, and the TNT is laid down by an arrival afterwards.
    let region = region_at(
        &layout,
        &World::default(),
        Inventory::default(),
        Some(NIGHT),
    );
    let mut scene = Scene {
        region,
        layout,
        inventory: Inventory::default(),
        actors: Vec::new(),
    };
    scene.join_with(1, FEET, survival);
    scene.input(PlayerInput::Loaded);
    let me = scene.me();
    let arrived = scene.region.tick(&TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 1,
                index: 0,
            },
            chunk: ChunkPos::new(0, 0),
            entity: world.entities[0].clone(),
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    let mut seen = vec![arrived];
    if !seen.iter().any(|output| !hurts_on(output, me).is_empty()) {
        until_hurt(&mut scene, "minecraft:explosion", 5, &mut seen);
    }
    let hit = seen.last().expect("the blast");
    assert_eq!(hurts_on(hit, me), ["minecraft:explosion"]);
    let centre = [3.5, -60.0 + 0.98 * 0.0625, 0.5];
    let distance = ((centre[0] - 0.5_f64).powi(2)
        + (centre[1] + 60.0_f64).powi(2)
        + (centre[2] - 0.5_f64).powi(2))
    .sqrt()
        / 8.0;
    let i = 1.0 - distance;
    let damage = ((i * i + i) / 2.0 * 7.0 * 8.0 + 1.0) as f32;
    let easy = (damage / 2.0 + 1.0).min(damage);
    let expected = 20.0 - easy;
    assert!(
        (health(&scene) - expected).abs() < 1e-3,
        "{} where {expected}",
        health(&scene)
    );
}

/// `EnderDragon.aiStep @1230-1394`: unless hurt recently, the dragon's wings knock and
/// hurt (`knockBack`: `mob_attack` 5) and its head and neck hurt (`hurt`: `mob_attack`
/// 10) the living entities their boxes reach, chosen by `NO_CREATIVE_OR_SPECTATOR`; a
/// dragon is a living non-player, so on easy 10 is `min(10 / 2 + 1, 10)`, 6. A dragon
/// from an egg laid five blocks south of player 1 flies at them: a survival player is
/// hit, a creative one never.
#[test]
fn the_dragon_hits_a_survival_player_and_never_a_creative_one() {
    let record = made(items::ENDER_DRAGON_SPAWN_EGG, entity_types::ENDER_DRAGON);
    let dragon = laid(&record, 1, [0.5, -60.0, 5.0], 90.0);
    let world = World::default().with_entities(&[dragon]);
    let (mut scene, loaded) = loaded_scene(&world, FEET, |_| {}, Vec::new());
    let me = scene.me();
    let mut seen = vec![loaded];
    seen.extend(scene.idle_for(40));
    let hits: Vec<_> = seen
        .iter()
        .flat_map(|output| damage_events_on(output, me))
        .collect();
    assert_eq!(
        hits,
        [("minecraft:mob_attack", Some(key(1)), Some(key(1)))],
        "the dragon's hit"
    );
    assert!(
        [14.0, 16.5].contains(&health(&scene)),
        "the head's 6 or a wing's 3.5: {}",
        health(&scene)
    );

    let (mut scene, loaded) = loaded_scene(&world, FEET, creative, Vec::new());
    let mut seen = vec![loaded];
    seen.extend(scene.idle_for(40));
    assert!(
        seen.iter()
            .all(|output| damage_events_on(output, me).is_empty())
    );
    assert_eq!(health(&scene), 20.0);
}

// ====================================================================================
// Across a pinned line (the design's section 3, "Across a line"; plan 4.13).

/// Two regions on either side of the pinned line at x = 16, at night, each with one
/// player, run a tick at a time as the store couples them (as in
/// `brains_specification.rs`): each takes the other's previous tick's entity changes
/// into its copies, its effects on its own land, its arrivals and the faces of the
/// other's players.
struct Line {
    west: Scene,
    east: Scene,
    from_west: TickOutput,
    from_east: TickOutput,
    sequence: u64,
}

impl Line {
    fn new(
        world: &World,
        (west_feet, west_change): (Vec3, fn(&mut Place)),
        (east_feet, east_change): (Vec3, fn(&mut Place)),
    ) -> Self {
        let side = |west: bool| {
            let layout = Layout::side(west);
            let region = region_at(&layout, world, Inventory::default(), Some(NIGHT));
            Scene {
                region,
                layout,
                inventory: Inventory::default(),
                actors: Vec::new(),
            }
        };
        let mut west = side(true);
        west.join_with(1, west_feet, west_change);
        let mut east = side(false);
        east.join_with(2, east_feet, east_change);
        let mut line = Self {
            west,
            east,
            from_west: TickOutput::default(),
            from_east: TickOutput::default(),
            sequence: 1_000,
        };
        // The joins' notes went by before the line was coupled: a step of a hair makes
        // each player's face again for the other side; and both load.
        let hair = |feet: Vec3| {
            vec![
                move_to(Vec3::new(feet.x, feet.y, feet.z + 0.001), true),
                PlayerInput::Loaded,
            ]
        };
        line.step(hair(west_feet), hair(east_feet));
        line
    }

    fn carried(&mut self, from: &TickOutput, into: &Layout) -> TickInputs {
        self.sequence += 1;
        let sequence = self.sequence;
        let mut updates: Vec<(ChunkPos, Vec<EntityChange>)> = Vec::new();
        for (chunk, change) in &from.entity_changes {
            if !into.copies.contains(chunk) {
                continue;
            }
            match updates.iter_mut().find(|(at, _)| at == chunk) {
                Some((_, changes)) => changes.push(change.clone()),
                None => updates.push((*chunk, vec![change.clone()])),
            }
        }
        let effects: Vec<FiledEffect> = from
            .effects
            .iter()
            .filter(|effect| into.held.contains(&effect.at.chunk()))
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence,
                    index: u32::try_from(index).expect("few"),
                },
                effect: effect.clone(),
                due: Due::FromTaking,
            })
            .collect();
        let arrivals: Vec<Arrival> = from
            .arrivals
            .iter()
            .filter(|(chunk, _)| into.held.contains(chunk))
            .enumerate()
            .map(|(index, (chunk, entity))| Arrival {
                id: EffectId {
                    sequence: sequence + 500_000,
                    index: u32::try_from(index).expect("few"),
                },
                chunk: *chunk,
                entity: entity.clone(),
                due: Due::FromTaking,
            })
            .collect();
        let faces: Vec<FaceChange> = from
            .stays
            .iter()
            .enumerate()
            .filter_map(|(index, note)| match note {
                StayNote::Has {
                    player,
                    entity,
                    hops,
                    face,
                    ..
                } => Some(FaceChange::Face {
                    player: *player,
                    entity: *entity,
                    hops: *hops,
                    seq: FaceSeq(sequence, u32::try_from(index).expect("few")),
                    face: face.clone(),
                }),
                _ => None,
            })
            .collect();
        TickInputs {
            copy_entity_updates: updates,
            effects,
            arrivals,
            faces,
            ..TickInputs::default()
        }
    }

    fn step(&mut self, west: Vec<PlayerInput>, east: Vec<PlayerInput>) -> (TickOutput, TickOutput) {
        let from_east = std::mem::take(&mut self.from_east);
        let from_west = std::mem::take(&mut self.from_west);
        let west_layout = self.west.layout.clone();
        let east_layout = self.east.layout.clone();
        let mut to_west = self.carried(&from_east, &west_layout);
        let mut to_east = self.carried(&from_west, &east_layout);
        to_west.inputs = numbered(&mut self.west, west);
        to_east.inputs = numbered(&mut self.east, east);
        let west_out = self.west.region.tick(&to_west);
        let east_out = self.east.region.tick(&to_east);
        self.from_west = west_out.clone();
        self.from_east = east_out.clone();
        (west_out, east_out)
    }

    fn idle(&mut self) -> (TickOutput, TickOutput) {
        self.step(Vec::new(), Vec::new())
    }
}

/// `inputs` of the only actor of `scene`, numbered on from their last.
fn numbered(
    scene: &mut Scene,
    inputs: Vec<PlayerInput>,
) -> Vec<(clustine_world::EdgeId, PlayerId, EntityId, u64, PlayerInput)> {
    let actor = &mut scene.actors[0];
    inputs
        .into_iter()
        .map(|input| {
            actor.input += 1;
            (EDGE, actor.id, actor.entity, actor.input, input)
        })
        .collect()
}

/// A zombie in the west, whose region keeps the face of the survival player 2 standing
/// just across the line in the east, hits them: the hurt goes to the east as an effect
/// on the player (`EntityUse::Hurt`), and their region takes it as its own player's hurt
/// from a zombie on easy, 2.5, and tells their client its knock.
#[test]
fn a_zombie_s_hit_across_a_line_reaches_the_player_s_region() {
    let world = World::default().with_entities(&[zombie_at([13.5, -60.0, 0.5])]);
    let mut line = Line::new(
        &world,
        (Vec3::new(-20.5, -60.0, 0.5), creative),
        (Vec3::new(16.5, -60.0, 0.5), survival),
    );
    let me = line.east.me();
    let mut hurt = None;
    for _ in 0..400 {
        let (_, east) = line.idle();
        if hurts_on(&east, me).contains(&"minecraft:mob_attack") {
            hurt = Some(east);
            break;
        }
    }
    let hit = hurt.expect("the zombie's hit reaches player 2's region");
    assert_eq!(health_of(&line.east, 2), 17.5);
    let mut knocks = knocked(&hit, 2);
    knocks.extend(knocked(&line.idle().1, 2));
    assert_eq!(knocks.len(), 1, "{knocks:?}");
    assert!(
        knocks[0][0] > 0.0,
        "pushed east, away from the zombie: {knocks:?}"
    );
}
