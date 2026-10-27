//! Tests of what a creative player places and ties in step N5, written from
//! `docs/groundwork/n5-plan.md` (decision 10, sections 7 and 8), the porting brief
//! `docs/groundwork/n5-brief.md` (sections 5.1 to 5.4, 6 and 7) and the public API
//! alone, by someone who did not write the code: item frames and glow item frames hung
//! on every face, refused where something is in the way, taking an item, turning it,
//! read by a comparator through a wall and losing their item and then themselves to two
//! hits; paintings drawn from the largest variants that fit; armour stands placed facing
//! the player, refused where an entity stands, dressed by the item's slot and undressed
//! by the click's height, and broken at once by a creative hit with nothing dropped;
//! boats on leads, pulled, snapped beyond 12 blocks, tied to fences through knots; and a
//! player's use of an entity in another region's land, which is an effect on it there.
//!
//! Every player of Clustine is a creative one (the plan, section 7 item 11), so what the
//! game drops only for a survival player is never expected here. Player 1 stands on a
//! stone floor at y = -61 (feet at y = -60), in chunk (0, 0), whose 5x5 is loaded so that
//! its entities tick.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_sim::api::{Face, HOTBAR_SLOTS, Inventory, ItemStack, PlayerInput, RegionEvent};
use clustine_sim::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerJoin, Region, RegionConfig, RegionState,
    TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    Attached, BlockPos, Chunk, ChunkArea, ChunkPos, Due, EdgeId, Effect, EffectId, Effected,
    Entity, EntityData, EntityId, EntityIds, EntityUse, FiledEffect, Float, Hurt, LeashHolder,
    PlayerId, RegionId, Section, Vec3,
};
use uuid::Uuid;

const EDGE: EdgeId = EdgeId(7);
const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

/// The game's `EquipmentSlot` ordinals, by which a stand's equipment is kept.
const MAINHAND: u8 = 0;
const FEET: u8 = 2;
const LEGS: u8 = 3;
const CHEST: u8 = 4;
const HEAD: u8 = 5;

/// The 3D data values of the directions (`Direction.get3DDataValue`).
const DOWN: u8 = 0;
const UP: u8 = 1;
const NORTH: u8 = 2;
const SOUTH: u8 = 3;
const WEST: u8 = 4;
const EAST: u8 = 5;

// ------------------------------------------------------------------------------------
// The world.

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn stack(item: i32, count: i32) -> ItemStack {
    ItemStack::new(item, count)
}

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap_or_else(|| panic!("{name} is a block state"))
}

fn config(hotbar: [Option<ItemStack>; HOTBAR_SLOTS]) -> RegionConfig {
    RegionConfig {
        spawn: SPAWN,
        starting_inventory: Inventory::with_hotbar(hotbar),
        return_after: 0,
        lowest_y: -64,
        seed: 0,
        game_seed: 0,
        sea_level: 63,
        light: Default::default(),
        features: Default::default(),
    }
}

/// An overworld column with a stone floor at y = -61 and air above.
fn floor() -> Chunk {
    let sections = (0..24)
        .map(|_| Section::filled(blocks::AIR, clustine_world::Biome(0)))
        .collect();
    let mut chunk = Chunk::from_sections(-64, sections);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, -61, z, blocks::STONE);
        }
    }
    chunk
}

fn around(radius: i32) -> Vec<ChunkPos> {
    (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// Blocks set into the floor's chunks, by world position.
type Blocks = Vec<(BlockPos, BlockState)>;

/// The chunk at `position`: a floor with the `blocks` of it and the `entities` of it.
fn chunk_at(position: ChunkPos, blocks: &[(BlockPos, BlockState)], entities: &[Entity]) -> Chunk {
    let mut chunk = floor();
    for (at, state) in blocks {
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
        entities
            .iter()
            .filter(|entity| entity.chunk() == position)
            .cloned()
            .collect(),
    );
    chunk
}

/// A region and player 1 in it, with a counter of their inputs and their actions.
struct Scene {
    region: Region,
    me: EntityId,
    input: u64,
    sequence: i32,
}

/// The chunks of one region's scene.
struct Layout {
    pinned: ChunkArea,
    held: Vec<ChunkPos>,
    loaded: Vec<ChunkPos>,
    copies: Vec<ChunkPos>,
    foreign: Vec<ChunkPos>,
    block: u32,
}

impl Layout {
    /// One region over all the land near the origin.
    fn alone() -> Self {
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
    /// side, with copies of the other's chunks, which another region holds.
    fn side(west: bool) -> Self {
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
}

/// A region of `layout`, its chunks with `blocks` and `entities`, nobody in it.
fn region_of(
    layout: &Layout,
    blocks: &[(BlockPos, BlockState)],
    entities: &[Entity],
    hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
) -> Region {
    let mut state = RegionState::new(EntityIds::block(layout.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let holdings = Holdings {
        held: layout.held.clone(),
        pinned: vec![layout.pinned],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let mut region = Region::restore(config(hotbar), state, holdings);
    region.tick(&TickInputs {
        tickets_added: layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: layout
            .loaded
            .iter()
            .map(|position| (*position, chunk_at(*position, blocks, entities)))
            .collect(),
        copy_bases: layout
            .copies
            .iter()
            .map(|position| (*position, chunk_at(*position, blocks, entities)))
            .collect(),
        foreign: layout
            .foreign
            .iter()
            .map(|position| (*position, RegionId(9)))
            .collect(),
        ..TickInputs::default()
    });
    for chunk in &layout.loaded {
        assert!(region.chunk(*chunk).is_some(), "{chunk:?} is loaded");
    }
    region
}

impl Scene {
    /// A region of `layout`, its chunks with `blocks` and `entities`, and player 1 in it
    /// at `feet`, holding `hotbar` with slot 0 selected.
    fn new(
        layout: Layout,
        blocks: &[(BlockPos, BlockState)],
        entities: &[Entity],
        hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
        feet: Vec3,
    ) -> Self {
        let region = region_of(&layout, blocks, entities, hotbar);
        Self::join(region, feet)
    }

    /// Player 1 joins `region` and is put at `feet`.
    fn join(mut region: Region, feet: Vec3) -> Self {
        region.tick(&TickInputs {
            player_changes: vec![PlayerChange::Join(
                EDGE,
                PlayerJoin {
                    player: player(1),
                    name: "Placer".to_owned(),
                    attempt: 7001,
                },
            )],
            ..TickInputs::default()
        });
        let entered = region
            .state()
            .entering
            .iter()
            .map(|(player, entering)| Entered {
                unit: None,
                player: *player,
                entity: entering.entity_id,
                place: None,
                holder: None,
            })
            .collect();
        region.tick(&TickInputs {
            entered,
            ..TickInputs::default()
        });
        let me = region.player(player(1)).expect("player 1 entered").0;
        let mut scene = Self {
            region,
            me,
            input: 0,
            sequence: 0,
        };
        scene.inputs(vec![PlayerInput::Move {
            position: Some(feet),
            rotation: Some((0.0, 0.0)),
            on_ground: true,
        }]);
        assert_eq!(
            scene.region.player(player(1)).expect("there").1.position,
            feet,
            "player 1 stands where the test puts them"
        );
        scene
    }

    /// One region over the land near the origin, player 1 at `feet`.
    fn alone(
        blocks: &[(BlockPos, BlockState)],
        entities: &[Entity],
        hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
        feet: Vec3,
    ) -> Self {
        Self::new(Layout::alone(), blocks, entities, hotbar, feet)
    }

    /// One tick with player 1's `inputs`.
    fn inputs(&mut self, inputs: Vec<PlayerInput>) -> TickOutput {
        let inputs = inputs
            .into_iter()
            .map(|input| {
                self.input += 1;
                (EDGE, player(1), self.me, self.input, input)
            })
            .collect();
        self.region.tick(&TickInputs {
            inputs,
            ..TickInputs::default()
        })
    }

    fn input(&mut self, input: PlayerInput) -> TickOutput {
        self.inputs(vec![input])
    }

    fn idle(&mut self) -> TickOutput {
        self.region.tick(&TickInputs::default())
    }

    fn idle_for(&mut self, ticks: u32) -> Vec<TickOutput> {
        (0..ticks).map(|_| self.idle()).collect()
    }

    /// Player 1 clicks `face` of the block at `position` with what they hold.
    fn click(&mut self, position: BlockPos, face: Face) -> TickOutput {
        self.sequence += 1;
        let cursor = match face {
            Face::Bottom => [0.5, 0.0, 0.5],
            Face::Top => [0.5, 1.0, 0.5],
            Face::North => [0.5, 0.5, 0.0],
            Face::South => [0.5, 0.5, 1.0],
            Face::West => [0.0, 0.5, 0.5],
            Face::East => [1.0, 0.5, 0.5],
        };
        self.input(PlayerInput::UseItemOn {
            position,
            face,
            cursor,
            sneaking: false,
            sequence: self.sequence,
        })
    }

    fn select(&mut self, slot: u8) {
        self.input(PlayerInput::SelectSlot { slot });
    }

    fn interact(&mut self, entity: EntityId, location: [f64; 3], sneaking: bool) -> TickOutput {
        self.input(PlayerInput::Interact {
            entity,
            location,
            sneaking,
        })
    }

    fn attack(&mut self, entity: EntityId) -> TickOutput {
        self.input(PlayerInput::Attack { entity })
    }

    fn walk_to(&mut self, feet: Vec3) -> TickOutput {
        self.input(PlayerInput::Move {
            position: Some(feet),
            rotation: None,
            on_ground: true,
        })
    }

    /// Every entity of the region's loaded chunks.
    fn all(&self) -> Vec<Entity> {
        all_of(&self.region)
    }

    /// The entities of the type `kind`.
    fn of_kind(&self, kind: i32) -> Vec<Entity> {
        of_kind(&self.region, kind)
    }

    /// The one entity of the type `kind`.
    fn one(&self, kind: i32) -> Entity {
        let found = self.of_kind(kind);
        assert_eq!(found.len(), 1, "one of the kind {kind}: {found:?}");
        found.into_iter().next().expect("one")
    }

    fn record(&self, key: EntityId) -> Option<Entity> {
        self.all().into_iter().find(|entity| entity.key == key)
    }

    /// What player 1 holds in the hotbar slot `slot`.
    fn holds(&self, slot: usize) -> Option<ItemStack> {
        self.region
            .player_state(player(1))
            .expect("player 1 is there")
            .inventory
            .0[slot]
            .clone()
    }
}

fn all_of(region: &Region) -> Vec<Entity> {
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

fn of_kind(region: &Region, kind: i32) -> Vec<Entity> {
    all_of(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == kind)
        .collect()
}

/// The stacks of the item entities lying about.
fn items_lying(entities: &[Entity]) -> Vec<ItemStack> {
    entities
        .iter()
        .filter_map(|entity| match &entity.data {
            EntityData::Item { stack, .. } => Some(stack.clone()),
            _ => None,
        })
        .collect()
}

fn frame_of(entity: &Entity) -> (Attached, Option<ItemStack>, u8) {
    match &entity.data {
        EntityData::ItemFrame {
            attached,
            item,
            rotation,
            ..
        } => (*attached, item.clone(), *rotation),
        other => panic!("not an item frame: {other:?}"),
    }
}

fn equipment_of(entity: &Entity) -> Vec<(u8, ItemStack)> {
    match &entity.data {
        EntityData::ArmorStand { equipment, .. } => equipment.clone(),
        other => panic!("not an armour stand: {other:?}"),
    }
}

fn leash_of(entity: &Entity) -> Option<LeashHolder> {
    entity.riding.leash.as_ref().map(|leash| leash.holder)
}

fn power_at(region: &Region, at: BlockPos) -> u8 {
    let chunk = region.chunk(at.chunk()).expect("loaded");
    let state = chunk
        .get(
            at.x.rem_euclid(16) as usize,
            at.y,
            at.z.rem_euclid(16) as usize,
        )
        .expect("in the world");
    state
        .property("power")
        .unwrap_or_else(|| panic!("{state:?} has a power"))
        .parse()
        .expect("a number")
}

fn hotbar(slots: &[(usize, ItemStack)]) -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [const { None }; HOTBAR_SLOTS];
    for (slot, stack) in slots {
        hotbar[*slot] = Some(stack.clone());
    }
    hotbar
}

fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// A key from a block no region of these tests is given.
fn key(n: u32) -> EntityId {
    EntityId((9 << 32) + u64::from(n))
}

fn entity(n: u32, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(kind).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    }
}

/// An item of one dirt lying at `position`, never to be picked up.
fn lying_item(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ITEM,
        position,
        EntityData::Item {
            stack: stack(items::DIRT, 1),
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    )
}

/// An oak boat on the floor at `position`, at rest.
fn boat(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::OAK_BOAT,
        position,
        EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            // `Status.ON_LAND`.
            status: [3, 3],
            contents: Vec::new(),
        },
    )
}

/// An armour stand of the default pose on the floor at `position`, wearing `equipment`.
fn stand(n: u32, position: [f64; 3], equipment: Vec<(u8, ItemStack)>) -> Entity {
    let pose = [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [-10.0, 0.0, -10.0],
        [-15.0, 0.0, 10.0],
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
    ];
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new(pose.map(|part: [f32; 3]| part.map(Float))),
            equipment,
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

/// An empty item frame hanging on the west face of the block east of `pos`.
fn frame_facing_west(n: u32, pos: BlockPos, item: Option<ItemStack>) -> Entity {
    let mut frame = entity(
        n,
        entity_types::ITEM_FRAME,
        [
            f64::from(pos.x) + 0.5 + 0.46875,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ],
        EntityData::ItemFrame {
            attached: Attached {
                block: pos,
                direction: WEST,
            },
            item,
            rotation: 0,
            drop_chance: Float(1.0),
            fixed: false,
            invisible: false,
        },
    );
    frame.yaw = 90.0;
    frame
}

/// The tick's `RegionEvent::Leashed` of `entity`, in order.
fn leashed_events(output: &TickOutput, entity: EntityId) -> Vec<Option<EntityId>> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Leashed {
                entity: leashee,
                holder,
                ..
            } if *leashee == entity => Some(*holder),
            _ => None,
        })
        .collect()
}

/// Idles `scene` until `done` holds, for at most `ticks` ticks, and says whether it did.
fn idle_until(scene: &mut Scene, ticks: u32, done: impl Fn(&Scene) -> bool) -> bool {
    for _ in 0..ticks {
        if done(scene) {
            return true;
        }
        scene.idle();
    }
    done(scene)
}

// ------------------------------------------------------------------------------------
// Item frames (the brief, sections 6.0, 6.3, 6.5 and 6.6).

/// A stone block with air on every side, at (8, -59, 10), and where player 1 stands
/// beside it.
const CUBE: BlockPos = BlockPos::new(8, -59, 10);
const BY_THE_CUBE: Vec3 = Vec3::new(8.5, -60.0, 7.5);

fn frames_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    hotbar(&[
        (0, stack(items::ITEM_FRAME, 1)),
        (1, stack(items::STONE, 64)),
        (2, stack(items::DIRT, 64)),
        (3, stack(items::GLOW_ITEM_FRAME, 1)),
    ])
}

/// Brief 6.3 and 6.5: a frame goes on any of the six faces, into the block in front of
/// the face, facing out of it, its position the centre of its box, 0.46875 back
/// towards the block from the centre of the block it is in; a horizontal one turned by
/// its direction, one on a floor looking up and one under a ceiling down. Six frames of
/// six directions coexist.
#[test]
fn an_item_frame_hangs_on_each_of_the_six_faces_of_a_block_facing_out_of_it() {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    let faces = [
        (
            Face::North,
            bp(8, -59, 9),
            NORTH,
            [8.5, -58.5, 9.968_75],
            (180.0, 0.0),
        ),
        (
            Face::South,
            bp(8, -59, 11),
            SOUTH,
            [8.5, -58.5, 11.031_25],
            (0.0, 0.0),
        ),
        (
            Face::West,
            bp(7, -59, 10),
            WEST,
            [7.968_75, -58.5, 10.5],
            (90.0, 0.0),
        ),
        (
            Face::East,
            bp(9, -59, 10),
            EAST,
            [9.031_25, -58.5, 10.5],
            (270.0, 0.0),
        ),
        (
            Face::Top,
            bp(8, -58, 10),
            UP,
            [8.5, -57.968_75, 10.5],
            (0.0, -90.0),
        ),
        (
            Face::Bottom,
            bp(8, -60, 10),
            DOWN,
            [8.5, -59.031_25, 10.5],
            (0.0, 90.0),
        ),
    ];
    for (face, pos, direction, position, (yaw, pitch)) in faces {
        scene.click(CUBE, face);
        let hung: Vec<Entity> = scene
            .of_kind(entity_types::ITEM_FRAME)
            .into_iter()
            .filter(|frame| frame_of(frame).0.direction == direction)
            .collect();
        assert_eq!(hung.len(), 1, "one frame facing {direction}: {hung:?}");
        let frame = &hung[0];
        let (attached, item, rotation) = frame_of(frame);
        assert_eq!(
            attached.block, pos,
            "the frame facing {direction} is in front of the face"
        );
        assert_eq!(
            (item, rotation),
            (None, 0),
            "a new frame is empty and unturned"
        );
        for axis in 0..3 {
            assert!(
                close(frame.position[axis], position[axis]),
                "the frame facing {direction} is at {:?}, not {position:?}",
                frame.position
            );
        }
        assert_eq!(
            (frame.yaw.rem_euclid(360.0), frame.pitch),
            (yaw, pitch),
            "the frame facing {direction} is turned by its direction"
        );
    }
    assert_eq!(scene.of_kind(entity_types::ITEM_FRAME).len(), 6);
    assert_eq!(
        scene.holds(0),
        Some(stack(items::ITEM_FRAME, 1)),
        "a creative player's stack is never used up (brief 6.3)"
    );
}

/// Brief 6.0 item 10: a glow item frame is placed as a frame is, by its own item, as
/// an entity of its own type.
#[test]
fn a_glow_item_frame_is_hung_as_an_entity_of_its_own_type() {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    scene.select(3);
    scene.click(CUBE, Face::North);
    let glowing = scene.one(entity_types::GLOW_ITEM_FRAME);
    assert_eq!(
        frame_of(&glowing).0,
        Attached {
            block: bp(8, -59, 9),
            direction: NORTH
        }
    );
    assert!(scene.of_kind(entity_types::ITEM_FRAME).is_empty());
}

/// Brief 6.0 item 5 and 6.5's `survives`: a frame does not coexist with another
/// hanging entity of the same direction overlapping it, so a second frame on the same
/// face is not hung.
#[test]
fn a_second_frame_on_the_same_face_is_not_hung() {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    scene.click(CUBE, Face::North);
    scene.click(CUBE, Face::North);
    scene.select(3);
    scene.click(CUBE, Face::North);
    assert_eq!(scene.of_kind(entity_types::ITEM_FRAME).len(), 1);
    assert!(
        scene.of_kind(entity_types::GLOW_ITEM_FRAME).is_empty(),
        "a glow frame is a hanging entity of the same direction too"
    );
}

/// Brief 6.5's `survives`: no block's collision may meet the frame's 0.75 box, so a
/// frame is not hung into a place a slab fills the bottom of.
#[test]
fn a_frame_is_not_hung_where_a_block_fills_the_place_in_front_of_the_wall() {
    let wall = bp(8, -60, 10);
    let mut scene = Scene::alone(
        &[
            (wall, blocks::STONE),
            (
                bp(8, -60, 9),
                state("minecraft:stone_slab[type=bottom,waterlogged=false]"),
            ),
        ],
        &[],
        frames_hotbar(),
        BY_THE_CUBE,
    );
    scene.click(wall, Face::North);
    assert!(scene.of_kind(entity_types::ITEM_FRAME).is_empty());
}

/// Brief 6.5, `interact`: an empty frame takes one of what the player holds, which a
/// creative player keeps; a frame with an item turns it by one at each click, whatever
/// the player holds, wrapping from 7 to 0, and the held item is not used.
#[test]
fn a_frame_takes_one_of_the_held_item_and_each_click_after_turns_it_round_eight_places() {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    scene.click(CUBE, Face::North);
    let key = scene.one(entity_types::ITEM_FRAME).key;
    scene.select(1);
    scene.interact(key, [0.0, 0.0, 0.0], false);
    let frame = scene.record(key).expect("the frame hangs");
    assert_eq!(
        frame_of(&frame).1,
        Some(stack(items::STONE, 1)),
        "one stone in the frame"
    );
    assert_eq!(frame_of(&frame).2, 0);
    assert_eq!(
        scene.holds(1),
        Some(stack(items::STONE, 64)),
        "the creative player keeps all"
    );
    scene.select(2);
    for turned in 1..=9u8 {
        scene.interact(key, [0.0, 0.0, 0.0], false);
        let frame = scene.record(key).expect("the frame hangs");
        assert_eq!(
            frame_of(&frame).2,
            turned % 8,
            "click {turned} turns it once more"
        );
        assert_eq!(
            frame_of(&frame).1,
            Some(stack(items::STONE, 1)),
            "the item stays the stone, whatever the player holds"
        );
    }
    assert_eq!(scene.holds(2), Some(stack(items::DIRT, 64)));
}

/// Brief 6.0 item 7, 6.5's damage and drops: a frame with an item takes two hits, the
/// first taking the item out (deleting it for a creative player) and the second breaking
/// the frame; a creative breaker gets nothing of either.
#[test]
fn a_creative_players_first_hit_takes_the_frames_item_away_and_the_second_breaks_it_dropping_nothing()
 {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    scene.click(CUBE, Face::North);
    let key = scene.one(entity_types::ITEM_FRAME).key;
    scene.select(1);
    scene.interact(key, [0.0, 0.0, 0.0], false);
    scene.interact(key, [0.0, 0.0, 0.0], false);
    scene.attack(key);
    let frame = scene.record(key).expect("the first hit leaves the frame");
    assert_eq!(frame_of(&frame).1, None, "the first hit takes the item out");
    assert_eq!(
        frame_of(&frame).2,
        1,
        "the frame keeps its rotation (brief 6.0 item 7)"
    );
    scene.idle_for(3);
    assert!(
        items_lying(&scene.all()).is_empty(),
        "a creative hit drops nothing"
    );
    scene.attack(key);
    assert!(
        scene.record(key).is_none(),
        "the second hit breaks the frame"
    );
    scene.idle_for(3);
    assert!(
        items_lying(&scene.all()).is_empty(),
        "a creative break drops nothing"
    );
}

/// Brief 6.6: a comparator reads, through the conductor in front of it, the one frame
/// of its facing hanging on the far side, as the frame's rotation plus one, and 0 for an
/// empty frame; putting the item in and every turn tell the comparator.
#[test]
fn a_comparator_behind_the_wall_reads_the_frames_rotation_plus_one() {
    // The frame hangs on the north face of the wall at (8, -59, 10), facing north; the
    // comparator at (8, -59, 11) faces north too, reading through the wall, and its
    // output lights the wire south of it.
    let wire = bp(8, -59, 12);
    let mut scene = Scene::alone(
        &[
            (CUBE, blocks::STONE),
            (bp(8, -60, 11), blocks::STONE),
            (bp(8, -60, 12), blocks::STONE),
            (
                bp(8, -59, 11),
                state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
            ),
            (
                wire,
                state("minecraft:redstone_wire[east=none,north=side,power=0,south=none,west=none]"),
            ),
        ],
        &[],
        frames_hotbar(),
        BY_THE_CUBE,
    );
    scene.click(CUBE, Face::North);
    let key = scene.one(entity_types::ITEM_FRAME).key;
    scene.idle_for(6);
    assert_eq!(
        power_at(&scene.region, wire),
        0,
        "an empty frame gives nothing"
    );
    scene.select(1);
    scene.interact(key, [0.0, 0.0, 0.0], false);
    for expected in [1u8, 2, 3, 4, 5, 6, 7, 8, 1] {
        let read = idle_until(&mut scene, 10, |scene| {
            power_at(&scene.region, wire) == expected
        });
        assert!(
            read,
            "the wire shows {expected} for rotation {}; it shows {}",
            expected - 1,
            power_at(&scene.region, wire)
        );
        scene.interact(key, [0.0, 0.0, 0.0], false);
    }
}

/// Brief 6.0 item 3 and 6.1's `tick`: survival is checked once every 101 ticks, the
/// first in the entity's 101st tick; a frame whose wall is gone hangs on until then, and
/// is then dropped with no breaker: the frame first, then its item.
#[test]
fn a_frame_whose_wall_is_broken_hangs_until_its_hundred_and_first_tick_and_then_drops_with_its_item()
 {
    let mut scene = Scene::alone(&[(CUBE, blocks::STONE)], &[], frames_hotbar(), BY_THE_CUBE);
    let placed = scene.click(CUBE, Face::North).tick;
    let key = scene.one(entity_types::ITEM_FRAME).key;
    scene.select(1);
    scene.interact(key, [0.0, 0.0, 0.0], false);
    scene.sequence += 1;
    let sequence = scene.sequence;
    scene.input(PlayerInput::Dig {
        position: CUBE,
        sequence,
    });
    assert!(
        scene
            .region
            .chunk(ChunkPos::new(0, 0))
            .expect("loaded")
            .get(8, -59, 10)
            .is_some_and(BlockState::is_air),
        "the wall is broken"
    );
    // The frame's first tick is the tick that placed it (the game ticks an entity a
    // packet added in the next server tick, before anything else of the player's), so
    // its 101st is 100 ticks later.
    while scene.region.tick_number() < placed + 99 {
        scene.idle();
    }
    assert!(
        scene.record(key).is_some(),
        "the frame hangs through its 100th tick"
    );
    scene.idle();
    assert!(
        scene.record(key).is_none(),
        "the frame goes at its 101st tick"
    );
    let mut dropped: Vec<Entity> = scene
        .all()
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == entity_types::ITEM)
        .collect();
    dropped.sort_by_key(|entity| entity.key);
    assert_eq!(
        items_lying(&dropped),
        [stack(items::ITEM_FRAME, 1), stack(items::STONE, 1)],
        "the frame drops first and its item after"
    );
}

// ------------------------------------------------------------------------------------
// Paintings (the brief, sections 0.34, 6.0 and 6.4).

/// The variants whose area is the largest of the placeable ones, 4x4: burning_skull,
/// orb, pigscene, pointer and unpacked, by their ids in the registry.
const FOUR_BY_FOUR: [u16; 5] = [7, 28, 31, 33, 44];

/// The placeable variants one block wide and two high: graham, prairie_ride, wanderer.
const ONE_BY_TWO: [u16; 3] = [22, 36, 46];

fn painting_of(entity: &Entity) -> (Attached, u16) {
    match &entity.data {
        EntityData::Painting { attached, variant } => (*attached, *variant),
        other => panic!("not a painting: {other:?}"),
    }
}

fn paintings_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    hotbar(&[(0, stack(items::PAINTING, 1))])
}

/// A wall of stone at z = 12, from x = 2 to 13 and y = -60 to -53.
fn wide_wall() -> Blocks {
    let mut wall = Vec::new();
    for x in 2..=13 {
        for y in -60..=-53 {
            wall.push((bp(x, y, 12), blocks::STONE));
        }
    }
    wall
}

/// Brief 6.4, `Painting.create`: of the placeable variants that survive, only those of
/// the largest area are drawn from; on a wall with room for any, that is a 4x4, which
/// for an even size reaches from the clicked block to its counter-clockwise side and
/// upwards, its position the centre of its box.
#[test]
fn a_painting_on_a_wide_wall_is_one_of_the_largest_placeable_variants() {
    let mut scene = Scene::alone(
        &wide_wall(),
        &[],
        paintings_hotbar(),
        Vec3::new(8.5, -60.0, 8.5),
    );
    scene.click(bp(8, -57, 12), Face::North);
    let painting = scene.one(entity_types::PAINTING);
    let (attached, variant) = painting_of(&painting);
    assert_eq!(
        attached,
        Attached {
            block: bp(8, -57, 11),
            direction: NORTH
        }
    );
    assert!(
        FOUR_BY_FOUR.contains(&variant),
        "variant {variant} is not of the largest area"
    );
    for (axis, expected) in [8.0, -56.0, 11.968_75].into_iter().enumerate() {
        assert!(
            close(painting.position[axis], expected),
            "a 4x4 facing north is centred at (8, -56, 11.96875), not {:?}",
            painting.position
        );
    }
    assert_eq!(scene.holds(0), Some(stack(items::PAINTING, 1)));
}

/// Brief 6.4: every survival test is a full one with the variant's box, so on a column
/// one block wide and two high only the 1x2 variants and the 1x1 ones fit, and the 1x2
/// ones have the larger area.
#[test]
fn a_painting_on_a_column_one_block_wide_and_two_high_is_a_tall_one_block_variant() {
    let column = vec![
        (bp(8, -60, 12), blocks::STONE),
        (bp(8, -59, 12), blocks::STONE),
    ];
    let mut scene = Scene::alone(&column, &[], paintings_hotbar(), Vec3::new(8.5, -60.0, 8.5));
    scene.click(bp(8, -60, 12), Face::North);
    let painting = scene.one(entity_types::PAINTING);
    let (attached, variant) = painting_of(&painting);
    assert_eq!(attached.block, bp(8, -60, 11));
    assert!(
        ONE_BY_TWO.contains(&variant),
        "variant {variant} is not a 1x2"
    );
    for (axis, expected) in [8.5, -59.0, 11.968_75].into_iter().enumerate() {
        assert!(
            close(painting.position[axis], expected),
            "a 1x2 facing north is centred at (8.5, -59, 11.96875), not {:?}",
            painting.position
        );
    }
}

/// Brief 6.3 and 6.4: where no placeable variant fits nothing is placed; a painting
/// does not go on a floor either, which only the four side faces take.
#[test]
fn nothing_is_placed_where_no_painting_fits_nor_on_a_floor() {
    let wall = bp(8, -60, 12);
    let mut scene = Scene::alone(
        &[
            (wall, blocks::STONE),
            (
                bp(8, -60, 11),
                state("minecraft:stone_slab[type=bottom,waterlogged=false]"),
            ),
        ],
        &[],
        paintings_hotbar(),
        Vec3::new(8.5, -60.0, 8.5),
    );
    scene.click(wall, Face::North);
    scene.click(bp(9, -61, 9), Face::Top);
    assert!(scene.of_kind(entity_types::PAINTING).is_empty());
}

/// Brief 6.1 and 6.4's `dropItem`: one hit of anything breaks a painting, and a
/// creative breaker gets no item.
#[test]
fn a_creative_hit_breaks_a_painting_and_drops_nothing() {
    let mut scene = Scene::alone(
        &wide_wall(),
        &[],
        paintings_hotbar(),
        Vec3::new(8.5, -60.0, 8.5),
    );
    scene.click(bp(8, -57, 12), Face::North);
    let key = scene.one(entity_types::PAINTING).key;
    scene.attack(key);
    assert!(scene.record(key).is_none(), "one hit breaks it");
    scene.idle_for(3);
    assert!(items_lying(&scene.all()).is_empty());
}

// ------------------------------------------------------------------------------------
// Armour stands (the brief, sections 0.25, 5.1, 5.3 and 5.4).

fn stands_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    hotbar(&[
        (0, stack(items::ARMOR_STAND, 1)),
        (1, stack(items::IRON_HELMET, 1)),
        (2, stack(items::IRON_CHESTPLATE, 1)),
        (3, stack(items::IRON_LEGGINGS, 1)),
        (4, stack(items::IRON_BOOTS, 1)),
        (5, stack(items::STONE, 64)),
    ])
}

const BY_THE_STAND: Vec3 = Vec3::new(8.5, -60.0, 6.5);

/// Places a stand on the floor at (8, -60, 9) with player 1 looking along `yaw`.
fn placed_stand(entities: &[Entity], blocks: &[(BlockPos, BlockState)], yaw: f32) -> Scene {
    let mut scene = Scene::alone(blocks, entities, stands_hotbar(), BY_THE_STAND);
    scene.input(PlayerInput::Move {
        position: None,
        rotation: Some((yaw, 0.0)),
        on_ground: true,
    });
    scene.click(bp(8, -61, 9), Face::Top);
    scene
}

/// Brief 5.1: a stand clicked onto a floor's top stands in the block above, at its
/// bottom centre, on the floor, turned to face the player and snapped to 45 degrees:
/// a player looking along yaw 30 gets a stand at -135.
#[test]
fn an_armour_stand_stands_on_the_floor_facing_the_player_snapped_to_forty_five_degrees() {
    let scene = placed_stand(&[], &[], 30.0);
    let stand = scene.one(entity_types::ARMOR_STAND);
    for (axis, expected) in [8.5, -60.0, 9.5].into_iter().enumerate() {
        assert!(
            close(stand.position[axis], expected),
            "the stand is at {:?}",
            stand.position
        );
    }
    assert_eq!(
        stand.yaw, -135.0,
        "Mth.floor((wrap(30 - 180) + 22.5) / 45) * 45"
    );
    assert!(equipment_of(&stand).is_empty());
    assert_eq!(scene.holds(0), Some(stack(items::ARMOR_STAND, 1)));
}

/// Brief 5.1, `getYOffset`: the stand lands on whatever collides in the block or the
/// one below: clicked onto a bottom slab, which cannot be replaced, it stands in the
/// block above the slab, half a block down, on the slab's top.
#[test]
fn an_armour_stand_placed_on_a_bottom_slab_stands_on_the_slabs_top() {
    let slab = bp(8, -60, 9);
    let mut scene = Scene::alone(
        &[(
            slab,
            state("minecraft:stone_slab[type=bottom,waterlogged=false]"),
        )],
        &[],
        stands_hotbar(),
        BY_THE_STAND,
    );
    scene.click(slab, Face::Top);
    let stand = scene.one(entity_types::ARMOR_STAND);
    assert!(
        close(stand.position[1], -59.5),
        "the stand is at {:?}",
        stand.position
    );
}

/// Brief 0.25 and 5.1 step 4: any non-spectator entity whose box meets the stand's,
/// an item lying there among them, stops it being placed; so does the bottom face.
#[test]
fn no_armour_stand_is_placed_where_an_item_lies_nor_under_a_block() {
    let scene = placed_stand(&[lying_item(1, [8.5, -60.0, 9.5])], &[], 0.0);
    assert!(
        scene.of_kind(entity_types::ARMOR_STAND).is_empty(),
        "the item is in the way"
    );

    let ceiling = bp(8, -58, 9);
    let mut scene = Scene::alone(
        &[(ceiling, blocks::STONE)],
        &[],
        stands_hotbar(),
        BY_THE_STAND,
    );
    scene.click(ceiling, Face::Bottom);
    assert!(
        scene.of_kind(entity_types::ARMOR_STAND).is_empty(),
        "never on a bottom face"
    );
}

/// Brief 5.1 step 4: a boat, which is collided with, is in a stand's way as well.
#[test]
fn no_armour_stand_is_placed_where_a_boat_lies() {
    let scene = placed_stand(&[boat(1, [8.5, -60.0, 9.5])], &[], 0.0);
    assert!(scene.of_kind(entity_types::ARMOR_STAND).is_empty());
}

/// Brief 5.3: an item in hand goes to its own equipment slot whatever the height of the
/// click, and a creative player keeps their stack (`swapItem` step 4).
#[test]
fn a_creative_player_dresses_a_stand_by_each_items_own_slot_and_keeps_every_item() {
    let mut scene = placed_stand(&[], &[], 0.0);
    let key = scene.one(entity_types::ARMOR_STAND).key;
    for slot in 1..=4u8 {
        scene.select(slot);
        // Every click at the stand's feet: the slot is the item's, not the height's.
        scene.interact(key, [0.0, 0.2, 0.0], false);
    }
    let mut worn = equipment_of(&scene.record(key).expect("the stand is there"));
    worn.sort_by_key(|(slot, _)| *slot);
    assert_eq!(
        worn,
        [
            (FEET, stack(items::IRON_BOOTS, 1)),
            (LEGS, stack(items::IRON_LEGGINGS, 1)),
            (CHEST, stack(items::IRON_CHESTPLATE, 1)),
            (HEAD, stack(items::IRON_HELMET, 1)),
        ]
    );
    for slot in 1..=4 {
        assert!(
            scene.holds(slot).is_some(),
            "the creative player keeps slot {slot}"
        );
    }
}

/// Brief 5.3, `getClickedSlot`: an empty hand takes from the first slot, in the order
/// feet, chest, legs, head, whose height band the click is in and which holds an item:
/// 1.7 is the head's, 1.0 the chest's, 0.5 the feet's and, once the feet are bare, the
/// legs'. Each comes into the hand, the stand's slot taking the empty hand.
#[test]
fn an_empty_hand_takes_from_the_stand_the_item_of_the_slot_at_the_clicks_height() {
    let worn = vec![
        (FEET, stack(items::IRON_BOOTS, 1)),
        (LEGS, stack(items::IRON_LEGGINGS, 1)),
        (CHEST, stack(items::IRON_CHESTPLATE, 1)),
        (HEAD, stack(items::IRON_HELMET, 1)),
    ];
    let mut scene = Scene::alone(
        &[],
        &[stand(1, [8.5, -60.0, 9.5], worn)],
        hotbar(&[]),
        BY_THE_STAND,
    );
    let key = key(1);
    for (hand, height, taken) in [
        (0u8, 1.7, items::IRON_HELMET),
        (1, 1.0, items::IRON_CHESTPLATE),
        (2, 0.5, items::IRON_BOOTS),
        (3, 0.5, items::IRON_LEGGINGS),
    ] {
        scene.select(hand);
        scene.interact(key, [0.0, height, 0.0], false);
        assert_eq!(
            scene.holds(usize::from(hand)),
            Some(stack(taken, 1)),
            "a click at {height} takes item {taken}"
        );
        let left = equipment_of(&scene.record(key).expect("there"));
        assert!(
            left.iter().all(|(_, worn)| worn.item != taken),
            "the stand no longer wears {taken}: {left:?}"
        );
    }
    assert!(equipment_of(&scene.record(key).expect("there")).is_empty());
}

/// Brief 5.3: an item with no slot of its own is for the main hand, which a stand
/// without arms refuses.
#[test]
fn a_stand_without_arms_refuses_an_item_for_the_hand() {
    let mut scene = placed_stand(&[], &[], 0.0);
    let key = scene.one(entity_types::ARMOR_STAND).key;
    scene.select(5);
    scene.interact(key, [0.0, 1.0, 0.0], false);
    let worn = equipment_of(&scene.record(key).expect("there"));
    assert!(
        worn.iter().all(|(slot, _)| *slot != MAINHAND) && worn.is_empty(),
        "nothing is put on: {worn:?}"
    );
}

/// Brief 0.25 and 5.4 rule 10: a creative player's hit breaks a stand at once, and
/// nothing drops, neither the stand nor what it wears.
#[test]
fn a_creative_hit_breaks_a_dressed_stand_at_once_and_nothing_drops() {
    let worn = vec![
        (HEAD, stack(items::IRON_HELMET, 1)),
        (CHEST, stack(items::IRON_CHESTPLATE, 1)),
    ];
    let mut scene = Scene::alone(
        &[],
        &[stand(1, [8.5, -60.0, 9.5], worn)],
        hotbar(&[]),
        BY_THE_STAND,
    );
    scene.attack(key(1));
    assert!(scene.record(key(1)).is_none(), "one creative hit breaks it");
    scene.idle_for(3);
    assert!(items_lying(&scene.all()).is_empty(), "nothing drops");
}

// ------------------------------------------------------------------------------------
// Leashes (the brief, section 7).

const BOAT_AT: [f64; 3] = [8.5, -60.0, 9.5];
const FENCE: BlockPos = BlockPos::new(10, -60, 9);

fn leash_hotbar() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    hotbar(&[(0, stack(items::LEAD, 1)), (1, stack(items::STONE, 64))])
}

/// A boat beside player 1, and a fence beside it; the boat tied to player 1 by the lead
/// in their hand, with the tick that tied it.
fn tied_boat() -> (Scene, TickOutput) {
    let mut scene = Scene::alone(
        &[(FENCE, blocks::OAK_FENCE)],
        &[boat(1, BOAT_AT)],
        leash_hotbar(),
        BY_THE_STAND,
    );
    let tying = scene.interact(key(1), [0.0, 0.3, 0.0], false);
    (scene, tying)
}

/// Brief 7.4 step 5 and 7.5: a lead used on a boat ties it to the player and is
/// reported (`set_entity_link`); the click is the lead's, so the player does not board,
/// and a creative player keeps the lead (`Player.interactOn`).
#[test]
fn a_lead_used_on_a_boat_ties_it_to_the_player_who_keeps_the_lead_and_does_not_board() {
    let (scene, tying) = tied_boat();
    let boat = scene.record(key(1)).expect("the boat is there");
    assert_eq!(leash_of(&boat), Some(LeashHolder::Entity(scene.me)));
    assert_eq!(leashed_events(&tying, key(1)), [Some(scene.me)]);
    assert_eq!(scene.holds(0), Some(stack(items::LEAD, 1)));
    let me = scene.region.player_state(player(1)).expect("there");
    assert!(me.riding.is_none(), "the player did not board");
    assert!(boat.riding.passengers.is_empty());
}

/// Brief 7.2: within `leashElasticDistance` less both widths (4.025 for a boat held by
/// a player) a leash does nothing; beyond it, and with the attachment points more than
/// 6 apart, the spring pulls the boat towards its holder.
#[test]
fn a_boat_within_the_leads_slack_stays_and_one_beyond_it_is_pulled_towards_its_holder() {
    let (mut scene, _) = tied_boat();
    scene.idle_for(5);
    let resting = scene.record(key(1)).expect("there");
    assert!(
        close(resting.position[2], BOAT_AT[2]) && close(resting.position[0], BOAT_AT[0]),
        "three blocks off the boat is not pulled: {:?}",
        resting.position
    );
    scene.walk_to(Vec3::new(8.5, -60.0, 17.5));
    scene.idle();
    let pulled = scene.record(key(1)).expect("there");
    assert!(
        pulled.position[2] > BOAT_AT[2] + 0.01,
        "eight blocks off the boat is pulled south: {:?}",
        pulled.position
    );
    assert!(
        close(pulled.position[0], BOAT_AT[0]),
        "and only south: {:?}",
        pulled.position
    );
    assert_eq!(leash_of(&pulled), Some(LeashHolder::Entity(scene.me)));
}

/// Brief 7.0 item 3 and 7.2: beyond 12 blocks between box centres the lead snaps, a
/// lead is dropped at the boat whatever the game mode, and the link's end is reported.
#[test]
fn a_lead_beyond_twelve_blocks_snaps_and_drops_a_lead_at_the_boat() {
    let (mut scene, _) = tied_boat();
    let snapping = scene.walk_to(Vec3::new(8.5, -60.0, 23.5));
    let mut said = leashed_events(&snapping, key(1));
    if said.is_empty() {
        said = leashed_events(&scene.idle(), key(1));
    }
    assert_eq!(said, [None], "the end of the link is reported");
    let boat = scene.record(key(1)).expect("the boat stays");
    assert_eq!(leash_of(&boat), None);
    let leads: Vec<Entity> = scene
        .all()
        .into_iter()
        .filter(|entity| {
            matches!(&entity.data, EntityData::Item { stack, .. } if stack.item == items::LEAD)
        })
        .collect();
    assert_eq!(leads.len(), 1, "one lead dropped: {leads:?}");
    assert!(
        (leads[0].position[2] - boat.position[2]).abs() < 1.0,
        "the lead drops at the boat, not at the player: {:?}",
        leads[0].position
    );
}

/// The knot at the fence, if there is one, with its key.
fn knot(scene: &Scene) -> Option<Entity> {
    let knots = scene.of_kind(entity_types::LEASH_KNOT);
    assert!(knots.len() <= 1, "one knot at most: {knots:?}");
    knots.into_iter().next()
}

fn held_by_the_knot(boat: &Entity, knot: &Entity) -> bool {
    matches!(
        leash_of(boat),
        Some(LeashHolder::Fence(at)) if at == FENCE
    ) || leash_of(boat) == Some(LeashHolder::Entity(knot.key))
}

/// Brief 7.6, `LeadItem.useOn` and `FenceBlock.useWithoutItem`: using a fence with a
/// lead in hand moves the player's boats onto a knot made there, at the fence's centre
/// and 0.375 up, and the lead in hand is kept.
#[test]
fn using_a_fence_with_a_lead_in_hand_ties_the_players_boat_to_a_knot_at_the_fence() {
    let (mut scene, _) = tied_boat();
    let tying = scene.click(FENCE, Face::Top);
    let knot = knot(&scene).expect("a knot at the fence");
    for (axis, expected) in [10.5, -59.625, 9.5].into_iter().enumerate() {
        assert!(
            close(knot.position[axis], expected),
            "the knot is at {:?}",
            knot.position
        );
    }
    let boat = scene.record(key(1)).expect("there");
    assert!(
        held_by_the_knot(&boat, &knot),
        "the boat is the knot's: {:?}",
        boat.riding
    );
    assert_eq!(leashed_events(&tying, key(1)), [Some(knot.key)]);
    assert_eq!(scene.holds(0), Some(stack(items::LEAD, 1)));
}

/// Brief 7.6: a fence binds the player's leashed boats with any item in hand.
#[test]
fn using_a_fence_with_any_item_in_hand_ties_the_players_boat_to_it() {
    let (mut scene, _) = tied_boat();
    scene.select(1);
    scene.click(FENCE, Face::Top);
    let knot = knot(&scene).expect("a knot at the fence");
    let boat = scene.record(key(1)).expect("there");
    assert!(
        held_by_the_knot(&boat, &knot),
        "the boat is the knot's: {:?}",
        boat.riding
    );
}

/// Brief 7.6, the knot's `interact` step 3 and `notifyLeasheeRemoved`: a player who
/// holds nothing on a lead and uses the knot without sneaking takes its boats back, and
/// a knot that holds nothing then goes, silently and dropping nothing.
#[test]
fn a_knot_whose_last_boat_the_player_takes_back_goes_and_drops_nothing() {
    let (mut scene, _) = tied_boat();
    scene.click(FENCE, Face::Top);
    let knot = knot(&scene).expect("a knot at the fence");
    scene.select(2);
    scene.interact(knot.key, [0.0, 0.25, 0.0], false);
    let boat = scene.record(key(1)).expect("there");
    assert_eq!(leash_of(&boat), Some(LeashHolder::Entity(scene.me)));
    assert!(
        self::knot(&scene).is_none(),
        "the knot holds nothing and goes"
    );
    scene.idle_for(3);
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 7.0 item 3, 7.2 and 7.6: a knot broken by a creative player's hit drops no
/// item of its own, but each of its leashees drops its lead on its next tick, even
/// then, and the link's end is reported.
#[test]
fn a_broken_knot_lets_its_boat_go_with_a_lead_dropped_even_for_a_creative_breaker() {
    let (mut scene, _) = tied_boat();
    scene.click(FENCE, Face::Top);
    let knot = knot(&scene).expect("a knot at the fence");
    let mut outputs = vec![scene.attack(knot.key)];
    assert!(self::knot(&scene).is_none(), "one hit breaks a knot");
    outputs.extend(scene.idle_for(2));
    let said: Vec<Option<EntityId>> = outputs
        .iter()
        .flat_map(|output| leashed_events(output, key(1)))
        .collect();
    assert_eq!(said, [None]);
    let boat = scene.record(key(1)).expect("there");
    assert_eq!(leash_of(&boat), None);
    assert_eq!(items_lying(&scene.all()), [stack(items::LEAD, 1)]);
}

// ------------------------------------------------------------------------------------
// Uses across a pinned line (the plan, decision 10, section 7 item 9 and section 8
// item 7): the line is at block x = 16; player 1 stands in the west at x = 14.5, the
// entity is the east region's, which the west sees in its copy of chunk (1, 0).

const BY_THE_LINE: Vec3 = Vec3::new(14.5, -60.0, 2.5);
const ACROSS: BlockPos = BlockPos::new(16, -59, 2);

/// The wall east of the line that a frame hangs on, facing the west.
fn wall_across() -> Blocks {
    vec![(bp(17, -59, 2), blocks::STONE)]
}

/// The west, with player 1 in it, and the east, holding `entities`.
fn two_sides(entities: &[Entity], hotbar: [Option<ItemStack>; HOTBAR_SLOTS]) -> (Scene, Region) {
    let blocks = wall_across();
    let west = Scene::new(
        Layout::side(true),
        &blocks,
        entities,
        hotbar.clone(),
        BY_THE_LINE,
    );
    let east = region_of(&Layout::side(false), &blocks, entities, hotbar);
    (west, east)
}

/// The effects on entities in `output`.
fn on_entities(output: &TickOutput) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| matches!(effect.what, Effected::OnEntity { .. }))
        .cloned()
        .collect()
}

/// Hands `effects` to the holder as the store files them, and lets it tick twice: they
/// are done in the next tick's seams.
fn filed(holder: &mut Region, effects: Vec<Effect>) {
    let effects = effects
        .into_iter()
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence: 60,
                index: u32::try_from(index).expect("few"),
            },
            effect,
            due: Due::FromTaking,
        })
        .collect();
    holder.tick(&TickInputs {
        effects,
        ..TickInputs::default()
    });
    holder.tick(&TickInputs::default());
}

fn east_record(east: &Region, key: EntityId) -> Option<Entity> {
    all_of(east).into_iter().find(|entity| entity.key == key)
}

/// Decision 10 and section 7 item 9: putting an item in a frame across a line is an
/// effect on the frame, carrying what the player holds, filed for its chunk; the
/// holder does it, and the player's own region changes nothing of its copy.
#[test]
fn an_item_put_in_a_frame_across_a_pinned_line_goes_as_an_effect_and_the_holder_puts_it_in() {
    let frame = frame_facing_west(1, ACROSS, None);
    let (mut west, mut east) = two_sides(
        std::slice::from_ref(&frame),
        hotbar(&[(0, stack(items::STONE, 64))]),
    );
    assert!(
        east_record(&east, frame.key).is_some(),
        "the east has the frame"
    );
    let using = west.interact(frame.key, [0.0, 0.0, 0.0], false);
    let effects = on_entities(&using);
    assert_eq!(
        effects.len(),
        1,
        "one effect on the frame: {:?}",
        using.effects
    );
    assert_eq!(
        effects[0].at.chunk(),
        ChunkPos::new(1, 0),
        "filed for the frame's chunk"
    );
    match effects[0].what.clone() {
        Effected::OnEntity {
            key,
            by: EntityUse::Interact {
                ref held, sneaking, ..
            },
        } => {
            assert_eq!(key, frame.key);
            assert_eq!(*held, Some(stack(items::STONE, 64)));
            assert!(!sneaking);
        }
        other => panic!("not a use of the frame: {other:?}"),
    }
    filed(&mut east, effects);
    let there = east_record(&east, frame.key).expect("the frame hangs");
    assert_eq!(frame_of(&there).1, Some(stack(items::STONE, 1)));
    assert_eq!(
        west.holds(0),
        Some(stack(items::STONE, 64)),
        "the creative player keeps it"
    );
}

/// Decision 10: a hit on a frame across a line is an effect too, and takes its item out.
#[test]
fn a_hit_on_a_frame_across_a_pinned_line_takes_its_item_out_there() {
    let frame = frame_facing_west(1, ACROSS, Some(stack(items::STONE, 1)));
    let (mut west, mut east) = two_sides(std::slice::from_ref(&frame), hotbar(&[]));
    let hitting = west.attack(frame.key);
    let effects = on_entities(&hitting);
    assert_eq!(
        effects.len(),
        1,
        "one effect on the frame: {:?}",
        hitting.effects
    );
    assert!(matches!(
        effects[0].what,
        Effected::OnEntity {
            by: EntityUse::Attack { creative: true, .. },
            ..
        }
    ));
    filed(&mut east, effects);
    let there = east_record(&east, frame.key).expect("the first hit leaves the frame");
    assert_eq!(frame_of(&there).1, None);
}

/// Section 7 item 9 and section 8 item 7: taking from a stand across a line does
/// nothing in N5, as it would have to give the player an item; whatever the west sends,
/// the stand keeps what it wears and the hand stays empty.
#[test]
fn an_empty_hand_on_a_stand_across_a_pinned_line_takes_nothing() {
    let worn = vec![(HEAD, stack(items::IRON_HELMET, 1))];
    let across = stand(1, [17.5, -60.0, 2.5], worn.clone());
    let (mut west, mut east) = two_sides(std::slice::from_ref(&across), hotbar(&[]));
    let using = west.interact(across.key, [0.0, 1.7, 0.0], false);
    filed(&mut east, on_entities(&using));
    west.idle();
    let there = east_record(&east, across.key).expect("the stand is there");
    assert_eq!(equipment_of(&there), worn, "the stand keeps its helmet");
    assert_eq!(west.holds(0), None, "the player is given nothing");
}

/// Section 8 item 7: in creative, putting an item on a stand across a line needs no
/// answer, and is an effect that dresses the stand there; the player keeps the item.
#[test]
fn a_helmet_put_on_a_stand_across_a_pinned_line_goes_as_an_effect_and_is_worn_there() {
    let across = stand(1, [17.5, -60.0, 2.5], Vec::new());
    let (mut west, mut east) = two_sides(
        std::slice::from_ref(&across),
        hotbar(&[(0, stack(items::IRON_HELMET, 1))]),
    );
    let using = west.interact(across.key, [0.0, 0.5, 0.0], false);
    let effects = on_entities(&using);
    assert_eq!(
        effects.len(),
        1,
        "one effect on the stand: {:?}",
        using.effects
    );
    filed(&mut east, effects);
    let there = east_record(&east, across.key).expect("the stand is there");
    assert_eq!(equipment_of(&there), [(HEAD, stack(items::IRON_HELMET, 1))]);
    assert_eq!(west.holds(0), Some(stack(items::IRON_HELMET, 1)));
}

/// Decision 10: a creative player's hit on a stand across a line breaks it there, and
/// nothing drops.
#[test]
fn a_creative_hit_on_a_stand_across_a_pinned_line_breaks_it_there() {
    let across = stand(
        1,
        [17.5, -60.0, 2.5],
        vec![(HEAD, stack(items::IRON_HELMET, 1))],
    );
    let (mut west, mut east) = two_sides(std::slice::from_ref(&across), hotbar(&[]));
    let hitting = west.attack(across.key);
    let effects = on_entities(&hitting);
    assert_eq!(
        effects.len(),
        1,
        "one effect on the stand: {:?}",
        hitting.effects
    );
    filed(&mut east, effects);
    assert!(
        east_record(&east, across.key).is_none(),
        "the stand is broken"
    );
    assert!(items_lying(&all_of(&east)).is_empty(), "and nothing drops");
}

// ------------------------------------------------------------------------------------
// More of the same rules.

/// Brief 6.0 item 9: a painting is filed by its position, the centre of its box, which
/// for an even width is half a block to one side of its block: a 2x1 facing south at
/// x = 15 is centred at x = 16 and belongs to chunk (1, 0), while its block is in
/// chunk (0, 0).
#[test]
fn a_two_wide_painting_whose_centre_is_over_a_chunk_border_is_kept_in_the_chunk_of_its_centre() {
    let wall = vec![
        (bp(15, -59, 4), blocks::STONE),
        (bp(16, -59, 4), blocks::STONE),
    ];
    let mut scene = Scene::alone(&wall, &[], paintings_hotbar(), Vec3::new(14.5, -60.0, 7.5));
    scene.click(bp(15, -59, 4), Face::South);
    let painting = scene.one(entity_types::PAINTING);
    let (attached, variant) = painting_of(&painting);
    assert_eq!(
        attached,
        Attached {
            block: bp(15, -59, 5),
            direction: SOUTH
        }
    );
    assert!(
        [12, 13, 35, 37, 42].contains(&variant),
        "variant {variant} is not a 2x1"
    );
    assert!(
        close(painting.position[0], 16.0),
        "centred at x = 16: {:?}",
        painting.position
    );
    let kept_in = scene
        .region
        .chunk(ChunkPos::new(1, 0))
        .expect("loaded")
        .entities()
        .iter()
        .any(|entity| entity.key == painting.key);
    assert!(kept_in, "the painting is a record of chunk (1, 0)");
}

/// Brief 5.3, `swapItem` step 6: a creative player holding one item, clicking a stand
/// whose slot for it is taken, swaps the two: the stand wears the new one and the
/// player holds the old.
#[test]
fn a_creative_player_holding_one_helmet_swaps_it_for_the_one_the_stand_wears() {
    let mut scene = Scene::alone(
        &[],
        &[stand(
            1,
            [8.5, -60.0, 9.5],
            vec![(HEAD, stack(items::IRON_HELMET, 1))],
        )],
        hotbar(&[(0, stack(items::GOLDEN_HELMET, 1))]),
        BY_THE_STAND,
    );
    scene.interact(key(1), [0.0, 0.3, 0.0], false);
    let worn = equipment_of(&scene.record(key(1)).expect("there"));
    assert_eq!(worn, [(HEAD, stack(items::GOLDEN_HELMET, 1))]);
    assert_eq!(scene.holds(0), Some(stack(items::IRON_HELMET, 1)));
}

/// Brief 7.4 step 4: a player who uses a boat they hold lets it go; a creative player's
/// leash is removed with no lead dropped, and the link's end is reported.
#[test]
fn a_creative_player_using_the_boat_they_hold_lets_it_go_and_no_lead_drops() {
    let (mut scene, _) = tied_boat();
    scene.select(2);
    let letting_go = scene.interact(key(1), [0.0, 0.3, 0.0], false);
    assert_eq!(leashed_events(&letting_go, key(1)), [None]);
    let boat = scene.record(key(1)).expect("there");
    assert_eq!(leash_of(&boat), None);
    assert!(
        scene
            .region
            .player_state(player(1))
            .expect("there")
            .riding
            .is_none(),
        "letting go is the click's: the player does not board"
    );
    scene.idle_for(3);
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 7.0 item 1 and 7.4 step 1: a sneaking player who uses another boat ties the
/// boats they hold to it: a boat is a holder.
#[test]
fn a_sneaking_player_using_another_boat_ties_the_boat_they_hold_to_it() {
    let mut scene = Scene::alone(
        &[],
        &[boat(1, BOAT_AT), boat(2, [11.5, -60.0, 9.5])],
        leash_hotbar(),
        BY_THE_STAND,
    );
    scene.interact(key(1), [0.0, 0.3, 0.0], false);
    let tying = scene.interact(key(2), [0.0, 0.3, 0.0], true);
    let held = scene.record(key(1)).expect("there");
    assert_eq!(leash_of(&held), Some(LeashHolder::Entity(key(2))));
    assert_eq!(leashed_events(&tying, key(1)), [Some(key(2))]);
    let holder = scene.record(key(2)).expect("there");
    assert!(
        holder.riding.passengers.is_empty(),
        "the sneaking player does not board"
    );
    assert_eq!(leash_of(&holder), None);
}

/// Brief 7.4 step 5: a lead used on a boat a knot holds takes it to the player, the
/// old leash dropping a lead, and the knot that then holds nothing goes.
#[test]
fn a_lead_used_on_a_boat_tied_to_a_knot_takes_it_and_the_old_leash_drops_a_lead() {
    let (mut scene, _) = tied_boat();
    scene.click(FENCE, Face::Top);
    assert!(knot(&scene).is_some());
    scene.interact(key(1), [0.0, 0.3, 0.0], false);
    let boat = scene.record(key(1)).expect("there");
    assert_eq!(leash_of(&boat), Some(LeashHolder::Entity(scene.me)));
    assert!(knot(&scene).is_none(), "the knot holds nothing and goes");
    assert_eq!(items_lying(&scene.all()), [stack(items::LEAD, 1)]);
}

/// The plan, section 8 item 7: picking an entity's item is answered by the player's
/// own region from its copy of the entity, not as an effect: a frame across a line
/// gives a creative player its item in the hotbar.
#[test]
fn picking_a_frame_across_a_pinned_line_gives_its_item_from_the_copy_with_no_effect() {
    let frame = frame_facing_west(1, ACROSS, Some(stack(items::STONE, 1)));
    let (mut west, _) = two_sides(std::slice::from_ref(&frame), hotbar(&[]));
    let picking = west.input(PlayerInput::PickItemFromEntity { entity: frame.key });
    assert!(
        on_entities(&picking).is_empty(),
        "no effect: {:?}",
        picking.effects
    );
    let hotbar = west
        .region
        .player_state(player(1))
        .expect("there")
        .inventory
        .hotbar();
    assert!(
        hotbar
            .iter()
            .flatten()
            .any(|held| held.item == items::STONE),
        "the frame's stone is in the hotbar: {hotbar:?}"
    );
}
