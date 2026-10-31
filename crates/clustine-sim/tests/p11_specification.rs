//! Tests of step P11, the rules that differ by dimension, written from the specification
//! and the public interface alone by someone who did not write the code:
//! `docs/groundwork/p-plan.md` sections 6 and 17 (17.1 to 17.6, the review's changes
//! holding where they differ), `docs/groundwork/p-brief.md` sections 1.2 and 5, and the
//! game's bytecode where a rule's detail is wanted (`reference/*-javap/`).
//!
//! A region of one dimension holds the chunks about the origin's, loaded, with a stone
//! floor at y = 40 (feet at 41), which every dimension's height holds, and air above and
//! below. Player 1 (creative, as every player is) stands near the origin. Each test says
//! which rule it checks, and runs it in each dimension where the rule can differ.
//!
//! Not reachable from this crate's interface, and so named rather than tested: the sea
//! level and lowest height the worker gives a region of each dimension (plan 17.1, 17.5;
//! `RegionConfig::sea_level` and `lowest_y` are the worker's to set, and these tests set
//! them as the plan says), and the push of lava on an entity (0.007 against
//! 0.0023333333333333335, plan 17.2 B).

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_light::engine::Kept;
use clustine_sim::api::{
    ClickKind, EnderChest, Face, HOTBAR_SLOTS, Inventory, ItemStack, MenuKind, OFF_HAND, Place,
    PlayerEvent, PlayerInput, Pose, RegionEvent, Respawn, WorldClock, WorldPlayer,
};
use clustine_sim::shown::{DataShown, index, mob_shared_flags, mob_values};
use clustine_sim::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerJoin, Region, RegionConfig, RegionState,
    TickInputs, TickOutput, Ticket, time,
};
use clustine_world::components::EffectInstance;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Dimension, Due, EdgeId, EffectId, Effected,
    Entity, EntityId, EntityIds, FiledEffect, KeptBox, KeptKind, KeptStructure, PlayerId, RegionId,
    SectionLight, Vec3,
};
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The scene.

const EDGE: EdgeId = EdgeId(7);
/// The floor's top, and the height of a player's feet on it.
const FLOOR: i32 = 40;
const FEET: i32 = FLOOR + 1;
const ORIGIN: Vec3 = Vec3::new(0.5, FEET as f64, 0.5);

/// Ticks of the overworld clock's day.
const NOON: u64 = 6_000;
const MIDNIGHT: u64 = 18_000;

/// The region another region believes to hold what it does not.
const OTHER: RegionId = RegionId(9);

/// `Entity.FLAG_INVISIBLE` of the shared flags.
const INVISIBLE: u8 = 0x20;

fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

/// An inventory holding `held` in slot 0, `second` in slot 1 and `off` in the off hand.
fn holding(
    held: Option<ItemStack>,
    second: Option<ItemStack>,
    off: Option<ItemStack>,
) -> Inventory {
    let mut inventory = Inventory::with_hotbar([const { None }; HOTBAR_SLOTS]);
    inventory.set(0, held);
    inventory.set(1, second);
    inventory.set(OFF_HAND, off);
    inventory
}

fn nothing() -> Inventory {
    holding(None, None, None)
}

fn one(item: i32) -> Inventory {
    holding(Some(stack(item, 1)), None, None)
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The sea level the worker gives a region of each dimension (plan 17.1: the
/// dimensions' noise settings).
fn sea_level(dimension: Dimension) -> i32 {
    match dimension {
        Dimension::Overworld => 63,
        Dimension::Nether => 32,
        Dimension::End => 0,
    }
}

fn biome_id(name: &str) -> u16 {
    let biomes = clustine_data::SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .expect("the biome registry");
    let at = biomes
        .entries
        .iter()
        .position(|entry| *entry == name)
        .unwrap_or_else(|| panic!("{name} is a biome"));
    u16::try_from(at).expect("few")
}

fn is_lava(state: BlockState) -> bool {
    state.block_id() == blocks::LAVA.block_id()
}

fn is_water(state: BlockState) -> bool {
    state.block_id() == blocks::WATER.block_id()
}

fn is_fire(state: BlockState) -> bool {
    state.block_id() == blocks::FIRE.block_id()
}

/// What a test's world holds beyond the floor.
#[derive(Clone, Default)]
struct World {
    blocks: Vec<(BlockPos, BlockState)>,
    /// Where the floor is left out, as positions at the floor's height.
    holes: Vec<BlockPos>,
    /// The biome every block is in: the badlands (id 0) where a test does not say.
    biome: u16,
    /// A village over the home chunk: its piece and a bell whose meeting point is taken
    /// (what makes a village of it, as `raids_n10_tests.rs` has it).
    village: bool,
    /// A swamp hut whose box is over all the land near the origin.
    hut: bool,
}

/// Where a village's raid is, in the home chunk on the floor, and its bell.
const CENTRE: BlockPos = BlockPos::new(8, FEET, 8);
const BELL: BlockPos = BlockPos::new(8, FEET, 4);

impl World {
    fn of(blocks: &[(BlockPos, BlockState)]) -> Self {
        Self {
            blocks: blocks.to_vec(),
            ..Self::default()
        }
    }

    fn chunk(&self, dimension: Dimension, position: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(dimension.rules(), Biome(self.biome));
        for z in 0..16 {
            for x in 0..16 {
                let at = bp(
                    position.x * 16 + x as i32,
                    FLOOR,
                    position.z * 16 + z as i32,
                );
                if !self.holes.contains(&at) {
                    chunk.set(x, FLOOR, z, blocks::STONE);
                }
            }
        }
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
        if self.hut {
            let all = KeptBox {
                min: bp(-64, FLOOR, -64),
                max: bp(64, FLOOR + 11, 64),
            };
            chunk.set_structures(vec![KeptStructure {
                kind: KeptKind::SwampHut,
                source: ChunkPos::new(0, 0),
                bounds: all,
                pieces: vec![all],
            }]);
        }
        if self.village && position == ChunkPos::new(0, 0) {
            let piece = KeptBox {
                min: bp(0, FLOOR, 0),
                max: bp(15, FLOOR + 11, 15),
            };
            chunk.set_structures(vec![KeptStructure {
                kind: KeptKind::VillagePlains,
                source: position,
                bounds: piece,
                pieces: vec![piece],
            }]);
            let (x, z) = BELL.in_chunk();
            chunk.set(x, BELL.y, z, blocks::BELL);
            assert!(chunk.acquire_ticket(clustine_world::InChunk::new(x, BELL.y, z)));
        }
        chunk
    }
}

#[derive(Clone)]
struct Options {
    dimension: Dimension,
    clock: Option<WorldClock>,
    inventory: Inventory,
    feet: Vec3,
    random_tick_speed: u32,
    spawn_mobs: bool,
    time_since_rest: u32,
    effects: Vec<EffectInstance>,
    raid_omen_position: Option<BlockPos>,
    pinned: ChunkArea,
    held: Vec<ChunkPos>,
    loaded: Vec<ChunkPos>,
    /// Chunks of other regions the region has copies of, held by [`OTHER`].
    copies: Vec<ChunkPos>,
    block: u32,
}

impl Options {
    fn in_(dimension: Dimension) -> Self {
        Self {
            dimension,
            clock: Some(clock(NOON)),
            inventory: nothing(),
            feet: ORIGIN,
            random_tick_speed: 0,
            spawn_mobs: false,
            time_since_rest: 0,
            effects: Vec::new(),
            raid_omen_position: None,
            pinned: ChunkArea::EVERYWHERE,
            held: around(3),
            loaded: around(2),
            copies: Vec::new(),
            block: 1,
        }
    }

    fn at(mut self, day: u64) -> Self {
        self.clock = Some(clock(day));
        self
    }

    fn holding(mut self, inventory: Inventory) -> Self {
        self.inventory = inventory;
        self
    }

    fn standing(mut self, feet: Vec3) -> Self {
        self.feet = feet;
        self
    }

    fn random_ticks(mut self, speed: u32) -> Self {
        self.random_tick_speed = speed;
        self
    }

    /// One side of a pinned line between the blocks x = 15 and x = 16, holding and
    /// loading its side, with copies of the other's chunks near it.
    fn side(mut self, west: bool) -> Self {
        let mine = move |chunk: &ChunkPos| if west { chunk.x <= 0 } else { chunk.x >= 1 };
        self.pinned = if west {
            ChunkArea {
                min_x: None,
                max_x: Some(1),
            }
        } else {
            ChunkArea {
                min_x: Some(1),
                max_x: None,
            }
        };
        let wider: Vec<ChunkPos> = (-3..=4)
            .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let near: Vec<ChunkPos> = (-2..=3)
            .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
            .collect();
        self.held = wider.into_iter().filter(mine).collect();
        self.loaded = near.iter().copied().filter(mine).collect();
        self.copies = near.iter().copied().filter(|chunk| !mine(chunk)).collect();
        self.block = if west { 1 } else { 2 };
        self
    }
}

fn clock(day: u64) -> WorldClock {
    WorldClock {
        game_time: day,
        day,
    }
}

fn config(options: &Options) -> RegionConfig {
    let rules = options.dimension.rules();
    RegionConfig {
        homes: Vec::new(),
        dimension: options.dimension,
        spawn: Some(ORIGIN),
        starting_inventory: options.inventory.clone(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 11,
        game_seed: 0,
        sea_level: sea_level(options.dimension),
        spawn_mobs: options.spawn_mobs,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: options.random_tick_speed,
    }
}

struct Scene {
    region: Region,
    options: Options,
    input: u64,
    sequence: i32,
    entity: Option<EntityId>,
}

impl Scene {
    /// A region of `options` with `world`, nobody in it.
    fn empty(options: &Options, world: &World) -> Self {
        let mut state = RegionState::new(EntityIds::block(options.block).expect("a block"));
        state.edges.insert(
            EDGE,
            EdgeState {
                start: 1,
                ..EdgeState::default()
            },
        );
        let holdings = Holdings {
            held: options.held.clone(),
            pinned: vec![options.pinned],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut region = Region::restore(config(options), state, holdings);
        region.tick(&TickInputs {
            tickets_added: options
                .loaded
                .iter()
                .chain(&options.copies)
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            clock: options.clock,
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: options
                .loaded
                .iter()
                .map(|position| (*position, world.chunk(options.dimension, *position)))
                .collect(),
            copy_bases: options
                .copies
                .iter()
                .map(|position| (*position, world.chunk(options.dimension, *position)))
                .collect(),
            foreign: options
                .copies
                .iter()
                .map(|position| (*position, OTHER))
                .collect(),
            ..TickInputs::default()
        });
        for chunk in &options.loaded {
            assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
        }
        Self {
            region,
            options: options.clone(),
            input: 0,
            sequence: 0,
            entity: None,
        }
    }

    /// A region of `options` with `world`, and player 1 standing where the options say.
    fn new(options: &Options, world: &World) -> Self {
        let mut scene = Self::empty(options, world);
        let feet = options.feet;
        scene.join(1, feet);
        scene
    }

    fn join(&mut self, n: u128, feet: Vec3) {
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
                    dimension: self.options.dimension,
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
                    inventory: self.options.inventory.clone(),
                    selected_slot: 0,
                    ender: EnderChest::default(),
                    effects: self.options.effects.clone(),
                    time_since_rest: self.options.time_since_rest,
                    raid_omen_position: self.options.raid_omen_position,
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
        if n == 1 {
            self.entity = Some(entity);
        }
    }

    fn input(&mut self, input: PlayerInput) -> TickOutput {
        self.input += 1;
        let entity = self.entity.expect("player 1 is in");
        self.region.tick(&TickInputs {
            inputs: vec![(EDGE, player(1), entity, self.input, input)],
            ..TickInputs::default()
        })
    }

    fn idle(&mut self) -> TickOutput {
        self.region.tick(&TickInputs::default())
    }

    /// Idles until `done` holds, for at most `ticks` ticks: the number of ticks idled,
    /// or `None` if it never did.
    fn idle_until(
        &mut self,
        ticks: u32,
        mut done: impl FnMut(&Scene, &TickOutput) -> bool,
    ) -> Option<u32> {
        for tick in 1..=ticks {
            let output = self.idle();
            if done(self, &output) {
                return Some(tick);
            }
        }
        None
    }

    fn use_on(&mut self, position: BlockPos, face: Face) -> PlayerInput {
        self.sequence += 1;
        PlayerInput::UseItemOn {
            position,
            face,
            cursor: cursor(face),
            sneaking: false,
            sequence: self.sequence,
        }
    }

    /// Player 1 clicks `face` of `position` with what they hold in the main hand.
    fn click(&mut self, position: BlockPos, face: Face) -> TickOutput {
        let input = self.use_on(position, face);
        self.input(input)
    }

    /// Player 1 clicks with the main hand and, as a client does when that did nothing,
    /// with the off hand, in one tick.
    fn click_both(&mut self, position: BlockPos, face: Face) -> TickOutput {
        let main = self.use_on(position, face);
        let off = self.use_on(position, face);
        self.input += 2;
        let entity = self.entity.expect("player 1 is in");
        self.region.tick(&TickInputs {
            inputs: vec![
                (EDGE, player(1), entity, self.input - 1, main),
                (
                    EDGE,
                    player(1),
                    entity,
                    self.input,
                    PlayerInput::WithOffHand(Box::new(off)),
                ),
            ],
            ..TickInputs::default()
        })
    }

    /// Player 1, standing where they stand, uses what they hold looking at the middle
    /// of the top of the block at `at` (a bucket finds its block by a ray).
    fn use_towards_top_of(&mut self, at: BlockPos) -> TickOutput {
        let feet = self.region.player(player(1)).expect("there").1.position;
        let dx = f64::from(at.x) + 0.5 - feet.x;
        let dy = f64::from(at.y) + 1.0 - (feet.y + 1.62);
        let dz = f64::from(at.z) + 0.5 - feet.z;
        let length = (dx * dx + dy * dy + dz * dz).sqrt();
        let yaw = (-dx).atan2(dz).to_degrees() as f32;
        let pitch = (-(dy / length).asin()).to_degrees() as f32;
        self.sequence += 1;
        let sequence = self.sequence;
        self.input(PlayerInput::UseItem {
            rotation: (yaw, pitch),
            sneaking: false,
            sequence,
        })
    }

    fn dig(&mut self, position: BlockPos) -> TickOutput {
        self.sequence += 1;
        let sequence = self.sequence;
        self.input(PlayerInput::Dig { position, sequence })
    }

    fn select(&mut self, slot: u8) -> TickOutput {
        self.input(PlayerInput::SelectSlot { slot })
    }

    fn block(&self, at: BlockPos) -> BlockState {
        let chunk = self
            .region
            .chunk(at.chunk())
            .or_else(|| self.region.copy(at.chunk()))
            .unwrap_or_else(|| panic!("{at:?} is in a loaded chunk"));
        chunk
            .get(
                at.x.rem_euclid(16) as usize,
                at.y,
                at.z.rem_euclid(16) as usize,
            )
            .expect("in the world")
    }

    fn entities(&self) -> Vec<Entity> {
        self.region
            .loaded_chunks()
            .into_iter()
            .flat_map(|chunk| {
                self.region
                    .chunk(chunk)
                    .expect("loaded")
                    .entities()
                    .to_vec()
            })
            .collect()
    }

    fn of_kind(&self, kind: i32) -> Vec<Entity> {
        self.entities()
            .into_iter()
            .filter(|entity| i32::from(entity.kind) == kind)
            .collect()
    }

    fn record(&self, key: EntityId) -> Option<Entity> {
        self.entities().into_iter().find(|entity| entity.key == key)
    }

    /// Player 1 uses the spawn egg in slot `slot` on the floor at (x, z): the mob it
    /// made.
    fn hatch(&mut self, slot: u8, x: i32, z: i32) -> EntityId {
        let before: Vec<EntityId> = self.entities().iter().map(|entity| entity.key).collect();
        self.select(slot);
        self.click(bp(x, FLOOR, z), Face::Top);
        let made: Vec<EntityId> = self
            .entities()
            .iter()
            .map(|entity| entity.key)
            .filter(|key| !before.contains(key))
            .collect();
        assert_eq!(made.len(), 1, "the egg made one mob: {made:?}");
        made[0]
    }

    fn respawn(&self) -> Option<Respawn> {
        self.region
            .player_state(player(1))
            .expect("player 1 is there")
            .respawn
    }

    /// Player 1 opens the container at `at`: the serial of its menu and its slots.
    fn open(&mut self, at: BlockPos) -> (u64, Vec<Option<ItemStack>>) {
        let serial = self.input + 1;
        let output = self.click(at, Face::Top);
        let slots = output
            .player_events
            .iter()
            .find_map(|(_, event)| match event {
                PlayerEvent::MenuOpened {
                    serial: opened,
                    slots,
                    ..
                } if *opened == serial => Some(slots.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a menu opened: {:?}", output.player_events));
        (serial, slots)
    }

    fn close(&mut self, serial: u64) {
        self.input(PlayerInput::CloseContainer { serial });
    }
}

fn cursor(face: Face) -> [f32; 3] {
    match face {
        Face::Bottom => [0.5, 0.0, 0.5],
        Face::Top => [0.5, 1.0, 0.5],
        Face::North => [0.5, 0.5, 0.0],
        Face::South => [0.5, 0.5, 1.0],
        Face::West => [0.0, 0.5, 0.5],
        Face::East => [1.0, 0.5, 0.5],
    }
}

// ------------------------------------------------------------------------------------
// What the region reports.

fn sounds_of(output: &TickOutput, sound: i32) -> Vec<(Vec3, f32)> {
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

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent { kind, position, .. } => Some((*kind, *position)),
            _ => None,
        })
        .collect()
}

fn explosions(output: &TickOutput) -> Vec<([f64; 3], f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion { centre, radius, .. } => Some((*centre, *radius)),
            _ => None,
        })
        .collect()
}

fn centre(at: BlockPos) -> [f64; 3] {
    [
        f64::from(at.x) + 0.5,
        f64::from(at.y) + 0.5,
        f64::from(at.z) + 0.5,
    ]
}

fn health(mob: &Entity) -> f32 {
    match mob_values(mob.kind, &mob.data)
        .into_iter()
        .find(|(at, _, _)| *at == index::HEALTH)
        .map(|(_, value, _)| value)
    {
        Some(DataShown::Float(health)) => health,
        other => panic!("a mob shows its health: {other:?}"),
    }
}

/// The fire within `reach` blocks of `at`, about the floor.
fn fires_near(scene: &Scene, at: BlockPos, reach: i32) -> usize {
    let mut found = 0;
    for x in at.x - reach..=at.x + reach {
        for y in FLOOR - 3..=FEET + reach {
            for z in at.z - reach..=at.z + reach {
                if is_fire(scene.block(bp(x, y, z))) {
                    found += 1;
                }
            }
        }
    }
    found
}

// ------------------------------------------------------------------------------------
// Day and night (plan 17.5, their 1 and 2; brief 1.2 `fixed_time`).

/// Plan 17.5: `isBrightOutside` is `!hasFixedTime && skyDarken < 4` and `isDarkOutside`
/// never holds in a level with a fixed time; a level with a fixed time darkens by its
/// own `15 − sky_light_level` (the Nether's 4, so 11; the End's none, so 0), whatever
/// the overworld's clock says.
#[test]
fn only_the_overworld_has_a_bright_or_a_dark_outside_and_the_others_darken_by_their_own_light() {
    for day in [NOON, MIDNIGHT] {
        assert_eq!(time::level_sky_darken(Dimension::Nether, day), 11);
        assert_eq!(time::level_sky_darken(Dimension::End, day), 0);
        for dimension in [Dimension::Nether, Dimension::End] {
            let darken = time::level_sky_darken(dimension, day);
            assert!(!time::is_bright_outside(dimension, darken), "{dimension:?}");
            assert!(!time::is_dark_outside(dimension, darken), "{dimension:?}");
        }
    }
    let noon = time::level_sky_darken(Dimension::Overworld, NOON);
    let midnight = time::level_sky_darken(Dimension::Overworld, MIDNIGHT);
    assert_eq!(noon, time::sky_darken(NOON));
    assert!(time::is_bright_outside(Dimension::Overworld, noon));
    assert!(!time::is_dark_outside(Dimension::Overworld, noon));
    assert!(!time::is_bright_outside(Dimension::Overworld, midnight));
    assert!(time::is_dark_outside(Dimension::Overworld, midnight));
}

/// An enderman made at noon under the open sky in `dimension`: the tick of its
/// teleport's sound, if any within `ticks` (`Enderman.customServerAiStep`: 600 ticks
/// after its target last changed, then about one tick in 25 under a bright sky).
fn enderman_teleports(dimension: Dimension, ticks: u32) -> Option<u32> {
    let options = Options::in_(dimension)
        .at(NOON)
        .holding(one(items::ENDERMAN_SPAWN_EGG))
        .standing(Vec3::new(0.5, FEET as f64, 6.5));
    let mut scene = Scene::new(&options, &World::default());
    // Behind player 1, who looks south: not stared at.
    scene.hatch(0, 0, 4);
    scene.idle_until(ticks, |_, output| {
        !sounds_of(output, sounds::ENTITY_ENDERMAN_TELEPORT).is_empty()
    })
}

/// Plan 6 and 17.5 (their tests: "an enderman under the End's sky stays"); brief 5.8:
/// only the daylight teleport depends on the dimension. Under the overworld's noon sky an
/// enderman teleports away within its 600 ticks and a few hundred more; under the End's
/// open sky, as bright, and in the Nether, it never does.
#[test]
fn an_enderman_flees_the_noon_sky_in_the_overworld_and_never_in_the_end_or_the_nether() {
    assert!(
        enderman_teleports(Dimension::Overworld, 1_500).is_some(),
        "the control: an enderman teleports at noon in the overworld"
    );
    for dimension in [Dimension::End, Dimension::Nether] {
        assert_eq!(
            enderman_teleports(dimension, 1_500),
            None,
            "no daylight teleport in {dimension:?}"
        );
    }
}

/// Plan 17.5, their 1: the undead burn where `isBrightOutside`, which no level with a
/// fixed time is: a zombie at the overworld's noon under the open sky catches fire
/// within 400 ticks (0.04 a tick); one under the End's open sky, as light, and one in
/// the Nether never do.
#[test]
fn a_zombie_burns_at_noon_in_the_overworld_and_never_in_the_end_or_the_nether() {
    for dimension in Dimension::ALL {
        let options = Options::in_(dimension)
            .at(NOON)
            .holding(one(items::ZOMBIE_SPAWN_EGG));
        let mut scene = Scene::new(&options, &World::default());
        let zombie = scene.hatch(0, 3, 3);
        let burnt = scene.idle_until(400, |scene, _| {
            let found = scene.record(zombie).expect("the zombie stays in the scene");
            found.fire_ticks > 0
        });
        match dimension {
            Dimension::Overworld => assert!(burnt.is_some(), "the control: it burns"),
            _ => assert_eq!(burnt, None, "no zombie burns by day in {dimension:?}"),
        }
    }
}

/// Plan 17.5, their 1 and 2: `isDarkOutside` never holds in a level with a fixed time,
/// so neither the Nether (darkening 11) nor the End is ever night: a wandering trader,
/// which drinks invisibility where it is dark outside (`WanderingTrader`'s
/// `UseItemGoal`), drinks it at the overworld's midnight in the overworld and never in
/// the Nether or the End.
#[test]
fn a_wandering_trader_drinks_invisibility_at_midnight_in_the_overworld_only() {
    for dimension in Dimension::ALL {
        let world = World {
            biome: biome_id("minecraft:plains"),
            ..World::default()
        };
        let options = Options::in_(dimension)
            .at(MIDNIGHT)
            .holding(one(items::WANDERING_TRADER_SPAWN_EGG));
        let mut scene = Scene::new(&options, &world);
        let trader = scene.hatch(0, 3, 3);
        let drunk = scene.idle_until(300, |scene, _| {
            scene
                .record(trader)
                .is_some_and(|trader| mob_shared_flags(&trader.data) & INVISIBLE != 0)
        });
        match dimension {
            Dimension::Overworld => assert!(drunk.is_some(), "the control: it drinks"),
            _ => assert_eq!(drunk, None, "no night in {dimension:?}"),
        }
    }
}

// ------------------------------------------------------------------------------------
// Lava (plan 17.2 B, 17.5 their 3; `LavaFluid.txt` 317–388: 10, 1, 4 where fast).

/// Player 1 empties a lava bucket onto the floor at the origin from beside it.
fn lava_at_origin(dimension: Dimension, world: &World) -> Scene {
    let options = Options::in_(dimension)
        .holding(one(items::LAVA_BUCKET))
        .standing(Vec3::new(0.5, FEET as f64, -2.5));
    let mut scene = Scene::new(&options, world);
    scene.use_towards_top_of(bp(0, FLOOR, 0));
    assert!(is_lava(scene.block(bp(0, FEET, 0))), "a source of lava");
    scene
}

/// Plan 17.2 B: lava's tick delay is 10 where `fast_lava` (the Nether) and 30 elsewhere:
/// the source placed by a bucket spreads to its neighbour that many ticks after.
#[test]
fn lava_spreads_after_10_ticks_in_the_nether_and_30_elsewhere() {
    let mut taken = Vec::new();
    for dimension in Dimension::ALL {
        let mut scene = lava_at_origin(dimension, &World::default());
        let ticks = scene
            .idle_until(60, |scene, _| is_lava(scene.block(bp(1, FEET, 0))))
            .expect("the lava spreads");
        taken.push((dimension, ticks));
    }
    // Counted from the tick after the bucket's: the delay less one.
    assert_eq!(
        taken,
        [
            (Dimension::Overworld, 29),
            (Dimension::Nether, 9),
            (Dimension::End, 29)
        ]
    );
}

/// Plan 17.2 B: the drop-off is 1 where `fast_lava`, 2 elsewhere: on a flat floor a
/// source's lava reaches 7 blocks out in the Nether and 3 elsewhere, and no further.
#[test]
fn lava_runs_seven_blocks_in_the_nether_and_three_elsewhere() {
    for dimension in Dimension::ALL {
        let mut scene = lava_at_origin(dimension, &World::default());
        scene.idle_until(400, |_, _| false);
        let reach = if dimension == Dimension::Nether { 7 } else { 3 };
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1)] {
            let last = bp(dx * reach, FEET, dz * reach);
            let beyond = bp(dx * (reach + 1), FEET, dz * (reach + 1));
            assert!(
                is_lava(scene.block(last)),
                "{dimension:?}: lava at {last:?}: {:?}",
                scene.block(last)
            );
            assert!(
                !is_lava(scene.block(beyond)),
                "{dimension:?}: no lava at {beyond:?}"
            );
        }
    }
}

/// Plan 17.2 B: the slope-find distance is 4 where `fast_lava`, 2 elsewhere
/// (`FlowingFluid.getSlopeDistance`): a source flows only towards a hole in the floor it
/// finds, and every way where it finds none. A hole under x = 3 is found at 2 and one
/// under x = 4 is not; one under x = 5 is found at 4 and one under x = 6 is not.
#[test]
fn lava_looks_for_a_hole_four_blocks_out_in_the_nether_and_two_elsewhere() {
    let cases = [
        (Dimension::Overworld, 3, true),
        (Dimension::Overworld, 4, false),
        (Dimension::End, 3, true),
        (Dimension::End, 4, false),
        (Dimension::Nether, 5, true),
        (Dimension::Nether, 6, false),
    ];
    for (dimension, hole, found) in cases {
        let world = World {
            holes: vec![bp(hole, FLOOR, 0)],
            ..World::default()
        };
        let mut scene = lava_at_origin(dimension, &world);
        scene
            .idle_until(60, |scene, _| is_lava(scene.block(bp(1, FEET, 0))))
            .expect("the lava spreads east");
        let others =
            [bp(-1, FEET, 0), bp(0, FEET, 1), bp(0, FEET, -1)].map(|at| is_lava(scene.block(at)));
        if found {
            assert_eq!(
                others, [false; 3],
                "{dimension:?}: towards the hole under x = {hole} only"
            );
        } else {
            assert_eq!(
                others, [true; 3],
                "{dimension:?}: no hole found under x = {hole}, so every way"
            );
        }
    }
}

/// Plan 17.5, their 3 and their tests: "lava in the Nether spreads fast across a region
/// line". The western region of a pinned line between x = 15 and 16 spreads lava into
/// the eastern's land as an effect; the eastern, taking it, runs that lava on at its own
/// dimension's pace: the lava filed at x = 16 reaches x = 17 twenty ticks sooner in the
/// Nether than in the overworld, after about the Nether's 10.
#[test]
fn lava_spread_across_a_region_line_runs_on_at_the_nether_s_pace() {
    let mut taken = Vec::new();
    for dimension in [Dimension::Overworld, Dimension::Nether] {
        let west_options = Options::in_(dimension)
            .side(true)
            .holding(one(items::LAVA_BUCKET))
            .standing(Vec3::new(12.5, FEET as f64, 0.5));
        let mut west = Scene::new(&west_options, &World::default());
        west.use_towards_top_of(bp(15, FLOOR, 0));
        assert!(is_lava(west.block(bp(15, FEET, 0))));
        let mut made = Vec::new();
        for _ in 0..80 {
            let output = west.idle();
            if output
                .effects
                .iter()
                .any(|effect| effect.at == bp(16, FEET, 0))
            {
                made = output.effects.clone();
                break;
            }
        }
        assert!(
            made.iter()
                .any(|effect| matches!(effect.what, Effected::Spread { .. })),
            "the west spreads lava across the line: {made:?}"
        );

        // The east's copy of the west's chunk shows the source, as the west's commit
        // brings it.
        let copied = World::of(&[(bp(15, FEET, 0), west.block(bp(15, FEET, 0)))]);
        let mut east = Scene::empty(&Options::in_(dimension).side(false), &copied);
        east.region.tick(&TickInputs {
            effects: made
                .iter()
                .enumerate()
                .map(|(index, effect)| FiledEffect {
                    id: EffectId {
                        sequence: 1,
                        index: u32::try_from(index).expect("few"),
                    },
                    effect: effect.clone(),
                    due: Due::FromTaking,
                })
                .collect(),
            ..TickInputs::default()
        });
        assert!(
            is_lava(east.block(bp(16, FEET, 0))),
            "{dimension:?}: the east takes the lava"
        );
        let ticks = east
            .idle_until(80, |scene, _| is_lava(scene.block(bp(17, FEET, 0))))
            .expect("the lava runs on in the east");
        taken.push((dimension, ticks));
    }
    let [(_, overworld), (_, nether)] = taken[..] else {
        unreachable!()
    };
    assert_eq!(overworld - nether, 20, "{taken:?}");
    assert!((9..=11).contains(&nether), "{taken:?}");
}

// ------------------------------------------------------------------------------------
// Water (plan 17.2 A, 17.5 their 9; brief 1.2 `water_evaporates`).

/// Brief 1.2 (BucketItem.emptyContents): where water evaporates, emptying a water bucket
/// places nothing and plays `fire_extinguish` at volume 0.5; elsewhere it places a
/// source.
#[test]
fn a_water_bucket_emptied_in_the_nether_fizzes_and_places_nothing() {
    for dimension in Dimension::ALL {
        let options = Options::in_(dimension).holding(one(items::WATER_BUCKET));
        let mut scene = Scene::new(&options, &World::default());
        let output = scene.use_towards_top_of(bp(2, FLOOR, 0));
        let target = bp(2, FEET, 0);
        let fizz = sounds_of(&output, sounds::BLOCK_FIRE_EXTINGUISH);
        if dimension == Dimension::Nether {
            assert_eq!(scene.block(target), blocks::AIR, "no water in the Nether");
            assert_eq!(fizz.len(), 1, "one fizz: {:?}", output.events);
            assert_eq!(fizz[0].1, 0.5);
            scene.idle_until(40, |_, _| false);
            for x in 0..=4 {
                for z in -2..=2 {
                    assert!(!is_water(scene.block(bp(x, FEET, z))));
                }
            }
        } else {
            assert!(is_water(scene.block(target)), "{dimension:?}: a source");
            assert!(fizz.is_empty(), "{dimension:?}: no fizz");
        }
    }
}

/// Brief 1.2 (inferred) and plan 17.2 A: a dispenser emptying a water bucket where water
/// evaporates places nothing and fizzes (`emptyContents` with no entity), and keeps the
/// empty bucket as anywhere; elsewhere water is placed in front.
#[test]
fn a_dispenser_s_water_bucket_evaporates_in_the_nether() {
    let dispenser = bp(2, FEET, 2);
    let front = bp(3, FEET, 2);
    for dimension in Dimension::ALL {
        let world = World::of(&[(
            dispenser,
            state("minecraft:dispenser[facing=east,triggered=false]"),
        )]);
        let options = Options::in_(dimension)
            .holding(holding(
                Some(stack(items::WATER_BUCKET, 1)),
                Some(stack(items::REDSTONE_BLOCK, 1)),
                None,
            ))
            .standing(Vec3::new(0.5, FEET as f64, 0.5));
        let mut scene = Scene::new(&options, &world);
        let (serial, _) = scene.open(dispenser);
        let hotbar = i16::try_from(MenuKind::Grid3x3.container_slots() + 27).expect("fits");
        scene.input(PlayerInput::ContainerClick {
            serial,
            slot: hotbar,
            button: 0,
            kind: ClickKind::QuickMove,
        });
        scene.close(serial);
        scene.select(1);
        // A redstone block north of the dispenser powers it.
        scene.click(bp(2, FLOOR, 1), Face::Top);
        assert_eq!(scene.block(bp(2, FEET, 1)), blocks::REDSTONE_BLOCK);
        let mut fizzes = 0;
        let fired = scene.idle_until(10, |_, output| {
            fizzes += sounds_of(output, sounds::BLOCK_FIRE_EXTINGUISH).len();
            level_events(output)
                .iter()
                .any(|(kind, at)| *at == dispenser && (*kind == 1000 || *kind == 1001))
        });
        assert!(fired.is_some(), "{dimension:?}: the dispenser fires");
        if dimension == Dimension::Nether {
            assert_eq!(scene.block(front), blocks::AIR, "no water in the Nether");
            assert_eq!(fizzes, 1, "the fizz");
        } else {
            assert!(
                is_water(scene.block(front)),
                "{dimension:?}: water in front"
            );
            assert_eq!(fizzes, 0);
        }
        scene.select(0);
        let (serial, slots) = scene.open(dispenser);
        scene.close(serial);
        assert_eq!(
            slots[0],
            Some(stack(items::BUCKET, 1)),
            "{dimension:?}: the empty bucket stays"
        );
    }
}

/// Plan 17.5, their 9 and their tests ("ice broken in creative leaves air"): ice broken
/// by a player turns to water only where the break makes drops, never in creative,
/// which every player is: so air in every dimension.
#[test]
fn ice_broken_by_a_creative_player_leaves_air_in_every_dimension() {
    for dimension in Dimension::ALL {
        let ice = bp(2, FEET, 0);
        let mut scene = Scene::new(&Options::in_(dimension), &World::of(&[(ice, blocks::ICE)]));
        scene.dig(ice);
        assert_eq!(scene.block(ice), blocks::AIR, "{dimension:?}");
        scene.idle_until(10, |_, _| false);
        assert_eq!(
            scene.block(ice),
            blocks::AIR,
            "{dimension:?}: no water after"
        );
    }
}

/// Brief 1.2 (IceBlock 20–100) and plan 17.2 A: ice melted by a block's light above 11
/// less its dampening becomes water (it stands on stone), and where water evaporates it
/// is removed and leaves nothing. A glowstone beside the ice lights it with 14.
#[test]
fn ice_melted_by_light_becomes_water_and_in_the_nether_air() {
    let ice = bp(2, FEET, 2);
    for dimension in Dimension::ALL {
        let world = World::of(&[(ice, blocks::ICE), (bp(3, FEET, 2), blocks::GLOWSTONE)]);
        let options = Options::in_(dimension).random_ticks(4096);
        let mut scene = Scene::new(&options, &world);
        let melted = scene.idle_until(600, |scene, _| scene.block(ice) != blocks::ICE);
        assert!(melted.is_some(), "{dimension:?}: the ice melts");
        if dimension == Dimension::Nether {
            assert_eq!(scene.block(ice), blocks::AIR, "the Nether: nothing left");
        } else {
            assert!(
                is_water(scene.block(ice)),
                "{dimension:?}: water: {:?}",
                scene.block(ice)
            );
        }
    }
}

/// Plan 17.5, their 9 (PointedDripstoneBlock 519–545): mud on the block a stalactite
/// hangs from dries to clay by its drip, its water given up, but not where water
/// evaporates, where the mud gives no water: there it stays mud.
#[test]
fn mud_over_dripstone_dries_to_clay_except_in_the_nether() {
    // The mud stands on the block the stalactite hangs from.
    let mud = bp(2, FEET + 4, 2);
    let root = bp(2, FEET + 3, 2);
    let tip = bp(2, FEET + 2, 2);
    let dripstone = state(
        "minecraft:pointed_dripstone[thickness=tip,vertical_direction=down,waterlogged=false]",
    );
    for dimension in Dimension::ALL {
        let world = World::of(&[(mud, blocks::MUD), (root, blocks::STONE), (tip, dripstone)]);
        let options = Options::in_(dimension).random_ticks(4096);
        let mut scene = Scene::new(&options, &world);
        let dried = scene.idle_until(400, |scene, _| scene.block(mud) != blocks::MUD);
        if dimension == Dimension::Nether {
            assert_eq!(dried, None, "the Nether's mud stays mud");
        } else {
            assert!(dried.is_some(), "{dimension:?}: the mud dries");
            assert_eq!(scene.block(mud), blocks::CLAY, "{dimension:?}");
        }
    }
}

// ------------------------------------------------------------------------------------
// Beds (plan 17.2 C, 14; brief 1.2 `bed_works`, 5.3; BedBlock 32–75, StrawBedBlock).

/// A bed facing north, its head at (2, 41, 0) and its foot at (2, 41, 1).
const HEAD: BlockPos = BlockPos::new(2, FEET, 0);
const FOOT: BlockPos = BlockPos::new(2, FEET, 1);

fn bed(kind: &str, part: &str) -> BlockState {
    state(&format!("minecraft:{kind}[facing=north,part={part}]"))
}

fn bedroom(kind: &str, dimension: Dimension, day: u64, inventory: Inventory) -> Scene {
    let world = World::of(&[(HEAD, bed(kind, "head")), (FOOT, bed(kind, "foot"))]);
    let options = Options::in_(dimension)
        .at(day)
        .holding(inventory)
        .standing(Vec3::new(4.5, FEET as f64, 0.5));
    Scene::new(&options, &world)
}

/// Brief 1.2 and plan 17.2 C, 14: in the Nether and the End a bed's rule is
/// `destroy_on_use`: using it removes both halves and explodes at the head's centre
/// with power 5 and fire, hurting nobody until survival (a cow beside it keeps its
/// health), and sets no spawn (`can_set_spawn: never`).
#[test]
fn a_bed_used_in_the_nether_or_the_end_is_removed_and_explodes_with_fire_hurting_nobody() {
    for dimension in [Dimension::Nether, Dimension::End] {
        for day in [NOON, MIDNIGHT] {
            let mut scene = bedroom("red_bed", dimension, day, one(items::COW_SPAWN_EGG));
            let cow = scene.hatch(0, 2, 3);
            let full = health(&scene.record(cow).expect("the cow"));
            scene.select(1);
            let output = scene.click(FOOT, Face::Top);
            assert_eq!(scene.block(HEAD), blocks::AIR, "{dimension:?}");
            assert_eq!(scene.block(FOOT), blocks::AIR, "{dimension:?}");
            assert_eq!(
                explosions(&output),
                [(centre(HEAD), 5.0)],
                "{dimension:?}: one explosion of power 5 at the head"
            );
            assert!(
                fires_near(&scene, HEAD, 6) > 0,
                "{dimension:?}: the explosion lights fires"
            );
            assert_eq!(scene.respawn(), None, "{dimension:?}: no spawn set");
            scene.idle();
            let after = scene.record(cow).expect("the cow is not killed");
            assert_eq!(health(&after), full, "{dimension:?}: nobody hurt");
        }
    }
}

/// The control of the rule above, and brief 1.2: the overworld's bed sets the spawn at
/// its head when used by day and never explodes.
#[test]
fn a_bed_used_by_day_in_the_overworld_sets_the_spawn_and_stands() {
    let mut scene = bedroom("red_bed", Dimension::Overworld, NOON, nothing());
    let output = scene.click(FOOT, Face::Top);
    assert!(explosions(&output).is_empty());
    assert_eq!(scene.block(HEAD), bed("red_bed", "head"));
    let respawn = scene.respawn().expect("a spawn set");
    assert_eq!(
        (respawn.pos, respawn.dimension),
        (HEAD, Dimension::Overworld)
    );
}

/// Plan 17.2 C (`straw_bed_rule`) and StrawBedBlock's `destroyOnUse`: a straw bed used in
/// the Nether or the End is destroyed (its `destroyBed`: `straw_bed_break_leave` and air,
/// the other half following) without an explosion.
#[test]
fn a_straw_bed_used_in_the_nether_or_the_end_is_destroyed_without_an_explosion() {
    for dimension in [Dimension::Nether, Dimension::End] {
        let mut scene = bedroom("straw_bed", dimension, MIDNIGHT, nothing());
        let output = scene.click(FOOT, Face::Top);
        let mut outputs = vec![output];
        outputs.extend((0..3).map(|_| scene.idle()));
        assert!(
            outputs.iter().all(|output| explosions(output).is_empty()),
            "{dimension:?}: no explosion"
        );
        assert_eq!(scene.block(HEAD), blocks::AIR, "{dimension:?}");
        assert_eq!(scene.block(FOOT), blocks::AIR, "{dimension:?}");
        let heard: usize = outputs
            .iter()
            .map(|output| sounds_of(output, sounds::BLOCK_STRAW_BED_BREAK_LEAVE).len())
            .sum();
        assert_eq!(heard, 1, "{dimension:?}: the straw bed's breaking");
        assert_eq!(scene.respawn(), None);
    }
}

/// The overworld player 1 sleeps in at midnight, and the world's other player 2 in
/// `elsewhere`, awake: whether the night is skipped within 130 ticks.
fn night_skipped_with_another_in(elsewhere: Dimension) -> bool {
    let mut scene = bedroom("red_bed", Dimension::Overworld, MIDNIGHT, nothing());
    scene.region.tick(&TickInputs {
        world_players: Some(vec![WorldPlayer {
            player: player(2),
            entity: EntityId((9 << 32) + 2),
            dimension: elsewhere,
            position: [40.5, f64::from(FEET), 40.5],
            game_mode: 0,
            sleeping: None,
        }]),
        ..TickInputs::default()
    });
    let output = scene.click(FOOT, Face::Top);
    assert!(
        scene
            .region
            .player_state(player(1))
            .expect("there")
            .sleeping
            .is_some(),
        "player 1 sleeps: {:?}",
        output.player_events
    );
    scene
        .idle_until(130, |_, output| output.clock_jumped)
        .is_some()
}

/// Plan 17.5, their 6 and their tests ("the night is skipped with a player in the
/// Nether"): the sleepers are counted among the players of the region's own dimension
/// only, so one player awake in the Nether or the End does not keep the overworld's
/// sleeper from skipping the night, while one awake in the overworld does.
#[test]
fn the_night_is_skipped_with_a_player_awake_in_the_nether_or_the_end() {
    assert!(
        !night_skipped_with_another_in(Dimension::Overworld),
        "the control: one awake in the overworld keeps the night"
    );
    for dimension in [Dimension::Nether, Dimension::End] {
        assert!(
            night_skipped_with_another_in(dimension),
            "one awake in {dimension:?} is not counted"
        );
    }
}

// ------------------------------------------------------------------------------------
// Respawn anchors (plan 17.2 C; brief 5.4; RespawnAnchorBlock and its `$1`).

const ANCHOR: BlockPos = BlockPos::new(2, FEET, 0);

fn anchor(charges: i32) -> BlockState {
    state(&format!("minecraft:respawn_anchor[charges={charges}]"))
}

fn anchor_room(
    dimension: Dimension,
    charges: i32,
    more: &[(BlockPos, BlockState)],
    inventory: Inventory,
) -> Scene {
    let mut placed = vec![(ANCHOR, anchor(charges))];
    placed.extend_from_slice(more);
    let options = Options::in_(dimension)
        .holding(inventory)
        .standing(Vec3::new(4.5, FEET as f64, 0.5));
    Scene::new(&options, &World::of(&placed))
}

/// Brief 5.4: glowstone charges an anchor below 4 charges in every dimension, by one,
/// with the `respawn_anchor_charge` sound; nothing explodes.
#[test]
fn glowstone_charges_an_anchor_in_every_dimension() {
    for dimension in Dimension::ALL {
        let mut scene = anchor_room(dimension, 0, &[], one(items::GLOWSTONE));
        for charges in 1..=4 {
            let output = scene.click(ANCHOR, Face::Top);
            assert_eq!(scene.block(ANCHOR), anchor(charges), "{dimension:?}");
            assert_eq!(
                sounds_of(&output, sounds::BLOCK_RESPAWN_ANCHOR_CHARGE).len(),
                1,
                "{dimension:?}: the charge's sound"
            );
            assert!(explosions(&output).is_empty());
        }
    }
}

/// Brief 5.4: with glowstone in the off hand and the anchor chargeable, the main hand
/// passes (an empty hand does not set the spawn or explode the anchor) and the off hand
/// charges it.
#[test]
fn glowstone_in_the_off_hand_charges_and_the_empty_main_hand_passes() {
    for dimension in Dimension::ALL {
        let inventory = holding(None, None, Some(stack(items::GLOWSTONE, 4)));
        let mut scene = anchor_room(dimension, 1, &[], inventory);
        let output = scene.click_both(ANCHOR, Face::Top);
        assert!(
            explosions(&output).is_empty(),
            "{dimension:?}: no explosion"
        );
        assert_eq!(
            scene.block(ANCHOR),
            anchor(2),
            "{dimension:?}: charged by one"
        );
        assert_eq!(scene.respawn(), None, "{dimension:?}: no spawn set");
    }
}

/// Brief 5.4 and plan 17.2 C: in the Nether an anchor with a charge used empty-handed
/// sets the spawn there (the Nether, the anchor's position) with the
/// `respawn_anchor_set_spawn` sound at its centre, spending no charge; used again at the
/// same spot it sets nothing and plays nothing.
#[test]
fn a_charged_anchor_used_in_the_nether_sets_the_spawn_there() {
    let mut scene = anchor_room(Dimension::Nether, 2, &[], nothing());
    let output = scene.click(ANCHOR, Face::Top);
    assert!(explosions(&output).is_empty());
    assert_eq!(scene.block(ANCHOR), anchor(2), "no charge spent");
    let respawn = scene.respawn().expect("a spawn set");
    assert_eq!(
        (respawn.pos, respawn.dimension),
        (ANCHOR, Dimension::Nether)
    );
    let heard = sounds_of(&output, sounds::BLOCK_RESPAWN_ANCHOR_SET_SPAWN);
    assert_eq!(heard.len(), 1, "{:?}", output.events);
    assert_eq!(heard[0].0, Vec3::new(2.5, f64::from(FEET) + 0.5, 0.5));
    let again = scene.click(ANCHOR, Face::Top);
    assert!(sounds_of(&again, sounds::BLOCK_RESPAWN_ANCHOR_SET_SPAWN).is_empty());
    assert_eq!(scene.respawn(), Some(respawn));
}

/// Brief 5.4 and plan 17.2 C, 14: outside the Nether an anchor with a charge used
/// empty-handed is removed and explodes at its centre with power 5 and fire, breaking
/// the floor under it; it sets no spawn.
#[test]
fn a_charged_anchor_used_outside_the_nether_explodes() {
    for dimension in [Dimension::Overworld, Dimension::End] {
        let mut scene = anchor_room(dimension, 1, &[], nothing());
        let output = scene.click(ANCHOR, Face::Top);
        assert_eq!(
            explosions(&output),
            [(centre(ANCHOR), 5.0)],
            "{dimension:?}: one explosion of power 5 at the anchor"
        );
        assert_eq!(scene.block(ANCHOR), blocks::AIR, "{dimension:?}");
        assert_ne!(
            scene.block(bp(ANCHOR.x, FLOOR, ANCHOR.z)),
            blocks::STONE,
            "{dimension:?}: the floor under it breaks"
        );
        assert!(fires_near(&scene, ANCHOR, 6) > 0, "{dimension:?}: fire");
        assert_eq!(scene.respawn(), None);
    }
}

/// `RespawnAnchorBlock$1`: where water that would flow is beside the anchor (a source)
/// or water is above it, the anchor's own position takes water's resistance, so that
/// every ray dies in it: the explosion breaks nothing, not even the floor under it.
#[test]
fn an_anchor_exploding_beside_or_under_water_breaks_nothing() {
    let beside = [(bp(ANCHOR.x, FEET, ANCHOR.z + 1), blocks::WATER)];
    let above = [(bp(ANCHOR.x, FEET + 1, ANCHOR.z), blocks::WATER)];
    for water in [&beside[..], &above[..]] {
        for dimension in [Dimension::Overworld, Dimension::End] {
            let mut scene = anchor_room(dimension, 1, water, nothing());
            let output = scene.click(ANCHOR, Face::Top);
            assert_eq!(explosions(&output).len(), 1, "{dimension:?}: it explodes");
            assert_eq!(
                scene.block(bp(ANCHOR.x, FLOOR, ANCHOR.z)),
                blocks::STONE,
                "{dimension:?}: the floor under it stands"
            );
            for (dx, dz) in [(-1, 0), (0, -1), (1, -1), (-1, -1)] {
                let floor = bp(ANCHOR.x + dx, FLOOR, ANCHOR.z + dz);
                assert_eq!(scene.block(floor), blocks::STONE, "{dimension:?} {floor:?}");
            }
        }
    }
}

/// RespawnAnchorBlock `useWithoutItem`: an anchor without a charge passes a click, so the
/// block in hand is placed against it, in every dimension, and nothing explodes.
#[test]
fn a_block_is_placed_against_an_empty_anchor() {
    for dimension in Dimension::ALL {
        let mut scene = anchor_room(dimension, 0, &[], one(items::STONE));
        let output = scene.click(ANCHOR, Face::Top);
        assert!(explosions(&output).is_empty(), "{dimension:?}");
        assert_eq!(scene.block(ANCHOR), anchor(0), "{dimension:?}");
        assert_eq!(
            scene.block(bp(ANCHOR.x, FEET + 1, ANCHOR.z)),
            blocks::STONE,
            "{dimension:?}: the stone placed on top"
        );
        assert_eq!(scene.respawn(), None);
    }
}

// ------------------------------------------------------------------------------------
// Light (plan 17.2 D1, 17.5 their 8; brief 1.2 `has_skylight`).

/// The light the region hands over with the chunk at `at`.
fn kept(region: &mut Region, at: ChunkPos) -> Kept {
    region
        .hand_over_light()
        .into_iter()
        .find(|(position, _)| *position == at)
        .unwrap_or_else(|| panic!("light for {at:?}"))
        .1
}

/// Whether light carries any sky: a section that is not absent.
fn has_sky(light: &Kept) -> bool {
    light
        .light
        .sky
        .iter()
        .any(|section| !matches!(section, SectionLight::Absent))
}

/// Plan 17.2 D1: a level without a sky has no sky light; the overworld's and the End's
/// chunks are lit by their sky, full above the floor.
#[test]
fn a_nether_region_s_chunks_carry_no_sky_light_and_the_others_do() {
    for dimension in Dimension::ALL {
        let mut scene = Scene::new(&Options::in_(dimension), &World::default());
        for at in around(1) {
            let light = kept(&mut scene.region, at);
            if dimension == Dimension::Nether {
                assert!(!has_sky(&light), "{at:?}: {:?}", light.light.sky);
            } else {
                assert!(
                    light.light.sky.contains(&SectionLight::Full),
                    "{dimension:?} {at:?}: lit by the sky"
                );
            }
        }
    }
}

/// Hands `chunks` to `region` as the runner hands back the chunks it kept warm at a
/// restore, a merge or a split: wanted first, then given with the light kept for them.
fn hand_back(region: &mut Region, chunks: Vec<(ChunkPos, Chunk)>, kept: Vec<(ChunkPos, Kept)>) {
    region.tick(&TickInputs {
        tickets_added: chunks.iter().map(|(at, _)| (*at, Ticket::Viewer)).collect(),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        chunks_kept: kept,
        ..TickInputs::default()
    });
    region.tick(&TickInputs::default());
}

/// `kept` with every sky section full, as light kept with a sky would be.
fn with_a_sky(mut kept: Kept) -> Kept {
    kept.light.sky.fill(SectionLight::Full);
    kept
}

/// Plan 17.5, their 8: light without a sky is kept through reshaping. A Nether region
/// restored from its state with its chunks handed back as the runner keeps them warm,
/// their light kept with them, has no sky; nor when the light handed in carries a sky,
/// which it drops; nor when the chunks come without light and are lit whole.
#[test]
fn a_restored_nether_region_has_no_sky_light_whatever_light_it_is_handed() {
    for handed in ["kept", "kept with a sky", "none"] {
        let options = Options::in_(Dimension::Nether);
        let mut scene = Scene::new(&options, &World::default());
        let light = scene.region.hand_over_light();
        let chunks: Vec<(ChunkPos, Chunk)> = options
            .loaded
            .iter()
            .map(|at| (*at, scene.region.saved_chunk(*at).expect("loaded")))
            .collect();
        let state = scene.region.state();
        let holdings = Holdings {
            held: options.held.clone(),
            pinned: vec![options.pinned],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut restored = Region::restore(config(&options), state, holdings);
        let kept_in = match handed {
            "kept" => light.clone(),
            "kept with a sky" => light
                .iter()
                .map(|(at, kept)| (*at, with_a_sky(kept.clone())))
                .collect(),
            _ => Vec::new(),
        };
        hand_back(&mut restored, chunks, kept_in);
        let handed_over = restored.hand_over_light();
        assert!(!handed_over.is_empty(), "{handed}: the chunks are lit");
        for (at, kept) in handed_over {
            assert!(!has_sky(&kept), "{handed}: {at:?}: {:?}", kept.light.sky);
        }
    }
}

/// Plan 17.5, their 8 and their tests ("a Nether region split or merged has no sky
/// light"): a Nether region split in two, each part given its chunks back with the light
/// kept for them (here with a sky, as a careless keeper might hand it), has no sky in
/// either part.
#[test]
fn a_nether_region_split_in_two_has_no_sky_light_in_either_part() {
    let options = Options::in_(Dimension::Nether);
    let mut scene = Scene::new(&options, &World::default());
    // Player 2 stands two chunks east, and goes with the part.
    scene.join(2, Vec3::new(40.5, f64::from(FEET), 0.5));
    scene.idle();
    let light = scene.region.hand_over_light();
    let splitting = scene
        .region
        .split(&[ChunkPos::new(2, 0)], OTHER, &[])
        .expect("player 2 goes");
    let (stays, part) = scene.region.take_split(splitting, &[]);
    let mut part_region = part.region;
    for (region, chunks) in [(&mut scene.region, stays), (&mut part_region, part.chunks)] {
        assert!(!chunks.is_empty());
        let kept_in = light
            .iter()
            .filter(|(at, _)| chunks.iter().any(|(c, _)| c == at))
            .map(|(at, kept)| (*at, with_a_sky(kept.clone())))
            .collect();
        hand_back(region, chunks, kept_in);
        let handed = region.hand_over_light();
        assert!(!handed.is_empty(), "the part's chunks are loaded");
        for (at, kept) in handed {
            assert!(!has_sky(&kept), "{at:?}: {:?}", kept.light.sky);
        }
    }
}

/// The same plan's merge: the western of two Nether regions on either side of a pinned
/// line takes in the eastern, and with it the eastern's chunks and their kept light, and
/// has no sky anywhere.
#[test]
fn a_nether_region_that_takes_in_another_has_no_sky_light() {
    let west_options = Options::in_(Dimension::Nether).side(true);
    let east_options = Options::in_(Dimension::Nether).side(false);
    let mut west = Scene::new(&west_options, &World::default());
    let mut east = Scene::empty(&east_options, &World::default());
    let mut light = west.region.hand_over_light();
    light.extend(east.region.hand_over_light());
    let east_chunks: Vec<(ChunkPos, Chunk)> = east_options
        .loaded
        .iter()
        .map(|at| (*at, east.region.saved_chunk(*at).expect("loaded")))
        .collect();
    let state = west.region.absorb(OTHER, &east.region.state());
    let mut chunks =
        west.region
            .take_absorbed(state, &east_options.held, &[east_options.pinned], &[], &[]);
    chunks.extend(east_chunks);
    let kept_in = light
        .iter()
        .map(|(at, kept)| (*at, with_a_sky(kept.clone())))
        .collect();
    hand_back(&mut west.region, chunks, kept_in);
    let handed = west.region.hand_over_light();
    assert!(
        handed.iter().any(|(at, _)| at.x >= 1),
        "the east's chunks came"
    );
    for (at, kept) in handed {
        assert!(!has_sky(&kept), "{at:?}: {:?}", kept.light.sky);
    }
}

/// Brief 1.2 (`has_skylight`: daylight detectors; DaylightDetectorBlock's ticker exists
/// only in a level with a sky light): in the Nether a detector never reads the sky and
/// gives no power; in the overworld at noon it gives 15; in the End, with no darkening
/// and the default sun angle, 15 by day and by night.
#[test]
fn a_daylight_detector_reads_the_sky_by_level_and_never_in_the_nether() {
    let detector = bp(2, FEET, 2);
    let power = |scene: &Scene| {
        let shown = scene.block(detector);
        (0..=15)
            .find(|power| {
                shown
                    == state(&format!(
                        "minecraft:daylight_detector[inverted=false,power={power}]"
                    ))
            })
            .unwrap_or_else(|| panic!("a detector: {shown:?}"))
    };
    for (dimension, day, expected) in [
        (Dimension::Overworld, NOON, 15),
        (Dimension::Nether, NOON, 0),
        (Dimension::Nether, MIDNIGHT, 0),
        (Dimension::End, NOON, 15),
        (Dimension::End, MIDNIGHT, 15),
    ] {
        let world = World::of(&[(
            detector,
            state("minecraft:daylight_detector[inverted=false,power=0]"),
        )]);
        let mut scene = Scene::new(&Options::in_(dimension).at(day), &world);
        scene.idle_until(60, |_, _| false);
        assert_eq!(power(&scene), expected, "{dimension:?} at {day}");
    }
}

// ------------------------------------------------------------------------------------
// Fire (plan 17.2 D2; brief 5.5; FireBlock 247–300).

/// Brief 5.5: fire on a block of the dimension's `infiniburn` never burns out: on
/// netherrack everywhere, and on bedrock in the End only. Fire on bedrock elsewhere, a
/// block that does not burn, goes out once it ages (`age > 3` with nothing to burn
/// beside it).
#[test]
fn fire_burns_on_bedrock_for_ever_in_the_end_only_and_on_netherrack_everywhere() {
    let bedrock = bp(2, FLOOR, 2);
    let netherrack = bp(6, FLOOR, 2);
    for dimension in Dimension::ALL {
        let world = World::of(&[(bedrock, blocks::BEDROCK), (netherrack, blocks::NETHERRACK)]);
        let options = Options::in_(dimension).holding(one(items::FLINT_AND_STEEL));
        let mut scene = Scene::new(&options, &world);
        for below in [bedrock, netherrack] {
            scene.click(below, Face::Top);
            assert!(
                is_fire(scene.block(below.offset(0, 1, 0))),
                "{dimension:?}: lit on {below:?}"
            );
        }
        let on_bedrock = bedrock.offset(0, 1, 0);
        let out = scene.idle_until(4_000, |scene, _| !is_fire(scene.block(on_bedrock)));
        if dimension == Dimension::End {
            assert_eq!(out, None, "the End's fire on bedrock burns for ever");
        } else {
            assert!(out.is_some(), "{dimension:?}: fire on bedrock goes out");
        }
        assert!(
            is_fire(scene.block(netherrack.offset(0, 1, 0))),
            "{dimension:?}: fire on netherrack burns on"
        );
    }
}

// ------------------------------------------------------------------------------------
// What is off outside the overworld (plan 17.2 D2).

/// Brief 1.2 (`can_start_raid`, n7k Raids 193): a raid omen that ends where the level
/// cannot start a raid begins none, and the omen ends all the same. A player whose raid
/// omen has a few ticks left in a village begins a raid there in the overworld and the
/// End, and none in the Nether.
#[test]
fn a_raid_omen_ending_in_a_village_begins_a_raid_except_in_the_nether() {
    for dimension in Dimension::ALL {
        let world = World {
            village: true,
            biome: biome_id("minecraft:plains"),
            ..World::default()
        };
        let mut options = Options::in_(dimension).standing(Vec3::new(8.5, f64::from(FEET), 8.5));
        options.effects = vec![EffectInstance {
            effect: "minecraft:raid_omen".to_owned(),
            amplifier: 0,
            duration: 3,
            ambient: false,
            visible: true,
            show_icon: true,
        }];
        options.raid_omen_position = Some(CENTRE);
        let mut scene = Scene::new(&options, &world);
        let raids = |scene: &Scene| {
            scene
                .region
                .loaded_chunks()
                .into_iter()
                .map(|at| scene.region.chunk(at).expect("loaded").raids().len())
                .sum::<usize>()
        };
        let begun = scene.idle_until(40, |scene, _| raids(scene) > 0);
        let omen_left = scene
            .region
            .player_state(player(1))
            .expect("there")
            .effects
            .iter()
            .any(|effect| format!("{effect:?}").contains("raid_omen"));
        if dimension == Dimension::Nether {
            assert_eq!(begun, None, "no raid begun in the Nether");
        } else {
            assert!(begun.is_some(), "{dimension:?}: a raid begins");
        }
        assert!(!omen_left, "{dimension:?}: the omen ends");
    }
}

/// Plan 17.2 D2: the custom spawners run in the overworld only. The cat spawner
/// (`CatSpawner`, every 1 200 ticks, for a player of the world's table) makes a cat 8 to
/// 31 blocks off a player standing in a swamp hut where no cat is near; nothing of its
/// own rules depends on the level, so only the gate keeps it from the Nether and the
/// End.
#[test]
fn the_cat_spawner_makes_a_cat_in_a_swamp_hut_in_the_overworld_only() {
    for dimension in Dimension::ALL {
        let world = World {
            hut: true,
            ..World::default()
        };
        let mut options = Options::in_(dimension);
        options.spawn_mobs = true;
        options.held = around(4);
        options.loaded = around(3);
        let mut scene = Scene::new(&options, &world);
        let entity = scene.entity.expect("player 1 is in");
        scene.region.tick(&TickInputs {
            world_players: Some(vec![WorldPlayer {
                player: player(1),
                entity,
                dimension,
                position: [ORIGIN.x, ORIGIN.y, ORIGIN.z],
                game_mode: 1,
                sleeping: None,
            }]),
            ..TickInputs::default()
        });
        let came = scene.idle_until(4_000, |scene, _| {
            !scene.of_kind(entity_types::CAT).is_empty()
        });
        if dimension == Dimension::Overworld {
            assert!(came.is_some(), "the control: a cat comes");
        } else {
            assert_eq!(came, None, "no cat in {dimension:?}");
        }
    }
}

/// Plan 17.2 D2 and brief 1.2 (`snow_golem_melts`, read at the golem's position): a snow
/// golem melts where its biome or its level says: in the Nether even in a snowy biome,
/// and in the overworld and the End only in a hot one.
#[test]
fn a_snow_golem_melts_in_the_nether_even_in_a_snowy_biome() {
    for (dimension, biome, melts) in [
        (Dimension::Overworld, "minecraft:snowy_plains", false),
        (Dimension::Overworld, "minecraft:desert", true),
        (Dimension::Nether, "minecraft:snowy_plains", true),
        (Dimension::End, "minecraft:snowy_plains", false),
    ] {
        let world = World {
            biome: biome_id(biome),
            ..World::default()
        };
        let options = Options::in_(dimension).holding(one(items::SNOW_GOLEM_SPAWN_EGG));
        let mut scene = Scene::new(&options, &world);
        let golem = scene.hatch(0, 3, 3);
        let full = health(&scene.record(golem).expect("the golem"));
        let hurt = scene.idle_until(100, |scene, _| {
            scene
                .record(golem)
                .is_none_or(|golem| health(&golem) < full)
        });
        assert_eq!(hurt.is_some(), melts, "{dimension:?} in {biome}");
    }
}

// ------------------------------------------------------------------------------------
// Heights (plan 17.5, their 7).

/// Plan 17.5, their 7, and 17.6 ("the pistons' top follows from `lowest_y` by
/// dimension"): a piston cannot push a block out of the top of its level. A piston at
/// y 254 facing up under a block at 255, the Nether's and the End's highest, does not
/// extend there when powered; the overworld's, whose top is 319, pushes the block to
/// 256.
#[test]
fn a_piston_pushes_no_block_out_of_the_top_of_the_nether_or_the_end() {
    let piston = bp(2, 254, 0);
    let pushed = bp(2, 255, 0);
    let stand = bp(3, 253, 0);
    for dimension in Dimension::ALL {
        let mut placed = vec![
            (piston, state("minecraft:piston[extended=false,facing=up]")),
            (pushed, blocks::STONE),
            (stand, blocks::STONE),
        ];
        for x in -2..=4 {
            for z in -2..=2 {
                placed.push((bp(x, 252, z), blocks::STONE));
            }
        }
        let options = Options::in_(dimension)
            .holding(one(items::REDSTONE_BLOCK))
            .standing(Vec3::new(0.5, 253.0, 0.5));
        let mut scene = Scene::new(&options, &World::of(&placed));
        // A redstone block beside the piston powers it.
        scene.click(stand, Face::Top);
        assert_eq!(scene.block(bp(3, 254, 0)), blocks::REDSTONE_BLOCK);
        scene.idle_until(10, |_, _| false);
        if dimension == Dimension::Overworld {
            assert_eq!(scene.block(bp(2, 256, 0)), blocks::STONE, "pushed up");
        } else {
            assert_eq!(
                scene.block(pushed),
                blocks::STONE,
                "{dimension:?}: not pushed"
            );
            assert_eq!(
                scene.block(piston),
                state("minecraft:piston[extended=false,facing=up]"),
                "{dimension:?}: the piston stays in"
            );
        }
    }
}

/// Plan 17.5, their 7 and their tests ("chorus fruit in the Nether at y 120 stays under
/// 128"): chorus fruit's teleport clamps its height to `minY + logical_height − 1`, the
/// Nether's 127. Player 1 eats it standing at y 125 under a bedrock roof at y 127: drawn
/// up to 7 above, a target above the roof would land on it at 128 without the clamp;
/// clamped, they land on their floor again, never at 128 or above.
#[test]
fn chorus_fruit_in_the_nether_never_takes_a_player_above_its_logical_height() {
    let floor = 124;
    let mut placed = Vec::new();
    for x in -12..=12 {
        for z in -12..=12 {
            placed.push((bp(x, floor, z), blocks::STONE));
            placed.push((bp(x, 127, z), blocks::BEDROCK));
        }
    }
    let options = Options::in_(Dimension::Nether)
        .holding(holding(Some(stack(items::CHORUS_FRUIT, 64)), None, None))
        .standing(Vec3::new(0.5, f64::from(floor + 1), 0.5));
    let mut scene = Scene::new(&options, &World::of(&placed));
    let mut teleports = 0;
    for _ in 0..12 {
        scene.sequence += 1;
        let sequence = scene.sequence;
        scene.input(PlayerInput::UseItem {
            rotation: (0.0, 0.0),
            sneaking: false,
            sequence,
        });
        let done = scene.idle_until(60, |_, output| {
            !sounds_of(output, sounds::ITEM_CHORUS_FRUIT_TELEPORT).is_empty()
        });
        if done.is_some() {
            teleports += 1;
        }
        let feet = scene.region.player(player(1)).expect("there").1.position;
        assert!(feet.y < 128.0, "teleported to {feet:?}");
    }
    assert!(teleports > 0, "the fruit teleports");
}
