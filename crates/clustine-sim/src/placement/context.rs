// Adapted from SteelMC's `steel-core/src/behavior/context.rs` (`BlockPlaceContext`).

//! Vanilla's `BlockPlaceContext`: where a block goes and what a rule may ask about the
//! click (section 4.1).

use clustine_data::BlockState;
use clustine_data::Face as Dir;
use clustine_world::BlockPos;

use super::Blocks;
use super::data;
use super::geometry;

/// The blocks a rule reads, with the clicked block that travels with a click.
pub(crate) struct World<'a> {
    blocks: &'a dyn Blocks,
    clicked_pos: BlockPos,
    clicked: BlockState,
}

impl<'a> World<'a> {
    pub(crate) fn new(blocks: &'a dyn Blocks, clicked_pos: BlockPos, clicked: BlockState) -> Self {
        Self {
            blocks,
            clicked_pos,
            clicked,
        }
    }

    /// The block at `pos`, if it is known: from the region's chunks, or the clicked
    /// block as the click carried it. A rule asks this of what it changes or what it
    /// places at, and places nothing where it is `None` (section 4.3).
    pub(crate) fn known(&self, pos: BlockPos) -> Option<BlockState> {
        self.blocks
            .block(pos)
            .or_else(|| (pos == self.clicked_pos).then_some(self.clicked))
    }

    /// The block at `pos` for a rule that only looks at it: a block of a chunk the
    /// region cannot see is taken as air (section 4.5).
    pub(crate) fn get(&self, pos: BlockPos) -> BlockState {
        self.known(pos).unwrap_or(data::AIR)
    }
}

/// The blocks of a placement as the shared rules of blocks (`clustine-blocks`) read
/// them: a block only looked at in a chunk the region cannot see is air, there is no
/// light (the light a mushroom needs is not judged, section 4.3), and the placing
/// side is taken where the rules' former copies disagreed.
pub(crate) struct View<'w, 'a>(pub(crate) &'w World<'a>);

impl clustine_blocks::BlockView for View<'_, '_> {
    fn next_int(&mut self, _bound: u32) -> u32 {
        0
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        self.0.get(at)
    }

    fn raw_brightness(&mut self, _at: BlockPos) -> Option<u8> {
        None
    }

    // Placing takes a block's shape from its neighbours and schedules nothing: the
    // neighbours do not change in turn (section 4.4), and a placed block's ticks wait
    // for phase B's updates.
    fn schedule_block_tick(&mut self, _at: BlockPos, _block: u16, _delay: i32) {}

    fn schedule_fluid_tick(&mut self, _at: BlockPos, _fluid: u8, _delay: i32) {}

    fn rules(&self) -> clustine_blocks::Rules {
        clustine_blocks::Rules::Placing
    }

    fn comparator_output(&mut self, at: BlockPos) -> u8 {
        self.0.blocks.comparator_output(at)
    }
}

/// One placement as vanilla's `BlockPlaceContext` sees it.
pub(crate) struct Context<'a> {
    pub(crate) world: World<'a>,
    /// The clicked face, `F`.
    pub(crate) face: Dir,
    /// Where the click landed: `P` plus the cursor.
    pub(crate) location: [f64; 3],
    /// Where the block goes, `T`.
    pub(crate) target: BlockPos,
    pub(crate) replace_clicked: bool,
    pub(crate) held: Option<i32>,
    pub(crate) sneaking: bool,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl Context<'_> {
    /// The player's facing from the yaw alone, vanilla's `getHorizontalDirection`.
    pub(crate) fn horizontal(&self) -> Dir {
        geometry::from_y_rot(self.yaw)
    }

    /// Vanilla's `getNearestLookingDirection`: the side the player looks most towards,
    /// whatever was clicked.
    pub(crate) fn nearest_looking(&self) -> Dir {
        geometry::ordered_by_nearest(self.yaw, self.pitch)[0]
    }

    /// Vanilla's `getNearestLookingDirections`: the sides by how directly the player
    /// looks towards each, with the side behind the clicked face first when the block
    /// goes beside the clicked one.
    pub(crate) fn nearest_looking_directions(&self) -> [Dir; 6] {
        let mut order = geometry::ordered_by_nearest(self.yaw, self.pitch);
        if !self.replace_clicked {
            let behind = self.face.opposite();
            if let Some(index) = order.iter().position(|&dir| dir == behind) {
                order.copy_within(0..index, 1);
                order[0] = behind;
            }
        }
        order
    }

    /// How high above the bottom of `T` the click landed. It differs from the cursor's
    /// height when `T` is not the clicked block.
    pub(crate) fn cursor_height(&self) -> f64 {
        self.location[1] - f64::from(self.target.y)
    }

    /// Where on `T` the click landed across, in x and z.
    pub(crate) fn cursor_across(&self) -> (f64, f64) {
        (
            self.location[0] - f64::from(self.target.x),
            self.location[2] - f64::from(self.target.z),
        )
    }

    /// Whether `T` holds a source of water, which a block that can hold water keeps.
    pub(crate) fn water_source_at_target(&self) -> bool {
        data::water_source(self.world.get(self.target))
    }
}
