//! Tests of the monsters' bodies of step N7k, written from `docs/groundwork/n7k-brief.md`
//! (K: sections 3.1, 3.6 to 3.8, 11.3, 11.4, 11.10 to 11.12) and the plan
//! `docs/groundwork/n7k-plan.md` (sections 1.3, 1.4, 10 and 11, which hold where they
//! differ from K), by someone who did not write the code, driving regions by their public
//! interface alone: players who use spawn eggs, items and their fists, and what the
//! regions report and keep.
//!
//! What they cover: the creeper lit by flint and steel or a fire charge (30 ticks, radius
//! 3, nothing of it left, blocks broken, across a pinned line as TNT's explosion is) and
//! its running from cats; the spiders (climbing, running from armadillos, the jockey, the
//! cave spider's egg that draws nothing but its yaw); the witch's drinking; the illagers'
//! eggs (what they hold, the leader's banner); the guardians (the flop, the spikes, the
//! elder's curse that finds no creative player); the world's difficulty; and the snowball
//! a player throws (its flight, its hit's push, the creative player keeping it, a pinned
//! line).
//!
//! The scene is `n6_scene`'s: a stone floor at y = -61, player 1 standing on it, every
//! player creative. Unless a test says otherwise, it is night (`Scene::alone_at_night`),
//! so that no daylight plays a part.

mod n6_scene;

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_sim::api::{Face, Inventory, PlayerInput, RegionEvent};
use clustine_sim::shown::{DataShown, mob_values};
use clustine_sim::{TickInputs, TickOutput, WorldClock};
use clustine_world::{
    Arrival, BlockPos, Due, Effect, EffectId, Effected, Entity, EntityData, EntityId, EntityUse,
    FiledEffect, ItemStack, Vec3,
};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// The numbers of the specification.

/// `Creeper.maxSwell`: the ticks from the first swell to the explosion (K 11.3).
const FUSE: u32 = 30;
/// `Creeper.explosionRadius` (K 11.3).
const CREEPER_RADIUS: f32 = 3.0;

/// The synced values' indices: `LivingEntity`'s health, `Mob`'s flags, the creeper's
/// direction of swelling and ignition (K 11.3), the spider's flags (K 11.4), the witch's
/// `DATA_USING_ITEM` (K 3.6).
const HEALTH: u8 = 9;
const MOB_FLAGS: u8 = 15;
const SWELL_DIR: u8 = 16;
const IGNITED: u8 = 18;
const SPIDER_FLAGS: u8 = 16;
const USING_ITEM: u8 = 17;

/// `Mob.DATA_MOB_FLAGS_ID`'s left-handed bit; the spider's climbing bit.
const LEFT_HANDED: u8 = 2;
const CLIMBING: u8 = 1;

/// `EquipmentSlot` ordinals: the main hand and the head.
const MAIN_HAND: u8 = 0;
const HEAD: u8 = 5;

/// The witch's health (K 3.0) and what a potion of healing gives (`4 << 0`).
const WITCH_HEALTH: f32 = 26.0;
const HEALING: f32 = 4.0;
/// A potion's use duration, 1.6 seconds (K 3.6, the plan's section 11 item 4).
const POTION_TICKS: u32 = 32;

/// The entity events: a snowball's particles (K 11.10), a living entity's death.
const SNOWBALL_HIT: u8 = 3;

/// `SnowballItem`'s throw: the sound's volume, and the shot's power (K 11.10).
const THROW_VOLUME: f32 = 0.5;
const THROW_POWER: f64 = 1.5;
/// `ThrowableProjectile`'s gravity and the air's drag (K 11.10).
const GRAVITY: f64 = 0.03;

/// The floor's top, where feet stand.
const FLOOR: f64 = -60.0;

// ------------------------------------------------------------------------------------
// Scenes and the mobs in them.

fn one(item: i32) -> ItemStack {
    stack(item, 1)
}

/// An inventory with `held` in slots 0 onwards, one of each.
fn holding(held: &[i32]) -> Inventory {
    let slots: Vec<(usize, ItemStack)> = held
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, one(*item)))
        .collect();
    inventory(&slots)
}

/// A region alone at night over `world`, player 1 at `feet` holding `held`.
fn night(world: &World, held: &[i32], feet: Vec3) -> Scene {
    Scene::alone_at_night(world, holding(held), feet)
}

/// Player 1 uses the egg in `slot` on the top of the floor block under `at`; the mobs of
/// `kind` it made, by key, the rider of a jockey not among them, and the tick's output.
fn hatch_at(scene: &mut Scene, slot: u8, kind: i32, at: BlockPos) -> (Vec<EntityId>, TickOutput) {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    let output = scene.click(bp(at.x, at.y - 1, at.z), Face::Top);
    let made = scene
        .of_kind(kind)
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    (made, output)
}

/// [`hatch_at`] of exactly one mob.
fn hatch(scene: &mut Scene, slot: u8, kind: i32, at: BlockPos) -> EntityId {
    let (made, _) = hatch_at(scene, slot, kind, at);
    assert_eq!(made.len(), 1, "the egg makes one of kind {kind}");
    made[0]
}

fn record(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn the(scene: &Scene, key: EntityId) -> Entity {
    record(scene, key).unwrap_or_else(|| panic!("{key:?} is there"))
}

fn value(entity: &Entity, index: u8) -> Option<DataShown> {
    mob_values(entity.kind, &entity.data)
        .into_iter()
        .find(|(at, _, _)| *at == index)
        .map(|(_, value, _)| value)
}

fn health(entity: &Entity) -> f32 {
    match value(entity, HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("a health: {other:?}"),
    }
}

fn byte(entity: &Entity, index: u8) -> u8 {
    match value(entity, index) {
        Some(DataShown::Byte(byte)) => byte,
        other => panic!("a byte at {index}: {other:?}"),
    }
}

fn boolean(entity: &Entity, index: u8) -> bool {
    match value(entity, index) {
        Some(DataShown::Boolean(flag)) => flag,
        other => panic!("a boolean at {index}: {other:?}"),
    }
}

fn int(entity: &Entity, index: u8) -> i32 {
    match value(entity, index) {
        Some(DataShown::Int(number)) => number,
        other => panic!("an int at {index}: {other:?}"),
    }
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ------------------------------------------------------------------------------------
// Inputs.

/// A use of `key` with what the hand holds, not sneaking.
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

fn attack(key: EntityId) -> PlayerInput {
    PlayerInput::Attack {
        part: None,
        entity: key,
    }
}

/// A use of the held item in the air, looking along `yaw` and `pitch`.
fn use_item(scene: &mut Scene, yaw: f32, pitch: f32) -> TickOutput {
    scene.actors[0].sequence += 1;
    let sequence = scene.actors[0].sequence;
    scene.input(PlayerInput::UseItem {
        rotation: (yaw, pitch),
        sneaking: false,
        sequence,
    })
}

// ------------------------------------------------------------------------------------
// What the regions report.

/// The sounds of `sound` the tick plays, at a place or at an entity: (pitch, the player
/// left out, if any).
fn played(output: &TickOutput, sound: i32) -> Vec<(f32, Option<clustine_world::PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id,
                pitch,
                except,
                ..
            } if *id == sound => Some((*pitch, *except)),
            RegionEvent::EntitySound {
                sound: id, pitch, ..
            } if *id == sound => Some((*pitch, None)),
            _ => None,
        })
        .collect()
}

fn plays(output: &TickOutput, sound: i32) -> bool {
    !played(output, sound).is_empty()
}

/// The sources the tick plays `sound` from, at a place or at an entity.
fn sources(output: &TickOutput, sound: i32) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id, source, ..
            } if *id == sound => Some(*source),
            RegionEvent::EntitySound {
                sound: id,
                category,
                ..
            } if *id == sound => Some(i32::from(*category)),
            _ => None,
        })
        .collect()
}

/// `SoundSource.HOSTILE`'s ordinal, a monster's sound source (K 3.1).
const HOSTILE: i32 = 5;

/// The radii of the explosions the tick reports.
fn explosions(output: &TickOutput) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion { radius, .. } => Some(*radius),
            _ => None,
        })
        .collect()
}

/// The entity events of `key` in the tick.
fn entity_events(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityGone { entity, .. } | RegionEvent::EntityRemoved { entity, .. } if *entity == key)
    })
}

/// What clients were last told `key` wears and holds in the slot `slot`, over
/// `outputs`: what its spawn's record wears (an edge shows a mob it meets wearing what
/// its record says, `shown::mob_equipment`), then each `EquipmentChanged` of it; `None`
/// if they were never told of it.
fn shown_in(outputs: &[TickOutput], key: EntityId, slot: u8) -> Option<Option<ItemStack>> {
    let mut shown = None;
    for output in outputs {
        for event in &output.events {
            match event {
                RegionEvent::EntitySpawned(state) if state.entity == key => {
                    if let clustine_sim::api::EntityKind::Other { data, .. } = &state.kind {
                        shown = Some(
                            clustine_sim::shown::mob_equipment(data)
                                .into_iter()
                                .find(|(at, _)| *at == slot)
                                .map(|(_, stack)| stack),
                        );
                    }
                }
                RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == key => {
                    shown = Some(
                        slots
                            .iter()
                            .find(|(at, _)| *at == slot)
                            .and_then(|(_, stack)| stack.clone()),
                    );
                }
                _ => {}
            }
        }
    }
    shown
}

/// The modifiers clients were last shown of the attribute `attribute` of `key`.
fn modifiers_shown(
    output: &TickOutput,
    key: EntityId,
    attribute: i32,
) -> Option<Vec<(String, f64, u8)>> {
    output.events.iter().rev().find_map(|event| match event {
        RegionEvent::AttributesChanged {
            entity, attributes, ..
        } if *entity == key => attributes
            .iter()
            .find(|shown| i32::from(shown.attribute) == attribute)
            .map(|shown| shown.modifiers.clone()),
        _ => None,
    })
}

// ------------------------------------------------------------------------------------
// The creeper (K 11.3; the plan's 1.3 and section 10, "The creeper's explosion").

/// Where the creepers of these tests are hatched: within reach of player 1.
const CREEPER_AT: BlockPos = BlockPos::new(3, -60, 0);

/// A creeper hatched at night beside player 1, who holds its egg in slot 0 and
/// `igniter` in slot 1, which is selected.
fn creeper_scene(world: &World, igniter: i32) -> (Scene, EntityId) {
    let mut scene = night(world, &[items::CREEPER_SPAWN_EGG, igniter], SPAWN);
    let creeper = hatch(&mut scene, 0, entity_types::CREEPER, CREEPER_AT);
    scene.select(1);
    (scene, creeper)
}

/// Idles until an explosion is reported, at most `ticks` ticks: the tick it came in,
/// counted on from `counted` (the ticks already counted), and its output.
fn until_it_explodes(scene: &mut Scene, counted: u32, ticks: u32) -> (u32, TickOutput) {
    for tick in counted + 1..=counted + ticks {
        let output = scene.idle();
        if !explosions(&output).is_empty() {
            return (tick, output);
        }
    }
    panic!("nothing exploded in {ticks} ticks");
}

/// K 11.3: flint and steel used on a creeper ignites it. From the tick of the use it
/// swells one a tick, `creeper_primed` at its first swell, and on the 30th tick, the
/// use's the first, it explodes where it stands, of radius 3; it is discarded, not
/// killed: no death, no loot, no experience. The item's sound plays at the creeper for
/// all but the player who used it, and the ignition is shown.
#[test]
fn a_creeper_lit_with_flint_and_steel_explodes_on_the_thirtieth_tick_and_leaves_nothing_of_itself()
{
    let (mut scene, creeper) = creeper_scene(&World::default(), items::FLINT_AND_STEEL);
    assert!(!boolean(&the(&scene, creeper), IGNITED));
    let lit = scene.input(used(creeper));
    let sound = played(&lit, sounds::ITEM_FLINTANDSTEEL_USE);
    assert_eq!(sound.len(), 1, "{:?}", lit.events);
    let (pitch, except) = sound[0];
    assert!(
        (0.8..1.2).contains(&pitch),
        "the creeper's own draw, 0.8 + 0.4 x: {pitch}"
    );
    assert_eq!(
        except,
        Some(scene.actors[0].id),
        "the user's client plays it"
    );
    assert_eq!(
        sources(&lit, sounds::ITEM_FLINTANDSTEEL_USE),
        [HOSTILE],
        "from the creeper's source"
    );
    let shown = the(&scene, creeper);
    assert!(boolean(&shown, IGNITED), "the ignition is synced");
    assert!(
        plays(&lit, sounds::ENTITY_CREEPER_PRIMED),
        "its first swell is in the use's tick"
    );
    assert!(explosions(&lit).is_empty());
    let mut ticks = 1;
    let exploded = loop {
        let output = scene.idle();
        ticks += 1;
        assert!(
            !plays(&output, sounds::ENTITY_CREEPER_PRIMED),
            "primed once, at the first swell: tick {ticks}"
        );
        if !explosions(&output).is_empty() {
            break output;
        }
        assert!(ticks < 60, "it exploded");
        // Its tick sets the direction to 1 and swells, then `SwellGoal`, which has no
        // target, sets it to -1 again: what the tick leaves is -1 (K 11.3).
        assert_eq!(int(&the(&scene, creeper), SWELL_DIR), -1, "tick {ticks}");
        assert!(boolean(&the(&scene, creeper), IGNITED), "tick {ticks}");
    };
    assert_eq!(ticks, FUSE, "the 30th tick, the use's the first");
    assert_eq!(explosions(&exploded), [CREEPER_RADIUS]);
    assert!(record(&scene, creeper).is_none(), "the creeper is gone");
    assert!(
        entity_events(&exploded, creeper).is_empty(),
        "discarded, with no death's event"
    );
    // Nothing of its own lies about: no gunpowder, no experience.
    scene.idle();
    let all = scene.all();
    assert_eq!(lying_count(&all, items::GUNPOWDER), 0);
    assert!(
        all.iter()
            .all(|entity| !matches!(entity.data, EntityData::ExperienceOrb { .. })),
        "no experience"
    );
    // A creative player keeps the flint and steel.
    assert_eq!(scene.holds(1), Some(one(items::FLINT_AND_STEEL)));
}

/// K 11.3: `ExplosionInteraction.MOB` breaks blocks, `mob_griefing` being on (the plan's
/// section 10), and their drops decay: of the stone it breaks, one block in three, the
/// radius, drops its item. The floor about the creeper is of dirt, which a radius of 3
/// breaks widely (stone it breaks only under its feet, the rays losing too much in it).
#[test]
fn a_creeper_s_explosion_breaks_the_floor_and_most_of_what_it_breaks_drops_nothing() {
    let mut dirt = Vec::new();
    for x in -2..=9 {
        for z in -6..=6 {
            dirt.push((bp(x, FLOOR as i32 - 1, z), blocks::DIRT));
        }
    }
    let (mut scene, creeper) = creeper_scene(&World::of(&dirt), items::FLINT_AND_STEEL);
    let stood = the(&scene, creeper).position;
    scene.input(used(creeper));
    until_it_explodes(&mut scene, 1, 40);
    let under = bp(
        stood[0].floor() as i32,
        FLOOR as i32 - 1,
        stood[2].floor() as i32,
    );
    assert_eq!(scene.block(under), blocks::AIR, "the floor it stood on");
    let mut broken = 0;
    for x in under.x - 4..=under.x + 4 {
        for z in under.z - 4..=under.z + 4 {
            if scene.block(bp(x, under.y, z)) == blocks::AIR {
                broken += 1;
            }
        }
    }
    assert!(broken >= 6, "a hole in the floor: {broken}");
    scene.idle();
    let dropped = lying_count(&scene.all(), items::DIRT);
    assert!(
        dropped < broken,
        "the drops decay: {dropped} of {broken} blocks"
    );
}

/// K 11.3: a fire charge is a creeper igniter too, with its own sound; a creative player
/// keeps it.
#[test]
fn a_fire_charge_lights_a_creeper_and_a_creative_player_keeps_it() {
    let (mut scene, creeper) = creeper_scene(&World::default(), items::FIRE_CHARGE);
    let lit = scene.input(used(creeper));
    assert_eq!(played(&lit, sounds::ITEM_FIRECHARGE_USE).len(), 1);
    assert!(!plays(&lit, sounds::ITEM_FLINTANDSTEEL_USE));
    assert!(boolean(&the(&scene, creeper), IGNITED));
    assert_eq!(scene.holds(1), Some(one(items::FIRE_CHARGE)));
    let (tick, _) = until_it_explodes(&mut scene, 1, 40);
    assert_eq!(tick, FUSE);
}

/// K 11.3 and the plan's 1.3: with no target a creeper never swells unless lit, and
/// anything else in the hand does not light it.
#[test]
fn a_creeper_left_alone_or_used_with_anything_else_never_swells() {
    let (mut scene, creeper) = creeper_scene(&World::default(), items::SNOWBALL);
    let output = scene.input(used(creeper));
    assert!(!plays(&output, sounds::ENTITY_CREEPER_PRIMED));
    for tick in 0..300 {
        let output = scene.idle();
        assert!(explosions(&output).is_empty(), "tick {tick}");
        assert!(
            !plays(&output, sounds::ENTITY_CREEPER_PRIMED),
            "tick {tick}"
        );
    }
    let shown = the(&scene, creeper);
    assert!(!boolean(&shown, IGNITED));
    assert_eq!(int(&shown, SWELL_DIR), -1);
}

// ------------------------------------------------------------------------------------
// A creeper at a pinned line.

/// A dirt floor from x = 10 to 21 and z = 0 to 9, on both sides of the line at x = 16.
fn dirt_by_the_line() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut dirt = Vec::new();
    for x in 10..=21 {
        for z in 0..=9 {
            dirt.push((bp(x, FLOOR as i32 - 1, z), blocks::DIRT));
        }
    }
    dirt
}

/// A side of the line at x = 16 over `world`, player `n` at `feet` holding `held`.
fn side(west: bool, world: &World, held: &[i32], player_at: Option<(u128, Vec3)>) -> Scene {
    let layout = Layout::side(west);
    let inventory = holding(held);
    let mut scene = Scene {
        region: region_of(&layout, world, inventory.clone()),
        layout,
        inventory,
        actors: Vec::new(),
    };
    if let Some((n, feet)) = player_at {
        scene.join(n, feet);
    }
    scene
}

/// The blocks a tick's effects break beyond the line, with the explosion they carry.
fn exploded_effects(output: &TickOutput) -> Vec<(BlockPos, clustine_world::Blast)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::Exploded { blast, .. } => Some((effect.at, blast)),
            _ => None,
        })
        .collect()
}

/// `effects` filed for `region` as the store files them, numbered `sequence`, taken in
/// one tick: its output.
fn file(region: &mut clustine_sim::Region, sequence: u64, effects: &[Effect]) -> TickOutput {
    let filed = effects
        .iter()
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
    region.tick(&TickInputs {
        effects: filed,
        ..TickInputs::default()
    })
}

/// The plan's section 10, "The creeper's explosion": at a pinned line a creeper's
/// explosion is TNT's: worked out whole in the creeper's region, the blocks beyond the
/// line broken by effects that carry it, here a radius of 3, the creeper its source and
/// its drops decaying (`ExplosionInteraction.MOB`), no player its owner; the copy is not
/// written. The holder breaks them and lays fewer drops than blocks.
#[test]
fn a_creeper_s_explosion_at_a_pinned_line_breaks_the_far_blocks_by_effects_whose_drops_decay() {
    let world = World::of(&dirt_by_the_line());
    let feet = Vec3::new(12.5, FLOOR, 4.5);
    let mut west = side(
        true,
        &world,
        &[items::CREEPER_SPAWN_EGG, items::FLINT_AND_STEEL],
        Some((1, feet)),
    );
    let creeper = hatch(&mut west, 0, entity_types::CREEPER, bp(15, -60, 4));
    west.select(1);
    west.input(used(creeper));
    let (_, output) = until_it_explodes(&mut west, 1, 40);
    assert_eq!(explosions(&output), [CREEPER_RADIUS]);
    let far = exploded_effects(&output);
    assert!(!far.is_empty(), "{:?}", output.effects);
    for (at, blast) in &far {
        assert!(at.x >= 16, "only the far side's blocks are effects: {at:?}");
        assert_eq!(blast.source, creeper);
        assert_eq!(blast.radius.0, CREEPER_RADIUS);
        assert!(blast.decay, "a mob's explosion's drops decay");
        assert!(blast.direct, "the creeper is its direct source");
        assert_ne!(blast.owner, Some(west.actors[0].id.0), "nobody primed it");
        assert!(!blast.owner_creative);
        assert_eq!(
            block_at(&west.region, *at),
            blocks::DIRT,
            "the copy is not written: {at:?}"
        );
    }
    assert!(
        far.iter().any(|(at, _)| *at == bp(16, -61, 4)),
        "the dirt beside the line: {far:?}"
    );
    // The holder takes them: its dirt goes, and its drops decay.
    let mut east = side(false, &world, &[], None);
    let effects: Vec<Effect> = output
        .effects
        .iter()
        .filter(|effect| matches!(effect.what, Effected::Exploded { .. }))
        .cloned()
        .collect();
    file(&mut east.region, 50, &effects);
    east.idle();
    for (at, _) in &far {
        assert_eq!(
            east.block(*at),
            blocks::AIR,
            "{at:?} is broken by its holder"
        );
    }
    let dropped = lying_count(&all_of(&east.region), items::DIRT);
    assert!(
        usize::try_from(dropped).expect("not negative") < far.len(),
        "the holder's drops decay: {dropped} of {}",
        far.len()
    );
}

/// A creeper beyond the line, as its egg made it: hatched in a scratch region and set
/// into the world as entity 1 at `at`.
fn creeper_record(at: [f64; 3]) -> Entity {
    let mut scratch = night(&World::default(), &[items::CREEPER_SPAWN_EGG], SPAWN);
    let made = hatch(&mut scratch, 0, entity_types::CREEPER, bp(3, -60, 3));
    let mut entity = the(&scratch, made);
    entity.key = key(1);
    entity.uuid = uuid::Uuid::from_u128(7_001);
    entity.position = at;
    entity.motion = [0.0; 3];
    entity.on_ground = true;
    entity
}

/// K 11.3 and ADR-0028: a player who lights a creeper across the line uses it through
/// its holder, which ignites it: it is shown ignited and explodes 30 ticks on, the
/// tick it was lit in the first.
#[test]
fn a_creeper_lit_across_a_pinned_line_is_ignited_by_its_holder_and_explodes_thirty_ticks_on() {
    let world = World::default().with_entities(&[creeper_record([17.5, FLOOR, 4.5])]);
    let mut west = side(
        true,
        &world,
        &[items::FLINT_AND_STEEL],
        Some((1, Vec3::new(14.5, FLOOR, 4.5))),
    );
    let mut east = side(false, &world, &[], None);
    let output = west.input(used(key(1)));
    let uses: Vec<Effect> = output
        .effects
        .iter()
        .filter(|effect| {
            matches!(effect.what, Effected::OnEntity { key: used, by: EntityUse::Interact { .. } } if used == key(1))
        })
        .cloned()
        .collect();
    assert_eq!(
        uses.len(),
        1,
        "the use goes to the holder: {:?}",
        output.effects
    );
    let lit = file(&mut east.region, 60, &uses);
    let mut ticks = 1;
    let mut primed = plays(&lit, sounds::ENTITY_CREEPER_PRIMED);
    assert!(
        boolean(&the(&east, key(1)), IGNITED),
        "ignited in the tick the use is taken"
    );
    let exploded = loop {
        let output = east.idle();
        ticks += 1;
        primed |= plays(&output, sounds::ENTITY_CREEPER_PRIMED);
        if !explosions(&output).is_empty() {
            break output;
        }
        assert!(ticks < 60, "it exploded");
    };
    assert!(primed);
    assert_eq!(ticks, FUSE, "the 30th tick, the taking's the first");
    assert_eq!(explosions(&exploded), [CREEPER_RADIUS]);
    assert!(record(&east, key(1)).is_none());
}

// ------------------------------------------------------------------------------------
// Mobs set into a world as their eggs make them.

/// A mob of `kind` as the egg `egg` makes it at night, hatched in a scratch region: its
/// record, to be set into a test's world with [`placed`].
fn made(egg: i32, kind: i32) -> Entity {
    let mut scratch = night(&World::default(), &[egg], SPAWN);
    let key = hatch(&mut scratch, 0, kind, bp(3, -60, 3));
    the(&scratch, key)
}

/// `record` as entity `n` standing at `at`, at rest, burning for `fire_ticks`.
fn placed(record: &Entity, n: u32, at: [f64; 3], fire_ticks: i32) -> Entity {
    let mut entity = record.clone();
    entity.key = key(n);
    entity.uuid = uuid::Uuid::from_u128(7_000 + u128::from(n));
    entity.position = at;
    entity.motion = [0.0; 3];
    entity.on_ground = true;
    entity.fall_distance = 0.0;
    entity.fire_ticks = fire_ticks;
    entity
}

/// A region alone at night over `world`, with nobody in it: its mobs act on their own.
fn empty_night(world: &World) -> Scene {
    let clock = WorldClock {
        game_time: 18_000,
        day: 18_000,
    };
    let layout = Layout::alone();
    Scene {
        region: region_at(&layout, world, inventory(&[]), Some(clock)),
        layout,
        inventory: inventory(&[]),
        actors: Vec::new(),
    }
}

fn position_of(scene: &Scene, key: EntityId) -> [f64; 3] {
    the(scene, key).position
}

/// Whether the mob of `egg` and `kind` runs from the one of `other_egg` and `other_kind`
/// set three blocks west of it, in a region with nobody in it: in ten ticks it moves
/// east, away, by half a block more than it does with the other twelve blocks west, out
/// of its sight of six; and it is soon more than six blocks from the other.
fn runs_from(egg: i32, kind: i32, other_egg: i32, other_kind: i32) {
    let mob = placed(&made(egg, kind), 1, [8.5, FLOOR, 8.5], 0);
    let other = made(other_egg, other_kind);
    let east_after_ten = |west: f64| -> (Scene, f64) {
        let world = World::default()
            .with_entities(&[mob.clone(), placed(&other, 2, [8.5 - west, FLOOR, 8.5], 0)]);
        let mut scene = empty_night(&world);
        scene.idle_for(10);
        let east = position_of(&scene, key(1))[0] - mob.position[0];
        (scene, east)
    };
    let (_, far) = east_after_ten(12.0);
    let (mut scene, near) = east_after_ten(3.0);
    assert!(
        near > far + 0.5,
        "it moves away at once: {near} east, against {far} with the other far"
    );
    let mut seen = Vec::new();
    let away = scene.idle_until(100, &mut seen, |scene, _| {
        horizontal(position_of(scene, key(1)), position_of(scene, key(2))) > 6.0
    });
    assert!(away, "it is more than six from the other");
}

/// K 11.3: a creeper runs from a cat within six blocks (`AvoidEntityGoal(Cat, 6.0, 1.0,
/// 1.2)`), whatever its target, and from an ocelot likewise.
#[test]
fn a_creeper_runs_from_a_cat_or_an_ocelot_near_it() {
    runs_from(
        items::CREEPER_SPAWN_EGG,
        entity_types::CREEPER,
        items::CAT_SPAWN_EGG,
        entity_types::CAT,
    );
    runs_from(
        items::CREEPER_SPAWN_EGG,
        entity_types::CREEPER,
        items::OCELOT_SPAWN_EGG,
        entity_types::OCELOT,
    );
}

/// K 11.4: a spider runs from an armadillo that is not rolled up
/// (`AvoidEntityGoal(Armadillo, 6.0, 1.0, 1.2, not scared)`); the plan's 1.3.
#[test]
fn a_spider_runs_from_an_armadillo_that_is_not_rolled_up() {
    runs_from(
        items::SPIDER_SPAWN_EGG,
        entity_types::SPIDER,
        items::ARMADILLO_SPAWN_EGG,
        entity_types::ARMADILLO,
    );
}

// ------------------------------------------------------------------------------------
// The spiders (K 11.4, 11.12).

/// A pit three blocks across and three deep, walled in stone, its floor the world's at
/// y = -61, its middle at x, z = 7.5.
fn pit() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut walls = Vec::new();
    for x in 5..=9 {
        for z in 5..=9 {
            if (6..=8).contains(&x) && (6..=8).contains(&z) {
                continue;
            }
            for y in -60..=-58 {
                walls.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    walls
}

/// K 11.4: a spider climbs: its flag is whether its last move ran into something
/// sideways, and with it set it rises up a wall. So it gets out of a pit three deep,
/// shown climbing as it does, where a creeper, which does not climb, stays in.
#[test]
fn a_spider_climbs_out_of_a_pit_a_creeper_cannot_leave() {
    let out = |at: [f64; 3]| at[1] >= -57.0;
    for (egg, kind) in [
        (items::SPIDER_SPAWN_EGG, entity_types::SPIDER),
        (items::CREEPER_SPAWN_EGG, entity_types::CREEPER),
    ] {
        let mob = made(egg, kind);
        let world = World::of(&pit()).with_entities(&[placed(&mob, 1, [7.5, FLOOR, 7.5], 0)]);
        let mut scene = empty_night(&world);
        let mut left = false;
        let mut climbing = false;
        for _ in 0..1200 {
            scene.idle();
            let now = the(&scene, key(1));
            left |= out(now.position);
            if kind == entity_types::SPIDER {
                climbing |= byte(&now, SPIDER_FLAGS) & CLIMBING != 0;
            }
            if left {
                break;
            }
        }
        if kind == entity_types::SPIDER {
            assert!(climbing, "it was shown climbing");
            assert!(left, "the spider is out");
        } else {
            assert!(!left, "a creeper stays in the pit");
        }
    }
}

/// Positions about player 1 at the spawn, within reach: the 24 blocks of the floor of
/// the five by five about them but theirs, as many times over as `count` needs.
fn about_the_spawn(count: usize) -> Vec<BlockPos> {
    let ring: Vec<BlockPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| bp(x, -60, z)))
        .filter(|at| (at.x, at.z) != (0, 0))
        .collect();
    ring.iter().copied().cycle().take(count).collect()
}

/// Player 1 uses the egg in `slot` once on the floor at each of `at`, all in one tick:
/// the keys of the mobs of `kind` made, ascending, which is the order they were made.
fn hatch_many(scene: &mut Scene, slot: u8, kind: i32, at: &[BlockPos]) -> Vec<EntityId> {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    let mut inputs = Vec::new();
    for position in at {
        scene.actors[0].sequence += 1;
        inputs.push(PlayerInput::UseItemOn {
            position: bp(position.x, position.y - 1, position.z),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence: scene.actors[0].sequence,
        });
    }
    scene.inputs_of(0, inputs);
    let mut made: Vec<EntityId> = scene
        .of_kind(kind)
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    made.sort();
    made
}

/// K 11.12: a spider's egg draws the Tail, whose last draw makes one in twenty
/// left-handed; a cave spider's draws none, so none is ever left-handed.
#[test]
fn a_spider_is_now_and_then_left_handed_and_a_cave_spider_never() {
    let mut scene = night(
        &World::default(),
        &[items::SPIDER_SPAWN_EGG, items::CAVE_SPIDER_SPAWN_EGG],
        SPAWN,
    );
    let spiders = hatch_many(&mut scene, 0, entity_types::SPIDER, &about_the_spawn(200));
    assert_eq!(spiders.len(), 200);
    let left = spiders
        .iter()
        .filter(|key| byte(&the(&scene, **key), MOB_FLAGS) & LEFT_HANDED != 0)
        .count();
    assert!(
        (2..=25).contains(&left),
        "about one in twenty: {left} of 200"
    );
    let caves = hatch_many(
        &mut scene,
        1,
        entity_types::CAVE_SPIDER,
        &about_the_spawn(200),
    );
    assert_eq!(caves.len(), 200);
    for cave in &caves {
        assert_eq!(
            byte(&the(&scene, *cave), MOB_FLAGS) & LEFT_HANDED,
            0,
            "no cave spider is left-handed"
        );
    }
}

/// K 11.4 and 11.12: one spider's egg in a hundred (`nextInt(100) == 0`, at every
/// difficulty) makes a skeleton too, made a passenger of the spider, which the egg adds
/// with it; the skeleton holds its bow (K 11.2's `S`).
#[test]
fn now_and_then_a_spider_s_egg_makes_it_ridden_by_a_skeleton() {
    let mut scene = night(&World::default(), &[items::SPIDER_SPAWN_EGG], SPAWN);
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    let spiders = hatch_many(&mut scene, 0, entity_types::SPIDER, &about_the_spawn(600));
    assert_eq!(spiders.len(), 600);
    let skeletons: Vec<Entity> = scene
        .of_kind(entity_types::SKELETON)
        .into_iter()
        .filter(|entity| !before.contains(&entity.key))
        .collect();
    assert!(
        (1..=20).contains(&skeletons.len()),
        "about one in a hundred: {} of 600",
        skeletons.len()
    );
    for skeleton in &skeletons {
        let vehicle = skeleton.riding.vehicle.expect("it rides");
        assert!(spiders.contains(&vehicle), "it rides a spider of an egg");
        let spider = the(&scene, vehicle);
        assert_eq!(
            spider
                .riding
                .passengers
                .iter()
                .map(|seat| seat.key)
                .collect::<Vec<_>>(),
            [skeleton.key]
        );
    }
    let ridden = spiders
        .iter()
        .filter(|key| !the(&scene, **key).riding.passengers.is_empty())
        .count();
    assert_eq!(
        ridden,
        skeletons.len(),
        "every jockey's skeleton is its own"
    );
}

/// K 11.4 and 11.12: a cave spider's egg draws nothing of the caller's random but the
/// yaw `EntityType.spawn` gives every egg's mob: no Tail, no jockey, no effect. A
/// creeper's egg draws its yaw and the Tail, four in all. So five cave spiders' eggs used
/// in one tick give their yaws from the stream's first five draws, and a creeper's egg
/// and a cave spider's used in the same tick of a world made the same give the creeper
/// the first's yaw and the cave spider the fifth's.
#[test]
fn a_cave_spider_s_egg_draws_nothing_but_its_yaw() {
    let held = [items::CREEPER_SPAWN_EGG, items::CAVE_SPIDER_SPAWN_EGG];
    let mut five = night(&World::default(), &held, SPAWN);
    let caves = hatch_many(&mut five, 1, entity_types::CAVE_SPIDER, &about_the_spawn(5));
    let yaws: Vec<f32> = caves.iter().map(|key| the(&five, *key).yaw).collect();
    assert_eq!(yaws.len(), 5);
    assert!(
        yaws.windows(2).all(|pair| pair[0] != pair[1]),
        "each its own draw: {yaws:?}"
    );

    let mut mixed = night(&World::default(), &held, SPAWN);
    // The selection in a tick of its own, as `hatch_many`'s is, so that the eggs are used
    // in the same tick of the region in both.
    mixed.select(1);
    let before: Vec<EntityId> = mixed.all().iter().map(|entity| entity.key).collect();
    let positions = about_the_spawn(2);
    let mut inputs = Vec::new();
    for (slot, at) in [0_u8, 1].iter().zip(&positions) {
        inputs.push(PlayerInput::SelectSlot { slot: *slot });
        mixed.actors[0].sequence += 1;
        inputs.push(PlayerInput::UseItemOn {
            position: bp(at.x, at.y - 1, at.z),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence: mixed.actors[0].sequence,
        });
    }
    mixed.inputs_of(0, inputs);
    let new_of = |kind: i32| -> Entity {
        let found: Vec<Entity> = mixed
            .of_kind(kind)
            .into_iter()
            .filter(|entity| !before.contains(&entity.key))
            .collect();
        assert_eq!(found.len(), 1, "one of kind {kind}");
        found[0].clone()
    };
    assert_eq!(
        new_of(entity_types::CREEPER).yaw,
        yaws[0],
        "the creeper's yaw is the first draw"
    );
    assert_eq!(
        new_of(entity_types::CAVE_SPIDER).yaw,
        yaws[4],
        "after the creeper's four draws, the fifth"
    );
}

// ------------------------------------------------------------------------------------
// The witch (K 3.6; the plan's 1.3, section 10's "Mob effects" and section 11 item 4).

/// What a witch's drink showed.
struct Drink {
    /// The tick the drink began in, counted from 0 for the first idled.
    began: usize,
    /// The tick it ended in: the potion out of its hand.
    ended: usize,
    /// The witch's health at the end of each tick, from the first idled.
    health: Vec<f32>,
}

fn drinking_slowness(modifiers: &[(String, f64, u8)]) -> bool {
    modifiers.iter().any(|(id, amount, operation)| {
        id == "minecraft:drinking" && *amount == -0.25 && *operation == 0
    })
}

/// Idles `scene` up to `ticks` ticks, watching the witch `witch`, until a drink of it
/// ends: what it showed. Checks what the drink shows as it goes: the potion in its main
/// hand and its using flag through the drink, `witch_drink` at a pitch of 0.8 to 1.2 at
/// its start, the slowness of drinking (`minecraft:drinking`, -0.25 added) on its speed
/// while it lasts, and all of them gone at its end.
fn watch_a_drink(scene: &mut Scene, witch: EntityId, ticks: usize) -> Drink {
    let mut health = Vec::new();
    let mut began = None;
    for tick in 0..ticks {
        let output = scene.idle();
        let now = the(scene, witch);
        health.push(self::health(&now));
        let outputs = std::slice::from_ref(&output);
        let speed = modifiers_shown(&output, witch, clustine_data::attributes::MOVEMENT_SPEED);
        match began {
            None => {
                if plays(&output, sounds::ENTITY_WITCH_DRINK) {
                    let (pitch, _) = played(&output, sounds::ENTITY_WITCH_DRINK)[0];
                    assert!((0.8..1.2).contains(&pitch), "its own draw: {pitch}");
                    assert_eq!(
                        sources(&output, sounds::ENTITY_WITCH_DRINK),
                        [HOSTILE],
                        "a monster's source"
                    );
                    assert!(boolean(&now, USING_ITEM), "shown drinking");
                    assert_eq!(
                        shown_in(outputs, witch, MAIN_HAND),
                        Some(Some(one(items::POTION))),
                        "a potion in its hand"
                    );
                    let slowed = speed.expect("its speed is shown");
                    assert!(
                        drinking_slowness(&slowed),
                        "slowed while it drinks: {slowed:?}"
                    );
                    began = Some(tick);
                } else {
                    assert!(!boolean(&now, USING_ITEM), "tick {tick}");
                }
            }
            Some(start) => {
                assert!(
                    !plays(&output, sounds::ENTITY_WITCH_DRINK),
                    "one drink at a time"
                );
                if !boolean(&now, USING_ITEM) {
                    assert_eq!(
                        shown_in(outputs, witch, MAIN_HAND),
                        Some(None),
                        "its hand emptied at the drink's end"
                    );
                    let speed = speed.expect("its speed is shown again");
                    assert!(!drinking_slowness(&speed), "the slowness is off: {speed:?}");
                    return Drink {
                        began: start,
                        ended: tick,
                        health,
                    };
                }
                assert!(tick - start <= POTION_TICKS as usize + 1, "the drink ends");
            }
        }
    }
    panic!("no drink ended in {ticks} ticks");
}

/// K 3.6 and the plan's section 11 item 4: a witch that is hurt, on land and not
/// burning, draws in turn for water breathing, fire resistance and healing each tick,
/// and so before long drinks a potion of healing: the potion is in its hand for its use
/// duration of 32 ticks, the drink ending at the 33rd tick after it began (`usingTime--
/// <= 0`), and its healing of 4 (here up to its most) comes a tick after that, at the
/// next `tickEffects`.
#[test]
fn a_hurt_witch_drinks_healing_and_is_healed_the_tick_after_its_drink_ends() {
    let mut scene = night(&World::default(), &[items::WITCH_SPAWN_EGG], SPAWN);
    let witch = hatch(&mut scene, 0, entity_types::WITCH, bp(2, -60, 0));
    assert_eq!(health(&the(&scene, witch)), WITCH_HEALTH);
    scene.input(attack(witch));
    let hurt = health(&the(&scene, witch));
    assert!(hurt < WITCH_HEALTH, "the fist hurts it: {hurt}");
    let drink = watch_a_drink(&mut scene, witch, 2000);
    assert_eq!(
        drink.ended - drink.began,
        POTION_TICKS as usize + 1,
        "the drink's length"
    );
    assert_eq!(
        drink.health[drink.ended], hurt,
        "nothing healed as the drink ends"
    );
    scene.idle();
    let healed = health(&the(&scene, witch));
    assert_eq!(
        healed,
        (hurt + HEALING).min(WITCH_HEALTH),
        "healed the tick after"
    );
}

/// K 3.6: a witch at its full health and not burning, on land, needs no potion: it
/// draws and never drinks.
#[test]
fn a_whole_witch_on_land_never_drinks() {
    let mut scene = night(&World::default(), &[items::WITCH_SPAWN_EGG], SPAWN);
    let witch = hatch(&mut scene, 0, entity_types::WITCH, bp(2, -60, 0));
    for tick in 0..600 {
        let output = scene.idle();
        assert!(!plays(&output, sounds::ENTITY_WITCH_DRINK), "tick {tick}");
        assert!(!boolean(&the(&scene, witch), USING_ITEM), "tick {tick}");
    }
}

/// K 3.6 and the plan's section 10: a burning witch drinks fire resistance (its second
/// draw passes about once in seven ticks), and from then on fire hurts it no more while
/// it burns; before it, fire hurts it, a point a second. Whatever potions it drinks
/// (healing too, once hurt), its health falls before its first drink and never in the
/// long burning after its drinks are done.
#[test]
fn a_burning_witch_drinks_fire_resistance_and_fire_hurts_it_no_more() {
    let witch = placed(
        &made(items::WITCH_SPAWN_EGG, entity_types::WITCH),
        1,
        [8.5, FLOOR, 8.5],
        3000,
    );
    let mut scene = empty_night(&World::default().with_entities(&[witch]));
    let mut health = vec![WITCH_HEALTH];
    let mut first_drink = None;
    let mut last_end = None;
    let mut was_drinking = false;
    for tick in 1..=1600 {
        let output = scene.idle();
        let now = the(&scene, key(1));
        health.push(self::health(&now));
        if plays(&output, sounds::ENTITY_WITCH_DRINK) {
            first_drink.get_or_insert(tick);
        }
        let drinking = boolean(&now, USING_ITEM);
        if was_drinking && !drinking {
            last_end = Some(tick);
        }
        was_drinking = drinking;
    }
    assert!(the(&scene, key(1)).fire_ticks > 0, "it burns still");
    let first = first_drink.expect("it drank");
    let end = last_end.expect("its drinks ended");
    assert!(end < 1000, "its drinks were done early: {end}");
    assert!(
        health[..=first].windows(2).any(|pair| pair[1] < pair[0]),
        "fire hurt it before it drank: {:?}",
        &health[..=first]
    );
    for tick in end + 2..health.len() {
        assert!(
            health[tick] >= health[tick - 1],
            "fire hurt it after its drinks, tick {tick}: {} to {}",
            health[tick - 1],
            health[tick]
        );
    }
}

// ------------------------------------------------------------------------------------
// The illagers (K 3.1, 3.7; K 5's rows; the plan's 1.3 and section 10, "Items without
// components").

/// K 3.7 and K 4: each raider's egg makes it holding what the game's holds: a pillager
/// its crossbow, a vindicator its iron axe (outside a raid), an evoker and a witch
/// nothing. Clients are told what it holds (`SetEquipment` in the game's pairing), which
/// in Clustine's interface is what its spawn's record wears, or an `EquipmentChanged`.
#[test]
fn each_raider_s_egg_makes_it_holding_what_the_game_s_holds_and_shows_it() {
    for (egg, kind, held) in [
        (
            items::PILLAGER_SPAWN_EGG,
            entity_types::PILLAGER,
            Some(one(items::CROSSBOW)),
        ),
        (
            items::VINDICATOR_SPAWN_EGG,
            entity_types::VINDICATOR,
            Some(one(items::IRON_AXE)),
        ),
        (items::EVOKER_SPAWN_EGG, entity_types::EVOKER, None),
        (items::WITCH_SPAWN_EGG, entity_types::WITCH, None),
    ] {
        let mut scene = night(&World::default(), &[egg], SPAWN);
        let (made, output) = hatch_at(&mut scene, 0, kind, bp(2, -60, 2));
        assert_eq!(made.len(), 1, "{kind}");
        let next = scene.idle();
        let outputs = [output, next];
        let shown = shown_in(&outputs, made[0], MAIN_HAND).flatten();
        assert_eq!(shown, held, "what clients are shown it holds: {kind}");
    }
}

/// The banner a patrol leader wears: the ominous banner, a white banner with its
/// patterns and name (N9's components).
const LEADER_BANNER: i32 = items::WHITE_BANNER;

/// K 3.1 and K 5: a raider's egg (`SPAWN_ITEM_USE`, neither a patrol, an event nor a
/// structure) draws `nextFloat() < 0.06` for a patrol leader, who wears the ominous
/// banner on its head: about six pillagers, vindicators and evokers in a hundred. A
/// witch draws it too and cannot lead (`canBeLeader` false): none wears a banner.
#[test]
fn about_six_raiders_in_a_hundred_from_eggs_lead_a_patrol_with_a_banner_and_no_witch() {
    for (egg, kind, can_lead) in [
        (items::PILLAGER_SPAWN_EGG, entity_types::PILLAGER, true),
        (items::VINDICATOR_SPAWN_EGG, entity_types::VINDICATOR, true),
        (items::EVOKER_SPAWN_EGG, entity_types::EVOKER, true),
        (items::WITCH_SPAWN_EGG, entity_types::WITCH, false),
    ] {
        let mut scene = night(&World::default(), &[egg], SPAWN);
        let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
        scene.select(0);
        let mut inputs = Vec::new();
        for at in about_the_spawn(300) {
            scene.actors[0].sequence += 1;
            inputs.push(PlayerInput::UseItemOn {
                position: bp(at.x, at.y - 1, at.z),
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence: scene.actors[0].sequence,
            });
        }
        let hatched = scene.inputs_of(0, inputs);
        let next = scene.idle();
        let outputs = [hatched, next];
        let made: Vec<EntityId> = scene
            .of_kind(kind)
            .iter()
            .map(|entity| entity.key)
            .filter(|key| !before.contains(key))
            .collect();
        assert_eq!(made.len(), 300, "{kind}");
        let leaders = made
            .iter()
            .filter(|key| {
                shown_in(&outputs, **key, HEAD)
                    .flatten()
                    .is_some_and(|stack| {
                        stack.item == LEADER_BANNER && !stack.components.is_empty()
                    })
            })
            .count();
        if can_lead {
            assert!(
                (6..=36).contains(&leaders),
                "about six in a hundred of kind {kind}: {leaders} of 300"
            );
        } else {
            assert_eq!(leaders, 0, "no witch leads");
        }
    }
}

// ------------------------------------------------------------------------------------
// The guardians (K 3.8; the plan's 1.3).

/// A pool of water from x, z = 4 to 11 and y = -60 to -56, walled in stone two blocks
/// higher than its surface.
fn pool() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut blocks = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            let wall = x == 3 || x == 12 || z == 3 || z == 12;
            for y in -60..=-54 {
                if wall {
                    blocks.push((bp(x, y, z), blocks::STONE));
                } else if y <= -56 {
                    blocks.push((bp(x, y, z), blocks::WATER));
                }
            }
        }
    }
    blocks
}

/// K 3.8: a guardian on land flops: each tick it is on the ground its motion gains 0.5
/// up and a random push sideways, and it turns to a random yaw. So it is soon off the
/// ground and turned.
#[test]
fn a_guardian_on_land_flops_up_and_about() {
    let guardian = placed(
        &made(items::GUARDIAN_SPAWN_EGG, entity_types::GUARDIAN),
        1,
        [8.5, FLOOR, 8.5],
        0,
    );
    let mut scene = empty_night(&World::default().with_entities(std::slice::from_ref(&guardian)));
    let mut highest = FLOOR;
    let mut yaws = Vec::new();
    for _ in 0..40 {
        scene.idle();
        let now = the(&scene, key(1));
        highest = highest.max(now.position[1]);
        yaws.push(now.yaw);
    }
    assert!(highest > FLOOR + 0.5, "it flops up: {highest}");
    yaws.dedup();
    assert!(yaws.len() >= 3, "it turns about as it flops: {yaws:?}");
}

/// K 3.8: a guardian in water does not flop: it stays in the water, never leaping
/// above its surface.
#[test]
fn a_guardian_in_water_does_not_flop() {
    let guardian = placed(
        &made(items::GUARDIAN_SPAWN_EGG, entity_types::GUARDIAN),
        1,
        [7.5, -59.0, 7.5],
        0,
    );
    let world = World::of(&pool()).with_entities(&[guardian]);
    let mut scene = empty_night(&world);
    for tick in 0..400 {
        scene.idle();
        let now = the(&scene, key(1));
        assert!(
            now.position[1] < -55.0,
            "it stays in the water, tick {tick}: {:?}",
            now.position
        );
    }
}

/// K 3.8 and the plan's 1.3: a guardian that keeps still hurts its living attacker by
/// two of thorns, but a creative player is not hurt (none of Clustine's players is in
/// survival); the guardian is hurt as ever.
#[test]
fn a_still_guardian_s_spikes_hurt_no_creative_player_who_hits_it() {
    let mut scene = night(&World::default(), &[items::GUARDIAN_SPAWN_EGG], SPAWN);
    let guardian = hatch(&mut scene, 0, entity_types::GUARDIAN, bp(2, -60, 0));
    let whole = health(&the(&scene, guardian));
    let output = scene.input(attack(guardian));
    let me = scene.me();
    let damaged: Vec<EntityId> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::DamageEvent { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    assert!(damaged.contains(&guardian), "the guardian is hurt");
    assert!(
        !damaged.contains(&me),
        "the creative player is not: {damaged:?}"
    );
    assert!(health(&the(&scene, guardian)) < whole);
    let after = scene.idle();
    assert!(
        after.events.iter().all(
            |event| !matches!(event, RegionEvent::DamageEvent { entity, .. } if *entity == me)
        ),
        "nor in the tick after"
    );
}

/// K 3.8 and the plan's 1.3: the elder guardian curses the survival players within 50
/// every 1200 ticks; it finds no creative player, and a creative player beside it for
/// longer than that is told nothing of a curse (no event of theirs at all, while they
/// stand still).
#[test]
fn an_elder_guardian_s_curse_finds_no_creative_player() {
    let elder = placed(
        &made(
            items::ELDER_GUARDIAN_SPAWN_EGG,
            entity_types::ELDER_GUARDIAN,
        ),
        1,
        [7.5, -59.0, 7.5],
        0,
    );
    let world = World::of(&pool()).with_entities(&[elder]);
    let mut scene = night(&world, &[], Vec3::new(1.5, FLOOR, 1.5));
    let id = scene.actors[0].id;
    for tick in 0..1300 {
        let output = scene.idle();
        assert!(
            events_of(&output, id).is_empty(),
            "tick {tick}: {:?}",
            events_of(&output, id)
        );
    }
    assert!(record(&scene, key(1)).is_some(), "the elder is there");
}

// ------------------------------------------------------------------------------------
// The difficulty (the plan's 1.4, section 10's "The difficulty" and section 11 item 5).

/// The world's difficulty is easy, the game's dedicated server's default, of id 1, and
/// the only one accepted. (That clients are sent it at login, unlocked, is the edge's,
/// out of this crate's reach.)
#[test]
fn the_world_s_difficulty_is_easy() {
    assert_eq!(
        clustine_world::Difficulty::WORLD,
        clustine_world::Difficulty::Easy
    );
    assert_eq!(clustine_world::Difficulty::WORLD.id(), 1);
}

// ------------------------------------------------------------------------------------
// The snowball a player throws (K 11.10; the plan's section 10, "The snowball").

/// `0.99f` widened, the air's drag on a thrown item.
fn drag() -> f64 {
    f64::from(0.99_f32)
}

/// The snowballs about.
fn snowballs(scene: &Scene) -> Vec<Entity> {
    scene.of_kind(entity_types::SNOWBALL)
}

fn the_snowball(scene: &Scene) -> Entity {
    let found = snowballs(scene);
    assert_eq!(found.len(), 1, "one snowball: {found:?}");
    found[0].clone()
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// K 11.10 and `SnowballItem.use`: a player's snowball is thrown from their eye less
/// 0.1, along their look at a power of 1.5 with a little spread, `snowball_throw` at
/// their feet at volume 0.5 and a pitch of 0.4 / (0.8 to 1.2); it is theirs, and a
/// creative player keeps the snowball. It flies from the tick of the throw: each tick
/// gravity takes 0.03 from its motion, the air keeps 0.99 of it, and it moves by it.
#[test]
fn a_player_s_snowball_flies_along_their_look_falling_and_slowing_and_a_creative_player_keeps_it() {
    let mut scene = night(&World::default(), &[items::SNOWBALL], SPAWN);
    let output = use_item(&mut scene, 0.0, 0.0);
    let throws: Vec<(Vec3, f32, f32)> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                position,
                volume,
                pitch,
                ..
            } if *sound == sounds::ENTITY_SNOWBALL_THROW => Some((*position, *volume, *pitch)),
            _ => None,
        })
        .collect();
    assert_eq!(throws.len(), 1, "{:?}", output.events);
    let (at, volume, pitch) = throws[0];
    assert_eq!(at, SPAWN, "at the thrower's feet");
    assert_eq!(volume, THROW_VOLUME);
    assert!(
        pitch > 0.4 / 1.2 && pitch <= 0.5,
        "0.4 / (0.8 + 0.4 x): {pitch}"
    );
    assert_eq!(scene.holds(0), Some(one(items::SNOWBALL)), "kept");

    let thrown = the_snowball(&scene);
    match &thrown.data {
        EntityData::Thrown { stack, owner, .. } => {
            assert_eq!(*stack, one(items::SNOWBALL));
            assert_eq!(*owner, Some(scene.me()), "the thrower's");
        }
        other => panic!("a thrown item: {other:?}"),
    }
    // It flew once in its throw's tick: from the eye less 0.1 by its motion.
    let start = [SPAWN.x, SPAWN.y + 1.62 - 0.1, SPAWN.z];
    for (axis, from) in start.iter().enumerate() {
        assert!(
            (thrown.position[axis] - (from + thrown.motion[axis])).abs() < 1e-6,
            "from the eye by its motion: {:?} and {:?}",
            thrown.position,
            thrown.motion
        );
    }
    // Its shot, before that tick's gravity and drag, is of about 1.5 along the look
    // (south, yaw 0, level).
    let shot = [
        thrown.motion[0] / drag(),
        thrown.motion[1] / drag() + GRAVITY,
        thrown.motion[2] / drag(),
    ];
    assert!(
        (length(shot) - THROW_POWER).abs() < 0.1,
        "a power of 1.5: {shot:?}"
    );
    assert!(shot[2] > 1.4, "along the look: {shot:?}");
    let mut position = thrown.position;
    let mut motion = thrown.motion;
    for tick in 0..5 {
        scene.idle();
        motion = [
            motion[0] * drag(),
            (motion[1] - GRAVITY) * drag(),
            motion[2] * drag(),
        ];
        position = [
            position[0] + motion[0],
            position[1] + motion[1],
            position[2] + motion[2],
        ];
        let now = the_snowball(&scene);
        for axis in 0..3 {
            assert!(
                (now.motion[axis] - motion[axis]).abs() < 1e-9,
                "tick {tick}: {:?} against {motion:?}",
                now.motion
            );
            assert!(
                (now.position[axis] - position[axis]).abs() < 1e-9,
                "tick {tick}: {:?} against {position:?}",
                now.position
            );
        }
    }
}

/// K 11.10: a snowball that hits a mob hurts it by nothing, but the hurt is taken and
/// knocks it back along the snowball's motion and up; at the hit the snowball shows its
/// particles (entity event 3) and is gone.
#[test]
fn a_player_s_snowball_hurts_a_cow_by_nothing_and_pushes_it_away_and_is_gone() {
    let mut scene = night(
        &World::default(),
        &[items::COW_SPAWN_EGG, items::SNOWBALL],
        SPAWN,
    );
    let cow = hatch(&mut scene, 0, entity_types::COW, bp(0, -60, 4));
    let whole = health(&the(&scene, cow));
    scene.select(1);
    // Looking south and a little down, at the cow's middle.
    use_item(&mut scene, 0.0, 10.0);
    let snowball = the_snowball(&scene).key;
    let mut seen = Vec::new();
    let hit = scene.idle_until(20, &mut seen, |_, seen| {
        seen.last()
            .is_some_and(|output| entity_events(output, snowball).contains(&SNOWBALL_HIT))
    });
    assert!(hit, "it hit something");
    let output = seen.last().expect("a tick");
    assert!(gone(output, snowball), "it is gone at the hit");
    assert!(snowballs(&scene).is_empty());
    let damaged = output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::DamageEvent { entity, .. } if *entity == cow));
    assert!(damaged, "the cow took the hurt");
    let now = the(&scene, cow);
    assert_eq!(health(&now), whole, "of nothing");
    assert!(
        now.motion[2] > 0.1 && now.motion[1] > 0.0,
        "pushed south, away from the thrower, and up: {:?}",
        now.motion
    );
}

/// The plan's section 10: a snowball's hit on a mob across a pinned line is an entity
/// effect to the mob's holder, which hurts it by nothing and pushes it along the
/// snowball's motion; the snowball shows its particles and is gone where it hit.
#[test]
fn a_player_s_snowball_hits_a_cow_across_a_pinned_line_through_its_holder() {
    let cow = placed(
        &made(items::COW_SPAWN_EGG, entity_types::COW),
        1,
        [17.5, FLOOR, 4.5],
        0,
    );
    let world = World::default().with_entities(std::slice::from_ref(&cow));
    let mut west = side(
        true,
        &world,
        &[items::SNOWBALL],
        Some((1, Vec3::new(14.5, FLOOR, 4.5))),
    );
    let mut east = side(false, &world, &[], None);
    // The cow stands a while in its holder, as it would have.
    east.idle_for(5);
    let whole = health(&the(&east, key(1)));
    // Looking east (yaw -90), a little down, at the cow's middle.
    use_item(&mut west, -90.0, 8.0);
    let snowball = the_snowball(&west).key;
    let mut seen = Vec::new();
    let hit = west.idle_until(10, &mut seen, |_, seen| {
        seen.last()
            .is_some_and(|output| entity_events(output, snowball).contains(&SNOWBALL_HIT))
    });
    assert!(hit, "it hit");
    let output = seen.last().expect("a tick");
    assert!(gone(output, snowball));
    let hits: Vec<Effect> = output
        .effects
        .iter()
        .filter(|effect| {
            matches!(effect.what, Effected::OnEntity { key: hit, by: EntityUse::Thrown { .. } } if hit == key(1))
        })
        .cloned()
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "one hit for the holder: {:?}",
        output.effects
    );
    let Effected::OnEntity {
        by: EntityUse::Thrown { damage, kind, .. },
        ..
    } = hits[0].what
    else {
        unreachable!("filtered");
    };
    assert_eq!(damage.0, 0.0);
    assert_eq!(i32::from(kind), entity_types::SNOWBALL);
    let before = the(&east, key(1)).motion;
    file(&mut east.region, 70, &hits);
    let now = the(&east, key(1));

    assert_eq!(health(&now), whole, "hurt by nothing");
    assert!(
        now.motion[0] > before[0] + 0.1 && now.motion[1] > 0.0,
        "pushed east and up by its holder: {:?}",
        now.motion
    );
}

/// The plan's section 10: the snowball itself crosses a line as any entity does: the
/// region it leaves sends it on, and the region beyond takes it in and it flies on there
/// by the same rule.
#[test]
fn a_player_s_snowball_crosses_a_pinned_line_in_flight_and_flies_on() {
    let mut west = side(
        true,
        &World::default(),
        &[items::SNOWBALL],
        Some((1, Vec3::new(14.5, FLOOR, 4.5))),
    );
    let mut east = side(false, &World::default(), &[], None);
    // Looking east and up: well over anything.
    use_item(&mut west, -90.0, -20.0);
    let snowball = the_snowball(&west);
    let mut sent = None;
    for _ in 0..5 {
        let output = west.idle();
        if let Some((chunk, entity)) = output
            .arrivals
            .iter()
            .find(|(_, entity)| entity.key == snowball.key)
        {
            sent = Some((*chunk, entity.clone()));
            break;
        }
    }
    let (chunk, sent) = sent.expect("it is sent across");
    assert!(
        sent.position[0] >= 16.0,
        "beyond the line: {:?}",
        sent.position
    );
    assert!(snowballs(&west).is_empty(), "gone from the west");
    east.region.tick(&TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 80,
                index: 0,
            },
            chunk,
            entity: sent.clone(),
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    let on = the_snowball(&east);
    assert_eq!(on.key, snowball.key);
    let motion = [
        sent.motion[0] * drag(),
        (sent.motion[1] - GRAVITY) * drag(),
        sent.motion[2] * drag(),
    ];
    for axis in 0..3 {
        assert!(
            (on.motion[axis] - motion[axis]).abs() < 1e-9,
            "it flies on: {:?} against {motion:?}",
            on.motion
        );
        assert!(
            (on.position[axis] - (sent.position[axis] + motion[axis])).abs() < 1e-9,
            "by its motion: {:?}",
            on.position
        );
    }
    match on.data {
        EntityData::Thrown { owner, .. } => assert_eq!(owner, Some(west.me()), "still theirs"),
        other => panic!("a thrown item: {other:?}"),
    }
}

/// The plan's section 10, "The difficulty": fire spreads by the world's difficulty
/// (`FireBlock.tick`: a cell of ignite odds `o` beside a fire of age `a` is set alight
/// when `nextInt(bound) <= (o + 40 + 7 d) / (a + 30)`, `d` the difficulty's id), and at
/// easy more slowly than at normal. Each of many fires of age 10 on netherrack has 17
/// cells of air beside wool within its reach at a bound of 100 and 9 at 300: at easy the
/// chance is 77 / 40 = 1, so each cell catches with 2 in its bound, 0.40 cells a fire in
/// its first tick; at normal 84 / 40 = 2, 3 in the bound, 0.60. Counted after every fire
/// has ticked once and before any it set has, the cells set alight are near the first.
/// The fires are in the chunks whose neighbours are all loaded, where blocks tick.
#[test]
fn at_easy_fire_spreads_as_slowly_as_the_easy_difficulty_says() {
    let fire = state("minecraft:fire[age=10]");
    assert_eq!(block_id(fire), block_id(blocks::FIRE));
    let mut world = World::default();
    let mut fires = Vec::new();
    // Chunks -4 to 4 loaded: those from -3 to 3, blocks -48 to 63, tick.
    for x in (-46..=62).step_by(4) {
        for z in (-46..=62).step_by(4) {
            let at = bp(x, -60, z);
            fires.push(at);
            world.blocks.push((bp(x, -61, z), blocks::NETHERRACK));
            world.blocks.push((at, fire));
            world.ticks.push((
                at,
                clustine_world::ScheduledTick {
                    at: clustine_world::InChunk::new(
                        x.rem_euclid(16) as usize,
                        -60,
                        z.rem_euclid(16) as usize,
                    ),
                    of: clustine_world::Ticked::Block(block_id(blocks::FIRE)),
                    delay: 1,
                    priority: 0,
                },
            ));
            for dx in -2..=2_i32 {
                for dz in -2..=2_i32 {
                    // A roof of wool over the fire's three by three, two above it, and a
                    // ring of wool about that three by three at its height.
                    let y = if dx.abs() <= 1 && dz.abs() <= 1 {
                        -58
                    } else {
                        -60
                    };
                    world
                        .blocks
                        .push((bp(x + dx, y, z + dz), blocks::WHITE_WOOL));
                }
            }
        }
    }
    let layout = Layout {
        held: around(5),
        loaded: around(4),
        ..Layout::alone()
    };
    let clock = WorldClock {
        game_time: 18_000,
        day: 18_000,
    };
    let mut scene = Scene {
        region: region_at(&layout, &world, inventory(&[]), Some(clock)),
        layout,
        inventory: inventory(&[]),
        actors: Vec::new(),
    };
    let scheduled = |scene: &Scene| -> usize {
        scene
            .region
            .loaded_chunks()
            .into_iter()
            .map(|chunk| scene.region.scheduled_ticks(chunk).len())
            .sum()
    };
    assert_eq!(scheduled(&scene), fires.len());
    scene.idle_for(20);
    let fire_id = block_id(blocks::FIRE);
    let mut set_alight = 0;
    for at in &fires {
        assert_eq!(
            block_id(scene.block(*at)),
            fire_id,
            "the fire burns on: {at:?}"
        );
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=4 {
                    let cell = at.offset(dx, dy, dz);
                    if (dx, dy, dz) != (0, 0, 0) && block_id(scene.block(cell)) == fire_id {
                        set_alight += 1;
                    }
                }
                assert_eq!(
                    scene.block(at.offset(dx, 2, dz)),
                    blocks::WHITE_WOOL,
                    "the roof is out of the fire's reach to burn"
                );
            }
        }
    }
    // Every fire ticked once: each scheduled its next tick, and each it set its first.
    assert_eq!(scheduled(&scene), fires.len() + set_alight);
    let count = fires.len() as f64;
    let (easy, normal) = (0.40 * count, 0.60 * count);
    let set = set_alight as f64;
    assert!(
        set < f64::midpoint(easy, normal) && set > easy * 0.8,
        "{set_alight} cells set alight by {count} fires: at easy about {easy}, at normal {normal}"
    );
}

/// `LivingEntity.travel`'s fall in a tick: the gravity of 0.08 taken off a motion of
/// nothing and the air's drag of 0.98 after, in floats as the game's constants are.
const RESTING_FALL: f64 = -0.078_400_001_525_878_9;

/// `Entity.move` on the server: `verticalCollision` is whether the move asked for was
/// cut short upwards or downwards, and the ground is under a mob only where it was
/// cut short going down. A mob laid down at rest with a motion of nothing (as these
/// tests lay mobs down) asks to move by nothing in its first tick, and so reads off
/// the ground after it although it stays where it was; the gravity of that tick gives
/// it the fall of a mob at rest, and from the second tick on its move is cut short by
/// the floor and it is on the ground. A mob saved at rest keeps that fall in its
/// record and is on the ground from its first tick; a new one (from an egg, a
/// generated chunk or a structure) starts off the ground as the game's does. So a
/// knockback in the tick a mob laid down so first ticks gives it no lift, as it would
/// in the game, and nothing of it shows: the mob neither falls, sinks nor moves.
#[test]
fn a_mob_laid_down_with_no_motion_reads_off_the_ground_for_its_first_tick_only_and_does_not_move() {
    let zombie = made(items::ZOMBIE_SPAWN_EGG, entity_types::ZOMBIE);
    let at = [8.5, FLOOR, 8.5];
    let still = placed(&zombie, 1, at, 0);
    let mut scene = empty_night(&World::default().with_entities(&[still]));
    // The region ticked the zombie in the tick its chunk was loaded, all eight
    // neighbours being loaded with it.
    let first = the(&scene, key(1));
    assert!(!first.on_ground, "off the ground after a move of nothing");
    assert_eq!(first.position, at, "and where it was laid");
    assert_eq!(first.motion, [0.0, RESTING_FALL, 0.0]);
    for tick in 1..4 {
        scene.idle();
        let now = the(&scene, key(1));
        assert!(now.on_ground, "tick {tick}: on the ground");
        assert_eq!(now.position, at, "tick {tick}");
    }

    // Laid down as a mob at rest is saved, it is on the ground from its first tick.
    let mut resting = placed(&zombie, 1, at, 0);
    resting.motion = [0.0, RESTING_FALL, 0.0];
    let mut scene = empty_night(&World::default().with_entities(&[resting]));
    for tick in 0..4 {
        let now = the(&scene, key(1));
        assert!(now.on_ground, "tick {tick}: on the ground");
        assert_eq!(now.position, at, "tick {tick}");
        scene.idle();
    }
}
