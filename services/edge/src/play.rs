//! The play state of one connection.
//!
//! The connection registers with the fan-out task and from then on does three things:
//! it writes the packets the fan-out task queues for it, it passes on what the client
//! sends, and it checks with keep-alives that the client is still there.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use clustine_data::{ITEMS, ItemComponents};
use clustine_protocol::item as protocol;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::Packet;
use clustine_protocol::packets::configuration::ClientInformation;
use clustine_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, Disconnect, LevelChunkWithLight, LightUpdate,
    PlayerAction, ServerboundPlay, ServerboundUseItem, UseItemOn, face, movement_flags,
    player_action, player_command, player_input,
};
use clustine_sim::api::{Face, HOTBAR_SLOTS, Inventory, ItemStack, Keys, PlayerInput, VehicleMove};
use clustine_world::{BlockPos, PlayerId, Vec3};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval_at};
use tracing::warn;

use crate::Shared;
use crate::connection::{Connection, ConnectionError};
use crate::fanout::{Command, NO_TELEPORT, OnEntity, SessionId};
use crate::figures::{self, FIGURES};
use crate::login::Profile;

/// The value of the hand field for the hand that holds the selected hotbar item.
const MAIN_HAND: i32 = 0;

/// Packets that may wait for a client before it is considered too slow and dropped.
const OUTBOUND_CAPACITY: usize = 2048;

pub(crate) async fn serve(
    connection: &mut Connection,
    shared: &Shared,
    profile: Profile,
    client_information: Option<ClientInformation>,
) -> Result<(), ConnectionError> {
    connection.stop_timing_reads();
    let session = SessionId(shared.next_session.fetch_add(1, Ordering::Relaxed));
    let player = PlayerId(profile.uuid);
    let (outbound, mut packets) = mpsc::channel(OUTBOUND_CAPACITY);
    let awaiting_teleport = Arc::new(AtomicI32::new(NO_TELEPORT));
    let join = Command::Join {
        session,
        profile,
        requested_view_distance: client_information
            .map(|information| i32::from(information.view_distance)),
        outbound,
        awaiting_teleport: Arc::clone(&awaiting_teleport),
    };
    if shared.fanout.send(join).await.is_err() {
        return Err(ConnectionError::ShuttingDown);
    }

    let client = Client {
        session,
        player,
        awaiting_teleport,
    };
    let result = pump(connection, shared, &client, &mut packets).await;
    // Whatever ended the connection, the fan-out task has to forget the player.
    let leave = Command::Leave { session, player };
    let _ = shared.fanout.send(leave).await;
    result
}

/// What the connection knows about its player while in the play state.
struct Client {
    session: SessionId,
    player: PlayerId,
    /// Set by the fan-out task; see [`NO_TELEPORT`].
    awaiting_teleport: Arc<AtomicI32>,
}

/// Moves packets in both directions until the connection ends.
async fn pump(
    connection: &mut Connection,
    shared: &Shared,
    client: &Client,
    packets: &mut mpsc::Receiver<Bytes>,
) -> Result<(), ConnectionError> {
    let interval = shared.config.keep_alive_interval;
    let mut keep_alive = interval_at(Instant::now() + interval, interval);
    keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut next_keep_alive_id = 0;
    let mut awaiting_keep_alive = None;
    // Whether the client last said the sneak key is down. A click carries it to the
    // region: sneaking with a block in hand places it against a door rather than
    // opening the door.
    let mut sneaking = false;
    // The keys the region was last told the player holds: `player_input` says them on
    // every change, and sprinting comes by `player_command` as well.
    let mut keys = Keys::default();
    // The flags of the client's last move (on the ground, against a wall), which a
    // confirmed teleport's look is passed on with; none before its first move.
    let mut flags = None;
    // The client only understands keep-alives once it has been put into the world.
    let mut in_world = false;
    let mut drops = DropThrottler::new(Instant::now());

    loop {
        tokio::select! {
            frame = connection.read_frame() => {
                let Some(frame) = frame? else {
                    return Ok(());
                };
                match ServerboundPlay::decode(&frame)? {
                    ServerboundPlay::ServerboundKeepAlive(answer) => {
                        if awaiting_keep_alive != Some(answer.id) {
                            return Err(ConnectionError::Protocol("unexpected keep-alive id"));
                        }
                        awaiting_keep_alive = None;
                    }
                    ServerboundPlay::ChunkBatchReceived(received) => {
                        let command = Command::ChunkBatchReceived {
                            session: client.session,
                            player: client.player,
                            chunks_per_tick: received.chunks_per_tick,
                        };
                        if shared.fanout.send(command).await.is_err() {
                            return Err(ConnectionError::ShuttingDown);
                        }
                    }
                    ServerboundPlay::ServerboundClientInformation(packet) => {
                        // Of the settings only the render distance matters to the
                        // server, and a player changes it in play as often as not.
                        let command = Command::ViewDistance {
                            session: client.session,
                            player: client.player,
                            requested: i32::from(packet.information.view_distance),
                        };
                        if shared.fanout.send(command).await.is_err() {
                            return Err(ConnectionError::ShuttingDown);
                        }
                    }
                    ServerboundPlay::ConfirmTeleportation(confirmation) => {
                        // Only the teleport sent last counts; an older id changes nothing.
                        let confirmed = client
                            .awaiting_teleport
                            .compare_exchange(
                                confirmation.teleport_id,
                                NO_TELEPORT,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            )
                            .is_ok();
                        if confirmed {
                            let rotation = confirmed_look(&confirmation)?;
                            if let Some(flags) = flags {
                                moved(shared, client, None, Some(rotation), flags).await?;
                            }
                        }
                    }
                    ServerboundPlay::SetPlayerPosition(packet) => {
                        flags = Some(packet.flags);
                        let position = Vec3::new(packet.x, packet.y, packet.z);
                        moved(shared, client, Some(position), None, packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerPositionAndRotation(packet) => {
                        flags = Some(packet.flags);
                        let position = Vec3::new(packet.x, packet.y, packet.z);
                        let rotation = (packet.yaw, packet.pitch);
                        moved(shared, client, Some(position), Some(rotation), packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerRotation(packet) => {
                        flags = Some(packet.flags);
                        let rotation = (packet.yaw, packet.pitch);
                        moved(shared, client, None, Some(rotation), packet.flags).await?;
                    }
                    ServerboundPlay::SetPlayerMovementFlags(packet) => {
                        flags = Some(packet.flags);
                        moved(shared, client, None, None, packet.flags).await?;
                    }
                    ServerboundPlay::PlayerAction(action) => {
                        acted(shared, client, action, sneaking).await?;
                    }
                    ServerboundPlay::ServerboundPlayerInput(packet) => {
                        sneaking = packet.flags & player_input::SHIFT != 0;
                        let held = |flag| packet.flags & flag != 0;
                        let now = Keys {
                            forward: held(player_input::FORWARD),
                            backward: held(player_input::BACKWARD),
                            left: held(player_input::LEFT),
                            right: held(player_input::RIGHT),
                            jump: held(player_input::JUMP),
                            sneak: sneaking,
                            sprint: keys.sprint,
                        };
                        keys_changed(shared, client, &mut keys, now).await?;
                    }
                    ServerboundPlay::PlayerCommand(packet) => {
                        // Sprinting is what N5 reads of these; the rest are for what
                        // later steps bring (a horse's jump, a chest boat's inventory).
                        let sprint = match packet.action {
                            player_command::START_SPRINTING => true,
                            player_command::STOP_SPRINTING => false,
                            _ => keys.sprint,
                        };
                        let now = Keys { sprint, ..keys };
                        keys_changed(shared, client, &mut keys, now).await?;
                    }
                    ServerboundPlay::Attack(packet) => {
                        on_entity(shared, client, packet.entity_id, OnEntity::Attack).await?;
                    }
                    ServerboundPlay::Interact(packet) => {
                        if !packet.location.iter().all(|value| value.is_finite()) {
                            return Err(ConnectionError::Protocol("a click that is not a number"));
                        }
                        let used = OnEntity::Interact {
                            location: packet.location,
                            sneaking: packet.using_secondary_action,
                            off_hand: packet.hand == 1,
                        };
                        on_entity(shared, client, packet.entity_id, used).await?;
                    }
                    ServerboundPlay::ServerboundMoveVehicle(packet) => {
                        let numbers = [
                            packet.x,
                            packet.y,
                            packet.z,
                            f64::from(packet.yaw),
                            f64::from(packet.pitch),
                        ];
                        if !numbers.iter().all(|value| value.is_finite()) {
                            return Err(ConnectionError::Protocol(
                                "a vehicle's move that is not a number",
                            ));
                        }
                        // As a player's moves: from before a teleport the client has
                        // not confirmed they are out of date.
                        if client.awaiting_teleport.load(Ordering::Relaxed) == NO_TELEPORT {
                            let input = PlayerInput::MoveVehicle(VehicleMove {
                                position: Vec3::new(packet.x, packet.y, packet.z),
                                yaw: packet.yaw,
                                pitch: packet.pitch,
                                on_ground: packet.on_ground,
                            });
                            send_input(shared, client, input).await?;
                        }
                    }
                    ServerboundPlay::PaddleBoat(packet) => {
                        let input = PlayerInput::PaddleBoat {
                            left: packet.left,
                            right: packet.right,
                        };
                        send_input(shared, client, input).await?;
                    }
                    ServerboundPlay::UseItemOn(packet) => {
                        used_item_on(shared, client, packet, sneaking).await?;
                    }
                    ServerboundPlay::ServerboundUseItem(packet) => {
                        used_item(shared, client, packet, sneaking).await?;
                    }
                    ServerboundPlay::SetHeldItem(packet) => {
                        let Some(slot) = hotbar_slot(i32::from(packet.slot)) else {
                            return Err(ConnectionError::Protocol("no such hotbar slot"));
                        };
                        send_input(shared, client, PlayerInput::SelectSlot { slot }).await?;
                    }
                    ServerboundPlay::SetCreativeModeSlot(packet) => {
                        // A slot of the inventory screen sets that slot of the
                        // inventory, and a negative one throws the stack out of the
                        // screen. The crafting grid is the client's own business.
                        let input = if packet.slot < 0 {
                            creative_throw(packet.stack, &mut drops, Instant::now())
                        } else {
                            creative_slot(packet.slot, packet.stack)
                        };
                        if let Some(input) = input {
                            send_input(shared, client, input).await?;
                        }
                    }
                    ServerboundPlay::ServerboundPlayerAbilities(packet) => {
                        // The one thing a client says with it is whether it flies; the
                        // region keeps that, so that whoever comes back comes back as
                        // they left (ADR-0020, section 13).
                        let flying = packet.is_flying();
                        send_input(shared, client, PlayerInput::SetFlying { flying }).await?;
                    }
                    ServerboundPlay::PickItemFromEntity(packet) => {
                        // Answered by the player's own region from the entity as it has
                        // it. `includeData` gives an operator an avatar's profile, which
                        // nothing N5 brings is.
                        on_entity(shared, client, packet.entity_id, OnEntity::Pick).await?;
                    }
                    ServerboundPlay::ClientTickEnd(_)
                    | ServerboundPlay::PlayerLoaded(_)
                    | ServerboundPlay::Unhandled { .. } => {}
                }
            }
            packet = packets.recv() => {
                let Some(packet) = packet else {
                    // The fan-out task dropped the player: it was refused, too slow, or
                    // the server is stopping. What it queued before has been written.
                    connection.close_gracefully().await;
                    return Ok(());
                };
                queue_counted(connection, &packet)?;
                // Write everything that is ready in one go.
                while let Ok(packet) = packets.try_recv() {
                    queue_counted(connection, &packet)?;
                }
                connection.flush().await?;
                in_world = true;
            }
            _ = keep_alive.tick(), if in_world => {
                if awaiting_keep_alive.is_some() {
                    let reason = Nbt::String("Timed out".to_owned());
                    connection.write(&Disconnect { reason }).await?;
                    connection.close_gracefully().await;
                    return Ok(());
                }
                next_keep_alive_id += 1;
                awaiting_keep_alive = Some(next_keep_alive_id);
                connection.write(&ClientboundKeepAlive { id: next_keep_alive_id }).await?;
            }
        }
    }
}

/// Queues an encoded packet and counts what it costs if it carries light
/// (`crate::figures`).
fn queue_counted(connection: &mut Connection, packet: &[u8]) -> Result<(), ConnectionError> {
    // Both ids are below 128, so a packet's first byte is its id.
    const _: () = assert!(LevelChunkWithLight::ID < 128 && LightUpdate::ID < 128);
    let (counter, time) = match packet.first().map(|&id| i32::from(id)) {
        Some(LevelChunkWithLight::ID) => (&FIGURES.chunks, &FIGURES.compress_chunk_nanos),
        Some(LightUpdate::ID) => {
            FIGURES
                .update_bytes
                .fetch_add(packet.len() as u64, std::sync::atomic::Ordering::Relaxed);
            (&FIGURES.updates, &FIGURES.compress_update_nanos)
        }
        _ => return connection.queue_encoded(packet),
    };
    let began = std::time::Instant::now();
    connection.queue_encoded(packet)?;
    figures::add_time(time, began);
    figures::count(counter);
    Ok(())
}

/// Passes a movement of the player on to the region they are in.
async fn moved(
    shared: &Shared,
    client: &Client,
    position: Option<Vec3>,
    rotation: Option<(f32, f32)>,
    flags: u8,
) -> Result<(), ConnectionError> {
    // Like vanilla, a client that sends something that is not a number is cut off.
    let numbers = position
        .iter()
        .flat_map(|position| [position.x, position.y, position.z])
        .chain(
            rotation
                .iter()
                .flat_map(|(yaw, pitch)| [f64::from(*yaw), f64::from(*pitch)]),
        );
    if !numbers.into_iter().all(f64::is_finite) {
        return Err(ConnectionError::Protocol("movement that is not a number"));
    }
    // Positions from before a teleport the client has not confirmed are out of date.
    if client.awaiting_teleport.load(Ordering::Relaxed) != NO_TELEPORT {
        return Ok(());
    }
    let input = PlayerInput::Move {
        position,
        rotation,
        on_ground: flags & movement_flags::ON_GROUND != 0,
    };
    send_input(shared, client, input).await
}

/// The look a client confirms a teleport with, which the game takes as the player's
/// (`handleAcceptTeleportPacket` passes the packet's place and look to
/// `handlePlayerPositionChange`, which for a rider takes the look alone): a client that
/// says it looks one way after being seated drives its vehicle that way. Its place is
/// where the teleport put it, which the region has already. Values that are not numbers
/// cut the client off, as the game's `containsInvalidValues` does. (The game passes the
/// move on as off the ground; here it goes with the flags of the client's last move,
/// which its next move gives anyway, and not at all before the client's first move,
/// whose look the region's teleport placing it gave it: the region is told nothing of
/// the ground the client did not say.)
fn confirmed_look(confirmation: &ConfirmTeleportation) -> Result<(f32, f32), ConnectionError> {
    let numbers = [confirmation.x, confirmation.y, confirmation.z]
        .into_iter()
        .chain([confirmation.yaw, confirmation.pitch].map(f64::from));
    if !numbers.into_iter().all(f64::is_finite) {
        return Err(ConnectionError::Protocol(
            "a teleport confirmed at no number",
        ));
    }
    Ok((confirmation.yaw, confirmation.pitch))
}

/// Handles something the player did with their hands. `sneaking` is whether the sneak
/// key is down, which lowers the eyes a thrown item leaves from.
async fn acted(
    shared: &Shared,
    client: &Client,
    action: PlayerAction,
    sneaking: bool,
) -> Result<(), ConnectionError> {
    match action.status {
        // In creative mode, starting to break a block breaks it.
        player_action::START_DESTROY_BLOCK => {
            let input = PlayerInput::Dig {
                position: BlockPos::new(action.position.x, action.position.y, action.position.z),
                sequence: action.sequence,
            };
            send_input(shared, client, input).await
        }
        // The other ways of breaking a block only exist in survival mode. The client
        // still counts them among its guesses and waits to hear they were handled.
        player_action::ABORT_DESTROY_BLOCK | player_action::STOP_DESTROY_BLOCK => {
            handled(shared, client, action.sequence).await
        }
        // F: the region swaps the hands and tells the client, which does not foresee it.
        player_action::SWAP_ITEM_WITH_OFFHAND => {
            send_input(shared, client, PlayerInput::SwapHands).await
        }
        player_action::DROP_ITEM | player_action::DROP_ALL_ITEMS => {
            let input = thrown_by(action.status, sneaking).expect("a status that throws");
            send_input(shared, client, input).await
        }
        // Using items over time does not exist yet.
        _ => Ok(()),
    }
}

/// Tells the region of the keys the player holds, if they are not what it was last
/// told.
async fn keys_changed(
    shared: &Shared,
    client: &Client,
    keys: &mut Keys,
    now: Keys,
) -> Result<(), ConnectionError> {
    if *keys == now {
        return Ok(());
    }
    *keys = now;
    send_input(shared, client, PlayerInput::Keys(now)).await
}

/// Tells the region what the player did to the entity their client knows by `id`, which
/// the fan-out task, which gave the id, turns into the entity's key.
async fn on_entity(
    shared: &Shared,
    client: &Client,
    id: i32,
    what: OnEntity,
) -> Result<(), ConnectionError> {
    let command = Command::OnEntity {
        session: client.session,
        player: client.player,
        id,
        what,
    };
    shared
        .fanout
        .send(command)
        .await
        .map_err(|_| ConnectionError::ShuttingDown)
}

async fn send_input(
    shared: &Shared,
    client: &Client,
    input: PlayerInput,
) -> Result<(), ConnectionError> {
    // Through the fan-out task, which knows which region the player is in.
    let command = Command::Input {
        session: client.session,
        player: client.player,
        input,
    };
    shared
        .fanout
        .send(command)
        .await
        .map_err(|_| ConnectionError::ShuttingDown)
}

/// Has the client told that its action with this sequence number was handled, in its
/// turn among what the regions report as handled.
async fn handled(shared: &Shared, client: &Client, sequence: i32) -> Result<(), ConnectionError> {
    let command = Command::Handled {
        session: client.session,
        player: client.player,
        sequence,
    };
    shared
        .fanout
        .send(command)
        .await
        .map_err(|_| ConnectionError::ShuttingDown)
}

/// Handles the player using the item in their hand on a block.
async fn used_item_on(
    shared: &Shared,
    client: &Client,
    packet: UseItemOn,
    sneaking: bool,
) -> Result<(), ConnectionError> {
    let face = match u8::try_from(packet.face) {
        Ok(face::BOTTOM) => Some(Face::Bottom),
        Ok(face::TOP) => Some(Face::Top),
        Ok(face::NORTH) => Some(Face::North),
        Ok(face::SOUTH) => Some(Face::South),
        Ok(face::WEST) => Some(Face::West),
        Ok(face::EAST) => Some(Face::East),
        _ => None,
    };
    match face {
        Some(face) => {
            let position = packet.position;
            let input = PlayerInput::UseItemOn {
                position: BlockPos::new(position.x, position.y, position.z),
                face,
                cursor: packet.cursor.map(within_block),
                sneaking,
                sequence: packet.sequence,
            };
            send_input(shared, client, in_hand(packet.hand, input)).await
        }
        // Nothing happens, but the client counts this among its guesses too.
        _ => handled(shared, client, packet.sequence).await,
    }
}

/// Where on a block a click landed, from 0 to 1 as the region takes it: a client
/// that says something else is put on the block, and one that says no number at all
/// in its middle.
fn within_block(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// Handles the player using the item in their hand without a block it uses on.
async fn used_item(
    shared: &Shared,
    client: &Client,
    packet: ServerboundUseItem,
    sneaking: bool,
) -> Result<(), ConnectionError> {
    // A rotation that is no number aims at nothing: only the guess is handled.
    let rotation = if packet.yaw.is_finite() && packet.pitch.is_finite() {
        (packet.yaw, packet.pitch.clamp(-90.0, 90.0))
    } else {
        return handled(shared, client, packet.sequence).await;
    };
    let input = PlayerInput::UseItem {
        rotation,
        sneaking,
        sequence: packet.sequence,
    };
    send_input(shared, client, in_hand(packet.hand, input)).await
}

/// `used` as made with `hand`: a client sends a use with the off hand after the same
/// use with the main hand did nothing, and the region uses what the off hand holds.
fn in_hand(hand: i32, used: PlayerInput) -> PlayerInput {
    if hand == MAIN_HAND {
        used
    } else {
        PlayerInput::WithOffHand(Box::new(used))
    }
}

/// What a creative-mode client's setting of the container slot `slot` of its inventory
/// screen asks of the region: the inventory slot shown there set to `stack`, if it is
/// one. The armour is shown from the head down, so the order of its slots is reversed
/// ([`Inventory::from_container`]).
fn creative_slot(slot: i16, stack: Option<protocol::ItemStack>) -> Option<PlayerInput> {
    let slot = Inventory::from_container(slot)?;
    let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
    let stack = stack.map(|stack| ItemStack {
        item: stack.item,
        count: stack.count,
    });
    Some(PlayerInput::SetInventorySlot { slot, stack })
}

/// What a player's action of `status` throws: Q (`DROP_ITEM`) one of what is held,
/// ctrl+Q (`DROP_ALL_ITEMS`) the stack, from eyes lowered if `sneaking`.
fn thrown_by(status: i32, sneaking: bool) -> Option<PlayerInput> {
    let all = match status {
        player_action::DROP_ITEM => false,
        player_action::DROP_ALL_ITEMS => true,
        _ => return None,
    };
    Some(PlayerInput::Drop {
        all,
        crouching: sneaking,
    })
}

/// The game's `dropSpamThrottler` of a connection: every throw out of the creative
/// inventory adds 20, every tick of the game takes 1 away, and a throw is let through
/// only while the count is below 1480. So 74 throws go at once, and then about one a
/// second.
#[derive(Debug)]
struct DropThrottler {
    count: u32,
    /// The time of the last tick counted.
    ticked: Instant,
}

impl DropThrottler {
    const STEP: u32 = 20;
    const THRESHOLD: u32 = 1480;
    const TICK: Duration = Duration::from_millis(50);

    fn new(now: Instant) -> Self {
        Self {
            count: 0,
            ticked: now,
        }
    }

    /// Whether a throw at `now` is let through, which counts it if it is.
    fn lets_through(&mut self, now: Instant) -> bool {
        let ticks = now.saturating_duration_since(self.ticked).as_millis() / Self::TICK.as_millis();
        let ticks = u32::try_from(ticks).unwrap_or(u32::MAX);
        self.count = self.count.saturating_sub(ticks);
        self.ticked += Self::TICK * ticks;
        if self.count < Self::THRESHOLD {
            self.count += Self::STEP;
            true
        } else {
            false
        }
    }
}

/// What a creative-mode client's throw of `stack` out of its inventory screen (a
/// setting of a negative slot) asks of the region, as the game's
/// `handleSetCreativeModeSlot` takes it: a stack of an item that does not exist, or of
/// more than a stack holds, is passed over; any other throw, an empty one too, counts
/// against the throttler, and what it lets through and is not empty is thrown.
fn creative_throw(
    stack: Option<protocol::ItemStack>,
    throttler: &mut DropThrottler,
    now: Instant,
) -> Option<PlayerInput> {
    // `handleSetCreativeModeSlot`: a stack of an item that exists, within the item's own
    // maximum (`minecraft:max_stack_size`).
    let valid = stack.is_none_or(|stack| {
        usize::try_from(stack.item).is_ok_and(|item| item < ITEMS.len())
            && (1..=i32::from(ItemComponents::of(stack.item).max_stack_size)).contains(&stack.count)
    });
    if !valid {
        return None;
    }
    if !throttler.lets_through(now) {
        warn!("a player was dropping items too fast in creative mode, ignoring");
        return None;
    }
    let stack = stack?;
    Some(PlayerInput::Throw {
        stack: ItemStack {
            item: stack.item,
            count: stack.count,
        },
    })
}

/// `slot` as a hotbar slot, if it is one.
fn hotbar_slot(slot: i32) -> Option<u8> {
    u8::try_from(slot)
        .ok()
        .filter(|slot| usize::from(*slot) < HOTBAR_SLOTS)
}

#[cfg(test)]
mod tests {
    use clustine_data::items;

    use super::*;

    fn confirmation(yaw: f32, pitch: f32, y: f64) -> ConfirmTeleportation {
        ConfirmTeleportation {
            teleport_id: 2,
            x: 21.5,
            y,
            z: 4.5,
            yaw,
            pitch,
        }
    }

    /// The look a client confirms a teleport with is the player's, as the official
    /// server has it: a rider whose client says yaw 0 after being seated drives south
    /// (`minecarts_oracle.rs`, 2026-10-18), and one that says 270 east.
    #[test]
    fn a_confirmed_teleports_look_is_the_players_and_one_at_no_number_cuts_the_client_off() {
        assert_eq!(
            confirmed_look(&confirmation(0.0, 0.0, -60.35)).unwrap(),
            (0.0, 0.0)
        );
        assert_eq!(
            confirmed_look(&confirmation(270.0, 12.5, -60.35)).unwrap(),
            (270.0, 12.5)
        );
        for bad in [
            confirmation(f32::NAN, 0.0, -60.35),
            confirmation(0.0, f32::INFINITY, -60.35),
            confirmation(0.0, 0.0, f64::NAN),
        ] {
            assert!(matches!(
                confirmed_look(&bad),
                Err(ConnectionError::Protocol(_))
            ));
        }
    }

    /// The slot a creative client's setting of `container` asks the region to set,
    /// with a stone in it.
    fn set_by(container: i16) -> Option<u8> {
        let stone = protocol::ItemStack { item: 1, count: 3 };
        match creative_slot(container, Some(stone)) {
            Some(PlayerInput::SetInventorySlot { slot, stack }) => {
                assert_eq!(stack, Some(ItemStack { item: 1, count: 3 }));
                Some(slot)
            }
            None => None,
            Some(other) => panic!("{other:?} is no setting of a slot"),
        }
    }

    #[test]
    fn creative_slots_of_every_part_of_the_screen_set_the_inventory_slot_shown_there() {
        // The armour from the head down: the helmet's container slot 5 is the
        // inventory's slot 39, the boots' 8 its 36.
        assert_eq!(set_by(5), Some(39));
        assert_eq!(set_by(6), Some(38));
        assert_eq!(set_by(7), Some(37));
        assert_eq!(set_by(8), Some(36));
        // The main inventory is numbered alike in both.
        assert_eq!(set_by(9), Some(9));
        assert_eq!(set_by(35), Some(35));
        // The hotbar is at the bottom of the screen and first in the inventory.
        assert_eq!(set_by(36), Some(0));
        assert_eq!(set_by(44), Some(8));
        // The off hand.
        assert_eq!(set_by(45), Some(40));
        // The crafting grid and its result are passed over, and so is what is beyond
        // the screen; a throw out of it, a negative slot, is no setting of a slot.
        for passed_over in [0, 1, 2, 3, 4, -1, -999, 46, i16::MAX] {
            assert_eq!(set_by(passed_over), None, "slot {passed_over}");
        }
        // Emptying a slot empties it.
        assert_eq!(
            creative_slot(45, None),
            Some(PlayerInput::SetInventorySlot {
                slot: 40,
                stack: None
            })
        );
    }

    fn thrown(
        stack: Option<protocol::ItemStack>,
        throttler: &mut DropThrottler,
        now: Instant,
    ) -> bool {
        match creative_throw(stack, throttler, now) {
            Some(PlayerInput::Throw { stack: thrown }) => {
                let stack = stack.expect("only a stack is thrown");
                assert_eq!((thrown.item, thrown.count), (stack.item, stack.count));
                true
            }
            None => false,
            Some(other) => panic!("{other:?} is no throw"),
        }
    }

    #[test]
    fn throws_out_of_the_creative_inventory_go_at_once_up_to_the_game_s_threshold_and_then_one_a_second()
     {
        let start = Instant::now();
        let mut throttler = DropThrottler::new(start);
        let stone = Some(protocol::ItemStack { item: 1, count: 64 });
        // 1480 / 20: the 74 throws that keep the count below the threshold before each.
        for throw in 0..74 {
            assert!(thrown(stone, &mut throttler, start), "throw {throw}");
        }
        assert!(!thrown(stone, &mut throttler, start));
        // A tick takes one away, which lets one more through; then twenty ticks have
        // to take its twenty away before the next.
        let tick = DropThrottler::TICK;
        assert!(thrown(stone, &mut throttler, start + tick));
        assert!(!thrown(stone, &mut throttler, start + tick * 2));
        assert!(!thrown(stone, &mut throttler, start + tick * 20));
        assert!(thrown(stone, &mut throttler, start + tick * 21));
        assert!(!thrown(stone, &mut throttler, start + tick * 21));
        // Time with nobody throwing lets the count down to nothing, and no further.
        let later = start + tick * 10_000;
        for throw in 0..74 {
            assert!(thrown(stone, &mut throttler, later), "throw {throw} later");
        }
        assert!(!thrown(stone, &mut throttler, later));
    }

    #[test]
    fn an_empty_throw_counts_and_a_stack_that_cannot_be_is_passed_over_without_counting() {
        let start = Instant::now();
        let mut throttler = DropThrottler::new(start);
        // Nothing thrown out of the screen throws nothing, and counts as the game's does.
        for _ in 0..74 {
            assert!(!thrown(None, &mut throttler, start));
        }
        assert!(!throttler.lets_through(start));
        let mut throttler = DropThrottler::new(start);
        let too_many = protocol::ItemStack {
            item: items::STONE,
            count: 65,
        };
        let too_many_snowballs = protocol::ItemStack {
            item: items::SNOWBALL,
            count: 17,
        };
        let two_swords = protocol::ItemStack {
            item: items::DIAMOND_SWORD,
            count: 2,
        };
        let unknown = protocol::ItemStack {
            item: i32::MAX,
            count: 1,
        };
        let negative = protocol::ItemStack { item: -1, count: 1 };
        for _ in 0..100 {
            for stack in [too_many, too_many_snowballs, two_swords, unknown, negative] {
                assert!(!thrown(Some(stack), &mut throttler, start));
            }
        }
        // None of those counted.
        assert_eq!(throttler.count, 0);
        // Each item's own maximum: 64 stone, 16 snowballs, one sword.
        for (item, count) in [
            (items::STONE, 64),
            (items::SNOWBALL, 16),
            (items::DIAMOND_SWORD, 1),
        ] {
            let most = protocol::ItemStack { item, count };
            assert!(thrown(Some(most), &mut throttler, start));
        }
    }

    #[test]
    fn q_throws_one_ctrl_q_the_stack_and_no_other_action_throws() {
        // The game's statuses: 5 is `DROP_ITEM`, 4 `DROP_ALL_ITEMS`.
        assert_eq!(
            thrown_by(5, false),
            Some(PlayerInput::Drop {
                all: false,
                crouching: false
            })
        );
        assert_eq!(
            thrown_by(4, true),
            Some(PlayerInput::Drop {
                all: true,
                crouching: true
            })
        );
        for status in [0, 1, 2, 3, 6, 7, 8] {
            assert_eq!(thrown_by(status, false), None, "status {status}");
        }
    }

    #[test]
    fn a_click_lands_on_the_block_whatever_the_client_says() {
        assert_eq!(within_block(0.25), 0.25);
        assert_eq!(within_block(0.0), 0.0);
        assert_eq!(within_block(1.0), 1.0);
        assert_eq!(within_block(-3.0), 0.0);
        assert_eq!(within_block(1.5), 1.0);
        assert_eq!(within_block(f32::NAN), 0.5);
        assert_eq!(within_block(f32::INFINITY), 0.5);
    }
}
