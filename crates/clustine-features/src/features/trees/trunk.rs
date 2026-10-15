// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/trunk.rs`.

//! The trunk placers (`TrunkPlacer` and its kinds): how tall a tree is, where its
//! logs go, and where its foliage is attached.

use std::f32::consts::TAU;
use std::f64::consts::PI;

use clustine_data::Face;
use clustine_noise::{Random, math, trig};
use clustine_world::BlockPos;

use super::common::{
    Axis, LOGS, Placement, is_air_or_leaves, manhattan, random_horizontal, shuffle, valid_tree_pos,
    with_axis,
};
use super::tree::TreeBlocks;
use crate::blocks::{BlockSet, relative, relative_by};
use crate::context::{Context, Level};
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

/// Where a foliage placer puts leaves (`FoliagePlacer.FoliageAttachment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attachment {
    pub pos: BlockPos,
    pub radius_offset: i32,
    pub double_trunk: bool,
}

impl Attachment {
    fn new(pos: BlockPos, radius_offset: i32, double_trunk: bool) -> Self {
        Self {
            pos,
            radius_offset,
            double_trunk,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Straight,
    Forking,
    Giant,
    MegaJungle,
    DarkOak,
    Fancy,
    Bending {
        min_height_for_leaves: i32,
        bend_length: IntProvider,
    },
    UpwardsBranching {
        extra_branch_steps: IntProvider,
        place_branch_per_log_probability: f32,
        extra_branch_length: IntProvider,
        can_grow_through: BlockSet,
    },
    Cherry {
        branch_count: IntProvider,
        branch_horizontal_length: IntProvider,
        /// A uniform range, whose bounds the second branch narrows.
        branch_start_offset_from_top: (i32, i32),
        branch_end_offset_from_top: IntProvider,
    },
    Poplar {
        trunk_height_above_branches: IntProvider,
        branch_amount: IntProvider,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrunkPlacer {
    base_height: i32,
    height_rand_a: i32,
    height_rand_b: i32,
    kind: Kind,
}

impl TrunkPlacer {
    pub fn read(reader: &Reader) -> Read<Self> {
        let int = |key: &str| reader.field(key)?.int();
        let provider = |key: &str| IntProvider::read(&reader.field(key)?);
        let kind = match reader.kind()? {
            "minecraft:straight_trunk_placer" => Kind::Straight,
            "minecraft:forking_trunk_placer" => Kind::Forking,
            "minecraft:giant_trunk_placer" => Kind::Giant,
            "minecraft:mega_jungle_trunk_placer" => Kind::MegaJungle,
            "minecraft:dark_oak_trunk_placer" => Kind::DarkOak,
            "minecraft:fancy_trunk_placer" => Kind::Fancy,
            "minecraft:bending_trunk_placer" => Kind::Bending {
                min_height_for_leaves: match reader.optional("min_height_for_leaves") {
                    Some(value) => value.int()?,
                    None => 1,
                },
                bend_length: provider("bend_length")?,
            },
            "minecraft:upwards_branching_trunk_placer" => Kind::UpwardsBranching {
                extra_branch_steps: provider("extra_branch_steps")?,
                place_branch_per_log_probability: reader
                    .field("place_branch_per_log_probability")?
                    .float()?,
                extra_branch_length: provider("extra_branch_length")?,
                can_grow_through: BlockSet::read(&reader.field("can_grow_through")?)?,
            },
            "minecraft:cherry_trunk_placer" => {
                // A `UniformInt` written without its type.
                let start = reader.field("branch_start_offset_from_top")?;
                let min = start.field("min_inclusive")?.int()?;
                let max = start.field("max_inclusive")?.int()?;
                start.finish()?;
                Kind::Cherry {
                    branch_count: provider("branch_count")?,
                    branch_horizontal_length: provider("branch_horizontal_length")?,
                    branch_start_offset_from_top: (min, max),
                    branch_end_offset_from_top: provider("branch_end_offset_from_top")?,
                }
            }
            "minecraft:poplar_trunk_placer" => Kind::Poplar {
                trunk_height_above_branches: provider("trunk_height_above_branches")?,
                branch_amount: provider("branch_amount")?,
            },
            other => return Err(reader.error(format!("{other} is not a trunk placer"))),
        };
        let placer = Self {
            base_height: int("base_height")?,
            height_rand_a: int("height_rand_a")?,
            height_rand_b: int("height_rand_b")?,
            kind,
        };
        reader.finish()?;
        Ok(placer)
    }

    /// `getTreeHeight`.
    pub fn tree_height<R: Random + ?Sized>(&self, random: &mut R) -> i32 {
        self.base_height
            + random.next_i32_bounded(self.height_rand_a + 1)
            + random.next_i32_bounded(self.height_rand_b + 1)
    }

    /// `validTreePos`, which the upwards branching placer widens to the blocks its
    /// trunk grows through.
    pub fn valid_tree_pos(&self, level: &mut Level<'_>, pos: BlockPos) -> bool {
        if valid_tree_pos(level, pos) {
            return true;
        }
        match &self.kind {
            Kind::UpwardsBranching {
                can_grow_through, ..
            } => can_grow_through.contains(level.block(pos)),
            _ => false,
        }
    }

    /// `isFree`: room for a log, or a log already.
    pub fn is_free(&self, level: &mut Level<'_>, pos: BlockPos) -> bool {
        self.valid_tree_pos(level, pos) || LOGS.contains(level.block(pos))
    }

    /// `placeLog`: a log from the trunk's provider, turned to `axis` where one is
    /// given, where the placer may put one.
    fn place_log(
        &self,
        context: &mut Context<'_, '_>,
        blocks: &TreeBlocks,
        placement: &mut Placement,
        pos: BlockPos,
        axis: Option<Axis>,
    ) -> bool {
        if !self.valid_tree_pos(context.level, pos) {
            return false;
        }
        let mut state = blocks.trunk.state(context, pos);
        if let Some(axis) = axis {
            state = with_axis(state, axis);
        }
        placement.set_trunk(context.level, pos, state);
        true
    }

    /// `placeBelowTrunkBlock`: what the provider below the trunk gives, if it gives
    /// anything there.
    fn place_below_trunk(
        context: &mut Context<'_, '_>,
        blocks: &TreeBlocks,
        placement: &mut Placement,
        pos: BlockPos,
    ) {
        if let Some(state) = blocks.below_trunk.optional_state(context, pos) {
            placement.set_trunk(context.level, pos, state);
        }
    }

    /// `placeTrunk`: the logs of a tree `height` tall from `origin`, and where its
    /// foliage goes.
    pub fn place(
        &self,
        context: &mut Context<'_, '_>,
        blocks: &TreeBlocks,
        placement: &mut Placement,
        height: i32,
        origin: BlockPos,
    ) -> Vec<Attachment> {
        let mut trunk = Trunk {
            placer: self,
            context,
            blocks,
            placement,
        };
        match &self.kind {
            Kind::Straight => trunk.straight(height, origin),
            Kind::Forking => trunk.forking(height, origin),
            Kind::Giant => trunk.giant(height, origin),
            Kind::MegaJungle => trunk.mega_jungle(height, origin),
            Kind::DarkOak => trunk.dark_oak(height, origin),
            Kind::Fancy => trunk.fancy(height, origin),
            Kind::Bending {
                min_height_for_leaves,
                bend_length,
            } => trunk.bending(height, origin, *min_height_for_leaves, bend_length),
            Kind::UpwardsBranching {
                extra_branch_steps,
                place_branch_per_log_probability,
                extra_branch_length,
                ..
            } => trunk.upwards_branching(
                height,
                origin,
                extra_branch_steps,
                *place_branch_per_log_probability,
                extra_branch_length,
            ),
            Kind::Cherry {
                branch_count,
                branch_horizontal_length,
                branch_start_offset_from_top,
                branch_end_offset_from_top,
            } => trunk.cherry(
                height,
                origin,
                branch_count,
                branch_horizontal_length,
                *branch_start_offset_from_top,
                branch_end_offset_from_top,
            ),
            Kind::Poplar {
                trunk_height_above_branches,
                branch_amount,
            } => trunk.poplar(height, origin, trunk_height_above_branches, branch_amount),
        }
    }
}

/// One trunk being placed.
struct Trunk<'p, 'c, 'a, 'v> {
    placer: &'p TrunkPlacer,
    context: &'c mut Context<'a, 'v>,
    blocks: &'p TreeBlocks,
    placement: &'p mut Placement,
}

impl Trunk<'_, '_, '_, '_> {
    fn log(&mut self, pos: BlockPos) -> bool {
        self.placer
            .place_log(self.context, self.blocks, self.placement, pos, None)
    }

    fn log_along(&mut self, pos: BlockPos, axis: Axis) -> bool {
        self.placer
            .place_log(self.context, self.blocks, self.placement, pos, Some(axis))
    }

    fn below(&mut self, pos: BlockPos) {
        TrunkPlacer::place_below_trunk(self.context, self.blocks, self.placement, pos);
    }

    fn random(&mut self) -> &mut clustine_noise::WorldgenRandom {
        self.context.random
    }

    fn straight(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        self.below(origin.offset(0, -1, 0));
        for y in 0..height {
            self.log(origin.offset(0, y, 0));
        }
        vec![Attachment::new(origin.offset(0, height, 0), 0, false)]
    }

    fn forking(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        self.below(origin.offset(0, -1, 0));
        let mut attachments = Vec::new();
        let lean = random_horizontal(self.random());
        let lean_height = height - self.random().next_i32_bounded(4) - 1;
        let mut lean_steps = 3 - self.random().next_i32_bounded(3);
        let (mut x, mut z) = (origin.x, origin.z);
        let mut top = None;
        for h in 0..height {
            let y = origin.y + h;
            if h >= lean_height && lean_steps > 0 {
                let moved = relative(BlockPos::new(x, y, z), lean);
                (x, z) = (moved.x, moved.z);
                lean_steps -= 1;
            }
            if self.log(BlockPos::new(x, y, z)) {
                top = Some(y + 1);
            }
        }
        if let Some(y) = top {
            attachments.push(Attachment::new(BlockPos::new(x, y, z), 1, false));
        }
        (x, z) = (origin.x, origin.z);
        let branch = random_horizontal(self.random());
        if branch != lean {
            let mut h = lean_height - self.random().next_i32_bounded(2) - 1;
            let mut steps = 1 + self.random().next_i32_bounded(3);
            top = None;
            while h < height && steps > 0 {
                if h >= 1 {
                    let y = origin.y + h;
                    let moved = relative(BlockPos::new(x, y, z), branch);
                    (x, z) = (moved.x, moved.z);
                    if self.log(BlockPos::new(x, y, z)) {
                        top = Some(y + 1);
                    }
                }
                h += 1;
                steps -= 1;
            }
            if let Some(y) = top {
                attachments.push(Attachment::new(BlockPos::new(x, y, z), 0, false));
            }
        }
        attachments
    }

    /// `placeLogIfFree`.
    fn log_if_free(&mut self, pos: BlockPos) {
        if self.placer.is_free(self.context.level, pos) {
            self.log(pos);
        }
    }

    fn giant(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        let below = origin.offset(0, -1, 0);
        self.below(below);
        self.below(below.offset(1, 0, 0));
        self.below(below.offset(0, 0, 1));
        self.below(below.offset(1, 0, 1));
        for y in 0..height {
            self.log_if_free(origin.offset(0, y, 0));
            if y < height - 1 {
                self.log_if_free(origin.offset(1, y, 0));
                self.log_if_free(origin.offset(1, y, 1));
                self.log_if_free(origin.offset(0, y, 1));
            }
        }
        vec![Attachment::new(origin.offset(0, height, 0), 0, true)]
    }

    fn mega_jungle(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        let mut attachments = self.giant(height, origin);
        let mut branch_height = height - 2 - self.random().next_i32_bounded(4);
        while branch_height > height / 2 {
            let angle = self.random().next_f32() * TAU;
            let (mut x, mut z) = (0, 0);
            for step in 0..5 {
                // The game's table of sines, as `Mth.cos` and `Mth.sin` read it.
                x = (1.5_f32 + trig::cos(f64::from(angle)) * step as f32) as i32;
                z = (1.5_f32 + trig::sin(f64::from(angle)) * step as f32) as i32;
                self.log(origin.offset(x, branch_height - 3 + step / 2, z));
            }
            attachments.push(Attachment::new(
                origin.offset(x, branch_height, z),
                -2,
                false,
            ));
            branch_height -= 2 + self.random().next_i32_bounded(4);
        }
        attachments
    }

    fn dark_oak(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        let mut attachments = Vec::new();
        let below = origin.offset(0, -1, 0);
        self.below(below);
        self.below(below.offset(1, 0, 0));
        self.below(below.offset(0, 0, 1));
        self.below(below.offset(1, 0, 1));
        let lean = random_horizontal(self.random());
        let lean_height = height - self.random().next_i32_bounded(4);
        let mut lean_steps = 2 - self.random().next_i32_bounded(3);
        let (mut x, mut z) = (origin.x, origin.z);
        let top = origin.y + height - 1;
        for h in 0..height {
            if h >= lean_height && lean_steps > 0 {
                let moved = relative(BlockPos::new(x, 0, z), lean);
                (x, z) = (moved.x, moved.z);
                lean_steps -= 1;
            }
            let pos = BlockPos::new(x, origin.y + h, z);
            if is_air_or_leaves(self.context.level, pos) {
                self.log(pos);
                self.log(pos.offset(1, 0, 0));
                self.log(pos.offset(0, 0, 1));
                self.log(pos.offset(1, 0, 1));
            }
        }
        attachments.push(Attachment::new(BlockPos::new(x, top, z), 0, true));
        for dx in -1..=2 {
            for dz in -1..=2 {
                if (0..=1).contains(&dx) && (0..=1).contains(&dz) {
                    continue;
                }
                if self.random().next_i32_bounded(3) > 0 {
                    continue;
                }
                let length = self.random().next_i32_bounded(3) + 2;
                for y in 0..length {
                    self.log(BlockPos::new(origin.x + dx, top - y - 1, origin.z + dz));
                }
                attachments.push(Attachment::new(
                    BlockPos::new(origin.x + dx, top, origin.z + dz),
                    0,
                    false,
                ));
            }
        }
        attachments
    }

    fn fancy(&mut self, tree_height: i32, origin: BlockPos) -> Vec<Attachment> {
        let height = tree_height + 2;
        let trunk_height = math::floor(f64::from(height) * 0.618);
        self.below(origin.offset(0, -1, 0));
        let clusters_per_y = 1.min(math::floor(1.382 + (f64::from(height) / 13.0).powi(2)));
        let trunk_top = origin.y + trunk_height;
        let mut relative_y = height - 5;
        // Each attachment with the y its branch starts from.
        let mut coords = vec![(
            Attachment::new(origin.offset(0, relative_y, 0), 0, false),
            trunk_top,
        )];
        while relative_y >= 0 {
            let shape = fancy_shape(height, relative_y);
            if shape >= 0.0 {
                for _ in 0..clusters_per_y {
                    let radius = f64::from(shape) * (f64::from(self.random().next_f32()) + 0.328);
                    let angle = f64::from(self.random().next_f32() * 2.0) * PI;
                    let x = radius * libm::sin(angle) + 0.5;
                    let z = radius * libm::cos(angle) + 0.5;
                    let start = origin.offset(math::floor(x), relative_y - 1, math::floor(z));
                    let end = start.offset(0, 5, 0);
                    if self.limb(start, end, false) {
                        let dx = origin.x - start.x;
                        let dz = origin.z - start.z;
                        let branch_height =
                            f64::from(start.y) - f64::from(dx * dx + dz * dz).sqrt() * 0.381;
                        let branch_top = if branch_height > f64::from(trunk_top) {
                            trunk_top
                        } else {
                            branch_height as i32
                        };
                        let base = BlockPos::new(origin.x, branch_top, origin.z);
                        if self.limb(base, start, false) {
                            coords.push((Attachment::new(start, 0, false), base.y));
                        }
                    }
                }
            }
            relative_y -= 1;
        }
        self.limb(origin, origin.offset(0, trunk_height, 0), true);
        for (attachment, base_y) in &coords {
            let base = BlockPos::new(origin.x, *base_y, origin.z);
            if base != attachment.pos && trim_fancy_branch(height, base_y - origin.y) {
                self.limb(base, attachment.pos, true);
            }
        }
        coords
            .into_iter()
            .filter(|(_, base_y)| trim_fancy_branch(height, base_y - origin.y))
            .map(|(attachment, _)| attachment)
            .collect()
    }

    /// `FancyTrunkPlacer.makeLimb`: a line of logs from `start` to `end`, or, where
    /// nothing is placed, whether there is room for one.
    fn limb(&mut self, start: BlockPos, end: BlockPos, place: bool) -> bool {
        if !place && start == end {
            return true;
        }
        let delta = (end.x - start.x, end.y - start.y, end.z - start.z);
        let steps = delta.0.abs().max(delta.1.abs()).max(delta.2.abs());
        // A limb of no length places its one log upright; the game's division by
        // nought gives the same.
        let (dx, dy, dz) = if steps == 0 {
            (0.0_f32, 0.0_f32, 0.0_f32)
        } else {
            (
                delta.0 as f32 / steps as f32,
                delta.1 as f32 / steps as f32,
                delta.2 as f32 / steps as f32,
            )
        };
        for step in 0..=steps {
            let i = step as f32;
            let pos = start.offset(
                math::floor(f64::from(0.5_f32 + i * dx)),
                math::floor(f64::from(0.5_f32 + i * dy)),
                math::floor(f64::from(0.5_f32 + i * dz)),
            );
            if place {
                let axis = fancy_log_axis(start, pos);
                self.log_along(pos, axis);
            } else if !self.placer.is_free(self.context.level, pos) {
                return false;
            }
        }
        true
    }

    fn bending(
        &mut self,
        height: i32,
        origin: BlockPos,
        min_height_for_leaves: i32,
        bend_length: &IntProvider,
    ) -> Vec<Attachment> {
        let direction = random_horizontal(self.random());
        let log_height = height - 1;
        let mut pos = origin;
        self.below(origin.offset(0, -1, 0));
        let mut points = Vec::new();
        for y in 0..=log_height {
            if y + 1 >= log_height + self.random().next_i32_bounded(2) {
                pos = relative(pos, direction);
            }
            self.log(pos);
            if y >= min_height_for_leaves {
                points.push(Attachment::new(pos, 0, false));
            }
            pos = pos.offset(0, 1, 0);
        }
        let bend = bend_length.sample(self.random());
        for _ in 0..=bend {
            self.log(pos);
            points.push(Attachment::new(pos, 0, false));
            pos = relative(pos, direction);
        }
        points
    }

    fn upwards_branching(
        &mut self,
        height: i32,
        origin: BlockPos,
        extra_branch_steps: &IntProvider,
        place_branch_per_log_probability: f32,
        extra_branch_length: &IntProvider,
    ) -> Vec<Attachment> {
        let mut attachments = Vec::new();
        for h in 0..height {
            let y = origin.y + h;
            let pos = BlockPos::new(origin.x, y, origin.z);
            if self.log(pos)
                && h < height - 1
                && self.random().next_f32() < place_branch_per_log_probability
            {
                let direction = random_horizontal(self.random());
                let length = extra_branch_length.sample(self.random());
                let start = 0.max(length - extra_branch_length.sample(self.random()) - 1);
                let steps = extra_branch_steps.sample(self.random());
                self.upwards_branch(height, &mut attachments, pos, y, direction, start, steps);
            }
            if h == height - 1 {
                attachments.push(Attachment::new(
                    BlockPos::new(origin.x, y + 1, origin.z),
                    0,
                    false,
                ));
            }
        }
        attachments
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the game's branch has this state"
    )]
    fn upwards_branch(
        &mut self,
        height: i32,
        attachments: &mut Vec<Attachment>,
        log: BlockPos,
        current_height: i32,
        direction: Face,
        start: i32,
        mut steps: i32,
    ) {
        let mut height_along = current_height + start;
        let (mut x, mut z) = (log.x, log.z);
        let mut index = start;
        while index < height && steps > 0 {
            if index >= 1 {
                let y = current_height + index;
                let moved = relative(BlockPos::new(x, y, z), direction);
                (x, z) = (moved.x, moved.z);
                height_along = y;
                let pos = BlockPos::new(x, y, z);
                if self.log(pos) {
                    height_along = y + 1;
                }
                attachments.push(Attachment::new(pos, 0, false));
            }
            index += 1;
            steps -= 1;
        }
        if height_along - current_height > 1 {
            let foliage = BlockPos::new(x, height_along, z);
            attachments.push(Attachment::new(foliage, 0, false));
            attachments.push(Attachment::new(foliage.offset(0, -2, 0), 0, false));
        }
    }

    fn cherry(
        &mut self,
        height: i32,
        origin: BlockPos,
        branch_count: &IntProvider,
        branch_horizontal_length: &IntProvider,
        (start_min, start_max): (i32, i32),
        branch_end_offset_from_top: &IntProvider,
    ) -> Vec<Attachment> {
        self.below(origin.offset(0, -1, 0));
        let first = 0.max(
            height - 1
                + crate::values::random_between_inclusive(self.random(), start_min, start_max),
        );
        let mut second = 0.max(
            height - 1
                + crate::values::random_between_inclusive(self.random(), start_min, start_max - 1),
        );
        if second >= first {
            second += 1;
        }
        let count = branch_count.sample(self.random());
        let middle = count == 3;
        let both_sides = count >= 2;
        let trunk_height = if middle {
            height
        } else if both_sides {
            first.max(second) + 1
        } else {
            first + 1
        };
        for y in 0..trunk_height {
            self.log(origin.offset(0, y, 0));
        }
        let mut attachments = Vec::new();
        if middle {
            attachments.push(Attachment::new(origin.offset(0, trunk_height, 0), 0, false));
        }
        let direction = super::common::random_horizontal(self.random());
        let sideways = Axis::of(direction);
        let branch = CherryBranch {
            height,
            origin,
            horizontal_length: branch_horizontal_length,
            end_offset_from_top: branch_end_offset_from_top,
            sideways,
        };
        attachments.push(self.cherry_branch(&branch, direction, first, first < trunk_height - 1));
        if both_sides {
            attachments.push(self.cherry_branch(
                &branch,
                direction.opposite(),
                second,
                second < trunk_height - 1,
            ));
        }
        attachments
    }

    fn cherry_branch(
        &mut self,
        branch: &CherryBranch<'_>,
        direction: Face,
        offset_from_origin: i32,
        middle_continues_upwards: bool,
    ) -> Attachment {
        let mut log = branch.origin.offset(0, offset_from_origin, 0);
        let end_y = branch.height - 1 + branch.end_offset_from_top.sample(self.random());
        let away = middle_continues_upwards || end_y < offset_from_origin;
        let distance_to_trunk = branch.horizontal_length.sample(self.random()) + i32::from(away);
        let end = relative_by(branch.origin, direction, distance_to_trunk).offset(0, end_y, 0);
        let steps_horizontally = if away { 2 } else { 1 };
        for _ in 0..steps_horizontally {
            log = relative(log, direction);
            self.log_along(log, branch.sideways);
        }
        let vertical = if end.y > log.y { Face::Up } else { Face::Down };
        loop {
            let distance = manhattan(log, end);
            if distance == 0 {
                return Attachment::new(end.offset(0, 1, 0), 0, false);
            }
            let chance = (end.y - log.y).abs() as f32 / distance as f32;
            let grow_vertically = self.random().next_f32() < chance;
            if grow_vertically {
                log = relative(log, vertical);
                self.log(log);
            } else {
                log = relative(log, direction);
                self.log_along(log, branch.sideways);
            }
        }
    }

    fn poplar(
        &mut self,
        height: i32,
        origin: BlockPos,
        trunk_height_above_branches: &IntProvider,
        branch_amount: &IntProvider,
    ) -> Vec<Attachment> {
        self.below(origin.offset(0, -1, 0));
        let up_to_branches = height - trunk_height_above_branches.sample(self.random());
        for y in 0..height {
            let log = origin.offset(0, y, 0);
            self.log(log);
            let mut directions = Face::ALL;
            shuffle(&mut directions, self.random());
            if up_to_branches - 1 != y {
                continue;
            }
            let branches = branch_amount.sample(self.random());
            for direction in directions
                .into_iter()
                .filter(|face| crate::blocks::is_horizontal(*face))
                .take(branches.max(0) as usize)
            {
                self.log_along(relative(log, direction), Axis::of(direction));
            }
        }
        vec![Attachment::new(
            origin.offset(0, up_to_branches, 0),
            0,
            false,
        )]
    }
}

/// What a cherry's two branches share.
struct CherryBranch<'p> {
    height: i32,
    origin: BlockPos,
    horizontal_length: &'p IntProvider,
    end_offset_from_top: &'p IntProvider,
    sideways: Axis,
}

/// `FancyTrunkPlacer.treeShape`: the radius of the crown `y` above the foot of a tree
/// `height` tall, or less than nought below it.
pub fn fancy_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = (radius * radius - adjacent * adjacent).sqrt();
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}

/// `FancyTrunkPlacer.getLogAxis`.
pub fn fancy_log_axis(start: BlockPos, pos: BlockPos) -> Axis {
    let x = (pos.x - start.x).abs();
    let z = (pos.z - start.z).abs();
    let most = x.max(z);
    if most == 0 {
        Axis::Y
    } else if x == most {
        Axis::X
    } else {
        Axis::Z
    }
}

/// `FancyTrunkPlacer.trimBranches`.
fn trim_fancy_branch(height: i32, local_y: i32) -> bool {
    f64::from(local_y) >= f64::from(height) * 0.2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fancy_crown_is_nothing_in_the_lower_third_and_a_half_circle_above() {
        assert_eq!(fancy_shape(12, 3), -1.0);
        assert_eq!(fancy_shape(12, 6), 3.0);
        assert_eq!(fancy_shape(12, 12), 0.0);
    }

    #[test]
    fn a_fancy_log_lies_along_the_longer_horizontal_step_and_x_on_a_tie() {
        let start = BlockPos::new(0, 0, 0);
        assert_eq!(fancy_log_axis(start, BlockPos::new(0, 3, 0)), Axis::Y);
        assert_eq!(fancy_log_axis(start, BlockPos::new(2, 3, 1)), Axis::X);
        assert_eq!(fancy_log_axis(start, BlockPos::new(1, 3, 2)), Axis::Z);
        assert_eq!(fancy_log_axis(start, BlockPos::new(2, 3, 2)), Axis::X);
    }
}
