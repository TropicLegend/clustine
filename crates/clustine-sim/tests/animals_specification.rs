//! Tests of the farm animals of step N7 (part N7g), written from the specification and
//! the public API alone, by someone who did not write them: the porting brief
//! `docs/groundwork/n7-brief.md` (sections 0, 4, 9.4 and 11.4), the plan
//! `docs/groundwork/n7-plan.md` (sections 7.3.6, 7.5, 8.5, 9.4, 10.7 and 11) and the
//! game's bytecode (`reference/n7-javap/`: `AgeableMob`, `Animal`, `AbstractCow`, `Cow`,
//! `MushroomCow`, `Pig`, `Sheep`, `Chicken`, `Rabbit`, `BreedGoal`, `SpawnEggItem`,
//! `Mob`, `ItemUtils`). Where the brief and the bytecode differ, the bytecode is
//! followed and the test says so.
//!
//! A region is driven as a player drives it: spawn eggs used on the floor and on
//! animals, items used on animals, ticks; at a pinned line, two regions whose copies,
//! effects and arrivals are handed from one to the other as the store would. What the
//! game keeps of an animal that no client is shown (its age, its love) is read from what
//! it does: whether food puts it in love (entity event 18), when it stops being a baby.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60), in the badlands unless
//! a test says otherwise: the floor's biome is 0, the first of the sorted registry, which
//! is in `#spawns_warm_variant_farm_animals`.

mod n6_entities;
mod n6_scene;

use clustine_data::{SYNCED_REGISTRIES, blocks, entity_types, items, sounds};
use clustine_sim::api::{EntityKind, Face, PlayerInput, RegionEvent};
use clustine_sim::shown::{DataShown, Holder, mob_values};
use clustine_sim::{TickInputs, TickOutput};
use clustine_world::{
    Arrival, BlockPos, ChunkPos, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData,
    EntityId, EntityUse, FiledEffect, ItemStack, Vec3,
};
use n6_scene::*;

// ------------------------------------------------------------------------------------
// The six animals as the game makes them.

/// A farm animal as the bytecode has it: its entity type, its egg, its `max_health`
/// (`createAttributes`), what its `isFood` takes (its item tag: `#cow_food`,
/// `#pig_food`, `#sheep_food`, `#chicken_food`, `#rabbit_food`) and an item it does not.
struct Kind {
    name: &'static str,
    kind: i32,
    egg: i32,
    health: f32,
    food: &'static [i32],
    not_food: i32,
}

const COW: Kind = Kind {
    name: "cow",
    kind: entity_types::COW,
    egg: items::COW_SPAWN_EGG,
    health: 10.0,
    food: &[items::WHEAT],
    not_food: items::CARROT,
};

const MOOSHROOM: Kind = Kind {
    name: "mooshroom",
    kind: entity_types::MOOSHROOM,
    egg: items::MOOSHROOM_SPAWN_EGG,
    // `AbstractCow.createAttributes`, which the mooshroom does not override.
    health: 10.0,
    food: &[items::WHEAT],
    not_food: items::CARROT,
};

const PIG: Kind = Kind {
    name: "pig",
    kind: entity_types::PIG,
    egg: items::PIG_SPAWN_EGG,
    health: 10.0,
    food: &[items::CARROT, items::POTATO, items::BEETROOT],
    not_food: items::WHEAT,
};

const SHEEP: Kind = Kind {
    name: "sheep",
    kind: entity_types::SHEEP,
    egg: items::SHEEP_SPAWN_EGG,
    health: 8.0,
    food: &[items::WHEAT],
    not_food: items::CARROT,
};

const CHICKEN: Kind = Kind {
    name: "chicken",
    kind: entity_types::CHICKEN,
    egg: items::CHICKEN_SPAWN_EGG,
    health: 4.0,
    food: &[
        items::WHEAT_SEEDS,
        items::MELON_SEEDS,
        items::PUMPKIN_SEEDS,
        items::BEETROOT_SEEDS,
        items::TORCHFLOWER_SEEDS,
        items::PITCHER_POD,
    ],
    not_food: items::WHEAT,
};

const RABBIT: Kind = Kind {
    name: "rabbit",
    kind: entity_types::RABBIT,
    egg: items::RABBIT_SPAWN_EGG,
    health: 3.0,
    food: &[items::CARROT, items::GOLDEN_CARROT, items::DANDELION],
    not_food: items::WHEAT,
};

const KINDS: [&Kind; 6] = [&COW, &MOOSHROOM, &PIG, &SHEEP, &CHICKEN, &RABBIT];

/// The hearts of `Animal.setInLove` and of a birth (`broadcastEntityEvent(this, 18)`).
const HEARTS: u8 = 18;
/// `EatBlockGoal.start`'s entity event: a sheep lowers its head to eat.
const EATING: u8 = 10;
/// The index of `AgeableMob.AGE_LOCKED` (brief 4.0 item 1, 9.4).
const AGE_LOCKED: u8 = 17;
/// The index of a sheep's `DATA_WOOL_ID`, a cow's or a chicken's variant, a pig's boost.
const OWN_FIRST: u8 = 18;
/// A pig's `DATA_VARIANT_ID`.
const PIG_VARIANT: u8 = 19;
/// `Sheep`'s sheared bit of its wool byte.
const SHEARED: u8 = 0x10;
/// `EquipmentSlot.SADDLE`'s ordinal.
const SADDLE_SLOT: u8 = 7;
/// `DyeColor` ids.
const WHITE: u8 = 0;
const PINK: u8 = 6;
const GRAY: u8 = 7;
const LIGHT_GRAY: u8 = 8;
const BROWN: u8 = 12;
const RED: u8 = 14;
const BLACK: u8 = 15;

/// The id of the biome `name` in the registry the server sends, which is what a
/// section's biome is.
fn biome(name: &str) -> u16 {
    let biomes = SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .expect("the biome registry");
    let at = biomes
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is a biome"));
    u16::try_from(at).expect("few")
}

/// A world of the stone floor in the biome `name`.
fn world_in(name: &str) -> World {
    World {
        biome: biome(name),
        ..World::default()
    }
}

/// The floor's block whose top a spawn egg is used on to make a mob standing at
/// (x + 0.5, -60, z + 0.5).
fn floor_at(x: i32, z: i32) -> BlockPos {
    bp(x, -61, z)
}

// ------------------------------------------------------------------------------------
// Driving a scene.

/// Player 1 selects hotbar `slot` and uses what it holds on the top of `at`: the one
/// living entity it made, if any.
fn hatch(scene: &mut Scene, slot: u8, at: BlockPos) -> Option<EntityId> {
    let before: Vec<EntityId> = scene.all().iter().map(|entity| entity.key).collect();
    scene.select(slot);
    scene.click(at, Face::Top);
    let made: Vec<EntityId> = scene
        .all()
        .iter()
        .map(|entity| entity.key)
        .filter(|key| !before.contains(key))
        .collect();
    assert!(made.len() <= 1, "one egg makes one mob: {made:?}");
    made.first().copied()
}

/// The use of `key` by the actor `who`, with what they hold (`interact`, main hand).
fn used(key: EntityId) -> PlayerInput {
    PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    }
}

/// Player 1 selects `slot` and, in the same tick, uses each of `keys` with it.
fn use_on(scene: &mut Scene, slot: u8, keys: &[EntityId]) -> TickOutput {
    let mut inputs = vec![PlayerInput::SelectSlot { slot }];
    inputs.extend(keys.iter().map(|key| used(*key)));
    scene.inputs_of(0, inputs)
}

fn entity_of(scene: &Scene, key: EntityId) -> Option<Entity> {
    scene.all().into_iter().find(|entity| entity.key == key)
}

fn the_entity(scene: &Scene, key: EntityId) -> Entity {
    entity_of(scene, key).unwrap_or_else(|| panic!("{key:?} is in the scene"))
}

/// What clients are shown of the mob `key`: its synced values, by index.
fn shown(scene: &Scene, key: EntityId) -> Vec<(u8, DataShown, bool)> {
    let entity = the_entity(scene, key);
    mob_values(entity.kind, &entity.data)
}

fn shown_at(scene: &Scene, key: EntityId, index: u8) -> Option<DataShown> {
    shown(scene, key)
        .into_iter()
        .find(|(at, _, _)| *at == index)
        .map(|(_, value, _)| value)
}

fn is_baby(scene: &Scene, key: EntityId) -> bool {
    match shown_at(scene, key, clustine_sim::shown::index::BABY) {
        Some(DataShown::Boolean(baby)) => baby,
        other => panic!("an ageable mob shows its baby flag: {other:?}"),
    }
}

fn age_locked(scene: &Scene, key: EntityId) -> bool {
    match shown_at(scene, key, AGE_LOCKED) {
        Some(DataShown::Boolean(locked)) => locked,
        other => panic!("an ageable mob shows its age lock: {other:?}"),
    }
}

fn wool(scene: &Scene, key: EntityId) -> u8 {
    match shown_at(scene, key, OWN_FIRST) {
        Some(DataShown::Byte(wool)) => wool,
        other => panic!("a sheep shows its wool: {other:?}"),
    }
}

fn health(scene: &Scene, key: EntityId) -> f32 {
    match shown_at(scene, key, clustine_sim::shown::index::HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("a mob shows its health: {other:?}"),
    }
}

/// How many times `key` was shown the entity event `event` in `outputs`.
fn events_of(outputs: &[TickOutput], key: EntityId, event: u8) -> usize {
    outputs
        .iter()
        .flat_map(|output| &output.events)
        .filter(|seen| {
            matches!(seen, RegionEvent::EntityEvent { entity, event: shown, .. }
                if *entity == key && *shown == event)
        })
        .count()
}

fn hearts(output: &TickOutput, key: EntityId) -> usize {
    events_of(std::slice::from_ref(output), key, HEARTS)
}

/// The sounds of id `sound` in the tick, wherever and however they play, with whom each
/// is not played to.
fn played(output: &TickOutput, sound: i32) -> Vec<Option<clustine_world::PlayerId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: id, except, ..
            } if *id == sound => Some(*except),
            RegionEvent::EntitySound { sound: id, .. } if *id == sound => Some(None),
            _ => None,
        })
        .collect()
}

/// How many of `item` player 1's inventory holds, in all its slots.
fn carried(scene: &Scene, item: i32) -> i32 {
    scene
        .region
        .player_state(scene.actors[0].id)
        .expect("player 1 is there")
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

/// The mobs of `kind` in the scene other than `known`.
fn others_of(scene: &Scene, kind: i32, known: &[EntityId]) -> Vec<Entity> {
    scene
        .of_kind(kind)
        .into_iter()
        .filter(|entity| !known.contains(&entity.key))
        .collect()
}

/// The experience orbs of the scene, with what each is worth.
fn orbs(scene: &Scene) -> Vec<(Entity, i32)> {
    scene
        .of_kind(entity_types::EXPERIENCE_ORB)
        .into_iter()
        .map(|orb| match orb.data {
            EntityData::ExperienceOrb { value, .. } => (orb, value),
            ref other => panic!("an orb's record: {other:?}"),
        })
        .collect()
}

fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The stone of a pen two blocks high around x and z from -3 to 4, which keeps the
/// animals within the player's reach and in the chunks that tick: left to wander for
/// thousands of ticks, an animal walks out of the land that ticks, and stops ageing.
fn pen() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut walls = Vec::new();
    for y in [-60, -59] {
        for along in -4..=5 {
            for (x, z) in [(along, -4), (along, 5), (-4, along), (5, along)] {
                walls.push((bp(x, y, z), blocks::STONE));
            }
        }
    }
    walls
}

/// A scene with player 1 at the spawn holding `slots` in the hotbar, in `world` with
/// a [`pen`] about the player.
fn holding(world: &World, slots: &[i32]) -> Scene {
    let stacks: Vec<(usize, ItemStack)> = slots
        .iter()
        .enumerate()
        .map(|(slot, item)| (slot, stack(*item, 1)))
        .collect();
    let mut penned = world.clone();
    penned.blocks.extend(pen());
    Scene::alone(&penned, inventory(&stacks), SPAWN)
}

/// How many entities of `kind` the ticks of `outputs` brought into being.
fn spawned(outputs: &[TickOutput], kind: i32) -> usize {
    outputs
        .iter()
        .flat_map(|output| &output.events)
        .filter(|event| {
            matches!(event, RegionEvent::EntitySpawned(state)
                if matches!(state.kind, EntityKind::Other { kind: made, .. } if i32::from(made) == kind))
        })
        .count()
}

// ------------------------------------------------------------------------------------
// 1. Spawn eggs (brief 1.11, 4.5.1; `SpawnEggItem.spawnOffspringFromSpawnEgg`).

/// Brief 4.8.1, 4.9.1, 4.10.1, 4.11.1, 4.12.2: an egg used on the floor makes one animal
/// of its type, an adult, at its `max_health` (cow and mooshroom 10, pig 10, sheep 8,
/// chicken 4, rabbit 3), standing in the block above the face used.
#[test]
fn an_egg_makes_each_farm_animal_with_its_types_health() {
    for kind in KINDS {
        let mut scene = holding(&World::default(), &[kind.egg]);
        let key = hatch(&mut scene, 0, floor_at(2, 2))
            .unwrap_or_else(|| panic!("the {} egg makes a mob", kind.name));
        let made = the_entity(&scene, key);
        assert_eq!(i32::from(made.kind), kind.kind, "{}", kind.name);
        assert_eq!(health(&scene, key), kind.health, "{}", kind.name);
        assert!(!is_baby(&scene, key), "an egg's {} is grown", kind.name);
        assert!(!age_locked(&scene, key), "{}", kind.name);
        assert!(
            horizontal(made.position, [2.5, -60.0, 2.5]) < 0.3,
            "{} at {:?}",
            kind.name,
            made.position
        );
        assert_eq!(
            carried(&scene, kind.egg),
            1,
            "a creative player keeps the egg"
        );
    }
}

/// `Mob.checkAndHandleImportantInteractions` and `SpawnEggItem.spawnOffspringFromSpawnEgg`
/// (SpawnEggItem.txt 298–358): an egg used on an adult of its own type makes a baby of
/// it with `getBreedOffspring(level, this)`, `setBaby(true)` (age −24000), snapped to the
/// mob's own `position()` with yaw and pitch 0. The brief's 4.5.1 says "the egg
/// position"; the bytecode passes the mob's position (Mob.txt 2813–2818). The baby comes
/// in the input's tick, before the parent ticks, so it stands where the parent stood.
#[test]
fn an_egg_used_on_an_adult_of_its_kind_makes_a_baby_where_it_stands() {
    for kind in KINDS {
        let mut scene = holding(&World::default(), &[kind.egg]);
        let parent = hatch(&mut scene, 0, floor_at(2, 2)).expect("an adult");
        scene.idle_for(3);
        let stood = the_entity(&scene, parent).position;
        let output = use_on(&mut scene, 0, &[parent]);
        let babies = others_of(&scene, kind.kind, &[parent]);
        assert_eq!(babies.len(), 1, "{}: one baby", kind.name);
        let baby = &babies[0];
        assert!(is_baby(&scene, baby.key), "{}: a baby", kind.name);
        assert!(!is_baby(&scene, parent), "{}: the parent is not", kind.name);
        assert!(
            horizontal(baby.position, stood) < 0.05 && (baby.position[1] - stood[1]).abs() < 0.05,
            "{}: the baby at {:?}, the parent stood at {stood:?}",
            kind.name,
            baby.position
        );
        assert!(
            output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::EntitySpawned(state) if state.entity == baby.key)),
            "{}: the baby is shown",
            kind.name
        );
        assert_eq!(carried(&scene, kind.egg), 1, "the egg is kept");
    }
}

/// `spawnOffspringFromSpawnEgg` returns empty unless the egg `spawnsEntity` of the mob's
/// type (SpawnEggItem.txt 300–312), so an egg of another type used on an animal makes
/// nothing of it: a pig's egg on a cow makes neither a calf nor a pig.
#[test]
fn an_egg_of_another_kind_used_on_an_animal_makes_no_baby() {
    let mut scene = holding(
        &World::default(),
        &[items::COW_SPAWN_EGG, items::PIG_SPAWN_EGG],
    );
    let cow = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    use_on(&mut scene, 1, &[cow]);
    scene.idle();
    assert_eq!(scene.of_kind(entity_types::COW).len(), 1);
    assert!(scene.of_kind(entity_types::PIG).is_empty());
}

/// Brief 4.6.1–4.6.2: an egg's cow, pig and chicken take the variant whose biome tag
/// holds the biome at their block (`selectVariantToSpawn`): warm in the badlands
/// (`#is_badlands`), cold in snowy plains, temperate in plains. Shown by the registry's
/// sorted ids, cold 0, temperate 1, warm 2 (9.4), at the cow's and chicken's index 18 and
/// the pig's 19; the temperate default is not paired.
#[test]
fn an_eggs_cow_pig_and_chicken_take_the_variant_of_their_biome() {
    for (place, id) in [
        ("minecraft:badlands", 2),
        ("minecraft:snowy_plains", 0),
        ("minecraft:plains", 1),
    ] {
        let world = world_in(place);
        for (kind, index, holder) in [
            (&COW, OWN_FIRST, Holder::CowVariant),
            (&PIG, PIG_VARIANT, Holder::PigVariant),
            (&CHICKEN, OWN_FIRST, Holder::ChickenVariant),
        ] {
            let mut scene = holding(&world, &[kind.egg]);
            let key = hatch(&mut scene, 0, floor_at(2, 2)).expect("a mob");
            let value = shown(&scene, key)
                .into_iter()
                .find(|(at, _, _)| *at == index)
                .unwrap_or_else(|| panic!("{} shows its variant", kind.name));
            assert_eq!(
                value.1,
                DataShown::Holder(holder, id),
                "{} in {place}",
                kind.name
            );
            assert_eq!(
                value.2,
                id == 1,
                "{} in {place}: default only if temperate",
                kind.name
            );
        }
    }
}

/// Brief 4.10.3: a sheep's colour at spawn is drawn from its biome's list; in a warm
/// biome gray, light gray, white or black (18 in 100), else brown, 1 in 500 of those
/// pink. Twenty sheep in the badlands are all of those colours, most of them brown, and
/// none is sheared.
#[test]
fn an_eggs_sheep_in_the_badlands_takes_a_warm_colour() {
    let mut scene = holding(&World::default(), &[items::SHEEP_SPAWN_EGG]);
    let mut colours = Vec::new();
    for n in 0..20 {
        let key = hatch(&mut scene, 0, floor_at(-3 + (n % 7), -3 + n / 7 * 2)).expect("a sheep");
        colours.push(wool(&scene, key));
    }
    for colour in &colours {
        assert!(
            [GRAY, LIGHT_GRAY, WHITE, BLACK, BROWN, PINK].contains(colour),
            "{colours:?}"
        );
    }
    assert!(
        colours.iter().filter(|colour| **colour == BROWN).count() >= 10,
        "{colours:?}"
    );
}

// ------------------------------------------------------------------------------------
// 2. Feeding (brief 4.3.3, 4.3.4).

/// Brief 4.3.4: each animal's food (`isFood`, its tag) puts an adult in love, which
/// `setInLove` shows with entity event 18 once; an item that is not its food does
/// nothing; a creative player's stack is kept either way. One animal per item, as love
/// lasts 600 ticks.
#[test]
fn each_animals_food_puts_an_adult_in_love_and_nothing_else_does() {
    for kind in KINDS {
        let mut slots = vec![kind.egg, kind.not_food];
        slots.extend(kind.food);
        let mut scene = holding(&World::default(), &slots);
        let places = [(2, 2), (2, -2), (-2, 2), (-2, -2), (3, 0), (0, 3), (-3, 0)];
        let keys: Vec<EntityId> = (0..slots.len() - 1)
            .map(|n| {
                let (x, z) = places[n];
                hatch(&mut scene, 0, floor_at(x, z)).expect("an adult")
            })
            .collect();
        let output = use_on(&mut scene, 1, &[keys[0]]);
        assert_eq!(hearts(&output, keys[0]), 0, "{}: not its food", kind.name);
        for (n, food) in kind.food.iter().enumerate() {
            let slot = u8::try_from(n + 2).expect("a hotbar slot");
            let output = use_on(&mut scene, slot, &[keys[n + 1]]);
            assert_eq!(
                hearts(&output, keys[n + 1]),
                1,
                "{}: item {food} is its food",
                kind.name
            );
            assert_eq!(carried(&scene, *food), 1, "{}: the food is kept", kind.name);
        }
        assert_eq!(carried(&scene, kind.not_food), 1);
    }
}

/// Brief 4.3.3–4.3.4: `canFallInLove` is `inLove <= 0`, so an animal in love takes no
/// more food: fed again a tick later it shows no hearts.
#[test]
fn an_animal_in_love_takes_no_more_food() {
    let mut scene = holding(&World::default(), &[items::COW_SPAWN_EGG, items::WHEAT]);
    let cow = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    assert_eq!(hearts(&use_on(&mut scene, 1, &[cow]), cow), 1);
    let again = use_on(&mut scene, 1, &[cow]);
    assert_eq!(hearts(&again, cow), 0, "already in love");
}

// ------------------------------------------------------------------------------------
// 3. Breeding in one region (brief 4.3.5, 4.3.6, 4.4).

/// What a breeding left: the child, the orb and its worth, the parent at whose feet it
/// was born, and the outputs of the ticks up to and with the birth.
struct Birth {
    child: Entity,
    orb: (Entity, i32),
    parent: EntityId,
    stood: [f64; 3],
    seen: Vec<TickOutput>,
}

/// Two adults of `kind` two blocks apart, both fed in one tick, and the ticks idled until
/// a third mob of the kind appears (at most 400): the scene, the parents and the birth.
fn bred(kind: &Kind) -> (Scene, [EntityId; 2], Birth) {
    let mut scene = holding(&World::default(), &[kind.egg, kind.food[0]]);
    let a = hatch(&mut scene, 0, floor_at(2, 2)).expect("a parent");
    let b = hatch(&mut scene, 0, floor_at(2, 4)).expect("a parent");
    scene.idle_for(2);
    let fed = use_on(&mut scene, 1, &[a, b]);
    assert_eq!(
        (hearts(&fed, a), hearts(&fed, b)),
        (1, 1),
        "{}: both in love",
        kind.name
    );
    let birth = born(&mut scene, kind, [a, b]);
    (scene, [a, b], birth)
}

fn born(scene: &mut Scene, kind: &Kind, parents: [EntityId; 2]) -> Birth {
    let mut seen = Vec::new();
    for _ in 0..400 {
        let before = [
            the_entity(scene, parents[0]).position,
            the_entity(scene, parents[1]).position,
        ];
        seen.push(scene.idle());
        let children = others_of(scene, kind.kind, &parents);
        if children.is_empty() {
            continue;
        }
        assert_eq!(children.len(), 1, "{}: one child", kind.name);
        let orbs = orbs(scene);
        assert_eq!(orbs.len(), 1, "{}: one orb", kind.name);
        let last = seen.last().expect("a tick");
        let shown: Vec<usize> = parents.iter().map(|parent| hearts(last, *parent)).collect();
        assert_eq!(
            shown.iter().sum::<usize>(),
            1,
            "{}: the breeding parent alone shows hearts at the birth",
            kind.name
        );
        let at = usize::from(shown[1] == 1);
        return Birth {
            child: children[0].clone(),
            orb: orbs[0].clone(),
            parent: parents[at],
            stood: before[at],
            seen,
        };
    }
    panic!("{}: no child within 400 ticks", kind.name);
}

/// Brief 4.3.6 and 4.4: two adults of a kind in love within reach breed: one child, of
/// their kind and a baby, born at the breeding parent's place (`snapTo(getX(), getY(),
/// getZ(), 0, 0)`, where the parent stood when its goal ran, before its own move), with
/// yaw and pitch 0; one experience orb worth `nextInt(7) + 1` at the same place, added
/// before the child; the hearts on the breeding parent alone; some 60 ticks after they
/// were fed, as the goal runs every other tick. Neither parent can be fed
/// into love right after (their age is 6000), and no second child comes.
#[test]
fn two_adults_in_love_breed_one_child_of_their_kind_with_one_orb() {
    for kind in KINDS {
        let (mut scene, parents, birth) = bred(kind);
        assert!(is_baby(&scene, birth.child.key), "{}: a baby", kind.name);
        assert_eq!(i32::from(birth.child.kind), kind.kind);
        assert!(
            (1..=7).contains(&birth.orb.1),
            "{}: worth {}",
            kind.name,
            birth.orb.1
        );
        assert_eq!(
            birth.child.position, birth.stood,
            "{}: the child where the breeding parent stood",
            kind.name
        );
        assert_eq!(
            birth.orb.0.position, birth.stood,
            "{}: the orb where the breeding parent stood",
            kind.name
        );
        assert_eq!(
            (birth.child.yaw, birth.child.pitch),
            (0.0, 0.0),
            "{}: the child faces south",
            kind.name
        );
        assert!(parents.contains(&birth.parent));
        // `BreedGoal` runs every other tick (it does not ask to be updated every tick)
        // and breeds at `adjustedTickDelay(60)` = 30 of its runs: some 60 ticks after the
        // feeding for two that start within reach (brief 4.4).
        assert!(
            (55..=70).contains(&birth.seen.len()),
            "{}: born {} ticks after the feeding",
            kind.name,
            birth.seen.len()
        );
        let fed = use_on(&mut scene, 1, &parents);
        assert_eq!(
            hearts(&fed, parents[0]) + hearts(&fed, parents[1]),
            0,
            "{}: neither takes food at age 6000",
            kind.name
        );
        assert_eq!(
            spawned(&birth.seen, entity_types::EXPERIENCE_ORB),
            1,
            "{}",
            kind.name
        );
        // The orb flies to the player and is taken: counted as it comes, not as it lies.
        let after = scene.idle_for(300);
        assert_eq!(
            others_of(&scene, kind.kind, &parents).len(),
            1,
            "{}: no second child",
            kind.name
        );
        assert_eq!(
            spawned(&after, entity_types::EXPERIENCE_ORB),
            0,
            "{}: no second orb",
            kind.name
        );
    }
}

/// Brief 4.3.6: both parents' ages are set to 6000 at a birth and count down a tick at a
/// time (4.2.3), and an adult takes food only at age 0 (4.3.4): fed 5900 ticks after
/// the birth they show no hearts; fed 6100 ticks after, both do, and they breed again.
#[test]
fn parents_breed_again_only_once_their_age_has_run_out() {
    let (mut scene, parents, birth) = bred(&COW);
    scene.idle_for(5_900);
    let early = use_on(&mut scene, 1, &parents);
    assert_eq!(hearts(&early, parents[0]) + hearts(&early, parents[1]), 0);
    scene.idle_for(200);
    let late = use_on(&mut scene, 1, &parents);
    assert_eq!(
        (hearts(&late, parents[0]), hearts(&late, parents[1])),
        (1, 1)
    );
    let mut known = parents.to_vec();
    known.push(birth.child.key);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(400, &mut seen, |scene, _| !others_of(
            scene,
            entity_types::COW,
            &known
        )
        .is_empty()),
        "a second calf"
    );
}

/// Brief 4.3.5: `canMate` asks for the exact class, so a cow and a mooshroom in love
/// side by side never breed.
#[test]
fn a_cow_and_a_mooshroom_in_love_do_not_breed() {
    let mut scene = holding(
        &World::default(),
        &[
            items::COW_SPAWN_EGG,
            items::MOOSHROOM_SPAWN_EGG,
            items::WHEAT,
        ],
    );
    let cow = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    let mooshroom = hatch(&mut scene, 1, floor_at(2, 4)).expect("a mooshroom");
    let fed = use_on(&mut scene, 2, &[cow, mooshroom]);
    assert_eq!((hearts(&fed, cow), hearts(&fed, mooshroom)), (1, 1));
    scene.idle_for(400);
    assert_eq!(scene.of_kind(entity_types::COW).len(), 1);
    assert_eq!(scene.of_kind(entity_types::MOOSHROOM).len(), 1);
    assert!(orbs(&scene).is_empty());
}

/// Brief 4.10.7: a lamb's colour is the mix the crafting recipes give for its parents'
/// dyes (`DyeColor.getMixedColor`): a red and a white sheep have a pink lamb. The
/// parents are dyed first (4.10.6), the dye kept in creative.
#[test]
fn a_red_and_a_white_sheep_have_a_pink_lamb() {
    let mut scene = holding(
        &World::default(),
        &[
            items::SHEEP_SPAWN_EGG,
            items::WHEAT,
            items::RED_DYE,
            items::WHITE_DYE,
        ],
    );
    let red = hatch(&mut scene, 0, floor_at(2, 2)).expect("a sheep");
    let white = hatch(&mut scene, 0, floor_at(2, 4)).expect("a sheep");
    use_on(&mut scene, 2, &[red]);
    use_on(&mut scene, 3, &[white]);
    assert_eq!((wool(&scene, red), wool(&scene, white)), (RED, WHITE));
    use_on(&mut scene, 1, &[red, white]);
    let birth = born(&mut scene, &SHEEP, [red, white]);
    assert_eq!(wool(&scene, birth.child.key), PINK);
}

/// Brief 4.3.2–4.3.3: love lasts 600 ticks (`inLove` set to 600 and counted down a tick
/// at a time), and `canFallInLove` is `inLove <= 0`: a cow fed 595 ticks after it fell in
/// love takes nothing; ten ticks later it falls in love again.
#[test]
fn love_lasts_600_ticks() {
    let mut scene = holding(&World::default(), &[items::COW_SPAWN_EGG, items::WHEAT]);
    let cow = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    assert_eq!(hearts(&use_on(&mut scene, 1, &[cow]), cow), 1);
    scene.idle_for(594);
    assert_eq!(
        hearts(&use_on(&mut scene, 1, &[cow]), cow),
        0,
        "still in love"
    );
    scene.idle_for(10);
    assert_eq!(
        hearts(&use_on(&mut scene, 1, &[cow]), cow),
        1,
        "love has run out"
    );
}

/// Brief 4.3.3: `Animal.actuallyHurt` resets love before the damage, so a cow in love
/// that is hit can be fed into love again at once (its age is still 0).
#[test]
fn a_hurt_animal_falls_out_of_love() {
    let mut scene = holding(&World::default(), &[items::COW_SPAWN_EGG, items::WHEAT]);
    let cow = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    assert_eq!(hearts(&use_on(&mut scene, 1, &[cow]), cow), 1);
    scene.input(PlayerInput::Attack {
        part: None,
        entity: cow,
    });
    assert!(health(&scene, cow) < COW.health, "the hit hurt it");
    assert_eq!(
        hearts(&use_on(&mut scene, 1, &[cow]), cow),
        1,
        "out of love, it takes food again"
    );
}

/// Brief 4.3.6: the orb is added before the child (`addFreshEntity` of the orb, then
/// `addFreshEntityWithPassengers` of the child), so in Clustine, which gives keys in
/// the order entities are made, the orb's key is the lower.
#[test]
fn a_births_orb_is_made_before_its_child() {
    let (_, _, birth) = bred(&COW);
    assert!(
        birth.orb.0.key < birth.child.key,
        "{:?} {:?}",
        birth.orb.0.key,
        birth.child.key
    );
}

/// Brief 4.0 item 5 and 4.6.4: a bred child and a baby of an egg used on an animal get no
/// `finalizeSpawn`, so their sound variant is always the default `classic` (a cow's and a
/// chicken's 0, a pig's 1 in the sent registries) and their variant is a parent's: two
/// warm parents of the badlands have a warm child (2). A mooshroom's calf is red (index
/// 18, 0, the default).
#[test]
fn a_child_takes_its_parents_variant_and_the_default_sound() {
    for (kind, variant_at, variant, sound_at, sound) in [
        (
            &COW,
            OWN_FIRST,
            DataShown::Holder(Holder::CowVariant, 2),
            19,
            DataShown::Holder(Holder::CowSoundVariant, 0),
        ),
        (
            &PIG,
            PIG_VARIANT,
            DataShown::Holder(Holder::PigVariant, 2),
            20,
            DataShown::Holder(Holder::PigSoundVariant, 1),
        ),
        (
            &CHICKEN,
            OWN_FIRST,
            DataShown::Holder(Holder::ChickenVariant, 2),
            19,
            DataShown::Holder(Holder::ChickenSoundVariant, 0),
        ),
    ] {
        let (mut scene, [parent, _], birth) = bred(kind);
        let child = birth.child.key;
        assert_eq!(
            shown_at(&scene, child, variant_at),
            Some(variant.clone()),
            "{}",
            kind.name
        );
        assert_eq!(
            shown_at(&scene, child, sound_at),
            Some(sound.clone()),
            "{}",
            kind.name
        );
        // The egg's baby, made with the parent as its own partner.
        let before: Vec<EntityId> = scene.of_kind(kind.kind).iter().map(|mob| mob.key).collect();
        use_on(&mut scene, 0, &[parent]);
        let baby = others_of(&scene, kind.kind, &before)[0].key;
        assert_eq!(
            shown_at(&scene, baby, variant_at),
            Some(variant.clone()),
            "{}'s egg baby",
            kind.name
        );
        assert_eq!(
            shown_at(&scene, baby, sound_at),
            Some(sound.clone()),
            "{}'s egg baby",
            kind.name
        );
    }
    let (scene, _, birth) = bred(&MOOSHROOM);
    let red = shown(&scene, birth.child.key)
        .into_iter()
        .find(|(index, _, _)| *index == OWN_FIRST);
    assert_eq!(red, Some((OWN_FIRST, DataShown::Int(0), true)));
}

/// Brief 4.12.6: an egg's rabbit draws its variant from the biome at its block
/// (`getRandomRabbitVariant`): in a biome of `#spawns_white_rabbits` (snowy plains) white
/// (1) or white splotched (3), in one of `#spawns_gold_rabbits` (the desert) gold (4),
/// elsewhere (the badlands) brown (0), salt (5) or black (2); shown at index 18.
#[test]
fn an_eggs_rabbit_takes_a_variant_of_its_biome() {
    for (place, allowed) in [
        ("minecraft:snowy_plains", &[1, 3][..]),
        ("minecraft:desert", &[4][..]),
        ("minecraft:badlands", &[0, 5, 2][..]),
    ] {
        let mut scene = holding(&world_in(place), &[items::RABBIT_SPAWN_EGG]);
        for (x, z) in [(2, 2), (-2, 2), (2, -2), (-2, -2)] {
            let rabbit = hatch(&mut scene, 0, floor_at(x, z)).expect("a rabbit");
            match shown_at(&scene, rabbit, OWN_FIRST) {
                Some(DataShown::Int(variant)) => {
                    assert!(allowed.contains(&variant), "{place}: {variant}")
                }
                other => panic!("{place}: a rabbit shows its variant: {other:?}"),
            }
        }
    }
}

// ------------------------------------------------------------------------------------
// 4. Growing up (brief 4.2.3, 4.2.4, 4.2.6).

/// Two calves made in one tick by eggs used on two cows, and the tick they were made in.
fn two_calves(slots: &[i32]) -> (Scene, [EntityId; 2], u64) {
    let mut all = vec![items::COW_SPAWN_EGG];
    all.extend(slots);
    let mut scene = holding(&World::default(), &all);
    let a = hatch(&mut scene, 0, floor_at(2, 2)).expect("a cow");
    let b = hatch(&mut scene, 0, floor_at(-2, 2)).expect("a cow");
    scene.idle();
    use_on(&mut scene, 0, &[a, b]);
    let born_in = scene.region.tick_number();
    let calves: Vec<EntityId> = others_of(&scene, entity_types::COW, &[a, b])
        .iter()
        .map(|calf| calf.key)
        .collect();
    assert_eq!(calves.len(), 2, "two calves");
    (scene, [calves[0], calves[1]], born_in)
}

/// Idles until `key` is no longer a baby, for at most `ticks`: the tick it grew up in.
fn grown_up_in(scene: &mut Scene, key: EntityId, ticks: u32) -> Option<u64> {
    for _ in 0..ticks {
        scene.idle();
        if !is_baby(scene, key) {
            return Some(scene.region.tick_number());
        }
    }
    None
}

/// Brief 4.2.3: a baby grows a tick of age a tick, so one born at −24000 is an adult
/// 24000 ticks on (counting its first tick, in which it was made, as the game ticks a
/// mob added by a player's use before the level's entities: give or take that one).
#[test]
fn a_baby_grows_up_after_24000_ticks() {
    let (mut scene, [calf, _], born_in) = two_calves(&[]);
    assert!(is_baby(&scene, calf));
    let grown = grown_up_in(&mut scene, calf, 24_100).expect("it grows up");
    let took = grown - born_in;
    assert!((23_999..=24_001).contains(&took), "grew up after {took}");
}

/// Brief 4.2.4 and 4.3.4: feeding a baby (`ageUp(getSpeedUpSecondsWhenFeeding(-age),
/// true)`) takes off `(int)((float)(-age / 20) · 0.1f)` seconds: fed some ten ticks after
/// its birth (age about −23990, so 1199 whole seconds left), a calf gains 119 seconds,
/// 2380 ticks, and grows up exactly that much before its twin. Feeding a baby shows no
/// hearts and keeps the creative player's wheat. Grown up by its own ticks it is not
/// charged the forced age (4.0 item 8): it takes food at once.
#[test]
fn feeding_a_baby_takes_a_tenth_of_what_is_left_off_its_childhood() {
    let (mut scene, [fed, twin], _) = two_calves(&[items::WHEAT]);
    scene.idle_for(10);
    let output = use_on(&mut scene, 1, &[fed]);
    assert_eq!(hearts(&output, fed), 0, "a baby is not put in love");
    assert_eq!(carried(&scene, items::WHEAT), 1);
    let first = grown_up_in(&mut scene, fed, 24_100).expect("the fed calf grows up");
    assert!(is_baby(&scene, twin), "the twin is still a calf");
    let fed_again = use_on(&mut scene, 1, &[fed]);
    assert_eq!(
        hearts(&fed_again, fed),
        1,
        "grown up by itself, it takes food"
    );
    let second = grown_up_in(&mut scene, twin, 3_000).expect("the twin grows up");
    assert_eq!(second - first, 2_380);
}

/// Brief 4.0 item 2 and 4.2.6: a golden dandelion used on a baby locks its age
/// (`AGE_LOCKED`, index 17, shown) with `item.golden_dandelion.use`; not again within its
/// 40 ticks; a locked baby does not grow (`canAgeUp` is false); used again it unlocks with
/// `.unuse` and resets the age to −24000, so it grows up 24000 ticks after the unlock. On
/// an adult it does nothing.
#[test]
fn a_golden_dandelion_stops_a_baby_growing_until_it_is_used_again() {
    let (mut scene, [calf, _], _) = two_calves(&[items::GOLDEN_DANDELION]);
    let lock = use_on(&mut scene, 1, &[calf]);
    assert!(age_locked(&scene, calf), "locked");
    assert_eq!(played(&lock, sounds::ITEM_GOLDEN_DANDELION_USE).len(), 1);
    let too_soon = use_on(&mut scene, 1, &[calf]);
    assert!(age_locked(&scene, calf), "not unlocked within 40 ticks");
    assert!(played(&too_soon, sounds::ITEM_GOLDEN_DANDELION_UNUSE).is_empty());
    assert_eq!(
        grown_up_in(&mut scene, calf, 24_100),
        None,
        "it does not grow"
    );
    let unlock = use_on(&mut scene, 1, &[calf]);
    assert!(!age_locked(&scene, calf), "unlocked");
    assert_eq!(
        played(&unlock, sounds::ITEM_GOLDEN_DANDELION_UNUSE).len(),
        1
    );
    let unlocked_in = scene.region.tick_number();
    let grown = grown_up_in(&mut scene, calf, 24_100).expect("it grows up");
    let took = grown - unlocked_in;
    assert!(
        (23_999..=24_001).contains(&took),
        "grew up {took} after the unlock"
    );
    // An adult (the calf, now grown) is not locked.
    let adult = use_on(&mut scene, 1, &[calf]);
    assert!(!age_locked(&scene, calf));
    assert!(played(&adult, sounds::ITEM_GOLDEN_DANDELION_USE).is_empty());
}

// ------------------------------------------------------------------------------------
// 6. Each animal's own uses (brief 4.7–4.11).

/// Brief 4.8.1 and 4.0 item 17: a bucket used on a cow gives a creative player a milk
/// bucket through `ItemUtils.createFilledResult` with duplicates prevented: the bucket
/// is kept, a milk bucket is added only if none is held, so a second milking adds none.
/// `entity.cow.milk` plays for everyone but the milker. A calf is not milked. A
/// mooshroom is milked so too (its `mobInteract` falls through to the cow's).
#[test]
fn a_bucket_milks_a_cow_and_a_mooshroom_once_for_a_creative_player() {
    for kind in [&COW, &MOOSHROOM] {
        let mut scene = holding(&World::default(), &[kind.egg, items::BUCKET]);
        let adult = hatch(&mut scene, 0, floor_at(2, 2)).expect("an adult");
        let milked = use_on(&mut scene, 1, &[adult]);
        assert_eq!(
            carried(&scene, items::MILK_BUCKET),
            1,
            "{}: milked",
            kind.name
        );
        assert_eq!(
            carried(&scene, items::BUCKET),
            1,
            "{}: the bucket kept",
            kind.name
        );
        if kind.kind == entity_types::COW {
            assert_eq!(
                played(&milked, sounds::ENTITY_COW_MILK),
                [Some(scene.actors[0].id)],
                "heard by all but the milker"
            );
        }
        use_on(&mut scene, 1, &[adult]);
        assert_eq!(
            carried(&scene, items::MILK_BUCKET),
            1,
            "{}: no second",
            kind.name
        );

        let mut scene = holding(&World::default(), &[kind.egg, items::BUCKET]);
        let adult = hatch(&mut scene, 0, floor_at(2, 2)).expect("an adult");
        use_on(&mut scene, 0, &[adult]);
        let baby = others_of(&scene, kind.kind, &[adult])[0].key;
        use_on(&mut scene, 1, &[baby]);
        assert_eq!(
            carried(&scene, items::MILK_BUCKET),
            0,
            "{}: a baby is not milked",
            kind.name
        );
    }
}

/// Brief 4.8.4 and 4.0 item 17: a bowl used on a mooshroom gives a mushroom stew with
/// duplicates allowed: every use adds one (a stew does not stack) and the creative
/// player keeps the bowl; `entity.mooshroom.milk` plays.
#[test]
fn a_bowl_on_a_mooshroom_gives_a_stew_every_time() {
    let mut scene = holding(
        &World::default(),
        &[items::MOOSHROOM_SPAWN_EGG, items::BOWL],
    );
    let mooshroom = hatch(&mut scene, 0, floor_at(2, 2)).expect("a mooshroom");
    let first = use_on(&mut scene, 1, &[mooshroom]);
    assert_eq!(played(&first, sounds::ENTITY_MOOSHROOM_MILK).len(), 1);
    use_on(&mut scene, 1, &[mooshroom]);
    assert_eq!(carried(&scene, items::MUSHROOM_STEW), 2);
    assert_eq!(carried(&scene, items::BOWL), 1);
}

/// Brief 4.10.4, 4.10.6 and 4.0 item 21: a dye changes a sheep's colour (the dye kept in
/// creative); shears shear it: one to three single items of wool of its colour
/// (`uniform(1, 3)` rolls, each item `copyWithCount(1)`), `entity.sheep.shear`, and the
/// sheared bit set. A sheared sheep gives no more wool and cannot be dyed.
#[test]
fn shears_take_a_sheeps_wool_of_its_colour_and_a_sheared_sheep_is_not_dyed() {
    let mut scene = holding(
        &World::default(),
        &[
            items::SHEEP_SPAWN_EGG,
            items::RED_DYE,
            items::SHEARS,
            items::WHITE_DYE,
        ],
    );
    let sheep = hatch(&mut scene, 0, floor_at(2, 2)).expect("a sheep");
    let dyed = use_on(&mut scene, 1, &[sheep]);
    assert_eq!(wool(&scene, sheep), RED);
    assert_eq!(carried(&scene, items::RED_DYE), 1);
    assert_eq!(
        played(&dyed, sounds::ITEM_DYE_USE),
        [Some(scene.actors[0].id)]
    );
    let shorn = use_on(&mut scene, 2, &[sheep]);
    assert_eq!(wool(&scene, sheep), RED | SHEARED);
    assert_eq!(played(&shorn, sounds::ENTITY_SHEEP_SHEAR).len(), 1);
    let lying = items_lying(&scene.all());
    assert!(
        (1..=3).contains(&lying.len()),
        "one to three items: {lying:?}"
    );
    for (stack, _) in &lying {
        assert_eq!(*stack, ItemStack::new(items::RED_WOOL, 1));
    }
    assert_eq!(carried(&scene, items::SHEARS), 1);
    use_on(&mut scene, 2, &[sheep]);
    assert_eq!(items_lying(&scene.all()).len(), lying.len(), "no more wool");
    use_on(&mut scene, 3, &[sheep]);
    assert_eq!(
        wool(&scene, sheep),
        RED | SHEARED,
        "a sheared sheep is not dyed"
    );
}

/// Brief 4.8.4–4.8.5: shears used on a mooshroom convert it into a cow where it stood
/// (`convertTo(COW)`; the mooshroom gone) and lay five red mushrooms, each a single item
/// (`shearing/mooshroom/red`), with `entity.mooshroom.shear`. The cow is an adult.
#[test]
fn shears_turn_a_mooshroom_into_a_cow_and_five_mushrooms() {
    let mut scene = holding(
        &World::default(),
        &[items::MOOSHROOM_SPAWN_EGG, items::SHEARS],
    );
    let mooshroom = hatch(&mut scene, 0, floor_at(2, 2)).expect("a mooshroom");
    scene.idle_for(2);
    let stood = the_entity(&scene, mooshroom).position;
    let shorn = use_on(&mut scene, 1, &[mooshroom]);
    assert_eq!(played(&shorn, sounds::ENTITY_MOOSHROOM_SHEAR).len(), 1);
    assert!(
        entity_of(&scene, mooshroom).is_none(),
        "the mooshroom is gone"
    );
    assert!(scene.of_kind(entity_types::MOOSHROOM).is_empty());
    let cows = scene.of_kind(entity_types::COW);
    assert_eq!(cows.len(), 1, "a cow in its place");
    assert!(
        horizontal(cows[0].position, stood) < 0.05,
        "{:?} {stood:?}",
        cows[0].position
    );
    assert!(!is_baby(&scene, cows[0].key));
    let lying = items_lying(&scene.all());
    assert_eq!(lying.len(), 5, "{lying:?}");
    for (stack, _) in &lying {
        assert_eq!(*stack, ItemStack::new(items::RED_MUSHROOM, 1));
    }
}

/// The equipment a tick showed of `key`, slot by slot, in its last report.
fn equipment(output: &TickOutput, key: EntityId) -> Option<Vec<(u8, Option<ItemStack>)>> {
    output.events.iter().rev().find_map(|event| match event {
        RegionEvent::EquipmentChanged { entity, slots, .. } if *entity == key => {
            Some(slots.clone())
        }
        _ => None,
    })
}

/// Brief 4.9.3: a saddle used on an adult pig equips its `SADDLE` slot (shown with
/// `SetEquipment`) with `entity.pig.saddle`, the creative player keeping theirs; a piglet
/// cannot use the slot (`canUseSlot(SADDLE)` is `isAlive() && !isBaby()`).
#[test]
fn a_saddle_goes_on_an_adult_pig_and_not_on_a_piglet() {
    let mut scene = holding(&World::default(), &[items::PIG_SPAWN_EGG, items::SADDLE]);
    let pig = hatch(&mut scene, 0, floor_at(2, 2)).expect("a pig");
    use_on(&mut scene, 0, &[pig]);
    let piglet = others_of(&scene, entity_types::PIG, &[pig])[0].key;
    let saddled = use_on(&mut scene, 1, &[pig]);
    let slots = equipment(&saddled, pig).expect("the pig's equipment is shown");
    assert!(
        slots.contains(&(SADDLE_SLOT, Some(ItemStack::new(items::SADDLE, 1)))),
        "{slots:?}"
    );
    assert_eq!(played(&saddled, sounds::ENTITY_PIG_SADDLE).len(), 1);
    assert_eq!(carried(&scene, items::SADDLE), 1, "the saddle is kept");
    let refused = use_on(&mut scene, 1, &[piglet]);
    let shown = equipment(&refused, piglet).unwrap_or_default();
    assert!(
        !shown.iter().any(|(_, stack)| stack.is_some()),
        "the piglet wears nothing: {shown:?}"
    );
}

/// Brief 4.11.1–4.11.2: a grown chicken lays an egg when its timer, drawn
/// `nextInt(6000) + 6000` at construction, runs out: within 12000 ticks, with
/// `entity.chicken.egg`. What it lays is `gameplay/chicken_lay`'s by its variant: a brown
/// egg for a warm chicken (the badlands), a blue egg for a cold one (snowy plains), an
/// egg for a temperate one (plains).
#[test]
fn a_chicken_lays_the_egg_of_its_variant_within_12000_ticks() {
    for (place, laid) in [
        ("minecraft:badlands", items::BROWN_EGG),
        ("minecraft:snowy_plains", items::BLUE_EGG),
        ("minecraft:plains", items::EGG),
    ] {
        let mut scene = holding(&world_in(place), &[items::CHICKEN_SPAWN_EGG]);
        hatch(&mut scene, 0, floor_at(2, 2)).expect("a chicken");
        // An egg laid near the player is soon taken: counted lying or carried.
        let eggs = |scene: &Scene| {
            let mut found: Vec<i32> = items_lying(&scene.all())
                .iter()
                .map(|(stack, _)| stack.item)
                .collect();
            for item in [items::EGG, items::BROWN_EGG, items::BLUE_EGG] {
                for _ in 0..carried(scene, item) {
                    found.push(item);
                }
            }
            found
        };
        let mut seen = Vec::new();
        assert!(
            scene.idle_until(12_001, &mut seen, |scene, _| !eggs(scene).is_empty()),
            "an egg in {place}"
        );
        assert_eq!(eggs(&scene), [laid], "{place}");
        assert_eq!(
            spawned(&seen, entity_types::ITEM),
            1,
            "{place}: one item laid"
        );
        let sounded: usize = seen
            .iter()
            .map(|output| played(output, sounds::ENTITY_CHICKEN_EGG).len())
            .sum();
        assert_eq!(sounded, 1, "{place}");
    }
}

/// Brief 4.0 item 16 and 4.7: shears used on a saddled pig take its saddle before the
/// pig's own use is asked (`Mob.attemptToShearEquipment`): the saddle dropped as an
/// item, the slot shown empty, `item.saddle.unequip` played; the shears kept.
#[test]
fn shears_take_the_saddle_off_a_pig() {
    let mut scene = holding(
        &World::default(),
        &[items::PIG_SPAWN_EGG, items::SADDLE, items::SHEARS],
    );
    let pig = hatch(&mut scene, 0, floor_at(2, 2)).expect("a pig");
    use_on(&mut scene, 1, &[pig]);
    let shorn = use_on(&mut scene, 2, &[pig]);
    let slots = equipment(&shorn, pig).expect("the pig's equipment is shown");
    assert!(slots.contains(&(SADDLE_SLOT, None)), "{slots:?}");
    assert_eq!(played(&shorn, sounds::ITEM_SADDLE_UNEQUIP).len(), 1);
    assert_eq!(
        lying_count(&scene.all(), items::SADDLE),
        1,
        "the saddle lies there"
    );
    assert_eq!(carried(&scene, items::SHEARS), 1);
}

/// Brief 4.10.4 and 4.8.4–4.8.5: shears do nothing to a lamb (`readyForShearing` is
/// `!isSheared() && !isBaby()`) or to a mooshroom's calf, and a bowl or a bucket gets
/// nothing of a calf.
#[test]
fn shears_a_bowl_and_a_bucket_get_nothing_of_a_baby() {
    let mut scene = holding(
        &World::default(),
        &[
            items::SHEEP_SPAWN_EGG,
            items::MOOSHROOM_SPAWN_EGG,
            items::SHEARS,
            items::BOWL,
            items::BUCKET,
        ],
    );
    let sheep = hatch(&mut scene, 0, floor_at(2, 2)).expect("a sheep");
    let mooshroom = hatch(&mut scene, 1, floor_at(-2, 2)).expect("a mooshroom");
    use_on(&mut scene, 0, &[sheep]);
    use_on(&mut scene, 1, &[mooshroom]);
    let lamb = others_of(&scene, entity_types::SHEEP, &[sheep])[0].key;
    let calf = others_of(&scene, entity_types::MOOSHROOM, &[mooshroom])[0].key;
    let colour = wool(&scene, lamb);
    use_on(&mut scene, 2, &[lamb, calf]);
    assert_eq!(wool(&scene, lamb), colour, "the lamb is not shorn");
    assert!(items_lying(&scene.all()).is_empty(), "nothing dropped");
    assert!(
        entity_of(&scene, calf).is_some(),
        "the calf is still a mooshroom"
    );
    assert!(scene.of_kind(entity_types::COW).is_empty());
    use_on(&mut scene, 3, &[calf]);
    use_on(&mut scene, 4, &[calf]);
    assert_eq!(carried(&scene, items::MUSHROOM_STEW), 0);
    assert_eq!(carried(&scene, items::MILK_BUCKET), 0);
}

/// Brief 4.0 item 5 and 4.8.5: the cow a mooshroom is sheared into gets no
/// `finalizeSpawn`, so it is the temperate cow (1, the default) even in the badlands,
/// where an egg's cow is warm.
#[test]
fn a_sheared_mooshrooms_cow_is_temperate_whatever_the_biome() {
    let mut scene = holding(
        &World::default(),
        &[items::MOOSHROOM_SPAWN_EGG, items::SHEARS],
    );
    let mooshroom = hatch(&mut scene, 0, floor_at(2, 2)).expect("a mooshroom");
    use_on(&mut scene, 1, &[mooshroom]);
    let cow = scene.of_kind(entity_types::COW)[0].key;
    let variant = shown(&scene, cow)
        .into_iter()
        .find(|(index, _, _)| *index == OWN_FIRST);
    assert_eq!(
        variant,
        Some((OWN_FIRST, DataShown::Holder(Holder::CowVariant, 1), true))
    );
}

/// `ItemUtils.createFilledResult` (ItemUtils.txt 17–74) for a creative player whose
/// inventory is full: milk (duplicates prevented) is only `add`ed, and what does not fit
/// is lost, nothing thrown; a stew (duplicates allowed) that does not fit is thrown from
/// the player (`player.drop`).
#[test]
fn a_full_inventory_loses_the_milk_and_throws_the_stew() {
    let mut slots: Vec<(usize, ItemStack)> =
        (3..36).map(|slot| (slot, stack(items::DIRT, 64))).collect();
    slots.push((0, stack(items::MOOSHROOM_SPAWN_EGG, 1)));
    slots.push((1, stack(items::BUCKET, 1)));
    slots.push((2, stack(items::BOWL, 1)));
    let mut world = World::default();
    world.blocks.extend(pen());
    let mut scene = Scene::alone(&world, inventory(&slots), SPAWN);
    let mooshroom = hatch(&mut scene, 0, floor_at(2, 2)).expect("a mooshroom");
    use_on(&mut scene, 1, &[mooshroom]);
    assert_eq!(carried(&scene, items::MILK_BUCKET), 0);
    assert_eq!(
        lying_count(&scene.all(), items::MILK_BUCKET),
        0,
        "no milk thrown"
    );
    use_on(&mut scene, 2, &[mooshroom]);
    assert_eq!(carried(&scene, items::MUSHROOM_STEW), 0);
    assert_eq!(
        lying_count(&scene.all(), items::MUSHROOM_STEW),
        1,
        "the stew thrown"
    );
}

/// Brief 4.10.5 and 4.0 item 21: a sheared sheep on grass eats it in time
/// (`EatBlockGoal`: entity event 10 when it lowers its head, the grass block below it
/// turned to dirt 36 ticks later) and `Sheep.ate()` grows its wool back, the sheared bit
/// cleared.
#[test]
fn a_sheared_sheep_eats_grass_and_grows_its_wool_back() {
    let mut grass = Vec::new();
    for x in -40..40 {
        for z in -40..40 {
            grass.push((bp(x, -61, z), blocks::GRASS_BLOCK));
        }
    }
    let world = World::of(&grass);
    let mut scene = holding(&world, &[items::SHEEP_SPAWN_EGG, items::SHEARS]);
    let sheep = hatch(&mut scene, 0, floor_at(2, 2)).expect("a sheep");
    let colour = wool(&scene, sheep);
    use_on(&mut scene, 1, &[sheep]);
    assert_eq!(wool(&scene, sheep), colour | SHEARED);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(20_000, &mut seen, |scene, _| wool(scene, sheep) & SHEARED
            == 0),
        "it grows its wool back"
    );
    assert_eq!(wool(&scene, sheep), colour, "of its colour");
    assert!(events_of(&seen, sheep, EATING) >= 1, "it lowered its head");
    let last = seen.last().expect("a tick");
    let eaten: Vec<BlockPos> = last
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } if *state == blocks::DIRT => Some(*position),
            _ => None,
        })
        .collect();
    assert_eq!(
        eaten.len(),
        1,
        "one grass block eaten in the tick: {eaten:?}"
    );
    let at = the_entity(&scene, sheep).position;
    assert_eq!(eaten[0].y, -61);
    assert!(
        horizontal(
            at,
            [
                f64::from(eaten[0].x) + 0.5,
                0.0,
                f64::from(eaten[0].z) + 0.5
            ]
        ) < 1.0,
        "the block under the sheep: {eaten:?} {at:?}"
    );
    assert_eq!(scene.block(eaten[0]), blocks::DIRT);
}

// ------------------------------------------------------------------------------------
// 7. What clients are shown (brief 9.4).

/// Brief 9.4: a baby shows index 16 true (not the default, so it is paired), an adult
/// false (the default); a sheep its wool byte at 18, the colour's id in the low four bits
/// and 0x10 when sheared; a pig its saddle as equipment and its boost at 18, 0 by
/// default.
#[test]
fn clients_are_shown_the_baby_flag_the_wool_and_the_saddle() {
    let mut scene = holding(
        &World::default(),
        &[
            items::SHEEP_SPAWN_EGG,
            items::RED_DYE,
            items::SHEARS,
            items::PIG_SPAWN_EGG,
            items::SADDLE,
        ],
    );
    let sheep = hatch(&mut scene, 0, floor_at(2, 2)).expect("a sheep");
    use_on(&mut scene, 0, &[sheep]);
    let lamb = others_of(&scene, entity_types::SHEEP, &[sheep])[0].key;
    let baby = |scene: &Scene, key| {
        shown(scene, key)
            .into_iter()
            .find(|(index, _, _)| *index == 16)
            .expect("index 16")
    };
    assert_eq!(baby(&scene, lamb), (16, DataShown::Boolean(true), false));
    assert_eq!(baby(&scene, sheep), (16, DataShown::Boolean(false), true));
    use_on(&mut scene, 1, &[sheep]);
    use_on(&mut scene, 2, &[sheep]);
    let byte = shown(&scene, sheep)
        .into_iter()
        .find(|(index, _, _)| *index == OWN_FIRST)
        .expect("index 18");
    assert_eq!(byte, (OWN_FIRST, DataShown::Byte(RED | SHEARED), false));

    let pig = hatch(&mut scene, 3, floor_at(-2, 2)).expect("a pig");
    let boost = shown(&scene, pig)
        .into_iter()
        .find(|(index, _, _)| *index == OWN_FIRST)
        .expect("index 18");
    assert_eq!(boost, (OWN_FIRST, DataShown::Int(0), true));
    let saddled = use_on(&mut scene, 4, &[pig]);
    assert!(
        equipment(&saddled, pig)
            .is_some_and(|slots| slots.contains(&(SADDLE_SLOT, Some(stack(items::SADDLE, 1))))),
        "the saddle is shown"
    );
}

// ------------------------------------------------------------------------------------
// 5. Across a pinned line (brief 11.4 item 1; plan 8.5, 9.4, 10.7).

/// Two regions on either side of the pinned line at x = 16, each with its player 1, run
/// a tick at a time as the store couples them: each takes the other's previous tick's
/// entity changes into its copies, its effects on its own land and its arrivals.
struct Line {
    west: Scene,
    east: Scene,
    from_west: TickOutput,
    from_east: TickOutput,
    sequence: u64,
}

impl Line {
    /// The two sides, the west's keys from block `west_block` and the east's from the
    /// other, the west's player at `west_feet` and the east's at `east_feet`, each holding
    /// `slots`.
    fn new(
        west_block: u32,
        west_feet: Vec3,
        east_feet: Vec3,
        slots: &[i32],
        world: &World,
    ) -> Self {
        let stacks: Vec<(usize, ItemStack)> = slots
            .iter()
            .enumerate()
            .map(|(slot, item)| (slot, stack(*item, 1)))
            .collect();
        let mut west_layout = Layout::side(true);
        let mut east_layout = Layout::side(false);
        west_layout.block = west_block;
        east_layout.block = 3 - west_block;
        let west = Scene::new(west_layout, world, inventory(&stacks), west_feet);
        let east = Scene::new(east_layout, world, inventory(&stacks), east_feet);
        Self {
            west,
            east,
            from_west: TickOutput::default(),
            from_east: TickOutput::default(),
            sequence: 1_000,
        }
    }

    /// What `into` takes of `from`'s tick.
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
        TickInputs {
            copy_entity_updates: updates,
            effects,
            arrivals,
            ..TickInputs::default()
        }
    }

    /// One tick of both, with the players' `west` and `east` inputs.
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

/// `inputs` of player 1 of `scene`, numbered on from their last.
fn numbered(
    scene: &mut Scene,
    inputs: Vec<PlayerInput>,
) -> Vec<(
    clustine_world::EdgeId,
    clustine_world::PlayerId,
    EntityId,
    u64,
    PlayerInput,
)> {
    let actor = &mut scene.actors[0];
    inputs
        .into_iter()
        .map(|input| {
            actor.input += 1;
            (EDGE, actor.id, actor.entity, actor.input, input)
        })
        .collect()
}

/// The breeding effects of a tick: requests (asker, partner) and answers (asked, partner,
/// accepted).
fn asks(output: &TickOutput) -> Vec<(EntityId, EntityId)> {
    output
        .effects
        .iter()
        .filter_map(|effect: &Effect| match effect.what {
            Effected::OnEntity {
                key,
                by: EntityUse::BreedAsk { asker, .. },
            } => Some((asker, key)),
            _ => None,
        })
        .collect()
}

fn answers(output: &TickOutput) -> Vec<(EntityId, EntityId, bool)> {
    output
        .effects
        .iter()
        .filter_map(|effect: &Effect| match effect.what {
            Effected::OnEntity {
                key,
                by:
                    EntityUse::BreedAnswer {
                        partner, accepted, ..
                    },
            } => Some((key, partner, accepted)),
            _ => None,
        })
        .collect()
}

/// Plan 9.4 and 10.7, brief 11.4 item 1: two cows in love on either side of a pinned
/// line, each seen by the other's region only in a copy, breed exactly once, whichever
/// side has the higher key: the higher key asks across the line (`BreedAsk`), the lower
/// key's region breeds as the game's `this` (the calf, the orb and the hearts at its
/// cow, in its region), and answers `accepted`; the asker's region makes no calf and,
/// by the answer, its cow's love is reset and its age set: neither cow takes food after,
/// and no second calf or orb comes. A wall on the west's side of the line keeps the
/// cows from walking over it (two cows that meet in one region breed there, which is
/// not what this tests); `BreedGoal`'s partner search ignores sight.
#[test]
fn two_animals_in_love_across_a_line_breed_once_whichever_region_asks() {
    let mut wall = Vec::new();
    for z in -32..48 {
        for y in [-60, -59] {
            wall.push((bp(15, y, z), blocks::STONE));
        }
    }
    let world = World::of(&wall);
    for west_block in [1, 2] {
        let mut line = Line::new(
            west_block,
            Vec3::new(12.5, -60.0, 4.5),
            Vec3::new(18.5, -60.0, 5.5),
            &[items::COW_SPAWN_EGG, items::WHEAT],
            &world,
        );
        let egg = |at: BlockPos| {
            vec![
                PlayerInput::SelectSlot { slot: 0 },
                PlayerInput::UseItemOn {
                    position: at,
                    face: Face::Top,
                    cursor: [0.5, 1.0, 0.5],
                    sneaking: false,
                    sequence: 1,
                },
            ]
        };
        line.step(egg(bp(14, -61, 4)), egg(bp(16, -61, 5)));
        let west_cow = line.west.of_kind(entity_types::COW)[0].key;
        let east_cow = line.east.of_kind(entity_types::COW)[0].key;
        line.idle();
        line.idle();
        let in_copy = |scene: &Scene, chunk: ChunkPos, key: EntityId| {
            scene
                .region
                .copy(chunk)
                .is_some_and(|copy| copy.entities().iter().any(|entity| entity.key == key))
        };
        assert!(
            in_copy(&line.west, ChunkPos::new(1, 0), east_cow),
            "the west sees the east's cow"
        );
        assert!(
            in_copy(&line.east, ChunkPos::new(0, 0), west_cow),
            "the east sees the west's cow"
        );
        let feed = |key| vec![PlayerInput::SelectSlot { slot: 1 }, used(key)];
        let (west_fed, east_fed) = line.step(feed(west_cow), feed(east_cow));
        assert_eq!(
            (hearts(&west_fed, west_cow), hearts(&east_fed, east_cow)),
            (1, 1)
        );

        let (asker, partner) = if west_cow > east_cow {
            (west_cow, east_cow)
        } else {
            (east_cow, west_cow)
        };
        let mut seen_asks = Vec::new();
        let mut seen_answers = Vec::new();
        let mut hearts_at_birth = Vec::new();
        let mut born = None;
        let mut seen = Vec::new();
        for _ in 0..500 {
            let (west_out, east_out) = line.idle();
            for output in [&west_out, &east_out] {
                seen_asks.extend(asks(output));
                seen_answers.extend(answers(output));
                assert!(
                    output.arrivals.is_empty(),
                    "nothing crosses: {:?}",
                    output.arrivals
                );
            }
            seen.push(west_out.clone());
            seen.push(east_out.clone());
            let calves: Vec<(bool, Entity)> =
                others_of(&line.west, entity_types::COW, &[west_cow, east_cow])
                    .into_iter()
                    .map(|calf| (true, calf))
                    .chain(
                        others_of(&line.east, entity_types::COW, &[west_cow, east_cow])
                            .into_iter()
                            .map(|calf| (false, calf)),
                    )
                    .collect();
            if !calves.is_empty() {
                assert_eq!(calves.len(), 1, "one calf: {calves:?}");
                hearts_at_birth = vec![
                    hearts(&west_out, west_cow) + hearts(&east_out, west_cow),
                    hearts(&west_out, east_cow) + hearts(&east_out, east_cow),
                ];
                born = Some(calves[0].clone());
                break;
            }
        }
        let (in_west, calf) = born.expect("a calf within 500 ticks");
        let partner_in_west = partner == west_cow;
        assert_eq!(
            in_west, partner_in_west,
            "block {west_block}: the calf is born in the lower key's region"
        );
        let breeder = if partner_in_west {
            &line.west
        } else {
            &line.east
        };
        let parent = the_entity(breeder, partner).position;
        assert!(
            horizontal(calf.position, parent) < 0.5,
            "at the lower key's cow: {:?} {parent:?}",
            calf.position
        );
        assert_eq!(
            hearts_at_birth[usize::from(partner == east_cow)],
            1,
            "the hearts on the breeding cow"
        );
        assert_eq!(
            hearts_at_birth[usize::from(partner == west_cow)],
            0,
            "none on the asker"
        );
        assert!(
            seen_asks
                .iter()
                .all(|(who, to)| (*who, *to) == (asker, partner)),
            "{seen_asks:?}"
        );
        assert!(!seen_asks.is_empty(), "the higher key asks across the line");
        for _ in 0..5 {
            let (west_out, east_out) = line.idle();
            seen_answers.extend(answers(&west_out));
            seen_answers.extend(answers(&east_out));
            seen.push(west_out);
            seen.push(east_out);
        }
        assert_eq!(
            seen_answers
                .iter()
                .filter(|(_, _, accepted)| *accepted)
                .count(),
            1,
            "accepted once: {seen_answers:?}"
        );
        assert!(
            seen_answers
                .iter()
                .all(|(asked, from, _)| (*asked, *from) == (asker, partner)),
            "{seen_answers:?}"
        );
        let mut breeders_orbs = 0;
        for _ in 0..300 {
            let (west_out, east_out) = line.idle();
            seen.push(west_out);
            seen.push(east_out);
        }
        for (n, output) in seen.iter().enumerate() {
            if (n % 2 == 0) == partner_in_west {
                breeders_orbs +=
                    spawned(std::slice::from_ref(output), entity_types::EXPERIENCE_ORB);
            }
        }
        let calves = others_of(&line.west, entity_types::COW, &[west_cow, east_cow]).len()
            + others_of(&line.east, entity_types::COW, &[west_cow, east_cow]).len();
        assert_eq!(calves, 1, "block {west_block}: no second calf");
        assert_eq!(
            spawned(&seen, entity_types::COW),
            1,
            "block {west_block}: one calf made"
        );
        assert_eq!(spawned(&seen, entity_types::EXPERIENCE_ORB), 1, "one orb");
        assert_eq!(breeders_orbs, 1, "the orb in the breeding region");
        let (west_fed, east_fed) = line.step(feed(west_cow), feed(east_cow));
        assert_eq!(
            (hearts(&west_fed, west_cow), hearts(&east_fed, east_cow)),
            (0, 0),
            "block {west_block}: both cows' ages were set, the asker's by the answer"
        );
    }
}

// ------------------------------------------------------------------------------------
// 8. Determinism (ADR-0028; `clustine-sim` is a function of its inputs).

/// Two regions given the same inputs, with cows hatched, fed, breeding and a calf fed,
/// report the same changes and events every tick.
#[test]
fn two_regions_given_the_same_inputs_commit_the_same_breeding() {
    let mut scenes: Vec<Scene> = (0..2)
        .map(|_| holding(&World::default(), &[items::COW_SPAWN_EGG, items::WHEAT]))
        .collect();
    let mut keys = Vec::new();
    for scene in &mut scenes {
        let a = hatch(scene, 0, floor_at(2, 2)).expect("a cow");
        let b = hatch(scene, 0, floor_at(2, 4)).expect("a cow");
        keys.push([a, b]);
    }
    assert_eq!(keys[0], keys[1]);
    let [a, b] = keys[0];
    let mut outputs: [Vec<TickOutput>; 2] = [Vec::new(), Vec::new()];
    for (scene, seen) in scenes.iter_mut().zip(&mut outputs) {
        seen.push(use_on(scene, 1, &[a, b]));
        seen.extend(scene.idle_for(300));
        let calves = others_of(scene, entity_types::COW, &[a, b]);
        assert_eq!(calves.len(), 1, "they bred");
        seen.push(use_on(scene, 1, &[calves[0].key]));
        seen.extend(scene.idle_for(100));
    }
    for (tick, (one, other)) in outputs[0].iter().zip(&outputs[1]).enumerate() {
        assert_eq!(one.entity_changes, other.entity_changes, "tick {tick}");
        assert_eq!(one.events, other.events, "tick {tick}");
    }
    assert_eq!(scenes[0].all(), scenes[1].all());
}
