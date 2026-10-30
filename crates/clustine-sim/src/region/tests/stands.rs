//! Tests of armour stands (step N5d, `docs/groundwork/n5-plan.md`, decision 4 and
//! section 8 item 7; `docs/groundwork/n5-brief.md`, section 5). The numbers each test
//! expects are worked out here from the game's formulas as its bytecode has them, not by
//! calling the code under test.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), facing south (yaw 0), on the stone
//! floor whose top is y = -60; only chunk (0, 0) ticks, its neighbours are loaded.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{
    Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityField,
    EntityUse, FiledEffect, Float,
};

use super::*;
use crate::living::slots;
use crate::region::stands::{DEFAULT_POSE, clicked_slot, placed_yaw};

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

const STAND_ITEM: ItemStack = ItemStack::new(items::ARMOR_STAND, 1);

const HELMET: ItemStack = ItemStack::new(items::DIAMOND_HELMET, 1);

const CHESTPLATE: ItemStack = ItemStack::new(items::DIAMOND_CHESTPLATE, 1);

const BOOTS: ItemStack = ItemStack::new(items::IRON_BOOTS, 1);

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

/// A stand at `position`, at rest, a tick old, wearing `equipment`.
fn stand(n: u64, position: [f64; 3], equipment: Vec<(u8, ItemStack)>) -> Entity {
    Entity {
        portal: Default::default(),
        key: key(n),
        kind: entity_types::ARMOR_STAND as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new(DEFAULT_POSE.map(|part| part.map(Float))),
            equipment,
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    }
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

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

/// The stands of the loaded chunks.
fn stands(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| matches!(entity.data, EntityData::ArmorStand { .. }))
        .cloned()
        .collect()
}

/// The stacks of the items lying in the loaded chunks, and where.
fn items_lying(region: &Region) -> Vec<(ItemStack, [f64; 3])> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some((stack.clone(), entity.position)),
            _ => None,
        })
        .collect()
}

fn worn(entity: &Entity) -> Vec<(u8, ItemStack)> {
    match &entity.data {
        EntityData::ArmorStand { equipment, .. } => equipment.clone(),
        _ => panic!("not a stand: {entity:?}"),
    }
}

/// The sounds of `output`, by id.
fn sounds_of(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

/// Player 1 holding `stack` in the hotbar's first slot.
fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

/// Player 1's click on the stand `target` at `height` above its feet.
fn click_on(target: &Entity, height: f64) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: target.key,
            location: [0.0, height, 0.0],
            sneaking: false,
        },
    )])
}

/// What player 1 holds in their hand.
fn in_hand(region: &Region) -> Option<ItemStack> {
    region.players.get(&player(1)).unwrap().held()
}

// Placing.

#[test]
fn a_stand_is_turned_to_face_the_player_to_the_nearest_eighth_of_a_turn() {
    // `floor((wrapDegrees(yaw - 180) + 22.5) / 45) * 45`: south-facing, the stand faces
    // north, and the eighths are cut at their middles.
    assert_eq!(placed_yaw(0.0), -180.0);
    assert_eq!(placed_yaw(30.0), -135.0);
    assert_eq!(placed_yaw(-100.0), 90.0);
    assert_eq!(placed_yaw(202.4), 0.0);
    assert_eq!(placed_yaw(202.6), 45.0);
}

#[test]
fn a_stand_is_placed_on_the_clicked_top_and_the_creative_player_keeps_the_item() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, STAND_ITEM);
    let output = checked(&mut region, &moves(vec![place(1, 3, -61, 3, Face::Top)]));
    let [placed] = &stands(&region)[..] else {
        panic!("{:?}", stands(&region));
    };
    // The stone gives no way: the stand goes into the block above it, at its bottom
    // centre, resting on the stone (`getYOffset` is 0 there), facing the player.
    assert_eq!(placed.position, [3.5, -60.0, 3.5]);
    assert_eq!(placed.yaw, -180.0);
    // Made before the entities' phase, it ticked in the tick it was made.
    assert_eq!(placed.lived, 1);
    let EntityData::ArmorStand {
        flags,
        health,
        equipment,
        air,
        ..
    } = &placed.data
    else {
        unreachable!();
    };
    assert_eq!((*flags, health.0, equipment.len(), *air), (0, 20.0, 0, 300));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_ARMOR_STAND_PLACE));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntitySpawned(state) if state.entity == placed.key
    )));
    assert_eq!(in_hand(&region), Some(STAND_ITEM));
}

#[test]
fn a_stand_placed_on_a_slab_stands_on_its_top_and_one_in_grass_replaces_its_place() {
    let slab = BlockState::parse("minecraft:stone_slab[type=bottom,waterlogged=false]").unwrap();
    let grass = BlockState::parse("minecraft:short_grass").unwrap();
    let mut region = on_ticking_floor_with(&[
        (BlockPos::new(3, -60, 3), slab),
        (BlockPos::new(5, -60, 2), grass),
    ]);
    holding(&mut region, STAND_ITEM);
    checked(
        &mut region,
        &moves(vec![
            place(1, 3, -60, 3, Face::Top),
            place(1, 5, -60, 2, Face::Top),
        ]),
    );
    let mut placed: Vec<[f64; 3]> = stands(&region).iter().map(|stand| stand.position).collect();
    placed.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Above the slab, the stand's box a block up falls 1.5 onto the slab's top: an
    // offset of -0.5 from the block above the slab. The grass gives way, and the stand
    // stands where it was, on the floor.
    assert_eq!(placed, [[3.5, -59.5, 3.5], [5.5, -60.0, 2.5]]);
}

#[test]
fn a_stand_is_not_placed_on_an_underside_nor_where_an_item_or_a_block_is_in_the_way() {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(5, -59, 3), blocks::STONE)]);
    holding(&mut region, STAND_ITEM);
    // An item lies where the stand would go, at (3.5, -60, 3.5).
    let mut lying = stand(9, [3.5, -60.0, 3.5], Vec::new());
    lying.kind = entity_types::ITEM as u16;
    lying.data = EntityData::Item {
        stack: STAND_ITEM,
        pick_up_delay: 32_767,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    lay(&mut region, &lying);
    checked(
        &mut region,
        &moves(vec![
            place(1, 3, -61, 3, Face::Top),
            // The stone at (5, -59, 3) is in the box of a stand on (5, -61, 3).
            place(1, 5, -61, 3, Face::Top),
            place(1, 2, -59, 2, Face::Bottom),
        ]),
    );
    assert!(stands(&region).is_empty(), "{:?}", stands(&region));
}

// The reduced living body.

#[test]
fn a_stand_dropped_from_a_height_falls_as_the_game_s_living_entities_do_and_lands_with_its_sounds()
{
    let mut region = on_ticking_floor_with(&[]);
    let start = -50.0;
    let dropped = stand(1, [8.5, start, 8.5], Vec::new());
    lay(&mut region, &dropped);
    // Each tick: motion under 0.003 is zeroed, the move goes as far as the floor lets
    // it, gravity takes 0.08 and the air's drag a fiftieth (`0.98f`), the friction of
    // the air 1 sideways.
    let drag = f64::from(0.98_f32);
    let (mut y, mut motion) = (start, 0.0_f64);
    let mut fallen = 0.0_f64;
    for tick in 1..200 {
        if motion.abs() < 0.003 {
            motion = 0.0;
        }
        let moved = motion.max(-60.0 - y);
        y += moved;
        if moved < 0.0 {
            fallen -= f64::from(moved as f32);
        }
        let landed = moved != motion;
        motion = ((if landed { 0.0 } else { motion }) - 0.08) * drag;
        let output = checked(&mut region, &TickInputs::default());
        let now = record(&region, dropped.key).unwrap();
        assert_eq!(now.position[1].to_bits(), y.to_bits(), "tick {tick}");
        assert_eq!(now.motion[1].to_bits(), motion.to_bits(), "tick {tick}");
        if landed {
            assert!(now.on_ground);
            assert_eq!(now.fall_distance, 0.0);
            // Ten blocks are seven beyond the safe three: the stand's fall sound and
            // the stone's, and no damage to it.
            assert!(fallen > 9.9 && fallen < 10.1, "{fallen}");
            let heard = sounds_of(&output);
            assert_eq!(heard.len(), 2, "{heard:?}");
            assert_eq!(heard[0], sounds::ENTITY_ARMOR_STAND_FALL);
            let EntityData::ArmorStand { health, .. } = now.data else {
                unreachable!();
            };
            assert_eq!(health.0, 20.0);
            return;
        }
        assert!(!now.on_ground, "tick {tick}");
    }
    panic!("the stand never landed");
}

/// A region whose chunk (0, 0) has still water three deep over its floor at x and z of
/// 7 to 9.
fn in_a_pool() -> Region {
    let water = BlockState::parse("minecraft:water[level=0]").unwrap();
    let mut pool = Vec::new();
    for x in 7..=9 {
        for z in 7..=9 {
            for y in -60..=-58 {
                pool.push((BlockPos::new(x, y, z), water));
            }
        }
    }
    on_ticking_floor_with(&pool)
}

#[test]
fn a_stand_sinks_in_water_by_the_game_s_slowing_and_holds_its_breath() {
    let mut region = in_a_pool();
    let start = -59.0;
    let sinking = stand(1, [8.5, start, 8.5], Vec::new());
    lay(&mut region, &sinking);
    // In water: the move, then 0.8 sideways and `0.8f` down, and gravity's sixteenth
    // taken (`getFluidFallingAdjustedMovement`), still water pushing nothing.
    let slow = f64::from(0.8_f32);
    let (mut y, mut motion) = (start, 0.0_f64);
    for tick in 1..=6 {
        if motion.abs() < 0.003 {
            motion = 0.0;
        }
        y += motion;
        let kept = motion * slow;
        let falling = motion <= 0.0;
        motion = if falling && (kept - 0.005).abs() >= 0.003 && (kept - 0.005).abs() < 0.003 {
            -0.003
        } else {
            kept - 0.08 / 16.0
        };
        checked(&mut region, &TickInputs::default());
        let now = record(&region, sinking.key).unwrap();
        assert_eq!(now.position[1].to_bits(), y.to_bits(), "tick {tick}");
        assert_eq!(now.motion[1].to_bits(), motion.to_bits(), "tick {tick}");
        // Its eyes, 1.7775 up, are under the surface at -57 + 8/9: a breath a tick.
        let EntityData::ArmorStand { air, .. } = now.data else {
            unreachable!();
        };
        assert_eq!(i32::from(air), 300 - tick, "tick {tick}");
    }
}

#[test]
fn a_stand_at_rest_commits_nothing_after_its_first_two_ticks_and_its_motion_holds() {
    let mut region = on_ticking_floor_with(&[]);
    let resting = Entity {
        on_ground: true,
        ..stand(1, [8.5, -60.0, 8.5], Vec::new())
    };
    lay(&mut region, &resting);
    // Its first tick's move of nothing leaves it off the ground with gravity's motion;
    // its second lands it again.
    let first = checked(&mut region, &TickInputs::default());
    assert_eq!(first.entity_changes.len(), 1, "{:?}", first.entity_changes);
    let second = checked(&mut region, &TickInputs::default());
    let [(_, EntityChange::Set(_, fields))] = &second.entity_changes[..] else {
        panic!("{:?}", second.entity_changes);
    };
    assert_eq!(fields, &[EntityField::OnGround(true)]);
    let held = (0.0 - 0.08) * f64::from(0.98_f32);
    for tick in 0..60 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(output.entity_changes.is_empty(), "tick {tick}");
        assert_eq!(output.entities_ageing, [ORIGIN], "tick {tick}");
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::EntityTracked(_))),
            "tick {tick}"
        );
        let now = record(&region, resting.key).unwrap();
        assert_eq!(now.position, resting.position);
        assert_eq!(now.motion, [0.0, held, 0.0]);
        assert!(now.on_ground);
    }
}

#[test]
fn a_submerged_stand_s_breath_is_counted_in_place_and_shown_but_not_committed() {
    let mut region = in_a_pool();
    let resting = Entity {
        on_ground: true,
        motion: [0.0, -0.005, 0.0],
        ..stand(1, [8.5, -60.0, 8.5], Vec::new())
    };
    lay(&mut region, &resting);
    checked(&mut region, &TickInputs::default());
    for tick in 0..10 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(
            output.entity_changes.is_empty(),
            "tick {tick}: {:?}",
            output.entity_changes
        );
        let reported = output.events.iter().find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) if tracked.entity == resting.key => {
                tracked.data.clone()
            }
            _ => None,
        });
        let Some(EntityData::ArmorStand { air, .. }) = reported else {
            panic!("tick {tick}: {:?}", output.events);
        };
        assert_eq!(i32::from(air), 300 - 2 - tick, "tick {tick}");
    }
}

// Equipping by the height of the click.

#[test]
fn a_click_takes_off_what_is_worn_at_its_height_and_puts_on_what_is_held_in_its_slot() {
    // `getClickedSlot`'s bands of a stand that is not small: the feet from 0.1 to 0.55,
    // the chest from 0.9 to 1.6, the legs from 0.4 to 1.2, the head from 1.6; each only
    // where something is worn there.
    let worn_all = vec![
        (slots::FEET, BOOTS),
        (slots::LEGS, BOOTS),
        (slots::CHEST, CHESTPLATE),
        (slots::HEAD, HELMET),
    ];
    assert_eq!(clicked_slot(0.3, 0, &worn_all), slots::FEET);
    assert_eq!(clicked_slot(0.6, 0, &worn_all), slots::LEGS);
    assert_eq!(clicked_slot(1.0, 0, &worn_all), slots::CHEST);
    assert_eq!(clicked_slot(1.7, 0, &worn_all), slots::HEAD);
    assert_eq!(clicked_slot(0.05, 0, &worn_all), slots::MAIN_HAND);
    // Nothing on the chest: a click there finds the legs.
    assert_eq!(clicked_slot(1.0, 0, &worn_all[..2]), slots::LEGS);
    // A small stand's click height is doubled: 0.5 is the legs' 1.0.
    assert_eq!(clicked_slot(0.5, 1, &[(slots::LEGS, BOOTS)]), slots::LEGS);

    let mut region = on_ticking_floor_with(&[]);
    let dressed = stand(1, [3.5, -60.0, 3.5], vec![(slots::HEAD, HELMET)]);
    lay(&mut region, &dressed);
    checked(
        &mut region,
        &moves(vec![set_slot(1, 0, None), select(1, 0)]),
    );
    // An empty hand at the head's height takes the helmet into the hand.
    let output = checked(&mut region, &click_on(&dressed, 1.8));
    assert!(worn(&record(&region, dressed.key).unwrap()).is_empty());
    assert_eq!(in_hand(&region), Some(HELMET));
    assert!(output.player_events.iter().any(|(_, event)| matches!(
        event,
        PlayerEvent::InventorySlot {
            slot: 0,
            stack: Some(HELMET)
        }
    )));
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        EntityChange::Set(key, _) if *key == dressed.key
    )));
    // A chestplate in hand goes on the chest wherever the click lands, with its equip
    // sound; the creative player keeps it.
    holding(&mut region, CHESTPLATE);
    let output = checked(&mut region, &click_on(&dressed, 0.2));
    assert_eq!(
        worn(&record(&region, dressed.key).unwrap()),
        [(slots::CHEST, CHESTPLATE)]
    );
    assert!(sounds_of(&output).contains(&sounds::ITEM_ARMOR_EQUIP_DIAMOND));
    assert_eq!(in_hand(&region), Some(CHESTPLATE));
    // A stick would go in the hand, and a stand without arms takes nothing there.
    holding(&mut region, ItemStack::new(items::STICK, 1));
    checked(&mut region, &click_on(&dressed, 1.0));
    assert_eq!(
        worn(&record(&region, dressed.key).unwrap()),
        [(slots::CHEST, CHESTPLATE)]
    );
}

#[test]
fn a_one_item_hand_swaps_with_what_the_slot_wears() {
    let mut region = on_ticking_floor_with(&[]);
    let dressed = stand(1, [3.5, -60.0, 3.5], vec![(slots::FEET, BOOTS)]);
    lay(&mut region, &dressed);
    let other_boots = ItemStack::new(items::DIAMOND_BOOTS, 1);
    holding(&mut region, other_boots.clone());
    let output = checked(&mut region, &click_on(&dressed, 1.0));
    // The edges are told what it wears now, which they send as `SetEquipment`.
    let told = output.events.iter().find_map(|event| match event {
        RegionEvent::EntityTracked(tracked) if tracked.entity == dressed.key => {
            tracked.data.clone()
        }
        _ => None,
    });
    assert!(matches!(
        told,
        Some(EntityData::ArmorStand { ref equipment, .. }) if equipment == &[(slots::FEET, other_boots.clone())]
    ));
    assert_eq!(
        worn(&record(&region, dressed.key).unwrap()),
        [(slots::FEET, other_boots)]
    );
    assert_eq!(in_hand(&region), Some(BOOTS));
}

// Being hit.

#[test]
fn a_creative_players_hit_breaks_a_stand_at_once_and_drops_nothing_not_even_what_it_wears() {
    let mut region = on_ticking_floor_with(&[]);
    let dressed = stand(1, [3.5, -60.0, 3.5], vec![(slots::HEAD, HELMET)]);
    lay(&mut region, &dressed);
    let output = checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Attack {
                entity: dressed.key,
            },
        )]),
    );
    assert!(record(&region, dressed.key).is_none());
    assert!(
        items_lying(&region).is_empty(),
        "{:?}",
        items_lying(&region)
    );
    assert!(sounds_of(&output).contains(&sounds::ENTITY_ARMOR_STAND_BREAK));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityGone { entity, .. } if *entity == dressed.key
    )));
}

/// A hit on `target` from another region's player, as an effect at its block.
fn hit(target: &Entity, sequence: u64, creative: bool) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: crate::physics::containing(target.position),
                what: Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Attack {
                        attacker: EntityId(77),
                        creative,
                        sprinting: false,
                        damage: Float(1.0),
                        from: [Double(3.5), Double(-60.0), Double(6.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

#[test]
fn a_survival_hit_is_a_first_hit_unless_one_came_within_five_ticks_and_the_second_drops_all() {
    let mut region = on_ticking_floor_with(&[]);
    let dressed = Entity {
        on_ground: true,
        motion: [0.0, -0.0784, 0.0],
        ..stand(1, [8.5, -60.0, 8.5], vec![(slots::HEAD, HELMET)])
    };
    lay(&mut region, &dressed);
    let wobbled = checked(&mut region, &hit(&dressed, 1, false));
    assert!(wobbled.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityEvent { entity, event: 32, .. } if *entity == dressed.key
    )));
    let first = record(&region, dressed.key).expect("a first hit only wobbles it");
    let EntityData::ArmorStand { last_hit, .. } = first.data else {
        unreachable!();
    };
    let at = last_hit.expect("the hit is kept");
    // Six ticks later the next is a first hit again: `now - lastHit > 5`.
    for _ in 0..5 {
        checked(&mut region, &TickInputs::default());
    }
    checked(&mut region, &hit(&dressed, 2, false));
    let again = record(&region, dressed.key).expect("six ticks on, a first hit again");
    let EntityData::ArmorStand { last_hit, .. } = again.data else {
        unreachable!();
    };
    assert_eq!(last_hit, Some(at + 6));
    // Five ticks on it breaks: the stand's item where it stood and the helmet a block
    // above, each popped within a quarter of its block's centre.
    for _ in 0..4 {
        checked(&mut region, &TickInputs::default());
    }
    let output = checked(&mut region, &hit(&dressed, 3, false));
    assert!(record(&region, dressed.key).is_none());
    assert!(sounds_of(&output).contains(&sounds::ENTITY_ARMOR_STAND_BREAK));
    let dropped = items_lying(&region);
    let at = |item: i32| {
        let found: Vec<[f64; 3]> = dropped
            .iter()
            .filter(|(stack, _)| stack.item == item)
            .map(|(_, at)| *at)
            .collect();
        let [at] = found[..] else {
            panic!("{dropped:?}");
        };
        at
    };
    assert_eq!(dropped.len(), 2, "{dropped:?}");
    let stand_at = at(items::ARMOR_STAND);
    let helmet_at = at(items::DIAMOND_HELMET);
    // `popResource`: within a quarter of the block's centre, the item's feet half its
    // height below that; the items, made before the entities' phase, have since made
    // their first move: sideways at most the 0.1 of their motion's draw
    // (`nextDouble() * 0.2 - 0.1`), up at most the 0.16 their throw up and gravity leave
    // them.
    for (at, block_y) in [(stand_at, -60.0_f64), (helmet_at, -59.0)] {
        assert!((at[0] - 8.5).abs() <= 0.25 + 0.1, "{at:?}");
        assert!((at[2] - 8.5).abs() <= 0.25 + 0.1, "{at:?}");
        let low = block_y + 0.5 - 0.125 - 0.25;
        assert!(at[1] >= low && at[1] <= low + 0.5 + 0.16, "{at:?}");
    }
}

#[test]
fn a_hit_from_another_regions_creative_player_breaks_the_stand_where_it_is_held() {
    let mut region = on_ticking_floor_with(&[]);
    let target = stand(1, [8.5, -60.0, 8.5], vec![(slots::FEET, BOOTS)]);
    lay(&mut region, &target);
    checked(&mut region, &hit(&target, 1, true));
    assert!(record(&region, target.key).is_none());
    assert!(items_lying(&region).is_empty());
}

/// A use of `target` by another region's player holding `held`, as an effect.
fn used_from_afar(
    target: &Entity,
    sequence: u64,
    held: Option<ItemStack>,
    height: f64,
) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: crate::physics::containing(target.position),
                what: Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Interact {
                        player: EntityId(77),
                        creative: true,
                        held,
                        location: [Double(0.0), Double(height), Double(0.0)],
                        sneaking: false,
                        from: [Double(3.5), Double(-60.0), Double(6.5)],
                        edge: None,
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

#[test]
fn across_a_line_a_creative_put_is_done_and_a_take_is_not() {
    let mut region = on_ticking_floor_with(&[]);
    let target = stand(1, [8.5, -60.0, 8.5], vec![(slots::FEET, BOOTS)]);
    lay(&mut region, &target);
    checked(&mut region, &used_from_afar(&target, 1, Some(HELMET), 1.8));
    assert_eq!(
        worn(&record(&region, target.key).unwrap()),
        [(slots::FEET, BOOTS), (slots::HEAD, HELMET)]
    );
    // An empty hand at the feet would take the boots into a hand the region does not
    // have: nothing happens (`docs/groundwork/n5-plan.md`, section 8 item 7).
    checked(&mut region, &used_from_afar(&target, 2, None, 0.3));
    assert_eq!(
        worn(&record(&region, target.key).unwrap()),
        [(slots::FEET, BOOTS), (slots::HEAD, HELMET)]
    );
}

#[test]
fn a_marker_and_an_invisible_stand_take_no_hit() {
    let mut region = on_ticking_floor_with(&[]);
    let mut marker = stand(1, [8.5, -60.0, 8.5], Vec::new());
    let mut hidden = stand(2, [6.5, -60.0, 8.5], Vec::new());
    for (entity, flag) in [(&mut marker, 16), (&mut hidden, 32)] {
        if let EntityData::ArmorStand { flags, .. } = &mut entity.data {
            *flags = flag;
        }
    }
    lay(&mut region, &marker);
    lay(&mut region, &hidden);
    checked(&mut region, &hit(&marker, 1, true));
    checked(&mut region, &hit(&hidden, 2, true));
    assert!(record(&region, marker.key).is_some());
    assert!(record(&region, hidden.key).is_some());
}

#[test]
fn a_stand_in_fire_burns_down_and_drops_what_it_wears_but_not_itself() {
    let mut region = on_ticking_floor_with(&[(BlockPos::new(8, -60, 8), blocks::FIRE)]);
    let burning = Entity {
        on_ground: true,
        motion: [0.0, -0.0784, 0.0],
        ..stand(1, [8.5, -60.0, 8.5], vec![(slots::HEAD, HELMET)])
    };
    lay(&mut region, &burning);
    let mut broke = None;
    for tick in 1..200 {
        let output = checked(&mut region, &TickInputs::default());
        if record(&region, burning.key).is_none() {
            assert!(sounds_of(&output).contains(&sounds::ENTITY_ARMOR_STAND_BREAK));
            broke = Some(tick);
            break;
        }
    }
    // The fire lights it for 160 ticks in its first tick and takes 0.15. Standing in it
    // lights it again to 160 every tick (`fireIgnite`), so every `baseTick` finds a
    // twentieth tick of burning and takes 4 (a stand has no time of grace between
    // hits): 19.85, 15.7, 11.55, 7.4, 3.25, and in the sixth tick's `baseTick` it is
    // under half a heart.
    assert_eq!(broke, Some(6));
    let dropped: Vec<ItemStack> = items_lying(&region)
        .iter()
        .map(|(stack, _)| stack.clone())
        .collect();
    assert_eq!(dropped, [HELMET]);
}
