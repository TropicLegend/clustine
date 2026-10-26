//! The random ticks of what grows by light (`docs/groundwork/n8-plan.md`, section 6.2):
//! crops and stems, the pitcher crop, sweet berries, beetroot and the torchflower, grass
//! and mycelium, bamboo and its shoot, mushrooms, and saplings and the propagule on the
//! ground, whose trees are `trees.rs`'s.
//!
//! Each rule is the game's `randomTick` of its class, with the same draws in the same
//! order, drawn before or after the light is read as the game draws them, so that a
//! plant in the dark takes from the stream what the game's would. The light is the
//! region's (`light.rs`): a chunk that ticks is lit with its eight neighbours, so every
//! read here has an answer; were one missing, the rule takes it as dark and does not
//! grow.
//!
//! What a rule sets beyond the region's land (a grass block's spread, a mushroom's, a
//! stem's fruit) becomes a conditional set for whoever holds it, expecting what the rule
//! read there (`set_block_updated`; `docs/adr/0025-effects-between-regions.md`, section
//! 4). A stem turns to its fruit at once whether or not the holder then refuses the
//! fruit; its next shape update from that side turns it back (`clustine_blocks::plants`).

use clustine_blocks::data::{
    HORIZONTAL, face_name, flag, fluid_in_tag, in_tag, is_block, is_class, number, relative, set,
    set_flag,
};
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::BlockPos;

use crate::api::TickOutput;
use crate::region::updates::{RegionView, UPDATE_CLIENTS, UPDATE_LIMIT};
use crate::region::vibrations::{Context, Vibration};
use crate::region::{Random, Region};

/// The tallest a stalk of bamboo grows by itself (`BambooStalkBlock.MAX_HEIGHT`).
const BAMBOO_MAX_HEIGHT: i32 = 16;

impl Region {
    /// The random tick of `state` at `pos` if it is one of these rules', and whether it
    /// was.
    pub(in crate::region) fn growth_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if is_class(state, BlockClass::MangrovePropaguleBlock) {
            if flag(state, "hanging") {
                return false;
            }
            // `MangrovePropaguleBlock.randomTick` (n8 216–245): one try in seven, and no
            // light read, so a propagule on the ground grows in the dark too.
            if random.next_int(7) == 0 {
                self.advance_tree(pos, state, random, output);
            }
        } else if crate::region::trees::is_sapling(state) {
            // `SaplingBlock.randomTick` (n8 SaplingBlock 39–89): the light above, darkened
            // by the night, is read before the chance is drawn.
            let light = self
                .max_local_raw_brightness(relative(pos, Face::Up))
                .unwrap_or(0);
            if light >= 9 && random.next_int(7) == 0 {
                self.advance_tree(pos, state, random, output);
            }
        } else if is_class(state, BlockClass::BeetrootBlock)
            || is_class(state, BlockClass::TorchflowerCropBlock)
        {
            // `BeetrootBlock.randomTick`, `TorchflowerCropBlock.randomTick`: one in three
            // skips, drawn before the light is read.
            if random.next_int(3) != 0 {
                self.crop_random_tick(pos, state, random, output);
            }
        } else if is_class(state, BlockClass::CropBlock) {
            self.crop_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::PitcherCropBlock) {
            self.pitcher_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::StemBlock) {
            self.stem_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::SweetBerryBushBlock) {
            self.berries_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::BambooStalkBlock) {
            self.bamboo_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::BambooSaplingBlock) {
            self.shoot_random_tick(pos, random, output);
        } else if is_class(state, BlockClass::MushroomBlock) {
            self.mushroom_random_tick(pos, state, random, output);
        } else if is_class(state, BlockClass::SpreadingSnowyBlock) {
            self.spreading_random_tick(pos, state, random, output);
        } else {
            return false;
        }
        true
    }

    /// The block at `pos` as these rules read it: air where no chunk is readable, which
    /// beside a chunk that ticks is only outside the world.
    fn grown_read(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(blocks::AIR)
    }

    /// `getRawBrightness(pos, 0)`, dark where there is no answer.
    fn raw_light(&self, pos: BlockPos) -> u8 {
        self.raw_brightness(pos, 0).unwrap_or(0)
    }

    /// `CropBlock.randomTick` (n7 CropBlock 100–140): in light of 9 or more, below its
    /// oldest, a crop ages by one at the chance its ground gives, with flags 2.
    fn crop_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if self.raw_light(pos) < 9 {
            return;
        }
        let max_age = crop_max_age(state);
        let age = number(state, "age").unwrap_or(max_age);
        if age >= max_age {
            return;
        }
        let speed = self.growth_speed(state, pos);
        if random.next_int(growth_bound(speed)) == 0 {
            let older = crop_state_for_age(state, age + 1);
            self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
    }

    /// `CropBlock.getGrowthSpeed(block, level, pos)` (n7 CropBlock 174–326): one, and for
    /// each block of the 3×3 of ground below that grows crops one (three if moist), a
    /// quarter of it off the centre; halved where plants of the same block stand in a row
    /// both ways beside it, or else on a diagonal.
    pub(in crate::region) fn growth_speed(&self, plant: BlockState, pos: BlockPos) -> f32 {
        let mut speed = 1.0_f32;
        let below = pos.offset(0, -1, 0);
        for dx in -1..=1 {
            for dz in -1..=1 {
                let ground = self.grown_read(below.offset(dx, 0, dz));
                let mut here = 0.0_f32;
                if in_tag(ground, "minecraft:grows_crops") {
                    here = 1.0;
                    if number(ground, "moisture").unwrap_or(0) > 0 {
                        here = 3.0;
                    }
                }
                if dx != 0 || dz != 0 {
                    here /= 4.0;
                }
                speed += here;
            }
        }
        let same = |at: BlockPos| is_block(self.grown_read(at), plant);
        let north = pos.offset(0, 0, -1);
        let south = pos.offset(0, 0, 1);
        let west = pos.offset(-1, 0, 0);
        let east = pos.offset(1, 0, 0);
        let row = same(west) || same(east);
        let column = same(north) || same(south);
        // The diagonals are read only where the rows are not both there, as the game's.
        let crowded = row && column
            || same(west.offset(0, 0, -1))
            || same(east.offset(0, 0, -1))
            || same(east.offset(0, 0, 1))
            || same(west.offset(0, 0, 1));
        if crowded {
            speed /= 2.0;
        }
        speed
    }

    /// `PitcherCropBlock.randomTick` (n8 PitcherCropBlock 219–375): the chance is drawn
    /// first; then `grow` by one where `canGrow`: not grown out, light of 8 or more, the
    /// block above inside the world, and from age 3 air or the crop above. The lower half
    /// takes its age with flags 2, and from age 3 the upper half is set above with every
    /// update.
    fn pitcher_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let speed = self.growth_speed(state, pos);
        if random.next_int(growth_bound(speed)) != 0 {
            return;
        }
        self.pitcher_grow(pos, state, output);
    }

    /// `PitcherCropBlock.grow(level, state, pos, 1)` of the lower half `state` at `pos`:
    /// by one where `canGrow`, the lower half with flags 2, and from age 3 the upper half
    /// above with every update. Bone meal grows it so too.
    pub(in crate::region) fn pitcher_grow(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let age = number(state, "age").unwrap_or(PITCHER_MAX_AGE);
        let grown_age = (age + 1).min(PITCHER_MAX_AGE);
        if !self.pitcher_can_grow(pos, state, grown_age) {
            return;
        }
        let grown = set(state, "age", &grown_age.to_string());
        self.set_block_updated(pos, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        if grown_age >= PITCHER_DOUBLE_FROM {
            let above = relative(pos, Face::Up);
            self.set_block(above, set(grown, "half", "upper"), output);
        }
    }

    /// `PitcherCropBlock.canGrow(level, pos, state, age)` of the lower half `state` at
    /// `pos` growing to `grown_age`: not grown out, light of 8 or more, the block above
    /// inside the world, and from age 3 air or the crop above.
    pub(in crate::region) fn pitcher_can_grow(
        &self,
        pos: BlockPos,
        state: BlockState,
        grown_age: u8,
    ) -> bool {
        let age = number(state, "age").unwrap_or(PITCHER_MAX_AGE);
        let above = relative(pos, Face::Up);
        age < PITCHER_MAX_AGE
            && self.raw_light(pos) >= 8
            && self.inside_build_height(above)
            && (grown_age < PITCHER_DOUBLE_FROM || {
                let there = self.grown_read(above);
                there.props().is_air() || is_block(there, blocks::PITCHER_CROP)
            })
    }

    /// `isInsideBuildHeight`: whether `pos` lies between the world's lowest and highest
    /// blocks.
    pub(in crate::region) fn inside_build_height(&self, pos: BlockPos) -> bool {
        self.readable_chunk(pos.chunk()).is_some_and(|chunk| {
            let height = i32::try_from(chunk.height()).unwrap_or(0);
            pos.y >= chunk.min_y() && pos.y < chunk.min_y() + height
        })
    }

    /// `StemBlock.randomTick` (n8 StemBlock 71–256): in light of 9 or more, at the chance
    /// its ground gives, a stem below age 7 ages with flags 2; one of age 7 draws a side,
    /// and where that side is air above ground its fruit may stand on, sets the fruit there
    /// and then turns to the stem attached to it, both with every update.
    pub(in crate::region) fn stem_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if self.raw_light(pos) < 9 {
            return;
        }
        let speed = self.growth_speed(state, pos);
        if random.next_int(growth_bound(speed)) != 0 {
            return;
        }
        let age = number(state, "age").unwrap_or(7);
        if age < 7 {
            let older = set(state, "age", &(age + 1).to_string());
            self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            return;
        }
        // `Direction.Plane.HORIZONTAL.getRandomDirection`: north, east, south, west.
        let direction = HORIZONTAL[random.next_int(4) as usize];
        let Some((fruit, attached, ground_tag)) = stem_parts(state) else {
            return;
        };
        let target = relative(pos, direction);
        let ground = self.grown_read(relative(target, Face::Down));
        if self.grown_read(target).props().is_air() && in_tag(ground, ground_tag) {
            self.set_block(target, default_of(fruit), output);
            let attached = set(default_of(attached), "facing", face_name(direction));
            self.set_block(pos, attached, output);
        }
    }

    /// `SweetBerryBushBlock.randomTick` (n7 SweetBerryBushBlock 72–110): below age 3, one
    /// try in five, drawn before the light above the bush is read; in light of 9 or more
    /// it ages with flags 2 and posts `block_change`.
    fn berries_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let age = number(state, "age").unwrap_or(3);
        if age >= 3 || random.next_int(5) != 0 {
            return;
        }
        if self.raw_light(relative(pos, Face::Up)) < 9 {
            return;
        }
        let older = set(state, "age", &(age + 1).to_string());
        self.set_block_updated(pos, older, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        if let Some(vibration) = Vibration::named("minecraft:block_change") {
            self.vibrate_at(vibration, pos, Context::of(None, Some(older)));
        }
    }

    /// `BambooStalkBlock.randomTick` (n8 BambooStalkBlock 227–270): a stalk of stage 0,
    /// one try in three, with air above in light of 9 or more and fewer than 15 stalks
    /// below it, grows (`growBamboo`).
    fn bamboo_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if number(state, "stage") != Some(0) {
            return;
        }
        if random.next_int(3) != 0 {
            return;
        }
        let above = relative(pos, Face::Up);
        if !self.grown_read(above).props().is_air() || self.raw_light(above) < 9 {
            return;
        }
        let height = self.bamboo_below(pos) + 1;
        if height < BAMBOO_MAX_HEIGHT {
            self.grow_bamboo(pos, state, random, height, output);
        }
    }

    /// `getHeightBelowUpToMax`: how many stalks stand right below `pos`, up to 16.
    pub(in crate::region) fn bamboo_below(&self, pos: BlockPos) -> i32 {
        (1..=BAMBOO_MAX_HEIGHT)
            .take_while(|down| is_block(self.grown_read(pos.offset(0, -down, 0)), blocks::BAMBOO))
            .count() as i32
    }

    /// `BambooStalkBlock.growBamboo` (n8 BambooStalkBlock 454–572): the new stalk above
    /// `pos` takes small leaves over a stalk without leaves (or over no stalk) and large
    /// ones over a stalk with leaves, which then, with a stalk below it, takes small leaves
    /// and that one none; it is of age 1 where this one is or two stalks stand below, and
    /// of stage 1, grown out, at height 15, or from height 11 at one chance in four,
    /// drawn only there. As the game, the leaves are read and rewritten below `pos`, not
    /// below the new stalk.
    pub(in crate::region) fn grow_bamboo(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        height: i32,
        output: &mut TickOutput,
    ) {
        let below = self.grown_read(pos.offset(0, -1, 0));
        let two_below_pos = pos.offset(0, -2, 0);
        let two_below = self.grown_read(two_below_pos);
        let mut leaves = "none";
        if height >= 1 {
            let below_is_bamboo = is_block(below, blocks::BAMBOO);
            if !below_is_bamboo || below.property("leaves") == Some("none") {
                leaves = "small";
            } else {
                leaves = "large";
                if is_block(two_below, blocks::BAMBOO) {
                    self.set_block(pos.offset(0, -1, 0), set(below, "leaves", "small"), output);
                    self.set_block(two_below_pos, set(two_below, "leaves", "none"), output);
                }
            }
        }
        let age = if number(state, "age") == Some(1) || is_block(two_below, blocks::BAMBOO) {
            "1"
        } else {
            "0"
        };
        let grown_out = (height >= 11 && random.next_float() < 0.25) || height == 15;
        let stalk = set(default_of(state), "age", age);
        let stalk = set(stalk, "leaves", leaves);
        let stalk = set(stalk, "stage", if grown_out { "1" } else { "0" });
        self.set_block(relative(pos, Face::Up), stalk, output);
    }

    /// `BambooSaplingBlock.randomTick` (n8 BambooSaplingBlock 21–44): one try in three,
    /// with air above in light of 9 or more, bamboo with small leaves grows above, with
    /// every update; its shape update then makes the shoot bamboo
    /// (`clustine_blocks::plants::bamboo_shoot`).
    fn shoot_random_tick(&mut self, pos: BlockPos, random: &mut Random, output: &mut TickOutput) {
        if random.next_int(3) != 0 {
            return;
        }
        let above = relative(pos, Face::Up);
        if !self.grown_read(above).props().is_air() || self.raw_light(above) < 9 {
            return;
        }
        let stalk = set(default_of(blocks::BAMBOO), "leaves", "small");
        self.set_block(above, stalk, output);
    }

    /// `MushroomBlock.randomTick` (n8 MushroomBlock 26–137): one try in 25; where at most
    /// four of its block stand in the 9×3×9 about it, a walk from it: a target drawn one
    /// across, four times stepped onto where it is air and the mushroom could stay, the
    /// next target drawn from where the walk stands whether or not it stepped; at the
    /// last target, if it is air where the mushroom could stay, the mushroom's own state
    /// with flags 2.
    fn mushroom_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if random.next_int(25) != 0 {
            return;
        }
        if !self.mushroom_can_spread(pos, state) {
            return;
        }
        let mut standing = pos;
        let mut target = mushroom_step(standing, random);
        for _ in 0..4 {
            if self.mushroom_fits(state, target) {
                standing = target;
            }
            target = mushroom_step(standing, random);
        }
        if self.mushroom_fits(state, target) {
            self.set_block_updated(target, state, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
    }

    /// `MushroomBlock.canSpread`: `atMostMatched(4)` of the mushroom's block in the box
    /// from (−4, −1, −4) to (4, 1, 4) about `pos`, itself counted.
    fn mushroom_can_spread(&self, pos: BlockPos, state: BlockState) -> bool {
        let mut found = 0;
        for dx in -4..=4 {
            for dy in -1..=1 {
                for dz in -4..=4 {
                    if is_block(self.grown_read(pos.offset(dx, dy, dz)), state) {
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

    /// `isEmptyBlock(target) && state.canSurvive(level, target)`: on a block that
    /// overrides its need of the dark, or in light below 13 on a block of solid render.
    fn mushroom_fits(&mut self, state: BlockState, target: BlockPos) -> bool {
        self.grown_read(target).props().is_air()
            && can_survive(state, target, &mut RegionView(self))
    }

    /// `SpreadingSnowyBlock.randomTick` (n8 SpreadingSnowyBlock 15–167): a grass block or
    /// mycelium that cannot stay alive becomes dirt with every update and draws nothing;
    /// one that can, where the light above it darkened by the night is 9 or more, tries
    /// four times a target one across and three down to one up, drawn x, y, z, and turns
    /// dirt there that could stay alive with no water above into its own block, snowy
    /// where the block above is snow, with every update. No light is read at the target.
    fn spreading_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        // Both are given the dirt block's key (n6 Blocks): what they die to and spread to.
        let base = default_of(blocks::DIRT);
        if !self.can_stay_alive(state, pos) {
            self.set_block(pos, base, output);
            return;
        }
        let light = self
            .max_local_raw_brightness(relative(pos, Face::Up))
            .unwrap_or(0);
        if light < 9 {
            return;
        }
        let spreading = default_of(state);
        for _ in 0..4 {
            let dx = random.next_int(3) as i32 - 1;
            let dy = random.next_int(5) as i32 - 3;
            let dz = random.next_int(3) as i32 - 1;
            let target = pos.offset(dx, dy, dz);
            if is_block(self.grown_read(target), base) && self.can_propagate(spreading, target) {
                let snowy = in_tag(
                    self.grown_read(relative(target, Face::Up)),
                    "minecraft:snow",
                );
                self.set_block(target, set_flag(spreading, "snowy", snowy), output);
            }
        }
    }

    /// `SpreadingSnowyBlock.canStayAlive`: under a single layer of snow, yes; under a full
    /// fluid, no; else whether light coming down into it from the block above would keep
    /// any of its level (`LightEngine.getLightDampeningInto` below 15): a full block or a
    /// bottom slab above kills it, a top slab does not.
    fn can_stay_alive(&self, state: BlockState, pos: BlockPos) -> bool {
        let above = self.grown_read(relative(pos, Face::Up));
        if is_block(above, blocks::SNOW) && number(above, "layers") == Some(1) {
            return true;
        }
        if above.props().fluid().is_some_and(|fluid| fluid.amount == 8) {
            return false;
        }
        let dampening = above.props().light_dampening();
        clustine_light::light_dampening_into(state, above, Face::Up, dampening) < 15
    }

    /// `SpreadingSnowyBlock.canPropagate`: the spreading block's default state could stay
    /// alive at `pos`, and no water stands above it.
    fn can_propagate(&self, spreading: BlockState, pos: BlockPos) -> bool {
        self.can_stay_alive(spreading, pos)
            && !fluid_in_tag(self.grown_read(relative(pos, Face::Up)), "minecraft:water")
    }
}

/// The oldest the pitcher crop gets (`PitcherCropBlock.MAX_AGE`).
const PITCHER_MAX_AGE: u8 = 4;

/// The age from which the pitcher crop stands two blocks tall
/// (`DOUBLE_PLANT_AGE_INTERSECTION`).
const PITCHER_DOUBLE_FROM: u8 = 3;

/// The age at which a crop is grown out by its class (`getMaxAge`): 2 for the torchflower
/// crop, whose own ages are 0 and 1 and whose age 2 is the torchflower (n7
/// TorchflowerCropBlock 45–66), 3 for beetroots, 7 for the rest.
pub(in crate::region) fn crop_max_age(state: BlockState) -> u8 {
    if is_class(state, BlockClass::TorchflowerCropBlock) {
        2
    } else if is_class(state, BlockClass::BeetrootBlock) {
        3
    } else {
        7
    }
}

/// `getStateForAge(age)` of the crop `state`: the torchflower crop at age 2 is the
/// torchflower (n7 TorchflowerCropBlock 55–66), any other its own block of that age.
pub(in crate::region) fn crop_state_for_age(state: BlockState, age: u8) -> BlockState {
    if is_class(state, BlockClass::TorchflowerCropBlock) && age >= 2 {
        return blocks::TORCHFLOWER;
    }
    set(default_of(state), "age", &age.to_string())
}

/// The bound of a growth's draw, `(int) (25.0F / speed) + 1`.
fn growth_bound(speed: f32) -> u32 {
    (25.0_f32 / speed) as u32 + 1
}

/// A stem's fruit, the stem attached to it and the tag of the ground the fruit may stand
/// on (n6 Blocks: the `StemBlock`s' fruit, `attachedStem` and `fruitSupportBlocks`).
fn stem_parts(stem: BlockState) -> Option<(BlockState, BlockState, &'static str)> {
    Some(match stem.block()?.name {
        "minecraft:melon_stem" => (
            blocks::MELON,
            blocks::ATTACHED_MELON_STEM,
            "minecraft:supports_melon_stem_fruit",
        ),
        "minecraft:pumpkin_stem" => (
            blocks::PUMPKIN,
            blocks::ATTACHED_PUMPKIN_STEM,
            "minecraft:supports_pumpkin_stem_fruit",
        ),
        _ => return None,
    })
}

/// The next target of a mushroom's walk from `from`: x by `nextInt(3) - 1`, y by
/// `nextInt(2) - nextInt(2)`, z by `nextInt(3) - 1`, drawn in that order.
fn mushroom_step(from: BlockPos, random: &mut Random) -> BlockPos {
    let dx = random.next_int(3) as i32 - 1;
    let up = random.next_int(2) as i32;
    let down = random.next_int(2) as i32;
    let dz = random.next_int(3) as i32 - 1;
    from.offset(dx, up - down, dz)
}

/// The default state of the block of `state`.
fn default_of(state: BlockState) -> BlockState {
    state.block().map_or(state, |block| block.default_state)
}
