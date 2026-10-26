//! The game's `BlockPattern` search, as `CarvedPumpkinBlock` uses it to build golems
//! (`docs/groundwork/n7k-brief.md`, 11.7; `docs/groundwork/n7k-plan.md`, sections 2.6
//! and 10): the three golems' patterns, the search over 27 corner cells and 24
//! orientations in the game's order, and where a built golem stands and which cells it
//! clears. Pure: the region reads its blocks through the closure it hands in, and
//! nothing here knows of a region.
//!
//! A cell the reader cannot read (`None`, beyond the region's land or not loaded) fails
//! whatever key wants it but the space. The game's `BlockInWorld.getState` stays null
//! for a cell of an unloaded chunk; `hasState`'s lambda hands that null to the block's
//! predicate, and `BlockStatePredicate.test` (the snow and iron blocks) says false of it.
//! The pumpkin's lambda, the copper's and `BlockStateBase::isAir` would throw on it
//! instead; failing the match is what Clustine does there, since a region cannot throw a
//! tick away.

use clustine_blocks::data::{in_tag, is_block};
use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

/// What a key of a pattern asks of its cell's block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Key {
    /// The space, which `BlockPatternBuilder` makes always true, read or not.
    Any,
    /// `PUMPKINS_PREDICATE`: a carved pumpkin or a jack o'lantern, any facing.
    Pumpkin,
    /// `BlockStatePredicate.forBlock`: any state of this block.
    Block(BlockState),
    /// `BlockStateBase::isAir` (the bootstrap arguments of the iron golem's `~`, read with
    /// `javap -v`): air, cave air or void air.
    Air,
    /// The copper golem's lambdas: `BlockState.is(BlockTags.COPPER)`, every weathering of
    /// the block of copper, waxed or not.
    Copper,
}

impl Key {
    fn test(self, state: Option<BlockState>) -> bool {
        match (self, state) {
            (Key::Any, _) => true,
            (_, None) => false,
            (Key::Pumpkin, Some(state)) => {
                is_block(state, blocks::CARVED_PUMPKIN) || is_block(state, blocks::JACK_O_LANTERN)
            }
            (Key::Block(block), Some(state)) => is_block(state, block),
            (Key::Air, Some(state)) => state.is_air(),
            (Key::Copper, Some(state)) => in_tag(state, "minecraft:copper"),
        }
    }
}

/// A pattern as `BlockPatternBuilder` takes it: its aisles, each its rows from the top
/// and each row its columns, and a key for each character. The space is always `Any`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pattern {
    aisles: &'static [&'static [&'static str]],
    keys: &'static [(char, Key)],
}

impl Pattern {
    /// The number of columns of a row.
    pub(crate) fn width(&self) -> i32 {
        self.aisles
            .first()
            .and_then(|aisle| aisle.first())
            .map_or(0, |row| row.chars().count() as i32)
    }

    /// The number of rows of an aisle.
    pub(crate) fn height(&self) -> i32 {
        self.aisles.first().map_or(0, |aisle| aisle.len() as i32)
    }

    /// The number of aisles.
    pub(crate) fn depth(&self) -> i32 {
        self.aisles.len() as i32
    }

    /// The key at column `x`, row `y` from the top and aisle `z`.
    fn key(&self, x: i32, y: i32, z: i32) -> Key {
        let c = self.aisles[z as usize][y as usize]
            .chars()
            .nth(x as usize)
            .expect("every row of a pattern is as wide as its first");
        if c == ' ' {
            return Key::Any;
        }
        self.keys
            .iter()
            .find(|(key, _)| *key == c)
            .map(|(_, key)| *key)
            .expect("every character of a pattern has its key, as `build` insists")
    }
}

/// `getOrCreateSnowGolemBase`: what a dispenser asks before it places a pumpkin.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the dispenser's question, which no dispenser asks of a pumpkin yet"
    )
)]
pub(crate) const SNOW_GOLEM_BASE: Pattern = Pattern {
    aisles: &[&[" ", "#", "#"]],
    keys: &[('#', Key::Block(blocks::SNOW_BLOCK))],
};

/// `getOrCreateSnowGolemFull`.
pub(crate) const SNOW_GOLEM_FULL: Pattern = Pattern {
    aisles: &[&["^", "#", "#"]],
    keys: &[('^', Key::Pumpkin), ('#', Key::Block(blocks::SNOW_BLOCK))],
};

/// `getOrCreateIronGolemBase`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the dispenser's question, which no dispenser asks of a pumpkin yet"
    )
)]
pub(crate) const IRON_GOLEM_BASE: Pattern = Pattern {
    aisles: &[&["~ ~", "###", "~#~"]],
    keys: &[('#', Key::Block(blocks::IRON_BLOCK)), ('~', Key::Air)],
};

/// `getOrCreateIronGolemFull`.
pub(crate) const IRON_GOLEM_FULL: Pattern = Pattern {
    aisles: &[&["~^~", "###", "~#~"]],
    keys: &[
        ('^', Key::Pumpkin),
        ('#', Key::Block(blocks::IRON_BLOCK)),
        ('~', Key::Air),
    ],
};

/// `getOrCreateCopperGolemBase`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the dispenser's question, which no dispenser asks of a pumpkin yet"
    )
)]
pub(crate) const COPPER_GOLEM_BASE: Pattern = Pattern {
    aisles: &[&[" ", "#"]],
    keys: &[('#', Key::Copper)],
};

/// `getOrCreateCopperGolemFull`.
pub(crate) const COPPER_GOLEM_FULL: Pattern = Pattern {
    aisles: &[&["^", "#"]],
    keys: &[('^', Key::Pumpkin), ('#', Key::Copper)],
};

/// The step of a side, `Direction.getStepX/Y/Z`.
fn step(face: Face) -> [i32; 3] {
    match face {
        Face::Down => [0, -1, 0],
        Face::Up => [0, 1, 0],
        Face::North => [0, 0, -1],
        Face::South => [0, 0, 1],
        Face::West => [-1, 0, 0],
        Face::East => [1, 0, 0],
    }
}

/// `Vec3i.cross`.
fn cross(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `BlockPattern.translateAndRotate`: column `x` runs along `forwards × up`, row `y` down
/// `up` and aisle `z` along `forwards`.
fn translate_and_rotate(
    origin: BlockPos,
    forwards: Face,
    up: Face,
    x: i32,
    y: i32,
    z: i32,
) -> BlockPos {
    let f = step(forwards);
    let u = step(up);
    let l = cross(f, u);
    origin.offset(
        -u[0] * y + l[0] * x + f[0] * z,
        -u[1] * y + l[1] * x + f[1] * z,
        -u[2] * y + l[2] * x + f[2] * z,
    )
}

/// The 24 pairs of `forwards` and `up` that `find` tries at each corner, in its order:
/// `forwards` in `Direction.values()`, then `up` in the same, skipping `up` equal or
/// opposite to `forwards`.
pub(crate) fn orientations() -> impl Iterator<Item = (Face, Face)> {
    Face::ALL.into_iter().flat_map(|forwards| {
        Face::ALL
            .into_iter()
            .filter(move |up| *up != forwards && *up != forwards.opposite())
            .map(move |up| (forwards, up))
    })
}

/// `BlockPattern$BlockPatternMatch`: where a pattern was found and how it lies, and the
/// states the search read of its cells.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Match {
    /// The cell of column 0, row 0 and aisle 0 (`getFrontTopLeft`).
    pub(crate) front_top_left: BlockPos,
    pub(crate) forwards: Face,
    pub(crate) up: Face,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) depth: i32,
    /// By `[z][y][x]`, flattened: what the search read of each cell, `None` for the
    /// spaces, which it does not read.
    states: Vec<Option<BlockState>>,
}

impl Match {
    /// `getBlock(x, y, z).getPos()`: the cell of column `x`, row `y` from the top and
    /// aisle `z`.
    pub(crate) fn block(&self, x: i32, y: i32, z: i32) -> BlockPos {
        translate_and_rotate(self.front_top_left, self.forwards, self.up, x, y, z)
    }

    /// `getBlock(x, y, z).getState()` as the search cached it: the state its key was
    /// tested on. `None` for a space, which the game reads afresh only when asked (after
    /// `clearPatternBlocks` has set it to air, so that it reads air); no golem's full
    /// pattern has one.
    pub(crate) fn state(&self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        self.states[((z * self.height + y) * self.width + x) as usize]
    }
}

/// `BlockPattern.find(level, at)`: the first match of `pattern` with its front top left
/// at one of the corner cells `at + (0..size)³` (`size` the largest of the pattern's
/// three sizes; `BlockPos.betweenClosed`'s order, x fastest, then y, then z), in each of
/// the 24 orientations in [`orientations`]' order, its cells tested column by column,
/// row by row, aisle by aisle as `matches` does. `block` is read once a cell, as the
/// game's cache of `BlockInWorld` does.
pub(crate) fn find(
    pattern: &Pattern,
    at: BlockPos,
    block: impl Fn(BlockPos) -> Option<BlockState>,
) -> Option<Match> {
    let (width, height, depth) = (pattern.width(), pattern.height(), pattern.depth());
    let size = width.max(height).max(depth);
    if size == 0 {
        return None;
    }
    // Every cell the search can reach lies within `size - 1` of a corner on each axis,
    // since the three directions are at right angles: the box `at - (size - 1)` to
    // `at + 2 (size - 1)`.
    let reach = 3 * (size - 1) + 1;
    let low = at.offset(-(size - 1), -(size - 1), -(size - 1));
    let mut cache: Vec<Option<Option<BlockState>>> = vec![None; (reach * reach * reach) as usize];
    let mut read = |pos: BlockPos| -> Option<BlockState> {
        let (dx, dy, dz) = (pos.x - low.x, pos.y - low.y, pos.z - low.z);
        if !(0..reach).contains(&dx) || !(0..reach).contains(&dy) || !(0..reach).contains(&dz) {
            return block(pos);
        }
        let slot = &mut cache[((dz * reach + dy) * reach + dx) as usize];
        *slot.get_or_insert_with(|| block(pos))
    };
    for index in 0..size * size * size {
        let corner = at.offset(index % size, index / size % size, index / size / size);
        for (forwards, up) in orientations() {
            if let Some(found) = matches(
                pattern, corner, forwards, up, &mut read, width, height, depth,
            ) {
                return Some(found);
            }
        }
    }
    None
}

/// `BlockPattern.matches(origin, forwards, up, cache)`.
#[expect(
    clippy::too_many_arguments,
    reason = "the game's sizes, passed as it passes them"
)]
fn matches(
    pattern: &Pattern,
    origin: BlockPos,
    forwards: Face,
    up: Face,
    read: &mut impl FnMut(BlockPos) -> Option<BlockState>,
    width: i32,
    height: i32,
    depth: i32,
) -> Option<Match> {
    let mut states = vec![None; (width * height * depth) as usize];
    for x in 0..width {
        for y in 0..height {
            for z in 0..depth {
                let key = pattern.key(x, y, z);
                if key == Key::Any {
                    continue;
                }
                let state = read(translate_and_rotate(origin, forwards, up, x, y, z));
                if !key.test(state) {
                    return None;
                }
                states[((z * height + y) * width + x) as usize] = state;
            }
        }
    }
    Some(Match {
        front_top_left: origin,
        forwards,
        up,
        width,
        height,
        depth,
        states,
    })
}

/// The golems a pumpkin builds, in the order `trySpawnGolem` tries them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Golem {
    Snow,
    Iron,
    Copper,
}

impl Golem {
    pub(crate) const ALL: [Golem; 3] = [Golem::Snow, Golem::Iron, Golem::Copper];

    /// The pattern without the pumpkin, which `canSpawnGolem` asks.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the dispenser's question, which no dispenser asks of a pumpkin yet"
        )
    )]
    pub(crate) fn base(self) -> &'static Pattern {
        match self {
            Golem::Snow => &SNOW_GOLEM_BASE,
            Golem::Iron => &IRON_GOLEM_BASE,
            Golem::Copper => &COPPER_GOLEM_BASE,
        }
    }

    /// The pattern with the pumpkin, which `trySpawnGolem` builds.
    pub(crate) fn full(self) -> &'static Pattern {
        match self {
            Golem::Snow => &SNOW_GOLEM_FULL,
            Golem::Iron => &IRON_GOLEM_FULL,
            Golem::Copper => &COPPER_GOLEM_FULL,
        }
    }

    /// The cell `trySpawnGolem` hands `spawnGolemInWorld`: the snow golem's bottom block
    /// `(0, 2, 0)`, the iron golem's leg `(1, 2, 0)`, and the copper golem's pumpkin
    /// `(0, 0, 0)`, where the golem stands above its chest.
    pub(crate) fn stand_cell(self, found: &Match) -> BlockPos {
        match self {
            Golem::Snow => found.block(0, 2, 0),
            Golem::Iron => found.block(1, 2, 0),
            Golem::Copper => found.block(0, 0, 0),
        }
    }

    /// `spawnGolemInWorld`'s `snapTo`: the stand cell's `(x + 0.5, y + 0.05, z + 0.5)`,
    /// with yaw and pitch 0.
    pub(crate) fn stand(self, found: &Match) -> [f64; 3] {
        let at = self.stand_cell(found);
        [
            f64::from(at.x) + 0.5,
            f64::from(at.y) + 0.05,
            f64::from(at.z) + 0.5,
        ]
    }
}

/// `CarvedPumpkinBlock.trySpawnGolem`'s search: the snow golem's full pattern, then the
/// iron golem's, then the copper golem's, the first found built.
pub(crate) fn first_golem(
    at: BlockPos,
    block: impl Fn(BlockPos) -> Option<BlockState>,
) -> Option<(Golem, Match)> {
    Golem::ALL
        .into_iter()
        .find_map(|golem| find(golem.full(), at, &block).map(|found| (golem, found)))
}

/// `CarvedPumpkinBlock.canSpawnGolem`: whether any golem's base pattern is found.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the dispenser's question, which no dispenser asks of a pumpkin yet"
    )
)]
pub(crate) fn can_spawn_golem(
    at: BlockPos,
    block: impl Fn(BlockPos) -> Option<BlockState>,
) -> bool {
    Golem::ALL
        .into_iter()
        .any(|golem| find(golem.base(), at, &block).is_some())
}

/// The cells `clearPatternBlocks` sets to air (each with level event 2001 of its state,
/// air's for the `~` cells too) and `updatePatternBlocks` then tells the neighbours of,
/// in their order: column by column, row by row, of the first aisle.
pub(crate) fn pattern_cells(found: &Match) -> Vec<BlockPos> {
    (0..found.width)
        .flat_map(|x| (0..found.height).map(move |y| found.block(x, y, 0)))
        .collect()
}

#[cfg(test)]
#[path = "golem_patterns_tests.rs"]
mod tests;
