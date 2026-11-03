//! Explosions, the dragon and falling blocks on players (V4a, `docs/groundwork/v-plan.md`,
//! section 2.3), from the game's rules: `ServerExplosion.hurtEntities` (n6, @154-528)
//! hurts a player by `ExplosionDamageCalculator.getEntityDamageAmount`'s
//! `((i² + i) / 2 · 7 · 2r + 1)`, `i = (1 − d / 2r) · seen`, before it pushes them by
//! `(1 − d / 2r) · seen · (1 − explosion_knockback_resistance)`; Blast Protection takes
//! `2 · level` protection points off the hurt (`× (1 − p / 25)`) and `0.15` a level off
//! the push; a creative player is pushed and not hurt; a bed in the Nether explodes with
//! `bad_respawn_point`; a falling anvil hurts `min(⌊2 · ⌈fallen − 1⌉⌋, 40)`
//! (`FallingBlockEntity.causeFallDamage`), and something on the head takes a quarter of
//! it off (`damages_helmet`); the dragon's wing hurts a survival player by 5
//! (`mobAttack`) and leaves a creative one be. The world is easy (`Difficulty::WORLD`),
//! whose scaling of a hurt that scales is `min(a / 2 + 1, a)`.
//!
//! The world is the overworld's floor of stone at y = -61 over 5 × 5 chunks about the
//! origin (`tnt/tests.rs`).

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::components::Component;
use clustine_world::{BlockPos, EntityData, EntityId, GameMode, ItemStack, PlayerId, PlayerMode};

use crate::api::{PlayerEvent, RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types::ender_dragon::{DRAGON, change, phases};
use crate::region::Region;
use crate::region::tnt::tests::{add, add_player, at, idle, nothing, primed, record, world};

/// Where the tests' TNT stands: the middle of a block on the floor.
const TNT_AT: [f64; 3] = [4.5, -60.0, 4.5];

/// The centre of the explosion of TNT standing at `feet`: `getY(0.0625)`.
fn centre_of(feet: [f64; 3]) -> [f64; 3] {
    [feet[0], feet[1] + f64::from(0.98_f32) * 0.0625, feet[2]]
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// `getEntityDamageAmount` of an explosion of `radius` at `distance` from the feet, of
/// which `seen` is seen.
fn blast_damage(radius: f32, distance: f64, seen: f64) -> f32 {
    let reach = f64::from(radius * 2.0);
    let impact = (1.0 - distance / reach) * seen;
    ((impact * impact + impact) / 2.0 * 7.0 * reach + 1.0) as f32
}

/// Easy's scaling of a hurt that scales.
fn easy(amount: f32) -> f32 {
    (amount / 2.0 + 1.0).min(amount)
}

/// The player `number` at `feet` in survival, holding and wearing nothing.
fn survival(region: &mut Region, number: u128, feet: [f64; 3]) -> (PlayerId, EntityId) {
    let (id, entity) = add_player(region, number, feet, false);
    region.players.get_mut(&id).expect("added").mode = PlayerMode::new(GameMode::Survival);
    (id, entity)
}

fn health(region: &Region, id: PlayerId) -> f32 {
    region.players.get(&id).expect("still here").health
}

/// TNT of fuse 1 at `feet`, primed by `owner`, gone off in one tick.
fn goes_off(region: &mut Region, feet: [f64; 3], owner: Option<uuid::Uuid>) -> TickOutput {
    primed(region, feet, 1, owner);
    idle(region)
}

/// The push the explosion of the tick gave the player whose entity is `entity`.
fn push_of(output: &TickOutput, entity: EntityId) -> Option<[f64; 3]> {
    output.events.iter().find_map(|event| match event {
        RegionEvent::Explosion { knockback, .. } => knockback
            .iter()
            .find(|(pushed, _)| *pushed == entity)
            .map(|(_, push)| *push),
        _ => None,
    })
}

/// The motions the player `id` was told in `output` (`markHurt`).
fn knocked(output: &TickOutput, id: PlayerId) -> Vec<[f64; 3]> {
    output
        .player_events
        .iter()
        .filter(|(to, _)| *to == id)
        .filter_map(|(_, event)| match event {
            PlayerEvent::Knocked { motion } => Some(*motion),
            _ => None,
        })
        .collect()
}

/// The damage types of the hurts the player `id` took, from their combat tracker.
fn hurt_by(region: &Region, id: PlayerId) -> Vec<String> {
    region.players[&id]
        .vitals
        .combat
        .iter()
        .map(|entry| entry.damage_type.clone())
        .collect()
}

/// TNT three blocks east of a survival player hurts them by the formula with what of
/// them it sees (told by their push, `(1 − d / 2r) · seen`), as `player_explosion` of its
/// owner where the owner is a player, and their client is told their motion with the
/// push in it; a survival player behind a wall of obsidian is seen not at all and takes
/// the 1 every explosion in reach deals.
#[test]
fn tnt_hurts_a_survival_player_by_the_game_s_formula_and_only_one_behind_a_wall() {
    let wall = |pos: BlockPos| {
        (pos.x == 2 && (-60..=-57).contains(&pos.y) && (2..=6).contains(&pos.z))
            .then_some(blocks::OBSIDIAN)
    };
    let mut region = world(41, wall);
    let (open, open_entity) = survival(&mut region, 1, [7.5, -60.0, 4.5]);
    let (hidden, hidden_entity) = survival(&mut region, 2, [1.5, -60.0, 4.5]);
    let (owner, _) = add_player(&mut region, 3, [-20.5, -60.0, 4.5], false);
    let output = goes_off(&mut region, TNT_AT, Some(owner.0));
    let centre = centre_of(TNT_AT);

    let far = distance([7.5, -60.0, 4.5], centre);
    let near = 1.0 - far / 8.0;
    let push = push_of(&output, open_entity).expect("pushed");
    let seen = length(push) / near;
    assert!(seen > 0.5 && seen <= 1.0, "seen {seen}");
    let expected = 20.0 - easy(blast_damage(4.0, far, seen));
    assert!(
        (health(&region, open) - expected).abs() < 1e-3,
        "{} against {expected}",
        health(&region, open)
    );
    assert_eq!(hurt_by(&region, open), ["minecraft:player_explosion"]);
    // Their motion was nothing before: what they are told is the push.
    assert_eq!(knocked(&output, open), [push]);

    assert_eq!(health(&region, hidden), 19.0);
    assert_eq!(push_of(&output, hidden_entity), Some([0.0; 3]));
}

/// Blast Protection IV on a chestplate (8 protection points and 0.6 knockback
/// resistance) against the same chestplate bare, each player alone in the same place:
/// the hurt after the armour is `17 / 25` of the bare one's, and the push four tenths.
#[test]
fn blast_protection_takes_its_points_off_the_hurt_and_its_part_off_the_push() {
    let chest = clustine_world::ARMOUR_START + 2;
    let mut outcomes = Vec::new();
    for protected in [false, true] {
        let mut region = world(42, nothing);
        let (id, entity) = survival(&mut region, 1, [7.5, -60.0, 4.5]);
        let mut chestplate = ItemStack::new(items::IRON_CHESTPLATE, 1);
        if protected {
            chestplate.components.set(Component::Enchantments(vec![(
                "minecraft:blast_protection".to_owned(),
                4,
            )]));
        }
        region
            .players
            .get_mut(&id)
            .expect("added")
            .inventory
            .set(chest, Some(chestplate));
        // A tick for the chestplate to be looked at.
        idle(&mut region);
        let output = goes_off(&mut region, TNT_AT, None);
        let push = push_of(&output, entity).expect("pushed");
        outcomes.push((20.0 - health(&region, id), push));
    }
    let [(bare_lost, bare_push), (lost, push)] = outcomes[..] else {
        panic!("{outcomes:?}");
    };
    assert!(bare_lost > 1.0, "{bare_lost}");
    assert!(
        (lost - bare_lost * (1.0 - 8.0 / 25.0)).abs() < 1e-3,
        "{lost} against {bare_lost}"
    );
    let kept = 1.0 - f64::from(0.15_f32 + 0.15_f32 * 3.0);
    for axis in 0..3 {
        assert!(
            (push[axis] - bare_push[axis] * kept).abs() < 1e-6,
            "{push:?} against {bare_push:?}"
        );
    }
}

/// A creative player is invulnerable to an explosion and pushed by it all the same, in
/// the packet and in the motion the server keeps for them, which nobody tells them.
#[test]
fn a_creative_player_is_pushed_by_tnt_and_not_hurt() {
    let mut region = world(43, nothing);
    let (id, entity) = add_player(&mut region, 1, [7.5, -60.0, 4.5], false);
    let output = goes_off(&mut region, TNT_AT, None);
    assert_eq!(health(&region, id), 20.0);
    let push = push_of(&output, entity).expect("pushed");
    assert!(push[0] > 0.0, "{push:?}");
    assert_eq!(region.players[&id].vitals.motion, push);
    assert!(knocked(&output, id).is_empty());
    assert!(hurt_by(&region, id).is_empty());
}

/// A bed used in the Nether explodes (`BedBlock`'s `destroyOnUse`) with power 5, its
/// hurt `bad_respawn_point` (`DamageSources.badRespawnPointExplosion`): a survival
/// player seven blocks off is hurt by the formula of its radius.
#[test]
fn a_bed_used_in_the_nether_hurts_a_survival_player_as_a_bad_respawn_point() {
    let head = at(4, -60, 4);
    let foot = at(4, -60, 5);
    let bed = move |pos: BlockPos| {
        if pos == head {
            BlockState::parse("minecraft:red_bed[facing=north,part=head,occupied=false]")
        } else if pos == foot {
            BlockState::parse("minecraft:red_bed[facing=north,part=foot,occupied=false]")
        } else {
            None
        }
    };
    let mut region = world(44, bed);
    region.config.dimension = clustine_world::Dimension::Nether;
    let (user, _) = add_player(&mut region, 1, [4.5, -60.0, 8.5], false);
    let (id, entity) = survival(&mut region, 2, [11.5, -60.0, 4.5]);
    let mut output = TickOutput::default();
    region.use_bed(head, user, &mut output);
    assert_eq!(region.block(head), Some(blocks::AIR));
    let centre = [4.5, -59.5, 4.5];
    let far = distance([11.5, -60.0, 4.5], centre);
    let push = push_of(&output, entity).expect("pushed");
    let seen = length(push) / (1.0 - far / 10.0);
    assert!(seen > 0.5 && seen <= 1.0, "seen {seen}");
    let expected = 20.0 - easy(blast_damage(5.0, far, seen));
    assert!(
        (health(&region, id) - expected).abs() < 1e-3,
        "{} against {expected}",
        health(&region, id)
    );
    assert_eq!(hurt_by(&region, id), ["minecraft:bad_respawn_point"]);
    // The creative player who used it is not hurt.
    assert_eq!(health(&region, user), 20.0);
}

/// An anvil falling from five and a half blocks up on a survival player hurts them
/// `min(⌊2 · ⌈fallen − 1⌉⌋, 40)` of `falling_anvil`, which no difficulty scales (its
/// cause, the falling block, is no living thing); a carved pumpkin on the head (no
/// armour) takes a quarter of it off; a creative player under one is not hurt.
#[test]
fn an_anvil_falling_on_a_survival_player_hurts_them_and_a_helmet_takes_a_quarter_off() {
    let head = clustine_world::ARMOUR_START + 3;
    let mut lost = Vec::new();
    for (mode, helmet) in [
        (GameMode::Survival, None),
        (
            GameMode::Survival,
            Some(ItemStack::new(items::CARVED_PUMPKIN, 1)),
        ),
        (GameMode::Creative, None),
    ] {
        let mut region = world(45, nothing);
        let (id, _) = survival(&mut region, 1, [8.5, -60.0, 8.5]);
        let player = region.players.get_mut(&id).expect("added");
        player.mode = PlayerMode::new(mode);
        player.inventory.set(head, helmet);
        let data = EntityData::FallingBlock {
            state: blocks::ANVIL.0,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        };
        let anvil = record(
            &mut region,
            entity_types::FALLING_BLOCK,
            [8.5, -54.5, 8.5],
            data,
        );
        let key = add(&mut region, anvil);
        for _ in 0..40 {
            idle(&mut region);
            if region.find_entity(key).is_none() {
                break;
            }
        }
        assert!(region.find_entity(key).is_none(), "landed");
        lost.push((20.0 - health(&region, id), hurt_by(&region, id)));
    }
    let (bare, ref types) = lost[0];
    // Fallen five and a half blocks: ⌈4.5⌉ = 5, ten.
    assert_eq!(bare, 10.0);
    assert_eq!(types, &["minecraft:falling_anvil"]);
    assert_eq!(lost[1].0, 7.5);
    assert_eq!(lost[2].0, 0.0);
    assert!(lost[2].1.is_empty());
}

/// A dragon of full health at `at` facing north in its holding pattern, its target far
/// ahead.
fn dragon(region: &mut Region, key: EntityId, at: [f64; 3]) {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 200.0;
    mob.body.vitals.air = 300;
    change(&mut mob, |own| {
        own.phase = phases::HOLDING_PATTERN;
        own.phases.holding.target = Some([at[0], at[1], at[2] - 50.0]);
    });
    let entity = clustine_world::Entity {
        portal: Default::default(),
        key,
        kind: DRAGON,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 13),
        position: at,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    };
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&clustine_world::EntityChange::Whole(entity));
    region.entity_chunks.insert(key, chunk);
}

/// The dragon's east wing over a survival player pushes them east and hurts them by 5 of
/// `mob_attack` (easy: 3.5), whose knockback throws them further east; its west wing
/// over a creative player does nothing to them (`NO_CREATIVE_OR_SPECTATOR`).
#[test]
fn the_dragon_s_wing_hits_a_survival_player_and_not_a_creative_one() {
    let mut region = world(46, nothing);
    dragon(&mut region, EntityId(700), [10.5, -60.0, 10.5]);
    let (hit, _) = survival(&mut region, 1, [15.5, -60.0, 10.5]);
    let (spared, _) = add_player(&mut region, 2, [5.5, -60.0, 10.5], false);
    let output = idle(&mut region);
    assert_eq!(health(&region, hit), 16.5);
    assert_eq!(hurt_by(&region, hit), ["minecraft:mob_attack"]);
    let motions = knocked(&output, hit);
    assert_eq!(motions.len(), 1, "{motions:?}");
    assert!(motions[0][0] > 0.4, "{motions:?}");
    assert_eq!(health(&region, spared), 20.0);
    assert_eq!(region.players[&spared].vitals.motion, [0.0; 3]);
    assert!(knocked(&output, spared).is_empty());
}
