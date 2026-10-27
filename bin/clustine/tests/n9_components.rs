//! Item components through every writer of the store's format and on the wire, tested
//! from the plan (`docs/groundwork/n9-plan.md`, sections 3.2, 3.3 and 14) and the game's
//! codecs (`n9-javap`) by someone who did not write them.
//!
//! - **The store** (section 3.2, finding 15): a stack with every component of the set,
//!   and one with a removal, read back as they were from every writer: an item entity's
//!   record and the other entities that hold stacks (a thrown item, a rocket, an item
//!   frame, a chest cart, an armour stand), an interaction's held stack and a gift in the
//!   effects' format, the player's place (inventory and ender chest) with their effects
//!   and time since rest, an entity sent across a line, and the entities' file.
//! - **The wire** (section 3.3): each component's bytes as the game's stream codecs
//!   write them, written down here from the dumps, both ways; a server's stack with a
//!   component outside the set is a decode error that names it; a client's components
//!   outside the set are stepped over by their lengths and dropped.
//! - **The edge** (section 3.3): a creative client's components outside the set are
//!   dropped and malformed ones refused, as another player sees the stack held.
//!
//! The containers' NBT and a stack's journey through a region are the region's tests
//! (`crates/clustine-sim/src/region/tests/n9_stacks.rs`).

mod common;

use clustine_data::items;
use clustine_format::{EntitiesFile, LogRecord, LoggedPlace, LoggedStay, read_log};
use clustine_protocol::codec::{Decode, DecodeError, Encode, Reader, Writer};
use clustine_protocol::components::{
    EffectDetails, EffectInstance as WireEffect, PotionContents as WirePotion,
    Rarity as WireRarity, UseCooldown as WireCooldown, WireComponent, WireComponents,
    data_component_type, mob_effect, potion,
};
use clustine_protocol::item::ItemStack as WireStack;
use clustine_protocol::packet_ids::play::serverbound;
use clustine_protocol::packets::Packet;
use clustine_protocol::packets::play::{
    ChunkBatchReceived, ClientboundPlay, ConfirmTeleportation, EntityDataValue,
    ServerboundKeepAlive,
};
use clustine_world::components::{
    BannerLayer, Component, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, Kind,
    PotionContents, Rarity, TooltipDisplay, UseCooldown,
};
use clustine_world::{
    Attached, BlockPos, ChunkPos, Double, Due, Effect, Effected, Entity, EntityChange, EntityData,
    EntityId, EntityUse, Float, Hurt, ItemStack,
};
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

// --- The store ----------------------------------------------------------------------

/// A stack with one of each component of the set.
fn with_every_component(item: i32) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    let components = [
        Component::PotionContents(PotionContents {
            potion: Some("minecraft:strong_swiftness".to_owned()),
            custom_colour: Some(0x0012_3456),
            custom_effects: vec![EffectInstance {
                effect: "minecraft:poison".to_owned(),
                amplifier: 1,
                duration: 200,
                ambient: true,
                visible: false,
                show_icon: true,
            }],
            custom_name: Some("brew".to_owned()),
        }),
        Component::PotionDurationScale(Float(0.5)),
        Component::ChargedProjectiles(vec![ItemStack::new(items::ARROW, 1), with_a_removal()]),
        Component::Enchantments(vec![
            ("minecraft:multishot".to_owned(), 1),
            ("minecraft:unbreaking".to_owned(), 3),
        ]),
        Component::UseCooldown(UseCooldown {
            seconds: Float(2.5),
            group: Some("minecraft:test_group".to_owned()),
        }),
        Component::Fireworks(Fireworks {
            flight_duration: 2,
            explosions: vec![FireworkExplosion {
                shape: FireworkShape::Creeper,
                colours: vec![0x00FF_0000, 0x0000_00FF],
                fade_colours: vec![0x0000_FF00],
                trail: true,
                twinkle: true,
            }],
        }),
        Component::Damage(7),
        Component::IntangibleProjectile,
        Component::BannerPatterns(vec![BannerLayer {
            pattern: "minecraft:stripe_bottom".to_owned(),
            colour: "red".to_owned(),
        }]),
        Component::ItemName("\"Named\"".to_owned()),
        Component::Rarity(Rarity::Epic),
        Component::TooltipDisplay(TooltipDisplay {
            hide_tooltip: true,
            hidden_components: vec!["minecraft:damage".to_owned()],
        }),
        Component::SuspiciousStewEffects(vec![("minecraft:night_vision".to_owned(), 100)]),
        Component::OminousBottleAmplifier(3),
        Component::ChickenVariant("minecraft:cold".to_owned()),
        Component::StoredEnchantments(vec![("minecraft:mending".to_owned(), 1)]),
        Component::DyedColor(0x00B0_2E26),
    ];
    assert_eq!(components.len(), Kind::ALL.len(), "one of each kind");
    for component in components {
        stack.components.set(component);
    }
    stack
}

/// A crossbow whose patch removes its default charge.
fn with_a_removal() -> ItemStack {
    let mut stack = ItemStack::new(items::CROSSBOW, 1);
    stack.components.remove(Kind::ChargedProjectiles);
    stack
}

fn entity(key: u64, kind: i32, data: EntityData) -> Entity {
    Entity {
        key: EntityId(key),
        kind: u16::try_from(kind).expect("a type"),
        uuid: uuid::Uuid::from_u128(u128::from(key) + 11),
        position: [3.5, -60.0, 3.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 4,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    }
}

/// An entity of each kind that holds stacks, each holding stacks with every component.
fn entities_with_stacks() -> Vec<Entity> {
    use clustine_data::entity_types as types;
    let full = with_every_component(items::POTION);
    vec![
        entity(
            1,
            types::ITEM,
            EntityData::Item {
                stack: full.clone(),
                pick_up_delay: 10,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
        ),
        entity(
            2,
            types::SPLASH_POTION,
            EntityData::Thrown {
                stack: full.clone(),
                owner: Some(EntityId(9)),
                left_owner: true,
                has_been_shot: true,
            },
        ),
        entity(
            3,
            types::FIREWORK_ROCKET,
            EntityData::Firework(Box::new(clustine_world::projectiles::Firework {
                stack: full.clone(),
                owner: None,
                left_owner: false,
                has_been_shot: true,
                life: 3,
                lifetime: 30,
                shot_at_angle: true,
            })),
        ),
        entity(
            4,
            types::ITEM_FRAME,
            EntityData::ItemFrame {
                attached: Attached {
                    block: BlockPos::new(3, -60, 4),
                    direction: 2,
                },
                item: Some(with_a_removal()),
                rotation: 3,
                drop_chance: Float(1.0),
                fixed: false,
                invisible: false,
            },
        ),
        entity(
            5,
            types::CHEST_MINECART,
            EntityData::Minecart {
                hurt: Hurt::default(),
                display: None,
                display_offset: None,
                flipped: false,
                contents: vec![(0, full.clone()), (26, with_a_removal())],
                fuel: 0,
                push: [Double(0.0); 2],
                fuse: -1,
                enabled: true,
            },
        ),
        entity(
            6,
            types::ARMOR_STAND,
            EntityData::ArmorStand {
                flags: 0,
                disabled_slots: 0,
                pose: Box::new([[Float(0.0); 3]; 6]),
                equipment: vec![(0, full), (5, with_a_removal())],
                health: Float(20.0),
                last_hit: None,
                air: 300,
                frozen: 0,
            },
        ),
    ]
}

/// A player's place with stacks of every component in their inventory and ender chest,
/// their effects and the time since they last slept.
fn place() -> LoggedPlace {
    let mut inventory: [Option<ItemStack>; clustine_format::INVENTORY_SLOTS] =
        std::array::from_fn(|_| None);
    inventory[0] = Some(with_every_component(items::POTION));
    inventory[40] = Some(with_a_removal());
    let mut ender: [Option<ItemStack>; clustine_format::ENDER_SLOTS] =
        std::array::from_fn(|_| None);
    ender[26] = Some(with_every_component(items::CROSSBOW));
    LoggedPlace {
        position: [1.5, -60.0, 2.5],
        yaw: 10.0,
        pitch: -5.0,
        on_ground: true,
        flying: false,
        selected_slot: 0,
        inventory,
        ender,
        experience: (0, 0.0, 0),
        respawn: None,
        effects: vec![EffectInstance {
            effect: "minecraft:speed".to_owned(),
            amplifier: 1,
            duration: 123,
            ambient: false,
            visible: true,
            show_icon: true,
        }],
        time_since_rest: 4_321,
    }
}

/// Section 3.2 (finding 15): every writer of a stack in a commit of the log keeps every
/// component and removal: entities' records, an interaction's held stack and a gift
/// in the effects, the player's place, and an entity sent across a line.
#[test]
fn every_writer_of_the_log_keeps_every_component_of_a_stack() {
    let held = with_every_component(items::POTION);
    let effects = vec![
        (
            Effect {
                at: BlockPos::new(3, -60, 3),
                what: Effected::OnEntity {
                    key: EntityId(1),
                    by: EntityUse::Interact {
                        player: EntityId(77),
                        creative: true,
                        held: Some(held.clone()),
                        location: [Double(0.1), Double(0.5), Double(0.2)],
                        sneaking: false,
                        from: [Double(1.5), Double(-60.0), Double(1.5)],
                        edge: None,
                    },
                },
            },
            Due::FromTaking,
        ),
        (
            Effect {
                at: BlockPos::new(3, -60, 3),
                what: Effected::Given {
                    player: EntityId(77),
                    stack: held.clone(),
                    no_duplicates: true,
                },
            },
            Due::FromTaking,
        ),
    ];
    let entity_changes: Vec<(ChunkPos, EntityChange)> = entities_with_stacks()
        .into_iter()
        .map(|entity| (ChunkPos::new(0, 0), EntityChange::Whole(entity)))
        .collect();
    let arrivals = vec![(
        ChunkPos::new(0, 0),
        entities_with_stacks().remove(0),
        Due::FromTaking,
    )];
    let record = LogRecord::Commit {
        region: 3,
        tick: 40,
        epoch: 2,
        changes: Vec::new(),
        state: vec![1, 2, 3],
        stays: vec![LoggedStay::Has {
            player: 5,
            entity: 77,
            hops: 1,
            place: place(),
            unit: None,
        }],
        ticks: Vec::new(),
        marks_taken: Vec::new(),
        seams: Vec::new(),
        effects,
        taken: Vec::new(),
        events: Vec::new(),
        entity_changes,
        arrivals,
        arrivals_taken: Vec::new(),
        claims: Vec::new(),
    };
    let (read, _) = read_log(&record.encode()).expect("the record reads");
    assert_eq!(read, [record]);
}

/// Section 3.2: the entities' file keeps every component of every stack its entities
/// hold.
#[test]
fn the_entities_file_keeps_every_component_of_a_stack() {
    let file = EntitiesFile {
        position: ChunkPos::new(0, 0),
        tick: 9,
        epoch: 1,
        entities: entities_with_stacks(),
    };
    assert_eq!(EntitiesFile::decode(&file.encode()), Ok(file));
}

// --- The wire -----------------------------------------------------------------------

fn var_int(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn varint(value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    var_int(value as u32, &mut out);
    out
}

fn string(text: &str) -> Vec<u8> {
    let mut out = varint(i32::try_from(text.len()).expect("short"));
    out.extend_from_slice(text.as_bytes());
    out
}

fn index_of(names: &[&str], name: &str) -> i32 {
    let at = names
        .iter()
        .position(|known| *known == name)
        .unwrap_or_else(|| panic!("{name} in its registry"));
    i32::try_from(at).expect("a registry is small")
}

/// One component of the set with the bytes the game's stream codec writes of it,
/// written down from the dumps.
fn cases() -> Vec<(&'static str, WireComponent, Vec<u8>)> {
    let swiftness = index_of(&potion::NAMES, "minecraft:swiftness");
    let poison = index_of(&mob_effect::NAMES, "minecraft:poison");
    let mut potion_bytes = Vec::new();
    // `PotionContents.STREAM_CODEC`: an optional potion (`holderRegistry`, its id),
    // an optional colour (`INT`, four bytes), the list of `MobEffectInstance`s (the
    // effect's id, then `Details`: amplifier and duration as var ints, ambient,
    // visible and the icon as booleans, an optional hidden one), an optional name.
    potion_bytes.push(1);
    potion_bytes.extend(varint(swiftness));
    potion_bytes.push(1);
    potion_bytes.extend(0x0012_3456_i32.to_be_bytes());
    potion_bytes.extend(varint(1));
    potion_bytes.extend(varint(poison));
    potion_bytes.extend(varint(1));
    potion_bytes.extend(varint(200));
    potion_bytes.extend([1, 0, 1, 0]);
    potion_bytes.push(1);
    potion_bytes.extend(string("brew"));
    let mut cooldown_bytes = 2.5_f32.to_be_bytes().to_vec();
    // `UseCooldown.STREAM_CODEC`: `FLOAT` seconds, an optional `Identifier`.
    cooldown_bytes.push(1);
    cooldown_bytes.extend(string("minecraft:test_group"));
    // An enchantment's number in the registry the edge sends.
    let multishot = 23;
    let mut enchantment_bytes = varint(1);
    // `ItemEnchantments.STREAM_CODEC`: a map, its size, then each enchantment's id
    // and its level as var ints.
    enchantment_bytes.extend(varint(multishot));
    enchantment_bytes.extend(varint(1));
    vec![
        ("damage, a var int", WireComponent::Damage(300), varint(300)),
        (
            "rarity, its ordinal (`idMapper`)",
            WireComponent::Rarity(WireRarity::Epic),
            varint(3),
        ),
        (
            "potion_duration_scale, a float",
            WireComponent::PotionDurationScale(0.25),
            0.25_f32.to_be_bytes().to_vec(),
        ),
        (
            "ominous_bottle_amplifier, a var int",
            WireComponent::OminousBottleAmplifier(3),
            varint(3),
        ),
        (
            "intangible_projectile, `Unit.CODEC` in a tag: an empty compound",
            WireComponent::IntangibleProjectile,
            vec![0x0A, 0x00],
        ),
        (
            "use_cooldown",
            WireComponent::UseCooldown(WireCooldown {
                seconds: 2.5,
                group: Some("minecraft:test_group".to_owned()),
            }),
            cooldown_bytes,
        ),
        (
            "enchantments",
            WireComponent::Enchantments(vec![(multishot, 1)]),
            enchantment_bytes,
        ),
        (
            "potion_contents",
            WireComponent::PotionContents(WirePotion {
                potion: Some(swiftness),
                custom_colour: Some(0x0012_3456),
                custom_effects: vec![WireEffect {
                    effect: poison,
                    details: EffectDetails {
                        amplifier: 1,
                        duration: 200,
                        ambient: true,
                        visible: false,
                        show_icon: true,
                        hidden: None,
                    },
                }],
                custom_name: Some("brew".to_owned()),
            }),
            potion_bytes,
        ),
    ]
}

/// A stack of one potion with `component`, as the wire carries it.
fn potion_with(component: WireComponent) -> WireStack {
    WireStack {
        item: items::POTION,
        count: 1,
        components: WireComponents {
            added: vec![component],
            removed: Vec::new(),
        },
    }
}

/// `DataComponentPatch.STREAM_CODEC` of a server: the count and the item, the numbers
/// of kinds added and removed, each added kind's id and its value, then the removed.
fn clientbound_bytes(id: i32, value: &[u8], removed: &[i32]) -> Vec<u8> {
    let mut out = varint(1);
    out.extend(varint(items::POTION));
    out.extend(varint(1));
    out.extend(varint(i32::try_from(removed.len()).expect("few")));
    out.extend(varint(id));
    out.extend_from_slice(value);
    for kind in removed {
        out.extend(varint(*kind));
    }
    out
}

/// Section 3.3: each component a server sends is written as the game's stream codec
/// writes it, after its kind's id, and Clustine's reader of servers' stacks reads those
/// bytes back into it.
#[test]
fn each_component_is_written_and_read_by_the_game_s_stream_codec() {
    for (name, component, value) in cases() {
        let stack = potion_with(component.clone());
        let mut writer = Writer::new();
        writer.put_item_stack(Some(&stack));
        let expected = clientbound_bytes(component.id(), &value, &[]);
        assert_eq!(writer.into_bytes(), expected, "{name}");
        let mut reader = Reader::new(&expected);
        assert_eq!(reader.item_stack(), Ok(Some(stack)), "{name}");
    }
}

/// `DataComponentPatch.DELIMITED_STREAM_CODEC` of a client: each added value after its
/// length. Clustine reads those bytes into the same component, and writes them so for
/// its bots.
#[test]
fn each_component_a_client_sends_is_read_after_its_length() {
    for (name, component, value) in cases() {
        let mut expected = varint(1);
        expected.extend(varint(items::POTION));
        expected.extend(varint(1));
        expected.extend(varint(0));
        expected.extend(varint(component.id()));
        expected.extend(varint(i32::try_from(value.len()).expect("short")));
        expected.extend_from_slice(&value);
        let stack = potion_with(component);
        let mut writer = Writer::new();
        writer.put_item_stack_serverbound(Some(&stack));
        assert_eq!(writer.into_bytes(), expected, "{name}");
        let mut reader = Reader::new(&expected);
        assert_eq!(reader.untrusted_item_stack(), Ok(Some(stack)), "{name}");
    }
}

/// Section 3.3: a removed kind is written by its id after the added ones.
#[test]
fn a_removed_kind_is_written_by_its_id_after_the_added_ones() {
    let stack = WireStack {
        item: items::POTION,
        count: 1,
        components: WireComponents {
            added: vec![WireComponent::Damage(2)],
            removed: vec![data_component_type::CHARGED_PROJECTILES],
        },
    };
    let mut writer = Writer::new();
    writer.put_item_stack(Some(&stack));
    let bytes = writer.into_bytes();
    assert_eq!(
        bytes,
        clientbound_bytes(
            data_component_type::DAMAGE,
            &varint(2),
            &[data_component_type::CHARGED_PROJECTILES]
        )
    );
    assert_eq!(Reader::new(&bytes).item_stack(), Ok(Some(stack)));
}

/// `minecraft:custom_name` (6): a kind outside the set.
const CUSTOM_NAME: i32 = 6;

/// Section 3.3 (finding 16): a server writes its components with no length before
/// them, so one outside the set cannot be stepped over: the reader of servers' stacks
/// fails, and names its id.
#[test]
fn a_server_s_stack_with_a_component_outside_the_set_is_an_error_that_names_it() {
    assert_eq!(
        data_component_type::NAMES[usize::try_from(CUSTOM_NAME).expect("an index")],
        "minecraft:custom_name"
    );
    // A literal text tag, "x", as the value.
    let bytes = clientbound_bytes(CUSTOM_NAME, &[0x08, 0x00, 0x01, b'x'], &[]);
    let read = Reader::new(&bytes).item_stack();
    let Err(error) = read else {
        panic!("read as {read:?}");
    };
    assert!(
        error.to_string().contains(&CUSTOM_NAME.to_string()),
        "names the id: {error}"
    );
}

/// The bytes of a client's stack of one potion with `custom_name` first (outside the
/// set) and `damage` 5 after it, each after its length.
fn with_a_name_and_damage() -> Vec<u8> {
    let name = [0x08, 0x00, 0x01, b'x'];
    let mut bytes = varint(1);
    bytes.extend(varint(items::POTION));
    bytes.extend(varint(2));
    bytes.extend(varint(0));
    bytes.extend(varint(CUSTOM_NAME));
    bytes.extend(varint(4));
    bytes.extend_from_slice(&name);
    bytes.extend(varint(data_component_type::DAMAGE));
    bytes.extend(varint(1));
    bytes.extend(varint(5));
    bytes
}

/// Section 3.3 (question 1): a client puts each component after its length, so the
/// reader of clients' stacks steps over a kind outside the set and drops it, and keeps
/// the set's.
#[test]
fn a_client_s_component_outside_the_set_is_dropped_and_the_rest_kept() {
    let bytes = with_a_name_and_damage();
    let read = Reader::new(&bytes).untrusted_item_stack();
    assert_eq!(read, Ok(Some(potion_with(WireComponent::Damage(5)))));
    // The patch alone, past the count and the item: the dropped kind is named.
    let mut patch = Reader::new(&bytes);
    patch.var_int().expect("the count");
    patch.var_int().expect("the item");
    let patch = patch.components_serverbound().expect("a patch");
    assert_eq!(patch.dropped, [CUSTOM_NAME]);
}

/// Section 3.3: a value cut short within its length is malformed and refused, not read
/// from what follows.
#[test]
fn a_client_s_component_cut_short_within_its_length_is_refused() {
    let mut bytes = varint(1);
    bytes.extend(varint(items::POTION));
    bytes.extend(varint(1));
    bytes.extend(varint(0));
    bytes.extend(varint(data_component_type::USE_COOLDOWN));
    // Two bytes of a float's four.
    bytes.extend(varint(2));
    bytes.extend([0x40, 0x20]);
    let read = Reader::new(&bytes).untrusted_item_stack();
    assert!(read.is_err(), "{read:?}");
}

// --- The edge, as another player sees it -----------------------------------------------

/// A creative client's slot set with its stack as raw bytes.
#[derive(Debug, Clone, PartialEq)]
struct RawCreativeSlot {
    slot: i16,
    stack: Vec<u8>,
}

impl Packet for RawCreativeSlot {
    const ID: i32 = serverbound::SET_CREATIVE_MODE_SLOT;
}

impl Encode for RawCreativeSlot {
    fn encode(&self, w: &mut Writer) {
        w.put_i16(self.slot);
        w.put_bytes(&self.stack);
    }
}

impl Decode for RawCreativeSlot {
    fn decode(_: &mut Reader<'_>) -> Result<Self, DecodeError> {
        unreachable!("only ever sent")
    }
}

/// How long the looker waits for what it is told to wait for.
const PATIENCE: Duration = Duration::from_secs(30);

/// A client that keeps every play packet it is sent and answers what a client must
/// (keep-alives, teleports, chunk batches), as `common/n9_scene.rs` does.
struct Looker {
    connection: clustine_botswarm::Connection,
    log: Vec<ClientboundPlay>,
    entity: i32,
}

impl Looker {
    async fn join(address: &str, name: &str) -> Looker {
        let mut bot = clustine_botswarm::Bot::join(address, name)
            .await
            .expect("the client joins");
        bot.wait_until(PATIENCE, |bot| {
            let x = (bot.location.0.floor() as i32).div_euclid(16);
            let z = (bot.location.2.floor() as i32).div_euclid(16);
            bot.chunks.contains_key(&(x, z))
        })
        .await
        .expect("the client's chunk comes");
        let entity = bot.info.login.entity_id;
        Looker {
            connection: bot.into_connection(),
            log: Vec::new(),
            entity,
        }
    }

    /// Reads on for `duration`, answering as a client does.
    async fn idle(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while let Ok(frame) = timeout_at(deadline, self.connection.read_frame()).await {
            let frame = frame.expect("the client's connection");
            if let Ok(packet) = ClientboundPlay::decode(&frame) {
                self.answer(&packet).await;
                self.log.push(packet);
            }
        }
    }

    async fn answer(&mut self, packet: &ClientboundPlay) {
        match packet {
            ClientboundPlay::ClientboundKeepAlive(alive) => {
                let answer = ServerboundKeepAlive { id: alive.id };
                self.connection.write(&answer).await.expect("answered");
            }
            ClientboundPlay::SynchronizePlayerPosition(teleport) => {
                let answer = ConfirmTeleportation {
                    teleport_id: teleport.teleport_id,
                    x: teleport.x,
                    y: teleport.y,
                    z: teleport.z,
                    yaw: teleport.yaw,
                    pitch: teleport.pitch,
                };
                self.connection.write(&answer).await.expect("answered");
            }
            ClientboundPlay::ChunkBatchFinished(_) => {
                let answer = ChunkBatchReceived {
                    chunks_per_tick: 20.0,
                };
                self.connection.write(&answer).await.expect("answered");
            }
            _ => {}
        }
    }

    /// Reads on until `done` holds of what was sent, at most for `PATIENCE`.
    async fn until(&mut self, what: &str, mut done: impl FnMut(&[ClientboundPlay]) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while !done(&self.log) {
            let frame = timeout_at(deadline, self.connection.read_frame())
                .await
                .unwrap_or_else(|_| {
                    let equipment: Vec<&ClientboundPlay> = self
                        .log
                        .iter()
                        .filter(|packet| matches!(packet, ClientboundPlay::SetEquipment(_)))
                        .collect();
                    let disconnected = self
                        .log
                        .iter()
                        .find(|packet| matches!(packet, ClientboundPlay::Disconnect(_)));
                    panic!(
                        "no {what} within {PATIENCE:?}; equipment shown: {equipment:?}; \
                         {disconnected:?}"
                    )
                })
                .expect("the looker's connection");
            let Ok(packet) = ClientboundPlay::decode(&frame) else {
                continue;
            };
            self.answer(&packet).await;
            self.log.push(packet);
        }
    }
}

/// The stacks of the item entities the looker was shown, in the order they were.
fn items_shown(log: &[ClientboundPlay]) -> Vec<Option<WireStack>> {
    let items: Vec<i32> = log
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::SpawnEntity(added)
                if added.kind == clustine_data::entity_types::ITEM =>
            {
                Some(added.entity_id)
            }
            _ => None,
        })
        .collect();
    log.iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::SetEntityData(data) if items.contains(&data.entity_id) => data
                .values
                .iter()
                .find_map(|(index, value)| match (index, value) {
                    (8, EntityDataValue::ItemStack(stack)) => Some(stack.clone()),
                    _ => None,
                }),
            _ => None,
        })
        .collect()
}

/// Section 3.3 (question 1): the edge drops a client's components outside the set and
/// keeps the set's; and it validates what it decodes against the registries, refusing
/// the components of a stack whose potion does not exist. What the server keeps is seen
/// by another player in the item the stack makes when it is dropped from the creative
/// inventory (slot -1).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_s_components_outside_the_set_are_dropped_and_malformed_ones_refused() {
    let (server, address) = common::start().await;
    let mut looker = Looker::join(&address, "Looker").await;
    let mut giver = Looker::join(&address, "Giver").await;
    let holder = giver.entity;
    looker
        .until("the giver shown", |log| {
            log.iter().any(|packet| {
                matches!(packet, ClientboundPlay::SpawnEntity(added) if added.entity_id == holder)
            })
        })
        .await;

    giver
        .connection
        .write(&RawCreativeSlot {
            slot: -1,
            stack: with_a_name_and_damage(),
        })
        .await
        .expect("sent");
    giver.idle(Duration::from_millis(200)).await;
    let kept = potion_with(WireComponent::Damage(5));
    looker
        .until("the dropped potion", |log| !items_shown(log).is_empty())
        .await;
    assert_eq!(
        items_shown(&looker.log),
        [Some(kept)],
        "kept with its damage and without its name"
    );

    // A potion that does not exist: the stack's components are refused.
    let mut bogus = Writer::new();
    bogus.put_item_stack_serverbound(Some(&WireStack {
        item: items::POTION,
        count: 1,
        components: WireComponents {
            added: vec![
                WireComponent::PotionContents(WirePotion {
                    potion: Some(9_999),
                    ..WirePotion::default()
                }),
                WireComponent::Damage(6),
            ],
            removed: Vec::new(),
        },
    }));
    giver
        .connection
        .write(&RawCreativeSlot {
            slot: -1,
            stack: bogus.into_bytes(),
        })
        .await
        .expect("sent");
    // And a plain stone after it, which shows that the bogus one was handled.
    let mut stone = Writer::new();
    stone.put_item_stack_serverbound(Some(&WireStack::new(items::STONE, 1)));
    giver
        .connection
        .write(&RawCreativeSlot {
            slot: -1,
            stack: stone.into_bytes(),
        })
        .await
        .expect("sent");
    giver.idle(Duration::from_millis(200)).await;
    assert!(
        !giver
            .log
            .iter()
            .any(|packet| matches!(packet, ClientboundPlay::Disconnect(_))),
        "the giver stays"
    );
    looker
        .until("the stone dropped after it", |log| {
            items_shown(log).iter().any(|stack| {
                stack
                    .as_ref()
                    .is_some_and(|stack| stack.item == items::STONE)
            })
        })
        .await;
    let shown = items_shown(&looker.log);
    let bogus_kept = shown.iter().flatten().any(|stack| {
        stack
            .components
            .added
            .iter()
            .any(|component| matches!(component, WireComponent::PotionContents(_)))
    });
    assert!(!bogus_kept, "no potion of id 9999 is kept: {shown:?}");
    server.stop().await;
}
