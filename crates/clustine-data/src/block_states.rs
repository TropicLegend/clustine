//! What the game's code says of every block state: light, shapes, fluid, and the like.
//!
//! The values are in `generated/block_states.bin`, a packed table that `cargo datagen`
//! makes by asking the game itself (ADR-0019, section 5): one row of 16 bytes for each
//! state id, and behind the rows the distinct shapes the rows point into. This module
//! reads it. [`BlockState::props`] is the way in.
//!
//! Three kinds of block do not answer by their state alone, and their rows hold what
//! they answer in one given place; see [`StateProps::answers_beyond_its_state`].

use crate::BlockState;
use crate::packed::{self, Table};

/// The layout of the table this module reads.
pub const LAYOUT: u16 = 1;

const STATES: usize = 0;
const FACE_SETS: usize = 1;
const FACE_SHAPE_STARTS: usize = 2;
const RECTANGLES: usize = 3;
const COLLISION_SHAPE_STARTS: usize = 4;
const BOXES: usize = 5;
const POST_PROCESS_OFFSETS: usize = 6;

/// The bytes of a row of each section.
pub const ROW_BYTES: [usize; 7] = [16, 12, 4, 32, 4, 48, 3];

static TABLE: Table<7> = Table::parse(
    include_bytes!("generated/block_states.bin"),
    packed::KIND_BLOCK_STATES,
    LAYOUT,
    ROW_BYTES,
);

const _: () = assert!(
    TABLE.rows(STATES) == crate::BLOCK_STATE_COUNT as usize,
    "block_states.bin does not have one row for each block state"
);

/// A side of a block, in the game's order, which is the order of every list of six in
/// the table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Face {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::Down,
        Face::Up,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];

    /// The side across the block.
    pub const fn opposite(self) -> Face {
        match self {
            Face::Down => Face::Up,
            Face::Up => Face::Down,
            Face::North => Face::South,
            Face::South => Face::North,
            Face::West => Face::East,
            Face::East => Face::West,
        }
    }
}

/// What a block has to offer on a side for something to rest on or hang from it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SupportType {
    /// The whole side.
    Full = 0,
    /// The middle of the side, as a torch or a hanging root needs.
    Center = 1,
    /// The rim of the side, as a rail or a snow layer needs.
    Rigid = 2,
}

/// What a piston does to a block.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PushReaction {
    /// The game's `PUSH_PULL`: moved either way.
    PushPull = 0,
    /// The game's `PUSH`: pushed, never pulled.
    Push = 1,
    /// The game's `POPPED`: broken when pushed.
    Popped = 2,
    /// The game's `IMMOVEABLE`.
    Immovable = 3,
    /// The game's `IGNORE_ENTITY`.
    IgnoreEntity = 4,
}

/// The fluid in a block state: of water and lava themselves and of a waterlogged block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FluidProps {
    /// The fluid's id, an index into [`crate::FLUIDS`]. Flowing and still water are two
    /// fluids, as in the game.
    pub fluid: u8,
    /// How much of the block is fluid, from 1 to 8; a source has 8.
    pub amount: u8,
    pub source: bool,
    /// Whether the fluid falls from the block above, which fills the block whatever
    /// its amount.
    pub falling: bool,
}

/// The row of one block state.
#[derive(Clone, Copy)]
pub struct StateProps(&'static [u8]);

impl BlockState {
    /// What the game's code says of this state.
    ///
    /// Panics if the id is not a block state's; ids come from the tables of this crate
    /// or from a chunk that was checked when it was read.
    pub fn props(self) -> StateProps {
        StateProps(TABLE.row(STATES, usize::from(self.0)))
    }
}

impl StateProps {
    /// The light the block gives, 0 to 15. Light starts its spread from these.
    pub fn light_emission(self) -> u8 {
        self.0[0]
    }

    /// How much light is lost on the way through the block, 0 to 15. Light uses it for
    /// every step, and 15 stops light whatever the shapes say.
    pub fn light_dampening(self) -> u8 {
        self.0[1]
    }

    fn flag(self, bit: u32) -> bool {
        packed::row_u16(self.0, 2) & (1 << bit) != 0
    }

    /// The game's `isAir`: air, cave air and void air. The world surface heightmap is
    /// "not air"; features ask it before they place.
    pub fn is_air(self) -> bool {
        self.flag(0)
    }

    /// The game's `isSolid`, which it has deprecated and features still ask (whether a
    /// tree may replace a block, where a spring may sit).
    pub fn is_solid(self) -> bool {
        self.flag(1)
    }

    /// The game's `isSolidRender`: a full, opaque cube. Features and the surface ask it
    /// of what lies under or beside a block.
    pub fn is_solid_render(self) -> bool {
        self.flag(2)
    }

    /// The game's `liquid`: the blocks of water and lava themselves, not a waterlogged
    /// block. See [`Self::fluid`] for what a heightmap or an aquifer asks.
    pub fn is_liquid(self) -> bool {
        self.flag(3)
    }

    /// The game's `canBeReplaced()`: what placing a block may overwrite, such as air,
    /// fluids and grass. Features and placing by a player ask it.
    pub fn can_be_replaced(self) -> bool {
        self.flag(4)
    }

    /// The game's `canOcclude`.
    pub fn occludes(self) -> bool {
        self.flag(5)
    }

    /// The game's `useShapeForLightOcclusion`: light between this block and a neighbour
    /// is stopped where their facing sides together cover the whole face, by
    /// [`Self::occlusion_face`]. Slabs, stairs and the like.
    pub fn occludes_by_shape(self) -> bool {
        self.flag(6)
    }

    /// The game's `propagatesSkylightDown`: sky light falls through without loss.
    pub fn propagates_skylight_down(self) -> bool {
        self.flag(7)
    }

    /// The game's `isLightPermeable`. In 26.3 it is the opposite of
    /// [`Self::is_solid_render`] for every state, which a test of this crate holds
    /// it to; nothing needs to ask both.
    pub fn is_light_permeable(self) -> bool {
        self.flag(8)
    }

    /// Whether the block keeps a block entity. Which type it is, is per block:
    /// [`crate::BlockInfo::block_entity_type`].
    pub fn has_block_entity(self) -> bool {
        self.flag(9)
    }

    /// Whether nothing collides with the block.
    pub fn collision_is_empty(self) -> bool {
        self.flag(10)
    }

    /// The game's `isCollisionShapeFullBlock`.
    pub fn collision_is_full_block(self) -> bool {
        self.flag(11)
    }

    /// Whether the block is in the tag `#minecraft:blocks_motion_in_heightmap`. The
    /// heightmaps "motion blocking" and "ocean floor" are made from it.
    pub fn in_motion_heightmap(self) -> bool {
        self.flag(12)
    }

    /// Whether the block is in the tag `#minecraft:blocks_motion_in_heightmap_no_leaves`,
    /// which the heightmap "motion blocking, no leaves" is made from.
    pub fn in_motion_heightmap_no_leaves(self) -> bool {
        self.flag(13)
    }

    /// Whether the game answers for this block by more than its state, so that the
    /// collision shape and the sturdy faces here are what it answers in one place only.
    ///
    /// - A shulker box and a moving piston look at their block entity. The row holds
    ///   the answer without one: the closed box, and nothing to collide with.
    /// - Bamboo, pointed dripstone and a sulfur spike are shifted sideways by where
    ///   they stand. The row holds the shape at the position (0, 0, 0). Whether the
    ///   shape is empty or a full block does not depend on the position.
    pub fn answers_beyond_its_state(self) -> bool {
        self.flag(14)
    }

    /// The faces that are sturdy for `support`, one bit a face, bit 0 for
    /// [`Face::Down`].
    pub fn sturdy_faces(self, support: SupportType) -> u8 {
        self.0[4 + support as usize]
    }

    /// The game's `isFaceSturdy`: whether `face` can carry what needs `support`.
    /// Features ask it of the ground under a plant and of the wall behind a vine.
    pub fn is_face_sturdy(self, face: Face, support: SupportType) -> bool {
        self.sturdy_faces(support) & (1 << face as u8) != 0
    }

    /// The game's `getPistonPushReaction`. Features ask it for what may be overgrown.
    ///
    /// It is what the state answers and not the whole of whether a piston moves the
    /// block: the game also leaves alone what cannot be broken (bedrock answers
    /// [`PushReaction::PushPull`] here) and what keeps a block entity (a chest does
    /// too). Neither a block's hardness nor that rule is in this table.
    pub fn push_reaction(self) -> PushReaction {
        match self.0[7] {
            0 => PushReaction::PushPull,
            1 => PushReaction::Push,
            2 => PushReaction::Popped,
            3 => PushReaction::Immovable,
            4 => PushReaction::IgnoreEntity,
            other => unreachable!("datagen writes no push reaction {other}"),
        }
    }

    /// The fluid in the block, if any. The two heightmaps of motion count a block with
    /// a fluid as blocking; aquifers and features ask what the fluid is.
    pub fn fluid(self) -> Option<FluidProps> {
        let fluid = self.0[8].checked_sub(1)?;
        let level = self.0[9];
        Some(FluidProps {
            fluid,
            amount: level & 0x3f,
            source: level & 0x40 != 0,
            falling: level & 0x80 != 0,
        })
    }

    /// Where the game marks a position for post-processing when generation places this
    /// state, as an offset from the block: soul sand and magma mark the block above
    /// them, a mushroom itself.
    pub fn post_process(self) -> Option<[i8; 3]> {
        match self.0[10] {
            0 => None,
            1 => Some([0, 0, 0]),
            index => {
                let row = TABLE.row(POST_PROCESS_OFFSETS, usize::from(index) - 2);
                Some([row[0] as i8, row[1] as i8, row[2] as i8])
            }
        }
    }

    /// What of `face` the block covers against light. It is empty unless the state
    /// [occludes by its shape](Self::occludes_by_shape); a block that stops light
    /// altogether does so by its dampening.
    pub fn occlusion_face(self, face: Face) -> FaceShape {
        let set = TABLE.row(FACE_SETS, usize::from(packed::row_u16(self.0, 12)));
        FaceShape(packed::row_u16(set, 2 * face as usize))
    }

    /// The set of six occlusion faces, as a number that two states share exactly when
    /// all six of their faces are equal. Set 0 covers nothing.
    pub fn occlusion_face_set(self) -> u16 {
        packed::row_u16(self.0, 12)
    }

    /// What collides with the block.
    pub fn collision_shape(self) -> CollisionShape {
        CollisionShape(packed::row_u16(self.0, 14))
    }
}

/// The number of distinct sets of six occlusion faces.
pub fn occlusion_face_set_count() -> usize {
    TABLE.rows(FACE_SETS)
}

/// The number of distinct face shapes; a [`FaceShape`]'s index is below it.
pub fn face_shape_count() -> usize {
    TABLE.rows(FACE_SHAPE_STARTS) - 1
}

/// The number of distinct collision shapes.
pub fn collision_shape_count() -> usize {
    TABLE.rows(COLLISION_SHAPE_STARTS) - 1
}

/// What a block covers of one of its faces: rectangles on the unit square.
///
/// Two states have equal shapes on a face exactly when the indices are equal, so light
/// can keep what it works out for a pair of indices.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FaceShape(u16);

impl FaceShape {
    /// Covers nothing.
    pub const EMPTY: FaceShape = FaceShape(0);
    /// Covers the whole face.
    pub const FULL: FaceShape = FaceShape(1);

    /// The shape's number among the distinct face shapes.
    pub fn index(self) -> u16 {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self == Self::EMPTY
    }

    pub fn is_full(self) -> bool {
        self == Self::FULL
    }

    /// The rectangles, each as its lower corner and its upper corner
    /// `[min u, min v, max u, max v]`. `u` and `v` are the two axes that lie in the
    /// face, in the order x, y, z: x and z for the bottom and the top, x and y for
    /// north and south, y and z for west and east.
    pub fn rectangles(self) -> impl Iterator<Item = [f64; 4]> {
        let index = usize::from(self.0);
        let first = packed::row_u32(TABLE.row(FACE_SHAPE_STARTS, index), 0) as usize;
        let end = packed::row_u32(TABLE.row(FACE_SHAPE_STARTS, index + 1), 0) as usize;
        (first..end).map(|rectangle| {
            let row = TABLE.row(RECTANGLES, rectangle);
            [0, 8, 16, 24].map(|at| packed::row_f64(row, at))
        })
    }
}

/// What collides with a block: boxes within (and for a fence or a wall, above) the
/// block's cube.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CollisionShape(u16);

impl CollisionShape {
    /// Nothing collides.
    pub const EMPTY: CollisionShape = CollisionShape(0);
    /// The whole cube.
    pub const FULL_BLOCK: CollisionShape = CollisionShape(1);

    /// The shape's number among the distinct collision shapes.
    pub fn index(self) -> u16 {
        self.0
    }

    /// The boxes, each `[min x, min y, min z, max x, max y, max z]`, in the game's
    /// order.
    pub fn boxes(self) -> impl Iterator<Item = [f64; 6]> {
        let index = usize::from(self.0);
        let first = packed::row_u32(TABLE.row(COLLISION_SHAPE_STARTS, index), 0) as usize;
        let end = packed::row_u32(TABLE.row(COLLISION_SHAPE_STARTS, index + 1), 0) as usize;
        (first..end).map(|shape_box| {
            let row = TABLE.row(BOXES, shape_box);
            [0, 8, 16, 24, 32, 40].map(|at| packed::row_f64(row, at))
        })
    }
}
