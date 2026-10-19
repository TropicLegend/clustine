//! Item frames, glow item frames and paintings (`HangingEntity`, `ItemFrame`,
//! `GlowItemFrame`, `Painting`, and the items `HangingEntityItem` and `ItemFrameItem`;
//! `docs/groundwork/n5-brief.md`, section 6 and findings 26, 31 to 35): how they are
//! hung, where they are and how big, what holds them, how a hit and a click change them,
//! what they drop, and what a comparator two blocks away reads of a frame.
//!
//! A hanging entity's position, box and rotations follow from its block and direction
//! (and a painting's variant), as the game's `recalculateBoundingBox` and `setDirection`
//! make them; the region works them out whenever it needs them, and keeps them in the
//! record so that the record's chunk is its position's (finding 35: a painting of even
//! width can stand in the chunk next to its block's).
//!
//! What is left out, named: maps (a frame holding one keeps the 0.75 box, and nothing is
//! tracked of the map), items' components (Clustine's stacks have none, so a painting
//! item never names its variant and a frame item never carries a name or entity data,
//! which is how a frame comes to be fixed or invisible), the world border
//! (`hasLevelCollision` checks block shapes only), the `entityDrops` and `mobGriefing`
//! game rules (taken as their defaults, true), spawn protection (`mayInteract`), game
//! events, and a survival player's use of the item put in a frame: every player is a
//! creative one (`ItemStack.consume` takes nothing from them), and a use made across a
//! line could not take it from the player's region anyway.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, Face, TAGS, blocks, entity_types, items, sounds};
use clustine_world::{
    Attached, BlockPos, ChunkPos, Entity, EntityData, EntityField, EntityId, EntityUse, Float,
    ItemStack, MAX_CHUNK_ENTITIES, PlayerId,
};

use super::Region;
use super::attached::Breaker;
use super::entities::insecure_uuid;
use super::items::View;
use super::random_ticks::Random;
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{self, Context};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

/// How far a hanging entity's centre is from its block's centre, towards the wall
/// (`Vec3.relative(direction, -0.46875)` in `calculateBoundingBox`).
const TOWARDS_THE_WALL: f64 = -0.468_75;

/// How thick a frame and a painting are (`ItemFrame.DEPTH`, and the painting's 0.0625).
const DEPTH: f64 = 0.0625;

/// How wide and high a frame without a map is (`ItemFrame.WIDTH`, `HEIGHT`, 0.75f).
const FRAME_SIZE: f64 = 0.75;

/// The frame's rotations of its item (`ItemFrame.NUM_ROTATIONS`).
const ROTATIONS: u8 = 8;

/// How far out from the wall a hanging entity's drops are laid (`spawnAtLocation`'s
/// `0.15f`, multiplied as a float by the direction's step).
const DROP_OUT: f32 = 0.15;

/// The width and height in blocks of each painting variant, by its id in the
/// `minecraft:painting_variant` registry (alphabetical, as the registry is sent). Read
/// by hand from the jar's data, `generated/data/minecraft/painting_variant/*.json`
/// (`width` and `height`), as `clustine-data` does not yet extract them; the brief's
/// table (section 6.4) agrees.
const PAINTING_SIZES: [(u8, u8); 51] = [
    (1, 1), // alban
    (1, 1), // aztec
    (1, 1), // aztec2
    (3, 4), // backyard
    (2, 2), // baroque
    (1, 1), // bomb
    (3, 3), // bouquet
    (4, 4), // burning_skull
    (2, 2), // bust
    (3, 3), // cavebird
    (4, 2), // changing
    (3, 3), // cotan
    (2, 1), // courbet
    (2, 1), // creebet
    (3, 3), // dennis
    (4, 3), // donkey_kong
    (2, 2), // earth
    (3, 3), // endboss
    (3, 3), // fern
    (4, 2), // fighters
    (4, 2), // finding
    (2, 2), // fire
    (1, 2), // graham
    (2, 2), // humble
    (1, 1), // kebab
    (4, 2), // lowmist
    (2, 2), // match
    (1, 1), // meditative
    (4, 4), // orb
    (3, 3), // owlemons
    (4, 2), // passage
    (4, 4), // pigscene
    (1, 1), // plant
    (4, 4), // pointer
    (3, 4), // pond
    (2, 1), // pool
    (1, 2), // prairie_ride
    (2, 1), // sea
    (4, 3), // skeleton
    (2, 2), // skull_and_roses
    (2, 2), // stage
    (3, 3), // sunflowers
    (2, 1), // sunset
    (3, 3), // tides
    (4, 4), // unpacked
    (2, 2), // void
    (1, 2), // wanderer
    (1, 1), // wasteland
    (2, 2), // water
    (2, 2), // wind
    (2, 2), // wither
];

/// The width and height of the painting variant `variant`, 1 by 1 for an id the
/// registry does not have.
pub(crate) fn painting_size(variant: u16) -> (u8, u8) {
    PAINTING_SIZES
        .get(usize::from(variant))
        .copied()
        .unwrap_or((1, 1))
}

/// The variants of `#minecraft:placeable`, in the tag's order, which the draw indexes.
fn placeable_variants() -> &'static [i32] {
    TAGS.iter()
        .find(|tags| tags.registry == "minecraft:painting_variant")
        .and_then(|tags| {
            tags.tags
                .iter()
                .find(|tag| tag.name == "minecraft:placeable")
        })
        .map_or(&[], |tag| tag.entries)
}

/// The step of the direction whose 3D data value is `direction` (0 down, 1 up, 2 north,
/// 3 south, 4 west, 5 east).
pub(crate) fn step(direction: u8) -> [i32; 3] {
    match direction {
        0 => [0, -1, 0],
        1 => [0, 1, 0],
        2 => [0, 0, -1],
        3 => [0, 0, 1],
        4 => [-1, 0, 0],
        _ => [1, 0, 0],
    }
}

/// The face of the 3D data value `direction`.
fn face_of(direction: u8) -> Face {
    match direction {
        0 => Face::Down,
        1 => Face::Up,
        2 => Face::North,
        3 => Face::South,
        4 => Face::West,
        _ => Face::East,
    }
}

fn is_horizontal(direction: u8) -> bool {
    (2..=5).contains(&direction)
}

/// `Direction.getCounterClockWise()` about the vertical: north to west, west to south,
/// south to east, east to north.
fn counter_clockwise(direction: u8) -> u8 {
    match direction {
        2 => 4,
        4 => 3,
        3 => 5,
        5 => 2,
        other => other,
    }
}

/// `Direction.get2DDataValue()`: south 0, west 1, north 2, east 3.
fn data_2d(direction: u8) -> i32 {
    match direction {
        3 => 0,
        4 => 1,
        2 => 2,
        _ => 3,
    }
}

fn opposite(direction: u8) -> u8 {
    match direction {
        0 => 1,
        1 => 0,
        2 => 3,
        3 => 2,
        4 => 5,
        _ => 4,
    }
}

/// `Vec3.relative(direction, by)`: the step's ints widened, times `by`, added.
fn relative(at: [f64; 3], direction: u8, by: f64) -> [f64; 3] {
    let step = step(direction);
    std::array::from_fn(|axis| at[axis] + by * f64::from(step[axis]))
}

/// `AABB.ofSize(centre, x, y, z)`: halved by a divide by 2.0.
fn of_size(centre: [f64; 3], size: [f64; 3]) -> Aabb {
    Aabb::new(
        std::array::from_fn(|axis| centre[axis] - size[axis] / 2.0),
        std::array::from_fn(|axis| centre[axis] + size[axis] / 2.0),
    )
}

/// The centre of the block `block` moved towards the wall a hanging entity of
/// `direction` hangs on (`Vec3.atCenterOf(pos).relative(direction, -0.46875)`).
fn against_the_wall(block: BlockPos, direction: u8) -> [f64; 3] {
    let centre = [
        f64::from(block.x) + 0.5,
        f64::from(block.y) + 0.5,
        f64::from(block.z) + 0.5,
    ];
    relative(centre, direction, TOWARDS_THE_WALL)
}

/// `Painting.calculateBoundingBox(pos, direction)` of a painting `width` by `height`:
/// an even size reaches from its block towards the counter-clockwise side and up.
fn painting_box(block: BlockPos, direction: u8, width: u8, height: u8) -> Aabb {
    let mut centre = against_the_wall(block, direction);
    let along = if width.is_multiple_of(2) { 0.5 } else { 0.0 };
    let up = if height.is_multiple_of(2) { 0.5 } else { 0.0 };
    centre = relative(centre, counter_clockwise(direction), along);
    centre = relative(centre, 1, up);
    let x_axis = matches!(direction, 4 | 5);
    let z_axis = matches!(direction, 2 | 3);
    let size = [
        if x_axis { DEPTH } else { f64::from(width) },
        f64::from(height),
        if z_axis { DEPTH } else { f64::from(width) },
    ];
    of_size(centre, size)
}

/// `ItemFrame.createBoundingBox(pos, direction, false)`: the 0.75 box of a frame
/// without a map, on any of the six faces.
fn frame_box(block: BlockPos, direction: u8) -> Aabb {
    let centre = against_the_wall(block, direction);
    let along = step(direction);
    let size = std::array::from_fn(|axis| if along[axis] != 0 { DEPTH } else { FRAME_SIZE });
    of_size(centre, size)
}

/// The box of a frame or a painting record.
fn box_of(data: &EntityData) -> Option<Aabb> {
    match data {
        EntityData::ItemFrame { attached, .. } => {
            Some(frame_box(attached.block, attached.direction))
        }
        EntityData::Painting { attached, variant } => {
            let (width, height) = painting_size(*variant);
            Some(painting_box(
                attached.block,
                attached.direction,
                width,
                height,
            ))
        }
        _ => None,
    }
}

/// The rotations `setDirection` gives a hanging entity of `direction`, yaw and pitch: a
/// quarter turn for each 2D value on a wall, and for a frame on a floor or a ceiling
/// none but a pitch of -90 up and 90 down.
fn rotations(direction: u8) -> (f32, f32) {
    if is_horizontal(direction) {
        ((data_2d(direction) * 90) as f32, 0.0)
    } else if direction == 1 {
        (0.0, -90.0)
    } else {
        (0.0, 90.0)
    }
}

/// `entity` placed as its block, direction and variant say: its position the centre of
/// its box (`recalculateBoundingBox`), its rotations `setDirection`'s.
fn settled(mut entity: Entity) -> Entity {
    let Some(bounds) = box_of(&entity.data) else {
        return entity;
    };
    entity.position = bounds.center();
    if let EntityData::ItemFrame { attached, .. } | EntityData::Painting { attached, .. } =
        &entity.data
    {
        (entity.yaw, entity.pitch) = rotations(attached.direction);
    }
    entity
}

/// A frame or a painting of the entity type `kind` and `data`, new, settled where its
/// data says.
fn hanging(key: EntityId, kind: i32, uuid: uuid::Uuid, data: EntityData) -> Entity {
    settled(Entity {
        key,
        kind: u16::try_from(kind).expect("entity type ids fit 16 bits"),
        uuid,
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    })
}

/// `ItemFrame.getAnalogOutput()`: nothing for an empty frame, else its rotation and one.
fn analog_output(item: Option<ItemStack>, rotation: u8) -> u8 {
    if item.is_none() {
        0
    } else {
        rotation % ROTATIONS + 1
    }
}

/// The item of a frame of the entity type `kind`, glowing or not.
fn frame_item(kind: u16) -> i32 {
    if i32::from(kind) == entity_types::GLOW_ITEM_FRAME {
        items::GLOW_ITEM_FRAME
    } else {
        items::ITEM_FRAME
    }
}

/// The sounds of a frame of the entity type `kind`: placing, breaking, adding,
/// removing and rotating its item (`GlowItemFrame` overrides all five).
struct FrameSounds {
    place: i32,
    broken: i32,
    add: i32,
    remove: i32,
    rotate: i32,
}

fn frame_sounds(kind: u16) -> FrameSounds {
    if i32::from(kind) == entity_types::GLOW_ITEM_FRAME {
        FrameSounds {
            place: sounds::ENTITY_GLOW_ITEM_FRAME_PLACE,
            broken: sounds::ENTITY_GLOW_ITEM_FRAME_BREAK,
            add: sounds::ENTITY_GLOW_ITEM_FRAME_ADD_ITEM,
            remove: sounds::ENTITY_GLOW_ITEM_FRAME_REMOVE_ITEM,
            rotate: sounds::ENTITY_GLOW_ITEM_FRAME_ROTATE_ITEM,
        }
    } else {
        FrameSounds {
            place: sounds::ENTITY_ITEM_FRAME_PLACE,
            broken: sounds::ENTITY_ITEM_FRAME_BREAK,
            add: sounds::ENTITY_ITEM_FRAME_ADD_ITEM,
            remove: sounds::ENTITY_ITEM_FRAME_REMOVE_ITEM,
            rotate: sounds::ENTITY_ITEM_FRAME_ROTATE_ITEM,
        }
    }
}

/// What a hanging entity's item makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hung {
    Painting,
    Frame { kind: i32 },
}

fn hung_by(item: i32) -> Option<Hung> {
    match item {
        items::PAINTING => Some(Hung::Painting),
        items::ITEM_FRAME => Some(Hung::Frame {
            kind: entity_types::ITEM_FRAME,
        }),
        items::GLOW_ITEM_FRAME => Some(Hung::Frame {
            kind: entity_types::GLOW_ITEM_FRAME,
        }),
        _ => None,
    }
}

/// `getPickResult()` of a frame or a painting: a frame's item, or else the frame's own
/// (`ItemFrame`); a plain painting (`Painting`).
pub(crate) fn hanging_pick(entity: &Entity) -> Option<ItemStack> {
    match &entity.data {
        EntityData::ItemFrame { item, .. } => Some(item.map_or(
            ItemStack {
                item: frame_item(entity.kind),
                count: 1,
            },
            |item| ItemStack { count: 1, ..item },
        )),
        EntityData::Painting { .. } => Some(ItemStack {
            item: items::PAINTING,
            count: 1,
        }),
        _ => None,
    }
}

/// Where a hanging entity's drops are laid (`HangingEntity.spawnAtLocation`): its
/// centre, 0.15 out from the wall along x and z (none for a frame on a floor or a
/// ceiling), the step multiplied as a float.
fn drop_point(entity: &Entity) -> [f64; 3] {
    let direction = match &entity.data {
        EntityData::ItemFrame { attached, .. } | EntityData::Painting { attached, .. } => {
            attached.direction
        }
        _ => return entity.position,
    };
    let [x, _, z] = step(direction);
    [
        entity.position[0] + f64::from(x as f32 * DROP_OUT),
        entity.position[1],
        entity.position[2] + f64::from(z as f32 * DROP_OUT),
    ]
}

impl Region {
    /// `HangingEntityItem.useOn` (`ItemFrameItem`'s for the two frames): a painting or a
    /// frame hung in the block in front of the clicked face, facing out of it. A
    /// painting goes only on the four sides, a frame on any face inside the build
    /// height; either only where it would survive there, a painting with a variant
    /// drawn as the game draws it (`Painting.create`). A creative player keeps the item
    /// (`ServerPlayerGameMode.useItemOn` puts its count back), and every player is one.
    /// True where the held item is one of these, whether hung or not.
    pub(crate) fn place_hanging(
        &mut self,
        click: &crate::placement::Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let _ = player;
        let Some(hung) = click.held.and_then(hung_by) else {
            return false;
        };
        // The 3D data value of the face: the protocol's order is the game's.
        let face = click.face as u8;
        let block = click.face.neighbour(click.position);
        // `mayPlace`, `mayUseItemAt` being true for a creative player.
        let may = match hung {
            Hung::Painting => is_horizontal(face),
            Hung::Frame { .. } => {
                let heights = self.heights_at(block);
                block.y >= heights.min_y && block.y < heights.max_y_exclusive
            }
        };
        if !may {
            return true;
        }
        // `Entity.<init>`: the new entity's own random, which gives its UUID first.
        let Some(key) = self.take_key() else {
            return true;
        };
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let attached = Attached {
            block,
            direction: face,
        };
        let entity = match hung {
            Hung::Painting => {
                let Some(variant) = self.draw_variant(key, attached, &mut own) else {
                    // `Painting.create` found nothing that fits: nothing is placed. The
                    // key taken is left unused, as keys may be.
                    return true;
                };
                let data = EntityData::Painting { attached, variant };
                hanging(key, entity_types::PAINTING, uuid, data)
            }
            Hung::Frame { kind } => {
                let data = EntityData::ItemFrame {
                    attached,
                    item: None,
                    rotation: 0,
                    drop_chance: Float(1.0),
                    fixed: false,
                    invisible: false,
                };
                hanging(key, kind, uuid, data)
            }
        };
        if !self.hanging_survives(&entity, entity.chunk()) {
            return true;
        }
        let chunk = entity.chunk();
        let here = self.ticks_chunk(chunk);
        let full = self
            .chunks
            .get(&chunk)
            .is_some_and(|loaded| loaded.entities().len() >= MAX_CHUNK_ENTITIES);
        if here && full {
            return true;
        }
        // `playPlacementSound()`, heard by everyone near, the placer too.
        let sound = match hung {
            Hung::Painting => sounds::ENTITY_PAINTING_PLACE,
            Hung::Frame { .. } => frame_sounds(entity.kind).place,
        };
        self.hanging_sound(&entity, sound, output);
        if here {
            // The draws the stream gave: two for the UUID and one for a painting's
            // variant (`nextInt` may draw again where it rejects, which no later draw
            // of this tick follows).
            let drawn = if hung == Hung::Painting { 3 } else { 2 };
            self.entity_drew(key, drawn);
            self.add_fresh_entity(entity, output);
        } else {
            // Into a chunk the region does not tick (another region's, across a line),
            // the entity goes as an arrival as it is made, as a throw does.
            self.send_made(entity, output);
        }
        true
    }

    /// `Painting.create(level, pos, face)`'s choice of variant, from the new painting's
    /// own stream `own` after its UUID: the placeable variants, in the tag's order, that
    /// would survive as `attached` says, then only those of the largest area among them,
    /// and of those one by `nextInt` of their number, drawn even where that is 1.
    /// `None` where none survives, and then nothing is drawn.
    fn draw_variant(&self, key: EntityId, attached: Attached, own: &mut Random) -> Option<u16> {
        let fitting: Vec<u16> = placeable_variants()
            .iter()
            .filter_map(|&variant| u16::try_from(variant).ok())
            .filter(|&variant| {
                let data = EntityData::Painting { attached, variant };
                let painting = hanging(key, entity_types::PAINTING, uuid::Uuid::nil(), data);
                self.hanging_survives(&painting, painting.chunk())
            })
            .collect();
        let area = |variant: u16| {
            let (width, height) = painting_size(variant);
            u32::from(width) * u32::from(height)
        };
        let largest = fitting.iter().map(|&variant| area(variant)).max()?;
        let largest: Vec<u16> = fitting
            .into_iter()
            .filter(|&variant| area(variant) >= largest)
            .collect();
        let count = u32::try_from(largest.len()).expect("fewer variants than fit a u32");
        let index = own.next_int(count);
        largest.get(usize::try_from(index).ok()?).copied()
    }

    /// `HangingEntity.survives()` of a painting, and `ItemFrame.survives()`: a fixed frame
    /// always; otherwise no block's shape in its box, the blocks behind it solid (or a
    /// diode, but under a frame on a floor or a ceiling), and no other hanging entity in
    /// its way (`canCoexist`).
    pub(crate) fn hanging_survives(&self, entity: &Entity, position: ChunkPos) -> bool {
        let Some(bounds) = box_of(&entity.data) else {
            return true;
        };
        if matches!(entity.data, EntityData::ItemFrame { fixed: true, .. }) {
            return true;
        }
        let view = View {
            region: self,
            home: position,
        };
        // `hasLevelCollision(getPopBox())`: any block's collision shape in the box. The
        // world border is not checked: Clustine has none.
        let context = Context {
            bottom: entity.position[1],
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        if !collide::block_collisions(&view, context, &bounds).is_empty() {
            return false;
        }
        let supporting = |pos: BlockPos| {
            let state = self.block(pos).unwrap_or(blocks::AIR);
            (
                state.props().is_solid(),
                is_class(state, BlockClass::DiodeBlock),
            )
        };
        match &entity.data {
            EntityData::ItemFrame { attached, .. } => {
                let behind = clustine_blocks::data::relative(
                    attached.block,
                    face_of(opposite(attached.direction)),
                );
                let (solid, diode) = supporting(behind);
                if !solid && !(is_horizontal(attached.direction) && diode) {
                    return false;
                }
                self.hanging_coexists(entity, attached.direction, bounds, true)
            }
            EntityData::Painting { attached, .. } => {
                // `calculateSupportBox()`: the box half a block into the wall, a hair
                // smaller, over exactly the blocks behind the painting. **Inferred**:
                // `findBlocksIn` visits every block from the floor of its minimum to the
                // floor of its maximum (`BlockScanUtils` is not in the dump).
                let back = Aabb {
                    min: relative(bounds.min, attached.direction, -0.5),
                    max: relative(bounds.max, attached.direction, -0.5),
                }
                .deflate(1.0e-7);
                let low = crate::physics::containing(back.min);
                let high = crate::physics::containing(back.max);
                for x in low.x..=high.x {
                    for y in low.y..=high.y {
                        for z in low.z..=high.z {
                            let (solid, diode) = supporting(BlockPos::new(x, y, z));
                            if !solid && !diode {
                                return false;
                            }
                        }
                    }
                }
                self.hanging_coexists(entity, attached.direction, bounds, false)
            }
            _ => true,
        }
    }

    /// `canCoexist(sameDirectionOnly)`: no other frame or painting of the loaded and
    /// copied chunks whose box meets `bounds` (strictly, **Inferred**: the level's
    /// lookup is not in the dump) and that faces `direction`, or, where not
    /// `same_direction_only`, is of the same type. Knots and cushions are not hanging
    /// entities and never count.
    fn hanging_coexists(
        &self,
        entity: &Entity,
        direction: u8,
        bounds: Aabb,
        same_direction_only: bool,
    ) -> bool {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                for other in chunk.entities() {
                    if other.key == entity.key {
                        continue;
                    }
                    let other_direction = match &other.data {
                        EntityData::ItemFrame { attached, .. }
                        | EntityData::Painting { attached, .. } => attached.direction,
                        _ => continue,
                    };
                    let in_the_way = (!same_direction_only && other.kind == entity.kind)
                        || other_direction == direction;
                    if in_the_way && box_of(&other.data).is_some_and(|own| own.intersects(bounds)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Whether a push or a move leaves the entity whole: a fixed frame ignores both
    /// (`ItemFrame.move` and `push`).
    pub(crate) fn hanging_ignores_moves(&self, entity: &Entity) -> bool {
        matches!(entity.data, EntityData::ItemFrame { fixed: true, .. })
    }

    /// `dropItem(level, breaker)` of a frame or a painting, already removed: its break
    /// sound, then what it drops but to a creative breaker. A painting drops a plain
    /// painting (which draws its variant again where it is hung); a frame that is not
    /// fixed its own item and then the item it held, the second only where one float of
    /// its stream is below its drop chance, and the comparators about it are told that
    /// its item went (`setItem(EMPTY)`).
    pub(crate) fn hanging_drop(
        &mut self,
        entity: &Entity,
        breaker: Breaker,
        output: &mut TickOutput,
    ) {
        let creative = breaker == (Breaker::Player { creative: true });
        match &entity.data {
            EntityData::Painting { .. } => {
                self.hanging_sound(entity, sounds::ENTITY_PAINTING_BREAK, output);
                if creative {
                    return;
                }
                let stack = ItemStack {
                    item: items::PAINTING,
                    count: 1,
                };
                self.spawn_item(drop_point(entity), stack, output);
            }
            EntityData::ItemFrame { .. } => {
                self.hanging_sound(entity, frame_sounds(entity.kind).broken, output);
                self.frame_drop(entity, creative, true, output);
            }
            _ => {}
        }
    }

    /// `ItemFrame.dropItem(level, breaker, dropSelf)`: nothing of a fixed frame; else its
    /// item is taken out (the record changed where it is still there, and the
    /// comparators told), and unless the breaker is creative the frame's own item drops
    /// where `drop_self`, then the item it held where the frame's float is below its
    /// chance.
    fn frame_drop(
        &mut self,
        entity: &Entity,
        creative: bool,
        drop_self: bool,
        output: &mut TickOutput,
    ) {
        let EntityData::ItemFrame {
            attached,
            item,
            fixed,
            drop_chance,
            ..
        } = entity.data
        else {
            return;
        };
        if fixed {
            return;
        }
        self.set_frame_item(entity, None, output);
        self.update_neighbour_for_output_signal(attached.block, blocks::AIR, output);
        if creative {
            return;
        }
        let at = drop_point(entity);
        if drop_self {
            let stack = ItemStack {
                item: frame_item(entity.kind),
                count: 1,
            };
            self.spawn_item(at, stack, output);
        }
        if let Some(item) = item {
            // Drawn though the chance is 1 unless set otherwise.
            if self.next_float_of(entity.key) < drop_chance.0 {
                self.spawn_item(at, item, output);
            }
        }
    }

    /// The next float of the stream of the entity `key` in this tick, on from what it
    /// has drawn.
    fn next_float_of(&mut self, key: EntityId) -> f32 {
        let mut random = self.entity_stream(key).random;
        self.entity_drew(key, 1);
        random.next_float()
    }

    /// `ItemFrame.setItem` and `setRotation` as the record keeps them: where the frame is
    /// still in its chunk, its data changed, committed, and reported to the edges, which
    /// send the changed values (its box does not change without maps).
    fn set_frame_data(&mut self, entity: &Entity, data: EntityData, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(entity.key) else {
            return;
        };
        if record.data == data {
            return;
        }
        let changed = Entity {
            data: data.clone(),
            ..record.clone()
        };
        self.commit_entity(chunk, entity.key, vec![EntityField::Data(data)], output);
        let mut report = super::items::tracked(&changed, 0, true);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `ItemFrame.setItem(item)`: one of it, or none.
    fn set_frame_item(
        &mut self,
        entity: &Entity,
        item: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        if let EntityData::ItemFrame {
            attached,
            rotation,
            drop_chance,
            fixed,
            invisible,
            ..
        } = entity.data
        {
            let data = EntityData::ItemFrame {
                attached,
                item: item.map(|item| ItemStack { count: 1, ..item }),
                rotation,
                drop_chance,
                fixed,
                invisible,
            };
            self.set_frame_data(entity, data, output);
        }
    }

    /// The box of a frame or a painting (`calculateBoundingBox`), by its direction and
    /// its variant.
    pub(crate) fn hanging_box(&self, entity: &Entity) -> Option<Aabb> {
        box_of(&entity.data)
    }

    /// A player's hit on, or use of, a frame or a painting, made here or in another
    /// region (`docs/groundwork/n5-plan.md`, section 7 item 9): a hit is
    /// `BlockAttachedEntity.skipAttackInteraction`'s `hurtServer` with no damage, which
    /// breaks a painting, takes a frame's item out or else breaks the frame; a click on a
    /// frame puts the held item in or turns the item it holds (`ItemFrame.interact`). A
    /// click on a painting does nothing (`Entity.interact`'s leash logic, a painting
    /// being no `Leashable`).
    pub(crate) fn hanging_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        match (&entity.data, by) {
            (EntityData::Painting { .. }, EntityUse::Attack { creative, .. }) => {
                self.hurt_attached(entity.key, Breaker::Player { creative }, output);
            }
            (EntityData::ItemFrame { .. }, EntityUse::Attack { creative, .. }) => {
                self.hurt_frame(entity, creative, output);
            }
            (EntityData::ItemFrame { .. }, EntityUse::Interact { held, .. }) => {
                self.frame_interact(entity, held, output);
            }
            _ => {}
        }
    }

    /// `ItemFrame.hurtServer` of a player's hit: a fixed frame breaks only to a creative
    /// player, dropping nothing; a frame with an item gives the item up (a creative
    /// player's is deleted) with the remove sound; an empty one breaks
    /// (`BlockAttachedEntity.hurtServer`).
    fn hurt_frame(&mut self, entity: &Entity, creative: bool, output: &mut TickOutput) {
        let EntityData::ItemFrame { item, fixed, .. } = entity.data else {
            return;
        };
        if fixed {
            if creative {
                self.hurt_attached(entity.key, Breaker::Player { creative }, output);
            }
            return;
        }
        if item.is_some() {
            self.frame_drop(entity, creative, false, output);
            self.hanging_sound(entity, frame_sounds(entity.kind).remove, output);
            return;
        }
        self.hurt_attached(entity.key, Breaker::Player { creative }, output);
    }

    /// `ItemFrame.interact`: nothing for a fixed frame; an empty frame takes one of what
    /// is held (the add sound, the comparators told), and one with an item turns it an
    /// eighth (the rotate sound, the comparators told), whatever is held. The held stack
    /// is a creative player's, which the game puts back (`Player.interactOn`).
    fn frame_interact(
        &mut self,
        entity: &Entity,
        held: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let EntityData::ItemFrame {
            attached,
            item,
            rotation,
            drop_chance,
            fixed,
            invisible,
        } = entity.data
        else {
            return;
        };
        if fixed {
            return;
        }
        let sounds = frame_sounds(entity.kind);
        if item.is_none() {
            let Some(held) = held.filter(|held| held.count > 0) else {
                return;
            };
            self.set_frame_item(entity, Some(held), output);
            self.hanging_sound(entity, sounds.add, output);
            self.update_neighbour_for_output_signal(attached.block, blocks::AIR, output);
            return;
        }
        self.hanging_sound(entity, sounds.rotate, output);
        let data = EntityData::ItemFrame {
            attached,
            item,
            rotation: (rotation + 1) % ROTATIONS,
            drop_chance,
            fixed,
            invisible,
        };
        self.set_frame_data(entity, data, output);
        self.update_neighbour_for_output_signal(attached.block, blocks::AIR, output);
    }

    /// `Entity.playSound(sound, 1, 1)` of a hanging entity: at its position, of the
    /// neutral source, heard by everyone near.
    fn hanging_sound(&mut self, entity: &Entity, sound: i32, output: &mut TickOutput) {
        let [x, y, z] = entity.position;
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(x, y, z),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// What a comparator facing `facing` reads of the frames at the block `block`
    /// beyond the conductor behind it (`ComparatorBlock.getItemFrame` and
    /// `getAnalogOutput`): the output of the one frame of the loaded and copied chunks
    /// that faces the same way and whose box meets the block's, and nothing where there
    /// are none or two or more.
    pub(crate) fn frame_signal(&self, block: BlockPos, facing: Face) -> Option<u8> {
        let cell = Aabb::of_block(block);
        let low = ChunkPos::containing(cell.min[0] - 2.0, cell.min[2] - 2.0);
        let high = ChunkPos::containing(cell.max[0] + 2.0, cell.max[2] + 2.0);
        let mut found = None;
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                for other in chunk.entities() {
                    let EntityData::ItemFrame {
                        attached,
                        item,
                        rotation,
                        ..
                    } = &other.data
                    else {
                        continue;
                    };
                    if attached.direction != facing as u8
                        || !frame_box(attached.block, attached.direction).intersects(cell)
                    {
                        continue;
                    }
                    if found.is_some() {
                        return None;
                    }
                    found = Some(analog_output(*item, *rotation));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_on_the_north_face_of_a_block_is_the_briefs_example() {
        // The brief's example (section 6.5): a frame on the north face of (0, 64, 0).
        let bounds = frame_box(BlockPos::new(0, 64, -1), 2);
        assert_eq!(bounds.min, [0.125, 64.125, -0.0625]);
        assert_eq!(bounds.max, [0.875, 64.875, 0.0]);
        assert_eq!(bounds.center(), [0.5, 64.5, -0.03125]);
    }

    #[test]
    fn a_frame_on_a_floor_lies_flat_and_one_on_a_ceiling_hangs_under_it() {
        let floor = frame_box(BlockPos::new(3, 10, 3), 1);
        assert_eq!(floor.min, [3.125, 10.0, 3.125]);
        assert_eq!(floor.max, [3.875, 10.0625, 3.875]);
        let ceiling = frame_box(BlockPos::new(3, 10, 3), 0);
        assert_eq!(ceiling.min, [3.125, 10.9375, 3.125]);
        assert_eq!(ceiling.max, [3.875, 11.0, 3.875]);
        assert_eq!(rotations(1), (0.0, -90.0));
        assert_eq!(rotations(0), (0.0, 90.0));
    }

    #[test]
    fn a_painting_of_even_width_reaches_to_the_counter_clockwise_side_and_up() {
        // A 2 by 1 facing south at (x, y, z) covers x..x+2, y..y+1, z..z+0.0625.
        let two = painting_box(BlockPos::new(15, 64, 7), 3, 2, 1);
        assert_eq!(two.min, [15.0, 64.0, 7.0]);
        assert_eq!(two.max, [17.0, 65.0, 7.0625]);
        // Its centre is on the chunk line: it is in chunk 1, its block in chunk 0.
        assert_eq!(two.center()[0], 16.0);
        // A 4 by 3 facing west: two blocks either side of the line between its block
        // and the one to its south, from a block below to two above.
        let four = painting_box(BlockPos::new(0, 64, 0), 4, 4, 3);
        assert_eq!(four.min, [0.9375, 63.0, -1.0]);
        assert_eq!(four.max, [1.0, 66.0, 3.0]);
        // A 1 by 1 facing south is centred on its block, against its north side.
        let one = painting_box(BlockPos::new(0, 0, 0), 3, 1, 1);
        assert_eq!(one.center(), [0.5, 0.5, 0.03125]);
        assert_eq!(rotations(3), (0.0, 0.0));
        assert_eq!(rotations(4), (90.0, 0.0));
        assert_eq!(rotations(2), (180.0, 0.0));
        assert_eq!(rotations(5), (270.0, 0.0));
    }

    #[test]
    fn the_placeable_tag_holds_forty_seven_variants_in_its_files_order() {
        let tag = placeable_variants();
        assert_eq!(tag.len(), 47);
        assert_eq!(&tag[..5], &[24, 1, 0, 2, 5]);
        assert_eq!(tag[46], 14);
        // kebab, the tag's first, is 1 by 1; dennis, its last, 3 by 3.
        assert_eq!(painting_size(24), (1, 1));
        assert_eq!(painting_size(14), (3, 3));
    }

    #[test]
    fn a_frame_gives_nothing_empty_and_its_rotation_and_one_with_an_item() {
        assert_eq!(analog_output(None, 5), 0);
        let item = Some(ItemStack { item: 1, count: 1 });
        assert_eq!(analog_output(item, 0), 1);
        assert_eq!(analog_output(item, 7), 8);
    }

    #[test]
    fn drops_are_laid_fifteen_hundredths_out_from_the_wall_as_a_float() {
        let data = EntityData::ItemFrame {
            attached: Attached {
                block: BlockPos::new(0, 64, -1),
                direction: 2,
            },
            item: None,
            rotation: 0,
            drop_chance: Float(1.0),
            fixed: false,
            invisible: false,
        };
        let entity = hanging(
            EntityId(1),
            entity_types::ITEM_FRAME,
            uuid::Uuid::nil(),
            data,
        );
        assert_eq!(entity.position, [0.5, 64.5, -0.03125]);
        assert_eq!((entity.yaw, entity.pitch), (180.0, 0.0));
        assert_eq!(
            drop_point(&entity),
            [0.5, 64.5, -0.03125 - 0.150_000_005_960_464_48]
        );
    }
}
