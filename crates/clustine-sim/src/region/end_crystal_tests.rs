//! The end crystal and its item from the game's rules (d EndCrystal.txt,
//! EndCrystalItem.txt; `docs/groundwork/p-brief.md`, section 6.3;
//! `docs/groundwork/p-plan.md`, section 19.5 item 13), not from the code: placed on
//! obsidian or bedrock only where the block above is air and no entity is in the box
//! above, without its base; its fire in the End; a player's hit that removes it with an
//! explosion of 6 that breaks blocks and hurts entities, and removes another crystal
//! without a second explosion; an explosion's removal without one; the dragon's and
//! fire's hurts refused; and across a line, a hit and an explosion that reach a crystal
//! another region holds, which that region removes and tells its fight of.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::{
    BlockPos, Dimension, Double, EdgeId, Effected, EndCrystal, Entity, EntityChange, EntityData,
    EntityId, EntityUse, Float, ItemStack, PlayerId, Thrower,
};

use super::{CrystalCause, FightCall, take_fight_calls};
use crate::api::{Face, PlayerInput, RegionEvent, TickInputs, TickOutput};
use crate::mobs::Mob;
use crate::region::Region;
use crate::region::tnt::tests::{add_player, idle, primed, records, west_of_a_line, world};

const SEED: u64 = 0xE7_C2_45;

/// Where player 1 stands unless a test says otherwise.
const SPAWN: [f64; 3] = [0.5, -60.0, 0.5];

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// Obsidian at (3, -60, 3) and (8, -60, 3), bedrock at (3, -60, 6), obsidian at
/// (5, -60, 6) with obsidian above it, and dirt at (5, -60, 1) with dirt above it and at
/// (6, -59, 3), on the stone floor.
fn blocks_of(pos: BlockPos) -> Option<BlockState> {
    match (pos.x, pos.y, pos.z) {
        (3, -60, 3) | (8, -60, 3) | (1, -60, 3) => Some(blocks::OBSIDIAN),
        (3, -60, 6) => Some(blocks::BEDROCK),
        (5, -60, 6) | (5, -59, 6) => Some(blocks::OBSIDIAN),
        (5, -60, 1) | (5, -59, 1) | (6, -59, 3) => Some(blocks::DIRT),
        _ => None,
    }
}

/// The world with [`blocks_of`] and player 1 at `feet` holding a crystal.
fn holding_a_crystal(dimension: Dimension, feet: [f64; 3]) -> (Region, PlayerId, EntityId) {
    let mut region = world(SEED, blocks_of);
    region.config.dimension = dimension;
    let (id, entity) = add_player(&mut region, 1, feet, false);
    let player = region.players.get_mut(&id).expect("player 1");
    player.inventory.0[0] = Some(ItemStack::new(items::END_CRYSTAL, 1));
    player.selected_slot = 0;
    take_fight_calls();
    (region, id, entity)
}

/// Player 1's click with what they hold on the top of `position`, their input `number`.
fn click(
    region: &mut Region,
    (id, entity): (PlayerId, EntityId),
    number: u64,
    position: BlockPos,
) -> TickOutput {
    region.tick(&TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            entity,
            number,
            PlayerInput::UseItemOn {
                position,
                face: Face::Top,
                cursor: [0.5, 1.0, 0.5],
                sneaking: false,
                sequence: i32::try_from(number).expect("a small number"),
            },
        )],
        ..TickInputs::default()
    })
}

/// Player 1's hit on `target`, their input `number`.
fn attack(
    region: &mut Region,
    (id, entity): (PlayerId, EntityId),
    number: u64,
    target: EntityId,
) -> TickOutput {
    region.tick(&TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            entity,
            number,
            PlayerInput::Attack {
                entity: target,
                part: None,
            },
        )],
        ..TickInputs::default()
    })
}

fn crystals(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::EndCrystal(_)))
        .collect()
}

fn crystal_data(entity: &Entity) -> EndCrystal {
    match entity.data {
        EntityData::EndCrystal(crystal) => crystal,
        _ => panic!("not a crystal: {entity:?}"),
    }
}

/// A crystal as the spikes' feature leaves it, its base shown, at `at`.
fn crystal(key: EntityId, at: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::END_CRYSTAL),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 11),
        position: at,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 3,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::EndCrystal(EndCrystal {
            beam_target: None,
            show_bottom: true,
        }),
    }
}

/// A cow of full health at `feet` with no AI.
fn cow(key: EntityId, feet: [f64; 3]) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::COW),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
        motion: [0.0; 3],
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

/// Lays `entity` in the region's loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// Lays `entity` in the region's copy of its chunk.
fn lay_in_copy(region: &mut Region, entity: &Entity) {
    region
        .copies
        .get_mut(&entity.chunk())
        .expect("the chunk is copied")
        .apply_entity(&EntityChange::Whole(entity.clone()));
}

/// The explosions of `output`: their centres and radii.
fn explosions(output: &TickOutput) -> Vec<([f64; 3], f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion { centre, radius, .. } => Some((*centre, *radius)),
            _ => None,
        })
        .collect()
}

fn health(region: &Region, key: EntityId) -> f32 {
    let (_, record) = region.find_entity(key).expect("the mob is there");
    Mob::decode(&record.data)
        .expect("a mob's record")
        .body
        .vitals
        .health
}

fn player_cause(entity: EntityId, id: PlayerId) -> CrystalCause {
    CrystalCause {
        key: Some(entity),
        kind: kind(entity_types::PLAYER),
        player: Some(id.0),
    }
}

// The item (`EndCrystalItem.useOn`).

#[test]
fn a_crystal_is_placed_above_obsidian_without_its_base_and_the_creative_player_keeps_the_item() {
    let (mut region, id, entity) = holding_a_crystal(Dimension::Overworld, SPAWN);
    let output = click(&mut region, (id, entity), 1, BlockPos::new(3, -60, 3));
    let [placed] = &crystals(&region)[..] else {
        panic!("one crystal: {:?}", crystals(&region));
    };
    // At the bottom middle of the block above, unturned, its base hidden and no beam.
    assert_eq!(placed.position, [3.5, -59.0, 3.5]);
    assert_eq!((placed.yaw, placed.pitch), (0.0, 0.0));
    assert_eq!(
        crystal_data(placed),
        EndCrystal {
            beam_target: None,
            show_bottom: false
        }
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntitySpawned(state) if state.entity == placed.key
    )));
    let held = region.players.get(&id).expect("player 1").inventory.0[0].clone();
    assert_eq!(held, Some(ItemStack::new(items::END_CRYSTAL, 1)));
    // Outside the End there is no fight to ask, and no fire under it.
    assert_eq!(take_fight_calls(), []);
    for _ in 0..3 {
        idle(&mut region);
    }
    assert_eq!(region.block(BlockPos::new(3, -59, 3)), Some(blocks::AIR));
}

#[test]
fn a_crystal_goes_on_bedrock_too_but_not_on_stone_nor_under_a_block_nor_into_an_entity() {
    // Within reach of every block it clicks, and clear of every box above them.
    let (mut region, id, entity) = holding_a_crystal(Dimension::Overworld, [4.5, -60.0, 4.5]);
    let user = (id, entity);
    // Stone (the floor) takes none; nor obsidian with obsidian above it.
    click(&mut region, user, 1, BlockPos::new(2, -61, 2));
    click(&mut region, user, 3, BlockPos::new(5, -60, 6));
    assert_eq!(crystals(&region), []);
    // Bedrock takes one.
    click(&mut region, user, 4, BlockPos::new(3, -60, 6));
    let positions: Vec<[f64; 3]> = crystals(&region).iter().map(|c| c.position).collect();
    assert_eq!(positions, [[3.5, -59.0, 6.5]]);
    // A cow whose box reaches into the box above the obsidian at (8, -60, 3) refuses it:
    // its box is 0.9 wide, so at x = 9.4 it reaches x = 8.95.
    let cow_key = EntityId(500);
    lay(&mut region, &cow(cow_key, [9.4, -60.0, 3.5]));
    click(&mut region, user, 5, BlockPos::new(8, -60, 3));
    assert_eq!(crystals(&region).len(), 1);
    // A player standing on the obsidian at (1, -60, 3) refuses it too.
    add_player(&mut region, 2, [1.5, -59.0, 3.5], false);
    click(&mut region, user, 6, BlockPos::new(1, -60, 3));
    assert_eq!(crystals(&region).len(), 1);
    // The first crystal's own box (2 wide) reaches over the obsidian at (3, -60, 3)? It
    // stands at x = 3.5, z = 6.5 and reaches z = 5.5: not into z 3 to 4, so a crystal
    // goes there.
    click(&mut region, user, 7, BlockPos::new(3, -60, 3));
    assert_eq!(crystals(&region).len(), 2);
}

#[test]
fn a_crystal_placed_in_the_end_asks_the_fight_to_respawn_and_keeps_fire_under_it() {
    let (mut region, id, entity) = holding_a_crystal(Dimension::End, SPAWN);
    click(&mut region, (id, entity), 1, BlockPos::new(3, -60, 3));
    assert_eq!(take_fight_calls(), [FightCall::TryRespawn]);
    // Made before the entities' phase, it ticked in the tick it was made, and set fire
    // in its block of air on the obsidian.
    let [placed] = &crystals(&region)[..] else {
        panic!("one crystal: {:?}", crystals(&region));
    };
    assert_eq!(placed.lived, 1);
    assert_eq!(region.block(BlockPos::new(3, -59, 3)), Some(blocks::FIRE));
    // Put out, it is lit again at its next tick.
    let mut out = TickOutput::default();
    region.set_block_updated(
        BlockPos::new(3, -59, 3),
        blocks::AIR,
        super::super::updates::UPDATE_ALL,
        super::super::updates::UPDATE_LIMIT,
        &mut out,
    );
    idle(&mut region);
    assert_eq!(region.block(BlockPos::new(3, -59, 3)), Some(blocks::FIRE));
    let [aged] = &crystals(&region)[..] else {
        panic!("one crystal");
    };
    assert_eq!(aged.lived, 2);
}

// Its hurt (`EndCrystal.hurtServer`).

/// In the End (whose world begins at y = 0, below which nothing breaks): the fight is
/// told of both crystals, by the player, the other first, as the explosion takes it
/// before the hit crystal's own `onDestroyedBy`.
#[test]
fn a_hit_removes_the_crystal_with_one_explosion_that_takes_another_crystal_silently() {
    let (mut region, id, entity) = holding_a_crystal(Dimension::End, SPAWN);
    let hit = EntityId(600);
    let other = EntityId(601);
    lay(&mut region, &crystal(hit, [3.5, -59.0, 3.5]));
    lay(&mut region, &crystal(other, [6.5, -59.0, 5.5]));
    let output = attack(&mut region, (id, entity), 1, hit);
    assert_eq!(crystals(&region), []);
    // One explosion, the hit crystal's, at its feet: the other is removed by it without
    // one of its own.
    assert_eq!(explosions(&output), [([3.5, -59.0, 3.5], 6.0)]);
    let by = Some(player_cause(entity, id));
    assert_eq!(
        take_fight_calls(),
        [
            FightCall::Destroyed {
                crystal: other,
                cause: by
            },
            FightCall::Destroyed {
                crystal: hit,
                cause: by
            },
        ]
    );
}

#[test]
fn an_explosion_removes_a_crystal_without_an_explosion_of_its_own() {
    let mut region = world(SEED, blocks_of);
    region.config.dimension = Dimension::End;
    take_fight_calls();
    let key = EntityId(700);
    lay(&mut region, &crystal(key, [3.5, -59.0, 3.5]));
    primed(&mut region, [3.5, -60.0, 1.5], 2, None);
    let mut seen = Vec::new();
    for _ in 0..5 {
        let output = idle(&mut region);
        seen.extend(explosions(&output));
        if !seen.is_empty() {
            break;
        }
    }
    // The TNT's alone.
    let radii: Vec<f32> = seen.iter().map(|(_, radius)| *radius).collect();
    assert_eq!(radii, [4.0]);
    assert_eq!(crystals(&region), []);
    assert_eq!(
        take_fight_calls(),
        [FightCall::Destroyed {
            crystal: key,
            cause: None
        }]
    );
}

#[test]
fn the_dragon_s_hurt_and_fire_s_do_nothing_and_a_player_s_snowball_explodes_it() {
    let mut region = world(SEED, blocks_of);
    region.config.dimension = Dimension::End;
    take_fight_calls();
    let key = EntityId(800);
    let laid = crystal(key, [3.5, -59.0, 3.5]);
    lay(&mut region, &laid);
    let thrown = |kind: u16, owner: Option<Thrower>| EntityUse::Thrown {
        projectile: EntityId(900),
        kind,
        at: [Double(3.5), Double(-58.0), Double(1.0)],
        motion: [Double(0.0), Double(0.0), Double(0.5)],
        damage: Float(5.0),
        owner,
    };
    let dragon = Thrower {
        key: EntityId(901),
        kind: kind(entity_types::ENDER_DRAGON),
        from: [Double(0.0), Double(80.0), Double(0.0)],
        player: None,
    };
    let mut output = TickOutput::default();
    // The dragon's hit, by its own body or by what it shot.
    region.entity_used(
        key,
        EntityUse::MobHit {
            attacker: dragon.key,
            kind: dragon.kind,
            from: dragon.from,
            damage: Float(10.0),
            knockback: None,
            fling: None,
            sound: None,
            thorns: false,
        },
        &mut output,
    );
    region.entity_used(
        key,
        thrown(kind(entity_types::SNOWBALL), Some(dragon)),
        &mut output,
    );
    // A blaze's small fireball: fire, which a fire-immune type does not take.
    region.entity_used(
        key,
        thrown(kind(entity_types::SMALL_FIREBALL), None),
        &mut output,
    );
    assert_eq!(crystals(&region), [laid]);
    assert_eq!(explosions(&output), []);
    assert_eq!(take_fight_calls(), []);
    // A snowball a player threw: no damage to speak of, but any hurt removes it, and it
    // explodes caused by the player.
    let player = uuid::Uuid::from_u128(42);
    let by_player = Thrower {
        key: EntityId(1_000_042),
        kind: kind(entity_types::PLAYER),
        from: [Double(3.5), Double(-60.0), Double(0.5)],
        player: Some(player),
    };
    let mut thrown_snowball = thrown(kind(entity_types::SNOWBALL), Some(by_player));
    if let EntityUse::Thrown { damage, .. } = &mut thrown_snowball {
        *damage = Float(0.0);
    }
    region.entity_used(key, thrown_snowball, &mut output);
    assert_eq!(crystals(&region), []);
    assert_eq!(explosions(&output), [([3.5, -59.0, 3.5], 6.0)]);
    assert_eq!(
        take_fight_calls(),
        [FightCall::Destroyed {
            crystal: key,
            cause: Some(CrystalCause {
                key: Some(by_player.key),
                kind: by_player.kind,
                player: Some(player),
            })
        }]
    );
}

/// Outside the End: the explosion of 6 breaks the blocks about it and hurts the
/// entities, and no fight is told.
#[test]
fn a_hit_s_explosion_breaks_blocks_and_hurts_entities_and_outside_the_end_tells_no_fight() {
    let mut region = world(SEED, blocks_of);
    take_fight_calls();
    let key = EntityId(850);
    let cow_key = EntityId(851);
    lay(&mut region, &crystal(key, [3.5, -59.0, 3.5]));
    lay(&mut region, &cow(cow_key, [3.5, -60.0, 5.5]));
    let (id, entity) = add_player(&mut region, 1, SPAWN, false);
    let output = attack(&mut region, (id, entity), 1, key);
    assert_eq!(crystals(&region), []);
    assert_eq!(explosions(&output), [([3.5, -59.0, 3.5], 6.0)]);
    // The dirt at its own height is broken. Every ray that goes down from its feet,
    // which are on the obsidian's top, is in the obsidian at its second step (0.3 along
    // it, still above the obsidian's square), which takes all its strength
    // (`(1200 + 0.3) * 0.3`): the dirt and the floor below that height are left, and the
    // obsidian it stood on resists.
    assert_eq!(region.block(BlockPos::new(5, -59, 1)), Some(blocks::AIR));
    assert_eq!(region.block(BlockPos::new(6, -59, 3)), Some(blocks::AIR));
    assert_eq!(region.block(BlockPos::new(5, -60, 1)), Some(blocks::DIRT));
    assert_eq!(region.block(BlockPos::new(4, -61, 3)), Some(blocks::STONE));
    assert_eq!(
        region.block(BlockPos::new(3, -60, 3)),
        Some(blocks::OBSIDIAN)
    );
    // The cow two blocks away is hurt (a power of 6 so near kills it).
    assert!(
        region.find_entity(cow_key).is_none() || health(&region, cow_key) < 10.0,
        "the cow is hurt"
    );
    assert_eq!(take_fight_calls(), []);
}

// Across a line.

/// The plan, section 19.5 item 13 and the brief's 6.3: a player of the region west of
/// the line hits a crystal the east holds; the west sends the hit, and the east removes
/// the crystal, explodes it by that player, and tells its fight.
#[test]
fn a_hit_from_across_a_line_explodes_the_crystal_where_it_is_held_and_tells_the_fight() {
    let key = EntityId(950);
    let laid = crystal(key, [17.5, -60.0, 0.5]);
    let mut west = west_of_a_line(SEED, |_| None);
    let (id, entity) = add_player(&mut west, 1, [14.5, -60.0, 0.5], false);
    lay_in_copy(&mut west, &laid);
    let output = attack(&mut west, (id, entity), 1, key);
    assert_eq!(explosions(&output), []);
    let hits: Vec<EntityUse> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity { key: hit, by } if *hit == key => Some(by.clone()),
            _ => None,
        })
        .collect();
    let [by] = &hits[..] else {
        panic!("one hit on the crystal: {:?}", output.effects);
    };
    let mut east = world(SEED, |_| None);
    east.config.dimension = Dimension::End;
    lay(&mut east, &laid);
    take_fight_calls();
    let mut taken = TickOutput::default();
    east.entity_used(key, by.clone(), &mut taken);
    assert_eq!(crystals(&east), []);
    assert_eq!(explosions(&taken), [([17.5, -60.0, 0.5], 6.0)]);
    assert_eq!(
        take_fight_calls(),
        [FightCall::Destroyed {
            crystal: key,
            cause: Some(player_cause(entity, id)),
        }]
    );
}

/// An explosion in the west reaches a crystal the east holds: the west files the blast
/// on it, and the east removes it without an explosion and tells its fight.
#[test]
fn an_explosion_from_across_a_line_removes_the_crystal_silently_where_it_is_held() {
    let key = EntityId(960);
    let laid = crystal(key, [17.5, -60.0, 0.5]);
    let mut west = west_of_a_line(SEED, |_| None);
    lay_in_copy(&mut west, &laid);
    primed(&mut west, [14.5, -60.0, 0.5], 2, None);
    let mut blasts = Vec::new();
    for _ in 0..5 {
        let output = idle(&mut west);
        blasts.extend(
            output
                .effects
                .iter()
                .filter_map(|effect| match &effect.what {
                    Effected::OnEntity { key: hit, by } if *hit == key => Some(by.clone()),
                    _ => None,
                }),
        );
        if !blasts.is_empty() {
            break;
        }
    }
    let [by @ EntityUse::Blasted { .. }] = &blasts[..] else {
        panic!("one blast on the crystal: {blasts:?}");
    };
    let mut east = world(SEED, |_| None);
    east.config.dimension = Dimension::End;
    lay(&mut east, &laid);
    take_fight_calls();
    let mut taken = TickOutput::default();
    east.entity_used(key, by.clone(), &mut taken);
    assert_eq!(crystals(&east), []);
    assert_eq!(explosions(&taken), []);
    assert_eq!(
        take_fight_calls(),
        [FightCall::Destroyed {
            crystal: key,
            cause: None
        }]
    );
}

// Its beam.

#[test]
fn a_beam_set_is_committed_and_reported_with_the_crystal_s_data() {
    let mut region = world(SEED, blocks_of);
    let key = EntityId(990);
    lay(&mut region, &crystal(key, [3.5, -59.0, 3.5]));
    let mut output = TickOutput::default();
    let target = Some(BlockPos::new(0, 128, 0));
    assert!(region.set_end_crystal_beam(key, target, &mut output));
    let (_, record) = region.find_entity(key).expect("the crystal");
    assert_eq!(crystal_data(record).beam_target, target);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(report)
            if report.entity == key
                && report.data.as_ref().is_some_and(|data| matches!(
                    data,
                    EntityData::EndCrystal(EndCrystal { beam_target, .. }) if *beam_target == target
                ))
    )));
    assert!(!output.entity_changes.is_empty());
    assert!(!region.set_end_crystal_beam(EntityId(991), None, &mut output));
}
