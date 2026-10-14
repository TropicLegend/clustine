// Adapted from SteelMC (steel-utils/src/rotation.rs, direction.rs, geometry.rs and
// types/ at commit 885c4b3e60ed79862c37311780774f76806cb714), which builds these on the
// crate `glam`; here they stand alone, with the names its structure code uses, so that
// that code could be adapted with few changes. See NOTICE in this crate.

//! Whole-number geometry as the game's structures use it: a vector, a box with both
//! ends included, the six directions, the four rotations, and names.

use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub};

use clustine_noise::random::Random;

/// Three whole numbers.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct IVec3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl IVec3 {
    pub const ZERO: Self = Self::new(0, 0, 0);
    pub const ONE: Self = Self::new(1, 1, 1);

    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn splat(value: i32) -> Self {
        Self::new(value, value, value)
    }

    /// The smaller of each coordinate.
    #[must_use]
    pub fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    /// The larger of each coordinate.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }
}

impl From<(i32, i32, i32)> for IVec3 {
    fn from((x, y, z): (i32, i32, i32)) -> Self {
        Self::new(x, y, z)
    }
}

impl From<[i32; 3]> for IVec3 {
    fn from([x, y, z]: [i32; 3]) -> Self {
        Self::new(x, y, z)
    }
}

impl From<IVec3> for (i32, i32, i32) {
    fn from(vector: IVec3) -> Self {
        (vector.x, vector.y, vector.z)
    }
}

impl Add for IVec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl AddAssign for IVec3 {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for IVec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<i32> for IVec3 {
    type Output = Self;
    fn mul(self, factor: i32) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

impl Neg for IVec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// The position of a block.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BlockPos(pub IVec3);

impl BlockPos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self(IVec3::new(x, y, z))
    }

    #[must_use]
    pub const fn x(&self) -> i32 {
        self.0.x
    }

    #[must_use]
    pub const fn y(&self) -> i32 {
        self.0.y
    }

    #[must_use]
    pub const fn z(&self) -> i32 {
        self.0.z
    }

    #[must_use]
    pub const fn offset(&self, dx: i32, dy: i32, dz: i32) -> Self {
        Self::new(self.0.x + dx, self.0.y + dy, self.0.z + dz)
    }

    /// The block `count` steps in a direction.
    #[must_use]
    pub fn relative_n(&self, direction: Direction, count: i32) -> Self {
        Self(self.0 + direction.offset_vec() * count)
    }

    #[must_use]
    pub fn relative(self, direction: Direction) -> Self {
        self.relative_n(direction, 1)
    }
}

/// A chunk by its x and z.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// The game's `ChunkPos.pack`: x in the low half, z in the high.
    #[must_use]
    pub const fn pack(self) -> i64 {
        (self.x as i64 & 0xffff_ffff) | ((self.z as i64 & 0xffff_ffff) << 32)
    }
}

/// A name of the game: a namespace and a path.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Identifier {
    pub namespace: &'static str,
    pub path: &'static str,
}

impl Identifier {
    #[must_use]
    pub const fn new_static(namespace: &'static str, path: &'static str) -> Self {
        Self { namespace, path }
    }

    /// A name of the game itself.
    #[must_use]
    pub const fn vanilla_static(path: &'static str) -> Self {
        Self::new_static("minecraft", path)
    }

    /// The name of a structure template of the game by its path, which a structure's
    /// code puts together from parts. The text is the table of templates' own; a
    /// path the table does not have gives a name that no template has.
    #[must_use]
    pub fn vanilla(path: impl AsRef<str>) -> Self {
        let table = clustine_worldgen_data::templates();
        match table.find(path.as_ref()) {
            Some(index) => Self::vanilla_static(table.get(index).name),
            None => Self::vanilla_static(""),
        }
    }

    /// [`Self::vanilla`]; the namespace is the game's whatever is given.
    #[must_use]
    pub fn new(_namespace: &'static str, path: impl AsRef<str>) -> Self {
        Self::vanilla(path)
    }

    /// A name written as `namespace:path`; without a namespace it is the game's.
    #[must_use]
    pub fn of(name: &'static str) -> Self {
        match name.split_once(':') {
            Some((namespace, path)) => Self::new_static(namespace, path),
            None => Self::vanilla_static(name),
        }
    }
}

/// A biome is the biome of a name where the registry has it under that name.
impl PartialEq<Identifier> for clustine_world::Biome {
    fn eq(&self, name: &Identifier) -> bool {
        crate::compat::biome_name(*self)
            .and_then(|known| known.split_once(':'))
            .is_some_and(|(namespace, path)| namespace == name.namespace && path == name.path)
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

/// One of the three axes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// One of the six directions, in the game's order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    /// The four sideways directions in the order of the game's
    /// `Direction.Plane.HORIZONTAL`.
    pub const HORIZONTAL: [Direction; 4] = [
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ];

    /// One step in the direction.
    #[must_use]
    pub const fn offset(self) -> (i32, i32, i32) {
        match self {
            Direction::Down => (0, -1, 0),
            Direction::Up => (0, 1, 0),
            Direction::North => (0, 0, -1),
            Direction::South => (0, 0, 1),
            Direction::West => (-1, 0, 0),
            Direction::East => (1, 0, 0),
        }
    }

    #[must_use]
    pub const fn offset_xz(self) -> (i32, i32) {
        let (x, _, z) = self.offset();
        (x, z)
    }

    #[must_use]
    pub const fn offset_vec(self) -> IVec3 {
        let (x, y, z) = self.offset();
        IVec3::new(x, y, z)
    }

    #[must_use]
    pub const fn axis(self) -> Axis {
        match self {
            Direction::Down | Direction::Up => Axis::Y,
            Direction::North | Direction::South => Axis::Z,
            Direction::West | Direction::East => Axis::X,
        }
    }

    #[must_use]
    pub const fn get_axis(self) -> Axis {
        self.axis()
    }

    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        !matches!(self, Direction::Down | Direction::Up)
    }

    #[must_use]
    pub const fn opposite(self) -> Direction {
        match self {
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::East => Direction::West,
        }
    }

    /// A quarter turn to the right seen from above; up and down stay.
    #[must_use]
    pub const fn rotate_y_clockwise(self) -> Direction {
        match self {
            Direction::North => Direction::East,
            Direction::East => Direction::South,
            Direction::South => Direction::West,
            Direction::West => Direction::North,
            other => other,
        }
    }

    #[must_use]
    pub const fn rotate_y_counter_clockwise(self) -> Direction {
        match self {
            Direction::North => Direction::West,
            Direction::West => Direction::South,
            Direction::South => Direction::East,
            Direction::East => Direction::North,
            other => other,
        }
    }

    /// The game's number of the direction among the six (`get3DDataValue`).
    #[must_use]
    pub const fn get_3d_data_value(self) -> i32 {
        self as i32
    }

    /// The game's number of a sideways direction (`get2DDataValue`): south, west,
    /// north, east. Up and down have none.
    #[must_use]
    pub const fn get_2d_data_value(self) -> i32 {
        match self {
            Direction::South => 0,
            Direction::West => 1,
            Direction::North => 2,
            Direction::East => 3,
            Direction::Down | Direction::Up => -1,
        }
    }

    /// The name the game writes the direction with.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::North => "north",
            Direction::South => "south",
            Direction::West => "west",
            Direction::East => "east",
        }
    }
}

/// A turn about the vertical axis, in the game's order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    CounterClockwise90,
}

const ALL_ROTATIONS: [Rotation; 4] = [
    Rotation::None,
    Rotation::Clockwise90,
    Rotation::Clockwise180,
    Rotation::CounterClockwise90,
];

impl Rotation {
    /// The game's `Rotation.getRandom`: one draw.
    #[must_use]
    pub fn get_random(rng: &mut impl Random) -> Self {
        ALL_ROTATIONS[rng.next_i32_bounded(4) as usize]
    }

    /// The game's `Rotation.getShuffled`: the four, shuffled as `Util.shuffle` does.
    #[must_use]
    pub fn get_shuffled(rng: &mut impl Random) -> [Rotation; 4] {
        let mut rotations = ALL_ROTATIONS;
        for i in (1..4).rev() {
            let j = rng.next_i32_bounded((i + 1) as i32) as usize;
            rotations.swap(i, j);
        }
        rotations
    }

    #[must_use]
    pub const fn rotate(self, direction: Direction) -> Direction {
        match self {
            Self::None => direction,
            Self::Clockwise90 => direction.rotate_y_clockwise(),
            Self::Clockwise180 => direction.rotate_y_clockwise().rotate_y_clockwise(),
            Self::CounterClockwise90 => direction.rotate_y_counter_clockwise(),
        }
    }

    /// This turn and then another.
    #[must_use]
    pub const fn then(self, other: Self) -> Self {
        ALL_ROTATIONS[((self as u8 + other as u8) % 4) as usize]
    }

    /// A position turned about a pivot (`StructureTemplate.transform` without a
    /// mirror).
    #[must_use]
    pub const fn transform_pos(self, pos: IVec3, pivot: IVec3) -> IVec3 {
        let (x, y, z) = (pos.x, pos.y, pos.z);
        let (px, pz) = (pivot.x, pivot.z);
        match self {
            Self::None => IVec3::new(x, y, z),
            Self::Clockwise90 => IVec3::new(px + pz - z, y, pz - px + x),
            Self::Clockwise180 => IVec3::new(px + px - x, y, pz + pz - z),
            Self::CounterClockwise90 => IVec3::new(px - pz + z, y, px + pz - x),
        }
    }

    /// A size as it is after the turn.
    #[must_use]
    pub const fn rotate_size(self, size: IVec3) -> IVec3 {
        match self {
            Self::Clockwise90 | Self::CounterClockwise90 => IVec3::new(size.z, size.y, size.x),
            Self::None | Self::Clockwise180 => size,
        }
    }

    /// A position mirrored front to back where that is asked, then turned.
    #[must_use]
    pub const fn transform_pos_mirrored(
        self,
        pos: IVec3,
        pivot: IVec3,
        mirror_front_back: bool,
    ) -> IVec3 {
        let mx = if mirror_front_back { -pos.x } else { pos.x };
        self.transform_pos(IVec3::new(mx, pos.y, pos.z), pivot)
    }

    /// The box of a template of a size placed at a position
    /// (`StructureTemplate.getBoundingBox`).
    #[must_use]
    pub fn get_bounding_box_full(
        self,
        pos: IVec3,
        size: IVec3,
        pivot: IVec3,
        mirror_front_back: bool,
    ) -> BoundingBox {
        let c1 = self.transform_pos_mirrored(IVec3::ZERO, pivot, mirror_front_back);
        let c2 = self.transform_pos_mirrored(
            IVec3::new(size.x - 1, size.y - 1, size.z - 1),
            pivot,
            mirror_front_back,
        );
        BoundingBox::new(c1.min(c2) + pos, c1.max(c2) + pos)
    }

    #[must_use]
    pub fn get_bounding_box_with_pivot(self, pos: IVec3, size: IVec3, pivot: IVec3) -> BoundingBox {
        self.get_bounding_box_full(pos, size, pivot, false)
    }

    #[must_use]
    pub fn get_bounding_box(self, pos: IVec3, size: IVec3) -> BoundingBox {
        self.get_bounding_box_full(pos, size, IVec3::ZERO, false)
    }

    /// The name the game writes the rotation with.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Clockwise90 => "clockwise_90",
            Self::Clockwise180 => "180",
            Self::CounterClockwise90 => "counterclockwise_90",
        }
    }
}

/// A box of blocks with both ends included (the game's `BoundingBox`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BoundingBox {
    min: IVec3,
    max: IVec3,
}

impl BoundingBox {
    /// The box between two corners, in any order.
    #[must_use]
    pub const fn new(a: IVec3, b: IVec3) -> Self {
        const fn ordered(a: i32, b: i32) -> (i32, i32) {
            if a <= b { (a, b) } else { (b, a) }
        }
        let (min_x, max_x) = ordered(a.x, b.x);
        let (min_y, max_y) = ordered(a.y, b.y);
        let (min_z, max_z) = ordered(a.z, b.z);
        Self {
            min: IVec3::new(min_x, min_y, min_z),
            max: IVec3::new(max_x, max_y, max_z),
        }
    }

    #[must_use]
    pub const fn from_corners(a: BlockPos, b: BlockPos) -> Self {
        Self::new(a.0, b.0)
    }

    #[must_use]
    pub const fn min_corner(&self) -> IVec3 {
        self.min
    }

    #[must_use]
    pub const fn max_corner(&self) -> IVec3 {
        self.max
    }

    #[must_use]
    pub const fn min_x(&self) -> i32 {
        self.min.x
    }

    #[must_use]
    pub const fn min_y(&self) -> i32 {
        self.min.y
    }

    #[must_use]
    pub const fn min_z(&self) -> i32 {
        self.min.z
    }

    #[must_use]
    pub const fn max_x(&self) -> i32 {
        self.max.x
    }

    #[must_use]
    pub const fn max_y(&self) -> i32 {
        self.max.y
    }

    #[must_use]
    pub const fn max_z(&self) -> i32 {
        self.max.z
    }

    /// The box moved.
    #[must_use]
    pub fn translate(self, delta: IVec3) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    /// The box grown by `amount` on every side.
    #[must_use]
    pub fn inflate(self, amount: i32) -> Self {
        self.inflate_xyz(amount, amount, amount)
    }

    #[must_use]
    pub fn inflate_xyz(self, x: i32, y: i32, z: i32) -> Self {
        let by = IVec3::new(x, y, z);
        Self {
            min: self.min - by,
            max: self.max + by,
        }
    }

    #[must_use]
    pub fn deflate(self, amount: i32) -> Self {
        self.inflate(-amount)
    }

    /// The smallest box that holds both.
    #[must_use]
    pub fn encapsulating(a: &Self, b: &Self) -> Self {
        Self {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        }
    }

    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.max.x >= other.min.x
            && self.min.x <= other.max.x
            && self.max.z >= other.min.z
            && self.min.z <= other.max.z
            && self.max.y >= other.min.y
            && self.min.y <= other.max.y
    }

    /// Whether the box reaches into a rectangle of columns.
    #[must_use]
    pub fn intersects_xz(self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        self.min.x <= max_x && self.max.x >= min_x && self.min.z <= max_z && self.max.z >= min_z
    }

    #[must_use]
    pub fn contains(self, pos: IVec3) -> bool {
        self.contains_xyz(pos.x, pos.y, pos.z)
    }

    #[must_use]
    pub fn contains_xyz(self, x: i32, y: i32, z: i32) -> bool {
        x >= self.min.x
            && x <= self.max.x
            && z >= self.min.z
            && z <= self.max.z
            && y >= self.min.y
            && y <= self.max.y
    }

    /// The game's `getXSpan`.
    #[must_use]
    pub fn width(&self) -> i32 {
        self.max.x - self.min.x + 1
    }

    #[must_use]
    pub fn height(&self) -> i32 {
        self.max.y - self.min.y + 1
    }

    #[must_use]
    pub fn depth(&self) -> i32 {
        self.max.z - self.min.z + 1
    }

    /// The game's `getCenter`: halves round towards zero, as its division does.
    #[must_use]
    pub fn center(&self) -> IVec3 {
        IVec3::new(
            self.min.x + (self.max.x - self.min.x + 1) / 2,
            self.min.y + (self.max.y - self.min.y + 1) / 2,
            self.min.z + (self.max.z - self.min.z + 1) / 2,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turned_template_keeps_its_corner_or_swings_about_it() {
        let size = IVec3::new(3, 2, 5);
        let at = IVec3::new(10, 60, 20);
        let none = Rotation::None.get_bounding_box(at, size);
        assert_eq!(
            (none.min_corner(), none.max_corner()),
            (at, IVec3::new(12, 61, 24))
        );
        let quarter = Rotation::Clockwise90.get_bounding_box(at, size);
        assert_eq!(quarter.min_corner(), IVec3::new(6, 60, 20));
        assert_eq!(quarter.max_corner(), IVec3::new(10, 61, 22));
        let half = Rotation::Clockwise180.get_bounding_box(at, size);
        assert_eq!(half.min_corner(), IVec3::new(8, 60, 16));
    }

    #[test]
    fn boxes_meet_when_they_share_a_block() {
        let a = BoundingBox::new(IVec3::new(0, 0, 0), IVec3::new(3, 3, 3));
        assert!(a.intersects(BoundingBox::new(IVec3::new(3, 3, 3), IVec3::new(5, 5, 5))));
        assert!(!a.intersects(BoundingBox::new(IVec3::new(4, 0, 0), IVec3::new(5, 5, 5))));
        assert!(a.contains(IVec3::new(3, 0, 3)) && !a.contains(IVec3::new(4, 0, 3)));
        assert_eq!(a.inflate(12).width(), 28);
    }

    #[test]
    fn turns_add_up_and_carry_directions() {
        assert_eq!(
            Rotation::Clockwise90.then(Rotation::Clockwise180),
            Rotation::CounterClockwise90
        );
        assert_eq!(
            Rotation::CounterClockwise90.rotate(Direction::North),
            Direction::West
        );
        assert_eq!(Rotation::Clockwise180.rotate(Direction::Up), Direction::Up);
        assert_eq!(ChunkPos::new(-1, 2).pack(), 0x0000_0002_ffff_ffff);
    }
}
