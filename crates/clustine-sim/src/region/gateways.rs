//! The End's gateways (`docs/groundwork/p-plan.md`, section 19.8 with its review; the
//! brief's 2.6; d-javap's TheEndGatewayBlockEntity.txt and EndGatewayBlock.txt).
//!
//! - **The block entity.** Its data keeps `Age`, `exit_portal` and `ExactTeleport` as
//!   the game saves them; the region keeps its age and its cooldown in memory
//!   ([`GatewayClock`]) and ticks it (`portalTick`): the age goes on; a cooldown counts
//!   down, and where there is none the gateway starts one every 2400 ticks of its age,
//!   which the clients see as its beam (block event 1). The age is written into the data
//!   only where the game calls `setChanged` (its spawning or its cooling begins or ends,
//!   its exit is set), so a gateway let go loses the ticks since, named.
//! - **Asked ahead.** A region of the End asks the store where each gateway it holds
//!   without an exit leads, once in its run (the question is not kept: a restore, a
//!   merge or a split asks again). The answer is written into the gateway where the
//!   region holds it still and it has no exit; in the same commit a traveller with no
//!   entities is filed for the exit's chunk, whose holder makes the island where one is
//!   wanted and the return gateway (`knownExit(from, false)`).
//! - **Entering.** A gateway takes in only what it can send on: it has an exit and is
//!   not cooling down; an entity marked inside it starts its cooldown
//!   (`triggerCooldown`), and goes the next tick (its transition time is none).
//! - **Where one comes out:** at the exit where it is exact, else on the tallest block
//!   within 5 of two above it (`findExitPosition`), with its velocity and look.
//!
//! Named: the beam of a gateway's spawning is shown by the client from its age, which a
//! loaded gateway may have a few ticks behind; an ender pearl through a gateway is not
//! built (it is not thrown through portals yet).

use clustine_blocks::data::is_block;
use clustine_data::{BlockState, blocks};
use clustine_features::features::end::end_gateway::gateway_block;
use clustine_features::features::end::gateway_exit::{find_tallest_block, island_writes};
use clustine_world::{BlockPos, ChunkPos, Dimension, LevelChunk, Travel, Traveller, Way};

use super::Region;
use super::block_entities::{
    Entry, TAG_BYTE, TAG_COMPOUND, TAG_END, TAG_INT_ARRAY, TAG_LONG, compound, each_entry,
};
use super::updates::UPDATE_ALL;
use crate::api::{GatewayExitAnswer, GatewayExitQuery, QueryId, RegionEvent, TickOutput};

/// How long a gateway shows its spawning beam (`SPAWN_TIME`).
const SPAWN_TIME: i64 = 200;

/// How long a gateway cools down after it fires or flashes (`COOLDOWN_TIME`).
const COOLDOWN_TIME: i32 = 40;

/// How often a gateway flashes of itself (`ATTENTION_INTERVAL`).
const ATTENTION_INTERVAL: i64 = 2400;

/// What the region keeps of a gateway in memory: its age and its cooldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct GatewayClock {
    pub(crate) age: i64,
    pub(crate) cooldown: i32,
}

/// A gateway's data as the game saves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct GatewayData {
    pub(crate) age: i64,
    pub(crate) exit: Option<BlockPos>,
    pub(crate) exact: bool,
}

/// `TheEndGatewayBlockEntity.loadAdditional` of `data`: what it does not say, the
/// defaults of a new gateway.
pub(crate) fn read_gateway(data: &[u8]) -> GatewayData {
    let mut read = GatewayData::default();
    let Some((&TAG_COMPOUND, entries)) = data.split_first() else {
        return read;
    };
    let _ = each_entry(entries, 0, &mut |tag, name, payload| match (tag, name) {
        (TAG_LONG, b"Age") => {
            if let Some(bytes) = payload.get(..8) {
                read.age = i64::from_be_bytes(bytes.try_into().expect("eight bytes"));
            }
        }
        (TAG_INT_ARRAY, b"exit_portal") => {
            let ints: Vec<i32> = payload
                .get(4..16)
                .map(|bytes| {
                    bytes
                        .chunks(4)
                        .map(|int| i32::from_be_bytes(int.try_into().expect("four bytes")))
                        .collect()
                })
                .unwrap_or_default();
            let length = payload
                .get(..4)
                .map(|bytes| i32::from_be_bytes(bytes.try_into().expect("four bytes")));
            if let (Some(3), [x, y, z]) = (length, &ints[..]) {
                read.exit = Some(BlockPos::new(*x, *y, *z));
            }
        }
        (TAG_BYTE, b"ExactTeleport") => {
            read.exact = payload.first().is_some_and(|byte| *byte != 0);
        }
        _ => {}
    });
    read
}

/// `TheEndGatewayBlockEntity.saveAdditional` of `gateway`, with the empty compound of
/// components every block entity saves: its age, its exit where it has one, and
/// `ExactTeleport` only where it is exact.
pub(crate) fn gateway_data(gateway: GatewayData) -> Vec<u8> {
    let mut entries = vec![
        Entry {
            name: "Age",
            tag: TAG_LONG,
            payload: gateway.age.to_be_bytes().to_vec(),
        },
        Entry {
            name: "components",
            tag: TAG_COMPOUND,
            payload: vec![TAG_END],
        },
    ];
    if let Some(exit) = gateway.exit {
        let mut payload = 3_i32.to_be_bytes().to_vec();
        for value in [exit.x, exit.y, exit.z] {
            payload.extend_from_slice(&value.to_be_bytes());
        }
        entries.push(Entry {
            name: "exit_portal",
            tag: TAG_INT_ARRAY,
            payload,
        });
    }
    if gateway.exact {
        entries.push(Entry {
            name: "ExactTeleport",
            tag: TAG_BYTE,
            payload: vec![1],
        });
    }
    // One nameless compound with its type byte first, as every block entity's data is.
    let mut data = vec![TAG_COMPOUND];
    data.extend(compound(entries));
    data
}

impl Region {
    /// The gateway's data at `pos`, where the region reads a gateway there.
    pub(crate) fn gateway_at(&self, pos: BlockPos) -> Option<GatewayData> {
        let state = self.block(pos)?;
        if !is_block(state, blocks::END_GATEWAY) {
            return None;
        }
        Some(
            self.block_entity_data(pos)
                .map_or_else(GatewayData::default, read_gateway),
        )
    }

    /// Whether the gateway at `pos` takes in a traveller now: it has an exit and is not
    /// cooling down (`entityInside`, and the review's first point).
    pub(crate) fn gateway_open(&self, pos: BlockPos) -> bool {
        self.gateway_at(pos)
            .is_some_and(|gateway| gateway.exit.is_some())
            && self
                .gateway_clock(pos)
                .is_some_and(|clock| clock.cooldown == 0)
    }

    /// `portalTick` of the gateway at `pos`.
    pub(super) fn gateway_tick(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(mut clock) = self.gateway_clock(pos) else {
            return;
        };
        let spawning = clock.age < SPAWN_TIME;
        let cooling = clock.cooldown > 0;
        clock.age += 1;
        if clock.cooldown > 0 {
            clock.cooldown -= 1;
            self.set_gateway_clock(pos, clock);
        } else {
            self.set_gateway_clock(pos, clock);
            if clock.age % ATTENTION_INTERVAL == 0 {
                self.trigger_gateway_cooldown(pos, output);
            }
        }
        let Some(clock) = self.gateway_clock(pos) else {
            return;
        };
        if spawning != (clock.age < SPAWN_TIME) || cooling != (clock.cooldown > 0) {
            self.write_gateway_age(pos, output);
        }
    }

    /// `triggerCooldown`: 40 ticks of cooling, and the block event that has the clients
    /// show the beam.
    pub(crate) fn trigger_gateway_cooldown(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(mut clock) = self.gateway_clock(pos) else {
            return;
        };
        clock.cooldown = COOLDOWN_TIME;
        self.set_gateway_clock(pos, clock);
        output.events.push(RegionEvent::BlockEvent {
            position: pos,
            block: blocks::END_GATEWAY
                .block_id()
                .and_then(|id| u16::try_from(id).ok())
                .expect("the gateway is a block of the registry"),
            kind: 1,
            data: 0,
        });
        self.write_gateway_age(pos, output);
    }

    /// The gateway's data written with its age as the region counts it, unseen
    /// (`setChanged` sends nothing).
    fn write_gateway_age(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let (Some(clock), Some(mut data)) = (self.gateway_clock(pos), self.gateway_at(pos)) else {
            return;
        };
        data.age = clock.age;
        self.set_block_entity_data_as(pos, gateway_data(data), false, output);
    }

    /// Asks the store where each gateway the region holds without an exit leads, once in
    /// the region's run.
    pub(super) fn ask_gateways(&mut self, output: &mut TickOutput) {
        if self.config.dimension != Dimension::End {
            return;
        }
        let tick = self.tick;
        let unknown: Vec<BlockPos> = self
            .gateway_positions()
            .into_iter()
            .filter(|pos| !self.gateways_asked.contains(pos))
            .filter(|pos| {
                self.gateway_at(*pos)
                    .is_some_and(|gateway| gateway.exit.is_none())
            })
            .collect();
        for (index, from) in unknown.into_iter().enumerate() {
            self.gateways_asked.insert(from);
            output.gateway_queries.push(GatewayExitQuery {
                id: QueryId {
                    tick,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                },
                from,
            });
        }
    }

    /// The store's answers: each exit written into its gateway where the region holds it
    /// still and it has none, with the traveller that has the exit's holder make the
    /// island and the return gateway.
    pub(super) fn take_gateway_answers(
        &mut self,
        answers: &[GatewayExitAnswer],
        output: &mut TickOutput,
    ) {
        for answer in answers {
            let Some(found) = answer.exit else {
                continue;
            };
            if self.own_block(answer.from).is_none() {
                continue;
            }
            let Some(mut data) = self.gateway_at(answer.from) else {
                continue;
            };
            if data.exit.is_some() {
                continue;
            }
            data.exit = Some(found.exit);
            if let Some(clock) = self.gateway_clock(answer.from) {
                data.age = clock.age;
            }
            self.set_block_entity_data_as(answer.from, gateway_data(data), false, output);
            let travel = Travel {
                to: Dimension::End,
                target: found.exit,
                way: Way::Gateway {
                    from: answer.from,
                    exact: data.exact,
                    island: found.island,
                },
            };
            output.travellers.push(Traveller {
                travel,
                chunk: LevelChunk::new(
                    Dimension::End,
                    ChunkPos::new(found.exit.x >> 4, found.exit.z >> 4),
                ),
                entities: Vec::new(),
            });
        }
    }

    /// The traveller with no entities a gateway's exit was filed with: the island where
    /// one is wanted (`end_island` about its centre, each block whatever stood there),
    /// then the return gateway at the exit (`knownExit(from, false)`), unless a gateway
    /// stands there already.
    pub(super) fn make_gateway_exit(&mut self, travel: &Travel, output: &mut TickOutput) {
        let Way::Gateway { from, island, .. } = travel.way else {
            return;
        };
        let exit = travel.target;
        if self
            .block(exit)
            .is_some_and(|state| is_block(state, blocks::END_GATEWAY))
        {
            return;
        }
        if let Some(island) = island {
            for (pos, state) in island_writes(island) {
                self.set_for_travel(pos, state, UPDATE_ALL, output);
            }
        }
        for dy in -2..=2 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let state: BlockState = gateway_block(dx, dy, dz);
                    self.set_for_travel(exit.offset(dx, dy, dz), state, UPDATE_ALL, output);
                }
            }
        }
        let data = GatewayData {
            age: 0,
            exit: Some(from),
            exact: false,
        };
        self.set_block_entity_data_as(exit, gateway_data(data), false, output);
    }

    /// Where a traveller through a gateway comes out (`getPortalPosition`): the exit's
    /// bottom centre where it is exact, else that of the block above the tallest within
    /// 5 of two above the exit, bedrock refused (`findExitPosition`).
    pub(super) fn gateway_exit_place(&self, travel: &Travel) -> [f64; 3] {
        let exit = travel.target;
        let exact = matches!(travel.way, Way::Gateway { exact: true, .. });
        let at = if exact {
            exit
        } else {
            let rules = self.config.dimension.rules();
            let top = rules.min_y + i32::try_from(rules.height).unwrap_or(i32::MAX) - 1;
            let read = |pos: BlockPos| self.block(pos).unwrap_or(blocks::AIR);
            find_tallest_block(read, rules.min_y, top, exit.offset(0, 2, 0), 5, false)
                .offset(0, 1, 0)
        };
        [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ]
    }
}

#[cfg(test)]
#[path = "gateways_tests.rs"]
mod tests;
