//! A client for N9's comparisons with the official server (`docs/groundwork/n9-plan.md`,
//! section 11) that keeps every packet it is sent and goes on answering as a client
//! does, without ever failing on a packet it cannot read.
//!
//! The bots of `clustine_botswarm` decode every packet with Clustine's own codec and end
//! at one it does not model whole: the official server sends particles of an item (the
//! crumbs of eating) and an area effect cloud's particle (the serializer `PARTICLE`),
//! which Clustine's codec does not read. So a [`Scene`] joins as a bot does, waits for
//! the chunks about it, and then reads the connection itself: it keeps every frame with
//! the time it came, decodes what it can, reads entity data leniently up to the first
//! value it cannot, and answers keep-alives, teleports and chunk batches.
//!
//! Times are a client's: a packet's arrival, and ticks counted as twentieths of a second
//! between two arrivals ([`ticks`]), which is what a client can tell and what the other
//! comparisons hold to (`monsters_oracle.rs`).

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine_botswarm::{Bot, Connection};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::delta::VecDeltaCodec;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::Packet;
use clustine_protocol::packets::play::{
    ChunkBatchReceived, ClientTickEnd, ClientboundPlay, ConfirmTeleportation, ContainerClick,
    EntityDataValue, HashedStack, LevelEvent, PlayerAction, ServerboundContainerClose,
    ServerboundKeepAlive, ServerboundPlayerAbilities, ServerboundUseItem, SetCreativeModeSlot,
    SetHeldItem, SetPlayerPosition, SetPlayerPositionAndRotation, SetPlayerRotation, SpawnEntity,
    UseItemOn, VecDelta, entity_data_serializer, face, movement_flags, player_action, relative,
};
use tokio::time::{Instant, timeout_at};

/// How long a scene waits for what it is told to wait for, unless told otherwise.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// The top of the grass on both servers' flat worlds, which a player stands on at -60.
pub const GROUND: i32 = -61;

/// The hotbar's first slot in the inventory window.
const HOTBAR_START: i16 = 36;

/// The off hand's slot in the inventory window.
pub const OFF_HAND: i16 = 45;

/// One packet as the scene was sent it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub at: Instant,
    pub id: i32,
    /// The packet's id and body; empty for chunks and their light, which are not kept.
    pub frame: Vec<u8>,
    /// The packet as Clustine's codec reads it, where it can.
    pub packet: Option<ClientboundPlay>,
}

/// A window the scene has open.
#[derive(Debug, Clone)]
pub struct Window {
    pub id: i32,
    pub state: i32,
    pub slots: Vec<Option<ItemStack>>,
}

/// A client in a world, as described in the module's comment.
pub struct Scene {
    connection: Connection,
    pub name: String,
    /// The player's own entity id.
    pub me: i32,
    pub feet: (f64, f64, f64),
    pub look: (f32, f32),
    /// The synchronised registries' entry names, an entry's place being its id.
    pub registries: Vec<(String, Vec<String>)>,
    /// The seed's hash the login gave.
    pub hashed_seed: i64,
    /// The entities shown before the scene began keeping packets, by id and kind.
    pub known: Vec<(i32, i32)>,
    pub log: Vec<Seen>,
    /// Blocks the server changed since the join, by place.
    pub blocks: BTreeMap<(i32, i32, i32), i32>,
    pub window: Option<Window>,
    sequence: i32,
    pub acknowledged: i32,
}

impl Scene {
    /// Joins `address` as `name` and waits until the nine chunks about the player have
    /// come, which both servers tick then.
    pub async fn join(address: &str, name: &str) -> Result<Scene> {
        let mut bot = Bot::join(address, name)
            .await
            .with_context(|| format!("{name} joining"))?;
        bot.wait_until(PATIENCE, |bot| {
            let x = (bot.location.0.floor() as i32).div_euclid(16);
            let z = (bot.location.2.floor() as i32).div_euclid(16);
            (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
        })
        .await
        .with_context(|| format!("{name}'s chunks"))?;
        let known = bot
            .entities
            .values()
            .map(|entity| (entity.entity_id, entity.kind))
            .collect();
        let scene = Scene {
            known,
            name: name.to_owned(),
            me: bot.info.login.entity_id,
            feet: bot.location,
            look: bot.look,
            registries: bot.info.registries.clone(),
            hashed_seed: bot.info.login.hashed_seed,
            log: Vec::new(),
            blocks: BTreeMap::new(),
            window: None,
            sequence: bot.acknowledged_sequence.max(0) + 1_000,
            acknowledged: 0,
            connection: bot.into_connection(),
        };
        Ok(scene)
    }

    /// The column the player stands in.
    pub fn column(&self) -> (i32, i32) {
        (self.feet.0.floor() as i32, self.feet.2.floor() as i32)
    }

    /// The id of `entry` in the synchronised registry `registry`.
    pub fn registered(&self, registry: &str, entry: &str) -> Result<i32> {
        let (_, entries) = self
            .registries
            .iter()
            .find(|(name, _)| name == registry)
            .with_context(|| format!("the registry {registry}"))?;
        let at = entries
            .iter()
            .position(|name| name == entry)
            .with_context(|| format!("{entry} in {registry}"))?;
        Ok(i32::try_from(at).expect("a registry is small"))
    }

    /// Reads and keeps the next packet, answering it where a client must. False if
    /// `deadline` passed first.
    async fn pump(&mut self, deadline: Instant) -> Result<bool> {
        let Ok(frame) = timeout_at(deadline, self.connection.read_frame()).await else {
            return Ok(false);
        };
        let frame = frame.with_context(|| format!("{}'s connection", self.name))?;
        let id = Reader::new(&frame).var_int()?;
        let packet = ClientboundPlay::decode(&frame).ok();
        match &packet {
            Some(ClientboundPlay::ClientboundKeepAlive(alive)) => {
                self.connection
                    .write(&ServerboundKeepAlive { id: alive.id })
                    .await?;
            }
            Some(ClientboundPlay::SynchronizePlayerPosition(teleport)) => {
                let flags = teleport.relative_flags;
                let pick = |flag: i32, own: f64, given: f64| {
                    if flags & flag != 0 {
                        own + given
                    } else {
                        given
                    }
                };
                let turn = |flag: i32, own: f32, given: f32| {
                    if flags & flag != 0 {
                        own + given
                    } else {
                        given
                    }
                };
                self.feet = (
                    pick(relative::X, self.feet.0, teleport.x),
                    pick(relative::Y, self.feet.1, teleport.y),
                    pick(relative::Z, self.feet.2, teleport.z),
                );
                self.look = (
                    turn(relative::Y_ROT, self.look.0, teleport.yaw),
                    turn(relative::X_ROT, self.look.1, teleport.pitch),
                );
                self.connection
                    .write(&ConfirmTeleportation {
                        teleport_id: teleport.teleport_id,
                        x: self.feet.0,
                        y: self.feet.1,
                        z: self.feet.2,
                        yaw: self.look.0,
                        pitch: self.look.1,
                    })
                    .await?;
            }
            Some(ClientboundPlay::ChunkBatchFinished(_)) => {
                self.connection
                    .write(&ChunkBatchReceived {
                        chunks_per_tick: 20.0,
                    })
                    .await?;
            }
            Some(ClientboundPlay::AcknowledgeBlockChange(acknowledged)) => {
                self.acknowledged = self.acknowledged.max(acknowledged.sequence);
            }
            Some(ClientboundPlay::BlockUpdate(update)) => {
                let Position { x, y, z } = update.position;
                self.blocks.insert((x, y, z), update.state);
            }
            Some(ClientboundPlay::SectionBlocksUpdate(update)) => {
                let (sx, sy, sz) = update.section;
                for ((x, y, z), state) in &update.blocks {
                    let at = (
                        sx * 16 + i32::from(*x),
                        sy * 16 + i32::from(*y),
                        sz * 16 + i32::from(*z),
                    );
                    self.blocks.insert(at, *state);
                }
            }
            Some(ClientboundPlay::OpenScreen(open)) => {
                self.window = Some(Window {
                    id: open.window_id,
                    state: 0,
                    slots: Vec::new(),
                });
            }
            Some(ClientboundPlay::SetContainerContent(content)) => {
                if let Some(window) = &mut self.window
                    && window.id == content.window_id
                {
                    window.state = content.state_id;
                    window.slots = content.slots.clone();
                }
            }
            Some(ClientboundPlay::ContainerSetSlot(set)) => {
                if let Some(window) = &mut self.window
                    && window.id == set.window_id
                {
                    window.state = set.state_id;
                    if let Some(slot) = usize::try_from(set.slot)
                        .ok()
                        .and_then(|slot| window.slots.get_mut(slot))
                    {
                        *slot = set.stack.clone();
                    }
                }
            }
            Some(ClientboundPlay::ClientboundContainerClose(_)) => self.window = None,
            Some(ClientboundPlay::Disconnect(_)) => bail!("{} was disconnected", self.name),
            _ => {}
        }
        let kept = !matches!(
            id,
            clientbound::LEVEL_CHUNK_WITH_LIGHT | clientbound::LIGHT_UPDATE
        );
        self.log.push(Seen {
            at: Instant::now(),
            id,
            frame: if kept { frame.to_vec() } else { Vec::new() },
            packet: if kept { packet } else { None },
        });
        Ok(true)
    }

    /// Reads on until `done` holds, at most for `patience`.
    pub async fn until(
        &mut self,
        patience: Duration,
        what: &str,
        mut done: impl FnMut(&Scene) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + patience;
        while !done(self) {
            if !self.pump(deadline).await? {
                bail!("{}: no {what} within {patience:?}", self.name);
            }
        }
        Ok(())
    }

    /// Reads on for `duration`: for what a scene does over time (a bow drawn), never to
    /// wait for the server.
    pub async fn idle(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        while self.pump(deadline).await? {}
        Ok(())
    }

    /// Reads on for `ticks` of a client's clock.
    pub async fn idle_ticks(&mut self, ticks: u64) -> Result<()> {
        self.idle(Duration::from_millis(50 * ticks)).await
    }

    async fn send<P: Packet>(&mut self, packet: &P) -> Result<()> {
        self.connection.write(packet).await
    }

    fn next_sequence(&mut self) -> i32 {
        self.sequence += 1;
        self.sequence
    }

    /// Waits until the server has acknowledged the sequence `sequence`.
    pub async fn acknowledged(&mut self, sequence: i32) -> Result<()> {
        self.until(PATIENCE, "acknowledgement", |scene| {
            scene.acknowledged >= sequence
        })
        .await
    }

    /// Waits until the server has handled everything sent before, and the tick that
    /// handled it has been sent: a creative break of the air above the head, which
    /// changes nothing and is acknowledged, twice.
    pub async fn round_trip(&mut self) -> Result<()> {
        for _ in 0..2 {
            let (x, z) = self.column();
            let y = self.feet.1.floor() as i32 + 3;
            let sequence = self.dig(x, y, z).await?;
            self.acknowledged(sequence).await?;
        }
        Ok(())
    }

    /// Puts `stack` into the inventory window's slot `slot` as a creative client does.
    pub async fn put_slot(&mut self, slot: i16, stack: Option<ItemStack>) -> Result<()> {
        self.send(&SetCreativeModeSlot { slot, stack }).await
    }

    /// Puts `stack` into the hotbar's slot `slot`, 0 to 8.
    pub async fn put(&mut self, slot: i16, stack: ItemStack) -> Result<()> {
        self.put_slot(HOTBAR_START + slot, Some(stack)).await
    }

    /// Empties the whole hotbar, which each server fills its own way for a new player.
    pub async fn empty_hotbar(&mut self) -> Result<()> {
        for slot in 0..9 {
            self.put_slot(HOTBAR_START + slot, None).await?;
        }
        Ok(())
    }

    /// Drops `stack` as a creative client does from its inventory screen (slot -1).
    pub async fn drop_creative(&mut self, stack: ItemStack) -> Result<()> {
        self.put_slot(-1, Some(stack)).await
    }

    pub async fn hold(&mut self, slot: i16) -> Result<()> {
        self.send(&SetHeldItem { slot }).await
    }

    /// Puts `stack` in the first hotbar slot and holds it.
    pub async fn take(&mut self, stack: ItemStack) -> Result<()> {
        self.put(0, stack).await?;
        self.hold(0).await
    }

    /// Turns to `yaw` and `pitch`, in degrees.
    pub async fn turn(&mut self, yaw: f32, pitch: f32) -> Result<()> {
        self.look = (yaw, pitch);
        self.send(&SetPlayerRotation {
            yaw,
            pitch,
            flags: movement_flags::ON_GROUND,
        })
        .await?;
        self.send(&ClientTickEnd).await
    }

    /// Says in one step that the player stands at `x`, `y`, `z`.
    pub async fn stand_at(&mut self, x: f64, y: f64, z: f64) -> Result<()> {
        self.feet = (x, y, z);
        self.send(&SetPlayerPosition {
            x,
            y,
            z,
            flags: movement_flags::ON_GROUND,
        })
        .await?;
        self.send(&ClientTickEnd).await
    }

    /// Flies to `to` through the air, half a block a tick, as a creative client does:
    /// for work above a pit that is out of reach from the ground.
    pub async fn hover_at(&mut self, to: (f64, f64, f64)) -> Result<()> {
        self.send(&ServerboundPlayerAbilities::flying(true)).await?;
        loop {
            let (dx, dy, dz) = (to.0 - self.feet.0, to.1 - self.feet.1, to.2 - self.feet.2);
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            let arrives = distance <= 0.5;
            self.feet = if arrives {
                to
            } else {
                let share = 0.5 / distance;
                (
                    self.feet.0 + dx * share,
                    self.feet.1 + dy * share,
                    self.feet.2 + dz * share,
                )
            };
            self.send(&SetPlayerPositionAndRotation {
                x: self.feet.0,
                y: self.feet.1,
                z: self.feet.2,
                yaw: self.look.0,
                pitch: self.look.1,
                flags: 0,
            })
            .await?;
            self.send(&ClientTickEnd).await?;
            self.idle_ticks(1).await?;
            if arrives {
                return Ok(());
            }
        }
    }

    /// The yaw and pitch from the eyes to `target`.
    pub fn aim_at(&self, target: (f64, f64, f64)) -> (f32, f32) {
        let eyes = self.feet.1 + 1.62;
        let (dx, dy, dz) = (
            target.0 - self.feet.0,
            target.1 - eyes,
            target.2 - self.feet.2,
        );
        let yaw = (-dx).atan2(dz).to_degrees() as f32;
        let pitch = (-dy).atan2(dx.hypot(dz)).to_degrees() as f32;
        (yaw, pitch)
    }

    /// Pours a water bucket onto the top of `floor` as a client does: a click on that
    /// top, which the game passes over for a bucket, then the bucket's use looking at
    /// its middle, which the game aims by a ray from the eyes (`BucketItem.use`); and
    /// waits for the water above it.
    pub async fn pour(&mut self, floor: (i32, i32, i32)) -> Result<()> {
        self.take(ItemStack::new(clustine_data::items::WATER_BUCKET, 1))
            .await?;
        let target = (
            f64::from(floor.0) + 0.5,
            f64::from(floor.1) + 1.0,
            f64::from(floor.2) + 0.5,
        );
        let (yaw, pitch) = self.aim_at(target);
        self.use_on(floor.0, floor.1, floor.2, face::TOP).await?;
        self.use_item(yaw, pitch).await?;
        let above = (floor.0, floor.1 + 1, floor.2);
        self.until(PATIENCE, "the water poured", |scene| {
            scene.block(above).is_some_and(|state| state != 0)
        })
        .await
    }

    /// Digs the columns `cells` down `depth` blocks from the grass, from above their
    /// middle, and waits until each block is air.
    pub async fn dig_pit(&mut self, cells: &[(i32, i32)], depth: i32) -> Result<()> {
        let count = cells.len() as f64;
        let middle = (
            cells.iter().map(|(x, _)| f64::from(*x) + 0.5).sum::<f64>() / count,
            cells.iter().map(|(_, z)| f64::from(*z) + 0.5).sum::<f64>() / count,
        );
        self.hover_at((middle.0, f64::from(GROUND) + 2.0, middle.1))
            .await?;
        for &(x, z) in cells {
            for down in 0..depth {
                self.break_block((x, GROUND - down, z)).await?;
            }
        }
        Ok(())
    }

    /// Uses the held item looking along `yaw` and `pitch`.
    pub async fn use_item(&mut self, yaw: f32, pitch: f32) -> Result<i32> {
        self.look = (yaw, pitch);
        let sequence = self.next_sequence();
        self.send(&ServerboundUseItem {
            hand: 0,
            sequence,
            yaw,
            pitch,
        })
        .await?;
        Ok(sequence)
    }

    /// Uses the held item on the side `side` of the block at `x`, `y`, `z`.
    pub async fn use_on(&mut self, x: i32, y: i32, z: i32, side: u8) -> Result<i32> {
        let sequence = self.next_sequence();
        self.send(&UseItemOn {
            hand: 0,
            position: Position { x, y, z },
            face: side.into(),
            cursor: [0.5, 0.5, 0.5],
            inside_block: false,
            world_border_hit: false,
            sequence,
        })
        .await?;
        Ok(sequence)
    }

    /// Lets go of the use key (`RELEASE_USE_ITEM`).
    pub async fn release(&mut self) -> Result<()> {
        self.send(&PlayerAction {
            status: player_action::RELEASE_USE_ITEM,
            position: Position { x: 0, y: 0, z: 0 },
            face: face::BOTTOM,
            sequence: 0,
        })
        .await
    }

    /// Breaks the block at `x`, `y`, `z` as a creative client does.
    pub async fn dig(&mut self, x: i32, y: i32, z: i32) -> Result<i32> {
        let sequence = self.next_sequence();
        self.send(&PlayerAction {
            status: player_action::START_DESTROY_BLOCK,
            position: Position { x, y, z },
            face: face::TOP,
            sequence,
        })
        .await?;
        Ok(sequence)
    }

    /// The block at `at` as the server last changed it since the join; `None` where it
    /// changed nothing there.
    pub fn block(&self, at: (i32, i32, i32)) -> Option<i32> {
        self.blocks.get(&at).copied()
    }

    /// Places the block of `item` against the side `side` of the block `against`, and
    /// waits until the server has put something at `at`.
    pub async fn place(
        &mut self,
        item: i32,
        against: (i32, i32, i32),
        side: u8,
        at: (i32, i32, i32),
    ) -> Result<()> {
        self.take(ItemStack::new(item, 1)).await?;
        self.use_on(against.0, against.1, against.2, side).await?;
        self.until(PATIENCE, "the block placed", |scene| {
            scene.block(at).is_some_and(|state| state != 0)
        })
        .await
    }

    /// Breaks the block at `at` and waits until it is air.
    pub async fn break_block(&mut self, at: (i32, i32, i32)) -> Result<()> {
        self.dig(at.0, at.1, at.2).await?;
        self.until(PATIENCE, "the block broken", |scene| {
            scene.block(at) == Some(0)
        })
        .await
    }

    /// Opens the block at `at` with an empty hand and waits for its window.
    pub async fn open(&mut self, at: (i32, i32, i32), empty_slot: i16) -> Result<()> {
        self.put_slot(HOTBAR_START + empty_slot, None).await?;
        self.hold(empty_slot).await?;
        self.window = None;
        self.use_on(at.0, at.1, at.2, face::TOP).await?;
        self.until(PATIENCE, "a window", |scene| {
            scene
                .window
                .as_ref()
                .is_some_and(|window| !window.slots.is_empty())
        })
        .await
    }

    /// Clicks the slot `slot` of the window open, saying what the client guesses.
    pub async fn click(
        &mut self,
        slot: i16,
        button: i8,
        input: i32,
        changed: Vec<(i16, Option<HashedStack>)>,
        carried: Option<HashedStack>,
    ) -> Result<()> {
        let window = self.window.as_ref().context("no window is open")?;
        let packet = ContainerClick {
            window_id: window.id,
            state_id: window.state,
            slot,
            button,
            input,
            changed,
            carried,
        };
        self.send(&packet).await
    }

    pub async fn close_window(&mut self) -> Result<()> {
        if let Some(window) = self.window.take() {
            self.send(&ServerboundContainerClose {
                window_id: window.id,
            })
            .await?;
        }
        Ok(())
    }

    /// Where the log ends now, to look only at what comes after.
    pub fn mark(&self) -> usize {
        self.log.len()
    }

    /// The packets read, from `from` on.
    pub fn since(&self, from: usize) -> &[Seen] {
        &self.log[from.min(self.log.len())..]
    }

    /// The entities added from `from` on, with when.
    pub fn added(&self, from: usize) -> Vec<(Instant, SpawnEntity)> {
        self.since(from)
            .iter()
            .filter_map(|seen| match &seen.packet {
                Some(ClientboundPlay::SpawnEntity(spawn)) => Some((seen.at, spawn.clone())),
                _ => None,
            })
            .collect()
    }

    /// The entities of `kind` added from `from` on.
    pub fn added_of(&self, from: usize, kind: i32) -> Vec<(Instant, SpawnEntity)> {
        self.added(from)
            .into_iter()
            .filter(|(_, spawn)| spawn.kind == kind)
            .collect()
    }

    /// Waits for `count` entities of `kind` added from `from` on, and returns them.
    pub async fn wait_added(
        &mut self,
        from: usize,
        kind: i32,
        count: usize,
        patience: Duration,
    ) -> Result<Vec<(Instant, SpawnEntity)>> {
        let name = clustine_data::ENTITY_TYPES[kind as usize];
        self.until(patience, &format!("{count} of {name}"), |scene| {
            scene.added_of(from, kind).len() >= count
        })
        .await?;
        Ok(self.added_of(from, kind))
    }

    /// The entities of `kind` shown so far, whenever they were, by id.
    pub fn shown_of(&self, kind: i32) -> Vec<i32> {
        let mut shown: Vec<i32> = self
            .known
            .iter()
            .filter(|(_, known)| *known == kind)
            .map(|(id, _)| *id)
            .collect();
        shown.extend(
            self.added_of(0, kind)
                .into_iter()
                .map(|(_, spawn)| spawn.entity_id),
        );
        shown
    }

    /// Everything shown of the entity `id`.
    pub fn track(&self, id: i32) -> Track {
        track(&self.log, id)
    }

    /// The sounds heard from `from` on, at a place or at an entity: when, the sound
    /// and the entity it was at.
    pub fn sounds(&self, from: usize) -> Vec<(Instant, i32, Option<i32>)> {
        self.since(from)
            .iter()
            .filter_map(|seen| match &seen.packet {
                Some(ClientboundPlay::Sound(sound)) => Some((seen.at, sound.sound, None)),
                Some(ClientboundPlay::SoundEntity(sound)) => {
                    Some((seen.at, sound.sound, Some(sound.entity_id)))
                }
                _ => None,
            })
            .collect()
    }

    /// The level events from `from` on.
    pub fn level_events(&self, from: usize) -> Vec<(Instant, LevelEvent)> {
        self.since(from)
            .iter()
            .filter_map(|seen| match &seen.packet {
                Some(ClientboundPlay::LevelEvent(event)) => Some((seen.at, event.clone())),
                _ => None,
            })
            .collect()
    }

    /// The particle packets from `from` on, whatever their particle, as the particle's
    /// type id and when.
    pub fn particles(&self, from: usize) -> Vec<(Instant, i32)> {
        self.since(from)
            .iter()
            .filter(|seen| seen.id == clientbound::LEVEL_PARTICLES)
            .filter_map(|seen| {
                let mut reader = Reader::new(&seen.frame);
                reader.var_int().ok()?;
                Some((seen.at, reader.var_int().ok()?))
            })
            .collect()
    }

    /// The packets of `filter` from `from` on.
    pub fn of<T>(
        &self,
        from: usize,
        filter: impl Fn(&ClientboundPlay) -> Option<T>,
    ) -> Vec<(Instant, T)> {
        self.since(from)
            .iter()
            .filter_map(|seen| Some((seen.at, filter(seen.packet.as_ref()?)?)))
            .collect()
    }
}

/// Runs `run` against the official server and then against Clustine, each on a fresh
/// flat world, and gives what each did, the official server's first.
pub async fn on_both<T>(run: impl AsyncFn(&str) -> Result<T>) -> (Result<T>, Result<T>) {
    let oracle = clustine_botswarm::Oracle::start(false)
        .await
        .expect("the official server starts");
    let theirs = run(oracle.address()).await;
    drop(oracle);
    let (server, address) = crate::common::start().await;
    let ours = run(&address).await;
    server.stop().await;
    (theirs, ours)
}

/// The ticks, rounded, from `from` to `to`, as a client's clock tells them.
pub fn ticks(from: Instant, to: Instant) -> i64 {
    let millis = i64::try_from(to.duration_since(from).as_millis()).unwrap_or(i64::MAX);
    let back = i64::try_from(from.duration_since(to).as_millis()).unwrap_or(i64::MAX);
    (millis - back + 25).div_euclid(50)
}

/// Everything shown of one entity.
#[derive(Debug, Clone)]
pub struct Track {
    pub id: i32,
    pub add: Option<(Instant, SpawnEntity)>,
    /// Its place after each packet that moved it, and whether the packet gave it whole
    /// (an add or a sync) rather than as a step.
    pub places: Vec<(Instant, [f64; 3], bool)>,
    /// Its yaw in degrees after each packet that turned it.
    pub yaws: Vec<(Instant, f32)>,
    pub motions: Vec<(Instant, [f64; 3])>,
    /// Its data, each packet's values as far as they could be read.
    pub data: Vec<(Instant, Vec<(u8, EntityDataValue)>)>,
    pub events: Vec<(Instant, i8)>,
    pub removed: Option<Instant>,
}

impl Track {
    /// The first time the value at `index` was sent as `wanted` says.
    pub fn first_data(
        &self,
        index: u8,
        wanted: impl Fn(&EntityDataValue) -> bool,
    ) -> Option<Instant> {
        self.data.iter().find_map(|(at, values)| {
            values
                .iter()
                .any(|(at_index, value)| *at_index == index && wanted(value))
                .then_some(*at)
        })
    }

    /// The first time the entity event `event` came.
    pub fn first_event(&self, event: i8) -> Option<Instant> {
        self.events
            .iter()
            .find_map(|(at, seen)| (*seen == event).then_some(*at))
    }
}

fn track(log: &[Seen], id: i32) -> Track {
    let mut track = Track {
        id,
        add: None,
        places: Vec::new(),
        yaws: Vec::new(),
        motions: Vec::new(),
        data: Vec::new(),
        events: Vec::new(),
        removed: None,
    };
    let mut base = [0.0; 3];
    for seen in log {
        let at = seen.at;
        if seen.id == clientbound::SET_ENTITY_DATA {
            if let Some((entity, values)) = lenient_data(&seen.frame)
                && entity == id
            {
                track.data.push((at, values));
            }
            continue;
        }
        let Some(packet) = &seen.packet else {
            continue;
        };
        match packet {
            ClientboundPlay::SpawnEntity(spawn) if spawn.entity_id == id => {
                base = [spawn.x, spawn.y, spawn.z];
                track.add = Some((at, spawn.clone()));
                track.places.push((at, base, true));
                track.yaws.push((at, f32::from(spawn.yaw) * 360.0 / 256.0));
                track.motions.push((at, spawn.velocity));
            }
            ClientboundPlay::SyncEntityPosition(sync) if sync.entity_id == id => {
                if let Some((x, y, z)) = sync.path.end() {
                    base = [x, y, z];
                    track.places.push((at, base, true));
                }
                track.yaws.push((at, sync.yaw));
            }
            ClientboundPlay::MoveEntityPos(moved) if moved.entity_id == id => {
                base = moved_by(base, &moved.delta);
                track.places.push((at, base, false));
            }
            ClientboundPlay::MoveEntityPosRot(moved) if moved.entity_id == id => {
                base = moved_by(base, &moved.delta);
                track.places.push((at, base, false));
                track.yaws.push((at, unpack_degrees(moved.yaw)));
            }
            ClientboundPlay::MoveEntityRot(turned) if turned.entity_id == id => {
                track.yaws.push((at, unpack_degrees(turned.yaw)));
            }
            ClientboundPlay::SetEntityMotion(motion) if motion.entity_id == id => {
                track.motions.push((at, motion.motion));
            }
            ClientboundPlay::EntityEvent(event) if event.entity_id == id => {
                track.events.push((at, event.event));
            }
            ClientboundPlay::RemoveEntities(removed) if removed.entity_ids.contains(&id) => {
                track.removed.get_or_insert(at);
            }
            _ => {}
        }
    }
    track
}

/// An angle of a move packet, a byte of 1/256 turns, in degrees.
pub fn unpack_degrees(packed: i8) -> f32 {
    f32::from(packed) * 360.0 / 256.0
}

fn moved_by(base: [f64; 3], delta: &VecDelta) -> [f64; 3] {
    let delta = match delta {
        VecDelta::Linear(delta) => *delta,
        VecDelta::Stepped(steps) => match steps.last() {
            Some((_, delta)) => *delta,
            None => return base,
        },
    };
    VecDeltaCodec::new(base).decode(delta)
}

/// The entity and the values of a `set_entity_data` frame, read up to the first value
/// whose serializer this does not know (the cloud's `PARTICLE`, and the like): what
/// comes before it is all a test needs of such a packet.
pub fn lenient_data(frame: &[u8]) -> Option<(i32, Vec<(u8, EntityDataValue)>)> {
    if let Ok(ClientboundPlay::SetEntityData(data)) = ClientboundPlay::decode(frame) {
        return Some((data.entity_id, data.values));
    }
    let mut reader = Reader::new(frame);
    reader.var_int().ok()?;
    let entity = reader.var_int().ok()?;
    let mut values = Vec::new();
    while let Ok(index) = reader.u8() {
        if index == 0xFF {
            break;
        }
        let Ok(serializer) = reader.var_int() else {
            break;
        };
        let value = match serializer {
            entity_data_serializer::BYTE => reader.u8().ok().map(EntityDataValue::Byte),
            entity_data_serializer::INT => reader.var_int().ok().map(EntityDataValue::Int),
            entity_data_serializer::LONG => reader.var_long().ok().map(EntityDataValue::Long),
            entity_data_serializer::FLOAT => reader.f32().ok().map(EntityDataValue::Float),
            entity_data_serializer::BOOLEAN => reader.bool().ok().map(EntityDataValue::Boolean),
            entity_data_serializer::ITEM_STACK => {
                reader.item_stack().ok().map(EntityDataValue::ItemStack)
            }
            entity_data_serializer::OPTIONAL_UNSIGNED_INT => reader.var_int().ok().map(|raw| {
                EntityDataValue::OptionalUnsignedInt(
                    u32::from_ne_bytes(raw.to_ne_bytes()).checked_sub(1),
                )
            }),
            _ => None,
        };
        let Some(value) = value else {
            break;
        };
        values.push((index, value));
    }
    Some((entity, values))
}

/// The bytes of a `set_entity_data` frame after the entity's id: the values as sent,
/// which two servers that number entities differently send alike.
pub fn data_bytes(frame: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::new(frame);
    reader.var_int().ok()?;
    reader.var_int().ok()?;
    Some(reader.bytes(reader.remaining()).ok()?.to_vec())
}

/// The length of a motion, in blocks a tick.
pub fn speed(motion: [f64; 3]) -> f64 {
    (motion[0] * motion[0] + motion[1] * motion[1] + motion[2] * motion[2]).sqrt()
}

/// The yaw, in the game's degrees (0 south, 90 west), a motion points along.
pub fn yaw_of(motion: [f64; 3]) -> f64 {
    (-motion[0]).atan2(motion[2]).to_degrees()
}

/// The difference `a - b` of two angles in degrees, from -180 to 180.
pub fn angle_between(a: f64, b: f64) -> f64 {
    (a - b + 540.0).rem_euclid(360.0) - 180.0
}

/// The flags of an arrow (index 8): crit.
pub const ARROW_CRIT: u8 = 0x01;

/// The value at `index` among `values`.
pub fn value_at(values: &[(u8, EntityDataValue)], index: u8) -> Option<&EntityDataValue> {
    values
        .iter()
        .find_map(|(at, value)| (*at == index).then_some(value))
}

/// How a projectile's flight steps, as a kind's tick has it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flight {
    /// An arrow or a trident (`AbstractArrow.tick`): the move, then the drag and the
    /// gravity.
    Arrow { gravity: f64 },
    /// A thrown item (`ThrowableProjectile.tick`): the gravity and the drag, then the
    /// move.
    Thrown { gravity: f64 },
}

impl Flight {
    /// The places a projectile shown at `place` moving by `motion` passes through in
    /// the air, tick by tick, `ticks` of them, the first being `place` itself.
    pub fn path(self, place: [f64; 3], motion: [f64; 3], ticks: usize) -> Vec<[f64; 3]> {
        let (mut place, mut motion) = (place, motion);
        let mut path = vec![place];
        for _ in 1..ticks {
            match self {
                Flight::Arrow { gravity } => {
                    for axis in 0..3 {
                        place[axis] += motion[axis];
                    }
                    motion = [
                        motion[0] * 0.99,
                        motion[1] * 0.99 - gravity,
                        motion[2] * 0.99,
                    ];
                }
                Flight::Thrown { gravity } => {
                    motion = [
                        motion[0] * 0.99,
                        (motion[1] - gravity) * 0.99,
                        motion[2] * 0.99,
                    ];
                    for axis in 0..3 {
                        place[axis] += motion[axis];
                    }
                }
            }
            path.push(place);
        }
        path
    }
}

/// How far each place a projectile was shown at in the air lies from the path its rule
/// gives from its add packet: for each place shown after the add and before
/// `landed`, the distance to the nearest point of the path. The tick of a place is
/// not taken from the clock, which cannot tell it to the tick, but found as the
/// nearest; a wrong gravity or drag bends the path away from the places within a few
/// ticks.
pub fn off_the_path(track: &Track, flight: Flight, landed: Option<Instant>) -> Vec<f64> {
    let Some((_, add)) = &track.add else {
        return Vec::new();
    };
    let path = flight.path([add.x, add.y, add.z], add.velocity, 200);
    track
        .places
        .iter()
        .skip(1)
        .filter(|(at, _, _)| landed.is_none_or(|landed| *at < landed))
        .map(|(_, place, _)| {
            path.iter()
                .map(|point| {
                    let d: f64 = (0..3).map(|axis| (point[axis] - place[axis]).powi(2)).sum();
                    d.sqrt()
                })
                .fold(f64::INFINITY, f64::min)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_between_wrap_about_a_turn() {
        assert_eq!(angle_between(170.0, -170.0), -20.0);
        assert_eq!(angle_between(-170.0, 170.0), 20.0);
        assert_eq!(angle_between(10.0, 10.0), 0.0);
    }

    #[test]
    fn a_motion_south_is_yaw_zero_and_west_is_ninety() {
        assert!(yaw_of([0.0, 0.0, 1.0]).abs() < 1e-9);
        assert!((yaw_of([-1.0, 0.0, 0.0]) - 90.0).abs() < 1e-9);
        assert!((yaw_of([1.0, 0.0, 0.0]) + 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_thrown_items_path_falls_by_its_gravity_before_it_moves() {
        let path = Flight::Thrown { gravity: 0.03 }.path([0.0; 3], [1.0, 0.0, 0.0], 2);
        assert!((path[1][0] - 0.99).abs() < 1e-12);
        assert!((path[1][1] + 0.03 * 0.99).abs() < 1e-12);
    }

    #[test]
    fn an_arrows_path_moves_before_it_falls() {
        let path = Flight::Arrow { gravity: 0.05 }.path([0.0; 3], [1.0, 0.0, 0.0], 3);
        assert_eq!(path[1], [1.0, 0.0, 0.0]);
        assert!((path[2][1] + 0.05).abs() < 1e-12);
    }

    #[test]
    fn data_with_a_value_it_cannot_read_is_read_up_to_it() {
        // set_entity_data of entity 7: index 8 a float 3.0, index 9 a boolean, index
        // 10 of a serializer no codec knows (120), which is not read.
        let mut frame = vec![clientbound::SET_ENTITY_DATA as u8, 7, 8, 3];
        frame.extend(3.0f32.to_be_bytes());
        frame.extend([9, 8, 1, 10, 120, 0, 0xFF]);
        let (entity, values) = lenient_data(&frame).unwrap();
        assert_eq!(entity, 7);
        assert_eq!(
            values,
            [
                (8, EntityDataValue::Float(3.0)),
                (9, EntityDataValue::Boolean(true))
            ]
        );
    }

    #[test]
    fn a_clients_ticks_round_to_the_nearest() {
        let now = Instant::now();
        assert_eq!(ticks(now, now + Duration::from_millis(74)), 1);
        assert_eq!(ticks(now, now + Duration::from_millis(76)), 2);
        assert_eq!(ticks(now + Duration::from_millis(100), now), -2);
    }
}
