//! Fireballs from the game's rules (`AbstractHurtingProjectile`, `LargeFireball`,
//! `SmallFireball`, `Projectile`, `ProjectileDeflection`, `Player.deflectProjectile`,
//! `CampfireBlock.onProjectileHit`; `docs/groundwork/p-plan.md`, sections 18.5 and
//! 18.6), not from the code, on a region alone and on a world divided by a line: the
//! flight worked out by hand tick by tick, a ghast's fireball crossing a line and flying
//! on, its end where its owner dies or where it reaches land nobody holds, a player's
//! deflection here and from across a line that kills the ghast by that player with its
//! loot and experience, the large one's direct hit across a line before its blast, the
//! blast's fire, the small one's fire across a line and on a mob, and TNT and a campfire
//! lit.

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Double, EdgeId, Effected, Entity, EntityChange, EntityData, EntityId,
    EntityUse, Float, ItemStack,
    projectiles::{Fireball, ProjectileOwner},
};

use super::{INITIAL_ACCELERATION_POWER, inertia, rotate_towards_movement};
use crate::api::{PlayerInput, RegionEvent, TickInputs, TickOutput, Tracked};
use crate::mobs::Mob;
use crate::mobs::types::ghast::view_vector;
use crate::physics::{self, Vec3};
use crate::region::Region;
use crate::region::tnt::tests::{
    add_player, idle, nothing, primed_tnt, records, west_of_a_line, world,
};

const SEED: u64 = 0xF12E_BA11;

/// An owner no region has.
const NOBODY: EntityId = EntityId(999_999);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// A fireball of the type `id` at `at` moving by `motion`, owned by `owner` (of the type
/// `owner_kind`, a player's where `player`), as it is after its first tick.
fn fireball(
    key: EntityId,
    id: i32,
    at: Vec3,
    motion: Vec3,
    owner: Option<(EntityId, u16, Option<uuid::Uuid>)>,
) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(id),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 3),
        position: at,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 5,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 20,
        riding: Default::default(),
        data: EntityData::Fireball(Box::new(Fireball {
            item: ItemStack::new(items::FIRE_CHARGE, 1),
            owner: owner.map(|(key, kind, player)| ProjectileOwner { key, kind, player }),
            left_owner: true,
            has_been_shot: true,
            acceleration_power: Double(INITIAL_ACCELERATION_POWER),
            explosion_power: 1,
        })),
    }
}

/// A ghast of `health` at `feet` with no AI, so that it floats where it is.
fn ghast(key: EntityId, feet: Vec3, health: f32) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = health;
    mob.body.vitals.air = crate::living::MAX_AIR;
    mob.body.flags.no_ai = true;
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::GHAST),
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 1),
        position: feet,
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
    }
}

/// A cow of full health at `feet` with no AI.
fn cow(key: EntityId, feet: Vec3) -> Entity {
    let mut entity = ghast(key, feet, 10.0);
    entity.kind = kind(entity_types::COW);
    entity.on_ground = true;
    entity
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

fn fireballs(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::Fireball(_)))
        .collect()
}

fn the_fireball(region: &Region) -> Entity {
    let [fireball] = &fireballs(region)[..] else {
        panic!("one fireball: {:?}", fireballs(region));
    };
    fireball.clone()
}

fn data_of(entity: &Entity) -> Fireball {
    let EntityData::Fireball(fireball) = &entity.data else {
        panic!("a fireball: {entity:?}");
    };
    (**fireball).clone()
}

fn gone(output: &TickOutput, key: EntityId) -> bool {
    output
        .events
        .iter()
        .any(|event| matches!(event, RegionEvent::EntityGone { entity, .. } if *entity == key))
}

fn mob(region: &Region, key: EntityId) -> Mob {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    Mob::decode(&record.data).expect("a mob's record")
}

/// The block at `pos` as the region reads it.
fn block(region: &Region, pos: BlockPos) -> BlockState {
    region
        .readable_chunk(pos.chunk())
        .and_then(|chunk| {
            let (x, z) = pos.in_chunk();
            chunk.get(x, pos.y, z)
        })
        .expect("a readable block")
}

/// The sets the region's effects ask of other regions' blocks.
fn sets(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::SetIf { state, .. } => Some((effect.at, state)),
            _ => None,
        })
        .collect()
}

/// One tick of a fireball by hand: its motion grown by the power and kept, its move.
fn fly(position: Vec3, motion: Vec3) -> (Vec3, Vec3) {
    let grown = physics::add(motion, physics::scale(physics::normalize(motion), 0.1));
    let motion = physics::scale(grown, f64::from(0.95_f32));
    (physics::add(position, motion), motion)
}

#[test]
fn a_ghast_s_fireball_is_made_one_tenth_long_along_its_aim_and_flies_faster_each_tick() {
    let mut region = world(SEED, nothing);
    let owner = EntityId(70);
    lay(&mut region, &ghast(owner, [0.5, -50.0, 0.5], 10.0));
    region.shoot_fireball(
        owner,
        kind(entity_types::FIREBALL),
        [0.5, -50.0, 4.5],
        [0.0, 0.0, 3.0],
        1,
        &mut TickOutput::default(),
    );
    let made = the_fireball(&region);
    assert_eq!(made.motion, [0.0, 0.0, 0.1]);
    assert_eq!(made.position, [0.5, -50.0, 4.5]);
    let data = data_of(&made);
    assert_eq!(
        data.owner,
        Some(ProjectileOwner {
            key: owner,
            kind: kind(entity_types::GHAST),
            player: None
        })
    );
    assert_eq!(data.item, ItemStack::new(items::FIRE_CHARGE, 1));
    assert_eq!(data.explosion_power, 1);
    assert!(!data.has_been_shot);
    let (mut position, mut motion) = (made.position, made.motion);
    let (mut yaw, mut pitch) = (made.yaw, made.pitch);
    for _ in 0..4 {
        idle(&mut region);
        (position, motion) = fly(position, motion);
        [yaw, pitch] = rotate_towards_movement(yaw, pitch, motion);
        let now = the_fireball(&region);
        assert_eq!(now.position.map(f64::to_bits), position.map(f64::to_bits));
        assert_eq!(now.motion.map(f64::to_bits), motion.map(f64::to_bits));
        assert_eq!((now.yaw, now.pitch), (yaw, pitch));
        // It burns, a second renewed every tick.
        assert_eq!(now.fire_ticks, 20);
        assert!(data_of(&now).has_been_shot);
    }
    // 0.19, then 0.2755: each tick's growth by the power and loss to the air.
    assert!((motion[2] - 0.43).abs() < 0.1, "{motion:?}");
}

#[test]
fn a_fireball_in_water_keeps_less_of_its_motion() {
    let grown = 0.2 * f64::from(0.8_f32);
    assert_eq!(
        inertia([0.1, 0.0, 0.0], 0.1, true)[0].to_bits(),
        grown.to_bits()
    );
    // A still one stays still: its direction is none.
    assert_eq!(inertia([0.0; 3], 0.1, false), [0.0; 3]);
}

#[test]
fn a_ghast_s_fireball_crosses_a_line_and_flies_on_in_the_region_that_takes_it() {
    let mut west = west_of_a_line(SEED, nothing);
    let ghast_key = EntityId(71);
    lay(&mut west, &ghast(ghast_key, [8.0, -55.0, 0.5], 10.0));
    let owner = Some((ghast_key, kind(entity_types::GHAST), None));
    let key = EntityId(72);
    lay(
        &mut west,
        &fireball(
            key,
            entity_types::FIREBALL,
            [15.8, -55.0, 0.5],
            [0.3, 0.0, 0.0],
            owner,
        ),
    );
    let output = idle(&mut west);
    assert!(fireballs(&west).is_empty());
    let (crossed, motion) = fly([15.8, -55.0, 0.5], [0.3, 0.0, 0.0]);
    let [(chunk, sent)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!(*chunk, ChunkPos::new(1, 0));
    assert_eq!(sent.key, key);
    assert_eq!(sent.position.map(f64::to_bits), crossed.map(f64::to_bits));
    assert_eq!(data_of(sent).owner.map(|owner| owner.key), Some(ghast_key));
    // The region east of the line, which has no ghast, takes it, and it flies on.
    let mut east = world(SEED, nothing);
    let taken = TickInputs {
        arrivals: vec![clustine_world::Arrival {
            id: clustine_world::EffectId {
                sequence: 3,
                index: 0,
            },
            chunk: *chunk,
            entity: sent.clone(),
            due: clustine_world::Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    east.tick(&taken);
    let (on, _) = fly(crossed, motion);
    let now = the_fireball(&east);
    assert_eq!(now.key, key);
    assert_eq!(now.position.map(f64::to_bits), on.map(f64::to_bits));
    assert_eq!(
        data_of(&now).owner,
        Some(ProjectileOwner {
            key: ghast_key,
            kind: kind(entity_types::GHAST),
            player: None
        })
    );
}

#[test]
fn a_fireball_whose_owner_dies_here_is_gone_and_one_whose_owner_is_elsewhere_flies_on() {
    let mut region = world(SEED, nothing);
    let dying = EntityId(73);
    let mut owner = ghast(dying, [0.5, -50.0, 0.5], 0.0);
    let mut mob = Mob::decode(&owner.data).expect("a mob");
    mob.body.vitals.died_from = Some(0);
    owner.data = mob.encode();
    lay(&mut region, &owner);
    let of_dying = EntityId(74);
    let of_nobody = EntityId(75);
    let ghast_kind = kind(entity_types::GHAST);
    lay(
        &mut region,
        &fireball(
            of_dying,
            entity_types::FIREBALL,
            [0.5, -50.0, 8.5],
            [0.0, 0.0, 0.1],
            Some((dying, ghast_kind, None)),
        ),
    );
    lay(
        &mut region,
        &fireball(
            of_nobody,
            entity_types::FIREBALL,
            [4.5, -50.0, 8.5],
            [0.0, 0.0, 0.1],
            Some((NOBODY, ghast_kind, None)),
        ),
    );
    let output = idle(&mut region);
    assert!(gone(&output, of_dying));
    assert!(!gone(&output, of_nobody));
    assert_eq!(the_fireball(&region).key, of_nobody);
}

#[test]
fn a_fireball_that_would_fly_into_land_nobody_holds_is_gone_and_is_handed_to_nobody() {
    // The region holds the chunks of x from -2 to 2, and knows of nothing beyond: a move
    // of more than 30 from x = 24.5 ends in the chunk of x = 3.
    let mut region = world(SEED, nothing);
    let key = EntityId(76);
    lay(
        &mut region,
        &fireball(
            key,
            entity_types::FIREBALL,
            [24.5, -50.0, 0.5],
            [30.0, 0.0, 0.0],
            None,
        ),
    );
    let output = idle(&mut region);
    assert!(gone(&output, key));
    assert!(output.arrivals.is_empty(), "{:?}", output.arrivals);
}

#[test]
fn a_player_s_hit_sends_a_large_fireball_along_their_look_owned_by_them_at_once() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    {
        let pose = &mut region.players.get_mut(&id).expect("player 1").pose;
        pose.yaw = 30.0;
        pose.pitch = -20.0;
    }
    let key = EntityId(77);
    let ghast_kind = kind(entity_types::GHAST);
    let mut ball = fireball(
        key,
        entity_types::FIREBALL,
        [0.5, -58.0, 3.0],
        [0.0, 0.0, -0.5],
        Some((NOBODY, ghast_kind, None)),
    );
    if let EntityData::Fireball(data) = &mut ball.data {
        data.acceleration_power = Double(0.05);
    }
    lay(&mut region, &ball);
    let attack = TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            player,
            1,
            PlayerInput::Attack {
                part: None,
                entity: key,
            },
        )],
        ..TickInputs::default()
    };
    let output = region.tick(&attack);
    // The hit's no-damage sound where the player stands.
    let heard: Vec<clustine_world::Vec3> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound, position, ..
            } if *sound == sounds::ENTITY_PLAYER_ATTACK_NODAMAGE => Some(*position),
            _ => None,
        })
        .collect();
    assert_eq!(heard, [clustine_world::Vec3::new(0.5, -60.0, 0.5)]);
    // Sent along the look at once, then flown a tick.
    let look = view_vector(30.0, -20.0);
    let synced = output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityTracked(report)
            if report.entity == key && report.flags & Tracked::NEEDS_SYNC != 0
                && report.motion.map(f64::to_bits) == look.map(f64::to_bits))
    });
    assert!(synced, "{:?}", output.events);
    let now = the_fireball(&region);
    let (_, motion) = fly(ball.position, look);
    assert_eq!(now.motion.map(f64::to_bits), motion.map(f64::to_bits));
    let data = data_of(&now);
    assert_eq!(
        data.owner,
        Some(ProjectileOwner {
            key: player,
            kind: kind(entity_types::PLAYER),
            player: Some(id.0)
        })
    );
    assert_eq!(data.acceleration_power, Double(INITIAL_ACCELERATION_POWER));
}

#[test]
fn a_small_fireball_is_not_sent_back_by_a_hit() {
    let mut region = world(SEED, nothing);
    let key = EntityId(78);
    let ball = fireball(
        key,
        entity_types::SMALL_FIREBALL,
        [0.5, -58.0, 3.0],
        [0.0, 0.0, -0.5],
        None,
    );
    lay(&mut region, &ball);
    let by = EntityUse::Attack {
        part: None,
        attacker: EntityId(5),
        creative: true,
        sprinting: false,
        damage: Float(1.0),
        from: [Double(0.5), Double(-60.0), Double(0.5)],
        look: [Float(0.0), Float(0.0)],
        player: Some(uuid::Uuid::from_u128(5)),
    };
    region.entity_used(key, by, &mut TickOutput::default());
    assert_eq!(the_fireball(&region), ball);
}

/// Section 18.6: a player of the region west of the line hits the ghast's fireball that
/// the region east of it holds; the east takes the hit, sends the fireball back along
/// the player's look, and it kills the ghast by that player, who is not there, with the
/// ghast's loot and its experience.
#[test]
fn a_fireball_a_player_sends_back_from_across_a_line_kills_the_ghast_by_that_player() {
    let ghast_key = EntityId(80);
    let key = EntityId(81);
    let ghast_kind = kind(entity_types::GHAST);
    let ball = fireball(
        key,
        entity_types::FIREBALL,
        [16.8, -58.0, 0.5],
        [-0.3, 0.0, 0.0],
        Some((ghast_key, ghast_kind, None)),
    );
    let mut west = west_of_a_line(SEED, nothing);
    let (id, player) = add_player(&mut west, 1, [14.5, -60.0, 0.5], false);
    {
        let pose = &mut west.players.get_mut(&id).expect("player 1").pose;
        pose.yaw = -90.0;
        pose.pitch = 0.0;
    }
    lay_in_copy(&mut west, &ball);
    let attack = TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            player,
            1,
            PlayerInput::Attack {
                part: None,
                entity: key,
            },
        )],
        ..TickInputs::default()
    };
    let output = west.tick(&attack);
    let hits: Vec<EntityUse> = output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity { key: hit, by } if *hit == key => Some(by.clone()),
            _ => None,
        })
        .collect();
    let [by] = &hits[..] else {
        panic!("one hit on the fireball: {:?}", output.effects);
    };
    // The east holds the fireball and the ghast, and takes the hit.
    let mut east = world(SEED, nothing);
    let target = ghast(ghast_key, [26.0, -60.0, 0.5], 10.0);
    lay(&mut east, &target);
    lay(&mut east, &ball);
    east.entity_used(key, by.clone(), &mut TickOutput::default());
    let deflected = the_fireball(&east);
    assert_eq!(
        deflected.motion.map(f64::to_bits),
        view_vector(-90.0, 0.0).map(f64::to_bits)
    );
    assert_eq!(
        data_of(&deflected).owner,
        Some(ProjectileOwner {
            key: player,
            kind: kind(entity_types::PLAYER),
            player: Some(id.0)
        })
    );
    // It flies east into the ghast, which dies of it by the player.
    let mut died = false;
    for _ in 0..12 {
        idle(&mut east);
        if let Some((_, record)) = east.find_entity(ghast_key)
            && Mob::decode(&record.data).is_ok_and(|mob| mob.body.vitals.died_from.is_some())
        {
            died = true;
            break;
        }
    }
    assert!(died, "the ghast died: {:?}", fireballs(&east));
    let body = mob(&east, ghast_key);
    assert_eq!(body.body.vitals.health, 0.0);
    assert_eq!(
        body.body.combat.last_hurt_by_player.map(|(uuid, _)| uuid),
        Some(id.0.as_u128())
    );
    for _ in 0..25 {
        idle(&mut east);
    }
    assert!(east.find_entity(ghast_key).is_none());
    let dropped: Vec<ItemStack> = records(&east)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect();
    // A ghast killed by a player's fireball: its tear by the player's kill, and the disc
    // of a player's fireball (`minecraft:entities/ghast`).
    assert!(
        dropped
            .iter()
            .any(|stack| stack.item == items::MUSIC_DISC_TEARS),
        "{dropped:?}"
    );
    let experience: i32 = records(&east)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::ExperienceOrb { value, count, .. } => Some(value * count),
            _ => None,
        })
        .sum();
    assert_eq!(experience, 5);
}

/// Section 18.5's eleventh: the large fireball's hurt of a cow across a line goes to the
/// cow's holder as a hurt of its own, `fireball` 6 of the ghast, before its blast.
#[test]
fn a_large_fireball_s_direct_hit_reaches_a_cow_across_a_line_before_its_blast() {
    let mut west = west_of_a_line(SEED, nothing);
    let cow_key = EntityId(82);
    lay_in_copy(&mut west, &cow(cow_key, [17.5, -60.0, 0.5]));
    let ghast_key = EntityId(83);
    lay(&mut west, &ghast(ghast_key, [4.0, -59.0, 0.5], 10.0));
    let key = EntityId(84);
    lay(
        &mut west,
        &fireball(
            key,
            entity_types::FIREBALL,
            [15.9, -59.5, 0.5],
            [1.2, 0.0, 0.0],
            Some((ghast_key, kind(entity_types::GHAST), None)),
        ),
    );
    let output = idle(&mut west);
    assert!(gone(&output, key));
    let hurts: Vec<(usize, clustine_world::HurtBy)> = output
        .effects
        .iter()
        .enumerate()
        .filter_map(|(at, effect)| match &effect.what {
            Effected::OnEntity {
                key: hit,
                by: EntityUse::Hurt(hurt),
            } if *hit == cow_key && hurt.damage_type == "minecraft:fireball" => {
                Some((at, (**hurt).clone()))
            }
            _ => None,
        })
        .collect();
    let [(first, hurt)] = &hurts[..] else {
        panic!("one hurt of the cow: {:?}", output.effects);
    };
    assert_eq!(hurt.amount, Float(6.0));
    assert_eq!(hurt.cause.as_ref().map(|cause| cause.key), Some(ghast_key));
    assert_eq!(
        hurt.direct.as_ref().map(|direct| direct.kind),
        Some(kind(entity_types::FIREBALL))
    );
    // Its blast reaches the cow after, as an explosion's hit.
    let blasted = output.effects.iter().enumerate().any(|(at, effect)| {
        at > *first
            && matches!(&effect.what,
                Effected::OnEntity { key: hit, by: EntityUse::Blasted { .. } } if *hit == cow_key)
    });
    assert!(blasted, "{:?}", output.effects);
}

/// The explosion's fire by the rule (`mob_griefing`, on): netherrack two deep on the
/// floor, a fireball falling on it breaks it and sets fire in the hole.
#[test]
fn a_large_fireball_explodes_where_it_hits_breaking_blocks_and_setting_fire() {
    let netherrack = BlockState::parse("minecraft:netherrack").expect("netherrack");
    let mound = move |pos: BlockPos| (pos.y == -60 || pos.y == -59).then_some(netherrack);
    let mut region = world(SEED, mound);
    let key = EntityId(85);
    lay(
        &mut region,
        &fireball(
            key,
            entity_types::FIREBALL,
            [8.5, -57.0, 8.5],
            [0.0, -0.6, 0.0],
            Some((NOBODY, kind(entity_types::GHAST), None)),
        ),
    );
    let mut output = idle(&mut region);
    for _ in 0..3 {
        if gone(&output, key) {
            break;
        }
        output = idle(&mut region);
    }
    assert!(gone(&output, key));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Explosion { radius, .. } if *radius == 1.0
    )));
    let around: Vec<BlockPos> = (6..=10)
        .flat_map(|x| (-64..=-58).flat_map(move |y| (6..=10).map(move |z| BlockPos::new(x, y, z))))
        .collect();
    let broken = around
        .iter()
        .filter(|pos| pos.y >= -60 && pos.y <= -59 && block(&region, **pos).props().is_air())
        .count();
    let fire = around
        .iter()
        .filter(|pos| clustine_blocks::data::in_tag(block(&region, **pos), "minecraft:fire"))
        .count();
    assert!(broken > 0);
    assert!(fire > 0, "fire in the hole");
}

/// Section 18.5's twelfth: a small fireball that hits a wall of the region east of the
/// line sets fire beside it there, by the effect of a set.
#[test]
fn a_small_fireball_sets_fire_beside_a_wall_across_a_line() {
    let wall = |pos: BlockPos| (pos.x == 17 && pos.y > -61 && pos.y < -55).then_some(blocks::STONE);
    let mut west = west_of_a_line(SEED, wall);
    let key = EntityId(86);
    lay(
        &mut west,
        &fireball(
            key,
            entity_types::SMALL_FIREBALL,
            [15.9, -59.5, 0.5],
            [1.2, 0.0, 0.0],
            Some((NOBODY, kind(entity_types::BLAZE), None)),
        ),
    );
    // It hits the wall's west side at x = 17 in its first tick, and the fire is set in
    // the empty block beside it, east of the line.
    let output = idle(&mut west);
    assert!(gone(&output, key));
    let sent = sets(&output);
    let fire = BlockPos::new(16, -60, 0);
    assert!(
        sent.iter()
            .any(|(at, state)| *at == fire
                && clustine_blocks::data::in_tag(*state, "minecraft:fire")),
        "{sent:?}"
    );
}

#[test]
fn a_small_fireball_sets_a_cow_alight_for_five_seconds_and_hurts_it_by_five() {
    let mut region = world(SEED, nothing);
    let cow_key = EntityId(87);
    lay(&mut region, &cow(cow_key, [3.5, -60.0, 0.5]));
    let key = EntityId(88);
    lay(
        &mut region,
        &fireball(
            key,
            entity_types::SMALL_FIREBALL,
            [1.5, -59.5, 0.5],
            [0.6, 0.0, 0.0],
            Some((NOBODY, kind(entity_types::BLAZE), None)),
        ),
    );
    let mut hit = false;
    for _ in 0..4 {
        let output = idle(&mut region);
        if gone(&output, key) {
            hit = true;
            break;
        }
    }
    assert!(hit);
    let (_, record) = region.find_entity(cow_key).expect("the cow");
    // Its fire, set before the hurt, has burnt for at most the tick after.
    assert!(record.fire_ticks >= 98, "{}", record.fire_ticks);
    let health = mob(&region, cow_key).body.vitals.health;
    assert!(health <= 5.0, "{health}");
    // No blast: a small fireball does not explode.
    assert!(!fireballs(&region).iter().any(|ball| ball.key == key));
}

#[test]
fn a_fireball_primes_the_tnt_it_hits_and_lights_a_campfire() {
    let tnt = BlockPos::new(4, -60, 0);
    let campfire = BlockPos::new(4, -60, 4);
    let unlit = BlockState::parse(
        "minecraft:campfire[facing=north,lit=false,signal_fire=false,waterlogged=false]",
    )
    .expect("a campfire");
    let blocks_of = move |pos: BlockPos| {
        if pos == tnt {
            Some(blocks::TNT)
        } else if pos == campfire {
            Some(unlit)
        } else {
            None
        }
    };
    let mut region = world(SEED, blocks_of);
    lay(
        &mut region,
        &fireball(
            EntityId(89),
            entity_types::SMALL_FIREBALL,
            [2.5, -59.5, 0.5],
            [0.6, 0.0, 0.0],
            None,
        ),
    );
    lay(
        &mut region,
        &fireball(
            EntityId(90),
            entity_types::SMALL_FIREBALL,
            [2.5, -59.6, 4.5],
            [0.6, 0.0, 0.0],
            None,
        ),
    );
    for _ in 0..4 {
        idle(&mut region);
    }
    assert_eq!(primed_tnt(&region).len(), 1);
    assert_eq!(block(&region, tnt), blocks::AIR);
    let lit = block(&region, campfire);
    assert_eq!(lit.property("lit"), Some("true"), "{lit:?}");
}

#[test]
fn a_snowball_s_hit_turns_a_large_fireball_along_its_thrower_s_look_and_makes_them_its_owner() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, [0.5, -60.0, 0.5], false);
    region.players.get_mut(&id).expect("player 1").pose.yaw = 90.0;
    let key = EntityId(91);
    lay(
        &mut region,
        &fireball(
            key,
            entity_types::FIREBALL,
            [0.5, -58.0, 6.0],
            [0.0, 0.0, -0.3],
            Some((NOBODY, kind(entity_types::GHAST), None)),
        ),
    );
    let hit = super::super::thrown::Hit {
        projectile: EntityId(92),
        kind: kind(entity_types::SNOWBALL),
        at: [0.5, -58.0, 5.0],
        motion: [0.0, 0.0, 1.0],
        damage: 0.0,
        owner: region.thrower(player),
    };
    region.thrown_hit_here(key, hit, &mut TickOutput::default());
    let now = the_fireball(&region);
    assert_eq!(
        now.motion.map(f64::to_bits),
        view_vector(90.0, 0.0).map(f64::to_bits)
    );
    assert_eq!(data_of(&now).owner.map(|owner| owner.key), Some(player));
    assert_eq!(
        data_of(&now).owner.and_then(|owner| owner.player),
        Some(id.0)
    );
}

#[test]
fn only_the_large_fireball_is_a_target_of_the_hit_test() {
    let large = fireball(
        EntityId(1),
        entity_types::FIREBALL,
        [0.0; 3],
        [0.0; 3],
        None,
    );
    let small = fireball(
        EntityId(2),
        entity_types::SMALL_FIREBALL,
        [0.0; 3],
        [0.0; 3],
        None,
    );
    assert_eq!(
        super::super::projectiles::pickable_and_alive(&large),
        (true, true)
    );
    assert_eq!(
        super::super::projectiles::pickable_and_alive(&small),
        (false, true)
    );
    // The large one is a block wide, the small one a quarter of a block.
    assert_eq!(EntityTypeInfo::of(entity_types::FIREBALL).width, 1.0);
}
