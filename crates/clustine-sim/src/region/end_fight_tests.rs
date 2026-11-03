//! Tests of the dragon fight's bar, reports, arena and crystals, tick by tick
//! (`docs/groundwork/p-plan.md`, sections 19.5 items 6 and 7 and 19.6; the brief's 6.1):
//! the End's home shows the bar to its players within 192 blocks of (0, 128, 0) at the
//! twentieth tick, reports it to the store, keeps the arena loaded while it has one, and
//! takes the bar away as they leave the range; another End region shows the published
//! bar and reports its sighting; the dragon's health moves the bar; the crystals on the
//! spikes are counted every hundred ticks.

use clustine_data::{blocks, entity_types};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EndCrystal, EntityData, EntityId,
    EntityIds, PlayerId, RegionId, Vec3,
};
use uuid::Uuid;

use super::super::tnt::tests::{add, add_player, at, idle, record};
use super::super::{Holdings, Region, RegionConfig};
use crate::api::{
    BarChange, BarName, BarStyle, EdgeEvent, FightSeen, Inventory, PlayerEvent, TickInputs,
    TickOutput, Ticket,
};
use crate::state::RegionState;

const EDGE: EdgeId = EdgeId(1);
const OVERWORLD_HOME: RegionId = RegionId(0);
const WORLD_SPAWN: BlockPos = BlockPos::new(0, -60, 0);

/// A region of `dimension` with a stone floor at `floor` over the chunks within `reach`
/// of `centre`, the blocks `blocks` names above it, the homes and a known edge; the
/// overworld's home where `spawn` is given (its home chunk is the spawn's).
fn world(dimension: Dimension, centre: ChunkPos, reach: i32) -> Region {
    let spawn: Option<Vec3> = None;
    let floor = 40;
    let blocks = |_: BlockPos| -> Option<clustine_data::BlockState> { None };
    let rules = dimension.rules();
    let held: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(centre.x + x, centre.z + z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, floor, z, blocks::STONE);
                    for y in floor + 1..=floor + 12 {
                        let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                        if let Some(state) = blocks(pos) {
                            chunk.set(x, y, z, state);
                        }
                    }
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
        world_spawn: WORLD_SPAWN,
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, OVERWORLD_HOME),
            (Dimension::Nether, RegionId(1)),
            (Dimension::End, RegionId(2)),
        ],
        dimension,
        spawn,
        starting_inventory: Inventory::default(),
        default_mode: clustine_world::GameMode::Creative,
        force_mode: false,
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 9,
        game_seed: 0,
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
            edge: EDGE,
            start: 1,
        }],
        ..TickInputs::default()
    });
    region
}

/// The bar's changes told to `id` in a tick.
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

/// Runs `region` until a tick tells `id` a change of the bar, at most 25 ticks: that
/// tick's output and its changes.
fn until_told(region: &mut Region, id: PlayerId) -> (TickOutput, Vec<(Uuid, BarChange)>) {
    for _ in 0..25 {
        let output = idle(region);
        let changes = bar_changes(&output, id);
        if !changes.is_empty() {
            return (output, changes);
        }
    }
    panic!("no change of the bar in 25 ticks");
}

/// The End's home, holding (0, 0) and the chunks about it.
fn end_home() -> Region {
    world(Dimension::End, ChunkPos::new(0, 0), 3)
}

/// `updatePlayers`: a player within 192 of (0, 128, 0) is added to the pink bar of the
/// dragon, full, at the next twentieth tick; the home tells the store its fight, and
/// keeps the arena loaded.
#[test]
fn the_end_s_home_shows_the_dragon_s_bar_to_a_player_within_its_reach_and_keeps_the_arena() {
    let mut home = end_home();
    let (id, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], false);
    let (_, changes) = until_told(&mut home, id);
    let [
        (
            bar,
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
    assert_eq!(
        (name, progress, style),
        (BarName::Dragon, 1.0, BarStyle::DRAGON)
    );
    let reported = home
        .fight_memory
        .reported_fight
        .expect("reported to the store");
    assert_eq!(
        reported,
        FightSeen {
            bar,
            shown: true,
            progress: 1.0,
        }
    );
    for corner in [
        ChunkPos::new(8, 8),
        ChunkPos::new(-8, -8),
        ChunkPos::new(0, 0),
    ] {
        assert!(
            home.arrival_tickets.contains_key(&corner),
            "{corner:?} kept loaded"
        );
    }
}

/// A player beyond the reach is not added, and one who walks out of it is taken off
/// at the next twentieth tick.
#[test]
fn a_player_who_leaves_the_reach_is_taken_off_the_bar() {
    let mut home = end_home();
    let (far, _) = add_player(&mut home, 2, [40.5, 100.0, 40.5], false);
    let (near, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], false);
    if let Some(player) = home.players.get_mut(&far) {
        player.pose.position = Vec3::new(300.5, 100.0, 0.5);
    }
    let (_, changes) = until_told(&mut home, near);
    let bar = changes[0].0;
    if let Some(player) = home.players.get_mut(&near) {
        player.pose.position = Vec3::new(0.5, 100.0, 300.5);
    }
    let (output, changes) = until_told(&mut home, near);
    assert_eq!(changes, [(bar, BarChange::Remove)]);
    assert!(
        bar_changes(&output, far).is_empty(),
        "never shown, never taken"
    );
}

/// The dragon's word of its health moves the bar, told to whoever is shown it.
#[test]
fn the_fight_s_dragon_s_health_moves_the_bar() {
    let mut home = end_home();
    let (id, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], false);
    let (_, changes) = until_told(&mut home, id);
    let bar = changes[0].0;
    let dragon = EntityId(77);
    if let Some(fight) = home.dragon_fight.as_mut() {
        fight.dragon = Some(dragon);
    }
    // Another dragon's word is not the fight's.
    home.fight_dragon_updated(EntityId(78), 10.0, 200.0);
    home.fight_dragon_updated(dragon, 100.0, 200.0);
    let (_, changes) = until_told(&mut home, id);
    assert_eq!(changes, [(bar, BarChange::Progress(0.5))]);
}

/// The fight's dragon dead: the bar is hidden, taken from its players at once, in the
/// tick of the death (`setDragonKilled`'s `setVisible(false)`), and not again.
#[test]
fn the_bar_is_taken_away_when_the_fight_s_dragon_is_killed() {
    let mut home = end_home();
    let (id, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], false);
    let (_, changes) = until_told(&mut home, id);
    let bar = changes[0].0;
    let dragon = EntityId(77);
    if let Some(fight) = home.dragon_fight.as_mut() {
        fight.dragon = Some(dragon);
    }
    let mut output = TickOutput::default();
    home.fight_dragon_killed(dragon, &mut output);
    assert_eq!(bar_changes(&output, id), [(bar, BarChange::Remove)]);
    for _ in 0..25 {
        assert_eq!(bar_changes(&idle(&mut home), id), []);
    }
    let fight = home.dragon_fight.as_ref().expect("the fight");
    assert!(fight.dragon_killed && fight.previously_killed);
}

/// Another End region shows the bar the store published of the home's fight to its own
/// players within the reach, and reports its sighting so that the home loads the arena.
#[test]
fn another_end_region_shows_the_published_bar_and_reports_its_sighting() {
    let mut other = world(Dimension::End, ChunkPos::new(10, 0), 1);
    let bar = Uuid::from_u128(5);
    other.published_end.fight = Some(FightSeen {
        bar,
        shown: true,
        progress: 0.25,
    });
    let (id, _) = add_player(&mut other, 1, [160.5, 100.0, 0.5], false);
    let mut sighted = None;
    let mut added = None;
    for _ in 0..25 {
        let output = idle(&mut other);
        if let Some(village) = &output.village
            && village.in_arena.is_some()
        {
            sighted = village.in_arena;
        }
        let changes = bar_changes(&output, id);
        if !changes.is_empty() {
            added = Some(changes);
            break;
        }
    }
    assert_eq!(sighted, Some(true));
    let changes = added.expect("the bar is shown");
    assert_eq!(
        changes,
        [(
            bar,
            BarChange::Add {
                name: BarName::Dragon,
                progress: 0.25,
                style: BarStyle::DRAGON,
            }
        )]
    );
    assert!(other.dragon_fight.is_none(), "no fight of its own");
}

/// `updateCrystalCount`: with a player near and the arena loaded, a crystal in the top
/// box of one of the world's spikes is counted at the hundredth tick, and the dragon
/// reads it.
#[test]
fn the_crystals_on_the_spikes_are_counted_every_hundred_ticks() {
    let (mut home, _) = arena_with_a_player();
    let spike = clustine_features::features::end::end_spike::spikes_of_seed(0)[0];
    let position = [
        f64::from(spike.centre_x) + 0.5,
        90.0,
        f64::from(spike.centre_z) + 0.5,
    ];
    let data = EntityData::EndCrystal(EndCrystal {
        beam_target: None,
        show_bottom: true,
    });
    let crystal = record(&mut home, entity_types::END_CRYSTAL, position, data);
    add(&mut home, crystal);
    let dragon = EntityId(77);
    if let Some(fight) = home.dragon_fight.as_mut() {
        fight.dragon = Some(dragon);
    }
    let mut counted = None;
    for tick in 0..=100 {
        idle(&mut home);
        let view = home.fight_view(dragon).expect("the fight's own dragon");
        if view.crystals_alive == 1 {
            counted = Some(tick);
            break;
        }
    }
    assert!(counted.is_some(), "counted within a hundred ticks");
    assert!(
        home.fight_view(EntityId(78)).is_none(),
        "not the fight's dragon"
    );
}

/// The End's home with the whole arena loaded and a player over the island.
fn arena_with_a_player() -> (Region, PlayerId) {
    let mut home = world(Dimension::End, ChunkPos::new(0, 0), ARENA);
    let (id, _) = add_player(&mut home, 1, [0.5, 100.0, 0.5], false);
    (home, id)
}

const ARENA: i32 = clustine_world::dragon::ARENA_REACH;

/// `scanState` and `createNewDragon`: once a player is near and the arena loaded, a fight
/// never looked at places its exit portal inactive (bedrock, no portal) and makes its
/// dragon at (0, 128, 0) in its holding pattern.
#[test]
fn a_new_fight_places_its_podium_inactive_and_makes_its_dragon_over_the_island() {
    let (mut home, _) = arena_with_a_player();
    let mut dragon = None;
    for _ in 0..30 {
        idle(&mut home);
        dragon = home.dragon_fight.as_ref().and_then(|fight| fight.dragon);
        if dragon.is_some() {
            break;
        }
    }
    let fight = home.dragon_fight.as_ref().expect("the fight");
    assert!(!fight.needs_state_scanning && !fight.previously_killed && !fight.dragon_killed);
    let origin = fight.exit_portal.expect("its exit portal placed");
    let state = |pos: BlockPos| home.block(pos).expect("loaded");
    assert_eq!(state(origin), blocks::BEDROCK, "the pillar at {origin:?}");
    assert_eq!(state(origin.offset(0, 3, 0)), blocks::BEDROCK);
    assert!(
        state(origin.offset(1, 0, 0)).props().is_air(),
        "no portal while inactive"
    );
    let dragon = dragon.expect("a dragon made");
    let (_, made) = home.find_entity(dragon).expect("in the arena");
    assert_eq!(i32::from(made.kind), entity_types::ENDER_DRAGON);
}

/// `setDragonKilled`: the portal opens, the first gateway of the shuffled twenty is
/// placed with its level event, and the egg is put on the podium at the first kill.
#[test]
fn the_dragon_s_death_opens_the_portal_places_a_gateway_and_the_egg() {
    let (mut home, _) = arena_with_a_player();
    let mut dragon = None;
    for _ in 0..30 {
        idle(&mut home);
        dragon = home.dragon_fight.as_ref().and_then(|fight| fight.dragon);
        if dragon.is_some() {
            break;
        }
    }
    let dragon = dragon.expect("a dragon made");
    let next = *home
        .dragon_fight
        .as_ref()
        .and_then(|fight| fight.gateways.last())
        .expect("twenty gateways");
    let mut output = TickOutput::default();
    home.fight_dragon_killed(dragon, &mut output);
    let fight = home.dragon_fight.as_ref().expect("the fight");
    assert!(fight.dragon_killed && fight.previously_killed);
    assert_eq!(fight.gateways.len(), 19);
    let origin = fight.exit_portal.expect("its exit portal");
    let state = |pos: BlockPos| home.block(pos).expect("loaded");
    assert_eq!(state(origin.offset(1, 0, 0)), blocks::END_PORTAL);
    let angle = 2.0 * (-std::f64::consts::PI + 0.157_079_632_679_489_66 * f64::from(next));
    let gateway = BlockPos::new(
        (96.0 * libm::cos(angle)).floor() as i32,
        75,
        (96.0 * libm::sin(angle)).floor() as i32,
    );
    assert_eq!(state(gateway), blocks::END_GATEWAY);
    assert!(output.events.iter().any(|event| matches!(
        event,
        crate::api::RegionEvent::LevelEvent { kind: 3000, position, .. } if *position == gateway
    )));
    assert_eq!(
        state(origin.offset(0, 4, 0)),
        blocks::DRAGON_EGG,
        "the egg on top"
    );
}
