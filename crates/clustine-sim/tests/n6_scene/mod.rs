//! The world the tests of step N6 from its specification share: a stone floor at
//! y = -61 with air above, regions alone or on either side of a pinned line at block
//! x = 16, players who join them and act through their inputs, and readers of what the
//! regions report. Written from the public interface alone, as
//! `riders_specification.rs` and `placed_specification.rs` are.

// Each test file uses its own part of these.
#![allow(dead_code)]

use clustine_data::{BlockState, blocks};
use clustine_sim::api::{
    ClickKind, EnderChest, Face, HOTBAR_SLOTS, Inventory, ItemStack, MenuKind, Place, PlayerEvent,
    PlayerInput, Pose, RegionEvent,
};
use clustine_sim::{
    Durable, EdgeState, Entered, Holdings, PlayerChange, PlayerJoin, PlayerTransfer, Region,
    RegionConfig, RegionState, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    BlockPos, Chunk, ChunkArea, ChunkPos, Due, EdgeId, Effect, EffectId, Entity, EntityData,
    EntityId, EntityIds, FiledEffect, PlayerId, RegionId, ScheduledTick, Section, Vec3,
};
use uuid::Uuid;

pub const EDGE: EdgeId = EdgeId(7);
pub const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);
/// The region the west believes to hold the east, and the other way round.
pub const OTHER: RegionId = RegionId(9);

pub fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

pub fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

pub fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

pub fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// The id of the block a state belongs to, as a block event names it.
pub fn block_id(state: BlockState) -> u16 {
    u16::try_from(state.block_id().expect("a block")).expect("fits")
}

/// An inventory with `slots` filled, numbered as [`Inventory`] numbers them.
pub fn inventory(slots: &[(usize, ItemStack)]) -> Inventory {
    let mut inventory = Inventory::with_hotbar([const { None }; HOTBAR_SLOTS]);
    for (slot, stack) in slots {
        inventory.set(*slot, Some(stack.clone()));
    }
    inventory
}

pub fn config(inventory: Inventory) -> RegionConfig {
    RegionConfig {
        world_spawn: clustine_world::BlockPos::new(0, -60, 0),
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension: Default::default(),
        spawn: Some(SPAWN),
        starting_inventory: inventory,
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: clustine_sim::RANDOM_TICK_SPEED,
    }
}

/// An overworld column with a stone floor at y = -61 and air above.
pub fn floor() -> Chunk {
    floor_in(0)
}

/// [`floor`] in the biome of id `biome` (0, the badlands, for the floor of every test
/// that does not say).
pub fn floor_in(biome: u16) -> Chunk {
    let sections = (0..24)
        .map(|_| Section::filled(blocks::AIR, clustine_world::Biome(biome)))
        .collect();
    let mut chunk = Chunk::from_sections(-64, sections);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, blocks::STONE);
        }
    }
    chunk
}

pub fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// What a scene's chunks hold beyond the floor: blocks, entities, and scheduled ticks
/// (a fire placed in a chunk is ticked only by a tick its placing scheduled), and the
/// biome all of it is in.
#[derive(Default, Clone)]
pub struct World {
    pub blocks: Vec<(BlockPos, BlockState)>,
    pub entities: Vec<Entity>,
    pub ticks: Vec<(BlockPos, ScheduledTick)>,
    pub biome: u16,
}

impl World {
    pub fn of(blocks: &[(BlockPos, BlockState)]) -> Self {
        Self {
            blocks: blocks.to_vec(),
            ..Self::default()
        }
    }

    pub fn with_entities(mut self, entities: &[Entity]) -> Self {
        self.entities = entities.to_vec();
        self
    }

    /// The chunk at `position`: the floor with what of this world is in it.
    pub fn chunk(&self, position: ChunkPos) -> Chunk {
        let mut chunk = floor_in(self.biome);
        for (at, state) in &self.blocks {
            if at.chunk() == position {
                chunk.set(
                    at.x.rem_euclid(16) as usize,
                    at.y,
                    at.z.rem_euclid(16) as usize,
                    *state,
                );
            }
        }
        chunk.set_entities(
            self.entities
                .iter()
                .filter(|entity| entity.chunk() == position)
                .cloned()
                .collect(),
        );
        chunk.set_ticks(
            self.ticks
                .iter()
                .filter(|(at, _)| at.chunk() == position)
                .map(|(_, tick)| *tick)
                .collect(),
        );
        chunk
    }
}

/// The chunks of one region's scene.
#[derive(Clone)]
pub struct Layout {
    pub pinned: ChunkArea,
    pub held: Vec<ChunkPos>,
    pub loaded: Vec<ChunkPos>,
    pub copies: Vec<ChunkPos>,
    pub foreign: Vec<ChunkPos>,
    pub block: u32,
}

impl Layout {
    /// One region over all the land near the origin.
    pub fn alone() -> Self {
        Self {
            pinned: ChunkArea::EVERYWHERE,
            held: around(3),
            loaded: around(2),
            copies: Vec::new(),
            foreign: Vec::new(),
            block: 1,
        }
    }

    /// One side of a pinned line at block x = 16, chunk x = 1: holding and loading its
    /// side, with copies of the other's chunks, which [`OTHER`] holds.
    pub fn side(west: bool) -> Self {
        let (pinned, mine): (ChunkArea, fn(&ChunkPos) -> bool) = if west {
            (
                ChunkArea {
                    min_x: None,
                    max_x: Some(1),
                },
                |chunk| chunk.x <= 0,
            )
        } else {
            (
                ChunkArea {
                    min_x: Some(1),
                    max_x: None,
                },
                |chunk| chunk.x >= 1,
            )
        };
        let wider: Vec<ChunkPos> = (-3..=4)
            .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let near: Vec<ChunkPos> = (-2..=3)
            .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let others: Vec<ChunkPos> = near.iter().copied().filter(|c| !mine(c)).collect();
        Self {
            pinned,
            held: wider.into_iter().filter(mine).collect(),
            loaded: near.iter().copied().filter(mine).collect(),
            copies: others.clone(),
            foreign: others,
            block: if west { 1 } else { 2 },
        }
    }

    pub fn holdings(&self) -> Holdings {
        Holdings {
            held: self.held.clone(),
            pinned: vec![self.pinned],
            lent: Vec::new(),
            lent_away: Vec::new(),
        }
    }
}

/// A region of `layout` with `world` in its chunks and copies, nobody in it, whose
/// players enter with `inventory`.
pub fn region_of(layout: &Layout, world: &World, inventory: Inventory) -> Region {
    region_at(layout, world, inventory, None)
}

/// As [`region_of`], with the world's clock at `clock` before any chunk is loaded, where
/// a test wants a time of day (the undead burn by day).
pub fn region_at(
    layout: &Layout,
    world: &World,
    inventory: Inventory,
    clock: Option<clustine_sim::api::WorldClock>,
) -> Region {
    let mut state = RegionState::new(EntityIds::block(layout.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let mut region = Region::restore(config(inventory), state, layout.holdings());
    region.tick(&TickInputs {
        // The chunks beyond the line are wanted too, so that the region claims them and
        // learns who holds them: a player who walks into them is let go there.
        tickets_added: layout
            .loaded
            .iter()
            .chain(&layout.foreign)
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        clock,
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: layout
            .loaded
            .iter()
            .map(|position| (*position, world.chunk(*position)))
            .collect(),
        copy_bases: layout
            .copies
            .iter()
            .map(|position| (*position, world.chunk(*position)))
            .collect(),
        foreign: layout
            .foreign
            .iter()
            .map(|position| (*position, OTHER))
            .collect(),
        ..TickInputs::default()
    });
    for chunk in &layout.loaded {
        assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
    }
    region
}

/// A player in a scene, with the numbers of their inputs and actions.
pub struct Actor {
    pub id: PlayerId,
    pub entity: EntityId,
    pub input: u64,
    pub sequence: i32,
}

/// A region and the players in it.
pub struct Scene {
    pub region: Region,
    pub layout: Layout,
    pub inventory: Inventory,
    pub actors: Vec<Actor>,
}

impl Scene {
    /// A region of `layout` with `world`, and player 1 in it at `feet`, holding
    /// `inventory` with slot 0 selected.
    pub fn new(layout: Layout, world: &World, inventory: Inventory, feet: Vec3) -> Self {
        let region = region_of(&layout, world, inventory.clone());
        let mut scene = Self {
            region,
            layout,
            inventory,
            actors: Vec::new(),
        };
        scene.join(1, feet);
        scene
    }

    /// One region over the land near the origin, player 1 at `feet`.
    pub fn alone(world: &World, inventory: Inventory, feet: Vec3) -> Self {
        Self::new(Layout::alone(), world, inventory, feet)
    }

    /// As [`Scene::alone`], at night: the world's clock at the day's tick 18000.
    #[allow(dead_code, reason = "not every test of the scene wants the night")]
    pub fn alone_at_night(world: &World, inventory: Inventory, feet: Vec3) -> Self {
        let night = clustine_sim::api::WorldClock {
            game_time: 18_000,
            day: 18_000,
        };
        let layout = Layout::alone();
        let region = region_at(&layout, world, inventory.clone(), Some(night));
        let mut scene = Self {
            region,
            layout,
            inventory,
            actors: Vec::new(),
        };
        scene.join(1, feet);
        scene
    }

    /// Player `n` joins and enters at `feet` with the scene's inventory, as the store's
    /// place for them says. Returns their index among the actors.
    pub fn join(&mut self, n: u128, feet: Vec3) -> usize {
        self.region.tick(&TickInputs {
            player_changes: vec![PlayerChange::Join(
                EDGE,
                PlayerJoin {
                    player: player(n),
                    name: format!("Player{n}"),
                    attempt: 7000 + n as u64,
                },
            )],
            ..TickInputs::default()
        });
        let entering = self
            .region
            .entering_state(player(n))
            .expect("the join is entering")
            .entity_id;
        self.region.tick(&TickInputs {
            entered: vec![Entered {
                unit: None,
                player: player(n),
                entity: entering,
                place: Some(Place {
                    health: 20.0,
                    dead: false,
                    dimension: Default::default(),
                    travel: None,
                    seen_credits: false,
                    portal_cooldown: 0,
                    respawn: None,
                    experience: Default::default(),
                    pose: Pose {
                        position: feet,
                        yaw: 0.0,
                        pitch: 0.0,
                        on_ground: true,
                    },
                    flying: false,
                    inventory: self.inventory.clone(),
                    selected_slot: 0,
                    ender: EnderChest::default(),
                    effects: Vec::new(),
                    time_since_rest: 0,
                    raid_omen_position: None,
                }),
                holder: None,
            }],
            ..TickInputs::default()
        });
        let entity = self
            .region
            .player(player(n))
            .unwrap_or_else(|| panic!("player {n} entered"))
            .0;
        self.actors.push(Actor {
            id: player(n),
            entity,
            input: 0,
            sequence: 0,
        });
        let who = self.actors.len() - 1;
        let feet_now = self.region.player(player(n)).expect("there").1.position;
        assert_eq!(feet_now, feet, "player {n} stands where the test puts them");
        who
    }

    pub fn me(&self) -> EntityId {
        self.actors[0].entity
    }

    /// One tick with `inputs` of the actor `who`, numbered on from their last.
    pub fn inputs_of(&mut self, who: usize, inputs: Vec<PlayerInput>) -> TickOutput {
        let actor = &mut self.actors[who];
        let inputs = inputs
            .into_iter()
            .map(|input| {
                actor.input += 1;
                (EDGE, actor.id, actor.entity, actor.input, input)
            })
            .collect();
        self.region.tick(&TickInputs {
            inputs,
            ..TickInputs::default()
        })
    }

    pub fn input_of(&mut self, who: usize, input: PlayerInput) -> TickOutput {
        self.inputs_of(who, vec![input])
    }

    pub fn input(&mut self, input: PlayerInput) -> TickOutput {
        self.input_of(0, input)
    }

    /// The number the next input of `who` will have.
    pub fn next_input(&self, who: usize) -> u64 {
        self.actors[who].input + 1
    }

    pub fn idle(&mut self) -> TickOutput {
        self.region.tick(&TickInputs::default())
    }

    pub fn idle_for(&mut self, ticks: u32) -> Vec<TickOutput> {
        (0..ticks).map(|_| self.idle()).collect()
    }

    /// `who` clicks `face` of the block at `position` with what they hold, not
    /// sneaking. The input's number is the serial of a menu it opens.
    pub fn click_by(&mut self, who: usize, position: BlockPos, face: Face) -> TickOutput {
        self.actors[who].sequence += 1;
        let sequence = self.actors[who].sequence;
        let cursor = match face {
            Face::Bottom => [0.5, 0.0, 0.5],
            Face::Top => [0.5, 1.0, 0.5],
            Face::North => [0.5, 0.5, 0.0],
            Face::South => [0.5, 0.5, 1.0],
            Face::West => [0.0, 0.5, 0.5],
            Face::East => [1.0, 0.5, 0.5],
        };
        self.input_of(
            who,
            PlayerInput::UseItemOn {
                position,
                face,
                cursor,
                sneaking: false,
                sequence,
            },
        )
    }

    pub fn click(&mut self, position: BlockPos, face: Face) -> TickOutput {
        self.click_by(0, position, face)
    }

    /// Player 1 opens the container at `position` with a click on its top: the
    /// serial of the menu, and the tick's output.
    pub fn open(&mut self, position: BlockPos) -> (u64, TickOutput) {
        self.open_by(0, position)
    }

    pub fn open_by(&mut self, who: usize, position: BlockPos) -> (u64, TickOutput) {
        let serial = self.next_input(who);
        let output = self.click_by(who, position, Face::Top);
        (serial, output)
    }

    pub fn dig(&mut self, position: BlockPos) -> TickOutput {
        self.actors[0].sequence += 1;
        let sequence = self.actors[0].sequence;
        self.input(PlayerInput::Dig { position, sequence })
    }

    pub fn menu_click_by(
        &mut self,
        who: usize,
        serial: u64,
        slot: i16,
        button: i8,
        kind: ClickKind,
    ) -> TickOutput {
        self.input_of(
            who,
            PlayerInput::ContainerClick {
                serial,
                slot,
                button,
                kind,
            },
        )
    }

    /// Player 1 clicks `slot` of the menu of `serial`.
    pub fn menu_click(
        &mut self,
        serial: u64,
        slot: i16,
        button: i8,
        kind: ClickKind,
    ) -> TickOutput {
        self.menu_click_by(0, serial, slot, button, kind)
    }

    pub fn close_by(&mut self, who: usize, serial: u64) -> TickOutput {
        self.input_of(who, PlayerInput::CloseContainer { serial })
    }

    pub fn close(&mut self, serial: u64) -> TickOutput {
        self.close_by(0, serial)
    }

    pub fn select(&mut self, slot: u8) -> TickOutput {
        self.input(PlayerInput::SelectSlot { slot })
    }

    pub fn walk_to(&mut self, feet: Vec3) -> TickOutput {
        self.input(PlayerInput::Move {
            position: Some(feet),
            rotation: None,
            on_ground: true,
        })
    }

    /// What player 1 has in the inventory slot `slot`.
    pub fn holds(&self, slot: usize) -> Option<ItemStack> {
        self.region
            .player_state(self.actors[0].id)
            .expect("player 1 is there")
            .inventory
            .0[slot]
            .clone()
    }

    pub fn block(&self, at: BlockPos) -> BlockState {
        block_at(&self.region, at)
    }

    pub fn all(&self) -> Vec<Entity> {
        all_of(&self.region)
    }

    pub fn of_kind(&self, kind: i32) -> Vec<Entity> {
        of_kind(&self.region, kind)
    }

    /// Idles until `done` holds, for at most `ticks` ticks, and says whether it did; the
    /// outputs of the ticks idled go to `seen`.
    pub fn idle_until(
        &mut self,
        ticks: u32,
        seen: &mut Vec<TickOutput>,
        done: impl Fn(&Scene, &[TickOutput]) -> bool,
    ) -> bool {
        for _ in 0..ticks {
            if done(self, seen) {
                return true;
            }
            let output = self.idle();
            seen.push(output);
        }
        done(self, seen)
    }
}

/// The block at `at` in a loaded chunk or a copy of the region.
pub fn block_at(region: &Region, at: BlockPos) -> BlockState {
    let chunk = region
        .chunk(at.chunk())
        .or_else(|| region.copy(at.chunk()))
        .unwrap_or_else(|| panic!("{at:?} is in a loaded chunk or a copy"));
    chunk
        .get(
            at.x.rem_euclid(16) as usize,
            at.y,
            at.z.rem_euclid(16) as usize,
        )
        .expect("in the world")
}

pub fn all_of(region: &Region) -> Vec<Entity> {
    let mut found = Vec::new();
    for chunk in region.loaded_chunks() {
        found.extend(
            region
                .chunk(chunk)
                .expect("loaded")
                .entities()
                .iter()
                .cloned(),
        );
    }
    found
}

pub fn of_kind(region: &Region, kind: i32) -> Vec<Entity> {
    all_of(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == kind)
        .collect()
}

/// The stacks of the item entities among `entities`, with where each lies.
pub fn items_lying(entities: &[Entity]) -> Vec<(ItemStack, [f64; 3])> {
    entities
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, .. } => Some((stack.clone(), entity.position)),
            _ => None,
        })
        .collect()
}

/// How many of `item` lie about in all.
pub fn lying_count(entities: &[Entity], item: i32) -> i32 {
    items_lying(entities)
        .iter()
        .filter(|(stack, _)| stack.item == item)
        .map(|(stack, _)| stack.count)
        .sum()
}

/// A key from a block no region of these tests is given.
pub fn key(n: u32) -> EntityId {
    EntityId((9 << 32) + u64::from(n))
}

// ------------------------------------------------------------------------------------
// What the regions report.

/// A [`PlayerEvent::MenuOpened`] taken apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub serial: u64,
    pub applied: u64,
    pub kind: MenuKind,
    pub title: String,
    pub slots: Vec<Option<ItemStack>>,
    pub carried: Option<ItemStack>,
}

/// A [`PlayerEvent::MenuChanged`] taken apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Changed {
    pub serial: u64,
    pub applied: u64,
    pub slots: Vec<(u16, Option<ItemStack>)>,
    pub carried: Option<Option<ItemStack>>,
}

/// The events of player `who` in a tick, in order.
pub fn events_of(output: &TickOutput, who: PlayerId) -> Vec<PlayerEvent> {
    output
        .player_events
        .iter()
        .filter(|(player, _)| *player == who)
        .map(|(_, event)| event.clone())
        .collect()
}

pub fn opened(output: &TickOutput, who: PlayerId) -> Vec<Opened> {
    events_of(output, who)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::MenuOpened {
                serial,
                applied,
                kind,
                title,
                slots,
                carried,
                ..
            } => Some(Opened {
                serial,
                applied,
                kind,
                title,
                slots,
                carried,
            }),
            _ => None,
        })
        .collect()
}

pub fn changed(output: &TickOutput, who: PlayerId) -> Vec<Changed> {
    events_of(output, who)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::MenuChanged {
                serial,
                applied,
                slots,
                carried,
            } => Some(Changed {
                serial,
                applied,
                slots,
                carried,
            }),
            _ => None,
        })
        .collect()
}

/// The serials and last inputs of the tick's [`PlayerEvent::MenuClosed`].
pub fn closed(output: &TickOutput, who: PlayerId) -> Vec<(u64, u64)> {
    events_of(output, who)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::MenuClosed { serial, applied } => Some((serial, applied)),
            _ => None,
        })
        .collect()
}

pub fn inventory_slots(output: &TickOutput, who: PlayerId) -> Vec<(u8, Option<ItemStack>)> {
    events_of(output, who)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::InventorySlot { slot, stack } => Some((slot, stack)),
            _ => None,
        })
        .collect()
}

/// The one menu opened for `who` in the tick.
pub fn the_opened(output: &TickOutput, who: PlayerId) -> Opened {
    let found = opened(output, who);
    assert_eq!(
        found.len(),
        1,
        "one menu opened: {:?}",
        output.player_events
    );
    found.into_iter().next().expect("one")
}

/// The one change of a menu reported for `who` in the tick.
pub fn the_changed(output: &TickOutput, who: PlayerId) -> Changed {
    let found = changed(output, who);
    assert_eq!(
        found.len(),
        1,
        "one menu change: {:?}",
        output.player_events
    );
    found.into_iter().next().expect("one")
}

/// The block events of the tick at `at`, as (kind, data).
pub fn block_events_at(output: &TickOutput, at: BlockPos) -> Vec<(u16, i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } if *position == at => Some((*block, *kind, *data)),
            _ => None,
        })
        .collect()
}

/// The sounds of the tick of the id `sound`, with where each plays and how loud.
pub fn sounds(output: &TickOutput, sound: i32) -> Vec<(Vec3, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: played,
                position,
                volume,
                ..
            } if *played == sound => Some((*position, *volume)),
            _ => None,
        })
        .collect()
}

/// The level events of the tick, as (kind, position, data).
pub fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

/// The players let go in the tick, with the transfer each carries.
pub fn departures(output: &TickOutput) -> Vec<(PlayerId, PlayerTransfer, RegionId)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, durable)| match durable {
            Durable::Departed {
                player,
                transfer,
                to,
            } => Some((*player, transfer.clone(), *to)),
            _ => None,
        })
        .collect()
}

/// Hands `effects` to `holder` as the store files them, numbered from `sequence`, and
/// lets it tick twice: they are done in the next tick's seams. Returns both outputs.
pub fn filed(holder: &mut Region, sequence: u64, effects: Vec<Effect>) -> Vec<TickOutput> {
    let effects = effects
        .into_iter()
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence,
                index: u32::try_from(index).expect("few"),
            },
            effect,
            due: Due::FromTaking,
        })
        .collect();
    let first = holder.tick(&TickInputs {
        effects,
        ..TickInputs::default()
    });
    let second = holder.tick(&TickInputs::default());
    vec![first, second]
}

/// The centre of the block at `at`.
pub fn centre(at: BlockPos) -> Vec3 {
    Vec3::new(
        f64::from(at.x) + 0.5,
        f64::from(at.y) + 0.5,
        f64::from(at.z) + 0.5,
    )
}

pub fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}
