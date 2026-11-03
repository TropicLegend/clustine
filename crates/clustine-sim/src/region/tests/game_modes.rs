//! What a player's game mode changes of what they do (`docs/groundwork/v-plan.md`,
//! sections 3.1, 4.1 and 4.6, step V1; `region/materials.rs`), from the game's rules: a
//! survival player's placed block, poured and filled bucket, egg, bone meal, food and
//! fuel are used up and their tools worn, where a creative player's are not; reach is the
//! player's attributes with creative's modifiers; a pick from an entity puts nothing new
//! in a survival player's hotbar; a monster near keeps a survival player awake; and what
//! a survival player picks up stays on the ground where it finds no room.

use clustine_data::{BlockState, entity_types, items};
use clustine_world::components::{Component, Kind};
use clustine_world::{Entity, EntityChange, EntityId, GameMode, PlayerMode};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::types::animal::{self, Animal, Own};

const COW: EntityId = EntityId(40);
const SHEEP: EntityId = EntityId(41);
const ZOMBIE: EntityId = EntityId(42);

fn stacked(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

/// The region of `on_ticking_floor_with(blocks)`, player 1 at the spawn in `mode`,
/// holding `held` in the first slots of the hotbar, slot 0 selected, and nothing else.
fn as_player(
    mode: GameMode,
    blocks: &[(BlockPos, BlockState)],
    held: &[Option<ItemStack>],
) -> Region {
    let mut region = on_ticking_floor_with(blocks);
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.mode = PlayerMode::new(mode);
    me.inventory = Inventory::default();
    for (slot, stack) in held.iter().enumerate() {
        me.inventory.set(slot, stack.clone());
    }
    me.selected_slot = 0;
    region
}

fn slot(region: &Region, slot: usize) -> Option<ItemStack> {
    region.players[&player(1)].inventory.0[slot].clone()
}

fn told_slots(output: &TickOutput) -> Vec<(u8, Option<ItemStack>)> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::InventorySlot { slot, stack } => Some((*slot, stack.clone())),
            _ => None,
        })
        .collect()
}

fn wear(stack: &Option<ItemStack>) -> i32 {
    match stack
        .as_ref()
        .and_then(|stack| stack.components.get(Kind::Damage))
    {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    }
}

fn click(region: &mut Region, x: i32, y: i32, z: i32, face: Face) -> TickOutput {
    region.tick(&moves(vec![place(1, x, y, z, face)]))
}

/// Player 1 uses what they hold in the air, looking `pitch` degrees down.
fn use_in_air(region: &mut Region, pitch: f32) -> TickOutput {
    let input = PlayerInput::UseItem {
        rotation: (0.0, pitch),
        sneaking: false,
        sequence: 1,
    };
    region.tick(&moves(vec![numbered(1, input)]))
}

fn use_entity(region: &mut Region, key: EntityId) -> TickOutput {
    let input = PlayerInput::Interact {
        entity: key,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    };
    region.tick(&moves(vec![numbered(1, input)]))
}

/// An animal of the type `kind` at `position`, standing, grown, of its full health.
fn animal_at<O: Own>(key: EntityId, kind: i32, position: [f64; 3], own: &O) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .unwrap_or(10.0) as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = animal::write(&Animal::default(), own);
    mob_entity(key, kind, position, mob)
}

fn mob_entity(key: EntityId, kind: u16, position: [f64; 3], mob: Mob) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
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

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn cow_at(x: f64) -> Entity {
    animal_at(
        COW,
        entity_types::COW,
        [x, -60.0, 0.5],
        &crate::mobs::types::cow::CowState::default(),
    )
}

/// Whether `output` shows the cow `key` falling in love (entity event 18).
fn in_love(output: &TickOutput, key: EntityId) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityEvent { entity, event: 18, .. } if *entity == key)
    })
}

fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

#[test]
fn a_survival_player_s_placed_block_is_taken_from_the_stack_and_a_creative_player_s_is_not() {
    let mut survival = as_player(GameMode::Survival, &[], &[stacked(items::STONE, 5)]);
    let output = click(&mut survival, 2, -61, 0, Face::Top);
    assert_eq!(
        survival.block(BlockPos::new(2, -60, 0)),
        Some(blocks::STONE)
    );
    assert_eq!(slot(&survival, 0), stacked(items::STONE, 4));
    assert_eq!(told_slots(&output), [(0, stacked(items::STONE, 4))]);

    // The last of the stack leaves the slot empty.
    let mut last = as_player(GameMode::Survival, &[], &[stacked(items::STONE, 1)]);
    click(&mut last, 2, -61, 0, Face::Top);
    assert_eq!(slot(&last, 0), None);

    let mut creative = as_player(GameMode::Creative, &[], &[stacked(items::STONE, 5)]);
    let output = click(&mut creative, 2, -61, 0, Face::Top);
    assert_eq!(
        creative.block(BlockPos::new(2, -60, 0)),
        Some(blocks::STONE)
    );
    assert_eq!(slot(&creative, 0), stacked(items::STONE, 5));
    assert!(told_slots(&output).is_empty());
}

/// The floor's top at x is `(x − 0.5)² + 1.62²` from the eyes squared: within 5.5 (4.5
/// and the block of tolerance) up to x = 5 for a survival player, within 6 (creative's
/// 0.5 more) up to x = 6 for a creative one (`isWithinBlockInteractionRange`).
#[test]
fn a_block_is_reached_within_the_player_s_block_interaction_range_and_a_block_of_tolerance() {
    for (mode, reached) in [(GameMode::Survival, 5), (GameMode::Creative, 6)] {
        let mut region = as_player(mode, &[], &[stacked(items::STONE, 5)]);
        click(&mut region, reached, -61, 0, Face::Top);
        assert_eq!(
            region.block(BlockPos::new(reached, -60, 0)),
            Some(blocks::STONE),
            "{mode:?} reaches x = {reached}"
        );
        click(&mut region, reached + 1, -61, 0, Face::Top);
        assert_eq!(
            region.block(BlockPos::new(reached + 1, -60, 0)),
            Some(blocks::AIR),
            "{mode:?} does not reach x = {}",
            reached + 1
        );
    }
}

/// A cow's box is 0.9 wide: at x it is `x − 0.95` from the eyes sideways and 0.22 above
/// them, within 6 (3 and the handler's 3) for a survival player at x = 6.5 and not at
/// 7.5, within 8 (creative's 2 more) for a creative one at 8.5 and not at 9.5
/// (`isWithinEntityInteractionRange`).
#[test]
fn an_entity_is_reached_within_the_player_s_entity_interaction_range_and_three_of_tolerance() {
    for (mode, within, beyond) in [
        (GameMode::Survival, 6.5, 7.5),
        (GameMode::Creative, 8.5, 9.5),
    ] {
        let mut region = as_player(mode, &[], &[stacked(items::WHEAT, 4)]);
        lay(&mut region, &cow_at(beyond));
        let output = use_entity(&mut region, COW);
        assert!(
            !in_love(&output, COW),
            "{mode:?} does not reach a cow at {beyond}"
        );
        assert_eq!(slot(&region, 0), stacked(items::WHEAT, 4));
        let mut region = as_player(mode, &[], &[stacked(items::WHEAT, 4)]);
        lay(&mut region, &cow_at(within));
        let output = use_entity(&mut region, COW);
        assert!(in_love(&output, COW), "{mode:?} reaches a cow at {within}");
        // `usePlayerItem`: a survival player's wheat is eaten.
        let left = if mode == GameMode::Survival { 3 } else { 4 };
        assert_eq!(slot(&region, 0), stacked(items::WHEAT, left));
    }
}

#[test]
fn a_survival_player_s_water_bucket_empties_to_a_bucket_which_fills_again_in_their_hand() {
    let feet = BlockPos::new(0, -60, 0);
    let mut region = as_player(GameMode::Survival, &[], &[stacked(items::WATER_BUCKET, 1)]);
    use_in_air(&mut region, 90.0);
    assert!(
        region
            .block(feet)
            .is_some_and(clustine_blocks::data::has_water)
    );
    assert_eq!(slot(&region, 0), stacked(items::BUCKET, 1));
    use_in_air(&mut region, 90.0);
    assert_eq!(region.block(feet), Some(blocks::AIR));
    assert_eq!(slot(&region, 0), stacked(items::WATER_BUCKET, 1));

    // Of a stack of empty buckets, one is used and the full one goes elsewhere.
    let mut region = as_player(
        GameMode::Survival,
        &[(
            feet,
            BlockState::parse("minecraft:water[level=0]").expect("water"),
        )],
        &[stacked(items::BUCKET, 3)],
    );
    use_in_air(&mut region, 90.0);
    assert_eq!(slot(&region, 0), stacked(items::BUCKET, 2));
    assert_eq!(slot(&region, 1), stacked(items::WATER_BUCKET, 1));
}

#[test]
fn a_creative_player_s_water_bucket_stays_full_and_an_empty_one_adds_a_full_one_beside_it() {
    let feet = BlockPos::new(0, -60, 0);
    let mut region = as_player(GameMode::Creative, &[], &[stacked(items::WATER_BUCKET, 1)]);
    use_in_air(&mut region, 90.0);
    assert!(
        region
            .block(feet)
            .is_some_and(clustine_blocks::data::has_water)
    );
    assert_eq!(slot(&region, 0), stacked(items::WATER_BUCKET, 1));

    let mut region = as_player(
        GameMode::Creative,
        &[(
            feet,
            BlockState::parse("minecraft:water[level=0]").expect("water"),
        )],
        &[stacked(items::BUCKET, 1)],
    );
    use_in_air(&mut region, 90.0);
    assert_eq!(slot(&region, 0), stacked(items::BUCKET, 1));
    assert_eq!(slot(&region, 1), stacked(items::WATER_BUCKET, 1));
}

#[test]
fn a_survival_player_s_spawn_egg_is_used_up_and_a_creative_player_s_is_not() {
    for (mode, left) in [(GameMode::Survival, 1), (GameMode::Creative, 2)] {
        let mut region = as_player(mode, &[], &[stacked(items::COW_SPAWN_EGG, 2)]);
        click(&mut region, 2, -61, 0, Face::Top);
        assert_eq!(all_of(&region, entity_types::COW).len(), 1, "{mode:?}");
        assert_eq!(
            slot(&region, 0),
            stacked(items::COW_SPAWN_EGG, left),
            "{mode:?}"
        );
    }
}

#[test]
fn a_survival_player_s_bone_meal_is_used_up_on_a_crop_and_a_creative_player_s_is_not() {
    let soil = (
        BlockPos::new(2, -61, 0),
        BlockState::parse("minecraft:farmland[moisture=7]").expect("farmland"),
    );
    let crop = (
        BlockPos::new(2, -60, 0),
        BlockState::parse("minecraft:wheat[age=0]").expect("wheat"),
    );
    for (mode, left) in [(GameMode::Survival, 2), (GameMode::Creative, 3)] {
        let mut region = as_player(mode, &[soil, crop], &[stacked(items::BONE_MEAL, 3)]);
        click(&mut region, 2, -60, 0, Face::Top);
        assert_ne!(
            region.block(crop.0),
            Some(crop.1),
            "{mode:?}: the wheat grew"
        );
        assert_eq!(
            slot(&region, 0),
            stacked(items::BONE_MEAL, left),
            "{mode:?}"
        );
    }
}

#[test]
fn a_survival_player_s_flint_and_steel_wears_as_it_sets_fire_and_a_creative_player_s_does_not() {
    for (mode, worn) in [(GameMode::Survival, 1), (GameMode::Creative, 0)] {
        let mut region = as_player(mode, &[], &[stacked(items::FLINT_AND_STEEL, 1)]);
        click(&mut region, 2, -61, 0, Face::Top);
        assert!(
            region
                .block(BlockPos::new(2, -60, 0))
                .is_some_and(|state| clustine_blocks::data::same_block(state, blocks::FIRE)),
            "{mode:?}"
        );
        assert_eq!(wear(&slot(&region, 0)), worn, "{mode:?}");
    }
    // A fire charge is used up.
    let mut region = as_player(GameMode::Survival, &[], &[stacked(items::FIRE_CHARGE, 2)]);
    click(&mut region, 2, -61, 0, Face::Top);
    assert_eq!(slot(&region, 0), stacked(items::FIRE_CHARGE, 1));
}

#[test]
fn a_survival_player_s_shears_wear_by_one_on_a_sheep_and_a_creative_player_s_do_not() {
    for (mode, worn) in [(GameMode::Survival, 1), (GameMode::Creative, 0)] {
        let mut region = as_player(mode, &[], &[stacked(items::SHEARS, 1)]);
        let sheep = animal_at(
            SHEEP,
            entity_types::SHEEP,
            [2.5, -60.0, 0.5],
            &crate::mobs::types::sheep::SheepState::default(),
        );
        lay(&mut region, &sheep);
        use_entity(&mut region, SHEEP);
        let (_, sheared) = animal::read::<crate::mobs::types::sheep::SheepState>(
            &Mob::decode(&region.find_entity(SHEEP).expect("the sheep").1.data)
                .expect("a mob")
                .state,
        );
        assert!(sheared.sheared(), "{mode:?}");
        assert_eq!(wear(&slot(&region, 0)), worn, "{mode:?}");
    }
}

/// `Player.canEat`: a survival player's food is full until V4, so they eat only what is
/// always eaten; a creative player, invulnerable, eats anything. What is eaten is used up
/// but by a creative player.
#[test]
fn a_survival_player_eats_only_what_is_always_eaten_and_it_is_used_up() {
    let mut region = as_player(GameMode::Survival, &[], &[stacked(items::BREAD, 2)]);
    use_in_air(&mut region, 0.0);
    assert_eq!(region.players[&player(1)].using, None);

    let mut region = as_player(GameMode::Creative, &[], &[stacked(items::BREAD, 2)]);
    use_in_air(&mut region, 0.0);
    assert!(region.players[&player(1)].using.is_some());
    for _ in 0..40 {
        idle(&mut region);
    }
    assert_eq!(slot(&region, 0), stacked(items::BREAD, 2));

    let mut region = as_player(GameMode::Survival, &[], &[stacked(items::GOLDEN_APPLE, 2)]);
    use_in_air(&mut region, 0.0);
    assert!(region.players[&player(1)].using.is_some());
    for _ in 0..40 {
        idle(&mut region);
    }
    assert_eq!(region.players[&player(1)].using, None);
    assert_eq!(slot(&region, 0), stacked(items::GOLDEN_APPLE, 1));
}

/// A painting hung uses up the item, but a creative player's; a pick of it then puts a
/// painting into a creative player's hotbar, and nothing into a survival player's who
/// has none (`tryPickItem`'s `addAndPickItem` is for infinite materials).
#[test]
fn a_pick_from_an_entity_puts_nothing_new_in_a_survival_player_s_hotbar() {
    let wall = (BlockPos::new(3, -60, 0), blocks::STONE);
    for mode in [GameMode::Survival, GameMode::Creative] {
        let mut region = as_player(mode, &[wall], &[stacked(items::PAINTING, 1)]);
        click(&mut region, 3, -60, 0, Face::West);
        let painting = all_of(&region, entity_types::PAINTING);
        assert_eq!(painting.len(), 1, "{mode:?}: hung");
        let kept = if mode == GameMode::Survival {
            None
        } else {
            stacked(items::PAINTING, 1)
        };
        assert_eq!(slot(&region, 0), kept, "{mode:?}");
        region
            .players
            .get_mut(&player(1))
            .expect("here")
            .inventory
            .set(0, None);
        let input = PlayerInput::PickItemFromEntity {
            entity: painting[0].key,
        };
        region.tick(&moves(vec![numbered(1, input)]));
        let picked = if mode == GameMode::Survival {
            None
        } else {
            stacked(items::PAINTING, 1)
        };
        assert_eq!(slot(&region, 0), picked, "{mode:?}");
    }
}

/// A red bed whose head is at (2, -60, 0) and foot at (2, -60, 1), facing north, used at
/// night with a zombie four blocks away (B 1.5 step 8): it keeps a survival player
/// awake, and a creative one lies down.
#[test]
fn a_monster_near_the_bed_keeps_a_survival_player_awake_and_not_a_creative_one() {
    let bed = |part: &str| {
        BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).expect("a bed")
    };
    let head = BlockPos::new(2, -60, 0);
    let foot = BlockPos::new(2, -60, 1);
    for mode in [GameMode::Survival, GameMode::Creative] {
        let mut region = as_player(mode, &[(head, bed("head")), (foot, bed("foot"))], &[]);
        region.clock.day = 13_000;
        let zombie = mob_entity(
            ZOMBIE,
            entity_types::ZOMBIE as u16,
            [6.5, -60.0, 0.5],
            Mob::fresh(),
        );
        lay(&mut region, &zombie);
        let output = click(&mut region, foot.x, foot.y, foot.z, Face::Top);
        let not_safe = output.player_events.iter().any(|(_, event)| {
            matches!(event, PlayerEvent::Message { key, .. } if key == "block.minecraft.bed.not_safe")
        });
        let asleep = region.players[&player(1)].sleeping.is_some();
        match mode {
            GameMode::Survival => assert!(not_safe && !asleep),
            _ => assert!(!not_safe && asleep),
        }
    }
}

/// `ItemEntity.playerTouch`: what finds no room in a survival player's inventory stays
/// on the ground, the rest of a stack part of which went in too; a creative player takes
/// every stack and destroys what finds no room (n6 Inventory.add @63-87).
#[test]
fn what_finds_no_room_stays_on_the_ground_but_a_creative_player_destroys_a_stack_with_none() {
    for mode in [GameMode::Survival, GameMode::Creative] {
        let mut region = as_player(mode, &[], &[]);
        {
            let me = region.players.get_mut(&player(1)).expect("here");
            for slot in 0..36 {
                me.inventory.set(slot, stacked(items::DIRT, 64));
            }
            me.inventory.set(0, stacked(items::STONE, 60));
        }
        let mut output = TickOutput::default();
        assert!(region.spawn_item(
            [0.5, -60.0, 0.5],
            ItemStack::new(items::STONE, 10),
            &mut output
        ));
        let mut output = TickOutput::default();
        assert!(region.spawn_item(
            [0.5, -60.0, 0.5],
            ItemStack::new(items::SAND, 3),
            &mut output
        ));
        for _ in 0..20 {
            idle(&mut region);
        }
        assert_eq!(slot(&region, 0), stacked(items::STONE, 64), "{mode:?}");
        let lying: Vec<ItemStack> = all_of(&region, entity_types::ITEM)
            .into_iter()
            .filter_map(|entity| match entity.data {
                clustine_world::EntityData::Item { stack, .. } => Some(stack),
                _ => None,
            })
            .collect();
        let mut kinds: Vec<(i32, i32)> = lying
            .iter()
            .map(|stack| (stack.item, stack.count))
            .collect();
        kinds.sort_unstable();
        let mut expected = Vec::new();
        if mode == GameMode::Survival {
            expected.extend([(items::STONE, 6), (items::SAND, 3)]);
        }
        expected.sort_unstable();
        assert_eq!(kinds, expected, "{mode:?}");
    }
}

/// `ItemUtils.createFilledResult`: a cow milked into a survival player's only bucket
/// leaves the milk in their hand, and into one of a stack, the milk beside it; a
/// creative player's bucket stays, with the milk added where they have none.
#[test]
fn a_cow_milked_fills_one_of_a_survival_player_s_buckets_and_none_of_a_creative_player_s() {
    for (mode, buckets, hand, beside) in [
        (GameMode::Survival, 1, stacked(items::MILK_BUCKET, 1), None),
        (
            GameMode::Survival,
            3,
            stacked(items::BUCKET, 2),
            stacked(items::MILK_BUCKET, 1),
        ),
        (
            GameMode::Creative,
            1,
            stacked(items::BUCKET, 1),
            stacked(items::MILK_BUCKET, 1),
        ),
    ] {
        let mut region = as_player(mode, &[], &[stacked(items::BUCKET, buckets)]);
        lay(&mut region, &cow_at(2.5));
        use_entity(&mut region, COW);
        assert_eq!(slot(&region, 0), hand, "{mode:?} with {buckets}");
        assert_eq!(slot(&region, 1), beside, "{mode:?} with {buckets}");
    }
}
