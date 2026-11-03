//! Projectiles hitting survival players (V4a; `docs/groundwork/v-plan.md`, section 2.3),
//! from the game's classes: `AbstractArrow.onHitEntity` and `ThrownTrident.onHitEntity`,
//! `Snowball.onHitEntity` with `Player.hurtServer` (108–115: a hurt of exactly nothing
//! is refused), `SmallFireball.onHitEntity`, `ShulkerBullet.onHitEntity`,
//! `LlamaSpit.onHitEntity` and `EvokerFangs.dealDamageTo`. The world's difficulty is
//! easy (`Difficulty::WORLD`), so a hurt caused by a living thing that is no player is
//! `min(d / 2 + 1, d)` (`Player.hurtServer` 51–107); one with no living cause is not
//! scaled. No player wears armour, so what is left is taken off their 20 health.
//!
//! The worlds are those of the tests of TNT: a stone floor whose top is y = -60, the
//! player standing at x = 3.5 and each projectile coming at them from the west along x.

use clustine_data::{entity_types, items};
use clustine_world::projectiles::{Fireball, ProjectileOwner, Spit};
use clustine_world::{
    Arrow, ArrowKind, Double, Effected, Entity, EntityData, EntityId, EntityUse, GameMode,
    ItemStack, Pickup, PlayerMode, ShulkerBullet, Vec3,
};

use super::n9_world::{east, entering_place, face_of, filed, west};
use super::thrown::{cow, lay};
use super::*;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::region::projectiles::{Aim, Making};
use crate::region::tnt::tests::{add_player, idle, nothing, world};

const SEED: u64 = 0x5EED_A770;

/// Where the player stands, and where the projectiles come from.
const FEET: [f64; 3] = [3.5, -60.0, 0.5];
const FROM: [f64; 3] = [1.5, -59.0, 0.5];

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type's id")
}

/// A region alone with player 1 standing at [`FEET`] in `mode`.
fn standing(mode: GameMode) -> (Region, PlayerId, EntityId) {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, FEET, false);
    region.players.get_mut(&id).expect("added").mode = PlayerMode::new(mode);
    (region, id, entity)
}

fn the_player(region: &Region, id: PlayerId) -> &Player {
    region.players.get(&id).expect("the player is here")
}

/// A mob of the type `of` with no AI standing at `feet`, laid in the region.
fn still_mob(region: &mut Region, of: i32, feet: [f64; 3]) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut mob = cow(key, feet);
    mob.kind = kind(of);
    lay(region, &mob);
    key
}

/// An arrow as a bow's shot leaves it, of `owner`, which may not be picked up.
fn arrow(owner: Option<EntityId>) -> Arrow {
    Arrow {
        pickup: Pickup::Disallowed,
        pickup_stack: ItemStack::new(items::ARROW, 1),
        weapon: None,
        owner,
        left_owner: false,
        has_been_shot: false,
        base_damage: Double(2.0),
        crit: false,
        pierce_level: 0,
        pierced: Vec::new(),
        hit_sound: "minecraft:entity.arrow.hit".to_owned(),
        in_ground: None,
        shake_until: 0,
        of_kind: ArrowKind::Arrow,
    }
}

/// A projectile of the type `of` made at `at` with `data`, shot east at `power` with no
/// uncertainty, so that its motion is `(power, 0, 0)`.
fn shoot_east(
    region: &mut Region,
    of: i32,
    at: [f64; 3],
    data: EntityData,
    power: f32,
) -> EntityId {
    let making = Making {
        kind: kind(of),
        at,
        data,
        aim: Aim::Direction([1.0, 0.0, 0.0]),
        power,
        uncertainty: 0.0,
    };
    region
        .make_projectile(making, |_, _| {}, &mut TickOutput::default())
        .expect("the projectile is made")
}

/// A record of the type `of` at `at` moving by `motion`, as after a few ticks of flight.
fn flying(key: EntityId, of: i32, at: [f64; 3], motion: [f64; 3], data: EntityData) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(of),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 5),
        position: at,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 3,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    }
}

/// Whether `output` tells player `id` they were knocked.
fn knocked(output: &TickOutput, id: PlayerId) -> bool {
    output
        .player_events
        .iter()
        .any(|(to, event)| *to == id && matches!(event, PlayerEvent::Knocked { .. }))
}

/// `AbstractArrow.onHitEntity`: a skeleton's arrow at 2 a tick deals `ceil(2 · 2)` = 4,
/// which easy makes 3 (a living cause that is no player); the hurt took, so the arrow is
/// counted in the player and is gone (`discard`, its pierce level 0), and the player is
/// knocked along its motion.
#[test]
fn a_skeleton_s_arrow_hurts_a_survival_player_sticks_in_them_and_is_gone() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let skeleton = still_mob(&mut region, entity_types::SKELETON, [-4.5, -60.0, 0.5]);
    let key = shoot_east(
        &mut region,
        entity_types::ARROW,
        FROM,
        EntityData::Arrow(Box::new(arrow(Some(skeleton)))),
        2.0,
    );
    let output = idle(&mut region);
    assert!(
        region.find_entity(key).is_none(),
        "the arrow is gone: {:?}",
        output.events
    );
    let player = the_player(&region, id);
    assert_eq!(player.health, 17.0);
    assert_eq!(player.vitals.arrows, 1);
    assert!(knocked(&output, id), "{:?}", output.player_events);
    assert!(player.vitals.motion[0] > 0.0, "{:?}", player.vitals.motion);
}

/// The same arrow at a creative player: refused (`Player.hurtServer` 11–32), so the
/// arrow turns back at a tenth of its speed (`deflect(REVERSE, …, 0.2)`), and the player
/// is neither hurt nor counts it.
#[test]
fn the_same_arrow_at_a_creative_player_turns_back() {
    let (mut region, id, _) = standing(GameMode::Creative);
    let skeleton = still_mob(&mut region, entity_types::SKELETON, [-4.5, -60.0, 0.5]);
    let key = shoot_east(
        &mut region,
        entity_types::ARROW,
        FROM,
        EntityData::Arrow(Box::new(arrow(Some(skeleton)))),
        2.0,
    );
    idle(&mut region);
    let (_, after) = region.find_entity(key).expect("a refused arrow lives on");
    assert!(after.motion[0] < 0.0, "{:?}", after.motion);
    let player = the_player(&region, id);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.vitals.arrows, 0);
}

/// A burning arrow sets a survival player alight for five seconds where its hurt took
/// (`igniteForSeconds(5)` before the hurt, kept).
#[test]
fn a_burning_arrow_sets_a_survival_player_alight() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let key = shoot_east(
        &mut region,
        entity_types::ARROW,
        FROM,
        EntityData::Arrow(Box::new(arrow(None))),
        2.0,
    );
    let (_, made) = region.find_entity(key).expect("made");
    let lit = Entity {
        fire_ticks: 100,
        ..made.clone()
    };
    lay(&mut region, &lit);
    idle(&mut region);
    let player = the_player(&region, id);
    // No living cause: 4, not scaled.
    assert_eq!(player.health, 16.0);
    assert!(player.vitals.fire > 90, "{}", player.vitals.fire);
}

/// `Snowball.onHitEntity` hurts a player by nothing, which `Player.hurtServer` refuses
/// (108–115, `fcmpl ifne`) before `LivingEntity.hurtServer`'s knockback: the player is
/// neither hurt nor knocked, and the snowball is gone.
#[test]
fn a_snowball_neither_hurts_nor_knocks_a_survival_player() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let data = EntityData::Thrown {
        stack: ItemStack::new(items::SNOWBALL, 1),
        owner: None,
        left_owner: false,
        has_been_shot: false,
    };
    let key = shoot_east(&mut region, entity_types::SNOWBALL, FROM, data, 1.5);
    let mut outputs = Vec::new();
    for _ in 0..3 {
        outputs.push(idle(&mut region));
    }
    assert!(region.find_entity(key).is_none(), "the snowball is gone");
    assert!(outputs.iter().all(|output| !knocked(output, id)));
    let player = the_player(&region, id);
    assert_eq!(player.health, 20.0);
    assert_eq!(player.vitals.motion, [0.0; 3]);
}

/// `SmallFireball.onHitEntity`: the player set alight for five seconds, then hurt by
/// `fireball` 5, which easy makes 3.5 for a blaze's; both kept as the hurt took.
#[test]
fn a_blaze_s_small_fireball_sets_a_survival_player_alight_and_hurts_them() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let key = region
        .take_entity_key(ChunkPos::containing(2.5, 0.5))
        .expect("a key");
    let data = EntityData::Fireball(Box::new(Fireball {
        item: ItemStack::new(items::FIRE_CHARGE, 1),
        owner: Some(ProjectileOwner {
            key: EntityId(999_999),
            kind: kind(entity_types::BLAZE),
            player: None,
        }),
        left_owner: true,
        has_been_shot: true,
        acceleration_power: Double(0.1),
        explosion_power: 1,
    }));
    let fireball = flying(
        key,
        entity_types::SMALL_FIREBALL,
        [2.2, -59.0, 0.5],
        [0.5, 0.0, 0.0],
        data,
    );
    lay(&mut region, &fireball);
    let mut tick = 0;
    while region.find_entity(key).is_some() {
        tick += 1;
        assert!(tick < 5, "the fireball hits within a few ticks");
        idle(&mut region);
    }
    let player = the_player(&region, id);
    assert_eq!(player.health, 16.5);
    assert!(player.vitals.fire > 90, "{}", player.vitals.fire);
}

/// `ShulkerBullet.onHitEntity`: `mob_projectile` 4 (no owner: not scaled), and where it
/// took, levitation for 200 ticks.
#[test]
fn a_shulker_bullet_hurts_a_survival_player_and_makes_them_levitate() {
    let (mut region, id, entity) = standing(GameMode::Survival);
    let key = region
        .take_entity_key(ChunkPos::containing(2.5, 0.5))
        .expect("a key");
    let motion = [0.4, 0.0, 0.0];
    let data = EntityData::ShulkerBullet(Box::new(ShulkerBullet {
        owner: None,
        target: Some(entity),
        steps: 40,
        direction: None,
        target_delta: motion.map(Double),
    }));
    let bullet = flying(
        key,
        entity_types::SHULKER_BULLET,
        [2.6, -59.0, 0.5],
        motion,
        data,
    );
    lay(&mut region, &bullet);
    let mut tick = 0;
    while region.find_entity(key).is_some() {
        tick += 1;
        assert!(tick < 10, "the bullet hits within a few ticks");
        idle(&mut region);
    }
    let player = the_player(&region, id);
    assert_eq!(player.health, 16.0);
    assert!(player.effects.has(MobEffect::LEVITATION));
}

/// `LlamaSpit.onHitEntity`: `spit` 1 from its llama, which easy leaves at 1
/// (`min(1.5, 1)`).
#[test]
fn a_llama_s_spit_hurts_a_survival_player_by_one() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let llama = still_mob(&mut region, entity_types::LLAMA, [-4.5, -60.0, 0.5]);
    let data = EntityData::Spit(Spit {
        owner: Some(llama),
        left_owner: false,
        has_been_shot: false,
    });
    shoot_east(&mut region, entity_types::LLAMA_SPIT, FROM, data, 1.5);
    let hurt = (0..5).any(|_| {
        idle(&mut region);
        the_player(&region, id).health < 20.0
    });
    assert!(hurt, "the spit reaches the player");
    assert_eq!(the_player(&region, id).health, 19.0);
}

/// `EvokerFangs.dealDamageTo`: fangs with no owner bite by `magic` 6 at the eighth tick
/// past their warmup; with an evoker's, by `indirect_magic` 6, which easy makes 4.
#[test]
fn evoker_fangs_bite_a_survival_player_by_six_and_an_evoker_s_by_four_on_easy() {
    for (evoker, left) in [(false, 14.0), (true, 16.0)] {
        let (mut region, id, _) = standing(GameMode::Survival);
        let owner =
            evoker.then(|| still_mob(&mut region, entity_types::EVOKER, [-4.5, -60.0, 0.5]));
        let mut output = TickOutput::default();
        region
            .make_fangs(FEET, 0.0, 0, owner, &mut output)
            .expect("the fangs are made");
        for _ in 0..12 {
            idle(&mut region);
        }
        assert_eq!(
            the_player(&region, id).health,
            left,
            "an evoker's: {evoker}"
        );
    }
}

/// `ThrownTrident.onHitEntity`: 8 (thrown by nobody: not scaled), then the trident's
/// hit sound and its bounce back (`deflect(REVERSE, …, (0.02, 0.2, 0.02))`).
#[test]
fn a_trident_hurts_a_survival_player_by_eight_and_bounces() {
    let (mut region, id, _) = standing(GameMode::Survival);
    let trident = Arrow {
        pickup_stack: ItemStack::new(items::TRIDENT, 1),
        hit_sound: "minecraft:item.trident.hit_ground".to_owned(),
        of_kind: ArrowKind::Trident {
            dealt_damage: false,
            returning: 0,
        },
        ..arrow(None)
    };
    let key = shoot_east(
        &mut region,
        entity_types::TRIDENT,
        FROM,
        EntityData::Arrow(Box::new(trident)),
        2.0,
    );
    idle(&mut region);
    let (_, after) = region.find_entity(key).expect("the trident bounces");
    assert!(after.motion[0] < 0.0, "{:?}", after.motion);
    assert_eq!(the_player(&region, id).health, 12.0);
}

/// Across a pinned line: the western region keeps the face of player 2, who stands in
/// the eastern one; an arrow it shoots at them is gone and sends its hurt, addressed to
/// player 2, to their region, which takes it: 4 (no living cause) and the arrow counted.
#[test]
fn an_arrow_at_a_player_across_a_line_sends_the_hurt_to_their_region() {
    let clock = super::n9_world::clock_at(0);
    let mut west = west(10, clock);
    let mut east = east(10, clock);
    let place = crate::api::Place {
        mode: PlayerMode::new(GameMode::Survival),
        ..entering_place(Vec3::new(16.8, -60.0, 0.5), Vec::new(), 0)
    };
    super::n9_world::enter_with(&mut east, 2, place);
    let (entity, _) = east.player(player(2)).expect("player 2 is in the east");
    // A joining client is hurt by nothing until it has loaded, or its timer ran out
    // (`ServerPlayer.isInvulnerableTo`, `hasClientLoaded`).
    let mut waited = 0;
    while !east.client_loaded(player(2)) {
        waited += 1;
        assert!(waited <= 100, "player 2's client never counts as loaded");
        east.tick(&TickInputs::default());
    }
    west.tick(&TickInputs {
        faces: vec![face_of(2, entity, [16.8, -60.0, 0.5], 1)],
        ..TickInputs::default()
    });
    let key = shoot_east(
        &mut west,
        entity_types::ARROW,
        [15.0, -59.0, 0.5],
        EntityData::Arrow(Box::new(arrow(None))),
        2.0,
    );
    let output = idle(&mut west);
    assert!(west.find_entity(key).is_none(), "the arrow is gone");
    let sent: Vec<_> = output
        .effects
        .iter()
        .filter(|effect| {
            matches!(
                &effect.what,
                Effected::OnPlayer { player: to, entity: stay, by: EntityUse::Hurt(_) }
                    if *to == player(2) && *stay == entity
            )
        })
        .cloned()
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", output.effects);
    east.tick(&TickInputs {
        effects: filed(sent, 1),
        ..TickInputs::default()
    });
    let taken = east.players.get(&player(2)).expect("in the east");
    assert_eq!(taken.health, 16.0);
    assert_eq!(taken.vitals.arrows, 1);
}
