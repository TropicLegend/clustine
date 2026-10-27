//! The iron and snow golems in a region (`docs/groundwork/n7k-brief.md`, sections 1.2
//! to 1.5, 11.6 and 11.7; the plan's sections 1.1, 2.5, 2.6 and 10), as a player meets
//! them: built from a pumpkin in any of the pattern's orientations and nowhere a match
//! touches another region's land, an iron golem repaired by an ingot, cracking and
//! fighting a monster with its fling, also across a pinned line, and a snow golem's
//! trail, melting, hurt by water, shorn and throwing snowballs at a monster.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{Effected, Entity, EntityChange, EntityId, EntityUse, ItemStack};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::types::{copper_golem, iron_golem, snow_golem};
use crate::region::Random;
use crate::region::updates::{UPDATE_ALL, UPDATE_LIMIT};

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(41);
/// Keyed below the golems, so that it ticks before them and its record holds what
/// their hits leave it with.
const FOE: EntityId = EntityId(30);

/// A mob of the type `kind` at `position`, of its type's full health and air.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
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

/// A zombie at `position` wearing a carved pumpkin, which keeps the sun off it and
/// draws nothing.
fn zombie_at(key: EntityId, position: [f64; 3]) -> Entity {
    let mut zombie = Mob::fresh();
    zombie.body.equipment.slots[usize::from(crate::living::slots::HEAD)] =
        one(items::CARVED_PUMPKIN);
    mob_at(key, entity_types::ZOMBIE, position, zombie)
}

/// `entity`, a mob, at `health`.
fn with_health(mut entity: Entity, health: f32) -> Entity {
    let mut mob = Mob::decode(&entity.data).unwrap();
    mob.body.vitals.health = health;
    entity.data = mob.encode();
    entity
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

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
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

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

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

fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

/// Player 1's use of the mob `key` holding `held`, between ticks: the mob's stream aside
/// as it was before, and what the use showed.
fn used_now(region: &mut Region, key: EntityId, held: Option<ItemStack>) -> (Random, TickOutput) {
    let stream = region.entity_stream(key).random;
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .unwrap();
    let player_key = region.players[&player(1)].entity_id;
    let from = region.players[&player(1)].pose.position;
    let mut output = TickOutput::default();
    region.mob_used_by(
        &entity,
        chunk,
        player_key,
        held,
        true,
        false,
        [from.x, from.y, from.z],
        &mut output,
    );
    (stream, output)
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, 1))
}

/// A pumpkin set at `head` as a placement sets it, with every update, between ticks.
fn pumpkin_at(region: &mut Region, head: BlockPos) -> TickOutput {
    let mut output = TickOutput::default();
    region.set_block_updated(
        head,
        blocks::CARVED_PUMPKIN,
        UPDATE_ALL,
        UPDATE_LIMIT,
        &mut output,
    );
    output
}

/// The draws of a made mob's constructor of its own stream aside: its UUID's two and
/// its yaw's one; a golem's constructor draws nothing more.
fn constructor_draws(region: &Region, key: EntityId) -> u64 {
    let start = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
    let mut own = start.clone();
    own.next_u64();
    own.next_u64();
    own.next_float();
    own.draws_since(&start)
}

// ------------------------------------------------------------------------------------
// Building.

/// An iron golem's nine cells, by what is there: the pumpkin, the four iron blocks and
/// the four cells of air, each by its place.
struct Iron {
    head: BlockPos,
    iron: [BlockPos; 4],
    air: [BlockPos; 4],
    leg: BlockPos,
}

impl Iron {
    /// The iron golem whose head is at `head`, whose body runs from it along `down`
    /// and whose arms run along `across`.
    fn at(head: BlockPos, down: [i32; 3], across: [i32; 3]) -> Self {
        let step = |from: BlockPos, by: [i32; 3], times: i32| {
            from.offset(by[0] * times, by[1] * times, by[2] * times)
        };
        let body = step(head, down, 1);
        let leg = step(head, down, 2);
        Self {
            head,
            iron: [step(body, across, -1), body, step(body, across, 1), leg],
            air: [
                step(head, across, -1),
                step(head, across, 1),
                step(leg, across, -1),
                step(leg, across, 1),
            ],
            leg,
        }
    }

    fn blocks(&self) -> Vec<(BlockPos, BlockState)> {
        self.iron
            .iter()
            .map(|at| (*at, blocks::IRON_BLOCK))
            .collect()
    }
}

#[test]
fn an_iron_golem_is_built_upright_either_way_lying_and_upside_down_where_the_game_puts_it() {
    let shapes = [
        // Upright, its arms along x, then along z.
        Iron::at(BlockPos::new(4, -58, 4), [0, -1, 0], [1, 0, 0]),
        Iron::at(BlockPos::new(4, -58, 4), [0, -1, 0], [0, 0, 1]),
        // Lying on the ground, its head to the north, then to the west.
        Iron::at(BlockPos::new(4, -60, 2), [0, 0, 1], [1, 0, 0]),
        Iron::at(BlockPos::new(2, -60, 4), [1, 0, 0], [0, 0, 1]),
        // Upside down, its head on the ground.
        Iron::at(BlockPos::new(4, -60, 4), [0, 1, 0], [1, 0, 0]),
    ];
    for shape in shapes {
        let mut region = on_ticking_floor_with(&shape.blocks());
        let output = pumpkin_at(&mut region, shape.head);
        // Every cell is broken as it was, the air too, and is air now.
        let mut broken: Vec<(BlockPos, i32)> = level_events(&output)
            .into_iter()
            .filter(|(kind, _, _)| *kind == 2001)
            .map(|(_, at, data)| (at, data))
            .collect();
        broken.sort_by_key(|(at, _)| (at.x, at.y, at.z));
        let mut wanted: Vec<(BlockPos, i32)> = shape
            .iron
            .iter()
            .map(|at| (*at, i32::from(blocks::IRON_BLOCK.0)))
            .chain(shape.air.iter().map(|at| (*at, i32::from(blocks::AIR.0))))
            .chain([(shape.head, i32::from(blocks::CARVED_PUMPKIN.0))])
            .collect();
        wanted.sort_by_key(|(at, _)| (at.x, at.y, at.z));
        assert_eq!(broken, wanted);
        for at in shape.iron.iter().chain([&shape.head]) {
            assert_eq!(region.block(*at), Some(blocks::AIR), "{at:?}");
        }
        // The golem stands at its leg, facing south, a player's, made without
        // `finalizeSpawn`: nothing drawn but its constructor's.
        let golems = all_of(&region, entity_types::IRON_GOLEM);
        assert_eq!(golems.len(), 1);
        let golem = &golems[0];
        let leg = shape.leg;
        assert_eq!(
            golem.position,
            [
                f64::from(leg.x) + 0.5,
                f64::from(leg.y) + 0.05,
                f64::from(leg.z) + 0.5
            ]
        );
        assert_eq!((golem.yaw, golem.pitch), (0.0, 0.0));
        let made = Mob::decode(&golem.data).unwrap();
        assert!(iron_golem::state_of(&made).player_created);
        assert!(!made.body.flags.left_handed);
        assert!(
            made.body
                .attributes
                .made(clustine_data::attributes::FOLLOW_RANGE as u16)
                .is_none()
        );
        assert_eq!(
            region.aside_draws.get(&golem.key).copied(),
            Some(constructor_draws(&region, golem.key))
        );
        let values = crate::shown::mob_values(golem.kind, &golem.data);
        assert!(values.contains(&(16, crate::shown::DataShown::Byte(1), false)));
    }
}

#[test]
fn a_jack_o_lantern_builds_one_too_and_a_pumpkin_on_a_broken_pattern_builds_nothing() {
    let shape = Iron::at(BlockPos::new(4, -58, 4), [0, -1, 0], [1, 0, 0]);
    let mut region = on_ticking_floor_with(&shape.blocks());
    let mut output = TickOutput::default();
    region.set_block_updated(
        shape.head,
        blocks::JACK_O_LANTERN,
        UPDATE_ALL,
        UPDATE_LIMIT,
        &mut output,
    );
    assert_eq!(all_of(&region, entity_types::IRON_GOLEM).len(), 1);

    // A shoulder that is not air spoils it.
    let mut spoilt = shape.blocks();
    spoilt.push((shape.air[0], blocks::STONE));
    let mut region = on_ticking_floor_with(&spoilt);
    pumpkin_at(&mut region, shape.head);
    assert!(all_of(&region, entity_types::IRON_GOLEM).is_empty());
    assert_eq!(region.block(shape.iron[1]), Some(blocks::IRON_BLOCK));
}

#[test]
fn a_snow_golem_is_found_before_an_iron_golem_the_same_pumpkin_completes() {
    let head = BlockPos::new(4, -58, 4);
    // Two snow blocks under the head, and an iron golem lying north of it at its height.
    let iron = Iron::at(head, [0, 0, -1], [1, 0, 0]);
    let mut set = iron.blocks();
    set.push((BlockPos::new(4, -59, 4), blocks::SNOW_BLOCK));
    set.push((BlockPos::new(4, -60, 4), blocks::SNOW_BLOCK));
    let mut region = on_ticking_floor_with(&set);
    pumpkin_at(&mut region, head);
    let snow = all_of(&region, entity_types::SNOW_GOLEM);
    assert_eq!(snow.len(), 1);
    assert_eq!(snow[0].position, [4.5, -59.95, 4.5]);
    assert!(all_of(&region, entity_types::IRON_GOLEM).is_empty());
    for at in iron.iron {
        assert_eq!(region.block(at), Some(blocks::IRON_BLOCK));
    }
    let made = Mob::decode(&snow[0].data).unwrap();
    assert!(snow_golem::has_pumpkin(&made));
    assert_eq!(
        region.aside_draws.get(&snow[0].key).copied(),
        Some(constructor_draws(&region, snow[0].key))
    );
}

#[test]
fn a_copper_golem_is_built_lying_down_at_its_pumpkin_over_its_chest() {
    let head = BlockPos::new(4, -60, 4);
    let copper = BlockPos::new(5, -60, 4);
    let mut region = on_ticking_floor_with(&[(copper, blocks::WEATHERED_COPPER)]);
    let facing = clustine_blocks::data::set(blocks::CARVED_PUMPKIN, "facing", "west");
    let mut output = TickOutput::default();
    region.set_block_updated(head, facing, UPDATE_ALL, UPDATE_LIMIT, &mut output);
    let broken: Vec<_> = level_events(&output)
        .into_iter()
        .filter(|(kind, _, _)| *kind == 2001)
        .collect();
    assert_eq!(
        broken,
        [
            (2001, head, i32::from(facing.0)),
            (2001, copper, i32::from(blocks::WEATHERED_COPPER.0))
        ]
    );
    let golems = all_of(&region, entity_types::COPPER_GOLEM);
    assert_eq!(golems.len(), 1);
    assert_eq!(golems[0].position, [4.5, -59.95, 4.5]);
    let own = copper_golem::state_of(&Mob::decode(&golems[0].data).unwrap());
    assert_eq!(own.weather, copper_golem::WEATHERED);
    let chest = region.block(copper).unwrap();
    assert_eq!(
        chest.block().unwrap().name,
        "minecraft:weathered_copper_chest"
    );
    assert_eq!(chest.property("facing"), Some("west"));
    assert_eq!(region.block(head), Some(blocks::AIR));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_COPPER_GOLEM_SPAWN));
}

/// A region of the land west of chunk x = 2 (x below 32), its chunks x of -1 to 1 by z
/// of -1 to 1 loaded on a stone floor with `here` set and `own` laid, and the chunks of
/// x = 2 another's, copied, with `beyond` set and `entities` in the one at z = 0.
fn at_a_line(
    here: &[(BlockPos, BlockState)],
    own: &[Entity],
    beyond: &[(BlockPos, BlockState)],
    entities: &[Entity],
) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let set = |chunk: &mut Chunk, position: ChunkPos, blocks: &[(BlockPos, BlockState)]| {
        for (at, state) in blocks {
            if at.chunk() == position {
                chunk.set(
                    at.x.rem_euclid(16) as usize,
                    at.y,
                    at.z.rem_euclid(16) as usize,
                    *state,
                );
            }
        }
    };
    let chunks = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            set(&mut chunk, *position, here);
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    for entity in own {
        lay(&mut region, entity);
    }
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    let bases = copies
        .iter()
        .map(|position| {
            let mut base = floor();
            set(&mut base, *position, beyond);
            if position.z == 0 {
                base.set_entities(entities.to_vec());
            }
            (*position, base)
        })
        .collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: bases,
        ..TickInputs::default()
    });
    region
}

#[test]
fn a_match_touching_another_region_s_land_builds_nothing_and_no_later_match_is_tried() {
    // A snow golem lying east across the line from its head, and a copper golem lying
    // west of the same head, all on this side.
    let head = BlockPos::new(31, -60, 4);
    let copper = BlockPos::new(30, -60, 4);
    let snow = [
        (BlockPos::new(32, -60, 4), blocks::SNOW_BLOCK),
        (BlockPos::new(33, -60, 4), blocks::SNOW_BLOCK),
    ];
    let mut region = at_a_line(&[(copper, blocks::COPPER_BLOCK)], &[], &snow, &[]);
    let output = pumpkin_at(&mut region, head);
    assert!(all_of(&region, entity_types::SNOW_GOLEM).is_empty());
    assert!(all_of(&region, entity_types::COPPER_GOLEM).is_empty());
    assert!(
        level_events(&output)
            .iter()
            .all(|(kind, _, _)| *kind != 2001)
    );
    assert!(clustine_blocks::data::is_block(
        region.block(head).unwrap(),
        blocks::CARVED_PUMPKIN
    ));
    assert_eq!(region.block(copper), Some(blocks::COPPER_BLOCK));
    // Nothing beyond is cleared: the only effects are the pumpkin's own updates.
    assert!(
        region
            .made
            .iter()
            .all(|effect| !matches!(effect.what, Effected::SetIf { .. })),
        "{:?}",
        region.made
    );

    // Without the snow beyond, the copper golem is the first match, and is built.
    let mut region = at_a_line(&[(copper, blocks::COPPER_BLOCK)], &[], &[], &[]);
    pumpkin_at(&mut region, head);
    assert_eq!(all_of(&region, entity_types::COPPER_GOLEM).len(), 1);
}

// ------------------------------------------------------------------------------------
// The iron golem.

#[test]
fn an_iron_ingot_repairs_a_hurt_iron_golem_with_two_draws_and_not_a_whole_one() {
    let mut region = on_ticking_floor_with(&[]);
    let golem = mob_at(
        ONE,
        entity_types::IRON_GOLEM,
        [3.5, -60.0, 3.5],
        Mob::fresh(),
    );
    lay(&mut region, &with_health(golem, 50.0));
    let (mut stream, output) = used_now(&mut region, ONE, one(items::IRON_INGOT));
    assert_eq!(mob(&region, ONE).body.vitals.health, 75.0);
    let pitch = 1.0 + (stream.next_float() - stream.next_float()) * 0.2;
    let played: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, pitch, .. } => Some((*sound, *pitch)),
            _ => None,
        })
        .collect();
    assert_eq!(played, [(sounds::ENTITY_IRON_GOLEM_REPAIR, pitch)]);
    // Twice more brings it to its most, and then an ingot does nothing.
    used_now(&mut region, ONE, one(items::IRON_INGOT));
    assert_eq!(mob(&region, ONE).body.vitals.health, 100.0);
    let (_, output) = used_now(&mut region, ONE, one(items::IRON_INGOT));
    assert!(!sounds_of(&output).contains(&sounds::ENTITY_IRON_GOLEM_REPAIR));
    assert_eq!(mob(&region, ONE).body.vitals.health, 100.0);
}

#[test]
fn an_iron_golem_s_damage_sound_plays_only_when_a_hurt_takes_it_below_a_crack_level() {
    let mut region = on_ticking_floor_with(&[]);
    for (key, health, amount, cracks) in [
        // To three quarters exactly: no crack yet.
        (ONE, 80.0, 5.0, false),
        // Below three quarters.
        (TWO, 80.0, 5.5, true),
        // Within a level.
        (EntityId(42), 60.0, 5.0, false),
        // Below a quarter.
        (EntityId(43), 30.0, 6.0, true),
    ] {
        let golem = mob_at(
            key,
            entity_types::IRON_GOLEM,
            [3.5, -60.0, 3.5],
            Mob::fresh(),
        );
        lay(&mut region, &with_health(golem, health));
        let mut output = TickOutput::default();
        let source = DamageSource::of(DamageType::MAGIC);
        assert!(region.hurt_mob(key, &source, amount, &mut output));
        let heard = sounds_of(&output);
        assert!(heard.contains(&sounds::ENTITY_IRON_GOLEM_HURT));
        assert_eq!(
            heard.contains(&sounds::ENTITY_IRON_GOLEM_DAMAGE),
            cracks,
            "{health} less {amount}"
        );
        // What its hurt kept of its cracks is gone with the hurt.
        assert_eq!(iron_golem::state_of(&mob(&region, key)).cracks_before, None);
    }
}

#[test]
fn an_iron_golem_fights_a_zombie_hitting_it_by_its_draw_and_flinging_it() {
    let mut region = on_ticking_floor_with(&[]);
    // The zombie fights back (N9 S6), which moves the golem's stream: given more health
    // than its own, it outlives a hit that would kill one of 20, and the hit is read
    // whole.
    lay(
        &mut region,
        &with_health(zombie_at(FOE, [8.5, -60.0, 4.5]), 40.0),
    );
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::IRON_GOLEM,
            [3.5, -60.0, 4.5],
            Mob::fresh(),
        ),
    );
    let mut hit = None;
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        let foe = mob(&region, FOE);
        if foe.body.vitals.health < 40.0 {
            hit = Some((output, foe, record(&region, FOE).unwrap()));
            break;
        }
    }
    let (output, foe, foe_record) = hit.expect("the golem hits the zombie");
    assert_eq!(mob(&region, ONE).body.combat.target, Some(FOE));
    // Its arms swing and its attack sounds.
    assert!(entity_events(&output, ONE).contains(&4));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_IRON_GOLEM_ATTACK));
    let own = iron_golem::state_of(&mob(&region, ONE));
    assert!(iron_golem::attack_animation(&own, record(&region, ONE).unwrap().lived) > 0);
    // Half its attack of 15 and a draw of the whole: 7.5 and a whole number up to 14,
    // less what the zombie's armour of 2 takes off a hit that hard (`CombatRules`: a
    // fifth of the armour, of 25).
    let lost = 40.0 - foe.body.vitals.health;
    let hit = lost / (1.0 - 0.4 / 25.0);
    let drawn = hit - 7.5;
    assert!(
        (0.0..=14.0).contains(&drawn) && (drawn - drawn.round()).abs() < 1e-3,
        "{lost}"
    );
    // The hit's own knockback lifts it at most 0.4; the fling adds 0.4 more.
    assert!(foe_record.motion[1] > 0.7, "{:?}", foe_record.motion);
}

#[test]
fn across_a_pinned_line_an_iron_golem_s_hit_carries_its_fling_which_lands_only_where_the_hurt_takes()
 {
    let zombie = zombie_at(FOE, [33.5, -60.0, 4.5]);
    let golem = mob_at(
        ONE,
        entity_types::IRON_GOLEM,
        [29.5, -60.0, 4.5],
        Mob::fresh(),
    );
    let mut region = at_a_line(&[], &[golem], &[], &[zombie]);
    let mut sent = None;
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        sent = output.effects.iter().find_map(|effect| match &effect.what {
            Effected::OnEntity {
                key,
                by:
                    EntityUse::MobHit {
                        attacker,
                        kind,
                        from,
                        damage,
                        knockback,
                        fling,
                        sound,
                        thorns: false,
                    },
            } if *key == FOE => Some((
                *attacker,
                *kind,
                from.map(|value| value.0),
                damage.0,
                *knockback,
                fling.map(|up| up.0),
                *sound,
            )),
            _ => None,
        });
        if sent.is_some() {
            break;
        }
    }
    let (attacker, kind, from, damage, knockback, fling, sound) =
        sent.expect("the golem hits the zombie across the line");
    assert_eq!(attacker, ONE);
    assert_eq!(i32::from(kind), entity_types::IRON_GOLEM);
    assert_eq!(fling, Some(iron_golem::FLING));
    assert_eq!((knockback, sound), (None, None));
    assert!((7.5..=21.5).contains(&damage) && (damage - 7.5).fract() == 0.0);
    // The zombie's holder: the first hit takes and flings it; a second as hard within
    // its cooldown does not take, and flings nothing.
    let mut holder = on_ticking_floor_with(&[]);
    lay(&mut holder, &zombie_at(FOE, [4.5, -60.0, 4.5]));
    let mut output = TickOutput::default();
    holder.hit_by_mob(
        attacker,
        kind,
        from,
        FOE,
        damage.min(10.0),
        (None, fling),
        None,
        &mut output,
    );
    let flung = record(&holder, FOE).unwrap().motion[1];
    assert!(flung > 0.7, "{flung}");
    let mut stilled = record(&holder, FOE).unwrap();
    stilled.motion = [0.0; 3];
    lay(&mut holder, &stilled);
    holder.hit_by_mob(
        attacker,
        kind,
        from,
        FOE,
        damage.min(10.0),
        (None, fling),
        None,
        &mut output,
    );
    assert_eq!(record(&holder, FOE).unwrap().motion, [0.0; 3]);
}

#[test]
fn an_iron_golem_from_an_egg_draws_mob_s_spawn_draws_and_is_no_player_s() {
    let mut region = on_ticking_floor_with(&[]);
    let player_key = region.players[&player(1)].entity_id;
    let mut level = region.entity_stream(player_key).random;
    let mut output = TickOutput::default();
    for (kind, at) in [
        (entity_types::IRON_GOLEM, BlockPos::new(3, -60, 3)),
        (entity_types::SNOW_GOLEM, BlockPos::new(6, -60, 3)),
    ] {
        let key = region
            .spawn_mob(kind as u16, player(1), at, true, true, &mut output)
            .expect("made");
        let made = mob(&region, key);
        let _yaw = level.next_float();
        let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        let left = level.next_float() < 0.05;
        let follow = made
            .body
            .attributes
            .made(clustine_data::attributes::FOLLOW_RANGE as u16)
            .unwrap();
        assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
        assert_eq!(made.body.flags.left_handed, left);
        let values = crate::shown::mob_values(kind as u16, &record(&region, key).unwrap().data);
        if kind == entity_types::IRON_GOLEM {
            assert!(!iron_golem::state_of(&made).player_created);
            assert_eq!(made.body.vitals.health, 100.0);
            assert!(values.contains(&(16, crate::shown::DataShown::Byte(0), true)));
        } else {
            assert!(snow_golem::has_pumpkin(&made));
            assert_eq!(made.body.vitals.health, 4.0);
            assert!(values.contains(&(16, crate::shown::DataShown::Byte(0x10), true)));
        }
    }
}

// ------------------------------------------------------------------------------------
// The snow golem.

/// A snow golem at `position` that thinks of nothing: only its body and its own step.
fn still_snow_golem(key: EntityId, position: [f64; 3]) -> Entity {
    let mut golem = Mob::fresh();
    golem.body.flags.no_ai = true;
    mob_at(key, entity_types::SNOW_GOLEM, position, golem)
}

fn snow_set(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } if *state == blocks::SNOW => Some(*position),
            _ => None,
        })
        .collect()
}

#[test]
fn a_snow_golem_leaves_one_layer_at_each_corner_where_a_layer_stands_each_cell_once() {
    // Its four corners in one cell: one layer, set once.
    let mut region = on_ticking_floor_with(&[(BlockPos::new(8, -61, 8), blocks::ICE)]);
    lay(&mut region, &still_snow_golem(ONE, [4.5, -60.0, 4.5]));
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(snow_set(&output), [BlockPos::new(4, -60, 4)]);
    assert_eq!(region.block(BlockPos::new(4, -60, 4)), Some(blocks::SNOW));
    // Astride four cells: the four, in its order.
    lay(&mut region, &still_snow_golem(TWO, [6.0, -60.0, 6.0]));
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(
        snow_set(&output),
        [
            BlockPos::new(5, -60, 5),
            BlockPos::new(6, -60, 5),
            BlockPos::new(5, -60, 6),
            BlockPos::new(6, -60, 6),
        ]
    );
    // On ice no layer stands.
    lay(
        &mut region,
        &still_snow_golem(EntityId(42), [8.5, -60.0, 8.5]),
    );
    let output = checked(&mut region, &TickInputs::default());
    assert!(snow_set(&output).is_empty());
    assert_eq!(region.block(BlockPos::new(8, -60, 8)), Some(blocks::AIR));
}

#[test]
fn across_a_pinned_line_a_snow_golem_s_trail_is_an_effect_that_expects_air() {
    let golem = still_snow_golem(ONE, [31.9, -60.0, 4.5]);
    let mut region = at_a_line(&[], &[golem], &[], &[]);
    // Laid before the copies came, its first tick left its own cell snow and read
    // nothing beyond; with the copy its next sends the cell beyond, and its own is
    // snow already.
    assert_eq!(region.block(BlockPos::new(31, -60, 4)), Some(blocks::SNOW));
    let output = checked(&mut region, &TickInputs::default());
    assert!(snow_set(&output).is_empty());
    let beyond: Vec<_> = output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::SetIf {
                expected,
                state,
                flags,
            } => Some((effect.at, expected, state, flags)),
            _ => None,
        })
        .collect();
    assert_eq!(
        beyond,
        [(
            BlockPos::new(32, -60, 4),
            blocks::AIR,
            blocks::SNOW,
            UPDATE_ALL
        )]
    );
}

/// The stone floor's chunks with the biome `biome` throughout.
fn on_floor_of(biome: &str) -> Region {
    let index = clustine_data::synced_registry("minecraft:worldgen/biome")
        .unwrap()
        .entries
        .iter()
        .position(|name| *name == biome)
        .unwrap();
    let overworld = clustine_data::DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .unwrap();
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| {
            let mut chunk = Chunk::empty(overworld, clustine_world::Biome(index as u16));
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, -61, z, blocks::STONE);
                }
            }
            (position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    region
}

#[test]
fn a_snow_golem_melts_in_a_desert_and_not_in_the_plains() {
    for (biome, health) in [("minecraft:desert", 3.0), ("minecraft:plains", 4.0)] {
        let mut region = on_floor_of(biome);
        lay(&mut region, &still_snow_golem(ONE, [4.5, -60.0, 4.5]));
        let output = checked(&mut region, &TickInputs::default());
        assert_eq!(mob(&region, ONE).body.vitals.health, health, "{biome}");
        assert_eq!(
            sounds_of(&output).contains(&sounds::ENTITY_SNOW_GOLEM_HURT),
            health < 4.0
        );
    }
}

#[test]
fn a_snow_golem_in_water_is_hurt_by_it_and_leaves_no_snow() {
    let mut water = Vec::new();
    for x in 3..=6 {
        for z in 3..=6 {
            water.push((BlockPos::new(x, -60, z), blocks::WATER));
        }
    }
    let mut region = on_ticking_floor_with(&water);
    lay(&mut region, &still_snow_golem(ONE, [4.5, -60.0, 4.5]));
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(mob(&region, ONE).body.vitals.health, 3.0);
    assert!(snow_set(&output).is_empty());
    // Its hurt's cooldown: the next is 10 ticks on.
    idle(&mut region, 9);
    assert_eq!(mob(&region, ONE).body.vitals.health, 3.0);
    idle(&mut region, 1);
    assert_eq!(mob(&region, ONE).body.vitals.health, 2.0);
}

#[test]
fn shears_take_a_snow_golem_s_pumpkin_dropped_at_its_eyes_once() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &still_snow_golem(ONE, [3.5, -60.0, 3.5]));
    let (_, output) = used_now(&mut region, ONE, one(items::SHEARS));
    let shorn = mob(&region, ONE);
    assert!(!snow_golem::has_pumpkin(&shorn));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_SNOW_GOLEM_SHEAR));
    let dropped: Vec<_> = all_of(&region, entity_types::ITEM)
        .into_iter()
        .map(|item| item.position)
        .collect();
    assert_eq!(dropped.len(), 1);
    assert!((dropped[0][1] - (-60.0 + 1.7)).abs() < 1e-6, "{dropped:?}");
    let values = crate::shown::mob_values(
        entity_types::SNOW_GOLEM as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(values.contains(&(16, crate::shown::DataShown::Byte(0), false)));
    // Shorn, it has nothing more to give.
    let (_, output) = used_now(&mut region, ONE, one(items::SHEARS));
    assert!(!sounds_of(&output).contains(&sounds::ENTITY_SNOW_GOLEM_SHEAR));
    assert_eq!(all_of(&region, entity_types::ITEM).len(), 1);
}

#[test]
fn a_snow_golem_throws_snowballs_at_a_zombie_within_ten_with_its_sound() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &zombie_at(FOE, [9.5, -60.0, 4.5]));
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::SNOW_GOLEM,
            [3.5, -60.0, 4.5],
            Mob::fresh(),
        ),
    );
    let mut thrown = false;
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        if !all_of(&region, entity_types::SNOWBALL).is_empty() {
            assert!(sounds_of(&output).contains(&sounds::ENTITY_SNOW_GOLEM_SHOOT));
            thrown = true;
            break;
        }
    }
    assert!(thrown);
    assert_eq!(mob(&region, ONE).body.combat.target, Some(FOE));
}

#[test]
fn an_iron_golem_s_poppy_goes_onto_an_empty_antenna_of_a_mob_that_accepts_it_only() {
    let mut region = on_ticking_floor_with(&[]);
    let copper = mob_at(
        TWO,
        entity_types::COPPER_GOLEM,
        [4.5, -60.0, 4.5],
        Mob::fresh(),
    );
    let cow = mob_at(
        EntityId(42),
        entity_types::COW,
        [6.5, -60.0, 4.5],
        Mob::fresh(),
    );
    lay(&mut region, &copper);
    lay(&mut region, &cow);
    let antenna = usize::from(crate::living::slots::SADDLE);
    let mut output = TickOutput::default();
    region.apply_locals(
        ONE,
        vec![
            crate::mobs::tick::Local::GiveFlower { key: TWO },
            crate::mobs::tick::Local::GiveFlower { key: EntityId(42) },
        ],
        &mut output,
    );
    let given = mob(&region, TWO);
    assert_eq!(given.body.equipment.slots[antenna], one(items::POPPY));
    // A sure drop.
    assert_eq!(given.body.equipment.drop_chance(antenna), 2.0);
    assert_eq!(
        mob(&region, EntityId(42)).body.equipment.slots[antenna],
        None
    );
    // Its antenna full, a second gift is not taken.
    let mut held = mob(&region, TWO);
    held.body.equipment.slots[antenna] = one(items::DANDELION);
    let mut laid = record(&region, TWO).unwrap();
    laid.data = held.encode();
    lay(&mut region, &laid);
    region.apply_locals(
        ONE,
        vec![crate::mobs::tick::Local::GiveFlower { key: TWO }],
        &mut output,
    );
    assert_eq!(
        mob(&region, TWO).body.equipment.slots[antenna],
        one(items::DANDELION)
    );
}
