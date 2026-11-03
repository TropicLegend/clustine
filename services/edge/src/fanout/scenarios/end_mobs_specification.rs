//! What a client is shown of the End's mobs and of the dragon's fight, from their
//! specification (`docs/groundwork/p-brief.md`, sections 6.1 and 6.4;
//! `docs/groundwork/p-plan.md`, sections 19.2 E7, E8 and E9, 19.5 items 7 and 10) and
//! the game's bytecode (d EnderDragon.txt `recreateFromPacket` 2959–2986 and
//! `defineSynchedData`, EndCrystal.txt 46–59, Shulker.txt's synced data,
//! EnderDragonFight.txt `init` 307–376, DragonSittingFlamingPhase.txt), by someone who
//! did not write the code, from reports fed by hand as a region would send them.
//!
//! The mobs' records are made as a structure's template makes them
//! (`clustine_sim::generation::template_entity`), from the tags the game saves
//! (`DragonPhase`, `AttachFace`, `Peek`, `Color`), so that what is shown is what such a
//! record holds.

use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{BossOperation, EffectParticle, particle};
use clustine_sim::api::{BarChange, BarName, BarStyle, Tracked};
use clustine_world::components::{EffectInstance, PotionContents};
use clustine_world::generation::{Draws, Makings, Placed};
use clustine_world::projectiles::Cloud;
use clustine_world::{Biome, EndCrystal, Float};

use super::*;

/// The player's own entity.
const WATCHER: EntityId = EntityId(5);

/// One player at the spawn, shown `HOME` from the west.
async fn at_home() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let mut client = edge.joined(player(1), WATCHER).await;
    edge.quiet().await;
    let there = vec![EntityState {
        entity: WATCHER,
        kind: EntityKind::Player {
            game_mode: 1,
            data: Default::default(),
            player: player(1),
            name: "Player1".to_owned(),
        },
        pose: Pose::at(SPAWN),
    }];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut client, WEST, HOME).await);
    (edge, client)
}

/// No draws: a template's entity made without its `finalizeSpawn` asks none of the
/// level.
struct NoDraws;

impl Draws for NoDraws {
    fn next_int(&mut self, _: u32) -> u32 {
        0
    }

    fn next_float(&mut self) -> f32 {
        0.0
    }

    fn next_double(&mut self) -> f64 {
        0.0
    }

    fn next_bool(&mut self) -> bool {
        false
    }
}

/// The record of a mob a template's tag of the type `id` with `entries` makes.
fn mob_data(id: &str, entries: Vec<(&str, Nbt)>) -> EntityData {
    let mut compound = vec![("id".to_owned(), Nbt::String(id.to_owned()))];
    compound.extend(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value)),
    );
    let mut writer = clustine_protocol::codec::Writer::new();
    writer.put_nbt(&Nbt::Compound(compound));
    let tag = writer.into_bytes();
    let mut makings = Makings::new(2, ChunkPos::new(0, 0));
    let mut level = NoDraws;
    let made = clustine_sim::generation::template_entity(
        1,
        &tag,
        false,
        Placed {
            position: [2.5, -60.0, 2.5],
            yaw: 0.0,
            pitch: 0.0,
            structure: None,
            biome: Biome(0),
            makings: &mut makings,
            level: &mut level,
        },
    );
    made.into_iter()
        .next()
        .unwrap_or_else(|| panic!("a template makes a {id}"))
        .data
}

fn state(key: u64, kind: i32, data: EntityData) -> EntityState {
    EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: kind as u16,
            uuid: Uuid::from_u128(u128::from(key) + 9_000),
            motion: [0.0; 3],
            hops: 0,
            lived: 0,
            data,
            riding: Default::default(),
        },
        pose: Pose::at(Vec3::new(2.5, -50.0, 2.5)),
    }
}

fn report(key: u64, data: EntityData) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position: [2.5, -50.0, 2.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        flags: 0,
        data: Some(data),
    }
}

async fn tick(edge: &mut Harness, client: &mut Client, tick: u64, events: Vec<RegionEvent>) {
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
}

/// The wire id `client` knows `key` by.
fn wire_id(client: &Client, key: u64) -> i32 {
    client.naming(key)[0].1
}

/// Every value of the data `client` was sent for the wire id `id`, in order.
fn data_of(client: &Client, id: i32) -> Vec<(u8, EntityDataValue)> {
    client
        .entity_data
        .iter()
        .filter(|data| data.entity_id == id)
        .flat_map(|data| data.values.clone())
        .collect()
}

/// The values of the first data `client` was sent for the wire id `id`: those of its
/// add (`sendPairingData`).
fn added_with(client: &Client, id: i32) -> Vec<(u8, EntityDataValue)> {
    client
        .entity_data
        .iter()
        .find(|data| data.entity_id == id)
        .map(|data| data.values.clone())
        .unwrap_or_default()
}

/// A dragon's record in the phase `phase` (`DragonPhase`).
fn dragon_in(phase: i32) -> EntityData {
    mob_data(
        "minecraft:ender_dragon",
        vec![("DragonPhase", Nbt::Int(phase))],
    )
}

// The dragon (brief 6.4; plan 19.5 item 10).

/// `recreateFromPacket`: the client numbers the dragon's eight parts `id + 1` to
/// `id + 8`, so no other entity may have them: the next entities shown after a dragon
/// take ids from `id + 9`, and a second dragon keeps its own eight free too.
#[tokio::test]
async fn the_ids_after_a_dragon_s_own_are_kept_for_its_eight_parts() {
    let (mut edge, mut client) = at_home().await;
    let first = (1 << 32) + 20;
    let cow = (1 << 32) + 21;
    let second = (1 << 32) + 22;
    let pig = (1 << 32) + 23;
    let events = vec![RegionEvent::EntitySpawned(state(
        first,
        entity_types::ENDER_DRAGON,
        dragon_in(0),
    ))];
    tick(&mut edge, &mut client, 10, events).await;
    let events = vec![
        RegionEvent::EntitySpawned(state(
            cow,
            entity_types::COW,
            mob_data("minecraft:cow", Vec::new()),
        )),
        RegionEvent::EntitySpawned(state(second, entity_types::ENDER_DRAGON, dragon_in(0))),
        RegionEvent::EntitySpawned(state(
            pig,
            entity_types::PIG,
            mob_data("minecraft:pig", Vec::new()),
        )),
    ];
    tick(&mut edge, &mut client, 11, events).await;
    let dragons = [wire_id(&client, first), wire_id(&client, second)];
    let others = [wire_id(&client, cow), wire_id(&client, pig)];
    for dragon in dragons {
        let spawn = (client.spawns.iter())
            .find(|spawn| spawn.entity_id == dragon)
            .expect("the dragon is spawned");
        assert_eq!(spawn.kind, entity_types::ENDER_DRAGON);
        for id in others.iter().chain(&dragons) {
            assert!(
                *id == dragon || !(dragon + 1..=dragon + 8).contains(id),
                "{id} is one of the parts of {dragon}"
            );
        }
        for spawn in &client.spawns {
            assert!(
                !(dragon + 1..=dragon + 8).contains(&spawn.entity_id),
                "a spawn on a part's id {}",
                spawn.entity_id
            );
        }
    }
    assert_eq!(others[0], dragons[0] + 9, "the next id after the parts");
    edge.end().await;
}

/// `DATA_PHASE` (index 16, an int, its default the hover's 10): a dragon made in its
/// holding pattern is shown phase 0 with its add, and a change of phase is sent.
#[tokio::test]
async fn a_dragon_shows_its_phase_at_its_add_and_as_it_changes() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 30;
    let events = vec![RegionEvent::EntitySpawned(state(
        key,
        entity_types::ENDER_DRAGON,
        dragon_in(0),
    ))];
    tick(&mut edge, &mut client, 10, events).await;
    let id = wire_id(&client, key);
    assert!(
        added_with(&client, id).contains(&(16, EntityDataValue::Int(0))),
        "{:?}",
        added_with(&client, id)
    );
    // Landing (3), reported so for a few ticks, as a region reports what it holds.
    for number in 11..15 {
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report(key, dragon_in(3))),
        ];
        tick(&mut edge, &mut client, number, events).await;
    }
    let mut phases: Vec<EntityDataValue> = (data_of(&client, id).into_iter())
        .filter(|(index, _)| *index == 16)
        .map(|(_, value)| value)
        .collect();
    phases.dedup();
    assert_eq!(phases, [EntityDataValue::Int(0), EntityDataValue::Int(3)]);
    edge.end().await;
}

/// A hovering dragon, the phase's default, is sent no phase at its add.
#[tokio::test]
async fn a_hovering_dragon_is_sent_no_phase_at_its_add() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 31;
    let events = vec![RegionEvent::EntitySpawned(state(
        key,
        entity_types::ENDER_DRAGON,
        dragon_in(10),
    ))];
    tick(&mut edge, &mut client, 10, events).await;
    let id = wire_id(&client, key);
    assert!(
        !data_of(&client, id).iter().any(|(index, _)| *index == 16),
        "{:?}",
        data_of(&client, id)
    );
    edge.end().await;
}

/// `EnderDragonPart`s in `subEntities` order (head, neck, body, three of tail, two
/// wings): the client's hit on `id + 1 + n` reaches the region as the player's attack on
/// the dragon at part n.
#[tokio::test]
async fn a_hit_on_a_part_s_id_reaches_the_region_as_a_hit_on_that_part() {
    let (mut edge, mut client) = at_home().await;
    let key = (1 << 32) + 40;
    let events = vec![RegionEvent::EntitySpawned(state(
        key,
        entity_types::ENDER_DRAGON,
        dragon_in(0),
    ))];
    tick(&mut edge, &mut client, 10, events).await;
    let id = wire_id(&client, key);
    edge.said(WEST).await;
    for (hit, part) in [(id + 1, 0), (id + 2, 1), (id + 4, 3), (id + 7, 6)] {
        let command = Command::OnEntity {
            session: client.session,
            player: client.player,
            id: hit,
            what: OnEntity::Attack,
        };
        edge.commands.send(command).await.unwrap();
        edge.drained().await;
        let said = edge.said(WEST).await;
        let attacks: Vec<(EntityId, Option<u8>)> = (said.iter())
            .filter_map(|message| match &message.body {
                EdgeToWorker::Input {
                    input: PlayerInput::Attack { entity, part },
                    ..
                } => Some((*entity, *part)),
                _ => None,
            })
            .collect();
        assert_eq!(attacks, [(EntityId(key), Some(part))], "the hit on {hit}");
    }
    edge.end().await;
}

// The end crystal (brief 6.3; EndCrystal.txt 46–59).

/// `DATA_BEAM_TARGET` (8, an optional block position, none by default) and
/// `DATA_SHOW_BOTTOM` (9, a boolean, true by default): a crystal its item placed hides
/// its base, one beaming at a block shows that block; a spike's crystal, with no beam
/// and its base, is sent neither.
#[tokio::test]
async fn a_crystal_shows_its_beam_and_whether_it_shows_its_base() {
    let (mut edge, mut client) = at_home().await;
    let spike = (1 << 32) + 50;
    let placed = (1 << 32) + 51;
    let beaming = (1 << 32) + 52;
    let crystal = |beam_target, show_bottom| {
        EntityData::EndCrystal(EndCrystal {
            beam_target,
            show_bottom,
        })
    };
    let target = BlockPos::new(0, 128, 0);
    let events = vec![
        RegionEvent::EntitySpawned(state(spike, entity_types::END_CRYSTAL, crystal(None, true))),
        RegionEvent::EntitySpawned(state(
            placed,
            entity_types::END_CRYSTAL,
            crystal(None, false),
        )),
        RegionEvent::EntitySpawned(state(
            beaming,
            entity_types::END_CRYSTAL,
            crystal(Some(target), true),
        )),
    ];
    tick(&mut edge, &mut client, 10, events).await;
    let own = |client: &Client, key: u64| -> Vec<(u8, EntityDataValue)> {
        let id = wire_id(client, key);
        assert!(
            client
                .spawns
                .iter()
                .any(|spawn| spawn.entity_id == id && spawn.kind == entity_types::END_CRYSTAL),
            "{key} spawned as a crystal"
        );
        (added_with(client, id).into_iter())
            .filter(|(index, _)| (8..=9).contains(index))
            .collect()
    };
    assert_eq!(own(&client, spike), []);
    assert_eq!(own(&client, placed), [(9, EntityDataValue::Boolean(false))]);
    assert_eq!(
        own(&client, beaming),
        [(
            8,
            EntityDataValue::OptionalBlockPos(Some(clustine_protocol::codec::Position {
                x: 0,
                y: 128,
                z: 0,
            }))
        )]
    );
    edge.end().await;
}

// The shulker (plan 19.2 E8; Shulker.txt's synced data).

/// `DATA_ATTACH_FACE_ID` (16, a direction, down by default), `DATA_PEEK_ID` (17, a
/// byte, 0) and `DATA_COLOR_ID` (18, a byte, 16 for none): a shulker attached by the
/// south (3), peeking at 30, of colour 5, is shown each; a plain one on the floor,
/// closed, none.
#[tokio::test]
async fn a_shulker_shows_its_face_its_peek_and_its_colour() {
    let (mut edge, mut client) = at_home().await;
    let plain = (1 << 32) + 60;
    let coloured = (1 << 32) + 61;
    let events = vec![
        RegionEvent::EntitySpawned(state(
            plain,
            entity_types::SHULKER,
            mob_data("minecraft:shulker", Vec::new()),
        )),
        RegionEvent::EntitySpawned(state(
            coloured,
            entity_types::SHULKER,
            mob_data(
                "minecraft:shulker",
                vec![
                    ("AttachFace", Nbt::Byte(3)),
                    ("Peek", Nbt::Byte(30)),
                    ("Color", Nbt::Byte(5)),
                ],
            ),
        )),
    ];
    tick(&mut edge, &mut client, 10, events).await;
    let own = |client: &Client, key: u64| -> Vec<(u8, EntityDataValue)> {
        (added_with(client, wire_id(client, key)).into_iter())
            .filter(|(index, _)| (16..=18).contains(index))
            .collect()
    };
    assert_eq!(own(&client, plain), []);
    assert_eq!(
        own(&client, coloured),
        [
            (16, EntityDataValue::Direction(3)),
            (17, EntityDataValue::Byte(30)),
            (18, EntityDataValue::Byte(5)),
        ]
    );
    // It closes: the peek's change is sent.
    let closed = mob_data(
        "minecraft:shulker",
        vec![("AttachFace", Nbt::Byte(3)), ("Color", Nbt::Byte(5))],
    );
    for number in 11..15 {
        let events = vec![
            RegionEvent::BlockEventsBegin,
            RegionEvent::EntityTracked(report(coloured, closed.clone())),
        ];
        tick(&mut edge, &mut client, number, events).await;
    }
    let mut peeks: Vec<EntityDataValue> = (data_of(&client, wire_id(&client, coloured))
        .into_iter())
    .filter(|(index, _)| *index == 17)
    .map(|(_, value)| value)
    .collect();
    peeks.dedup();
    assert_eq!(peeks, [EntityDataValue::Byte(30), EntityDataValue::Byte(0)]);
    edge.end().await;
}

// The fight's bar (brief 6.1 `init`; plan 19.5 item 7).

/// What the edge sends `client` until it has handled everything so far, decoded.
async fn packets(edge: &mut Harness, client: &mut Client) -> Vec<ClientboundPlay> {
    edge.settle(WEST).await;
    let sequence = edge.sequence();
    let handled = Command::Handled {
        session: client.session,
        player: client.player,
        sequence,
    };
    edge.commands.send(handled).await.unwrap();
    let mut taken = Vec::new();
    while client.acknowledged.last() != Some(&sequence) {
        let packet = edge.packet(client).await.expect("the connection is open");
        taken.push(ClientboundPlay::decode(&packet).expect("the edge sends packets"));
        client.take(packet);
    }
    taken
}

fn boss_events(packets: &[ClientboundPlay]) -> Vec<(Uuid, BossOperation)> {
    packets
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::BossEvent(event) => Some((event.id, event.operation.clone())),
            _ => None,
        })
        .collect()
}

/// `new ServerBossEvent(…, PINK, PROGRESS)` with `setPlayBossMusic(true)` and
/// `setCreateWorldFog(true)`: the bar is added named `entity.minecraft.ender_dragon`,
/// pink (0) of the plain overlay (0), its properties the music (bit 1) and the fog (bit
/// 2) without the darkening (bit 0); its progress and its removal follow as the region
/// tells them.
#[tokio::test]
async fn the_dragon_s_bar_is_added_pink_with_music_and_fog_and_its_progress_follows() {
    let (mut edge, mut client) = at_home().await;
    let bar = Uuid::from_u128(0xD7A6);
    let say = |change| WorkerToEdge::ToPlayer {
        player: player(1),
        event: PlayerEvent::BossBar { id: bar, change },
    };
    edge.tell(
        WEST,
        say(BarChange::Add {
            name: BarName::Dragon,
            progress: 1.0,
            style: BarStyle::DRAGON,
        }),
    );
    let told = boss_events(&packets(&mut edge, &mut client).await);
    let [
        (
            id,
            BossOperation::Add {
                name,
                progress,
                colour,
                overlay,
                properties,
            },
        ),
    ] = &told[..]
    else {
        panic!("one addition: {told:?}");
    };
    assert_eq!(*id, bar);
    assert_eq!(
        (*progress, *colour, *overlay, *properties),
        (1.0, 0, 0, 0b110)
    );
    assert_eq!(
        name.get("translate"),
        Some(&Nbt::String("entity.minecraft.ender_dragon".to_owned()))
    );
    edge.tell(WEST, say(BarChange::Progress(0.25)));
    edge.tell(WEST, say(BarChange::Remove));
    let told = boss_events(&packets(&mut edge, &mut client).await);
    assert_eq!(
        told,
        [
            (bar, BossOperation::UpdateProgress(0.25)),
            (bar, BossOperation::Remove)
        ]
    );
    edge.end().await;
}

// The dragon's breath (brief 6.4, SittingFlaming; DragonSittingFlamingPhase.txt).

fn cloud(effect: &str) -> EntityData {
    EntityData::Cloud(Box::new(Cloud {
        duration: 200,
        wait_time: 20,
        reapplication_delay: 20,
        radius: Float(5.0),
        radius_at: 0,
        radius_on_use: Float(0.0),
        radius_per_tick: Float(0.0),
        duration_on_use: 0,
        potion: PotionContents {
            potion: None,
            custom_colour: None,
            custom_effects: vec![EffectInstance {
                effect: effect.to_owned(),
                amplifier: 0,
                duration: 0,
                ambient: false,
                visible: true,
                show_icon: true,
            }],
            custom_name: None,
        },
        duration_scale: Float(0.25),
        owner: None,
        victims: Vec::new(),
    }))
}

/// `setCustomParticle(PowerParticleOption.create(DRAGON_BREATH, 1.0f))`: the flame's
/// cloud of instant damage shows the dragon's breath at a power of 1, where a cloud of
/// another effect shows the effect's coloured particle.
#[tokio::test]
async fn the_dragon_s_breath_cloud_shows_the_dragon_breath_particle() {
    let (mut edge, mut client) = at_home().await;
    let breath = (1 << 32) + 70;
    let other = (1 << 32) + 71;
    let events = vec![
        RegionEvent::EntitySpawned(state(
            breath,
            entity_types::AREA_EFFECT_CLOUD,
            cloud("minecraft:instant_damage"),
        )),
        RegionEvent::EntitySpawned(state(
            other,
            entity_types::AREA_EFFECT_CLOUD,
            cloud("minecraft:instant_health"),
        )),
    ];
    tick(&mut edge, &mut client, 10, events).await;
    let particles = |client: &Client, key: u64| -> Vec<EffectParticle> {
        (added_with(client, wire_id(client, key)).into_iter())
            .filter_map(|(_, value)| match value {
                EntityDataValue::Particle(particle) => Some(particle),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        particles(&client, breath),
        [EffectParticle::Power {
            kind: particle::DRAGON_BREATH,
            power: 1.0,
        }]
    );
    let shown = particles(&client, other);
    assert!(
        matches!(shown[..], [EffectParticle::Colour { kind, .. }] if kind == particle::ENTITY_EFFECT),
        "{shown:?}"
    );
    edge.end().await;
}
