//! The random ticks of plants that need no light (step B7): leaves that decay, sugar
//! cane, cactus and its flower, kelp, cave vines, weeping and twisting vines, vines,
//! cocoa, nether wart, the chorus flower and a hanging mangrove propagule's age.
//!
//! Each rule is the game's `randomTick` of its class, with the same draws in the same
//! order, so that its chances are the game's. What a rule changes it sets with the
//! game's flags: `setBlockAndUpdate` is every update; flags 2 tell the clients and ask
//! the neighbours' shapes again but tell no neighbour; 260 (`UPDATE_INVISIBLE` and
//! `UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`) does the same without the clients, whom
//! Clustine tells of every change anyway.
//!
//! What the game does here that Clustine does not:
//!
//! - a mangrove propagule on the ground grows into a tree (`advanceTree`), which
//!   places a feature, and whose first stage is `SaplingBlock`'s: the propagule draws
//!   its chance as the game does and then does nothing;
//! - the chorus flower's sounds as it grows and dies (level events 1033 and 1034) have
//!   no event in Clustine yet;
//! - leaves drop nothing as they decay: the region has no items in the world.
//!
//! Two things that follow a growth are the shape updates' and not this file's, and
//! `clustine_blocks::update_shape` does not make them yet: the head of kelp or of
//! vines that has grown a new head stays a head where the game turns it into the
//! plant's body (`GrowingPlantHeadBlock.updateShape`), and a chorus plant does not
//! join the flower or the plant set beside it after it
//! (`ChorusPlantBlock.updateShape`).

use clustine_blocks::data::{
    HORIZONTAL, clockwise, counter_clockwise, face_name, flag, has_water, in_tag, is_block,
    is_class, number, relative, set, set_flag,
};
use clustine_blocks::survive::{can_survive, multiface_can_attach};
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::BlockPos;

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;
use crate::region::updates::{RegionView, UPDATE_CLIENTS, UPDATE_LIMIT};

/// `Block.UPDATE_INVISIBLE`: the clients are not told. Clustine tells them anyway.
const UPDATE_INVISIBLE: u32 = 4;
/// `Block.UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`, which nothing here has a block entity
/// for.
const UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
/// The flags with which sugar cane and cactus age: 260.
const AGE_FLAGS: u32 = UPDATE_INVISIBLE | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS;

/// The oldest a growing plant's head gets (`GrowingPlantHeadBlock.MAX_AGE`).
const HEAD_MAX_AGE: u8 = 25;

/// The six directions in the order of the game's `Direction.values()`, from which
/// `Direction.getRandom` draws.
const DIRECTIONS: [Face; 6] = [
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
    Face::West,
    Face::East,
];

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was.
    pub(super) fn plant_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if is_class(state, BlockClass::LeavesBlock) {
            self.leaves_random_tick(pos, state, output);
        } else if is_class(state, BlockClass::SugarCaneBlock) {
            self.sugar_cane_random_tick(pos, state, output);
        } else if is_class(state, BlockClass::CactusBlock) {
            self.cactus_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::GrowingPlantHeadBlock) {
            let Some(head) = Head::of(state) else {
                return false;
            };
            self.head_random_tick(pos, state, head, random, output);
        } else if is_class(state, BlockClass::VineBlock) {
            self.vine_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::CocoaBlock) {
            // The game draws from the level's random, which is the one it passes in,
            // and draws before it looks at the age.
            if random.next_int(5) == 0 {
                let age = number(state, "age").unwrap_or(2);
                if age < 2 {
                    let older = set(state, "age", &(age + 1).to_string());
                    self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                }
            }
        } else if is_class(state, BlockClass::NetherWartBlock) {
            let age = number(state, "age").unwrap_or(3);
            if age < 3 && random.next_int(10) == 0 {
                let older = set(state, "age", &(age + 1).to_string());
                self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
        } else if is_class(state, BlockClass::ChorusFlowerBlock) {
            self.chorus_flower_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::MangrovePropaguleBlock) {
            if flag(state, "hanging") {
                let age = number(state, "age").unwrap_or(4);
                if age != 4 {
                    let older = set(state, "age", &(age + 1).to_string());
                    self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                }
            } else {
                // `advanceTree` is not ported (see the top of this file); the chance is
                // drawn all the same, so that what follows in the stream draws as the
                // game's would.
                let _ = random.next_int(7);
            }
        } else {
            return false;
        }
        true
    }

    /// The block at `pos` as the game's rules read it: air outside the world, and
    /// where no chunk is loaded, which beside a chunk that ticks is only outside it.
    fn plant_read(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(blocks::AIR)
    }

    /// `isEmptyBlock`: whether air stands at `pos`.
    fn plant_empty(&self, pos: BlockPos) -> bool {
        self.plant_read(pos).props().is_air()
    }

    /// The lowest and the highest block of the world at `pos` (`getMinY`, `getMaxY`).
    fn plant_heights(&self, pos: BlockPos) -> Option<(i32, i32)> {
        let chunk = self.readable_chunk(pos.chunk())?;
        let height = i32::try_from(chunk.height()).ok()?;
        Some((chunk.min_y(), chunk.min_y() + height - 1))
    }

    /// `LeavesBlock.randomTick`: leaves that no log holds and that no player placed
    /// go, and leave their water. The game drops what they drop first.
    fn leaves_random_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let decaying = !flag(state, "persistent") && number(state, "distance") == Some(7);
        if decaying {
            // `removeBlock`: every update.
            self.set_block(pos, fluid_block(state), output);
        }
    }

    /// `SugarCaneBlock.randomTick`: a cane with air above and less than three canes
    /// tall ages, and at sixteen grows a cane above itself and starts again.
    fn sugar_cane_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let above = relative(pos, Face::Up);
        if !self.plant_empty(above) {
            return;
        }
        let mut height = 1;
        while is_block(self.plant_read(pos.offset(0, -height, 0)), state) {
            height += 1;
        }
        if height >= 3 {
            return;
        }
        let age = number(state, "age").unwrap_or(0);
        if age == 15 {
            self.set_block(above, default_of(state), output);
            let young = set(state, "age", "0");
            self.set_block_updated(pos, young, AGE_FLAGS, UPDATE_LIMIT, output);
        } else {
            let older = set(state, "age", &(age + 1).to_string());
            self.set_block_updated(pos, older, AGE_FLAGS, UPDATE_LIMIT, output);
        }
    }

    /// `CactusBlock.randomTick`: as sugar cane, and at age 8 a cactus with room for a
    /// cactus above may grow a flower there instead, more likely on a column of three.
    fn cactus_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let above = relative(pos, Face::Up);
        if !self.plant_empty(above) {
            return;
        }
        let age = number(state, "age").unwrap_or(0);
        let mut height = 1;
        while is_block(self.plant_read(pos.offset(0, -height, 0)), state) {
            height += 1;
            if height == 3 && age == 15 {
                return;
            }
        }
        if age == 8 && can_survive(default_of(state), above, &mut RegionView(self)) {
            let chance = if height >= 3 { 0.25 } else { 0.1 };
            if random.next_double() <= chance {
                self.set_block(above, blocks::CACTUS_FLOWER, output);
            }
        } else if age == 15 && height < 3 {
            self.set_block(above, default_of(state), output);
            let young = set(state, "age", "0");
            self.set_block_updated(pos, young, AGE_FLAGS, UPDATE_LIMIT, output);
            // The game then has the new cactus told that this one changed
            // (`neighborChanged`), which a cactus does nothing about.
        }
        if age < 15 {
            let older = set(state, "age", &(age + 1).to_string());
            self.set_block_updated(pos, older, AGE_FLAGS, UPDATE_LIMIT, output);
        }
    }

    /// `GrowingPlantHeadBlock.randomTick`: the head of kelp or of vines that is not
    /// yet at its oldest may grow a head one older into the next block its way.
    fn head_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        head: Head,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let age = number(state, "age").unwrap_or(HEAD_MAX_AGE);
        if age >= HEAD_MAX_AGE || random.next_double() >= head.chance {
            return;
        }
        let next = relative(pos, head.direction);
        let into = self.plant_read(next);
        let can_grow = if head.in_water {
            is_block(into, blocks::WATER)
        } else {
            into.props().is_air()
        };
        if !can_grow {
            return;
        }
        // `getGrowIntoState`: one older; cave vines draw whether it bears berries.
        let mut grown = set(state, "age", &(age + 1).to_string());
        if head.berries {
            grown = set_flag(grown, "berries", random.next_float() < 0.11);
        }
        self.set_block(next, grown, output);
    }

    /// `VineBlock.randomTick`: one time in four a vine spreads in a direction drawn at
    /// random: sideways onto a face beside it or round a corner, up onto the block
    /// above or into the air above, down into the air or the vine below. Sideways
    /// and up it spreads only where few vines are near.
    fn vine_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        // The game rule `spreadVines` is true unless changed, which Clustine does not
        // offer.
        if random.next_int(4) != 0 {
            return;
        }
        let direction = DIRECTIONS[random.next_int(6) as usize];
        let above = relative(pos, Face::Up);
        let horizontal = !matches!(direction, Face::Up | Face::Down);
        if horizontal && !flag(state, face_name(direction)) {
            if !self.vine_can_spread(pos, state) {
                return;
            }
            let target = relative(pos, direction);
            if self.plant_empty(target) {
                self.vine_spread_into_air(pos, state, direction, random, output);
            } else if vine_acceptable(self, target, direction) {
                let more = set_flag(state, face_name(direction), true);
                self.set_block_updated(pos, more, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            return;
        }
        let heights = self.plant_heights(pos);
        if direction == Face::Up && heights.is_some_and(|(_, max_y)| pos.y < max_y) {
            if self.vine_can_support_at_face(pos, state, direction) {
                let up = set_flag(state, "up", true);
                self.set_block_updated(pos, up, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                return;
            }
            if self.plant_empty(above) {
                if !self.vine_can_spread(pos, state) {
                    return;
                }
                let mut grown = state;
                for face in HORIZONTAL {
                    // `nextBoolean`, and only then whether the side holds.
                    if random.next_int(2) != 0
                        || !vine_acceptable(self, relative(above, face), face)
                    {
                        grown = set_flag(grown, face_name(face), false);
                    }
                }
                if has_horizontal_connection(grown) {
                    self.set_block_updated(above, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                }
                return;
            }
        }
        if heights.is_some_and(|(min_y, _)| pos.y > min_y) {
            let below = relative(pos, Face::Down);
            let under = self.plant_read(below);
            if under.props().is_air() || is_block(under, state) {
                let from = if under.props().is_air() {
                    default_of(state)
                } else {
                    under
                };
                let grown = copy_random_faces(state, from, random);
                if from != grown && has_horizontal_connection(grown) {
                    self.set_block_updated(below, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                }
            }
        }
    }

    /// A vine spreads sideways into the air towards `direction`: onto the face beside
    /// that a side of it already holds, round the corner of the block it holds, or,
    /// seldom, onto the bottom of the block above the air.
    fn vine_spread_into_air(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        direction: Face,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let target = relative(pos, direction);
        let clockwise = clockwise(direction);
        let counter = counter_clockwise(direction);
        let has_clockwise = flag(state, face_name(clockwise));
        let has_counter = flag(state, face_name(counter));
        let clockwise_pos = relative(target, clockwise);
        let counter_pos = relative(target, counter);
        let opposite = direction.opposite();
        let with = |face: Face| set_flag(default_of(state), face_name(face), true);
        let (at, grown) = if has_clockwise && vine_acceptable(self, clockwise_pos, clockwise) {
            (target, with(clockwise))
        } else if has_counter && vine_acceptable(self, counter_pos, counter) {
            (target, with(counter))
        } else if has_clockwise
            && self.plant_empty(clockwise_pos)
            && vine_acceptable(self, relative(pos, clockwise), opposite)
        {
            (clockwise_pos, with(opposite))
        } else if has_counter
            && self.plant_empty(counter_pos)
            && vine_acceptable(self, relative(pos, counter), opposite)
        {
            (counter_pos, with(opposite))
        } else if f64::from(random.next_float()) < 0.05
            && vine_acceptable(self, relative(target, Face::Up), Face::Up)
        {
            (target, with(Face::Up))
        } else {
            return;
        };
        self.set_block_updated(at, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
    }

    /// `VineBlock.canSpread`: at most four vines in the nine by three by nine blocks
    /// around, the vine itself among them.
    fn vine_can_spread(&self, pos: BlockPos, state: BlockState) -> bool {
        let mut found = 0;
        for x in -4..=4 {
            for y in -1..=1 {
                for z in -4..=4 {
                    if is_block(self.plant_read(pos.offset(x, y, z)), state) {
                        found += 1;
                        if found > 4 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// `VineBlock.canSupportAtFace`: whether the vine at `pos` can hold on at its side
    /// `face`: the block there holds it, or, sideways, the vine above holds on that
    /// side.
    fn vine_can_support_at_face(&self, pos: BlockPos, state: BlockState, face: Face) -> bool {
        if face == Face::Down {
            return false;
        }
        if vine_acceptable(self, relative(pos, face), face) {
            return true;
        }
        if face == Face::Up {
            return false;
        }
        let above = self.plant_read(relative(pos, Face::Up));
        is_block(above, state) && flag(above, face_name(face))
    }

    /// `ChorusFlowerBlock.randomTick`: a flower with air above grows up where little of
    /// its plant is below it, or else branches out sideways, or dies; it becomes the
    /// plant where it grew.
    fn chorus_flower_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let above = relative(pos, Face::Up);
        let in_world = self
            .plant_heights(pos)
            .is_some_and(|(_, max_y)| above.y <= max_y);
        if !self.plant_empty(above) || !in_world {
            return;
        }
        let age = number(state, "age").unwrap_or(5);
        if age >= 5 {
            return;
        }
        let mut grows_up = false;
        let mut end_stone_below = false;
        let below = self.plant_read(relative(pos, Face::Down));
        if in_tag(below, "minecraft:supports_chorus_flower") {
            grows_up = true;
        } else if is_block(below, blocks::CHORUS_PLANT) {
            let mut height = 1;
            for _ in 0..4 {
                let further = self.plant_read(pos.offset(0, -(height + 1), 0));
                if is_block(further, blocks::CHORUS_PLANT) {
                    height += 1;
                } else {
                    if in_tag(further, "minecraft:supports_chorus_flower") {
                        end_stone_below = true;
                    }
                    break;
                }
            }
            // Drawn only where the plant below is two or more tall.
            let bound = if end_stone_below { 5 } else { 4 };
            if height < 2 || height <= random.next_int(bound) as i32 {
                grows_up = true;
            }
        } else if below.props().is_air() {
            grows_up = true;
        }
        if grows_up
            && self.chorus_neighbours_empty(above, None)
            && self.plant_empty(pos.offset(0, 2, 0))
        {
            let plant = self.chorus_plant_at(pos);
            self.set_block_updated(pos, plant, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            self.place_chorus_flower(above, age, output);
        } else if age < 4 {
            let mut branches = random.next_int(4);
            if end_stone_below {
                branches += 1;
            }
            let mut placed = false;
            for _ in 0..branches {
                // `Direction.Plane.HORIZONTAL.getRandomDirection`.
                let direction = HORIZONTAL[random.next_int(4) as usize];
                let target = relative(pos, direction);
                if self.plant_empty(target)
                    && self.plant_empty(relative(target, Face::Down))
                    && self.chorus_neighbours_empty(target, Some(direction.opposite()))
                {
                    self.place_chorus_flower(target, age + 1, output);
                    placed = true;
                }
            }
            if placed {
                let plant = self.chorus_plant_at(pos);
                self.set_block_updated(pos, plant, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            } else {
                self.place_chorus_flower(pos, 5, output);
            }
        } else {
            self.place_chorus_flower(pos, 5, output);
        }
    }

    /// `placeGrownFlower` and `placeDeadFlower`: a flower of `age` at `pos`.
    fn place_chorus_flower(&mut self, pos: BlockPos, age: u8, output: &mut TickOutput) {
        let flower = set(blocks::CHORUS_FLOWER, "age", &age.to_string());
        self.set_block_updated(pos, flower, UPDATE_CLIENTS, UPDATE_LIMIT, output);
    }

    /// `ChorusFlowerBlock.allNeighborsEmpty`: air on every side of `pos` but `except`.
    fn chorus_neighbours_empty(&self, pos: BlockPos, except: Option<Face>) -> bool {
        HORIZONTAL
            .into_iter()
            .filter(|face| Some(*face) != except)
            .all(|face| self.plant_empty(relative(pos, face)))
    }

    /// `ChorusPlantBlock.getStateWithConnections` of a plant at `pos`: joined to the
    /// plants and flowers beside it, and below to what holds a plant.
    fn chorus_plant_at(&self, pos: BlockPos) -> BlockState {
        let joins = |state: BlockState| {
            is_block(state, blocks::CHORUS_PLANT) || is_block(state, blocks::CHORUS_FLOWER)
        };
        let mut plant = blocks::CHORUS_PLANT;
        for face in [
            Face::Down,
            Face::Up,
            Face::North,
            Face::East,
            Face::South,
            Face::West,
        ] {
            let neighbour = self.plant_read(relative(pos, face));
            let joined = joins(neighbour)
                || (face == Face::Down && in_tag(neighbour, "minecraft:supports_chorus_plant"));
            plant = set_flag(plant, face_name(face), joined);
        }
        plant
    }
}

/// What tells the heads of growing plants apart: their subclasses of
/// `GrowingPlantHeadBlock`.
#[derive(Debug, Clone, Copy)]
struct Head {
    /// The way the plant grows.
    direction: Face,
    /// `growPerTickProbability`.
    chance: f64,
    /// Whether it grows into water (kelp) rather than air (`canGrowInto`).
    in_water: bool,
    /// Whether what it grows may bear berries (cave vines).
    berries: bool,
}

impl Head {
    fn of(state: BlockState) -> Option<Self> {
        let head = |direction, chance, in_water, berries| Self {
            direction,
            chance,
            in_water,
            berries,
        };
        Some(if is_class(state, BlockClass::KelpBlock) {
            head(Face::Up, 0.14, true, false)
        } else if is_class(state, BlockClass::CaveVinesBlock) {
            head(Face::Down, 0.1, false, true)
        } else if is_class(state, BlockClass::WeepingVinesBlock) {
            head(Face::Down, 0.1, false, false)
        } else if is_class(state, BlockClass::TwistingVinesBlock) {
            head(Face::Up, 0.1, false, false)
        } else {
            return None;
        })
    }
}

/// `VineBlock.isAcceptableNeighbour`: whether the block at `at`, on a vine's side
/// `face`, holds the vine.
fn vine_acceptable(region: &Region, at: BlockPos, face: Face) -> bool {
    multiface_can_attach(region.plant_read(at), face)
}

/// `VineBlock.hasHorizontalConnection`.
fn has_horizontal_connection(state: BlockState) -> bool {
    HORIZONTAL
        .into_iter()
        .any(|face| flag(state, face_name(face)))
}

/// `VineBlock.copyRandomFaces`: `to` with each side of `from` added at even odds,
/// drawn for every side.
fn copy_random_faces(from: BlockState, mut to: BlockState, random: &mut Random) -> BlockState {
    for face in HORIZONTAL {
        // `nextBoolean`.
        if random.next_int(2) != 0 && flag(from, face_name(face)) {
            to = set_flag(to, face_name(face), true);
        }
    }
    to
}

/// The default state of the block of `state`.
fn default_of(state: BlockState) -> BlockState {
    state.block().map_or(state, |block| block.default_state)
}

/// `FluidState.createLegacyBlock`: what a block leaves when it goes, water where it held
/// a source of water and air otherwise.
fn fluid_block(state: BlockState) -> BlockState {
    let source = state.props().fluid().is_some_and(|fluid| fluid.source);
    if source && has_water(state) {
        blocks::WATER
    } else {
        blocks::AIR
    }
}

#[cfg(test)]
mod tests {
    //! The rules driven one random tick at a time, on a 3×3 of chunks around the
    //! origin with a stone floor at y = -61. Where a rule draws, many streams are
    //! tried, and what is asserted is what can and cannot happen, or a count within
    //! wide margins of the game's chance.

    use clustine_world::{Biome, Chunk, ChunkPos, EntityIds, Vec3};

    use super::*;
    use crate::api::Inventory;
    use crate::region::{Holdings, RegionConfig};

    const FLOOR: i32 = -61;

    fn at(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    /// The state the game would print as `text`.
    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
    }

    fn floor() -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the overworld");
        let mut chunk = Chunk::empty(overworld, Biome(0));
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, FLOOR, z, blocks::STONE);
            }
        }
        chunk
    }

    /// A region with the 3×3 of floors around the origin loaded, and `placed` put
    /// there without any update.
    fn region_with(placed: &[(BlockPos, BlockState)]) -> Region {
        let config = RegionConfig {
            spawn: Vec3::new(0.5, -60.0, 0.5),
            starting_inventory: Inventory::default(),
            return_after: 0,
            lowest_y: -64,
            seed: 0,
        };
        let ids = EntityIds::block(0).expect("the first block of entity ids");
        let mut region = Region::new(config, ids, Holdings::default());
        for x in -1..=1 {
            for z in -1..=1 {
                region.chunks.insert(ChunkPos::new(x, z), floor());
            }
        }
        for (pos, placed) in placed {
            put(&mut region, *pos, *placed);
        }
        region
    }

    /// Puts `state` at `pos` without any update, as a test sets the scene.
    fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
        let (x, z) = pos.in_chunk();
        region
            .chunks
            .get_mut(&pos.chunk())
            .expect("a loaded chunk")
            .set(x, pos.y, z, state);
    }

    fn read(region: &Region, pos: BlockPos) -> BlockState {
        region.block(pos).expect("a loaded block")
    }

    /// A stream of its own for each `seed`.
    fn stream(seed: u64) -> Random {
        Random::of_section(seed, ChunkPos::new(0, 0), 0, seed.wrapping_mul(31))
    }

    /// The random tick of what stands at `pos`, drawn from the stream of `seed`;
    /// whether it was a plant's.
    fn random_tick(region: &mut Region, pos: BlockPos, seed: u64) -> bool {
        let state = read(region, pos);
        let mut output = TickOutput::default();
        region.plant_random_tick(pos, state, &mut stream(seed), &mut output)
    }

    /// How many of the blocks from `pos` up are of the block of `of`.
    /// 1 if `of` stands right above the column of other blocks from `pos` up, or at
    /// `pos`: the head of a growing plant.
    fn column_from(region: &Region, pos: BlockPos, of: BlockState) -> i32 {
        let below = (0..)
            .take_while(|up| {
                let here = read(region, pos.offset(0, *up, 0));
                !is_block(here, of) && !here.props().is_air() && here != blocks::WATER
            })
            .count() as i32;
        i32::from(is_block(read(region, pos.offset(0, below, 0)), of))
    }

    fn column(region: &Region, pos: BlockPos, of: BlockState) -> i32 {
        (0..)
            .take_while(|up| is_block(read(region, pos.offset(0, *up, 0)), of))
            .count() as i32
    }

    /// A wall of stone just north of z = 0, for vines to hold on to.
    fn wall() -> Vec<(BlockPos, BlockState)> {
        let mut placed = Vec::new();
        for x in -6..=6 {
            for y in -60..=-45 {
                placed.push((at(x, y, -1), blocks::STONE));
            }
        }
        placed
    }

    #[test]
    fn a_decaying_leaf_goes_and_leaves_its_water() {
        let dry = at(0, -60, 0);
        let wet = at(2, -60, 0);
        let mut region = region_with(&[
            (
                dry,
                state("minecraft:oak_leaves[distance=7,persistent=false,waterlogged=false]"),
            ),
            (
                wet,
                state("minecraft:oak_leaves[distance=7,persistent=false,waterlogged=true]"),
            ),
        ]);
        assert!(random_tick(&mut region, dry, 1));
        assert!(random_tick(&mut region, wet, 2));
        assert_eq!(read(&region, dry), blocks::AIR);
        assert_eq!(read(&region, wet), blocks::WATER);
    }

    #[test]
    fn a_persistent_leaf_or_one_near_a_log_stays() {
        let kept = [
            state("minecraft:oak_leaves[distance=7,persistent=true,waterlogged=false]"),
            state("minecraft:oak_leaves[distance=6,persistent=false,waterlogged=false]"),
            state("minecraft:mangrove_leaves[distance=1,persistent=false,waterlogged=false]"),
        ];
        for (index, leaf) in kept.into_iter().enumerate() {
            let pos = at(0, -60, 0);
            let mut region = region_with(&[(pos, leaf)]);
            for draw in 0..20 {
                assert!(random_tick(&mut region, pos, index as u64 * 100 + draw));
            }
            assert_eq!(read(&region, pos), leaf);
        }
    }

    #[test]
    fn sugar_cane_grows_to_three_high_and_no_higher() {
        let base = at(0, -60, 0);
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::SAND),
            (at(1, FLOOR, 0), blocks::WATER),
            (base, blocks::SUGAR_CANE),
        ]);
        let mut seen = Vec::new();
        for seed in 0..200 {
            for up in 0..4 {
                let pos = base.offset(0, up, 0);
                if is_block(read(&region, pos), blocks::SUGAR_CANE) {
                    random_tick(&mut region, pos, seed);
                }
            }
            seen.push(column(&region, base, blocks::SUGAR_CANE));
        }
        // Sixteen ticks of the bottom cane grow the second.
        assert_eq!(seen[14], 1, "{seen:?}");
        assert_eq!(seen[15], 2, "{seen:?}");
        assert_eq!(seen.last(), Some(&3));
        assert!(seen.iter().all(|height| *height <= 3), "{seen:?}");
    }

    #[test]
    fn sugar_cane_under_a_block_does_not_age() {
        let pos = at(0, -60, 0);
        let cane = state("minecraft:sugar_cane[age=3]");
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::SAND),
            (at(1, FLOOR, 0), blocks::WATER),
            (pos, cane),
            (at(0, -59, 0), blocks::STONE),
        ]);
        assert!(random_tick(&mut region, pos, 0));
        assert_eq!(read(&region, pos), cane);
    }

    #[test]
    fn cactus_grows_to_three_high_and_no_higher() {
        let mut reached = 0;
        for trial in 0..20 {
            let base = at(0, -60, 0);
            let mut region =
                region_with(&[(at(0, FLOOR, 0), blocks::SAND), (base, blocks::CACTUS)]);
            for round in 0..300 {
                for up in 0..4 {
                    let pos = base.offset(0, up, 0);
                    if is_block(read(&region, pos), blocks::CACTUS) {
                        random_tick(&mut region, pos, trial * 1000 + round);
                    }
                }
                assert!(column(&region, base, blocks::CACTUS) <= 3);
            }
            let height = column(&region, base, blocks::CACTUS);
            let top = read(&region, base.offset(0, height, 0));
            assert!(
                top.props().is_air() || is_block(top, blocks::CACTUS_FLOWER),
                "{top:?}"
            );
            if height == 3 {
                reached += 1;
            }
        }
        // A flower on the first or the second cactus stops the column, one time in
        // ten for each: about a fifth of the columns.
        assert!((8..=20).contains(&reached), "{reached} reached three");
    }

    #[test]
    fn a_cactus_of_age_eight_flowers_one_time_in_ten_and_on_three_one_in_four() {
        let count = |height: i32| {
            let top = at(0, FLOOR + height, 0);
            let mut placed = vec![(at(0, FLOOR, 0), blocks::SAND)];
            for up in 1..height {
                placed.push((at(0, FLOOR + up, 0), blocks::CACTUS));
            }
            let mut region = region_with(&placed);
            let mut flowers = 0;
            for seed in 0..2000 {
                put(&mut region, top, state("minecraft:cactus[age=8]"));
                put(&mut region, top.offset(0, 1, 0), blocks::AIR);
                random_tick(&mut region, top, seed);
                // It ages whether it flowers or not.
                assert_eq!(read(&region, top), state("minecraft:cactus[age=9]"));
                if read(&region, top.offset(0, 1, 0)) == blocks::CACTUS_FLOWER {
                    flowers += 1;
                }
            }
            flowers
        };
        let alone = count(1);
        let on_three = count(3);
        assert!((120..=280).contains(&alone), "{alone} of 2000");
        assert!((400..=600).contains(&on_three), "{on_three} of 2000");
    }

    #[test]
    fn a_cactus_with_a_block_beside_where_it_would_flower_does_not_flower() {
        let pos = at(0, -60, 0);
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::SAND),
            (at(1, -59, 0), blocks::STONE),
        ]);
        for seed in 0..500 {
            put(&mut region, pos, state("minecraft:cactus[age=8]"));
            random_tick(&mut region, pos, seed);
            assert!(read(&region, pos.offset(0, 1, 0)).props().is_air());
        }
    }

    #[test]
    fn the_third_cactus_at_fifteen_neither_grows_nor_ages() {
        let top = at(0, -58, 0);
        let old = state("minecraft:cactus[age=15]");
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::SAND),
            (at(0, -60, 0), blocks::CACTUS),
            (at(0, -59, 0), blocks::CACTUS),
            (top, old),
        ]);
        for seed in 0..50 {
            random_tick(&mut region, top, seed);
        }
        assert_eq!(read(&region, top), old);
        assert!(read(&region, top.offset(0, 1, 0)).props().is_air());
    }

    #[test]
    fn kelp_grows_into_water_about_one_time_in_seven() {
        let pos = at(0, -60, 0);
        let above = at(0, -59, 0);
        let mut region = region_with(&[]);
        let mut grown = 0;
        for seed in 0..2000 {
            put(&mut region, pos, state("minecraft:kelp[age=3]"));
            put(&mut region, above, blocks::WATER);
            random_tick(&mut region, pos, seed);
            let now = read(&region, above);
            if now != blocks::WATER {
                assert_eq!(now, state("minecraft:kelp[age=4]"));
                grown += 1;
            }
        }
        assert!((180..=380).contains(&grown), "{grown} of 2000");
    }

    #[test]
    fn kelp_grows_only_into_water_and_not_past_its_oldest() {
        let pos = at(0, -60, 0);
        let above = at(0, -59, 0);
        let mut region = region_with(&[]);
        for seed in 0..300 {
            for (head, into) in [
                ("minecraft:kelp[age=3]", blocks::AIR),
                ("minecraft:kelp[age=3]", blocks::STONE),
                ("minecraft:kelp[age=25]", blocks::WATER),
            ] {
                put(&mut region, pos, state(head));
                put(&mut region, above, into);
                assert!(random_tick(&mut region, pos, seed));
                assert_eq!(read(&region, above), into, "{head} into {into:?}");
            }
        }
        let mut reached = false;
        for seed in 0..300 {
            put(&mut region, pos, state("minecraft:kelp[age=24]"));
            put(&mut region, above, blocks::WATER);
            random_tick(&mut region, pos, seed);
            reached |= read(&region, above) == state("minecraft:kelp[age=25]");
        }
        assert!(reached);
    }

    #[test]
    fn a_column_of_kelp_stops_at_the_top_of_the_water() {
        let base = at(0, -60, 0);
        let mut placed = vec![(base, state("minecraft:kelp[age=0]"))];
        for y in -59..=-55 {
            placed.push((at(0, y, 0), blocks::WATER));
        }
        let mut region = region_with(&placed);
        // Below its head a column of kelp is kelp plant, which the head's shape rule
        // makes it as the head grows on.
        let kelp_column = |region: &Region| {
            column(region, base, blocks::KELP_PLANT) + column_from(region, base, blocks::KELP)
        };
        for seed in 0..3000 {
            let top = base.offset(0, kelp_column(&region) - 1, 0);
            random_tick(&mut region, top, seed);
        }
        assert_eq!(kelp_column(&region), 6);
        assert_eq!(read(&region, at(0, -55, 0)), state("minecraft:kelp[age=5]"));
        assert!(read(&region, at(0, -54, 0)).props().is_air());
    }

    #[test]
    fn cave_vines_grow_down_and_bear_berries_about_one_time_in_nine() {
        let pos = at(0, -50, 0);
        let below = at(0, -51, 0);
        let mut region = region_with(&[(at(0, -49, 0), blocks::STONE)]);
        let (mut grown, mut berries) = (0, 0);
        for seed in 0..10_000 {
            put(
                &mut region,
                pos,
                state("minecraft:cave_vines[age=7,berries=false]"),
            );
            put(&mut region, below, blocks::AIR);
            random_tick(&mut region, pos, seed);
            assert_eq!(read(&region, at(0, -49, 0)), blocks::STONE);
            let now = read(&region, below);
            if now.props().is_air() {
                continue;
            }
            grown += 1;
            if now == state("minecraft:cave_vines[age=8,berries=true]") {
                berries += 1;
            } else {
                assert_eq!(now, state("minecraft:cave_vines[age=8,berries=false]"));
            }
        }
        assert!((800..=1200).contains(&grown), "{grown} of 10000");
        assert!((50..=200).contains(&berries), "{berries} of {grown}");
    }

    #[test]
    fn twisting_vines_grow_up_and_weeping_vines_down_into_air_only() {
        let pos = at(0, -55, 0);
        let (up, down) = (at(0, -54, 0), at(0, -56, 0));
        let mut region = region_with(&[]);
        let (mut twisted, mut wept) = (0, 0);
        for seed in 0..2000 {
            put(&mut region, pos, state("minecraft:twisting_vines[age=0]"));
            put(&mut region, up, blocks::AIR);
            put(&mut region, down, blocks::AIR);
            random_tick(&mut region, pos, seed);
            assert!(read(&region, down).props().is_air());
            if read(&region, up) == state("minecraft:twisting_vines[age=1]") {
                twisted += 1;
            }
            put(&mut region, pos, state("minecraft:weeping_vines[age=0]"));
            put(&mut region, up, blocks::AIR);
            put(&mut region, down, blocks::AIR);
            random_tick(&mut region, pos, seed);
            assert!(read(&region, up).props().is_air());
            if read(&region, down) == state("minecraft:weeping_vines[age=1]") {
                wept += 1;
            }
            // Water is no room for them.
            put(&mut region, pos, state("minecraft:weeping_vines[age=0]"));
            put(&mut region, down, blocks::WATER);
            random_tick(&mut region, pos, seed);
            assert_eq!(read(&region, down), blocks::WATER);
        }
        assert!((120..=280).contains(&twisted), "{twisted} of 2000");
        assert!((120..=280).contains(&wept), "{wept} of 2000");
    }

    #[test]
    fn a_vine_on_a_wall_spreads_along_it_and_down_holding_on_to_it() {
        let origin = at(0, -50, 0);
        let mut placed = wall();
        placed.push((origin, state("minecraft:vine[north=true]")));
        let mut region = region_with(&placed);
        for seed in 0..4000 {
            random_tick(&mut region, origin, seed);
        }
        let mut vines = Vec::new();
        for x in -6..=6 {
            for y in -60..=-44 {
                for z in -1..=1 {
                    let pos = at(x, y, z);
                    let found = read(&region, pos);
                    if is_block(found, blocks::VINE) {
                        vines.push((pos, found));
                    }
                }
            }
        }
        let has = |pos: BlockPos| vines.iter().any(|(at, _)| *at == pos);
        assert!(has(origin.offset(0, -1, 0)), "down: {vines:?}");
        assert!(has(origin.offset(0, 1, 0)), "up: {vines:?}");
        assert!(
            has(origin.offset(1, 0, 0)) || has(origin.offset(-1, 0, 0)),
            "sideways: {vines:?}"
        );
        for (pos, vine) in &vines {
            assert_eq!(pos.z, 0, "{pos:?} off the wall");
            assert!(flag(*vine, "north"), "{pos:?}: {vine:?}");
            for face in ["east", "south", "west", "up"] {
                assert!(!flag(*vine, face), "{pos:?}: {vine:?}");
            }
        }
        // Sideways and up only while at most four vines are near, the vine itself among
        // them: one each way, and one down.
        let near = vines
            .iter()
            .filter(|(pos, _)| (pos.y - origin.y).abs() <= 1 && (pos.x - origin.x).abs() <= 4)
            .count();
        assert_eq!(near, 5, "{vines:?}");
        assert_eq!(vines.len(), near, "{vines:?}");
    }

    #[test]
    fn a_vine_with_many_vines_near_spreads_neither_sideways_nor_up() {
        let mut placed = wall();
        let vine = state("minecraft:vine[north=true]");
        // Five vines in the box around the one ticked, itself among them, and stone
        // below all of them, so that nothing grows down.
        for x in [-2, -1, 0, 1, 2] {
            placed.push((at(x, -50, 0), vine));
            placed.push((at(x, -51, 0), blocks::STONE));
        }
        let mut region = region_with(&placed);
        for seed in 0..2000 {
            random_tick(&mut region, at(0, -50, 0), seed);
        }
        let expected = region_with(&placed);
        for x in -6..=6 {
            for y in -60..=-44 {
                for z in -1..=1 {
                    let pos = at(x, y, z);
                    assert_eq!(read(&region, pos), read(&expected, pos), "{pos:?}");
                }
            }
        }
    }

    #[test]
    fn a_vine_below_a_block_takes_hold_of_it() {
        let pos = at(0, -50, 0);
        let mut region = region_with(&[
            (at(0, -50, -1), blocks::STONE),
            (at(0, -49, 0), blocks::STONE),
            (at(0, -51, 0), blocks::STONE),
            (pos, state("minecraft:vine[north=true]")),
        ]);
        for seed in 0..500 {
            random_tick(&mut region, pos, seed);
        }
        assert_eq!(
            read(&region, pos),
            state("minecraft:vine[north=true,up=true]")
        );
    }

    #[test]
    fn cocoa_ages_to_two_about_one_time_in_five() {
        let pos = at(0, -60, 0);
        let young = state("minecraft:cocoa[age=0,facing=north]");
        let mut region = region_with(&[(at(0, -60, -1), blocks::JUNGLE_LOG), (pos, young)]);
        let mut aged = 0;
        for seed in 0..2000 {
            put(&mut region, pos, young);
            random_tick(&mut region, pos, seed);
            if read(&region, pos) != young {
                assert_eq!(
                    read(&region, pos),
                    state("minecraft:cocoa[age=1,facing=north]")
                );
                aged += 1;
            }
        }
        assert!((300..=500).contains(&aged), "{aged} of 2000");
        for seed in 0..200 {
            random_tick(&mut region, pos, seed);
        }
        assert_eq!(
            read(&region, pos),
            state("minecraft:cocoa[age=2,facing=north]")
        );
    }

    #[test]
    fn nether_wart_ages_to_three_about_one_time_in_ten() {
        let pos = at(0, -60, 0);
        let young = state("minecraft:nether_wart[age=0]");
        let mut region = region_with(&[(at(0, FLOOR, 0), blocks::SOUL_SAND), (pos, young)]);
        let mut aged = 0;
        for seed in 0..2000 {
            put(&mut region, pos, young);
            random_tick(&mut region, pos, seed);
            if read(&region, pos) != young {
                aged += 1;
            }
        }
        assert!((120..=280).contains(&aged), "{aged} of 2000");
        for seed in 0..500 {
            random_tick(&mut region, pos, seed);
        }
        assert_eq!(read(&region, pos), state("minecraft:nether_wart[age=3]"));
    }

    #[test]
    fn a_hanging_propagule_ages_to_four() {
        let pos = at(0, -50, 0);
        let young = "minecraft:mangrove_propagule[age=0,hanging=true,stage=0,waterlogged=false]";
        let old = "minecraft:mangrove_propagule[age=4,hanging=true,stage=0,waterlogged=false]";
        let mut region = region_with(&[
            (at(0, -49, 0), blocks::MANGROVE_LEAVES),
            (pos, state(young)),
        ]);
        for seed in 0..10 {
            assert!(random_tick(&mut region, pos, seed));
        }
        assert_eq!(read(&region, pos), state(old));
    }

    #[test]
    fn a_chorus_flower_on_end_stone_grows_up_and_leaves_its_plant() {
        let pos = at(0, -60, 0);
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::END_STONE),
            (pos, state("minecraft:chorus_flower[age=2]")),
        ]);
        assert!(random_tick(&mut region, pos, 0));
        let plant = read(&region, pos);
        assert!(is_block(plant, blocks::CHORUS_PLANT), "{plant:?}");
        assert!(flag(plant, "down"));
        assert_eq!(
            read(&region, pos.offset(0, 1, 0)),
            state("minecraft:chorus_flower[age=2]")
        );
    }

    #[test]
    fn a_dead_chorus_flower_does_nothing() {
        let pos = at(0, -60, 0);
        let dead = state("minecraft:chorus_flower[age=5]");
        let mut region = region_with(&[(at(0, FLOOR, 0), blocks::END_STONE), (pos, dead)]);
        for seed in 0..50 {
            assert!(random_tick(&mut region, pos, seed));
        }
        assert_eq!(read(&region, pos), dead);
        assert!(read(&region, pos.offset(0, 1, 0)).props().is_air());
    }

    #[test]
    fn an_old_chorus_flower_on_a_tall_plant_grows_up_one_time_in_five_or_dies() {
        // Four of plant on end stone, and the flower at age 4 on top.
        let top = at(0, -56, 0);
        let mut placed = vec![(at(0, FLOOR, 0), blocks::END_STONE)];
        for y in -60..=-57 {
            placed.push((at(0, y, 0), blocks::CHORUS_PLANT));
        }
        let mut region = region_with(&placed);
        let (mut up, mut dead) = (0, 0);
        for seed in 0..2000 {
            put(&mut region, top, state("minecraft:chorus_flower[age=4]"));
            put(&mut region, top.offset(0, 1, 0), blocks::AIR);
            random_tick(&mut region, top, seed);
            let now = read(&region, top);
            if is_block(now, blocks::CHORUS_PLANT) {
                assert_eq!(
                    read(&region, top.offset(0, 1, 0)),
                    state("minecraft:chorus_flower[age=4]")
                );
                up += 1;
            } else {
                assert_eq!(now, state("minecraft:chorus_flower[age=5]"));
                dead += 1;
            }
        }
        assert_eq!(up + dead, 2000);
        assert!((300..=500).contains(&up), "{up} of 2000");
    }

    #[test]
    fn a_young_chorus_flower_that_cannot_grow_up_branches_out() {
        // On two of plant on end stone, with a block two above: it cannot grow up,
        // and with end stone below it draws one to four branches, each of which finds
        // room on a side with air below.
        let pos = at(0, -58, 0);
        let mut region = region_with(&[
            (at(0, FLOOR, 0), blocks::END_STONE),
            (at(0, -60, 0), blocks::CHORUS_PLANT),
            (at(0, -59, 0), blocks::CHORUS_PLANT),
            (at(0, -56, 0), blocks::STONE),
        ]);
        let sides = HORIZONTAL.map(|face| relative(pos, face));
        for seed in 0..1000 {
            put(&mut region, pos, state("minecraft:chorus_flower[age=1]"));
            for side in sides {
                put(&mut region, side, blocks::AIR);
            }
            random_tick(&mut region, pos, seed);
            assert!(is_block(read(&region, pos), blocks::CHORUS_PLANT));
            let branches: Vec<BlockState> = sides
                .iter()
                .map(|side| read(&region, *side))
                .filter(|side| !side.props().is_air())
                .collect();
            assert!((1..=4).contains(&branches.len()), "{branches:?}");
            for branch in branches {
                assert_eq!(branch, state("minecraft:chorus_flower[age=2]"));
            }
        }
    }

    #[test]
    fn what_is_not_a_plant_is_left_to_the_other_rules() {
        let pos = at(0, -60, 0);
        let mut region = region_with(&[(pos, blocks::STONE)]);
        assert!(!random_tick(&mut region, pos, 0));
        put(&mut region, pos, state("minecraft:kelp_plant"));
        assert!(!random_tick(&mut region, pos, 0));
    }
}
