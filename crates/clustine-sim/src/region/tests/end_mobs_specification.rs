//! The End's mobs and the dragon's fight, from their specification
//! (`docs/groundwork/p-brief.md`, sections 4 and 6.1, 6.3 and 6.4;
//! `docs/groundwork/p-plan.md`, sections 19.2 E7, E8, E9 and "the fight", 19.5 items 6,
//! 7, 10, 13 and 14, and 19.6) and the game's bytecode (d EnderDragonFight.txt,
//! EnderDragon.txt and its phases, EndCrystal.txt, EndCrystalItem.txt, Shulker.txt,
//! ShulkerBullet.txt, EndPodiumFeature.txt), by someone who did not write the code.
//!
//! The fight's tests run in an End of their own: end stone at y = 59 over the chunks
//! about (0, 0), held by the End's home, with a player within the bar's reach. The
//! dragon's, the crystal's and the shulker's run on the shared stone floor at y = -61
//! (`tnt::tests`) unless they need the End.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::projectiles::ProjectileOwner;
use clustine_world::{
    Arrival, Biome, BlockPos, Chunk, ChunkPos, Dimension, Double, Due, EdgeId, EffectId,
    EndCrystal, Entity, EntityChange, EntityData, EntityId, EntityIds, EntityUse, Float, ItemStack,
    PlayerId, ShulkerBullet, Vec3,
};
use uuid::Uuid;

use crate::api::{
    BarChange, BarName, EdgeEvent, Face, Inventory, PlayerEvent, PlayerInput, RegionEvent,
    TickInputs, TickOutput, Ticket,
};
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::types::ender_dragon::{self as dragon_type, DRAGON, DragonState, parts, phases};
use crate::mobs::types::shulker::{self as shulker_type, SHULKER, ShulkerState};
use crate::region::tnt::tests::{
    add_player, idle, nothing, primed, records, west_of_a_line, world,
};
use crate::region::{Holdings, Random, Region, RegionConfig};
use crate::state::RegionState;

const SEED: u64 = 0x5EC_E9E8;

/// The End's floor in the fight's tests: end stone, so the podium's column is 60 high.
const END_FLOOR: i32 = 59;

/// The arena's reach in chunks about (0, 0), the fight's ticket's.
const ARENA: i32 = clustine_world::dragon::ARENA_REACH;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

// What the tests share.

/// An End of chunks within `reach` of (0, 0), all held by the End's home and loaded, end
/// stone at y = 59, the world's seed `game_seed`; its edge started.
fn end_home(reach: i32, game_seed: i64) -> Region {
    let rules = Dimension::End.rules();
    let held: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, END_FLOOR, z, blocks::END_STONE);
                }
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let config = RegionConfig {
        world_spawn: BlockPos::new(0, -60, 0),
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, clustine_world::RegionId(0)),
            (Dimension::Nether, clustine_world::RegionId(1)),
            (Dimension::End, clustine_world::RegionId(2)),
        ],
        dimension: Dimension::End,
        spawn: None,
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: SEED,
        game_seed,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: EdgeId(1),
            start: 1,
        }],
        ..TickInputs::default()
    });
    region
}

/// Where the fight's tests keep their player: on the bar (153 blocks from (0, 128, 0)),
/// in the arena, and far from the island, so that no orb of the dragon's reaches them.
const WATCHER: [f64; 3] = [100.5, 70.0, 100.5];

/// The arena loaded, with a player on the bar.
fn arena(game_seed: i64) -> (Region, PlayerId) {
    let mut home = end_home(ARENA, game_seed);
    let (id, _) = add_player(&mut home, 1, WATCHER, true);
    (home, id)
}

/// Ticks the arena until its fight has made a dragon, at most 60 ticks: its key.
fn fight_s_dragon(home: &mut Region) -> EntityId {
    for _ in 0..60 {
        idle(home);
        let dragon = home.dragon_fight.as_ref().and_then(|fight| fight.dragon);
        if let Some(dragon) = dragon
            && home.find_entity(dragon).is_some()
        {
            return dragon;
        }
    }
    panic!("no dragon made in 60 ticks");
}

/// Lays `entity` in the region's loaded chunk, as a load would have it, or puts it back
/// over what was there under its key.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    if let Some((was, _)) = region.find_entity(entity.key)
        && was != chunk
    {
        panic!("{:?} moved chunks", entity.key);
    }
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record_of(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .map(|(_, record)| record.clone())
        .unwrap_or_else(|| panic!("{key:?} is there"))
}

fn mob_of(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record_of(region, key).data).expect("a mob's record")
}

fn health(region: &Region, key: EntityId) -> f32 {
    mob_of(region, key).body.vitals.health
}

/// The record of `key` with its mob changed by `change`, laid back.
fn alter(region: &mut Region, key: EntityId, change: impl FnOnce(&mut Mob)) {
    let mut record = record_of(region, key);
    let mut mob = Mob::decode(&record.data).expect("a mob's record");
    change(&mut mob);
    record.data = mob.encode();
    lay(region, &record);
}

/// A mob of the type `kind` made as an egg makes one (its constructor's health and
/// attributes), at `feet`, its type state then changed by `change`.
fn made(
    region: &Region,
    key: EntityId,
    kind: u16,
    feet: [f64; 3],
    yaw: f32,
    change: impl FnOnce(&mut Mob),
) -> Entity {
    let mut own = Random::of_entity(region.seed(), key, 0);
    let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, yaw);
    mob.body.vitals.air = crate::living::MAX_AIR;
    change(&mut mob);
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid,
        position: feet,
        motion: [0.0; 3],
        yaw,
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

/// A dragon at `at` facing `yaw`, of full health, its state set by `set`.
fn dragon(
    region: &Region,
    key: EntityId,
    at: [f64; 3],
    yaw: f32,
    set: impl FnOnce(&mut DragonState),
) -> Entity {
    made(region, key, DRAGON, at, yaw, |mob| {
        mob.body.vitals.health = dragon_type::MAX_HEALTH;
        dragon_type::change(mob, set);
    })
}

/// The dragon `key`'s state now.
fn dragon_state(region: &Region, key: EntityId) -> DragonState {
    dragon_type::state_of(&mob_of(region, key))
}

/// The phase a new dragon's state is set to by `setPhase`, from the hover.
fn in_phase(phase: u8) -> impl FnOnce(&mut DragonState) {
    move |own| {
        let mut ignored = Vec::new();
        phases::set_phase(&mut own.phase, &mut own.phases, phase, &mut ignored);
    }
}

/// An end crystal at `at`, its base shown and no beam, as a spike's.
fn crystal(key: EntityId, at: [f64; 3]) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::END_CRYSTAL),
        uuid: Uuid::from_u128(u128::from(key.0) + 0xC0),
        position: at,
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
        data: EntityData::EndCrystal(EndCrystal {
            beam_target: None,
            show_bottom: true,
        }),
    }
}

/// A source of hurt caused and dealt by the player whose entity is `key`, standing at
/// `at`, of the type `name`.
fn by_player(key: EntityId, at: [f64; 3], name: &str) -> DamageSource {
    let player = Causer {
        key,
        kind: kind(entity_types::PLAYER),
        position: at,
        player: Some(0xAB),
        living: true,
    };
    DamageSource {
        kind: DamageType::named(name).expect("a damage type"),
        cause: Some(player.clone()),
        direct: Some(player),
        position: None,
        projectile_motion: None,
        clean_water: false,
    }
}

/// A source of hurt of the type `name` dealt by `direct` and caused by `cause`.
fn source(name: &str, cause: Option<Causer>, direct: Option<Causer>) -> DamageSource {
    DamageSource {
        kind: DamageType::named(name).expect("a damage type"),
        cause,
        direct,
        position: None,
        projectile_motion: None,
        clean_water: false,
    }
}

fn causer(key: EntityId, kind_id: i32, at: [f64; 3], living: bool) -> Causer {
    Causer {
        key,
        kind: kind(kind_id),
        position: at,
        player: None,
        living,
    }
}

/// The experience the orbs of the region's chunks hold together.
fn experience(region: &Region) -> i64 {
    records(region)
        .iter()
        .map(|entity| match entity.data {
            EntityData::ExperienceOrb { value, count, .. } => i64::from(value) * i64::from(count),
            _ => 0,
        })
        .sum()
}

fn level_events(output: &TickOutput, kind: i32) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind: told,
                position,
                ..
            } if *told == kind => Some(*position),
            _ => None,
        })
        .collect()
}

fn global_events(output: &TickOutput, event: i32) -> usize {
    output.village.as_ref().map_or(0, |village| {
        village
            .global_events
            .iter()
            .filter(|told| told.event == event)
            .count()
    })
}

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

fn of_kind(region: &Region, kind_id: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == kind_id)
        .collect()
}

/// Player `id`'s hit on `target` at its part `part`, their input `number`.
fn attack(
    region: &mut Region,
    (id, entity): (PlayerId, EntityId),
    number: u64,
    target: EntityId,
    part: Option<u8>,
) -> TickOutput {
    region.tick(&TickInputs {
        inputs: vec![(
            EdgeId(1),
            id,
            entity,
            number,
            PlayerInput::Attack {
                entity: target,
                part,
            },
        )],
        ..TickInputs::default()
    })
}

fn block(region: &Region, pos: BlockPos) -> BlockState {
    region.block(pos).expect("a loaded block")
}

// The fight's bar (brief 6.1, `init`, `tick` steps 1 to 3, `updatePlayers`; plan 19.5
// item 7 and 19.6).

fn bar_changes(output: &TickOutput, id: PlayerId) -> Vec<(Uuid, BarChange)> {
    output
        .player_events
        .iter()
        .filter_map(|(player, event)| match event {
            PlayerEvent::BossBar { id: bar, change } if *player == id => Some((*bar, *change)),
            _ => None,
        })
        .collect()
}

/// Ticks `region` until a tick tells `id` of the bar, at most `most` ticks: how many
/// ticks that took and what it was told.
fn told_bar(region: &mut Region, id: PlayerId, most: u32) -> Option<(u32, Vec<BarChange>)> {
    for tick in 1..=most {
        let output = idle(region);
        let changes = bar_changes(&output, id);
        if !changes.is_empty() {
            return Some((
                tick,
                changes.into_iter().map(|(_, change)| change).collect(),
            ));
        }
    }
    None
}

/// `init` and `updatePlayers`: a living player within 192 of (0, 128, 0) is put on a
/// pink bar of the plain overlay, with the boss music and the fog, named after the
/// dragon, at the latest at the twentieth tick; one just beyond 192 never is.
#[test]
fn a_player_within_192_of_the_island_s_heart_is_put_on_the_pink_bar_within_twenty_ticks() {
    let mut home = end_home(ARENA, 0);
    // 191.9 and 192.1 blocks from (0, 128, 0), at x = z = ±127 (within the arena) and
    // high above it.
    let up = |reach: f64| 128.0 + (reach * reach - 2.0 * 127.0 * 127.0).sqrt();
    let (near, _) = add_player(&mut home, 1, [127.0, up(191.9), 127.0], true);
    let (far, _) = add_player(&mut home, 2, [-127.0, up(192.1), -127.0], true);
    let mut far_told = Vec::new();
    let mut added = None;
    for tick in 1..=20 {
        let output = idle(&mut home);
        far_told.extend(bar_changes(&output, far));
        let changes = bar_changes(&output, near);
        if !changes.is_empty() {
            added = Some((tick, changes));
            break;
        }
    }
    let (_, changes) = added.expect("put on the bar within twenty ticks");
    let [
        (
            _,
            BarChange::Add {
                name,
                progress,
                style,
            },
        ),
    ] = changes[..]
    else {
        panic!("one addition: {changes:?}");
    };
    assert_eq!(name, BarName::Dragon);
    assert_eq!(progress, 1.0, "a new fight's bar is full");
    // `BossBarColor.PINK` and `BossBarOverlay.PROGRESS` are the first of each.
    assert_eq!((style.colour, style.overlay), (0, 0));
    assert!(style.play_music && style.create_fog && !style.darken_screen);
    for _ in 0..20 {
        far_told.extend(bar_changes(&idle(&mut home), far));
    }
    assert!(far_told.is_empty(), "beyond 192: {far_told:?}");
}

/// `updatePlayers` takes off the bar, at the latest at the next twentieth tick, a
/// player who has gone beyond 192 and one who has died (`validPlayer` asks for one
/// alive).
#[test]
fn a_player_who_goes_beyond_192_or_dies_is_taken_off_the_bar_within_twenty_ticks() {
    let mut home = end_home(ARENA, 0);
    let (walker, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], true);
    let (dying, _) = add_player(&mut home, 2, [4.5, 100.0, 0.5], true);
    let (_, first) = told_bar(&mut home, walker, 20).expect("put on the bar");
    assert!(matches!(first[..], [BarChange::Add { .. }]));
    // Player 2 was put on it the same tick.
    if let Some(player) = home.players.get_mut(&walker) {
        // 217 from (0, 128, 0), still in the arena.
        player.pose.position = Vec3::new(127.5, 250.0, 127.5);
    }
    if let Some(player) = home.players.get_mut(&dying) {
        player.dead = true;
    }
    let mut taken = Vec::new();
    for _ in 0..20 {
        let output = idle(&mut home);
        for id in [walker, dying] {
            if bar_changes(&output, id)
                .iter()
                .any(|(_, change)| *change == BarChange::Remove)
            {
                taken.push(id);
            }
        }
    }
    taken.sort();
    let mut both = vec![walker, dying];
    both.sort();
    assert_eq!(taken, both, "both taken off within twenty ticks");
}

/// `tick` step 4: while a player is on the bar, the fight's ticket of radius 9 at chunk
/// (0, 0) keeps the arena's chunks, −8 to 8 on both axes, loaded (plan 19.5 item 6).
#[test]
fn the_arena_is_kept_loaded_while_a_player_is_on_the_bar() {
    let (mut home, id) = arena(0);
    told_bar(&mut home, id, 20).expect("put on the bar");
    idle(&mut home);
    for x in [-ARENA, 0, ARENA] {
        for z in [-ARENA, 0, ARENA] {
            let chunk = ChunkPos::new(x, z);
            assert!(
                home.arrival_tickets.contains_key(&chunk),
                "{chunk:?} kept loaded"
            );
        }
    }
}

/// `tick` step 3: with nobody on the bar the fight goes no further: no podium is placed
/// and no dragon made, however long.
#[test]
fn with_nobody_on_the_bar_the_fight_neither_scans_nor_makes_a_dragon() {
    let mut home = end_home(ARENA, 0);
    // One dead beside the podium, one alive beyond the reach.
    let (id, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], true);
    if let Some(player) = home.players.get_mut(&id) {
        player.dead = true;
    }
    add_player(&mut home, 2, [127.5, 250.0, 127.5], true);
    for _ in 0..60 {
        idle(&mut home);
    }
    let fight = home
        .dragon_fight
        .as_ref()
        .expect("the home's fight is made");
    assert!(fight.needs_state_scanning, "never scanned");
    assert_eq!(fight.dragon, None);
    assert!(of_kind(&home, entity_types::ENDER_DRAGON).is_empty());
    assert_eq!(
        block(&home, BlockPos::new(0, END_FLOOR + 1, 0)),
        blocks::AIR
    );
}

/// `init`: the twenty gateways' order is 0 to 19 shuffled by `Util.shuffle` with
/// `RandomSource.createThreadLocalInstance(seed)`: from the last, each swapped with
/// `nextInt(i)` of the legacy generator seeded with the world's seed.
#[test]
fn the_twenty_gateways_are_shuffled_by_the_world_s_seed() {
    for game_seed in [0_i64, 0x1234_5678_9ABC, -77] {
        let mut home = end_home(1, game_seed);
        idle(&mut home);
        let fight = home
            .dragon_fight
            .as_ref()
            .expect("the home's fight is made");
        assert_eq!(fight.gateways, java_shuffled(game_seed), "seed {game_seed}");
    }
}

/// `java.util.Random`'s generator, written out here from its definition, so that the
/// expected order owes nothing to the code under test.
struct JavaRandom(i64);

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5_DEEC_E66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self((seed ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & -bound == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits - value + (bound - 1) >= 0 {
                return value;
            }
        }
    }
}

fn java_shuffled(seed: i64) -> Vec<i32> {
    let mut list: Vec<i32> = (0..20).collect();
    let mut random = JavaRandom::new(seed);
    let mut i = list.len() as i32;
    while i > 1 {
        let to = random.next_int(i) as usize;
        list.swap(i as usize - 1, to);
        i -= 1;
    }
    list
}

// The fight's first scan and its dragon (brief 6.1, `scanState`, `findOrCreateDragon`,
// `createNewDragon`; `spawnExitPortal`; EndPodiumFeature.txt).

/// `scanState` on a fight never scanned, once a player is on the bar and the arena is
/// loaded: no portal open, so not killed before; the exit portal is placed inactive on
/// the island's top (the bedrock pillar of four, no end portal about it); and the dragon
/// is made at (0, 128, 0) in its holding pattern, of full health.
#[test]
fn the_first_scan_places_the_podium_inactive_and_makes_the_dragon_over_the_island() {
    let (mut home, _) = arena(0);
    let key = fight_s_dragon(&mut home);
    let fight = home.dragon_fight.as_ref().expect("the fight");
    assert!(!fight.needs_state_scanning);
    assert!(!fight.previously_killed && !fight.dragon_killed);
    // `spawnExitPortal`: the top of (0, 0), less one; not bedrock, so kept.
    let origin = BlockPos::new(0, END_FLOOR, 0);
    assert_eq!(fight.exit_portal, Some(origin));
    for up in 0..4 {
        assert_eq!(
            block(&home, origin.offset(0, up, 0)),
            blocks::BEDROCK,
            "the pillar at {up}"
        );
    }
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (2, 0)] {
        assert!(
            !clustine_blocks::data::is_block(
                block(&home, origin.offset(dx, 0, dz)),
                blocks::END_PORTAL
            ),
            "no end portal while inactive"
        );
    }
    let made = record_of(&home, key);
    assert_eq!(i32::from(made.kind), entity_types::ENDER_DRAGON);
    let [x, y, z] = made.position;
    assert!(
        x.abs() < 2.0 && (y - 128.0).abs() < 2.0 && z.abs() < 2.0,
        "made at (0, 128, 0): {:?}",
        made.position
    );
    assert_eq!(health(&home, key), 200.0);
    assert_eq!(dragon_state(&home, key).phase, phases::HOLDING_PATTERN);
}

/// `createNewDragon`: the new dragon's yaw is `nextFloat() × 360`: a draw, not a fixed
/// angle, so that two worlds' dragons face different ways.
#[test]
fn a_new_dragon_is_turned_at_random() {
    let yaws: Vec<f32> = [1_i64, 2, 3]
        .into_iter()
        .map(|game_seed| {
            let mut home = end_home(ARENA, game_seed);
            home.config.seed = SEED.wrapping_add(game_seed as u64 * 7919);
            add_player(&mut home, 1, WATCHER, true);
            let key = fight_s_dragon(&mut home);
            record_of(&home, key).yaw
        })
        .collect();
    assert!(
        yaws[0] != yaws[1] || yaws[1] != yaws[2],
        "the same yaw thrice: {yaws:?}"
    );
}

/// `tick` step 7: a fight whose dragon has gone unseen for 1200 ticks looks for one, and
/// finding none makes another; not before.
#[test]
fn a_dragon_unseen_for_1200_ticks_is_made_again() {
    let (mut home, _) = arena(0);
    let first = fight_s_dragon(&mut home);
    let (chunk, record) = home.find_entity(first).expect("the dragon");
    let hops = record.hops;
    home.remove_entity(first, hops, chunk, &mut TickOutput::default());
    let mut again = None;
    for tick in 1..=1250_u32 {
        idle(&mut home);
        let dragons = of_kind(&home, entity_types::ENDER_DRAGON);
        if let Some(dragon) = dragons.first() {
            again = Some((tick, dragon.key));
            break;
        }
    }
    let (tick, key) = again.expect("made again within 1250 ticks");
    assert!(
        (1195..=1201).contains(&tick),
        "made again {tick} ticks after it went"
    );
    assert_ne!(key, first);
    let fight = home.dragon_fight.as_ref().expect("the fight");
    assert_eq!(fight.dragon, Some(key), "the fight's own");
}

/// The spikes' places (brief 6.3): spike i stands at `(floor(42 cos θ), floor(42 sin θ))`
/// with `θ = 2(−π + 0.3141592653589793 i)`.
fn spike_centre(i: i32) -> (i32, i32) {
    let angle = 2.0 * (-std::f64::consts::PI + 0.314_159_265_358_979_3 * f64::from(i));
    (
        (42.0 * angle.cos()).floor() as i32,
        (42.0 * angle.sin()).floor() as i32,
    )
}

/// `updateCrystalCount` every 100 ticks: the crystals within the spikes' top boxes,
/// whatever their height, are counted, and a crystal beside a spike, beyond the widest
/// box (radius 5), is not; the dragon reads the count.
#[test]
fn the_crystals_within_the_spikes_top_boxes_are_counted_every_hundred_ticks() {
    let (mut home, _) = arena(0);
    let dragon = fight_s_dragon(&mut home);
    let mut keys = Vec::new();
    for (i, y) in [(0, 70.0), (3, 120.0), (7, 200.0)] {
        let (x, z) = spike_centre(i);
        let at = [f64::from(x) + 0.5, y, f64::from(z) + 0.5];
        let key = home
            .take_entity_key(ChunkPos::containing(at[0], at[2]))
            .expect("a key");
        home.add_fresh_entity(crystal(key, at), &mut TickOutput::default());
        keys.push(key);
    }
    // Beside spike 5, 7.5 out: its box reaches 2.5 to 4.5 beyond the widest top box.
    let (x, z) = spike_centre(5);
    let beside = [f64::from(x) + 7.5, 90.0, f64::from(z) + 0.5];
    let key = home
        .take_entity_key(ChunkPos::containing(beside[0], beside[2]))
        .expect("a key");
    home.add_fresh_entity(crystal(key, beside), &mut TickOutput::default());
    let mut counted = Vec::new();
    for _ in 0..=100 {
        idle(&mut home);
        let view = home
            .fight_view(dragon)
            .expect("the fight's dragon reads its fight");
        counted.push(view.crystals_alive);
    }
    assert_eq!(counted.last(), Some(&3), "{counted:?}");
    let changes = counted.windows(2).filter(|pair| pair[0] != pair[1]).count();
    assert_eq!(changes, 1, "counted once in a hundred ticks: {counted:?}");
}

// The dragon's death and the fight (brief 6.1 `setDragonKilled`, gateways; 6.4 death).

/// Where the gateway of the index `i` stands (brief 6.1, `spawnNewGateway`).
fn gateway_at(i: i32) -> BlockPos {
    let angle = 2.0 * (-std::f64::consts::PI + 0.157_079_632_679_489_66 * f64::from(i));
    BlockPos::new(
        (96.0 * angle.cos()).floor() as i32,
        75,
        (96.0 * angle.sin()).floor() as i32,
    )
}

/// Runs a dying dragon `key` until it is gone, at most 260 ticks: the outputs of the
/// ticks, the first the one after its health reached 0.
fn until_gone(region: &mut Region, key: EntityId) -> Vec<TickOutput> {
    let mut outputs = Vec::new();
    for _ in 0..260 {
        outputs.push(idle(region));
        if region.find_entity(key).is_none() {
            return outputs;
        }
    }
    panic!("{key:?} still there after 260 ticks");
}

/// The fight's dragon `key` struck dead, its health 0 as the death phase's arrival or a
/// sitting dragon's killing blow leaves it, and put at (12.5, 100, 12.5): away from the
/// podium, whose portal its orbs would fall into and leave the End by, as the game's do.
fn struck_dead(region: &mut Region, key: EntityId) {
    let (chunk, mut record) = region
        .find_entity(key)
        .map(|(chunk, record)| (chunk, record.clone()))
        .expect("the dragon");
    region
        .chunks
        .get_mut(&chunk)
        .expect("loaded")
        .apply_entity(&EntityChange::Gone(key));
    record.position = [12.5, 100.0, 12.5];
    let mut mob = Mob::decode(&record.data).expect("a mob's record");
    mob.body.vitals.health = 0.0;
    record.data = mob.encode();
    lay(region, &record);
}

/// `tickDeath` and `setDragonKilled`, the first time: the death roar to every player
/// (`globalLevelEvent(1028)`) at the death's first tick, the dragon gone at its 200th,
/// 12000 experience in orbs; then the exit portal open, the gateway of the last index of
/// the shuffled twenty placed with level event 3000 there, and the egg on the podium's
/// top; the fight killed and killed before, its bar hidden.
#[test]
fn the_first_dragon_s_death_opens_the_portal_places_a_gateway_and_the_egg() {
    let (mut home, id) = arena(0);
    told_bar(&mut home, id, 20).expect("put on the bar");
    let key = fight_s_dragon(&mut home);
    let gateways = home.dragon_fight.as_ref().expect("fight").gateways.clone();
    let next = *gateways.last().expect("twenty gateways");
    let before = experience(&home);
    struck_dead(&mut home, key);
    let outputs = until_gone(&mut home, key);
    let roars: Vec<usize> = (outputs.iter().enumerate())
        .filter(|(_, output)| global_events(output, 1028) > 0)
        .map(|(tick, _)| tick)
        .collect();
    assert_eq!(roars.len(), 1, "one roar: {roars:?}");
    assert_eq!(
        outputs.len() - roars[0],
        200,
        "gone at the 200th tick of its death, counting the roar's as the first"
    );
    assert_eq!(experience(&home) - before, 12_000);
    let fight = home.dragon_fight.as_ref().expect("fight");
    assert!(fight.dragon_killed && fight.previously_killed);
    assert_eq!(fight.gateways[..], gateways[..19]);
    let origin = fight.exit_portal.expect("its exit portal");
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        assert_eq!(
            block(&home, origin.offset(dx, 0, dz)),
            blocks::END_PORTAL,
            "the portal open beside the pillar"
        );
    }
    assert_eq!(block(&home, origin.offset(0, 4, 0)), blocks::DRAGON_EGG);
    let gateway = gateway_at(next);
    assert_eq!(block(&home, gateway), blocks::END_GATEWAY);
    let told: Vec<BlockPos> = outputs
        .iter()
        .flat_map(|output| level_events(output, 3000))
        .collect();
    assert_eq!(told, [gateway]);
    // `setDragonKilled`: progress 0 and hidden: taken from the player, at the latest at
    // the next twentieth tick.
    let mut changes: Vec<BarChange> = (outputs.iter())
        .flat_map(|output| bar_changes(output, id))
        .map(|(_, change)| change)
        .collect();
    for _ in 0..20 {
        let output = idle(&mut home);
        changes.extend(
            bar_changes(&output, id)
                .into_iter()
                .map(|(_, change)| change),
        );
    }
    assert!(changes.contains(&BarChange::Remove), "{changes:?}");
}

/// `tickDeath` and `setDragonKilled` of a fight killed before: 500 experience, no egg,
/// and the next gateway of the twenty.
#[test]
fn a_later_dragon_s_death_gives_500_and_no_egg_and_the_next_gateway() {
    let (mut home, _) = arena(0);
    let key = fight_s_dragon(&mut home);
    let gateways = home.dragon_fight.as_ref().expect("fight").gateways.clone();
    struck_dead(&mut home, key);
    until_gone(&mut home, key);
    // The fight as a respawn leaves it (`setRespawnStage(END)`): not killed, its next
    // dragon to be made.
    if let Some(fight) = home.dragon_fight.as_mut() {
        fight.dragon_killed = false;
        fight.dragon = None;
    }
    let second = fight_s_dragon(&mut home);
    assert_ne!(second, key);
    let origin = home
        .dragon_fight
        .as_ref()
        .and_then(|fight| fight.exit_portal)
        .expect("portal");
    let before = experience(&home);
    struck_dead(&mut home, second);
    let outputs = until_gone(&mut home, second);
    assert_eq!(experience(&home) - before, 500);
    assert_eq!(
        block(&home, origin.offset(0, 4, 0)),
        blocks::DRAGON_EGG,
        "the first egg"
    );
    assert_eq!(
        block(&home, origin.offset(0, 5, 0)),
        blocks::AIR,
        "no second egg"
    );
    let fight = home.dragon_fight.as_ref().expect("fight");
    assert_eq!(fight.gateways[..], gateways[..18]);
    let told: Vec<BlockPos> = outputs
        .iter()
        .flat_map(|output| level_events(output, 3000))
        .collect();
    assert_eq!(told, [gateway_at(gateways[18])]);
    assert_eq!(block(&home, gateway_at(gateways[18])), blocks::END_GATEWAY);
}

/// `EnderDragonFight.onCrystalDestroyed` → `EnderDragon.onCrystalDestroyed`: a player's
/// destruction of the crystal the fight's dragon heals from hurts its head by 10 of
/// explosion caused by that player (2854–2906), which a head takes whole.
#[test]
fn destroying_the_crystal_the_dragon_heals_from_hurts_its_head_by_ten() {
    let (mut home, _) = arena(0);
    let dragon = fight_s_dragon(&mut home);
    let at = record_of(&home, dragon).position;
    // A crystal 20 blocks below, out of its explosion's reach (12) of the dragon, within
    // the dragon's box grown by 32; a player beside it.
    let spot = [at[0] + 0.5, at[1] - 20.0, at[2] + 0.5];
    let key = home
        .take_entity_key(ChunkPos::containing(spot[0], spot[2]))
        .expect("a key");
    home.add_fresh_entity(crystal(key, spot), &mut TickOutput::default());
    let (id, entity) = add_player(&mut home, 2, [spot[0] + 1.5, spot[1], spot[2]], true);
    alter(&mut home, dragon, |mob| {
        dragon_type::change(mob, |own| own.nearest_crystal = Some(key));
    });
    let health_before = health(&home, dragon);
    let output = attack(&mut home, (id, entity), 1, key, None);
    assert!(home.find_entity(key).is_none(), "the crystal destroyed");
    assert_eq!(explosions(&output), [6.0], "its explosion");
    let after = health(&home, dragon);
    // A tick's healing may come before the destruction.
    assert!(
        after <= health_before - 9.0 + 1.0e-3,
        "the head took 10: {health_before} then {after}"
    );
}

/// `EnderDragonFight.onCrystalDestroyed`: a crystal on a spike destroyed is taken from
/// the fight's count at once (`updateCrystalCount`), not at the next hundredth tick.
#[test]
fn a_crystal_destroyed_on_a_spike_is_counted_off_at_once() {
    let (mut home, _) = arena(0);
    let dragon = fight_s_dragon(&mut home);
    let (x, z) = spike_centre(2);
    let at = [f64::from(x) + 0.5, 90.0, f64::from(z) + 0.5];
    let key = home
        .take_entity_key(ChunkPos::containing(at[0], at[2]))
        .expect("a key");
    home.add_fresh_entity(crystal(key, at), &mut TickOutput::default());
    for _ in 0..=100 {
        idle(&mut home);
        if home
            .fight_view(dragon)
            .is_some_and(|view| view.crystals_alive == 1)
        {
            break;
        }
    }
    assert_eq!(
        home.fight_view(dragon).map(|view| view.crystals_alive),
        Some(1)
    );
    let (id, entity) = add_player(&mut home, 2, [at[0] + 1.5, at[1], at[2]], true);
    attack(&mut home, (id, entity), 1, key, None);
    assert!(home.find_entity(key).is_none(), "the crystal destroyed");
    assert_eq!(
        home.fight_view(dragon).map(|view| view.crystals_alive),
        Some(0),
        "counted off in the tick it went"
    );
}

// The dragon (brief 6.4; EnderDragon.txt).

/// `aiStep`'s `tickPart`s after a tick of a hovering dragon of yaw 90 (sine 1, cosine
/// 0) with a flat history: the body half a block east of it by the sine, the wings 4.5 to either side along z and 2 up, the head 6.5 and the neck 5.5
/// out along the sine and 1 down (a hovering dragon sits), the tail 3.5, 5.5 and 7.5 the
/// other way and 1.5 up; each a box of its own size (head 1, neck 3 × 3, body 5 × 3, tail
/// 2 × 2, wings 4 × 2) standing on its place.
#[test]
fn its_eight_parts_stand_about_it_at_their_sizes_after_a_tick() {
    let mut region = world(SEED, nothing);
    let key = EntityId(900);
    let made = dragon(&region, key, [8.5, -40.0, 8.5], 90.0, |_| {});
    lay(&mut region, &made);
    idle(&mut region);
    let record = record_of(&region, key);
    let [x, y, z] = record.position;
    let expected: [(u8, [f64; 3], (f64, f64)); 8] = [
        (parts::HEAD, [x + 6.5, y - 1.0, z], (1.0, 1.0)),
        (parts::NECK, [x + 5.5, y - 1.0, z], (3.0, 3.0)),
        (parts::BODY, [x + 0.5, y, z], (5.0, 3.0)),
        (parts::TAIL[0], [x - 3.5, y + 1.5, z], (2.0, 2.0)),
        (parts::TAIL[1], [x - 5.5, y + 1.5, z], (2.0, 2.0)),
        (parts::TAIL[2], [x - 7.5, y + 1.5, z], (2.0, 2.0)),
        (parts::WING[0], [x, y + 2.0, z + 4.5], (4.0, 2.0)),
        (parts::WING[1], [x, y + 2.0, z - 4.5], (4.0, 2.0)),
    ];
    let boxes = dragon_type::part_boxes(&record).expect("a dragon's parts");
    for (part, at, (width, height)) in expected {
        let bounds = boxes[usize::from(part)];
        let feet = [
            (bounds.min[0] + bounds.max[0]) / 2.0,
            bounds.min[1],
            (bounds.min[2] + bounds.max[2]) / 2.0,
        ];
        for axis in 0..3 {
            assert!(
                (feet[axis] - at[axis]).abs() < 1.0e-4,
                "part {part}: {feet:?}, not {at:?}"
            );
        }
        let size = [
            bounds.max[0] - bounds.min[0],
            bounds.max[1] - bounds.min[1],
            bounds.max[2] - bounds.min[2],
        ];
        assert!(
            (size[0] - width).abs() < 1.0e-6
                && (size[2] - width).abs() < 1.0e-6
                && (size[1] - height).abs() < 1.0e-6,
            "part {part}: {size:?}"
        );
    }
}

/// `hurt(part, …)`: the head and the neck take a player's hurt whole, any other part a
/// quarter of it and up to one more (`a/4 + min(a, 1)`).
#[test]
fn a_hit_on_the_head_or_neck_is_whole_and_on_any_other_part_a_quarter_and_up_to_one_more() {
    let mut region = world(SEED, nothing);
    let player = EntityId(1_000_001);
    let cases: [(u8, f32, f32); 6] = [
        (parts::HEAD, 8.0, 8.0),
        (parts::NECK, 8.0, 8.0),
        (parts::BODY, 8.0, 3.0),
        (parts::TAIL[1], 0.6, 0.75),
        (parts::WING[0], 12.0, 4.0),
        (parts::WING[1], 0.4, 0.5),
    ];
    for (i, (part, amount, taken)) in cases.into_iter().enumerate() {
        let key = EntityId(910 + i as u64);
        let made = dragon(
            &region,
            key,
            [8.5, -30.0 + 4.0 * i as f64, 8.5],
            0.0,
            in_phase(phases::HOLDING_PATTERN),
        );
        lay(&mut region, &made);
        let hit = by_player(player, [8.5, -30.0, 12.5], "minecraft:player_attack");
        let took = region.hurt_mob_at(key, Some(part), &hit, amount, &mut TickOutput::default());
        assert!(took, "part {part} took the hurt");
        let left = health(&region, key);
        assert!(
            (200.0 - left - taken).abs() < 1.0e-4,
            "part {part} hit by {amount}: took {}, not {taken}",
            200.0 - left
        );
    }
}

/// `hurt(part, …)`'s refusals: below 0.01 after the parts' rule, nothing; a hurt that
/// neither a player caused nor is of `#always_hurts_ender_dragons` (an explosion's),
/// nothing; while dying, nothing; and a sitting dragon takes nothing of an arrow. An
/// explosion that no player caused is taken, as its tag says.
#[test]
fn the_dragon_refuses_a_tiny_hurt_a_mob_s_hurt_any_while_dying_and_an_arrow_while_sitting() {
    let mut region = world(SEED, nothing);
    let player = EntityId(1_000_001);
    let zombie = causer(EntityId(77), entity_types::ZOMBIE, [0.5, -60.0, 0.5], true);
    let arrow = causer(EntityId(78), entity_types::ARROW, [0.5, -50.0, 0.5], false);
    let shooter = Causer {
        player: Some(0xAB),
        ..causer(player, entity_types::PLAYER, [0.5, -60.0, 0.5], true)
    };
    let cases: Vec<(u8, u8, DamageSource, f32, f32)> = vec![
        // Head 0.009, and a tail 0.007 (0.00175 + 0.007): below 0.01.
        (
            phases::HOLDING_PATTERN,
            parts::HEAD,
            by_player(player, [0.5, -60.0, 0.5], "minecraft:player_attack"),
            0.009,
            0.0,
        ),
        (
            phases::HOLDING_PATTERN,
            parts::TAIL[0],
            by_player(player, [0.5, -60.0, 0.5], "minecraft:player_attack"),
            0.007,
            0.0,
        ),
        (
            phases::HOLDING_PATTERN,
            parts::HEAD,
            source("minecraft:mob_attack", Some(zombie.clone()), Some(zombie)),
            6.0,
            0.0,
        ),
        (
            phases::DYING,
            parts::HEAD,
            by_player(player, [0.5, -60.0, 0.5], "minecraft:player_attack"),
            6.0,
            0.0,
        ),
        (
            phases::SITTING_SCANNING,
            parts::HEAD,
            source(
                "minecraft:arrow",
                Some(shooter.clone()),
                Some(arrow.clone()),
            ),
            6.0,
            0.0,
        ),
        // An explosion of nobody's on the body: `#is_explosion` always hurts it.
        (
            phases::HOLDING_PATTERN,
            parts::BODY,
            source("minecraft:explosion", None, None),
            8.0,
            3.0,
        ),
        // An arrow in flight is taken, at the head whole.
        (
            phases::HOLDING_PATTERN,
            parts::HEAD,
            source("minecraft:arrow", Some(shooter), Some(arrow)),
            6.0,
            6.0,
        ),
    ];
    for (i, (phase, part, hurt, amount, taken)) in cases.into_iter().enumerate() {
        let key = EntityId(920 + i as u64);
        let made = dragon(
            &region,
            key,
            [8.5, -35.0 + 3.0 * i as f64, -8.5],
            0.0,
            in_phase(phase),
        );
        lay(&mut region, &made);
        region.hurt_mob_at(key, Some(part), &hurt, amount, &mut TickOutput::default());
        let left = health(&region, key);
        assert!(
            (200.0 - left - taken).abs() < 1.0e-4,
            "case {i} (phase {phase}, part {part}): took {}, not {taken}",
            200.0 - left
        );
    }
}

/// The client's hit on a part's id comes to the region as an attack naming the part
/// (plan 19.5 item 10): a creative player's bare hand (1) at a wing takes
/// `1/4 + 1`, at the head 1.
#[test]
fn a_player_s_attack_naming_a_wing_hurts_as_a_wing() {
    let mut region = world(SEED, nothing);
    let key = EntityId(930);
    let made = dragon(
        &region,
        key,
        [8.5, -50.0, 8.5],
        0.0,
        in_phase(phases::HOVERING),
    );
    lay(&mut region, &made);
    idle(&mut region);
    let wing = dragon_type::part_boxes(&record_of(&region, key)).expect("parts")
        [usize::from(parts::WING[0])];
    let feet = [
        wing.max[0] + 1.0,
        wing.min[1],
        (wing.min[2] + wing.max[2]) / 2.0,
    ];
    let player = add_player(&mut region, 1, feet, true);
    attack(&mut region, player, 1, key, Some(parts::WING[0]));
    let left = health(&region, key);
    assert!(
        (200.0 - left - 1.25).abs() < 1.0e-4,
        "took {}",
        200.0 - left
    );
}

/// `handleKillingBlow`: the blow that kills a dragon outside a sitting phase leaves it
/// at 1 and dying; a sitting dragon's kills it, and it keeps its phase.
#[test]
fn the_killing_blow_in_flight_leaves_it_dying_at_one_and_a_sitting_one_dies() {
    let mut region = world(SEED, nothing);
    let player = EntityId(1_000_001);
    let flying = EntityId(940);
    let sitting = EntityId(941);
    let made = dragon(
        &region,
        flying,
        [8.5, -40.0, 8.5],
        0.0,
        in_phase(phases::HOLDING_PATTERN),
    );
    lay(&mut region, &made);
    let made = dragon(
        &region,
        sitting,
        [-8.5, -40.0, -8.5],
        0.0,
        in_phase(phases::SITTING_SCANNING),
    );
    lay(&mut region, &made);
    for key in [flying, sitting] {
        alter(&mut region, key, |mob| mob.body.vitals.health = 5.0);
        let hit = by_player(player, [0.5, -40.0, 0.5], "minecraft:player_attack");
        region.hurt_mob_at(
            key,
            Some(parts::HEAD),
            &hit,
            50.0,
            &mut TickOutput::default(),
        );
    }
    assert_eq!(health(&region, flying), 1.0);
    assert_eq!(dragon_state(&region, flying).phase, phases::DYING);
    assert!(
        health(&region, sitting) <= 0.0,
        "the sitting dragon is killed"
    );
    assert_eq!(
        dragon_state(&region, sitting).phase,
        phases::SITTING_SCANNING
    );
}

/// While sitting, more than a quarter of its maximum health taken since it sat makes it
/// take off: 50 is not more, 50.5 is.
#[test]
fn a_sitting_dragon_takes_off_once_more_than_a_quarter_of_its_health_is_taken() {
    let mut region = world(SEED, nothing);
    let player = EntityId(1_000_001);
    let key = EntityId(945);
    let made = dragon(
        &region,
        key,
        [8.5, -40.0, 8.5],
        0.0,
        in_phase(phases::SITTING_SCANNING),
    );
    lay(&mut region, &made);
    let hit = by_player(player, [0.5, -40.0, 0.5], "minecraft:player_attack");
    region.hurt_mob_at(
        key,
        Some(parts::HEAD),
        &hit,
        50.0,
        &mut TickOutput::default(),
    );
    assert_eq!(dragon_state(&region, key).phase, phases::SITTING_SCANNING);
    // The hurt time of the first passes before the second (`invulnerableTime`).
    for _ in 0..12 {
        idle(&mut region);
    }
    assert_eq!(dragon_state(&region, key).phase, phases::SITTING_SCANNING);
    region.hurt_mob_at(
        key,
        Some(parts::HEAD),
        &hit,
        0.5,
        &mut TickOutput::default(),
    );
    assert_eq!(health(&region, key), 149.5);
    assert_eq!(dragon_state(&region, key).phase, phases::TAKEOFF);
}

/// `checkCrystals`: while its nearest crystal stands, one health every tenth tick, so
/// five in fifty ticks; a crystal beyond its box grown by 32 is never chosen.
#[test]
fn the_nearest_crystal_heals_it_one_every_ten_ticks_and_a_far_one_never() {
    let mut region = world(SEED, nothing);
    let near = EntityId(950);
    let healed = EntityId(951);
    let lonely = EntityId(952);
    let far = EntityId(953);
    lay(&mut region, &crystal(near, [-10.5, -50.0, -14.5]));
    let made = dragon(&region, healed, [-10.5, -40.0, -6.5], 0.0, |own| {
        in_phase(phases::HOVERING)(own);
        own.nearest_crystal = Some(near);
    });
    lay(&mut region, &made);
    // The lonely dragon's box is 16 across: grown by 32 it reaches 40 out of its middle,
    // to z = -11.5; both crystals' boxes end below that.
    lay(&mut region, &crystal(far, [40.5, -40.0, -20.5]));
    let made = dragon(
        &region,
        lonely,
        [20.5, -40.0, 28.5],
        0.0,
        in_phase(phases::HOVERING),
    );
    lay(&mut region, &made);
    alter(&mut region, healed, |mob| mob.body.vitals.health = 150.0);
    alter(&mut region, lonely, |mob| mob.body.vitals.health = 150.0);
    for _ in 0..50 {
        idle(&mut region);
    }
    assert_eq!(health(&region, healed), 155.0);
    assert_eq!(dragon_state(&region, healed).nearest_crystal, Some(near));
    for _ in 0..100 {
        idle(&mut region);
    }
    assert_eq!(
        health(&region, lonely),
        150.0,
        "the far crystal heals nothing"
    );
    assert_eq!(dragon_state(&region, lonely).nearest_crystal, None);
}

// The phases' changes (brief 6.4, phases; the phases' bytecode).

/// Ticks `region` until the dragon `key`'s phase is no longer `from`, at most `most`
/// ticks: on which tick it changed, and to what.
fn phase_change(region: &mut Region, key: EntityId, from: u8, most: u32) -> (u32, u8) {
    for tick in 1..=most {
        idle(region);
        let phase = dragon_state(region, key).phase;
        if phase != from {
            return (tick, phase);
        }
    }
    panic!("still in phase {from} after {most} ticks");
}

/// The podium's top in the shared world: the floor's top is y = -61, so (0, -60, 0).
const PODIUM: [f64; 3] = [0.5, -60.0, 0.5];

/// The landing (DragonLandingPhase.txt): within one block of the podium's top it sits
/// and scans, its count of flames reset.
#[test]
fn a_landing_dragon_at_the_podium_s_top_sits_and_scans_with_no_flames_counted() {
    let mut region = world(SEED, nothing);
    let key = EntityId(960);
    let made = dragon(&region, key, PODIUM, 0.0, |own| {
        own.phases.flaming.count = 3;
        in_phase(phases::LANDING)(own);
    });
    lay(&mut region, &made);
    let (tick, to) = phase_change(&mut region, key, phases::LANDING, 5);
    assert_eq!((tick, to), (1, phases::SITTING_SCANNING));
    assert_eq!(dragon_state(&region, key).phases.flaming.count, 0);
}

/// The scan (DragonSittingScanningPhase.txt): a creative player beside it is no target
/// (`forCombat`, plan 19.5 item 14), so after 100 ticks it takes off.
#[test]
fn a_scanning_dragon_that_finds_no_player_it_may_fight_takes_off_at_the_hundredth_tick() {
    let mut region = world(SEED, nothing);
    let key = EntityId(961);
    let made = dragon(
        &region,
        key,
        PODIUM,
        0.0,
        in_phase(phases::SITTING_SCANNING),
    );
    lay(&mut region, &made);
    add_player(&mut region, 1, [4.5, -60.0, 0.5], false);
    let (tick, to) = phase_change(&mut region, key, phases::SITTING_SCANNING, 120);
    assert_eq!((tick, to), (100, phases::TAKEOFF));
}

/// The roar (DragonSittingAttackingPhase.txt 45–61): `attackingTicks++ >= 40`, a test of
/// the count before its rise, so the flame comes at its 41st tick.
#[test]
fn a_roaring_dragon_flames_at_its_forty_first_tick() {
    let mut region = world(SEED, nothing);
    let key = EntityId(962);
    let made = dragon(
        &region,
        key,
        PODIUM,
        0.0,
        in_phase(phases::SITTING_ATTACKING),
    );
    lay(&mut region, &made);
    let (tick, to) = phase_change(&mut region, key, phases::SITTING_ATTACKING, 60);
    assert_eq!((tick, to), (41, phases::SITTING_FLAMING));
}

/// The flame (DragonSittingFlamingPhase.txt): at its tenth tick a cloud of dragon's
/// breath (radius 5, 200 ticks, instant damage) by its head; at its 200th, back to the
/// scan after fewer than four flames, the cloud gone with the phase's end.
#[test]
fn a_flame_breathes_a_cloud_at_its_tenth_tick_and_scans_again_at_its_two_hundredth() {
    let mut region = world(SEED, nothing);
    let key = EntityId(963);
    let made = dragon(&region, key, PODIUM, 0.0, in_phase(phases::SITTING_FLAMING));
    lay(&mut region, &made);
    assert_eq!(
        dragon_state(&region, key).phases.flaming.count,
        1,
        "begin counts it"
    );
    let mut cloud_at = None;
    for tick in 1..=12 {
        idle(&mut region);
        let clouds = of_kind(&region, entity_types::AREA_EFFECT_CLOUD);
        if let Some(cloud) = clouds.first() {
            cloud_at = Some((tick, cloud.clone()));
            break;
        }
    }
    let (tick, cloud) = cloud_at.expect("a cloud within twelve ticks");
    assert_eq!(tick, 10);
    let EntityData::Cloud(data) = &cloud.data else {
        panic!("a cloud's record");
    };
    assert_eq!((data.radius.0, data.duration), (5.0, 200));
    assert!(
        crate::region::ender_dragon::is_breath(data),
        "dragon's breath"
    );
    let (tick, to) = phase_change(&mut region, key, phases::SITTING_FLAMING, 200);
    assert_eq!(
        (tick, to),
        (190, phases::SITTING_SCANNING),
        "its 200th tick"
    );
    assert!(
        of_kind(&region, entity_types::AREA_EFFECT_CLOUD).is_empty(),
        "the flame's cloud discarded at its end"
    );
}

/// The fourth flame ends in the take-off.
#[test]
fn the_fourth_flame_ends_in_the_take_off() {
    let mut region = world(SEED, nothing);
    let key = EntityId(964);
    let made = dragon(&region, key, PODIUM, 0.0, |own| {
        own.phases.flaming.count = 3;
        in_phase(phases::SITTING_FLAMING)(own);
    });
    lay(&mut region, &made);
    let (tick, to) = phase_change(&mut region, key, phases::SITTING_FLAMING, 210);
    assert_eq!((tick, to), (200, phases::TAKEOFF));
}

/// The take-off (DragonTakeoffPhase.txt 16–50): once its feet are 10 or more from the
/// middle of the podium's top block (`closerToCenterThan(…, 10)` false), the holding
/// pattern; not before.
#[test]
fn a_dragon_taking_off_holds_its_pattern_once_ten_from_the_podium() {
    let mut region = world(SEED, nothing);
    let key = EntityId(965);
    let made = dragon(&region, key, PODIUM, 0.0, in_phase(phases::TAKEOFF));
    lay(&mut region, &made);
    let centre = [0.5, -59.5, 0.5];
    let distance = |region: &Region| {
        let at = record_of(region, key).position;
        ((at[0] - centre[0]).powi(2) + (at[1] - centre[1]).powi(2) + (at[2] - centre[2]).powi(2))
            .sqrt()
    };
    let mut last = distance(&region);
    for _ in 0..600 {
        idle(&mut region);
        let phase = dragon_state(&region, key).phase;
        if phase != phases::TAKEOFF {
            assert_eq!(phase, phases::HOLDING_PATTERN);
            assert!(
                last >= 10.0,
                "it was {last} from the podium when it changed"
            );
            return;
        }
        last = distance(&region);
        assert!(last < 10.0 + 4.0, "still taking off at {last}");
    }
    panic!("still taking off after 600 ticks");
}

/// The charge (DragonChargePlayerPhase.txt 18–85): with its target within 10, the count
/// rises once on the first tick and twice on each after (`timeSinceCharge++ >= 10`, then
/// the nearness's rise), so the holding pattern comes at its seventh tick.
#[test]
fn a_charge_at_a_near_point_ends_in_the_holding_pattern_at_its_seventh_tick() {
    let mut region = world(SEED, nothing);
    let key = EntityId(966);
    let at = [8.5, -40.0, 8.5];
    let made = dragon(&region, key, at, 0.0, |own| {
        in_phase(phases::CHARGING_PLAYER)(own);
        own.phases.charge.target = Some([8.5, -38.0, 8.5]);
    });
    lay(&mut region, &made);
    let (tick, to) = phase_change(&mut region, key, phases::CHARGING_PLAYER, 20);
    assert_eq!((tick, to), (7, phases::HOLDING_PATTERN));
}

/// The charge and the strafe with nothing to go at give up at once for the holding
/// pattern (DragonChargePlayerPhase.txt 0–30, DragonStrafePlayerPhase's `attackTarget`
/// null).
#[test]
fn a_charge_or_strafe_with_nothing_to_go_at_holds_its_pattern_at_once() {
    let mut region = world(SEED, nothing);
    for (key, phase) in [
        (EntityId(967), phases::CHARGING_PLAYER),
        (EntityId(968), phases::STRAFE_PLAYER),
    ] {
        let made = dragon(
            &region,
            key,
            [8.5, -40.0 + f64::from(phase), 8.5],
            0.0,
            in_phase(phase),
        );
        lay(&mut region, &made);
        let (tick, to) = phase_change(&mut region, key, phase, 3);
        assert_eq!(
            (tick, to),
            (1, phases::HOLDING_PATTERN),
            "from phase {phase}"
        );
    }
}

/// The death's flight (DragonDeathPhase.txt): a dying dragon within 10 of the podium's
/// top has its health set to 0; `tickDeath` then counts 200 ticks, roars once at the
/// first (1028, to every player), gives 500 in orbs where it has no fight, rises 0.1 a
/// tick, and is gone at the 200th.
#[test]
fn a_dying_dragon_at_the_podium_dies_over_200_ticks_rising_and_gives_500_without_a_fight() {
    let mut region = world(SEED, nothing);
    let key = EntityId(970);
    let made = dragon(
        &region,
        key,
        [0.5, -55.0, 0.5],
        0.0,
        in_phase(phases::DYING),
    );
    lay(&mut region, &made);
    alter(&mut region, key, |mob| mob.body.vitals.health = 1.0);
    let output = idle(&mut region);
    assert!(health(&region, key) <= 0.0, "health 0 at the podium");
    assert_eq!(global_events(&output, 1028), 0);
    let start = record_of(&region, key).position[1];
    let mut rose = None;
    let mut outputs = Vec::new();
    for _ in 0..260 {
        let output = idle(&mut region);
        let gone = region.find_entity(key).is_none();
        outputs.push(output);
        if gone {
            break;
        }
        if outputs.len() == 100 {
            rose = Some(record_of(&region, key).position[1] - start);
        }
    }
    assert!(region.find_entity(key).is_none(), "gone");
    assert_eq!(outputs.len(), 200, "the 200th tick of its death");
    assert_eq!(global_events(&outputs[0], 1028), 1, "the roar at its first");
    let roars: usize = outputs
        .iter()
        .map(|output| global_events(output, 1028))
        .sum();
    assert_eq!(roars, 1);
    let rose = rose.expect("measured");
    assert!((rose - 10.0).abs() < 1.0e-3, "rose {rose} in 100 ticks");
    assert_eq!(experience(&region), 500);
}

// The end crystal (brief 6.3; plan 19.5 item 13; EndCrystal.txt, EndCrystalItem.txt).

/// `tick` (61–101): in a level with a dragon fight, the End, a crystal sets fire in its
/// own block where that is air; in the overworld it sets none.
#[test]
fn a_crystal_sets_fire_in_its_block_in_the_end_and_not_in_the_overworld() {
    let mut end = world(SEED, nothing);
    end.config.dimension = Dimension::End;
    let mut overworld = world(SEED, nothing);
    let at = [4.5, -60.0, 4.5];
    for region in [&mut end, &mut overworld] {
        lay(region, &crystal(EntityId(980), at));
        idle(region);
    }
    let pos = BlockPos::new(4, -60, 4);
    assert!(
        clustine_blocks::data::is_block(block(&end, pos), blocks::FIRE),
        "fire in the End: {:?}",
        block(&end, pos)
    );
    assert_eq!(block(&overworld, pos), blocks::AIR);
}

/// `hurtServer` (158–214): a player's hit removes it at once with an explosion of power
/// 6; a hurt the dragon caused does nothing; an explosion's removes it with no
/// explosion of its own.
#[test]
fn a_crystal_explodes_at_six_when_hit_ignores_the_dragon_and_goes_silently_in_a_blast() {
    let mut region = world(SEED, nothing);
    let hit = EntityId(981);
    let spared = EntityId(982);
    let blasted = EntityId(983);
    lay(&mut region, &crystal(hit, [-12.5, -60.0, 0.5]));
    lay(&mut region, &crystal(spared, [12.5, -60.0, 0.5]));
    // Within the hit crystal's blast (power 6 reaches 12 blocks).
    lay(&mut region, &crystal(blasted, [-12.5, -60.0, 6.5]));
    // The dragon's hurt, by its wing's mob attack.
    let dragon_key = EntityId(984);
    let wing = EntityUse::MobHit {
        attacker: dragon_key,
        kind: DRAGON,
        from: [Double(12.5), Double(-55.0), Double(0.5)],
        damage: Float(5.0),
        knockback: None,
        fling: None,
        sound: None,
        thorns: false,
    };
    let (chunk, record) = region
        .find_entity(spared)
        .map(|(c, r)| (c, r.clone()))
        .expect("there");
    let mut output = TickOutput::default();
    region.end_crystal_used(&record, chunk, wing, &mut output);
    assert!(
        region.find_entity(spared).is_some(),
        "the dragon's hurt does nothing"
    );
    assert!(explosions(&output).is_empty());
    let player = add_player(&mut region, 1, [-10.5, -60.0, 0.5], false);
    let output = attack(&mut region, player, 1, hit, None);
    assert!(region.find_entity(hit).is_none(), "removed at once");
    assert!(
        region.find_entity(blasted).is_none(),
        "the blast took the other"
    );
    assert_eq!(
        explosions(&output),
        [6.0],
        "one explosion, the hit crystal's"
    );
}

/// `EndCrystalItem.useOn`: on obsidian or bedrock whose block above is air, with no
/// entity in the 1 × 2 × 1 box above, a crystal at the bottom middle of that block
/// without its base; the block two above is not asked about. Not on stone, not under a
/// block, not into an entity.
#[test]
fn the_crystal_item_places_on_obsidian_or_bedrock_under_air_where_no_entity_stands() {
    let placed = |pos: BlockPos| -> Option<BlockState> {
        match (pos.x, pos.y, pos.z) {
            (2, -60, 2) | (8, -60, 2) | (14, -60, 2) | (17, -60, 2) => Some(blocks::OBSIDIAN),
            (5, -60, 2) => Some(blocks::BEDROCK),
            (11, -60, 2) => Some(blocks::STONE),
            // A block right above the obsidian at 8.
            (8, -59, 2) => Some(blocks::DIRT),
            // A block two above the obsidian at 14: not asked about.
            (14, -58, 2) => Some(blocks::STONE),
            _ => None,
        }
    };
    let mut region = world(SEED, placed);
    // A cow standing in the box above the obsidian at 17.
    let cow = made(
        &region,
        EntityId(985),
        kind(entity_types::COW),
        [17.5, -59.0, 2.5],
        0.0,
        |mob| {
            mob.body.flags.no_ai = true;
        },
    );
    lay(&mut region, &cow);
    let (id, entity) = add_player(&mut region, 1, [8.5, -60.0, 4.5], false);
    let player = region.players.get_mut(&id).expect("player 1");
    player.inventory.0[0] = Some(ItemStack::new(items::END_CRYSTAL, 1));
    player.selected_slot = 0;
    for (number, x) in [2, 5, 8, 11, 14, 17].into_iter().enumerate() {
        // Two blocks from the block clicked, out of the box above it.
        if let Some(player) = region.players.get_mut(&id) {
            player.pose.position = Vec3::new(f64::from(x) + 0.5, -60.0, 4.5);
        }
        region.tick(&TickInputs {
            inputs: vec![(
                EdgeId(1),
                id,
                entity,
                number as u64 + 1,
                PlayerInput::UseItemOn {
                    position: BlockPos::new(x, -60, 2),
                    face: Face::Top,
                    cursor: [0.5, 1.0, 0.5],
                    sneaking: false,
                    sequence: number as i32 + 1,
                },
            )],
            ..TickInputs::default()
        });
    }
    let mut placed: Vec<([f64; 3], bool)> = of_kind(&region, entity_types::END_CRYSTAL)
        .into_iter()
        .map(|entity| {
            let EntityData::EndCrystal(data) = entity.data else {
                panic!("a crystal's record");
            };
            (entity.position, data.show_bottom)
        })
        .collect();
    placed.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    assert_eq!(
        placed,
        [
            ([2.5, -59.0, 2.5], false),
            ([5.5, -59.0, 2.5], false),
            ([14.5, -59.0, 2.5], false),
        ]
    );
}

// The shulker and its bullet (plan 19.2 E8, 19.5 item 14; Shulker.txt, ShulkerBullet.txt).

/// A shulker laid on the floor at `feet` (clinging to the block below), made as an egg
/// makes one, its state set by `set`.
fn shulker(
    region: &mut Region,
    key: EntityId,
    feet: [f64; 3],
    set: impl FnOnce(&mut ShulkerState),
) {
    let entity = made(region, key, SHULKER, feet, 0.0, |mob| {
        shulker_type::change(mob, set)
    });
    lay(region, &entity);
}

fn shulker_state(region: &Region, key: EntityId) -> ShulkerState {
    shulker_type::state_of(&mob_of(region, key))
}

fn armour(region: &Region, key: EntityId) -> f64 {
    mob_of(region, key).body.attributes.value(
        SHULKER,
        crate::mobs::body::attribute(clustine_data::attributes::ARMOR),
    )
}

/// `ShulkerPeekGoal` and `setRawPeekAmount`: a shulker with no target now and then peeks
/// out at 30, its covered armour of 20 off while it is open, and closes again with it
/// back on.
#[test]
fn a_shulker_peeks_out_at_thirty_without_its_armour_and_closes_with_twenty() {
    let mut region = world(SEED, nothing);
    let key = EntityId(990);
    shulker(&mut region, key, [3.5, -60.0, 3.5], |_| {});
    let mut opened = None;
    for tick in 0..2000 {
        idle(&mut region);
        if shulker_state(&region, key).peek != 0 {
            opened = Some(tick);
            break;
        }
    }
    opened.expect("it peeks within 2000 ticks");
    assert_eq!(shulker_state(&region, key).peek, 30);
    assert_eq!(armour(&region, key), 0.0, "open: no covered armour");
    let mut closed = false;
    for _ in 0..200 {
        idle(&mut region);
        if shulker_state(&region, key).peek == 0 {
            closed = true;
            break;
        }
    }
    assert!(closed, "closed again within 200 ticks");
    assert_eq!(armour(&region, key), 20.0, "closed: covered");
}

/// `ShulkerAttackGoal`: a shulker with a target opens fully (peek 100) and shoots a
/// bullet each time its timer runs out, the timer then `20 + nextInt(10) · 20 / 2`: 20
/// to 110 ticks, in tens.
#[test]
fn an_attacking_shulker_opens_fully_and_shoots_twenty_to_a_hundred_and_ten_ticks_apart() {
    let mut region = world(SEED, nothing);
    let target = EntityId(991);
    let zombie = made(
        &region,
        target,
        kind(entity_types::ZOMBIE),
        [3.5, -60.0, 10.5],
        0.0,
        |mob| {
            mob.body.flags.no_ai = true;
        },
    );
    lay(&mut region, &zombie);
    let key = EntityId(992);
    let made_shulker = made(&region, key, SHULKER, [3.5, -60.0, 3.5], 0.0, |mob| {
        mob.body.combat.target = Some(target);
    });
    lay(&mut region, &made_shulker);
    let mut shots = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for tick in 0..400_u32 {
        idle(&mut region);
        for bullet in of_kind(&region, entity_types::SHULKER_BULLET) {
            if seen.insert(bullet.key) {
                shots.push(tick);
            }
        }
        if shots.len() == 4 {
            break;
        }
        if !shots.is_empty() {
            assert_eq!(
                shulker_state(&region, key).peek,
                100,
                "open while attacking"
            );
        }
    }
    assert!(shots.len() >= 3, "shots at {shots:?}");
    for pair in shots.windows(2) {
        let apart = pair[1] - pair[0];
        assert!(
            (20..=110).contains(&apart) && apart % 10 == 0,
            "shots {apart} ticks apart: {shots:?}"
        );
    }
}

/// `ShulkerNearestAttackGoal` chooses by `forCombat`, which no creative player passes
/// (plan 19.5 item 14): a creative player beside a shulker is never its target, and it
/// never opens fully nor shoots.
#[test]
fn a_shulker_never_attacks_a_creative_player() {
    let mut region = world(SEED, nothing);
    let key = EntityId(993);
    shulker(&mut region, key, [3.5, -60.0, 3.5], |_| {});
    add_player(&mut region, 1, [5.5, -60.0, 3.5], false);
    for _ in 0..300 {
        idle(&mut region);
        assert_eq!(mob_of(&region, key).body.combat.target, None);
        assert_ne!(shulker_state(&region, key).peek, 100);
    }
    assert!(of_kind(&region, entity_types::SHULKER_BULLET).is_empty());
}

/// `hurtServer`: a hurt that takes and leaves it below half its health teleports it
/// one draw in four, within 8 blocks each way, to an air block it may cling to, with
/// level event 2016 from its old block; a hurt that leaves it above half never does.
#[test]
fn a_shulker_hurt_below_half_its_health_sometimes_teleports_within_eight() {
    let mut region = world(SEED, nothing);
    let mut keys = Vec::new();
    for i in 0..48_u64 {
        let key = EntityId(1100 + i);
        // Within the chunks that tick, -16 to 31.
        let feet = [
            -15.5 + 6.0 * (i % 8) as f64,
            -60.0,
            -15.5 + 7.0 * (i / 8) as f64,
        ];
        shulker(&mut region, key, feet, |_| {});
        keys.push((key, feet));
    }
    let generic = source("minecraft:generic", None, None);
    for (i, (key, _)) in keys.iter().enumerate() {
        // The first half hurt to 10 of 30, the second to 29.
        let amount = if i < 24 { 20.0 } else { 1.0 };
        region.hurt_mob(*key, &generic, amount, &mut TickOutput::default());
    }
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(level_events(&idle(&mut region), 2016));
    }
    let mut went = 0;
    for (i, (key, feet)) in keys.iter().enumerate() {
        let now = record_of(&region, *key).position;
        let moved = now != *feet;
        if i >= 24 {
            assert!(!moved, "{key:?} above half went to {now:?}");
            continue;
        }
        if moved {
            went += 1;
            for axis in 0..3 {
                assert!(
                    (now[axis] - feet[axis]).abs() <= 8.0,
                    "{key:?} went to {now:?}"
                );
            }
            let pos = crate::physics::containing(now);
            assert!(block(&region, pos).props().is_air());
            let old = crate::physics::containing(*feet);
            assert!(events.contains(&old), "level event 2016 from {old:?}");
        }
    }
    assert!((1..24).contains(&went), "{went} of 24 went: a draw in four");
}

/// `hitByShulkerBullet`: an open shulker a bullet hurts teleports, and where no other
/// shulker is about (`(n − 1) / 5` of one shulker is 0) leaves a new one of its colour
/// where it was; a closed one neither teleports nor makes another.
#[test]
fn an_open_shulker_hit_by_a_bullet_teleports_and_leaves_one_of_its_colour_and_a_closed_one_does_not()
 {
    let mut region = world(SEED, nothing);
    let open = EntityId(1200);
    let closed = EntityId(1201);
    let open_at = [-10.5, -60.0, -10.5];
    let closed_at = [20.5, -60.0, 20.5];
    shulker(&mut region, open, open_at, |own| {
        own.peek = 30;
        own.colour = 5;
    });
    shulker(&mut region, closed, closed_at, |own| own.colour = 5);
    let bullet = causer(
        EntityId(1202),
        entity_types::SHULKER_BULLET,
        [0.5, -59.0, 0.5],
        false,
    );
    let hit = source("minecraft:mob_projectile", None, Some(bullet));
    for key in [open, closed] {
        assert!(
            region.hurt_mob(key, &hit, 4.0, &mut TickOutput::default()),
            "hurt"
        );
    }
    for _ in 0..3 {
        idle(&mut region);
    }
    assert_ne!(
        record_of(&region, open).position,
        open_at,
        "the open one went"
    );
    assert_eq!(
        record_of(&region, closed).position,
        closed_at,
        "the closed one stayed"
    );
    let shulkers = of_kind(&region, entity_types::SHULKER);
    assert_eq!(shulkers.len(), 3, "one new shulker");
    let new = shulkers
        .iter()
        .find(|entity| entity.key != open && entity.key != closed)
        .expect("the new one");
    assert_eq!(new.position, open_at, "where the open one was");
    let colour = shulker_type::state_of(&Mob::decode(&new.data).expect("a mob")).colour;
    assert_eq!(colour, 5);
}

/// `hitByShulkerBullet` among six shulkers: `(6 − 1) / 5` is 1, so no draw is below it
/// and no new shulker is made, though the hit one teleports.
#[test]
fn a_bullet_s_hit_among_five_other_shulkers_makes_no_new_one() {
    let mut region = world(SEED, nothing);
    let hit_key = EntityId(1210);
    let at = [0.5, -60.0, 0.5];
    shulker(&mut region, hit_key, at, |own| own.peek = 30);
    for i in 0..5_u64 {
        let feet = [-4.5 + 2.0 * i as f64, -60.0, 4.5];
        shulker(&mut region, EntityId(1211 + i), feet, |_| {});
    }
    let bullet = causer(
        EntityId(1220),
        entity_types::SHULKER_BULLET,
        [0.5, -59.0, 0.5],
        false,
    );
    let hit = source("minecraft:mob_projectile", None, Some(bullet));
    assert!(region.hurt_mob(hit_key, &hit, 4.0, &mut TickOutput::default()));
    for _ in 0..3 {
        idle(&mut region);
    }
    assert_ne!(record_of(&region, hit_key).position, at, "it teleported");
    assert_eq!(
        of_kind(&region, entity_types::SHULKER).len(),
        6,
        "no new shulker"
    );
}

/// A bullet at `at` moving by `motion` at `target`, owned by `owner`, with `steps`.
fn bullet(
    key: EntityId,
    at: [f64; 3],
    motion: [f64; 3],
    target: Option<EntityId>,
    owner: EntityId,
) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: kind(entity_types::SHULKER_BULLET),
        uuid: Uuid::from_u128(u128::from(key.0) + 0xB0),
        position: at,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 5,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::ShulkerBullet(Box::new(ShulkerBullet {
            owner: Some(ProjectileOwner {
                key: owner,
                kind: SHULKER,
                player: None,
            }),
            target,
            steps: 30,
            direction: None,
            target_delta: motion.map(Double),
        })),
    }
}

/// `ShulkerAttackGoal` and the bullet's constructor: the bullet starts at the middle of
/// the shulker's box, its course 10 to 50 steps in tens, its wanted motion 0.15 long,
/// and its first turn not along the axis of the shulker's face (up and down for one on
/// the floor).
#[test]
fn a_shulker_s_first_bullet_leaves_across_its_face_with_ten_to_fifty_steps_at_fifteen_hundredths() {
    let mut region = world(SEED, nothing);
    let target = EntityId(1230);
    let zombie = made(
        &region,
        target,
        kind(entity_types::ZOMBIE),
        [-9.5, -60.0, 12.5],
        0.0,
        |mob| {
            mob.body.flags.no_ai = true;
        },
    );
    lay(&mut region, &zombie);
    let key = EntityId(1231);
    let shooter = made(&region, key, SHULKER, [3.5, -60.0, 3.5], 0.0, |mob| {
        mob.body.combat.target = Some(target);
    });
    lay(&mut region, &shooter);
    let mut first = None;
    for _ in 0..60 {
        let output = idle(&mut region);
        let made: Vec<Entity> = output
            .entity_changes
            .iter()
            .filter_map(|(_, change)| match change {
                EntityChange::Whole(entity)
                    if i32::from(entity.kind) == entity_types::SHULKER_BULLET =>
                {
                    Some(entity.clone())
                }
                _ => None,
            })
            .collect();
        if let Some(bullet) = made.into_iter().next() {
            first = Some(bullet);
            break;
        }
    }
    let first = first.expect("a bullet within 60 ticks");
    let EntityData::ShulkerBullet(data) = &first.data else {
        panic!("a bullet's record");
    };
    // The middle of a closed-to-open box of a shulker on the floor: x and z its block's
    // middle, y between half a block and one up.
    assert!((first.position[0] - 3.5).abs() < 1.0e-9 && (first.position[2] - 3.5).abs() < 1.0e-9);
    assert!(
        (-59.5..=-58.5).contains(&first.position[1]),
        "{:?}",
        first.position
    );
    assert!(
        [10, 20, 30, 40, 50].contains(&data.steps),
        "{} steps",
        data.steps
    );
    let length = data
        .target_delta
        .iter()
        .map(|value| value.0 * value.0)
        .sum::<f64>()
        .sqrt();
    assert!((length - 0.15).abs() < 1.0e-9, "wanted motion {length}");
    let direction = data.direction.expect("far from its target it turns");
    assert!(direction >= 2, "not up or down: {direction}");
}

/// `onHitEntity`: a bullet that meets the mob it flies at hurts it by 4 and makes it
/// levitate; one that meets a creative player hurts nothing and levitates nothing (plan
/// 19.5 item 14); either way the bullet is gone.
#[test]
fn a_bullet_hurts_a_mob_by_four_and_levitates_it_but_not_a_creative_player() {
    let mut region = world(SEED, nothing);
    let target = EntityId(1240);
    // A cow, which wears no armour to lessen the hurt.
    let cow = made(
        &region,
        target,
        kind(entity_types::COW),
        [10.5, -60.0, 0.5],
        0.0,
        |mob| {
            mob.body.flags.no_ai = true;
        },
    );
    lay(&mut region, &cow);
    let before = health(&region, target);
    let flying = EntityId(1241);
    lay(
        &mut region,
        &bullet(
            flying,
            [8.9, -59.0, 0.5],
            [0.3, 0.0, 0.0],
            Some(target),
            EntityId(5),
        ),
    );
    let (player, player_entity) = add_player(&mut region, 1, [-9.5, -60.0, 0.5], false);
    let at_player = EntityId(1242);
    lay(
        &mut region,
        &bullet(
            at_player,
            [-7.9, -59.0, 0.5],
            [-0.3, 0.0, 0.0],
            Some(player_entity),
            EntityId(5),
        ),
    );
    for _ in 0..12 {
        idle(&mut region);
    }
    assert!(region.find_entity(flying).is_none(), "the bullet is gone");
    assert!(region.find_entity(at_player).is_none(), "the other is gone");
    assert_eq!(health(&region, target), before - 4.0);
    let levitation = Effect::named("minecraft:levitation").expect("an effect");
    assert!(
        mob_of(&region, target).body.effects.has(levitation),
        "levitating"
    );
    let player = region.players.get(&player).expect("player 1");
    assert_eq!(player.health, 20.0, "a creative player is not hurt");
    assert!(!player.effects.has(levitation), "nor levitates");
}

/// `ShulkerBullet.hurtServer`: any hurt destroys a bullet, with its hurt sound and 15
/// crit particles where it was: a player's hit, and an explosion.
#[test]
fn any_hurt_destroys_a_bullet_a_hit_or_a_blast() {
    let mut region = world(SEED, nothing);
    let hit = EntityId(1250);
    let blasted = EntityId(1251);
    lay(
        &mut region,
        &bullet(hit, [4.5, -58.0, 4.5], [0.0; 3], None, EntityId(5)),
    );
    lay(
        &mut region,
        &bullet(blasted, [-10.5, -57.0, -10.5], [0.0; 3], None, EntityId(5)),
    );
    let player = add_player(&mut region, 1, [5.5, -60.0, 5.5], false);
    let output = attack(&mut region, player, 1, hit, None);
    assert!(region.find_entity(hit).is_none(), "the hit bullet is gone");
    let crits = output.events.iter().any(|event| {
        matches!(event, RegionEvent::Particles { count: 15, particle, .. } if *particle == 13)
    });
    assert!(crits, "fifteen crits");
    let heard = output.events.iter().any(|event| {
        matches!(event, RegionEvent::Sound { sound, .. } if *sound == clustine_data::sounds::ENTITY_SHULKER_BULLET_HURT)
    });
    assert!(heard, "its hurt sound");
    primed(&mut region, [-10.5, -60.0, -8.5], 1, None);
    let mut blasts = Vec::new();
    for _ in 0..3 {
        blasts.extend(explosions(&idle(&mut region)));
    }
    assert_eq!(blasts, [4.0], "the TNT's blast");
    assert!(
        region.find_entity(blasted).is_none(),
        "the blasted bullet is gone"
    );
}

/// A bullet flying east across a region's line goes to the region beyond with its
/// target, owner, steps and course; there it flies on and hits the mob it flies at,
/// which levitates.
#[test]
fn a_bullet_crossing_a_line_keeps_its_course_and_hits_its_target_beyond() {
    let mut west = west_of_a_line(SEED, nothing);
    let target = EntityId(1260);
    let cow = made(
        &west,
        target,
        kind(entity_types::COW),
        [20.5, -60.0, 0.5],
        0.0,
        |mob| {
            mob.body.flags.no_ai = true;
        },
    );
    // The cow is the east's; the west sees it in its copy.
    west.copies
        .get_mut(&cow.chunk())
        .expect("a copied chunk")
        .apply_entity(&EntityChange::Whole(cow.clone()));
    let key = EntityId(1261);
    let mut flying = bullet(
        key,
        [15.7, -59.0, 0.5],
        [0.3, 0.0, 0.0],
        Some(target),
        EntityId(5),
    );
    if let EntityData::ShulkerBullet(data) = &mut flying.data {
        data.direction = Some(5);
        data.steps = 30;
    }
    lay(&mut west, &flying);
    let output = idle(&mut west);
    assert!(west.find_entity(key).is_none(), "gone from the west");
    let [(chunk, sent)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    let EntityData::ShulkerBullet(data) = &sent.data else {
        panic!("a bullet's record");
    };
    assert_eq!(data.target, Some(target));
    assert_eq!(data.owner.map(|owner| owner.key), Some(EntityId(5)));
    assert_eq!(data.direction, Some(5));
    assert_eq!(data.steps, 29, "one step taken");
    let mut east = world(SEED, nothing);
    lay(&mut east, &cow);
    let before = health(&east, target);
    east.tick(&TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 1,
                index: 0,
            },
            chunk: *chunk,
            entity: sent.clone(),
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    for _ in 0..40 {
        idle(&mut east);
        if east.find_entity(key).is_none() {
            break;
        }
    }
    assert!(
        east.find_entity(key).is_none(),
        "the bullet hit and is gone"
    );
    assert_eq!(health(&east, target), before - 4.0);
    let levitation = Effect::named("minecraft:levitation").expect("an effect");
    assert!(mob_of(&east, target).body.effects.has(levitation));
}
