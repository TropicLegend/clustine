//! Sleep, the bed and the respawn point (`docs/groundwork/n7d-plan.md`, the game's rules
//! in `docs/groundwork/n7d-brief.md`, cited as B): a bed used (`useWithoutItem`,
//! `startSleepInBed`), lying in it and getting up (`stopSleepInBed`), and the night
//! skipped when enough of the world's players sleep deeply, which a region decides for
//! the world by the store's list of the world's players and moves the world's clock for.
//!
//! The overworld's bed rules are constant (B 1.3): a bed sleeps when dark and always sets
//! the spawn; a straw bed sleeps when dark, never sets the spawn and breaks when its
//! sleeper leaves it. Dark is the day timeline's (`crate::time`).
//!
//! A villager sleeps in beds too (`mobs/brain/behaviours/rest.rs`); a player's use of a
//! bed a villager sleeps in gets it up and lays nobody down (`kickVillagerOutOfBed`,
//! `docs/groundwork/n10-brief.md` section 3.4).
//!
//! Named differences: monsters do not keep anyone awake until N9 (every player is
//! creative anyway); a bed is slept in only by a player whose region holds its head's
//! chunk, and one used across a line does nothing; another region's sleepers count a
//! second late, and a region announces the night's state to its own players only; the
//! sleep counter's run from 100 to 110 after a wake (the client's fade) is not kept;
//! statistics, the advancement and phantoms are not kept; a player riding does not lie
//! down; a villager a player's use wakes gets up at its own tick, not in the use (in the
//! same tick, as the players' inputs come before the mobs), and only a villager of the
//! region's own land is woken.

use clustine_blocks::data::{facing, flag, is_class, set_flag};
use clustine_data::{BlockClass, BlockState, Face, blocks, entity_types, sounds};
use clustine_world::{BlockPos, ChunkPos, PlayerId, Vec3};

use super::Region;
use super::cushions::is_suffocating;
use crate::api::{PlayerEvent, RegionEvent, Respawn, Sleep, TickOutput};
use crate::mobs::Mob;
use crate::mobs::brain::behaviours::rest;
use crate::mobs::types::villager;
use crate::physics::aabb::Aabb;
use crate::time::{DAY, is_dark};

/// The game rule `players_sleeping_percentage` (B 4.1): Clustine has no game rules yet.
const SLEEPING_PERCENTAGE: u32 = 100;

/// A sleeper is deep after this many ticks in bed (`isSleepingLongEnough`, B 3.1).
const DEEP: u64 = 100;

/// The way one step in a horizontal direction goes.
fn step_of(direction: Face) -> (i32, i32) {
    match direction {
        Face::North => (0, -1),
        Face::South => (0, 1),
        Face::West => (-1, 0),
        _ => (1, 0),
    }
}

fn offset(pos: BlockPos, (dx, dz): (i32, i32), times: i32) -> BlockPos {
    BlockPos::new(pos.x + dx * times, pos.y, pos.z + dz * times)
}

/// The height a sleeper lies at above a bed's block (`getSleepHeight`, B 1.1).
fn sleep_height(straw: bool) -> f64 {
    if straw { 0.25 } else { 0.5625 }
}

impl Region {
    /// The game time of the clock as the last tick left it, which what is decided between
    /// two ticks reads (B 1.4: `skyDarken` is the last level tick's).
    fn last_day(&self) -> u64 {
        self.sleep.last_day
    }

    /// A message to the region's player `id`, on their action bar or in their chat.
    fn say(&self, id: PlayerId, key: &str, args: Vec<i32>, overlay: bool, output: &mut TickOutput) {
        output.player_events.push((
            id,
            PlayerEvent::Message {
                key: key.to_owned(),
                args,
                overlay,
            },
        ));
    }

    /// `AbstractBedBlock.useWithoutItem` of the region's player `id` on the bed half at
    /// `pos` (B 1.2).
    pub(crate) fn use_bed(&mut self, pos: BlockPos, id: PlayerId, output: &mut TickOutput) {
        if !self.players.contains_key(&id) {
            // Used across a line by another region's player: the holder cannot lay them
            // down (named).
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let Some(toward) = facing(state).map(step_of) else {
            return;
        };
        let foot = state.property("part") == Some("foot");
        // The other half: towards the head from the foot, back from the head.
        let other = if foot {
            offset(pos, toward, 1)
        } else {
            offset(pos, toward, -1)
        };
        let Some(other_state) = self.own_block(other) else {
            return;
        };
        if !clustine_blocks::data::same_block(state, other_state)
            || other_state.property("part") == state.property("part")
        {
            return;
        }
        let (head, head_state) = if foot {
            (other, other_state)
        } else {
            (pos, state)
        };
        if flag(head_state, "occupied") {
            if !self.kick_villager_out_of_bed(head, output) {
                self.say(id, "block.minecraft.bed.occupied", Vec::new(), true, output);
            }
            return;
        }
        self.start_sleep(id, head, head_state, toward, output);
    }

    /// `ServerPlayer.startSleepInBed` (B 1.5) on the bed whose head is `head`.
    fn start_sleep(
        &mut self,
        id: PlayerId,
        head: BlockPos,
        head_state: BlockState,
        toward: (i32, i32),
        output: &mut TickOutput,
    ) {
        let straw = is_class(head_state, BlockClass::StrawBedBlock);
        let dark = is_dark(self.last_day());
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.sleeping.is_some() {
            return;
        }
        let foot = offset(head, toward, -1);
        let at = player.pose.position;
        let reachable = |p: BlockPos| {
            (at.x - (f64::from(p.x) + 0.5)).abs() <= 3.0
                && (at.y - f64::from(p.y)).abs() <= 2.0
                && (at.z - (f64::from(p.z) + 0.5)).abs() <= 3.0
        };
        if !reachable(head) && !reachable(foot) {
            self.say(
                id,
                "block.minecraft.bed.too_far_away",
                Vec::new(),
                true,
                output,
            );
            return;
        }
        let above = |p: BlockPos| BlockPos::new(p.x, p.y + 1, p.z);
        let blocked = |p: BlockPos| self.own_block(p).is_none_or(is_suffocating);
        if blocked(above(head)) || blocked(above(foot)) {
            self.say(
                id,
                "block.minecraft.bed.obstructed",
                Vec::new(),
                true,
                output,
            );
            return;
        }
        // The spawn first, before the darkness is asked (B 1.5 step 6).
        if !straw {
            let (yaw, pitch) = (player.pose.yaw, player.pose.pitch);
            let respawn = Respawn {
                pos: head,
                yaw: crate::mobs::ai::maths::wrap_degrees(yaw),
                pitch: pitch.clamp(-90.0, 90.0),
            };
            let moved = player.respawn.is_none_or(|old| old.pos != head);
            if let Some(player) = self.players.get_mut(&id) {
                player.respawn = Some(respawn);
            }
            self.journal.players.insert(id);
            if moved {
                self.say(id, "block.minecraft.set_spawn", Vec::new(), false, output);
            }
        }
        if !dark {
            self.say(id, "block.minecraft.bed.no_sleep", Vec::new(), true, output);
            return;
        }
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.riding.is_some() {
            return;
        }
        // `LivingEntity.startSleeping` (B 2.2).
        let lying = Vec3::new(
            f64::from(head.x) + 0.5,
            f64::from(head.y) + sleep_height(straw) + 0.125,
            f64::from(head.z) + 0.5,
        );
        self.set_block(head, set_flag(head_state, "occupied", true), output);
        let since = self.clock.game_time;
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let previous_chunk = player.chunk();
        player.pose.position = lying;
        player.sleeping = Some(Sleep { head, since });
        // `ServerPlayer.startSleeping`: the time since rest starts again.
        player.rested = self.tick;
        let (entity, pose) = (player.entity_id, player.pose);
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityMoved {
            entity,
            pose,
            previous_chunk,
        });
        output.events.push(RegionEvent::Sleeping {
            entity,
            chunk: ChunkPos::containing(lying.x, lying.z),
            head: Some(head),
        });
        output
            .player_events
            .push((id, PlayerEvent::Teleported { pose }));
        self.sleep_status_changed(output);
    }

    /// `ServerPlayer.stopSleepInBed(wakeImmediately, updateLevelForSleepingPlayers)`
    /// (B 3.3) of the region's player `id`, if they sleep.
    pub(crate) fn stop_sleep(
        &mut self,
        id: PlayerId,
        immediately: bool,
        update: bool,
        output: &mut TickOutput,
    ) {
        let _ = immediately;
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(sleep) = player.sleeping else {
            return;
        };
        let (entity, yaw) = (player.entity_id, player.pose.yaw);
        let previous_chunk = player.chunk();
        // The wake's animation, to those who see them and to them.
        output.events.push(RegionEvent::Animated {
            target: entity,
            attacker: entity,
            chunk: previous_chunk,
            action: 0,
        });
        // `LivingEntity.stopSleeping`: the bed freed, a straw bed broken, and up beside
        // it facing it; nothing of that where the bed is gone or cannot be read.
        let mut stand = None;
        if let Some(state) = self.own_block(sleep.head)
            && is_class(state, BlockClass::AbstractBedBlock)
        {
            let toward = facing(state).map_or((0, 1), step_of);
            self.set_block(sleep.head, set_flag(state, "occupied", false), output);
            if is_class(state, BlockClass::StrawBedBlock) {
                super::redstone::play(
                    sleep.head,
                    sounds::BLOCK_STRAW_BED_BREAK_LEAVE,
                    1.0,
                    1.0,
                    None,
                    output,
                );
                self.set_block(sleep.head, blocks::AIR, output);
            }
            let view = super::items::View {
                region: self,
                home: sleep.head.chunk(),
            };
            let up = rest::stand_up_position(&view, sleep.head, toward, yaw, 0.6, 1.8).unwrap_or(
                Vec3::new(
                    f64::from(sleep.head.x) + 0.5,
                    f64::from(sleep.head.y) + 1.1,
                    f64::from(sleep.head.z) + 0.5,
                ),
            );
            stand = Some(up);
        }
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if let Some(up) = stand {
            player.pose.position = up;
            player.pose.yaw = rest::yaw_to_bed(sleep.head, up);
            player.pose.pitch = 0.0;
        }
        player.sleeping = None;
        // Not counted while asleep: the count goes on from the waking.
        player.rested = self.tick;
        let pose = player.pose;
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityMoved {
            entity,
            pose,
            previous_chunk,
        });
        output.events.push(RegionEvent::Sleeping {
            entity,
            chunk: ChunkPos::containing(pose.position.x, pose.position.z),
            head: None,
        });
        output
            .player_events
            .push((id, PlayerEvent::Teleported { pose }));
        if update {
            self.sleep_status_changed(output);
        }
    }

    /// `AbstractBedBlock.kickVillagerOutOfBed(level, head)` (V 3.4): the first of the
    /// region's villagers in the head's block that sleeps is woken (it gets up at its
    /// tick, [`villager::VillagerState::kicked`]). Whether there was one.
    fn kick_villager_out_of_bed(&mut self, head: BlockPos, output: &mut TickOutput) -> bool {
        let block = Aabb {
            min: [f64::from(head.x), f64::from(head.y), f64::from(head.z)],
            max: [
                f64::from(head.x) + 1.0,
                f64::from(head.y) + 1.0,
                f64::from(head.z) + 1.0,
            ],
        };
        for (key, chunk) in self.entities_in(&block) {
            let Some(record) = self.entity_record(key, chunk).cloned() else {
                continue;
            };
            if i32::from(record.kind) != entity_types::VILLAGER {
                continue;
            }
            let Ok(mut mob) = Mob::decode(&record.data) else {
                continue;
            };
            if villager::state_of(&mob).1.sleeping.is_none() {
                continue;
            }
            villager::update(&mut mob, |own| own.kicked = true);
            self.commit_mob(&record, chunk, &mob, output);
            return true;
        }
        false
    }

    /// The region's view of the level's players for the night (B 4.1): the world's players
    /// the store last published, with its own in place of their entries and added where
    /// missing. The counts of the active, the sleeping and the deep.
    fn sleep_counts(&self) -> (u32, u32, u32) {
        let now = self.clock.game_time;
        let mut active = 0;
        let mut sleeping = 0;
        let mut deep = 0;
        for player in self.players.values() {
            active += 1;
            if let Some(sleep) = player.sleeping {
                sleeping += 1;
                if now.saturating_sub(sleep.since) >= DEEP {
                    deep += 1;
                }
            }
        }
        for other in self.faces.world.iter().flatten() {
            if self.players.contains_key(&other.player) {
                continue;
            }
            // Spectators count for nobody; there are none until survival.
            if other.game_mode == 3 {
                continue;
            }
            active += 1;
            if let Some(counter) = other.sleeping {
                sleeping += 1;
                if u64::from(counter) >= DEEP {
                    deep += 1;
                }
            }
        }
        (active, sleeping, deep)
    }

    /// `SleepStatus.sleepersNeeded`.
    fn sleepers_needed(active: u32) -> u32 {
        let wanted = (active * SLEEPING_PERCENTAGE) as f32 / 100.0;
        (wanted.ceil() as u32).max(1)
    }

    /// The level's sleep check (B 4.3), right after the inputs: enough of the world's
    /// players asleep and enough of them deep, with one of the region's own among them
    /// in the dark, moves the world's clock to the morning and wakes the region's
    /// sleepers, with nothing said. A morning taken from the store's clock is another
    /// region's skip, and wakes them the same way.
    pub(crate) fn check_sleep(&mut self, output: &mut TickOutput) {
        if self.sleep.morning_taken {
            self.wake_everyone(output);
            output.clock = self.clock;
            return;
        }
        let own_sleeper = self
            .players
            .values()
            .any(|player| player.sleeping.is_some());
        if !own_sleeper || !is_dark(self.last_day()) {
            return;
        }
        let (active, sleeping, deep) = self.sleep_counts();
        let needed = Self::sleepers_needed(active);
        if sleeping < needed || deep < needed {
            return;
        }
        // `moveToTimeMarker(overworld, WAKE_UP_FROM_SLEEP)` (B 4.5): the next whole day.
        let day = self.clock.day;
        if !day.is_multiple_of(DAY) {
            self.clock.day = day + (DAY - day % DAY);
        }
        output.clock = self.clock;
        output.clock_jumped = true;
        self.sleep.jumped = true;
        self.wake_everyone(output);
    }

    /// `wakeUpAllPlayers`: every sleeper of the region woken (`stopSleepInBed(false,
    /// false)`), the count of sleepers taken as none, and nothing announced.
    fn wake_everyone(&mut self, output: &mut TickOutput) {
        let sleepers: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| player.sleeping.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in sleepers {
            self.stop_sleep(id, false, false, output);
        }
        if let Some((active, _)) = self.sleep.announced {
            self.sleep.announced = Some((active, 0));
        }
    }

    /// The end of the players' ticks (B 3.2): a sleeper whose bed is gone wakes at once,
    /// one whose bed's rule no longer lets them sleep wakes as the night ends.
    pub(crate) fn sleepers_tick(&mut self, output: &mut TickOutput) {
        let day = self.clock.day;
        let sleepers: Vec<(PlayerId, BlockPos)> = self
            .players
            .iter()
            .filter_map(|(id, player)| Some((*id, player.sleeping?.head)))
            .collect();
        for (id, head) in sleepers {
            let bed = self
                .block(head)
                .is_some_and(|state| is_class(state, BlockClass::AbstractBedBlock));
            if !bed {
                self.stop_sleep(id, true, true, output);
            } else if !is_dark(day) {
                self.stop_sleep(id, false, true, output);
            }
        }
        // The world's players change by the publication too: their sleepers are
        // announced as they come.
        self.announce_sleep(output);
    }

    /// `updateSleepingPlayerList` as a sleeper lies down or gets up.
    fn sleep_status_changed(&mut self, output: &mut TickOutput) {
        self.announce_sleep(output);
    }

    /// `announceSleepStatus` (B 4.4), when the counts changed and someone sleeps or slept:
    /// to every player of the region, on the action bar. The first view after a restore,
    /// a merge or a split is taken without a word.
    fn announce_sleep(&mut self, output: &mut TickOutput) {
        let (active, sleeping, _) = self.sleep_counts();
        let Some((was_active, was_sleeping)) = self.sleep.announced else {
            self.sleep.announced = Some((active, sleeping));
            return;
        };
        let changed = (was_sleeping > 0 || sleeping > 0)
            && (was_active != active || was_sleeping != sleeping);
        self.sleep.announced = Some((active, sleeping));
        if !changed {
            return;
        }
        let needed = Self::sleepers_needed(active);
        let (key, args) = if sleeping >= needed {
            ("sleep.skipping_night", Vec::new())
        } else {
            (
                "sleep.players_sleeping",
                vec![sleeping as i32, needed as i32],
            )
        };
        let everyone: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in everyone {
            self.say(id, key, args.clone(), true, output);
        }
    }
}

/// The region's memory of the night, not its state: what it last announced, the clock
/// before the tick, and whether the tick took the morning from the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SleepMemory {
    /// The (active, sleeping) last announced; none before the first look.
    pub(crate) announced: Option<(u32, u32)>,
    /// The overworld clock as the tick before left it.
    pub(crate) last_day: u64,
    /// Whether the clock the tick took from the store crossed a morning.
    pub(crate) morning_taken: bool,
    /// Whether this tick skipped the night.
    pub(crate) jumped: bool,
}
