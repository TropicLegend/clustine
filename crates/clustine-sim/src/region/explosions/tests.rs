//! Tests of the explosion from the game's rules (`docs/groundwork/n6-brief.md`, section
//! 2): the rays' reach and what stops them, the drops and their stacks, what the
//! explosion does to each kind of entity and to players, what it makes into effects
//! across a line, and that it is a function of the region and its inputs.

use clustine_data::{blocks, entity_types, items};
use clustine_world::{
    Attached, BlockPos, ChunkPos, Effected, Entity, EntityChange, EntityData, EntityId, EntityUse,
    Float, ItemStack,
};
use uuid::Uuid;

use super::super::Region;
use super::super::random_ticks::Random;
use super::super::stands::{DEFAULT_POSE, INVISIBLE, MARKER};
use super::super::tnt::tests::{
    add, add_player, at, cart, idle, nothing, primed, record, records, state, west_of_a_line, world,
};
use super::{Explosion, add_or_append, damage_amount, resistance};
use crate::api::{RegionEvent, TickOutput};

/// Where the tests' TNT stands: the middle of a block on the floor.
const TNT_AT: [f64; 3] = [4.5, -60.0, 4.5];

/// The centre of the explosion of TNT standing at `feet`: `getY(0.0625)`.
fn centre_of(feet: [f64; 3]) -> [f64; 3] {
    [feet[0], feet[1] + f64::from(0.98_f32) * 0.0625, feet[2]]
}

/// What the tick's events say became air, in order.
fn broken(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } if *state == blocks::AIR => Some(*position),
            _ => None,
        })
        .collect()
}

/// The explosion the tick told the edges of.
fn explosion_of(output: &TickOutput) -> (i32, Vec<(EntityId, [f64; 3])>) {
    output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::Explosion {
                blocks, knockback, ..
            } => Some((*blocks, knockback.clone())),
            _ => None,
        })
        .expect("an explosion")
}

/// The items of the loaded chunks: where each is and what it holds.
fn items_lying(region: &Region) -> Vec<([f64; 3], ItemStack)> {
    records(region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some((entity.position, stack)),
            _ => None,
        })
        .collect()
}

fn count_of(region: &Region, item: i32) -> i32 {
    items_lying(region)
        .into_iter()
        .filter(|(_, stack)| stack.item == item)
        .map(|(_, stack)| stack.count)
        .sum()
}

/// The TNT of `fuse` 1 at `feet`, primed by `owner`, gone off in one tick.
fn goes_off(region: &mut Region, feet: [f64; 3], owner: Option<Uuid>) -> TickOutput {
    primed(region, feet, 1, owner);
    idle(region)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

fn block_centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

#[test]
fn blocks_and_fluids_resist_as_the_calculator_says() {
    // Air without a fluid gives no resistance, and a fluid at least its own.
    assert_eq!(resistance(blocks::AIR), None);
    assert_eq!(resistance(blocks::CAVE_AIR), None);
    assert_eq!(resistance(blocks::STONE), Some(6.0));
    assert_eq!(resistance(blocks::OBSIDIAN), Some(1200.0));
    assert_eq!(resistance(blocks::WATER), Some(100.0));
    assert_eq!(resistance(blocks::TNT), Some(0.0));
    let wet = state("minecraft:oak_slab[type=bottom,waterlogged=true]");
    assert_eq!(resistance(wet), Some(100.0));
}

#[test]
fn the_damage_is_the_games_by_distance_and_exposure() {
    // `getEntityDamageAmount` of TNT's radius 4: 57 at the centre, fully seen, and 1 at
    // its reach of 8.
    assert_eq!(damage_amount(4.0, 0.0, 1.0), 57.0);
    assert_eq!(damage_amount(4.0, 8.0, 1.0), 1.0);
    // Half way and half seen: an impact of a quarter, (1/16 + 1/4) / 2 · 7 · 8 + 1.
    assert_eq!(damage_amount(4.0, 4.0, 0.5), 9.75);
}

/// `areMergable` asks for the same components too: drops of one item that differ in them
/// keep a collector each, and those alike share one.
#[test]
fn drops_merge_only_where_their_components_are_the_same() {
    let plain = ItemStack::new(items::COBBLESTONE, 1);
    let mut named = plain.clone();
    named
        .components
        .set(clustine_world::Component::ItemName("\"Rubble\"".into()));
    let mut collectors = Vec::new();
    add_or_append(&mut collectors, plain.clone(), at(0, 0, 0));
    add_or_append(&mut collectors, named.clone(), at(1, 0, 0));
    add_or_append(&mut collectors, named.clone(), at(2, 0, 0));
    add_or_append(&mut collectors, plain.clone(), at(3, 0, 0));
    assert_eq!(
        collectors,
        [
            (at(0, 0, 0), plain.with_count(2)),
            (at(1, 0, 0), named.with_count(2))
        ]
    );
}

#[test]
fn drops_merge_into_stacks_of_at_most_sixteen_and_each_stays_at_its_first_block() {
    let mut collectors = Vec::new();
    let cobblestone = ItemStack::new(items::COBBLESTONE, 1);
    for x in 0..20 {
        add_or_append(&mut collectors, cobblestone.clone(), at(x, 0, 0));
    }
    let sixteen = cobblestone.with_count(16);
    let four = cobblestone.with_count(4);
    assert_eq!(collectors, [(at(0, 0, 0), sixteen), (at(16, 0, 0), four)]);
    // Two stacks that would not fit one stack of their item do not merge at all
    // (`areMergable`): ender pearls stack to 16.
    let pearls = ItemStack::new(items::ENDER_PEARL, 10);
    let mut collectors = Vec::new();
    add_or_append(&mut collectors, pearls.clone(), at(0, 0, 0));
    add_or_append(&mut collectors, pearls.clone(), at(1, 0, 0));
    assert_eq!(
        collectors,
        [(at(0, 0, 0), pearls.clone()), (at(1, 0, 0), pearls)]
    );
    // An empty drop after the first is lost, but a first starts a collector, whose
    // place `popResource` still draws.
    let mut collectors = Vec::new();
    let empty = ItemStack::new(items::COBBLESTONE, 0);
    add_or_append(&mut collectors, empty.clone(), at(0, 0, 0));
    add_or_append(&mut collectors, empty.clone(), at(1, 0, 0));
    assert_eq!(collectors, [(at(0, 0, 0), empty)]);
}

#[test]
fn rays_in_open_air_draw_once_each_and_reach_no_further_than_six_point_nine_blocks() {
    let region = world(1, nothing);
    let explosion = Explosion {
        fire: false,
        hurts: true,
        resistance_at: None,
        source: EntityId(1),
        centre: [8.5, -45.5, 8.5],
        radius: 4.0,
        owner: None,
        indirect: None,
        decay: false,
        rails_pass: false,
        source_in_water: false,
    };
    let start = Random::of_entity(1, EntityId(1), 9);
    let mut random = start.clone();
    let positions = region.exploded_positions(&explosion, &mut random);
    // One float for each of the 16³ - 14³ rays.
    assert_eq!(random.draws_since(&start), 1352);
    let farthest = positions
        .iter()
        .map(|pos| {
            // The nearest point of the block to the centre.
            let near = std::array::from_fn(|axis| {
                let low = f64::from([pos.x, pos.y, pos.z][axis]);
                explosion.centre[axis].clamp(low, low + 1.0)
            });
            distance(near, explosion.centre)
        })
        .fold(0.0, f64::max);
    assert!(farthest <= 6.9 + 1.0e-9, "{farthest}");
    assert!(farthest > 5.0, "the strongest rays go far: {farthest}");
    // Air, all of it: in the packet's count, broken as nothing.
    assert!(positions.len() > 1000, "{}", positions.len());
}

#[test]
fn tnt_in_stone_breaks_nothing_beyond_its_reach_leaves_obsidian_and_drops_all_it_breaks() {
    let centre_block = at(8, -55, 8);
    let obsidian = [at(9, -55, 8), at(8, -54, 8)];
    let blocks_of = |pos: BlockPos| {
        let near = (pos.x - 8).abs() <= 8 && (pos.y + 55).abs() <= 8 && (pos.z - 8).abs() <= 8;
        if !near || pos == centre_block {
            None
        } else if obsidian.contains(&pos) {
            Some(blocks::OBSIDIAN)
        } else {
            Some(blocks::STONE)
        }
    };
    let mut region = world(2, blocks_of);
    let feet = [8.5, -55.0, 8.5];
    let output = goes_off(&mut region, feet, None);
    let centre = centre_of(feet);
    let gone = broken(&output);
    assert!(!gone.is_empty(), "it breaks stone");
    for pos in &gone {
        assert!(
            distance(block_centre(*pos), centre) < 3.5,
            "{pos:?} is beyond what stone lets a ray reach"
        );
    }
    for pos in obsidian {
        assert_eq!(region.block(pos), Some(blocks::OBSIDIAN));
    }
    // TNT drops all it breaks (`DESTROY`): a cobblestone for each stone, merged into
    // stacks of at most 16.
    let stone_broken = i32::try_from(gone.len()).expect("few");
    assert_eq!(count_of(&region, items::COBBLESTONE), stone_broken);
    assert!(
        items_lying(&region)
            .iter()
            .all(|(_, stack)| stack.count <= 16)
    );
}

#[test]
fn tnt_in_water_breaks_nothing_and_tnt_beside_water_breaks_none_of_it() {
    // In a pool: its rays end where they start.
    let pool = |pos: BlockPos| {
        ((pos.x - 4).abs() <= 6 && (pos.z - 4).abs() <= 6 && (-60..=-57).contains(&pos.y))
            .then_some(blocks::WATER)
    };
    let mut region = world(3, pool);
    let output = goes_off(&mut region, [4.5, -59.0, 4.5], None);
    assert_eq!(explosion_of(&output).0, 0);
    assert!(broken(&output).is_empty());
    // Beside one, on dirt: the dirt goes and the water stays.
    let beside = |pos: BlockPos| {
        if pos.y == -60 && pos.x >= 6 {
            Some(blocks::WATER)
        } else if pos.y == -60 && pos.x < 4 {
            Some(blocks::DIRT)
        } else {
            None
        }
    };
    let mut region = world(4, beside);
    let output = goes_off(&mut region, TNT_AT, None);
    let gone = broken(&output);
    assert!(
        gone.iter().any(|pos| pos.x < 4 && pos.y == -60),
        "dirt breaks"
    );
    assert!(gone.iter().all(|pos| !(pos.y == -60 && pos.x >= 6)));
    for x in 6..=9 {
        assert_eq!(region.block(at(x, -60, 4)), Some(blocks::WATER));
    }
}

#[test]
fn the_drops_of_a_field_of_dirt_lie_in_stacks_of_at_most_sixteen_each_at_a_block_it_broke() {
    let field = |pos: BlockPos| {
        ((pos.x - 4).abs() <= 6 && (pos.z - 4).abs() <= 6 && (-60..=-58).contains(&pos.y))
            .then_some(blocks::DIRT)
    };
    let mut region = world(5, field);
    let feet = [4.5, -57.0, 4.5];
    let output = goes_off(&mut region, feet, None);
    let gone = broken(&output);
    let dirt_broken = gone.iter().filter(|pos| pos.y >= -60).count();
    let lying = items_lying(&region);
    let dirt: Vec<_> = lying
        .iter()
        .filter(|(_, stack)| stack.item == items::DIRT)
        .collect();
    let total: i32 = dirt.iter().map(|(_, stack)| stack.count).sum();
    assert_eq!(usize::try_from(total).expect("positive"), dirt_broken);
    assert!(dirt_broken > 16, "{dirt_broken}");
    assert!(dirt.iter().all(|(_, stack)| stack.count <= 16));
    assert!(dirt.iter().any(|(_, stack)| stack.count == 16));
    // `popResource`: within a quarter of a block of the middle of a block it broke, the
    // item's feet half its height lower.
    for (position, _) in &dirt {
        let at_a_block = gone.iter().any(|pos| {
            let middle = block_centre(*pos);
            (position[0] - middle[0]).abs() <= 0.25
                && (position[2] - middle[2]).abs() <= 0.25
                && (position[1] + 0.125 - middle[1]).abs() <= 0.25
        });
        assert!(at_a_block, "{position:?}");
    }
}

fn item_record(region: &mut Region, item: i32, position: [f64; 3]) -> EntityId {
    let data = EntityData::Item {
        stack: ItemStack::new(item, 1),
        pick_up_delay: 0,
        thrower: None,
        health: 5,
        age_offset: 0,
    };
    let entity = record(region, entity_types::ITEM, position, data);
    add(region, entity)
}

#[test]
fn an_item_near_tnt_is_destroyed_one_far_off_is_hurt_and_pushed_and_a_nether_star_is_not_hurt() {
    let mut region = world(6, nothing);
    let near = item_record(&mut region, items::STICK, [6.5, -60.0, 4.5]);
    let far_at = [12.0, -60.0, 4.5];
    let far = item_record(&mut region, items::STICK, far_at);
    let star = item_record(&mut region, items::NETHER_STAR, [4.5, -60.0, 6.5]);
    goes_off(&mut region, TNT_AT, None);
    assert!(region.find_entity(near).is_none(), "destroyed");
    // Fully seen, 7.5 away: `(int) (5 - damage)`.
    let damage = damage_amount(4.0, distance(far_at, centre_of(TNT_AT)), 1.0);
    let (_, far) = region.find_entity(far).expect("hurt, not destroyed");
    let EntityData::Item { health, .. } = far.data else {
        panic!("an item");
    };
    assert_eq!(i32::from(health), (5.0 - damage) as i32);
    assert_eq!(health, 2);
    assert!(far.motion[0] > 0.0, "pushed away: {:?}", far.motion);
    let (_, star) = region
        .find_entity(star)
        .expect("a nether star resists explosions");
    let EntityData::Item { health, .. } = star.data else {
        panic!("an item");
    };
    assert_eq!(health, 5);
    assert!(
        star.motion[2] > 0.0,
        "pushed all the same: {:?}",
        star.motion
    );
}

fn stand_record(region: &mut Region, position: [f64; 3], flags: u8) -> EntityId {
    let helmet = ItemStack::new(items::DIAMOND_HELMET, 1);
    let data = EntityData::ArmorStand {
        flags,
        disabled_slots: 0,
        pose: Box::new(DEFAULT_POSE.map(|part| part.map(Float))),
        // `EquipmentSlot.HEAD`'s ordinal.
        equipment: vec![(5, helmet)],
        health: Float(20.0),
        last_hit: None,
        air: 300,
        frozen: 0,
    };
    let entity = record(region, entity_types::ARMOR_STAND, position, data);
    add(region, entity)
}

#[test]
fn an_armour_stand_breaks_dropping_what_it_wears_and_not_itself_and_a_marker_is_left() {
    let mut region = world(7, nothing);
    let stand = stand_record(&mut region, [6.5, -60.0, 4.5], 0);
    let marker = stand_record(&mut region, [4.5, -60.0, 6.5], MARKER);
    let invisible = stand_record(&mut region, [2.5, -60.0, 4.5], INVISIBLE);
    goes_off(&mut region, TNT_AT, None);
    assert!(region.find_entity(stand).is_none());
    assert_eq!(count_of(&region, items::DIAMOND_HELMET), 1);
    assert_eq!(count_of(&region, items::ARMOR_STAND), 0);
    assert!(region.find_entity(marker).is_some(), "a marker is not hurt");
    // An invisible stand ignores the explosion: not even pushed.
    let (_, invisible) = region.find_entity(invisible).expect("left alone");
    assert_eq!(invisible.motion[0], 0.0);
}

#[test]
fn a_minecart_breaks_and_drops_its_item_unless_a_creative_player_primed_the_tnt() {
    for (owner, drops) in [(None, 1), (Some(Uuid::from_u128(3)), 0)] {
        let mut region = world(8, nothing);
        let minecart = cart(&mut region, entity_types::MINECART, [6.5, -60.0, 4.5]);
        goes_off(&mut region, TNT_AT, owner);
        assert!(region.find_entity(minecart).is_none(), "owner {owner:?}");
        assert_eq!(count_of(&region, items::MINECART), drops, "owner {owner:?}");
    }
}

/// A painting of kebab (one block) on the wall at `wall`, facing north, in front of it.
fn painting_record(region: &mut Region, wall: BlockPos) -> EntityId {
    let block = at(wall.x, wall.y, wall.z - 1);
    let data = EntityData::Painting {
        attached: Attached {
            block,
            direction: 2,
        },
        variant: 24,
    };
    let position = [
        f64::from(block.x) + 0.5,
        f64::from(block.y) + 0.5,
        f64::from(block.z) + 0.968_75,
    ];
    let entity = record(region, entity_types::PAINTING, position, data);
    add(region, entity)
}

#[test]
fn a_painting_drops_itself_unless_a_creative_player_primed_the_tnt_and_tnt_in_water_leaves_it() {
    let wall = at(4, -59, 8);
    let obsidian = move |pos: BlockPos| (pos == wall).then_some(blocks::OBSIDIAN);
    for (owner, drops) in [(None, 1), (Some(Uuid::from_u128(4)), 0)] {
        let mut region = world(9, obsidian);
        let painting = painting_record(&mut region, wall);
        goes_off(&mut region, [4.5, -60.0, 5.5], owner);
        assert!(region.find_entity(painting).is_none(), "owner {owner:?}");
        assert_eq!(count_of(&region, items::PAINTING), drops, "owner {owner:?}");
    }
    // TNT that has been in water since its last tick: frames and paintings ignore it
    // (`BlockAttachedEntity.ignoreExplosion`).
    let wet = move |pos: BlockPos| {
        if pos == wall {
            Some(blocks::OBSIDIAN)
        } else if pos == at(4, -60, 5) {
            Some(blocks::WATER)
        } else {
            None
        }
    };
    let mut region = world(10, wet);
    let painting = painting_record(&mut region, wall);
    primed(&mut region, [4.5, -60.0, 5.5], 2, None);
    idle(&mut region);
    let output = idle(&mut region);
    explosion_of(&output);
    assert!(region.find_entity(painting).is_some());
}

#[test]
fn a_creative_player_is_pushed_and_not_hurt_and_one_flying_is_left_alone() {
    let mut region = world(11, nothing);
    let (walking, walker) = add_player(&mut region, 1, [7.5, -60.0, 4.5], false);
    let (flying, _) = add_player(&mut region, 2, [1.5, -60.0, 4.5], true);
    let output = goes_off(&mut region, TNT_AT, None);
    let (_, knockback) = explosion_of(&output);
    let [(entity, push)] = knockback[..] else {
        panic!("{knockback:?}");
    };
    assert_eq!(entity, walker);
    // Away from the centre, up to the eyes, scaled by how near and how seen.
    assert!(push[0] > 0.0 && push[1] > 0.0, "{push:?}");
    assert!(region.players.contains_key(&walking));
    assert!(region.players.contains_key(&flying));
}

#[test]
fn a_block_and_an_entity_across_a_line_are_made_into_effects_with_the_blast() {
    let dirt = |pos: BlockPos| (pos.y == -60 && (16..=18).contains(&pos.x)).then_some(blocks::DIRT);
    let mut region = west_of_a_line(12, dirt);
    // An item across the line, which the region has only in its copy, above the dirt,
    // which would hide it from the centre.
    let across = Entity {
        key: EntityId(900_000),
        ..record_for_copy([16.5, -58.0, 4.5])
    };
    region
        .copies
        .get_mut(&ChunkPos::new(1, 0))
        .expect("copied")
        .apply_entity(&EntityChange::Whole(across.clone()));
    let owner = Some(Uuid::from_u128(6));
    let feet = [14.5, -60.0, 4.5];
    let tnt = primed(&mut region, feet, 1, owner);
    let output = idle(&mut region);
    // The far dirt is not broken here: its holder breaks it.
    assert_eq!(region.block(at(16, -60, 4)), Some(blocks::DIRT));
    let mut exploded = 0;
    let mut hit = 0;
    for effect in &output.effects {
        match effect.what {
            Effected::Exploded { expected, blast } => {
                assert!(effect.at.x >= 16, "{:?}", effect.at);
                assert_eq!(region.block(effect.at), Some(expected));
                assert_eq!((blast.source, blast.tick), (tnt, output.tick));
                assert_eq!(blast.centre.map(|value| value.0), centre_of(feet));
                assert_eq!(blast.radius, Float(4.0));
                assert!(!blast.decay && blast.direct);
                assert_eq!((blast.owner, blast.owner_creative), (owner, true));
                exploded += 1;
            }
            Effected::OnEntity {
                key,
                by:
                    EntityUse::Blasted {
                        damage,
                        push,
                        blast,
                    },
            } => {
                assert_eq!(key, across.key);
                assert_eq!(effect.at, at(16, -58, 4));
                assert!(damage.0 > 1.0);
                assert!(push[0].0 > 0.0);
                assert_eq!(blast.source, tnt);
                hit += 1;
            }
            _ => {}
        }
    }
    assert!(exploded > 0);
    assert_eq!(hit, 1);
    // This side's blocks it breaks itself.
    assert!(broken(&output).iter().all(|pos| pos.x < 16));
}

/// An item record for a copy, not taken from the region's keys.
fn record_for_copy(position: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key: EntityId(0),
        kind: u16::try_from(entity_types::ITEM).expect("a type's id"),
        uuid: Uuid::from_u128(99),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        data: EntityData::Item {
            stack: ItemStack::new(items::STICK, 1),
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

#[test]
fn the_same_explosion_twice_gives_the_same_result() {
    let field = |pos: BlockPos| {
        ((pos.x - 4).abs() <= 5 && (pos.z - 4).abs() <= 5 && (-60..=-59).contains(&pos.y))
            .then_some(if (pos.x + pos.z) % 3 == 0 {
                blocks::STONE
            } else {
                blocks::DIRT
            })
    };
    let run = || {
        let mut region = world(13, field);
        item_record(&mut region, items::STICK, [8.5, -58.0, 4.5]);
        let output = goes_off(&mut region, [4.5, -58.0, 4.5], None);
        (output, records(&region))
    };
    let (first, first_records) = run();
    let (second, second_records) = run();
    assert_eq!(first, second);
    assert_eq!(first_records, second_records);
    assert!(!broken(&first).is_empty());
}

#[test]
fn a_chest_that_tnt_breaks_spills_what_it_holds_and_drops_itself() {
    use super::super::containers::{container_data, read_container};
    let chest_at = at(6, -60, 4);
    let mut region = world(14, move |pos| (pos == chest_at).then_some(blocks::CHEST));
    let mut container = read_container(27, &[]);
    let held = [
        (0, items::STONE, 64),
        (13, items::DIAMOND, 1),
        (26, items::OAK_LOG, 10),
    ];
    for (slot, item, count) in held {
        container.slots[slot] = Some(ItemStack::new(item, count));
    }
    let data = container_data(blocks::CHEST, &container, &[]);
    let (x, z) = chest_at.in_chunk();
    region
        .chunks
        .get_mut(&chest_at.chunk())
        .expect("loaded")
        .set_block_entity_data(clustine_world::InChunk::new(x, chest_at.y, z), data)
        .expect("a chest keeps its slots");
    goes_off(&mut region, TNT_AT, None);
    assert_eq!(region.block(chest_at), Some(blocks::AIR));
    // `Containers.dropContents`, in pieces of 10 to 30, and the chest's own loot.
    for (_, item, count) in held {
        assert_eq!(count_of(&region, item), count, "item {item}");
    }
    assert_eq!(count_of(&region, items::CHEST), 1);
}

/// The holder's side of an explosion across a line (`docs/groundwork/n6-plan.md`, section
/// 6 item 9): what the exploding region made into effects, taken by the region that holds
/// the blocks and entities.
mod across {
    use clustine_world::{Blast, Double, Due, Effect, EffectId, FiledEffect};

    use super::*;
    use crate::api::TickInputs;

    fn blast(owner: Option<Uuid>) -> Blast {
        Blast {
            fire: false,
            hurts: true,
            source: EntityId(9_999),
            tick: 1,
            centre: [Double(4.5), Double(-59.9), Double(4.5)],
            radius: Float(4.0),
            decay: false,
            direct: true,
            owner,
            owner_creative: owner.is_some(),
        }
    }

    fn taken(region: &mut Region, effects: Vec<Effect>) -> TickOutput {
        let effects = effects
            .into_iter()
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence: 1,
                    index: u32::try_from(index).expect("few"),
                },
                effect,
                due: Due::FromTaking,
            })
            .collect();
        region.tick(&TickInputs {
            effects,
            ..TickInputs::default()
        })
    }

    fn items_of(region: &Region) -> Vec<ItemStack> {
        records(region)
            .into_iter()
            .filter_map(|entity| match entity.data {
                EntityData::Item { stack, .. } => Some(stack),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_block_an_explosion_across_a_line_broke_is_broken_with_its_drops_merged() {
        let stone = |pos: BlockPos| {
            (pos.y == -60 && (2..=3).contains(&pos.x) && pos.z == 4).then_some(blocks::STONE)
        };
        let mut region = world(7, stone);
        let effects = [2, 3]
            .into_iter()
            .map(|x| Effect {
                at: at(x, -60, 4),
                what: Effected::Exploded {
                    expected: blocks::STONE,
                    blast: blast(None),
                },
            })
            .collect();
        let output = taken(&mut region, effects);
        assert_eq!(broken(&output), [at(2, -60, 4), at(3, -60, 4)]);
        // Stone broken by TNT drops cobblestone, the two merged into one stack.
        assert_eq!(items_of(&region), [ItemStack::new(items::COBBLESTONE, 2)]);
    }

    #[test]
    fn a_block_that_is_no_longer_what_the_explosion_saw_is_left_as_it_is() {
        let mut region = world(7, |pos| (pos == at(2, -60, 4)).then_some(blocks::GLASS));
        let output = taken(
            &mut region,
            vec![Effect {
                at: at(2, -60, 4),
                what: Effected::Exploded {
                    expected: blocks::STONE,
                    blast: blast(None),
                },
            }],
        );
        assert_eq!(broken(&output), []);
        assert!(items_of(&region).is_empty());
    }

    #[test]
    fn tnt_an_explosion_across_a_line_reached_is_primed_short_by_its_owner() {
        let owner = Uuid::from_u128(42);
        let mut region = world(7, |pos| (pos == at(2, -60, 4)).then_some(blocks::TNT));
        taken(
            &mut region,
            vec![Effect {
                at: at(2, -60, 4),
                what: Effected::Exploded {
                    expected: blocks::TNT,
                    blast: blast(Some(owner)),
                },
            }],
        );
        let primed = super::super::super::tnt::tests::primed_tnt(&region);
        let [(_, fuse, by)] = &primed[..] else {
            panic!("{primed:?}");
        };
        assert!((10..30).contains(fuse), "{fuse}");
        assert_eq!(*by, Some(owner));
        assert!(
            items_of(&region).is_empty(),
            "TNT drops nothing: it is primed"
        );
    }

    #[test]
    fn tnt_fire_across_a_line_burnt_is_primed_by_nobody_with_the_whole_fuse() {
        let mut region = world(7, |pos| (pos == at(2, -60, 4)).then_some(blocks::TNT));
        taken(
            &mut region,
            vec![Effect {
                at: at(2, -60, 4),
                what: Effected::Burnt {
                    expected: blocks::TNT,
                },
            }],
        );
        let primed = super::super::super::tnt::tests::primed_tnt(&region);
        // Primed with 80 as the effects are taken, and one tick of it gone by the end of
        // the tick, as the entities tick after the effects.
        assert!(matches!(&primed[..], [(_, 79, None)]), "{primed:?}");
    }

    #[test]
    fn an_armour_stand_a_blast_across_a_line_hit_breaks() {
        let mut region = world(7, nothing);
        let stand = record(
            &mut region,
            entity_types::ARMOR_STAND,
            [2.5, -60.0, 4.5],
            EntityData::ArmorStand {
                flags: 0,
                disabled_slots: 0,
                pose: Box::new(DEFAULT_POSE.map(|part| part.map(Float))),
                equipment: Vec::new(),
                health: Float(20.0),
                last_hit: None,
                air: 300,
                frozen: 0,
            },
        );
        let at_block = at(2, -60, 4);
        let key = add(&mut region, stand);
        taken(
            &mut region,
            vec![Effect {
                at: at_block,
                what: Effected::OnEntity {
                    key,
                    by: EntityUse::Blasted {
                        damage: Float(30.0),
                        push: [Double(0.5), Double(0.2), Double(0.0)],
                        blast: blast(None),
                    },
                },
            }],
        );
        assert!(
            region.find_entity(key).is_none(),
            "the stand broke: {:?}",
            records(&region)
        );
    }
}

/// Plates and tripwire at a line (`docs/groundwork/n6-plan.md`, section 6 item 6, section
/// 7 item 12, section 8 item 10): a player of one region on a plate of the other's land
/// is counted there by an effect each tick, and an entity in a copy is counted by the
/// holder's own count.
mod plates_across {
    use clustine_world::{Double, Due, Effect, EffectId, FiledEffect};

    use super::super::super::tnt::tests::add_player;
    use super::*;
    use crate::api::TickInputs;

    fn plate() -> clustine_data::BlockState {
        state("minecraft:oak_pressure_plate[powered=false]")
    }

    #[test]
    fn a_player_on_a_plate_across_a_line_sends_their_box_to_its_holder() {
        // The plate is at x = 16, beyond the line; the player stands at x = 15.8, their
        // box reaching over it.
        let at_plate = at(16, -60, 4);
        let mut region = west_of_a_line(7, |pos| (pos == at_plate).then_some(plate()));
        add_player(&mut region, 1, [15.8, -60.0, 4.5], false);
        let output = idle(&mut region);
        let sent: Vec<&Effect> = output
            .effects
            .iter()
            .filter(|effect| matches!(effect.what, Effected::PlayerInside { .. }))
            .collect();
        assert!(
            matches!(&sent[..], [effect] if effect.at == at_plate),
            "{:?}",
            output.effects
        );
    }

    #[test]
    fn a_plate_pressed_by_another_regions_player_stays_down_while_their_box_comes() {
        let at_plate = at(2, -60, 4);
        let mut region = world(7, |pos| (pos == at_plate).then_some(plate()));
        let player_box = |id: u32, tick: u64| FiledEffect {
            id: EffectId {
                sequence: tick,
                index: id,
            },
            effect: Effect {
                at: at_plate,
                what: Effected::PlayerInside {
                    min: [Double(2.2), Double(-60.0), Double(4.2)],
                    max: [Double(2.8), Double(-58.2), Double(4.8)],
                },
            },
            due: Due::At { tick },
        };
        let powered = |region: &Region| {
            region
                .own_block(at_plate)
                .is_some_and(|state| state.property("powered") == Some("true"))
        };
        // The box comes every tick for 30 ticks, past the plate's look 20 ticks on.
        for _ in 0..30 {
            let tick = region.tick + 1;
            region.tick(&TickInputs {
                effects: vec![player_box(0, tick)],
                ..TickInputs::default()
            });
        }
        assert!(
            powered(&region),
            "still pressed while the player stands there"
        );
        // Then it stops coming: by the plate's next look it is let go.
        for _ in 0..25 {
            idle(&mut region);
        }
        assert!(!powered(&region), "let go once the player has gone");
    }
}
