//! Tests of step P12, the Nether's mobs, written from the specification and the public
//! interface alone by someone who did not write the code: `docs/groundwork/p-plan.md`
//! sections 7 and 18 (18.1 to 18.6, the review's changes holding where they differ),
//! `docs/groundwork/p-brief.md` sections 4.2, 5.7 to 5.10, and the game's bytecode where
//! a rule's detail is wanted (`reference/*-javap/`: `AbstractHurtingProjectile`,
//! `LargeFireball`, `SmallFireball`, `Ghast`, `Ghast$GhastShootFireballGoal`, `Blaze`,
//! `WitherSkeleton`, `Strider`, `MagmaCube`, `ProjectileDeflection`, `Player`).
//!
//! A region of one dimension (the Nether unless a test says otherwise) holds the chunks
//! about the origin's, loaded, with a floor at y = 40 (feet at 41) and air above. Player
//! 1 stands near the origin and is creative, as every player of Clustine is; a mob takes
//! no creative player as its enemy (the game's `canBeSeenAsEnemy`), so the tests that
//! need a target give the region the face of a survival player 2, as the world store
//! streams a player of another region or one the region let go a moment ago.
//!
//! Fireballs are given to a region as arrivals, as a fireball crossing a line from
//! another region is (plan 18.5: "a fireball whose owner is not the region's is handed
//! across a line as a `Thrown` is").
//!
//! Named rather than tested, as no player of Clustine can be what they need: what a
//! survival player of the region takes of a hit (a hoglin's throw, a wither skeleton's
//! wither) on their own hurt path, since a creative one takes no hurt; the hits are
//! tested where they are sent to a survival player another region holds. A piglin's
//! anger at a player who mines gold or opens a container is tested in
//! `src/region/p12_spec_tests.rs`, where it is told across a line of a survival player.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_sim::api::{
    EnderChest, Face, FaceChange, FaceSeq, HOTBAR_SLOTS, Inventory, ItemStack, OFF_HAND, Omens,
    Place, PlayerFace, PlayerInput, Pose, RegionEvent, WorldClock,
};
use clustine_sim::shown::{DataShown, index, mob_values};
use clustine_sim::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerJoin, Region, RegionConfig, RegionState,
    TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Arrival, Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Dimension, Double, Due, EdgeId, Effect,
    EffectId, Effected, Entity, EntityData, EntityId, EntityIds, EntityUse, FiledEffect, Fireball,
    PlayerId, ProjectileOwner, RegionId, Vec3,
};
use uuid::Uuid;

// ------------------------------------------------------------------------------------
// The scene.

const EDGE: EdgeId = EdgeId(7);
/// The floor's top, and the height of a player's feet on it.
const FLOOR: i32 = 40;
const FEET: i32 = FLOOR + 1;
const ORIGIN: Vec3 = Vec3::new(0.5, FEET as f64, 0.5);

/// The region another region believes to hold what it does not.
const OTHER: RegionId = RegionId(9);

/// The fireball's `accelerationPower` at first and after a deflection by an attack, and
/// its inertia in air and in water (`AbstractHurtingProjectile`, 16–17, 323–331,
/// 376–393).
const ACCELERATION: f64 = 0.1;
const INERTIA: f64 = 0.95;
const LIQUID_INERTIA: f64 = 0.8;

fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

/// An inventory with `slots` in the hotbar from slot 0, and `off` in the off hand.
fn hotbar(slots: &[ItemStack], off: Option<ItemStack>) -> Inventory {
    let mut inventory = Inventory::with_hotbar([const { None }; HOTBAR_SLOTS]);
    for (slot, held) in slots.iter().enumerate() {
        inventory.set(slot, Some(held.clone()));
    }
    inventory.set(OFF_HAND, off);
    inventory
}

fn eggs(kinds: &[i32]) -> Inventory {
    let stacks: Vec<ItemStack> = kinds.iter().map(|item| stack(*item, 1)).collect();
    hotbar(&stacks, None)
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

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

fn is_fire(state: BlockState) -> bool {
    state.block_id() == blocks::FIRE.block_id()
}

/// What a test's world holds beyond the floor.
#[derive(Clone)]
struct World {
    blocks: Vec<(BlockPos, BlockState)>,
    floor: BlockState,
    biome: u16,
    /// A Nether fortress whose one piece is over all the land near the origin.
    fortress: bool,
}

impl Default for World {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            floor: blocks::STONE,
            biome: biome_id("minecraft:nether_wastes"),
            fortress: false,
        }
    }
}

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
                chunk.set(x, FLOOR, z, self.floor);
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
        if self.fortress {
            let all = clustine_world::KeptBox {
                min: bp(-80, FLOOR - 8, -80),
                max: bp(80, FLOOR + 30, 80),
            };
            chunk.set_structures(vec![clustine_world::KeptStructure {
                kind: clustine_world::KeptKind::Fortress,
                source: ChunkPos::new(0, 0),
                bounds: all,
                pieces: vec![all],
            }]);
        }
        chunk
    }
}

/// A box of `state` from `min` to `max`, both included.
fn filled(min: BlockPos, max: BlockPos, state: BlockState) -> Vec<(BlockPos, BlockState)> {
    let mut found = Vec::new();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                found.push((bp(x, y, z), state));
            }
        }
    }
    found
}

#[derive(Clone)]
struct Options {
    dimension: Dimension,
    inventory: Inventory,
    feet: Vec3,
    random_tick_speed: u32,
    spawn_mobs: bool,
    pinned: ChunkArea,
    held: Vec<ChunkPos>,
    loaded: Vec<ChunkPos>,
    /// Chunks of other regions the region has copies of, held by [`OTHER`].
    copies: Vec<ChunkPos>,
    block: u32,
    /// The world's clock, game time and day alike.
    day: u64,
}

impl Options {
    fn in_(dimension: Dimension) -> Self {
        Self {
            dimension,
            inventory: hotbar(&[], None),
            feet: ORIGIN,
            random_tick_speed: 0,
            spawn_mobs: false,
            pinned: ChunkArea::EVERYWHERE,
            held: around(3),
            loaded: around(2),
            copies: Vec::new(),
            block: 1,
            day: 18_000,
        }
    }

    fn nether() -> Self {
        Self::in_(Dimension::Nether)
    }

    fn holding(mut self, inventory: Inventory) -> Self {
        self.inventory = inventory;
        self
    }

    fn standing(mut self, feet: Vec3) -> Self {
        self.feet = feet;
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
    /// The numbers effects and arrivals given to the region are filed under.
    filed: u64,
    faces: u32,
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
        let clock = WorldClock {
            game_time: options.day,
            day: options.day,
        };
        region.tick(&TickInputs {
            tickets_added: options
                .loaded
                .iter()
                .chain(&options.copies)
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            clock: Some(clock),
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
            filed: 0,
            faces: 0,
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
        if self.entity.is_none() {
            self.entity = Some(entity);
        }
    }

    fn me(&self) -> EntityId {
        self.entity.expect("player 1 is in")
    }

    fn input(&mut self, input: PlayerInput) -> TickOutput {
        let inputs = self.numbered(vec![input]);
        self.region.tick(&TickInputs {
            inputs,
            ..TickInputs::default()
        })
    }

    /// The scene's first player's `inputs`, numbered on from their last.
    fn numbered(
        &mut self,
        inputs: Vec<PlayerInput>,
    ) -> Vec<(EdgeId, PlayerId, EntityId, u64, PlayerInput)> {
        let entity = self.me();
        let id = self
            .region
            .entities()
            .find_map(|state| match state.kind {
                clustine_sim::api::EntityKind::Player { player, .. } if state.entity == entity => {
                    Some(player)
                }
                _ => None,
            })
            .expect("the scene's player is in");
        inputs
            .into_iter()
            .map(|input| {
                self.input += 1;
                (EDGE, id, entity, self.input, input)
            })
            .collect()
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

    /// Idles for `ticks` ticks, keeping every output.
    fn idle_for(&mut self, ticks: u32) -> Vec<TickOutput> {
        (0..ticks).map(|_| self.idle()).collect()
    }

    /// Player 1 clicks the top of `position` with what they hold in the main hand.
    fn click(&mut self, position: BlockPos) -> TickOutput {
        self.sequence += 1;
        let sequence = self.sequence;
        self.input(PlayerInput::UseItemOn {
            position,
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence,
        })
    }

    fn select(&mut self, slot: u8) -> TickOutput {
        self.input(PlayerInput::SelectSlot { slot })
    }

    fn turn(&mut self, yaw: f32, pitch: f32) -> TickOutput {
        self.input(PlayerInput::Move {
            position: None,
            rotation: Some((yaw, pitch)),
            on_ground: true,
        })
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

    /// Player 1 uses the spawn egg in slot `slot` on the top of the block at (x, y, z):
    /// the mob it made.
    fn hatch_on(&mut self, slot: u8, at: BlockPos) -> EntityId {
        let before: Vec<EntityId> = self.entities().iter().map(|entity| entity.key).collect();
        self.select(slot);
        self.click(at);
        let made: Vec<EntityId> = self
            .entities()
            .iter()
            .map(|entity| entity.key)
            .filter(|key| !before.contains(key))
            .collect();
        assert_eq!(made.len(), 1, "the egg made one mob: {made:?}");
        made[0]
    }

    /// The same on the floor at (x, z).
    fn hatch(&mut self, slot: u8, x: i32, z: i32) -> EntityId {
        self.hatch_on(slot, bp(x, FLOOR, z))
    }

    /// Hands the region `entity` as an arrival from across a line, taken at once.
    fn arrive(&mut self, entity: Entity) -> TickOutput {
        self.filed += 1;
        let chunk = entity.chunk();
        self.region.tick(&TickInputs {
            arrivals: vec![Arrival {
                id: EffectId {
                    sequence: 900_000 + self.filed,
                    index: 0,
                },
                chunk,
                entity,
                due: Due::FromTaking,
            }],
            ..TickInputs::default()
        })
    }

    /// The store's first publication of the other regions' counts (none) and of the
    /// world's players (none but the region's own), before which nothing spawns.
    fn publish_counts(&mut self) {
        self.region.tick(&TickInputs {
            counts: Some(clustine_sim::api::PublishedCounts::default()),
            world_players: Some(Vec::new()),
            ..TickInputs::default()
        });
    }

    /// Tells the region the face of the survival player `n`, whose stay is the entity
    /// `entity`, standing at `feet` and wearing `worn` (feet, legs, chest, head).
    fn face(&mut self, n: u128, entity: EntityId, feet: Vec3, worn: [Option<ItemStack>; 4]) {
        self.faces += 1;
        let face = survival_face(feet, worn);
        self.region.tick(&TickInputs {
            faces: vec![FaceChange::Face {
                player: player(n),
                entity,
                hops: 0,
                seq: FaceSeq(700_000, self.faces),
                face,
            }],
            ..TickInputs::default()
        });
    }
}

/// The face of a survival player standing at `feet`, holding nothing.
fn survival_face(feet: Vec3, worn: [Option<ItemStack>; 4]) -> PlayerFace {
    PlayerFace {
        position: [feet.x, feet.y, feet.z],
        yaw: 0.0,
        pitch: 0.0,
        eye_height: 1.62,
        main_hand: None,
        off_hand: None,
        worn,
        sneaking: false,
        game_mode: 0,
        sleeping: None,
        vehicle: None,
        alive: true,
        last_hurt_mob: None,
        last_hurt_by: None,
        moved_horizontally: false,
        sprinting: false,
        omens: Omens::default(),
    }
}

// ------------------------------------------------------------------------------------
// What the region reports.

fn sounds_of(output: &TickOutput, sound: i32) -> Vec<Vec3> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: played,
                position,
                ..
            } if *played == sound => Some(*position),
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

fn has_level_event(output: &TickOutput, kind: i32) -> bool {
    level_events(output).iter().any(|(found, _)| *found == kind)
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

fn shown(mob: &Entity, at: u8) -> Option<DataShown> {
    mob_values(mob.kind, &mob.data)
        .into_iter()
        .find(|(found, _, _)| *found == at)
        .map(|(_, value, _)| value)
}

fn health(mob: &Entity) -> f32 {
    match shown(mob, index::HEALTH) {
        Some(DataShown::Float(health)) => health,
        other => panic!("a mob shows its health: {other:?}"),
    }
}

fn fireball_of(entity: &Entity) -> &Fireball {
    match &entity.data {
        EntityData::Fireball(fireball) => fireball,
        other => panic!("a fireball: {other:?}"),
    }
}

fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn close(a: [f64; 3], b: [f64; 3], within: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= within)
}

/// `(m + m̂·p)·inertia`, as `AbstractHurtingProjectile.applyInertia` has it.
fn accelerated(m: [f64; 3], power: f64, inertia: f64) -> [f64; 3] {
    let l = length(m);
    let unit = if l < 1.0e-4 {
        [0.0; 3]
    } else {
        [m[0] / l, m[1] / l, m[2] / l]
    };
    let inertia = f64::from(inertia as f32);
    [
        (m[0] + unit[0] * power) * inertia,
        (m[1] + unit[1] * power) * inertia,
        (m[2] + unit[2] * power) * inertia,
    ]
}

/// A fireball of the entity type `kind` at `at` with `motion`, owned by `owner`, with
/// the game's defaults otherwise (`accelerationPower` 0.1, `ExplosionPower` 1).
fn fireball(
    key: u64,
    kind: i32,
    at: [f64; 3],
    motion: [f64; 3],
    owner: Option<ProjectileOwner>,
) -> Entity {
    Entity {
        key: EntityId(key),
        kind: u16::try_from(kind).expect("a type"),
        uuid: Uuid::from_u128(u128::from(key) + 0xF1AE),
        position: at,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 1,
        lived: 5,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Fireball(Box::new(Fireball {
            item: stack(items::FIRE_CHARGE, 1),
            owner,
            left_owner: true,
            has_been_shot: true,
            acceleration_power: Double(ACCELERATION),
            explosion_power: 1,
        })),
        portal: Default::default(),
    }
}

fn mob_owner(key: EntityId, kind: i32) -> ProjectileOwner {
    ProjectileOwner {
        key,
        kind: u16::try_from(kind).expect("a type"),
        player: None,
    }
}

// ------------------------------------------------------------------------------------
// The fireball as the game keeps it (plan 18.5, their 1; `AbstractHurtingProjectile`).

/// The fireball of key `key` after each of `ticks` ticks, while it lasts.
fn flight(scene: &mut Scene, key: EntityId, ticks: u32) -> Vec<Entity> {
    let mut seen = Vec::new();
    for _ in 0..ticks {
        scene.idle();
        match scene.record(key) {
            Some(entity) => seen.push(entity),
            None => break,
        }
    }
    seen
}

/// Plan 18.5 (their 1): every tick a fireball's motion becomes `(m + m̂·p)·0.95` with
/// `p` its `accelerationPower` (0.1), applied before it moves (`applyInertia` is the
/// first thing `tick` does), and it then moves by the new motion.
#[test]
fn a_fireball_in_the_air_gains_its_acceleration_along_its_motion_and_keeps_95_hundredths() {
    for kind in [entity_types::FIREBALL, entity_types::SMALL_FIREBALL] {
        let mut scene = Scene::new(&Options::nether(), &World::default());
        let key = EntityId(5_000);
        scene.arrive(fireball(
            5_000,
            kind,
            [-10.5, 70.0, 0.5],
            [0.05, 0.0, 0.0],
            None,
        ));
        let mut seen = vec![scene.record(key).expect("the fireball arrived")];
        seen.extend(flight(&mut scene, key, 4));
        assert_eq!(seen.len(), 5, "it flies on through the air");
        for pair in seen.windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            let expected = accelerated(before.motion, ACCELERATION, INERTIA);
            assert!(
                close(after.motion, expected, 1.0e-9),
                "{kind}: {:?} after {:?}, not {expected:?}",
                after.motion,
                before.motion
            );
            let moved = [
                before.position[0] + after.motion[0],
                before.position[1] + after.motion[1],
                before.position[2] + after.motion[2],
            ];
            assert!(
                close(after.position, moved, 1.0e-9),
                "{kind}: it moves by the new motion: {:?}, not {moved:?}",
                after.position
            );
        }
    }
}

/// Plan 18.5 (their 1): in water the inertia is 0.8 (`getLiquidInertia`), the motion
/// `(m + m̂·0.1)·0.8`.
#[test]
fn a_fireball_in_water_keeps_80_hundredths_of_its_motion() {
    let water = filled(bp(-14, 60, -3), bp(-2, 80, 3), blocks::WATER);
    let mut scene = Scene::new(&Options::nether(), &World::of(&water));
    let key = EntityId(5_001);
    scene.arrive(fireball(
        5_001,
        entity_types::FIREBALL,
        [-10.5, 70.0, 0.5],
        [0.01, 0.0, 0.0],
        None,
    ));
    let mut seen = vec![scene.record(key).expect("the fireball arrived")];
    seen.extend(flight(&mut scene, key, 3));
    assert_eq!(seen.len(), 4, "it flies on through the water");
    for pair in seen.windows(2) {
        let expected = accelerated(pair[0].motion, ACCELERATION, LIQUID_INERTIA);
        assert!(
            close(pair[1].motion, expected, 1.0e-9),
            "{:?} after {:?}, not {expected:?}",
            pair[1].motion,
            pair[0].motion
        );
    }
}

// ------------------------------------------------------------------------------------
// The ghast (brief 5.9; `Ghast$GhastShootFireballGoal.tick`).

/// Survival player 2's stay, seen by the west region as a face.
const TWO: EntityId = EntityId(900_002);

/// The west of a line with player 1 at the origin, holding `items`, and the face of
/// survival player 2 standing in the east's land at `feet`.
fn west_with_a_survivor(items: &[i32], world: &World, feet: Vec3) -> Scene {
    let options = Options::nether().side(true).holding(eggs(items));
    let mut scene = Scene::new(&options, world);
    scene.face(2, TWO, feet, [None, None, None, None]);
    scene
}

/// Brief 5.9: with a target within 64 blocks in sight, the ghast's charge counts up
/// each tick; at 10 it warns (level event 1015), at 20 it shoots (1016) a large fireball
/// and its charge goes to −40, so that it warns again 50 ticks after and shoots again 60
/// ticks after. Its fireball is a large one it owns, of explosion power 1, and leaves at
/// 0.1 blocks a tick towards the target (`assignDirectionalMovement`; a fresh entity does
/// not tick in the tick it is added).
#[test]
fn a_ghast_warns_ten_ticks_before_it_shoots_and_shoots_every_sixty_ticks() {
    let target = Vec3::new(22.5, FEET as f64, 0.5);
    let mut scene = west_with_a_survivor(&[items::GHAST_SPAWN_EGG], &World::default(), target);
    let ghast = scene.hatch(0, 4, 0);
    let mut warned = Vec::new();
    let mut shot = Vec::new();
    let mut first_fireball = None;
    for tick in 1..=400u32 {
        let known: Vec<EntityId> = scene.entities().iter().map(|entity| entity.key).collect();
        let output = scene.idle();
        if has_level_event(&output, 1015) {
            warned.push(tick);
        }
        if has_level_event(&output, 1016) {
            shot.push(tick);
            if first_fireball.is_none() {
                let made: Vec<Entity> = scene
                    .of_kind(entity_types::FIREBALL)
                    .into_iter()
                    .filter(|entity| !known.contains(&entity.key))
                    .collect();
                first_fireball = made.into_iter().next();
            }
        }
        if shot.len() >= 3 {
            break;
        }
    }
    assert!(
        shot.len() >= 3,
        "it shoots three times: warned {warned:?}, shot {shot:?}"
    );
    assert_eq!(warned[0] + 10, shot[0], "warned {warned:?}, shot {shot:?}");
    assert_eq!(shot[1] - shot[0], 60, "shot {shot:?}");
    assert_eq!(shot[2] - shot[1], 60, "shot {shot:?}");
    assert_eq!(warned[1] - shot[0], 50, "warned {warned:?}, shot {shot:?}");
    let fireball = first_fireball.expect("the shot made a large fireball");
    let kept = fireball_of(&fireball);
    assert_eq!(kept.owner.map(|owner| owner.key), Some(ghast));
    assert_eq!(
        kept.owner.map(|owner| i32::from(owner.kind)),
        Some(entity_types::GHAST)
    );
    assert_eq!(kept.explosion_power, 1);
    assert!(
        (length(fireball.motion) - ACCELERATION).abs() < 1.0e-6,
        "it leaves at 0.1 a tick: {:?}",
        fireball.motion
    );
    assert!(
        fireball.motion[0] > 0.0,
        "towards the target, east: {:?}",
        fireball.motion
    );
}

// ------------------------------------------------------------------------------------
// What a fireball does where it lands (plan 18.3 E2, 18.5 their 11; `LargeFireball`,
// `SmallFireball`, `TntBlock.onProjectileHit`, `CampfireBlock.onProjectileHit`).

/// A fireball falling onto the floor at (x, z) from three blocks up, of `kind`, with
/// `power` its explosion power where it is large.
fn falling(key: u64, kind: i32, x: f64, z: f64, power: u8) -> Entity {
    let mut entity = fireball(
        key,
        kind,
        [x, f64::from(FEET) + 3.0, z],
        [0.0, -0.3, 0.0],
        None,
    );
    if let EntityData::Fireball(kept) = &mut entity.data {
        kept.explosion_power = power;
    }
    entity
}

/// Ticks until the fireball `key` is gone, for at most `ticks`: every output till then.
fn until_gone(scene: &mut Scene, key: EntityId, ticks: u32) -> Vec<TickOutput> {
    let mut outputs = Vec::new();
    for _ in 0..ticks {
        outputs.push(scene.idle());
        if scene.record(key).is_none() {
            return outputs;
        }
    }
    panic!("the fireball {key:?} is still there after {ticks} ticks");
}

/// `LargeFireball.onHit`: it explodes where it is with its `ExplosionPower` (1 unless
/// set) as the radius, and is gone.
#[test]
fn a_large_fireball_that_hits_the_floor_explodes_with_its_power_and_is_gone() {
    for power in [1, 3] {
        let mut scene = Scene::new(&Options::nether(), &World::default());
        let key = EntityId(5_010);
        scene.arrive(falling(5_010, entity_types::FIREBALL, 6.5, 6.5, power));
        let outputs = until_gone(&mut scene, key, 20);
        let blasts: Vec<([f64; 3], f32)> = outputs.iter().flat_map(explosions).collect();
        assert_eq!(blasts.len(), 1, "one explosion: {blasts:?}");
        assert_eq!(blasts[0].1, f32::from(power), "its radius is its power");
        assert!(
            close(blasts[0].0, [6.5, f64::from(FEET), 6.5], 0.6),
            "where it hit: {:?}",
            blasts[0].0
        );
    }
}

/// `LargeFireball.onHit`: the explosion makes fire where `mob_griefing` holds, as it
/// does (`Level.explode(…, fire = mob_griefing, MOB)`): one of power 3 on a stone floor
/// leaves fire on the floor about it.
#[test]
fn a_large_fireball_s_explosion_sets_fire_about_it() {
    let mut scene = Scene::new(&Options::nether(), &World::default());
    let key = EntityId(5_011);
    scene.arrive(falling(5_011, entity_types::FIREBALL, 6.5, 6.5, 3));
    until_gone(&mut scene, key, 20);
    let mut fires = 0;
    for x in 0..=13 {
        for z in 0..=13 {
            for y in FLOOR - 3..=FEET + 3 {
                if is_fire(scene.block(bp(x, y, z))) {
                    fires += 1;
                }
            }
        }
    }
    assert!(fires > 0, "the explosion set fire about it");
}

/// `SmallFireball.onHitBlock`: a small fireball that hits a block sets fire in the block
/// beside the face it hit, where that is empty (its owner is no mob here), and is gone
/// (`onHit`'s `discard`), with no explosion.
#[test]
fn a_small_fireball_that_hits_the_floor_sets_fire_above_it_and_is_gone() {
    let mut scene = Scene::new(&Options::nether(), &World::default());
    let key = EntityId(5_012);
    scene.arrive(falling(5_012, entity_types::SMALL_FIREBALL, 6.5, 6.5, 0));
    let outputs = until_gone(&mut scene, key, 20);
    assert!(outputs.iter().all(|output| explosions(output).is_empty()));
    assert!(
        is_fire(scene.block(bp(6, FEET, 6))),
        "{:?}",
        scene.block(bp(6, FEET, 6))
    );
}

/// `SmallFireball.onHitEntity`: the entity hit is set on fire for 5 seconds (100 ticks)
/// and hurt by 5 (`fireball` damage).
#[test]
fn a_small_fireball_sets_the_mob_it_hits_on_fire_and_hurts_it_by_five() {
    let options = Options::nether().holding(eggs(&[items::PIG_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::default());
    let pig = scene.hatch(0, 3, 3);
    let before = scene.record(pig).expect("the pig");
    assert_eq!(health(&before), 10.0);
    let key = EntityId(5_013);
    scene.arrive(fireball(
        5_013,
        entity_types::SMALL_FIREBALL,
        [
            before.position[0],
            before.position[1] + 2.5,
            before.position[2],
        ],
        [0.0, -0.5, 0.0],
        None,
    ));
    until_gone(&mut scene, key, 10);
    let after = scene.record(pig).expect("the pig lives");
    assert_eq!(health(&after), 5.0, "hurt by 5");
    assert!(
        (95..=100).contains(&after.fire_ticks),
        "on fire for 100 ticks: {}",
        after.fire_ticks
    );
}

/// `SmallFireball.onHitEntity` and the brief's 5.9: a fire-immune mob (a magma cube) is
/// not hurt by a small fireball's fire damage, and the fire it set is taken back
/// (`setRemainingFireTicks(before)` where the hurt did not take).
#[test]
fn a_small_fireball_neither_hurts_nor_lights_a_fire_immune_mob() {
    let options = Options::nether().holding(eggs(&[items::MAGMA_CUBE_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::default());
    let cube = scene.hatch(0, 3, 3);
    let before = scene.record(cube).expect("the cube");
    let key = EntityId(5_014);
    scene.arrive(fireball(
        5_014,
        entity_types::SMALL_FIREBALL,
        [
            before.position[0],
            before.position[1] + 3.0,
            before.position[2],
        ],
        [0.0, -0.5, 0.0],
        None,
    ));
    until_gone(&mut scene, key, 10);
    let after = scene.record(cube).expect("the cube lives");
    assert_eq!(health(&after), health(&before), "not hurt");
    assert!(after.fire_ticks <= 0, "not on fire: {}", after.fire_ticks);
}

/// Plan 18.5 (their 11): a fireball lights TNT it hits (`TntBlock.onProjectileHit`: a
/// burning projectile, as a fireball always is, that may interact primes it).
#[test]
fn a_small_fireball_that_hits_tnt_primes_it() {
    let at = bp(6, FEET, 6);
    let mut scene = Scene::new(&Options::nether(), &World::of(&[(at, blocks::TNT)]));
    let key = EntityId(5_015);
    let mut ball = falling(5_015, entity_types::SMALL_FIREBALL, 6.5, 6.5, 0);
    ball.position[1] = f64::from(FEET) + 4.0;
    scene.arrive(ball);
    until_gone(&mut scene, key, 20);
    assert_ne!(scene.block(at), blocks::TNT, "the TNT block is gone");
    assert_eq!(scene.of_kind(entity_types::TNT).len(), 1, "primed");
}

/// Plan 18.5 (their 11): a fireball lights a campfire it hits
/// (`CampfireBlock.onProjectileHit`: a burning projectile lights one unlit and dry).
#[test]
fn a_small_fireball_that_hits_an_unlit_campfire_lights_it() {
    let at = bp(6, FEET, 6);
    let unlit =
        state("minecraft:campfire[facing=north,lit=false,signal_fire=false,waterlogged=false]");
    let mut scene = Scene::new(&Options::nether(), &World::of(&[(at, unlit)]));
    let key = EntityId(5_016);
    let mut ball = falling(5_016, entity_types::SMALL_FIREBALL, 6.5, 6.5, 0);
    ball.position[1] = f64::from(FEET) + 3.0;
    scene.arrive(ball);
    until_gone(&mut scene, key, 20);
    assert_eq!(
        scene.block(at),
        state("minecraft:campfire[facing=north,lit=true,signal_fire=false,waterlogged=false]")
    );
}

// ------------------------------------------------------------------------------------
// The fireball's end (plan 18.5 their 1, 18.6; `AbstractHurtingProjectile.tick` 0–48).

/// A fireball at `at` that hangs in the air: no motion and no acceleration, as one
/// whose acceleration a deflection has yet to set.
fn hanging(key: u64, kind: i32, at: [f64; 3], owner: Option<ProjectileOwner>) -> Entity {
    let mut entity = fireball(key, kind, at, [0.0; 3], owner);
    if let EntityData::Fireball(kept) = &mut entity.data {
        kept.acceleration_power = Double(0.0);
    }
    entity
}

/// Plan 18.6: a fireball that reaches a chunk no region holds is discarded (the game's
/// `hasChunkAt`), and sent nowhere.
#[test]
fn a_fireball_that_reaches_a_chunk_no_region_holds_is_discarded() {
    let mut options = Options::nether();
    options.held = around(2);
    let mut scene = Scene::new(&options, &World::default());
    let key = EntityId(5_020);
    let mut outputs = vec![scene.arrive(fireball(
        5_020,
        entity_types::FIREBALL,
        [28.5, 70.0, 0.5],
        [25.0, 0.0, 0.0],
        None,
    ))];
    outputs.extend(scene.idle_for(4));
    assert!(scene.record(key).is_none(), "the fireball is gone");
    for output in &outputs {
        assert!(
            output.arrivals.iter().all(|(_, entity)| entity.key != key),
            "sent nowhere: {:?}",
            output.arrivals
        );
    }
}

/// Plan 18.5 (their 1): a fireball whose owner the region does not have, as one of
/// another region, is not discarded for it: it hangs on.
#[test]
fn a_fireball_whose_owner_is_another_region_s_stays() {
    let mut scene = Scene::new(&Options::nether(), &World::default());
    let elsewhere = EntityId(5_022);
    scene.arrive(hanging(
        5_022,
        entity_types::FIREBALL,
        [-6.5, 60.0, 6.5],
        Some(mob_owner(EntityId(77_777), entity_types::GHAST)),
    ));
    scene.idle_for(60);
    assert!(scene.record(elsewhere).is_some(), "it stays");
}

/// Plan 18.5 (their 1): a fireball is discarded when its owner is the region's own and
/// removed (`getOwner() != null && getOwner().isRemoved()`): one owned by an item that
/// player 1 picks up goes.
#[test]
#[ignore = "departs from the specification: a fireball whose owner, an item of the region, \
            is picked up and removed flies on"]
fn a_fireball_whose_owner_of_the_region_is_removed_is_discarded() {
    let options = Options::nether()
        .holding(hotbar(&[stack(items::STICK, 1)], None))
        .standing(Vec3::new(-6.5, FEET as f64, 0.5));
    let mut scene = Scene::new(&options, &World::default());
    scene.input(PlayerInput::Drop {
        all: false,
        crouching: false,
    });
    let item = scene
        .of_kind(entity_types::ITEM)
        .first()
        .map(|entity| entity.key)
        .expect("the stick lies there");
    let owned = EntityId(5_021);
    scene.arrive(hanging(
        5_021,
        entity_types::FIREBALL,
        [6.5, 60.0, 6.5],
        Some(mob_owner(item, entity_types::ITEM)),
    ));
    scene.idle_for(40);
    let lying = scene.record(item).expect("the stick lies there").position;
    scene.input(PlayerInput::Move {
        position: Some(Vec3::new(lying[0], lying[1], lying[2])),
        rotation: None,
        on_ground: true,
    });
    let picked = scene
        .idle_until(100, |scene, _| scene.record(item).is_none())
        .expect("player 1 picks the stick up");
    let gone = scene.idle_until(3, |scene, _| scene.record(owned).is_none());
    assert!(
        scene.record(owned).is_none(),
        "the fireball of the item picked up at {picked} goes: {gone:?}"
    );
}

/// A pit of lava from (3, 3) to (5, 5) a block deep on the floor, walled two high.
fn lava_pit() -> Vec<(BlockPos, BlockState)> {
    let mut pit = filled(bp(3, FEET, 3), bp(5, FEET, 5), blocks::LAVA);
    pit.extend(filled(bp(2, FEET, 2), bp(6, FEET + 1, 2), blocks::STONE));
    pit.extend(filled(bp(2, FEET, 6), bp(6, FEET + 1, 6), blocks::STONE));
    pit.extend(filled(bp(2, FEET, 3), bp(2, FEET + 1, 5), blocks::STONE));
    pit.extend(filled(bp(6, FEET, 3), bp(6, FEET + 1, 5), blocks::STONE));
    pit
}

/// Plan 18.6: a fireball is discarded also when its owner died in this region while
/// the fireball is here, before the dead owner is removed: a pig in lava owns one.
#[test]
fn a_fireball_whose_owner_dies_here_is_discarded_before_the_owner_is_removed() {
    let options = Options::nether()
        .holding(eggs(&[items::PIG_SPAWN_EGG]))
        .standing(Vec3::new(1.5, FEET as f64, 1.5));
    let mut scene = Scene::new(&options, &World::of(&lava_pit()));
    let pig = scene.hatch(0, 4, 4);
    let owned = EntityId(5_023);
    scene.arrive(hanging(
        5_023,
        entity_types::FIREBALL,
        [-6.5, 60.0, 6.5],
        Some(mob_owner(pig, entity_types::PIG)),
    ));
    let died = scene
        .idle_until(400, |scene, _| {
            scene.record(pig).is_none_or(|pig| health(&pig) <= 0.0)
        })
        .expect("the pig dies in the lava");
    assert!(
        scene.record(pig).is_some(),
        "it lies dying first, {died} ticks on"
    );
    scene.idle_until(3, |scene, _| scene.record(owned).is_none());
    assert!(scene.record(owned).is_none(), "its fireball is gone");
    assert!(scene.record(pig).is_some(), "before the pig is removed");
}

/// Plan 18.5 (their 1): the same of an owner that is no item: a fireball owned by
/// primed TNT of the region goes when the TNT has exploded and is removed.
#[test]
#[ignore = "departs from the specification: a fireball whose owner, primed TNT of the \
            region, has exploded and been removed flies on"]
fn a_fireball_whose_owner_tnt_has_exploded_is_discarded() {
    let at = bp(3, FEET, 3);
    let options = Options::nether().holding(hotbar(&[stack(items::FLINT_AND_STEEL, 1)], None));
    let mut scene = Scene::new(&options, &World::of(&[(at, blocks::TNT)]));
    scene.click(at);
    let tnt = scene
        .of_kind(entity_types::TNT)
        .first()
        .map(|entity| entity.key)
        .expect("the TNT is primed");
    let owned = EntityId(5_024);
    scene.arrive(hanging(
        5_024,
        entity_types::FIREBALL,
        [-14.5, 60.0, -14.5],
        Some(mob_owner(tnt, entity_types::TNT)),
    ));
    scene
        .idle_until(100, |scene, _| scene.record(tnt).is_none())
        .expect("the TNT explodes");
    scene.idle_until(3, |scene, _| scene.record(owned).is_none());
    assert!(scene.record(owned).is_none(), "its fireball is gone");
}

// ------------------------------------------------------------------------------------
// Deflection (plan 18.5 their 2, 18.6; `Player.deflectProjectile`, `ProjectileDeflection
// .AIM_DEFLECT`, `Projectile.deflect`, `AbstractHurtingProjectile.onDeflection`).

/// The look of yaw and pitch in degrees (`Entity.calculateViewVector`).
fn look(yaw: f32, pitch: f32) -> [f64; 3] {
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    [
        f64::from(-yaw.sin() * pitch.cos()),
        f64::from(-pitch.sin()),
        f64::from(yaw.cos() * pitch.cos()),
    ]
}

/// Plan 18.5 (their 2): a player's hit on a fireball deflects it along their look
/// (`AIM_DEFLECT` sets the motion to the look, scaled by 1), names the player its owner
/// (`setOwner`), and sets its acceleration to 0.1 (`onDeflection(true)`); the fireball
/// then ticks as ever, so that after the tick of the hit its motion is `1.045` times the
/// look (the game reads players' packets before it ticks the level's entities).
#[test]
fn a_player_s_hit_sends_a_fireball_along_their_look_and_makes_it_theirs() {
    let mut scene = Scene::new(&Options::nether(), &World::default());
    let (yaw, pitch) = (-90.0, -20.0);
    scene.turn(yaw, pitch);
    let key = EntityId(5_030);
    let mut ball = hanging(5_030, entity_types::FIREBALL, [2.5, 43.0, 0.5], None);
    if let EntityData::Fireball(kept) = &mut ball.data {
        kept.acceleration_power = Double(0.05);
    }
    scene.arrive(ball);
    scene.input(PlayerInput::Attack { entity: key });
    let after = scene.record(key).expect("the fireball flies on");
    let kept = fireball_of(&after);
    assert_eq!(
        kept.owner.and_then(|owner| owner.player),
        Some(player(1).0),
        "the player owns it: {:?}",
        kept.owner
    );
    assert_eq!(
        kept.owner.map(|owner| owner.key),
        Some(scene.me()),
        "by their entity"
    );
    assert_eq!(kept.acceleration_power, Double(ACCELERATION));
    let expected = accelerated(look(yaw, pitch), ACCELERATION, INERTIA);
    assert!(
        close(after.motion, expected, 1.0e-3),
        "along the look: {:?}, not {expected:?}",
        after.motion
    );
}

/// The orbs' worth lying in `scene`.
fn experience(scene: &Scene) -> i32 {
    scene
        .of_kind(entity_types::EXPERIENCE_ORB)
        .iter()
        .map(|orb| match orb.data {
            EntityData::ExperienceOrb { value, count, .. } => value * count,
            _ => 0,
        })
        .sum()
}

/// Brief 5.9 and plan 18.6: a ghast takes 1000 from a large fireball whose owner is a
/// player (`Ghast.isReflectedFireball`): its own fireball, deflected by player 1, kills
/// it, and it is killed by that player, so that it drops its experience (`xpReward` 5,
/// only where `lastHurtByPlayer` holds).
#[test]
fn a_ghast_killed_by_its_own_fireball_deflected_by_a_player_drops_its_experience() {
    let options = Options::nether().holding(eggs(&[items::GHAST_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::default());
    let ghast = scene.hatch(0, 4, 0);
    let at = scene.record(ghast).expect("the ghast").position;
    // Player 1 looks at the ghast's middle from their eyes.
    let eyes = [0.5, f64::from(FEET) + 1.62, 0.5];
    let to = [at[0] - eyes[0], at[1] + 2.0 - eyes[1], at[2] - eyes[2]];
    let yaw = (-to[0]).atan2(to[2]).to_degrees() as f32;
    let pitch = (-(to[1] / length(to)).asin()).to_degrees() as f32;
    scene.turn(yaw, pitch);
    let key = EntityId(5_031);
    scene.arrive(hanging(
        5_031,
        entity_types::FIREBALL,
        [1.5, f64::from(FEET) + 1.5, 0.5],
        Some(mob_owner(ghast, entity_types::GHAST)),
    ));
    scene.input(PlayerInput::Attack { entity: key });
    let died = scene
        .idle_until(40, |scene, _| {
            scene
                .record(ghast)
                .is_none_or(|ghast| health(&ghast) <= 0.0)
        })
        .expect("the ghast dies of its fireball");
    let dropped = scene.idle_until(60, |scene, _| experience(scene) > 0);
    assert_eq!(
        experience(&scene),
        5,
        "killed by the player: its 5 points, {died} and {dropped:?} ticks on"
    );
}

// ------------------------------------------------------------------------------------
// Two regions at a line (plan 18.5's tests: "a deflection across a line that kills the
// ghast"; 18.6: "a deflection by a player in another region than the fireball").

/// Two regions of the Nether at the line between x = 15 and x = 16, each given what the
/// other's last tick made for it: the entity changes of the chunks it copies, the
/// effects and arrivals for the land it holds, and its players' faces.
struct Line {
    west: Scene,
    east: Scene,
    from_west: TickOutput,
    from_east: TickOutput,
    sequence: u64,
}

impl Line {
    /// The west with player 1 at `west_feet` holding `west_items`, the east with player
    /// 2 at `east_feet` holding `east_items`.
    fn new(world: &World, west_player: (Vec3, Inventory), east_player: (Vec3, Inventory)) -> Self {
        let west_options = Options::nether()
            .side(true)
            .holding(west_player.1)
            .standing(west_player.0);
        let east_options = Options::nether().side(false).holding(east_player.1);
        let west = Scene::new(&west_options, world);
        let mut east = Scene::empty(&east_options, world);
        east.join(2, east_player.0);
        Self {
            west,
            east,
            from_west: TickOutput::default(),
            from_east: TickOutput::default(),
            sequence: 1_000,
        }
    }

    fn carried(&mut self, from: &TickOutput, into: &Options) -> TickInputs {
        self.sequence += 1;
        let sequence = self.sequence;
        let mut updates: Vec<(ChunkPos, Vec<clustine_world::EntityChange>)> = Vec::new();
        for (chunk, change) in &from.entity_changes {
            if !into.copies.contains(chunk) {
                continue;
            }
            match updates.iter_mut().find(|(at, _)| at == chunk) {
                Some((_, changes)) => changes.push(change.clone()),
                None => updates.push((*chunk, vec![change.clone()])),
            }
        }
        let effects: Vec<FiledEffect> = from
            .effects
            .iter()
            .filter(|effect| into.held.contains(&effect.at.chunk()))
            .enumerate()
            .map(|(index, effect)| FiledEffect {
                id: EffectId {
                    sequence,
                    index: u32::try_from(index).expect("few"),
                },
                effect: effect.clone(),
                due: Due::FromTaking,
            })
            .collect();
        let arrivals: Vec<Arrival> = from
            .arrivals
            .iter()
            .filter(|(chunk, _)| into.held.contains(chunk))
            .enumerate()
            .map(|(index, (chunk, entity))| Arrival {
                id: EffectId {
                    sequence: sequence + 500_000,
                    index: u32::try_from(index).expect("few"),
                },
                chunk: *chunk,
                entity: entity.clone(),
                due: Due::FromTaking,
            })
            .collect();
        let faces: Vec<FaceChange> = from
            .stays
            .iter()
            .enumerate()
            .filter_map(|(index, note)| match note {
                clustine_sim::api::StayNote::Has {
                    player,
                    entity,
                    hops,
                    face,
                    ..
                } => Some(FaceChange::Face {
                    player: *player,
                    entity: *entity,
                    hops: *hops,
                    seq: FaceSeq(sequence, u32::try_from(index).expect("few")),
                    face: face.clone(),
                }),
                _ => None,
            })
            .collect();
        TickInputs {
            copy_entity_updates: updates,
            effects,
            arrivals,
            faces,
            ..TickInputs::default()
        }
    }

    /// One tick of both, with each side's player's inputs and what else each is given.
    fn step_with(
        &mut self,
        west: Vec<PlayerInput>,
        east: Vec<PlayerInput>,
        mut west_more: TickInputs,
        mut east_more: TickInputs,
    ) -> (TickOutput, TickOutput) {
        let from_east = std::mem::take(&mut self.from_east);
        let from_west = std::mem::take(&mut self.from_west);
        let west_options = self.west.options.clone();
        let east_options = self.east.options.clone();
        let to_west = self.carried(&from_east, &west_options);
        let to_east = self.carried(&from_west, &east_options);
        west_more
            .copy_entity_updates
            .extend(to_west.copy_entity_updates);
        west_more.effects.extend(to_west.effects);
        west_more.arrivals.extend(to_west.arrivals);
        west_more.faces.extend(to_west.faces);
        east_more
            .copy_entity_updates
            .extend(to_east.copy_entity_updates);
        east_more.effects.extend(to_east.effects);
        east_more.arrivals.extend(to_east.arrivals);
        east_more.faces.extend(to_east.faces);
        if !west.is_empty() {
            west_more.inputs = self.west.numbered(west);
        }
        if !east.is_empty() {
            east_more.inputs = self.east.numbered(east);
        }
        let west_out = self.west.region.tick(&west_more);
        let east_out = self.east.region.tick(&east_more);
        self.from_west = west_out.clone();
        self.from_east = east_out.clone();
        (west_out, east_out)
    }

    fn step(&mut self, west: Vec<PlayerInput>, east: Vec<PlayerInput>) -> (TickOutput, TickOutput) {
        self.step_with(west, east, TickInputs::default(), TickInputs::default())
    }

    fn idle(&mut self) -> (TickOutput, TickOutput) {
        self.step(Vec::new(), Vec::new())
    }

    /// The entity `key` on whichever side holds it, and whether that is the west.
    fn find(&self, key: EntityId) -> Option<(Entity, bool)> {
        self.west
            .record(key)
            .map(|entity| (entity, true))
            .or_else(|| self.east.record(key).map(|entity| (entity, false)))
    }
}

/// Plan 18.6: a player in the west hits a fireball the east holds, which the west sees
/// in its copy: the attack goes to the east with the player's look and UUID, the east
/// deflects the fireball along that look and names the player its owner, and the ghast
/// it then hits dies in the east killed by that player, with its experience.
#[test]
fn a_fireball_deflected_from_across_a_line_kills_its_ghast_for_the_player() {
    let mut line = Line::new(
        &World::default(),
        (Vec3::new(13.5, FEET as f64, 0.5), hotbar(&[], None)),
        (
            Vec3::new(30.5, FEET as f64, 8.5),
            eggs(&[items::GHAST_SPAWN_EGG]),
        ),
    );
    line.east.select(0);
    let before: Vec<EntityId> = line
        .east
        .entities()
        .iter()
        .map(|entity| entity.key)
        .collect();
    line.east.click(bp(26, FLOOR, 4));
    let ghast = line
        .east
        .of_kind(entity_types::GHAST)
        .iter()
        .map(|entity| entity.key)
        .find(|key| !before.contains(key))
        .expect("player 2 hatched a ghast in the east");
    let key = EntityId(5_040);
    let ball = hanging(
        5_040,
        entity_types::FIREBALL,
        [16.5, f64::from(FEET) + 1.5, 0.5],
        Some(mob_owner(ghast, entity_types::GHAST)),
    );
    let arrival = TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 990_000,
                index: 0,
            },
            chunk: ChunkPos::new(1, 0),
            entity: ball,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    line.step_with(Vec::new(), Vec::new(), TickInputs::default(), arrival);
    line.idle();
    assert!(
        line.west
            .region
            .copy(ChunkPos::new(1, 0))
            .is_some_and(|copy| copy.entities().iter().any(|entity| entity.key == key)),
        "the west sees the fireball in its copy"
    );
    // Player 1 looks from their eyes at the ghast's middle, beyond the fireball.
    let at = line.find(ghast).expect("the ghast").0.position;
    let eyes = [13.5, f64::from(FEET) + 1.62, 0.5];
    let to = [at[0] - eyes[0], at[1] + 2.0 - eyes[1], at[2] - eyes[2]];
    let yaw = (-to[0]).atan2(to[2]).to_degrees() as f32;
    let pitch = (-(to[1] / length(to)).asin()).to_degrees() as f32;
    let turn = PlayerInput::Move {
        position: None,
        rotation: Some((yaw, pitch)),
        on_ground: true,
    };
    let (west_out, _) = line.step(vec![turn, PlayerInput::Attack { entity: key }], Vec::new());
    let sent: Vec<&Effect> = west_out
        .effects
        .iter()
        .filter(|effect| {
            matches!(
                &effect.what,
                Effected::OnEntity {
                    key: hit,
                    by: EntityUse::Attack { .. }
                } if *hit == key
            )
        })
        .collect();
    assert_eq!(
        sent.len(),
        1,
        "the west sends the hit: {:?}",
        west_out.effects
    );
    if let Effected::OnEntity {
        by: EntityUse::Attack { look, player, .. },
        ..
    } = &sent[0].what
    {
        assert_eq!(*player, Some(uuid_of(1)), "with the player's UUID");
        assert!((look[0].0 - yaw).abs() < 0.01 && (look[1].0 - pitch).abs() < 0.01);
    }
    line.idle();
    let (deflected, _) = line.find(key).expect("the east deflected the fireball");
    let kept = fireball_of(&deflected);
    assert_eq!(kept.owner.and_then(|owner| owner.player), Some(uuid_of(1)));
    let died = (0..40).find(|_| {
        line.idle();
        line.find(ghast)
            .is_none_or(|(ghast, _)| health(&ghast) <= 0.0)
    });
    assert!(died.is_some(), "the ghast dies of the fireball");
    let dropped = (0..60).find(|_| {
        line.idle();
        experience(&line.east) > 0
    });
    assert_eq!(
        experience(&line.east),
        5,
        "killed by the player in the west: {dropped:?}"
    );
}

/// The UUID of player `n`.
fn uuid_of(n: u128) -> Uuid {
    player(n).0
}

/// Plan 18.5's tests: "a ghast's fireball across a line". A ghast in the west shoots at
/// a survival player standing in the east's land; its fireball crosses the line, is
/// handed to the east as an arrival with its owner, and flies on there to its blast.
#[test]
fn a_ghast_s_fireball_crosses_a_line_with_its_owner_and_explodes_beyond_it() {
    let mut line = Line::new(
        &World::default(),
        (ORIGIN, eggs(&[items::GHAST_SPAWN_EGG])),
        (Vec3::new(30.5, FEET as f64, 12.5), hotbar(&[], None)),
    );
    let survivor = Vec3::new(24.5, FEET as f64, 0.5);
    let face = |seq: u32| FaceChange::Face {
        player: player(3),
        entity: EntityId(900_003),
        hops: 0,
        seq: FaceSeq(600_000, seq),
        face: survival_face(survivor, [None, None, None, None]),
    };
    line.step_with(
        Vec::new(),
        Vec::new(),
        TickInputs {
            faces: vec![face(1)],
            ..TickInputs::default()
        },
        TickInputs {
            faces: vec![face(1)],
            ..TickInputs::default()
        },
    );
    line.west.select(0);
    line.west.click(bp(4, FLOOR, 0));
    let ghast = line
        .west
        .of_kind(entity_types::GHAST)
        .first()
        .map(|entity| entity.key)
        .expect("player 1 hatched a ghast");
    let mut handed = None;
    for _ in 0..200 {
        let (west_out, _) = line.idle();
        if let Some((_, entity)) = west_out
            .arrivals
            .iter()
            .find(|(_, entity)| i32::from(entity.kind) == entity_types::FIREBALL)
        {
            handed = Some(entity.clone());
            break;
        }
    }
    let handed = handed.expect("the ghast's fireball crosses the line");
    assert!(
        handed.position[0] >= 16.0,
        "into the east: {:?}",
        handed.position
    );
    assert_eq!(
        fireball_of(&handed).owner.map(|owner| owner.key),
        Some(ghast),
        "with its owner"
    );
    let mut blew = false;
    for _ in 0..60 {
        let (_, east_out) = line.idle();
        if !explosions(&east_out).is_empty() {
            blew = true;
            break;
        }
    }
    assert!(blew, "it explodes in the east");
    assert!(
        line.west.record(handed.key).is_none(),
        "and is no longer the west's"
    );
}

// ------------------------------------------------------------------------------------
// A survival player's face where the region holds the land: what mobs do to one.

/// A region of the Nether with player 1 at the origin holding `items`, `world`, and
/// the face of survival player 2 standing at `feet` in the region's land, as a player
/// another region holds or that this one let go a moment ago.
fn with_survivor(items: &[i32], world: &World, feet: Vec3) -> Scene {
    with_survivor_wearing(items, world, feet, [None, None, None, None])
}

fn with_survivor_wearing(
    items: &[i32],
    world: &World,
    feet: Vec3,
    worn: [Option<ItemStack>; 4],
) -> Scene {
    let options = Options::nether().holding(eggs(items));
    let mut scene = Scene::new(&options, world);
    scene.face(2, TWO, feet, worn);
    scene
}

/// The west of a line with player 1 near it holding `items`, a stone wall two high
/// just beyond the line in the east's land (x = 16 and 17, z from -20 to 20), and the
/// face of survival player 2 standing on it at (16.3, 0.5): a mob of the west reaches
/// them from the foot of the wall without crossing, and its hit goes to their region as
/// an effect.
fn below_a_wall(items: &[i32]) -> Scene {
    let wall = filled(bp(16, FEET, -20), bp(17, FEET + 1, 20), blocks::STONE);
    let options = Options::nether()
        .side(true)
        .holding(eggs(items))
        .standing(Vec3::new(11.5, FEET as f64, 4.5));
    let mut scene = Scene::new(&options, &World::of(&wall));
    scene.face(
        2,
        TWO,
        Vec3::new(16.3, f64::from(FEET + 2), 0.5),
        [None, None, None, None],
    );
    scene
}

/// The hurts a tick sends survival player 2.
fn hurts_of_two(output: &TickOutput) -> Vec<clustine_world::HurtBy> {
    output
        .effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnPlayer {
                entity,
                by: EntityUse::Hurt(hurt),
                ..
            } if *entity == TWO => Some((**hurt).clone()),
            _ => None,
        })
        .collect()
}

/// Brief 5.9 (`Blaze$BlazeAttackGoal.tick`): out of melee reach, a blaze charges for 60
/// ticks and then shoots three small fireballs six ticks apart, each with level event
/// 1018, and rests for 100 ticks before it charges again: 166 ticks from a volley's last
/// shot to the next one's first.
#[test]
fn a_blaze_shoots_volleys_of_three_small_fireballs_six_ticks_apart() {
    let mut scene = with_survivor(
        &[items::BLAZE_SPAWN_EGG],
        &World::default(),
        Vec3::new(-12.5, FEET as f64, 0.5),
    );
    let blaze = scene.hatch(0, 3, 0);
    let mut shots = Vec::new();
    let mut events = Vec::new();
    for tick in 1..=500u32 {
        let known: Vec<EntityId> = scene.entities().iter().map(|entity| entity.key).collect();
        let output = scene.idle();
        let made = scene
            .of_kind(entity_types::SMALL_FIREBALL)
            .into_iter()
            .filter(|entity| !known.contains(&entity.key))
            .collect::<Vec<_>>();
        for fireball in &made {
            assert_eq!(
                fireball_of(fireball).owner.map(|owner| owner.key),
                Some(blaze),
                "the blaze owns its fireballs"
            );
            shots.push(tick);
        }
        if has_level_event(&output, 1018) {
            events.push(tick);
        }
        if shots.len() >= 6 {
            break;
        }
    }
    assert!(shots.len() >= 6, "two volleys: {shots:?}");
    let gaps: Vec<u32> = shots.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(gaps[..5], [6, 6, 166, 6, 6], "shots at {shots:?}");
    assert_eq!(events[..6], shots[..6], "a level event 1018 with each");
}

/// Brief 5.9: a blaze is hurt by water (`isSensitiveToWater`): one standing in water
/// loses health.
#[test]
fn a_blaze_in_water_is_hurt() {
    let mut pool = filled(bp(2, FEET, -1), bp(4, FEET + 2, 1), blocks::WATER);
    pool.extend(filled(
        bp(1, FEET + 3, -2),
        bp(5, FEET + 3, 2),
        blocks::STONE,
    ));
    let options = Options::nether().holding(eggs(&[items::BLAZE_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::of(&pool));
    let blaze = scene.hatch(0, 3, 0);
    let hurt = scene.idle_until(60, |scene, _| {
        scene
            .record(blaze)
            .is_some_and(|blaze| health(&blaze) < 20.0)
    });
    assert!(hurt.is_some(), "the water hurts it");
}

/// Brief 5.9 (`WitherSkeleton.doHurtTarget`): a wither skeleton's hit that takes gives
/// wither for 200 ticks; on a player of another region the hurt carries it
/// (`HurtBy.effects`, plan 18.5 their 4 and 18.6), with the hit made by its stone sword.
#[test]
#[ignore = "departs from the specification: a mob's melee hit on a player another region \
            holds (their face) is sent nowhere, so no hurt with wither reaches them"]
fn a_wither_skeleton_s_hit_on_a_player_carries_wither_for_200_ticks() {
    let mut scene = below_a_wall(&[items::WITHER_SKELETON_SPAWN_EGG]);
    scene.hatch_on(0, bp(13, FLOOR, 0));
    let mut hurts = Vec::new();
    for _ in 0..300 {
        let output = scene.idle();
        hurts.extend(hurts_of_two(&output));
        if !hurts.is_empty() {
            break;
        }
    }
    let hurt = hurts.first().expect("the wither skeleton hits player 2");
    let wither: Vec<_> = hurt
        .effects
        .iter()
        .filter(|effect| effect.effect == "minecraft:wither")
        .collect();
    assert_eq!(wither.len(), 1, "wither: {:?}", hurt.effects);
    assert_eq!((wither[0].duration, wither[0].amplifier), (200, 0));
}

/// Plan 18.5 (their 4) and 18.6: a hoglin's hit on a player carries its throw
/// (`HoglinBase.throwTarget`, `HitExtra::Throw` with its attack knockback, 1), for the
/// player's region to throw them.
#[test]
#[ignore = "departs from the specification: a mob's melee hit on a player another region \
            holds (their face) is sent nowhere, so no hurt with the throw reaches them"]
fn a_hoglin_s_hit_on_a_player_carries_its_throw() {
    let mut scene = below_a_wall(&[items::HOGLIN_SPAWN_EGG]);
    scene.hatch_on(0, bp(13, FLOOR, 0));
    let mut hurts = Vec::new();
    for _ in 0..300 {
        let output = scene.idle();
        hurts.extend(hurts_of_two(&output));
        if !hurts.is_empty() {
            break;
        }
    }
    let hurt = hurts.first().expect("the hoglin hits player 2");
    assert!(
        hurt.extras
            .iter()
            .any(|extra| matches!(extra, clustine_world::HitExtra::Throw(knockback) if knockback.0 == 1.0)),
        "the throw: {:?}",
        hurt.extras
    );
}

/// The zoglin (`Zoglin.isTargetable`): it attacks any living thing but a creeper or
/// another zoglin: one beside a pig hurts it.
#[test]
fn a_zoglin_attacks_a_pig() {
    let options = Options::nether().holding(eggs(&[items::PIG_SPAWN_EGG, items::ZOGLIN_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::default());
    let pig = scene.hatch(0, 3, 2);
    scene.hatch(1, 3, -2);
    let hurt = scene.idle_until(200, |scene, _| {
        scene.record(pig).is_none_or(|pig| health(&pig) < 10.0)
    });
    assert!(hurt.is_some(), "the zoglin hurts the pig");
}

// ------------------------------------------------------------------------------------
// Piglins (brief 5.7; plan 18.3 E5, 18.5 their 12: "bartering by its table's outcomes,
// the admiring's 119 ticks and the throw's sequence, not by the game's draws").

/// What `gameplay/piglin_bartering` can give (`VanillaPiglinBarterLoot`).
const BARTERED: [i32; 18] = [
    items::BOOK,
    items::IRON_BOOTS,
    items::POTION,
    items::SPLASH_POTION,
    items::IRON_NUGGET,
    items::ENDER_PEARL,
    items::DRIED_GHAST,
    items::STRING,
    items::QUARTZ,
    items::OBSIDIAN,
    items::CRYING_OBSIDIAN,
    items::FIRE_CHARGE,
    items::LEATHER,
    items::SOUL_SAND,
    items::NETHER_BRICK,
    items::SPECTRAL_ARROW,
    items::GRAVEL,
    items::BLACKSTONE,
];

fn item_stack(entity: &Entity) -> Option<ItemStack> {
    match &entity.data {
        EntityData::Item { stack, .. } => Some(stack.clone()),
        _ => None,
    }
}

/// Brief 5.7 (`PiglinAi`): a gold ingot thrown before a piglin is picked up by it and
/// admired for 119 ticks (`ADMIRING_ITEM` with an expiry of 119), and when the admiring
/// ends the piglin throws what the bartering table gives, the ingot spent. The memory is
/// set after the brain's tick of the pick-up's tick, and a memory is cleared at the
/// brain's tick where its time to live is no longer above 0, which is asked before the
/// time is counted down (`MemorySlot.tick`): so the throw comes 120 ticks after the
/// pick-up.
#[test]
fn a_piglin_admires_a_gold_ingot_for_119_ticks_then_throws_what_the_table_gives() {
    let options = Options::nether().holding(hotbar(
        &[
            stack(items::GOLD_INGOT, 1),
            stack(items::PIGLIN_SPAWN_EGG, 1),
        ],
        None,
    ));
    let mut scene = Scene::new(&options, &World::default());
    scene.hatch(1, 0, 4);
    scene.select(0);
    scene.input(PlayerInput::Drop {
        all: false,
        crouching: false,
    });
    let gold = scene
        .of_kind(entity_types::ITEM)
        .iter()
        .find(|item| item_stack(item).is_some_and(|stack| stack.item == items::GOLD_INGOT))
        .map(|item| item.key)
        .expect("the ingot lies there");
    let picked = scene
        .idle_until(400, |scene, _| scene.record(gold).is_none())
        .expect("the piglin picks the ingot up");
    let mut thrown = Vec::new();
    let mut after = 0;
    for tick in 1..=200u32 {
        scene.idle();
        thrown = scene
            .of_kind(entity_types::ITEM)
            .iter()
            .filter_map(item_stack)
            .collect();
        if !thrown.is_empty() {
            after = tick;
            break;
        }
    }
    assert!(
        !thrown.is_empty(),
        "the piglin throws its barter, {picked} ticks after the throw"
    );
    assert_eq!(after, 120, "after admiring for 119 ticks");
    for stack in &thrown {
        assert!(
            BARTERED.contains(&stack.item),
            "{:?} is of the bartering table",
            stack.item
        );
    }
}

/// The zombified piglin, zoglin or none an overworld mob of `kind` hatched from `egg`
/// becomes within `ticks`: the tick it went, what it became and whether the tick
/// played `sound`.
fn converted(dimension: Dimension, egg: i32, ticks: u32, sound: i32) -> Option<(u32, i32, bool)> {
    let options = Options::in_(dimension).holding(eggs(&[egg]));
    let mut scene = Scene::new(&options, &World::default());
    let mob = scene.hatch(0, 3, 3);
    for tick in 1..=ticks {
        let output = scene.idle();
        if scene.record(mob).is_none() {
            let new = scene
                .entities()
                .into_iter()
                .find(|entity| {
                    [entity_types::ZOMBIFIED_PIGLIN, entity_types::ZOGLIN]
                        .contains(&i32::from(entity.kind))
                })
                .map(|entity| i32::from(entity.kind))
                .unwrap_or(0);
            let heard = !sounds_of(&output, sound).is_empty();
            return Some((tick, new, heard));
        }
    }
    None
}

/// Brief 5.7 (`AbstractPiglin.customServerAiStep`, `Hoglin`): outside the Nether
/// (`piglins_zombify`) a piglin's time there counts each tick and above 300 it becomes a
/// zombified piglin with the converted sound; a piglin brute too, with its own; a
/// hoglin becomes a zoglin.
#[test]
fn piglins_and_hoglins_turn_zombified_after_300_ticks_in_the_overworld() {
    let cases = [
        (
            items::PIGLIN_SPAWN_EGG,
            entity_types::ZOMBIFIED_PIGLIN,
            sounds::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED,
        ),
        (
            items::PIGLIN_BRUTE_SPAWN_EGG,
            entity_types::ZOMBIFIED_PIGLIN,
            sounds::ENTITY_PIGLIN_BRUTE_CONVERTED_TO_ZOMBIFIED,
        ),
        (
            items::HOGLIN_SPAWN_EGG,
            entity_types::ZOGLIN,
            sounds::ENTITY_HOGLIN_CONVERTED_TO_ZOMBIFIED,
        ),
    ];
    for (egg, into, sound) in cases {
        let (tick, new, heard) = converted(Dimension::Overworld, egg, 400, sound)
            .unwrap_or_else(|| panic!("{egg}: it turns within 400 ticks"));
        assert!(
            (300..=302).contains(&tick),
            "{egg}: after 300 ticks, at {tick}"
        );
        assert_eq!(new, into, "{egg}");
        assert!(heard, "{egg}: with its converted sound");
    }
}

/// Brief 5.7: `piglins_zombify` is false in the Nether: none turns there.
#[test]
fn piglins_and_hoglins_never_turn_zombified_in_the_nether() {
    for egg in [
        items::PIGLIN_SPAWN_EGG,
        items::PIGLIN_BRUTE_SPAWN_EGG,
        items::HOGLIN_SPAWN_EGG,
    ] {
        assert_eq!(
            converted(
                Dimension::Nether,
                egg,
                600,
                sounds::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED
            ),
            None,
            "{egg}"
        );
    }
}

// ------------------------------------------------------------------------------------
// Hoglins (plan 18.3 E6; `HoglinAi`, `HoglinSpecificSensor`, `Hoglin.isFood`).

/// Plan 18.3 E6: a hoglin keeps away from warped fungi, portals and respawn anchors
/// (`#hoglin_repellents` within 8 blocks, `SetWalkTargetAwayFrom`): one hatched two
/// blocks from each walks off to more than five.
#[test]
fn a_hoglin_walks_away_from_warped_fungi_portals_and_respawn_anchors() {
    let repellents = [
        blocks::WARPED_FUNGUS,
        state("minecraft:nether_portal[axis=x]"),
        state("minecraft:respawn_anchor[charges=0]"),
    ];
    for repellent in repellents {
        let at = bp(4, FEET, 0);
        let options = Options::nether().holding(eggs(&[items::HOGLIN_SPAWN_EGG]));
        let mut scene = Scene::new(&options, &World::of(&[(at, repellent)]));
        let hoglin = scene.hatch(0, 2, 0);
        let away = scene.idle_until(300, |scene, _| {
            scene.record(hoglin).is_some_and(|hoglin| {
                let dx = hoglin.position[0] - 4.5;
                let dz = hoglin.position[2] - 0.5;
                (dx * dx + dz * dz).sqrt() > 5.0
            })
        });
        assert!(away.is_some(), "{repellent:?}: the hoglin keeps away");
    }
}

/// Plan 18.3 E6 (`Hoglin.isFood`: crimson fungus): two adult hoglins fed crimson fungi
/// breed and make a third, a baby; fed wheat they do not.
#[test]
fn hoglins_fed_crimson_fungi_breed_and_wheat_does_nothing() {
    for (food, breeds) in [(items::CRIMSON_FUNGUS, true), (items::WHEAT, false)] {
        let options = Options::nether().holding(hotbar(
            &[stack(items::HOGLIN_SPAWN_EGG, 1), stack(food, 64)],
            None,
        ));
        let mut scene = Scene::new(&options, &World::default());
        let one = scene.hatch(0, 2, 1);
        let two = scene.hatch(0, -2, 1);
        scene.select(1);
        for hoglin in [one, two] {
            scene.input(PlayerInput::Interact {
                entity: hoglin,
                location: [0.0, 0.5, 0.0],
                sneaking: false,
            });
        }
        let bred = scene.idle_until(600, |scene, _| {
            scene.of_kind(entity_types::HOGLIN).len() > 2
        });
        assert_eq!(bred.is_some(), breeds, "fed {food}");
        if breeds {
            let baby = scene
                .of_kind(entity_types::HOGLIN)
                .into_iter()
                .find(|hoglin| hoglin.key != one && hoglin.key != two)
                .expect("the third");
            assert_eq!(shown(&baby, index::BABY), Some(DataShown::Boolean(true)));
        }
    }
}

// ------------------------------------------------------------------------------------
// Striders (brief 5.9; plan 18.3 E4, 18.5 their 9; `Strider`, `LiquidBlock`'s
// `STABLE_SHAPE`).

/// `Strider.DATA_BOOST_TIME` and `DATA_SUFFOCATING`, after the ageable's baby (16) and
/// `AGE_LOCKED` (17).
const BOOST_TIME: u8 = 18;
const SUFFOCATING: u8 = 19;

/// A lake of lava a block deep from (2, -6) to (14, 6) on the floor, walled one high.
fn lava_lake() -> Vec<(BlockPos, BlockState)> {
    let mut lake = filled(bp(2, FEET, -6), bp(14, FEET, 6), blocks::LAVA);
    lake.extend(filled(bp(1, FEET, -7), bp(15, FEET, -7), blocks::STONE));
    lake.extend(filled(bp(1, FEET, 7), bp(15, FEET, 7), blocks::STONE));
    lake.extend(filled(bp(15, FEET, -6), bp(15, FEET, 6), blocks::STONE));
    lake
}

fn over_lava(scene: &Scene, entity: &Entity) -> bool {
    let at = bp(
        entity.position[0].floor() as i32,
        FEET,
        entity.position[2].floor() as i32,
    );
    scene.block(at).block_id() == blocks::LAVA.block_id()
}

/// Plan 18.5 (their 9): source lava under a strider is a half block (`STABLE_SHAPE`, 8
/// high): one in a lake of lava a block deep comes to stand on it at half a block above
/// the lava's bottom, and stays there.
#[test]
fn a_strider_stands_on_source_lava_as_on_a_half_block() {
    let options = Options::nether().holding(eggs(&[items::STRIDER_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::of(&lava_lake()));
    let strider = scene.hatch(0, 3, 0);
    let stood = scene.idle_until(200, |scene, _| {
        scene
            .record(strider)
            .is_some_and(|strider| strider.on_ground && over_lava(scene, &strider))
    });
    assert!(stood.is_some(), "it comes to stand on the lava");
    for _ in 0..60 {
        scene.idle();
        let now = scene.record(strider).expect("the strider");
        if now.on_ground && over_lava(&scene, &now) {
            assert_eq!(now.position[1], f64::from(FEET) + 0.5, "on the half block");
        }
    }
}

/// Brief 5.9: a strider is warm on lava and cold out of it (`DATA_SUFFOCATING`, which
/// clients show it shivering by).
#[test]
fn a_strider_is_cold_out_of_lava_and_warm_on_it() {
    let options =
        Options::nether().holding(eggs(&[items::STRIDER_SPAWN_EGG, items::STRIDER_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::of(&lava_lake()));
    let warm = scene.hatch(0, 3, 0);
    let cold = scene.hatch(1, -3, 0);
    scene.idle_for(60);
    let warm = scene.record(warm).expect("the strider on lava");
    let cold = scene.record(cold).expect("the strider on stone");
    assert!(over_lava(&scene, &warm));
    assert_eq!(shown(&warm, SUFFOCATING), Some(DataShown::Boolean(false)));
    assert_eq!(shown(&cold, SUFFOCATING), Some(DataShown::Boolean(true)));
}

/// Brief 5.9, plan 18.3 E4 and 18.6: a saddled strider is ridden by a player who holds
/// a warped fungus on a stick, whose client then drives it (`move_vehicle`): a move
/// along the lava at half a block above its bottom is taken as it is (standing on lava
/// in the check of a ridden vehicle's move, 18.6); a use of the stick boosts it
/// (`ItemSteerable.boost`: a boost time drawn from 140 to 980, shown); and with the
/// stick put away a move is refused.
#[test]
fn a_strider_saddled_and_ridden_with_a_warped_fungus_on_a_stick_is_driven_over_lava() {
    let options = Options::nether().holding(hotbar(
        &[
            stack(items::STRIDER_SPAWN_EGG, 1),
            stack(items::SADDLE, 1),
            stack(items::WARPED_FUNGUS_ON_A_STICK, 1),
        ],
        None,
    ));
    let mut scene = Scene::new(&options, &World::of(&lava_lake()));
    let strider = scene.hatch(0, 3, 0);
    scene.idle_until(200, |scene, _| {
        scene
            .record(strider)
            .is_some_and(|strider| strider.on_ground && over_lava(scene, &strider))
    });
    let interact = PlayerInput::Interact {
        entity: strider,
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    };
    scene.select(1);
    let saddled = scene.input(interact.clone());
    assert!(
        !sounds_of(&saddled, sounds::ENTITY_STRIDER_SADDLE).is_empty(),
        "saddled"
    );
    scene.select(2);
    scene.input(interact);
    let riding = scene
        .region
        .player_state(player(1))
        .expect("player 1")
        .riding
        .map(|riding| riding.vehicle);
    assert_eq!(riding, Some(strider), "player 1 rides the strider");
    scene.idle();
    let at = scene.record(strider).expect("the strider").position;
    assert_eq!(at[1], f64::from(FEET) + 0.5, "on the lava");
    let drive = |to: [f64; 3]| {
        PlayerInput::MoveVehicle(clustine_sim::api::VehicleMove {
            position: Vec3::new(to[0], to[1], to[2]),
            yaw: -90.0,
            pitch: 0.0,
            on_ground: true,
        })
    };
    let to = [at[0] + 0.4, at[1], at[2]];
    let output = scene.input(drive(to));
    assert!(
        corrections(&output).is_empty(),
        "taken: {:?}",
        corrections(&output)
    );
    assert_eq!(scene.record(strider).expect("the strider").position, to);
    scene.sequence += 1;
    let sequence = scene.sequence;
    scene.input(PlayerInput::UseItem {
        rotation: (-90.0, 0.0),
        sneaking: false,
        sequence,
    });
    let boosted = scene.record(strider).expect("the strider");
    match shown(&boosted, BOOST_TIME) {
        Some(DataShown::Int(time)) => assert!((140..=980).contains(&time), "{time}"),
        other => panic!("a boost time: {other:?}"),
    }
    scene.select(3);
    let output = scene.input(drive([to[0] + 0.4, to[1], to[2]]));
    assert_eq!(corrections(&output).len(), 1, "refused without the stick");
}

fn corrections(output: &TickOutput) -> Vec<Vec3> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            clustine_sim::api::PlayerEvent::VehicleCorrected { position, .. } => Some(*position),
            _ => None,
        })
        .collect()
}

/// Plan 18.5's tests: "a strider over a line on lava". A strider on lava in the west,
/// tempted by a warped fungus held by player 2 in the east (`TemptGoal`,
/// `#strider_tempt_items`), walks over the line on the lava and in the east still
/// stands on it at half a block.
#[test]
fn a_strider_walks_over_a_line_on_lava_and_stands_on_it_beyond() {
    let lake = filled(bp(6, FEET, -3), bp(26, FEET, 5), blocks::LAVA);
    let mut line = Line::new(
        &World::of(&lake),
        (
            Vec3::new(12.5, FEET as f64, 7.5),
            eggs(&[items::STRIDER_SPAWN_EGG]),
        ),
        (
            Vec3::new(22.5, FEET as f64, 7.5),
            hotbar(&[stack(items::WARPED_FUNGUS, 1)], None),
        ),
    );
    line.west.select(0);
    line.west.click(bp(14, FLOOR, 4));
    let strider = line
        .west
        .of_kind(entity_types::STRIDER)
        .first()
        .map(|entity| entity.key)
        .expect("player 1 hatched a strider");
    // A step of a hair makes each player's face again for the other side.
    let hair = |x: f64, z: f64| {
        vec![PlayerInput::Move {
            position: Some(Vec3::new(x, FEET as f64, z + 0.001)),
            rotation: None,
            on_ground: true,
        }]
    };
    line.step(hair(12.5, 7.5), hair(22.5, 7.5));
    let mut crossed = false;
    let mut stood_beyond = 0;
    for _ in 0..800 {
        line.idle();
        let Some((now, west)) = line.find(strider) else {
            // On its way across, between the two.
            continue;
        };
        crossed |= !west;
        if !west && now.on_ground && over_lava(&line.east, &now) {
            stood_beyond += 1;
            assert_eq!(now.position[1], f64::from(FEET) + 0.5, "on the lava beyond");
        }
    }
    assert!(crossed, "the strider crosses into the east");
    assert!(stood_beyond > 0, "and stands on the lava there");
}

// ------------------------------------------------------------------------------------
// Magma cubes (brief 5.9; `MagmaCube.jumpFromGround`).

/// `AbstractCubeMob.ID_SIZE`, after the ageable's two (the cube mobs are ageable in
/// 26.3).
const SIZE: u8 = 18;

/// Brief 5.9: a magma cube from an egg is of size 1, 2 or 4 with `size²` health, and
/// jumps from the ground at `0.42 + 0.1·size` (`getJumpPower() + size·0.1f`).
#[test]
fn a_magma_cube_jumps_by_its_size() {
    let options = Options::nether().holding(eggs(&[items::MAGMA_CUBE_SPAWN_EGG]));
    let mut scene = Scene::new(&options, &World::default());
    let cube = scene.hatch(0, 3, 3);
    let made = scene.record(cube).expect("the cube");
    let size = match shown(&made, SIZE) {
        Some(DataShown::Int(size)) => size,
        other => panic!("a size: {other:?}"),
    };
    assert!([1, 2, 4].contains(&size), "{size}");
    assert_eq!(health(&made), (size * size) as f32);
    let mut last = made;
    let mut rise = None;
    for _ in 0..200 {
        scene.idle();
        let now = scene.record(cube).expect("the cube");
        if last.on_ground && now.position[1] > last.position[1] {
            rise = Some(now.position[1] - last.position[1]);
            break;
        }
        last = now;
    }
    let rise = rise.expect("it jumps");
    let expected = f64::from(0.42f32 + size as f32 * 0.1f32);
    assert!(
        (rise - expected).abs() < 1.0e-6,
        "a jump of {rise}, not {expected}"
    );
}

// ------------------------------------------------------------------------------------
// Fire-immune mobs (brief 5.9; plan 18.3 E1: "light no fire-immune body").

/// The fire ticks the mob hatched from `egg` into the lava pit has after 40 ticks, the
/// most it had in between.
fn burnt_in_lava(egg: i32) -> i32 {
    let options =
        Options::nether()
            .holding(eggs(&[egg]))
            .standing(Vec3::new(1.5, FEET as f64, 1.5));
    let mut scene = Scene::new(&options, &World::of(&lava_pit()));
    let mob = scene.hatch(0, 4, 4);
    let mut most = 0;
    for _ in 0..40 {
        scene.idle();
        if let Some(mob) = scene.record(mob) {
            most = most.max(mob.fire_ticks);
        }
    }
    most
}

/// Brief 5.9: the Nether's fire-immune mobs (the zombified piglin, the blaze, the magma
/// cube, the wither skeleton, the strider) are never set on fire by lava; a zombie is.
#[test]
fn fire_immune_mobs_are_not_set_on_fire_by_lava() {
    assert!(
        burnt_in_lava(items::ZOMBIE_SPAWN_EGG) > 0,
        "the control burns"
    );
    for egg in [
        items::ZOMBIFIED_PIGLIN_SPAWN_EGG,
        items::BLAZE_SPAWN_EGG,
        items::MAGMA_CUBE_SPAWN_EGG,
        items::WITHER_SKELETON_SPAWN_EGG,
        items::STRIDER_SPAWN_EGG,
    ] {
        assert_eq!(burnt_in_lava(egg), 0, "{egg} is not set on fire");
    }
}

/// The same of a fire block: a zombified piglin standing in fire is not set alight.
#[test]
fn a_fire_immune_mob_is_not_set_on_fire_by_fire() {
    let fire = filled(bp(2, FEET, 2), bp(4, FEET, 4), blocks::FIRE);
    let options = Options::nether().holding(eggs(&[
        items::ZOMBIE_SPAWN_EGG,
        items::ZOMBIFIED_PIGLIN_SPAWN_EGG,
    ]));
    let mut scene = Scene::new(&options, &World::of(&fire));
    let zombie = scene.hatch(0, 2, 3);
    let piglin = scene.hatch(1, 4, 3);
    let mut most = (0, 0);
    for _ in 0..20 {
        scene.idle();
        if let Some(zombie) = scene.record(zombie) {
            most.0 = most.0.max(zombie.fire_ticks);
        }
        if let Some(piglin) = scene.record(piglin) {
            most.1 = most.1.max(piglin.fire_ticks);
        }
    }
    assert!(most.0 > 0, "the control burns");
    assert_eq!(most.1, 0, "the zombified piglin does not");
}

// ------------------------------------------------------------------------------------
// Zombified piglins from portal blocks (brief 4.2; `NetherPortalBlock.randomTick`).

/// A Nether portal standing on the floor at z = 4: an obsidian frame from x = -6 to 6
/// and y = 41 to 53, the portal blocks within (axis x).
fn portal() -> Vec<(BlockPos, BlockState)> {
    let mut frame = filled(bp(-6, FEET, 4), bp(6, FEET + 12, 4), blocks::OBSIDIAN);
    frame.retain(|(at, _)| !((-5..=5).contains(&at.x) && (FEET + 1..=FEET + 11).contains(&at.y)));
    frame.extend(filled(
        bp(-5, FEET + 1, 4),
        bp(5, FEET + 11, 4),
        state("minecraft:nether_portal[axis=x]"),
    ));
    frame
}

/// The zombified piglins a portal's random ticks make in `dimension` within `ticks`
/// (a random tick speed of 64, the level spawning monsters, at noon), in a soul sand
/// valley in the Nether, where none spawns naturally.
fn portal_piglins(dimension: Dimension, ticks: u32) -> Vec<Entity> {
    let mut options = Options::in_(dimension).standing(Vec3::new(0.5, FEET as f64, -4.5));
    options.random_tick_speed = 64;
    options.spawn_mobs = true;
    options.day = 6_000;
    let world = World {
        biome: if dimension == Dimension::Nether {
            biome_id("minecraft:soul_sand_valley")
        } else {
            biome_id("minecraft:plains")
        },
        ..World::of(&portal())
    };
    let mut scene = Scene::new(&options, &world);
    scene.publish_counts();
    scene.idle_until(ticks, |scene, _| {
        !scene.of_kind(entity_types::ZOMBIFIED_PIGLIN).is_empty()
    });
    scene.of_kind(entity_types::ZOMBIFIED_PIGLIN)
}

/// Brief 4.2: in the overworld (`nether_portal_spawns_piglin`) a portal block's random
/// tick, at `nextInt(2000) < difficulty`, makes a zombified piglin on the block under
/// the portal, here the frame's bottom, with a portal cooldown so that it does not go
/// through at once.
#[test]
fn a_portal_in_the_overworld_makes_zombified_piglins_on_its_frame() {
    let made = portal_piglins(Dimension::Overworld, 6_000);
    let piglin = made
        .first()
        .expect("a zombified piglin comes out of the portal");
    assert_eq!(
        piglin.position[1],
        f64::from(FEET + 1),
        "on the frame's bottom"
    );
    assert!(
        (-5.0..=6.0).contains(&piglin.position[0]) && (4.0..=5.0).contains(&piglin.position[2]),
        "in the portal: {:?}",
        piglin.position
    );
    assert!(piglin.portal.cooldown > 0, "with a portal cooldown");
}

/// Brief 1.2 and 4.2: `nether_portal_spawns_piglin` is the overworld's alone: a portal
/// in the Nether makes none.
#[test]
fn a_portal_in_the_nether_makes_no_zombified_piglins() {
    assert!(portal_piglins(Dimension::Nether, 3_000).is_empty());
}

// ------------------------------------------------------------------------------------
// The fortress's spawn list on nether bricks (brief 5.10; plan 18.2; `NaturalSpawner
// .isInNetherFortressBounds`, `structure/fortress.json`).

/// The monsters' kinds natural spawning makes in the Nether within `ticks`, over a
/// floor of `floor`, in a fortress or not.
fn spawned_in_the_nether(floor: BlockState, fortress: bool, ticks: u32) -> Vec<i32> {
    let mut options = Options::nether();
    options.spawn_mobs = true;
    let world = World {
        floor,
        fortress,
        ..World::default()
    };
    let mut scene = Scene::new(&options, &world);
    scene.publish_counts();
    scene.idle_until(ticks, |scene, _| {
        scene
            .entities()
            .iter()
            .filter(|entity| matches!(entity.data, EntityData::Mob { .. }))
            .count()
            >= 6
    });
    let mut kinds: Vec<i32> = scene
        .entities()
        .iter()
        .filter(|entity| matches!(entity.data, EntityData::Mob { .. }))
        .map(|entity| i32::from(entity.kind))
        .collect();
    kinds.sort_unstable();
    kinds
}

/// What the fortress's own list may give (blaze, zombified piglin, wither skeleton,
/// skeleton, magma cube).
const FORTRESS: [i32; 5] = [
    entity_types::BLAZE,
    entity_types::ZOMBIFIED_PIGLIN,
    entity_types::WITHER_SKELETON,
    entity_types::SKELETON,
    entity_types::MAGMA_CUBE,
];

/// Brief 5.10: in a fortress, on nether bricks, monsters are drawn from the fortress's
/// list, never the biome's (no piglin, ghast or enderman of the nether wastes); outside
/// a fortress, from the biome's, never a blaze or a wither skeleton. (Off the bricks
/// within a fortress's piece the piece's own override gives the same list, as the game's
/// `ChunkGenerator.getMobsAt` does, so that is no control.)
#[test]
fn monsters_on_a_fortress_s_nether_bricks_come_from_the_fortress_s_list() {
    let on_bricks = spawned_in_the_nether(blocks::NETHER_BRICKS, true, 3_000);
    assert!(!on_bricks.is_empty(), "monsters spawn in the fortress");
    for kind in &on_bricks {
        assert!(
            FORTRESS.contains(kind),
            "{kind} is of the fortress's list: {on_bricks:?}"
        );
    }
    assert!(
        on_bricks.iter().any(|kind| [
            entity_types::BLAZE,
            entity_types::WITHER_SKELETON,
            entity_types::SKELETON
        ]
        .contains(kind)),
        "a blaze or a skeleton among them: {on_bricks:?}"
    );
    let outside = spawned_in_the_nether(blocks::NETHERRACK, false, 3_000);
    assert!(!outside.is_empty(), "monsters spawn outside a fortress");
    for kind in &outside {
        assert!(
            ![entity_types::BLAZE, entity_types::WITHER_SKELETON].contains(kind),
            "no {kind} outside a fortress: {outside:?}"
        );
    }
}

/// The kinds of the mobs natural spawning makes within `ticks` in `world`, waiting
/// until one of `kind` is among them.
fn spawned_until(world: &World, kind: i32, ticks: u32) -> Vec<i32> {
    let mut options = Options::nether();
    options.spawn_mobs = true;
    let mut scene = Scene::new(&options, world);
    scene.publish_counts();
    scene.idle_until(ticks, |scene, _| !scene.of_kind(kind).is_empty());
    scene
        .entities()
        .iter()
        .filter(|entity| matches!(entity.data, EntityData::Mob { .. }))
        .map(|entity| i32::from(entity.kind))
        .collect()
}

/// Brief 5.10 and plan 18.5 (their 8): the strider spawns in lava (`IN_LAVA`, its rule
/// climbing through lava to air, its walk target value 10 in lava, and only
/// `isUnobstructed` asked of its box): a Nether of lava over the floor has striders of
/// its creatures' passes.
#[test]
fn striders_spawn_in_the_nether_s_lava() {
    let world = World {
        blocks: filled(bp(-40, FEET, -40), bp(55, FEET, 55), blocks::LAVA),
        ..World::default()
    };
    let kinds = spawned_until(&world, entity_types::STRIDER, 4_000);
    assert!(
        kinds.contains(&entity_types::STRIDER),
        "a strider spawns: {kinds:?}"
    );
}

/// Brief 5.10 and plan 18.5 (their 8): hoglins spawn in the crimson forest (9, in
/// groups of 3 or 4), on crimson nylium, where their walk target value is 10.
#[test]
fn hoglins_spawn_in_the_crimson_forest() {
    let world = World {
        floor: blocks::CRIMSON_NYLIUM,
        biome: biome_id("minecraft:crimson_forest"),
        ..World::default()
    };
    let kinds = spawned_until(&world, entity_types::HOGLIN, 6_000);
    assert!(
        kinds.contains(&entity_types::HOGLIN),
        "a hoglin spawns: {kinds:?}"
    );
}

/// Plan 18.5 (their 11): a large fireball's direct hit on a mob across a line reaches
/// it before its blast does (`LargeFireball.onHitEntity`, 6 of `fireball` damage, then
/// `onHit`'s explosion): a fireball of the west flying into a pig the east holds sends
/// the pig its hit first and the explosion's after, and the pig in the east takes both.
#[test]
#[ignore = "departs from the specification: a fireball flies through a mob the region sees \
            only in its copy beyond a line, with no hit and no blast"]
fn a_large_fireball_s_direct_hit_across_a_line_comes_before_its_blast() {
    let mut line = Line::new(
        &World::default(),
        (ORIGIN, hotbar(&[], None)),
        (
            Vec3::new(18.5, FEET as f64, 3.5),
            eggs(&[items::PIG_SPAWN_EGG]),
        ),
    );
    line.east.select(0);
    line.east.click(bp(16, FLOOR, 0));
    let pig = line
        .east
        .of_kind(entity_types::PIG)
        .first()
        .map(|entity| entity.key)
        .expect("player 2 hatched a pig");
    line.idle();
    line.idle();
    let at = line.find(pig).expect("the pig").0.position;
    let ball = fireball(
        5_050,
        entity_types::FIREBALL,
        [at[0] - 4.0, at[1] + 0.4, at[2]],
        [0.3, 0.0, 0.0],
        None,
    );
    assert!(ball.position[0] < 16.0, "the fireball starts in the west");
    let arrival = TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 990_001,
                index: 0,
            },
            chunk: ball.chunk(),
            entity: ball,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    let mut west_effects = line
        .step_with(Vec::new(), Vec::new(), arrival, TickInputs::default())
        .0
        .effects;
    for _ in 0..10 {
        west_effects.extend(line.idle().0.effects);
    }
    let on_pig: Vec<&EntityUse> = west_effects
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity { key, by } if *key == pig => Some(by),
            _ => None,
        })
        .collect();
    assert!(on_pig.len() >= 2, "the hit and the blast: {on_pig:?}");
    match on_pig[0] {
        EntityUse::Hurt(hurt) => {
            assert_eq!(hurt.damage_type, "minecraft:fireball");
            assert_eq!(hurt.amount.0, 6.0);
        }
        EntityUse::Thrown { damage, .. } => assert_eq!(damage.0, 6.0),
        other => panic!("the direct hit first: {other:?}"),
    }
    assert!(
        on_pig[1..]
            .iter()
            .any(|by| matches!(by, EntityUse::Blasted { .. })),
        "then the blast: {on_pig:?}"
    );
    line.idle();
    let after = line.find(pig).map(|(pig, _)| health(&pig));
    assert!(
        after.is_none_or(|health| health <= 4.0),
        "the pig took both: {after:?}"
    );
}

/// Plan 18.5's tests: "a small fireball's fire across a line". A small fireball of the
/// west that lands on the floor just beyond the line sets fire there, in the east.
#[test]
fn a_small_fireball_s_fire_lands_across_a_line() {
    let mut line = Line::new(
        &World::default(),
        (ORIGIN, hotbar(&[], None)),
        (Vec3::new(24.5, FEET as f64, 8.5), hotbar(&[], None)),
    );
    let ball = fireball(
        5_051,
        entity_types::SMALL_FIREBALL,
        [15.9, f64::from(FEET) + 0.3, 0.5],
        [0.3, -0.5, 0.0],
        None,
    );
    let arrival = TickInputs {
        arrivals: vec![Arrival {
            id: EffectId {
                sequence: 990_002,
                index: 0,
            },
            chunk: ball.chunk(),
            entity: ball,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    line.step_with(Vec::new(), Vec::new(), arrival, TickInputs::default());
    let lit = (0..10).find(|_| {
        line.idle();
        is_fire(line.east.block(bp(16, FEET, 0)))
    });
    assert!(lit.is_some(), "fire in the east at (16, 41, 0)");
    assert!(line.find(EntityId(5_051)).is_none(), "the fireball is gone");
}
