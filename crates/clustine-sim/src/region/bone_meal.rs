//! Bone meal (`docs/groundwork/n8-plan.md`, section 7.5; part D4b of N8): a player's use
//! of it on a block (`BoneMealItem.useOn`, n7) and a dispenser's (`DispenseItemBehavior$5`,
//! n6), both through `BoneMealItem.growCrop`, which asks the block's own
//! `isValidBonemealTarget`, draws its `isBonemealSuccess` and on success runs its
//! `performBonemeal`, each block's as the dumps have them.
//!
//! The game draws from the level's random. Clustine draws from the stream of the clicked
//! block in the tick (`Random::of_block` with a purpose of its own, kept on in the
//! region's block streams as drops keep theirs), or from the dispenser's own stream; where
//! the game hands that random to a feature (grass's flowers, a huge mushroom, moss), the
//! feature's random is seeded with the stream's next long at that moment (the plan's
//! 7.3). The draws are the game's in kind and order, the values not.
//!
//! A use on a block of a chunk the region does not tick does nothing, and uses no bone
//! meal. What a feature sets beyond the region's land goes as `SetIf` effects through the
//! region's view (`feature_view.rs`).
//!
//! Named and left: bone meal under water (`growWaterPlant`: seagrass and coral) and on
//! the blocks whose classes are not among the dumps (kelp and the other growing plants,
//! vines, dripleaf, nylium, rooted dirt, flowers that spread, …), where the use passes as
//! on any block; the arm's swing that the game's `SUCCESS_SERVER` shows the other
//! players. A sapling's and the azalea's tree are `trees.rs`'s (`TreeGrower`).

use clustine_blocks::data::{flag, is_block, is_class, number, relative, set};
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockClass, BlockState, Face, blocks, items};
use clustine_world::{BlockPos, PlayerId};

use super::dispensers::level_event;
use super::random_ticks::growth::{crop_max_age, crop_state_for_age};
use super::trees::TreeGrower;
use super::updates::{RegionView, UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use super::{Random, Region};
use crate::api::TickOutput;
use crate::placement::Click;

/// Mixed into a block's stream for a player's bone meal used on it.
const BONE_MEAL: u64 = 0x626f_6e65_6d65_616c;

/// `LevelEvent.PARTICLES_AND_SOUND_PLANT_GROWTH`: the green sparks and the sound of
/// bone meal, with the count 15 as its data.
pub(crate) const PLANT_GROWTH: i32 = 1505;

/// The data the game sends with [`PLANT_GROWTH`] for bone meal.
pub(crate) const PLANT_GROWTH_DATA: i32 = 15;

/// The flags a feature of bone meal sets its blocks with where the game's
/// `Feature.setBlock` does (3): a huge mushroom's stem and cap.
const FEATURE_FLAGS: u32 = UPDATE_ALL;

/// The flags of the plant features grass's bone meal and moss place (a simple block, a
/// patch of vegetation), which set with 2 in the game (**Inferred**: those features are
/// not among the dumps).
const PLANT_FEATURE_FLAGS: u32 = UPDATE_CLIENTS;

/// A block that takes bone meal (`BonemealableBlock`), by the rules it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `CropBlock` and its kin: wheat, carrots, potatoes, beetroots, the torchflower crop.
    Crop,
    Pitcher,
    Stem,
    Berries,
    Cocoa,
    Bamboo,
    Shoot,
    Sapling,
    Propagule,
    Azalea,
    Grass,
    /// `TallGrassBlock`: short grass and the fern.
    TallGrass,
    Mushroom,
    /// `BonemealableFeaturePlacerBlock`: moss and pale moss.
    Moss,
}

impl Target {
    fn of(state: BlockState) -> Option<Self> {
        // Subclasses before the classes they extend: the propagule is a sapling.
        let classes = [
            (BlockClass::PitcherCropBlock, Target::Pitcher),
            (BlockClass::CropBlock, Target::Crop),
            (BlockClass::StemBlock, Target::Stem),
            (BlockClass::SweetBerryBushBlock, Target::Berries),
            (BlockClass::CocoaBlock, Target::Cocoa),
            (BlockClass::BambooStalkBlock, Target::Bamboo),
            (BlockClass::BambooSaplingBlock, Target::Shoot),
            (BlockClass::MangrovePropaguleBlock, Target::Propagule),
            (BlockClass::SaplingBlock, Target::Sapling),
            (BlockClass::AzaleaBlock, Target::Azalea),
            (BlockClass::GrassBlock, Target::Grass),
            (BlockClass::TallGrassBlock, Target::TallGrass),
            (BlockClass::MushroomBlock, Target::Mushroom),
            (BlockClass::BonemealableFeaturePlacerBlock, Target::Moss),
        ];
        classes
            .into_iter()
            .find(|(class, _)| is_class(state, *class))
            .map(|(_, target)| target)
    }
}

impl Region {
    /// `BoneMealItem.useOn` of a click of `player`'s with bone meal on a block that did
    /// nothing with it: where the clicked block takes it (`growCrop`), the
    /// `item_interact_finish` the item's use effects post at the player, then the level
    /// event of the sparks at the block. A creative player's stack is not used up, as
    /// the game puts back what a creative player's use took
    /// (`ServerPlayerGameMode.useItemOn`). Whether it took the click.
    pub(crate) fn bone_meal_used_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held != Some(items::BONE_MEAL) {
            return false;
        }
        let pos = click.position;
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.config.seed, pos, self.tick, BONE_MEAL));
        let grew = self.grow_crop(pos, &mut random, output);
        self.block_streams.insert(pos, random);
        if !grew {
            // `growWaterPlant` is not built (named in the module).
            return false;
        }
        if let Some(own) = self.players.get(&player) {
            let at = own.pose.position;
            let context = Context::of(Some(Source::of(own.entity_id)), None);
            if let Some(event) = Vibration::named("minecraft:item_interact_finish") {
                self.vibrate(event, [at.x, at.y, at.z], context);
            }
        }
        level_event(PLANT_GROWTH, pos, PLANT_GROWTH_DATA, output);
        true
    }

    /// `BoneMealItem.growCrop` at `pos`, drawing from `random`: whether the block there
    /// took the bone meal (`isValidBonemealTarget`), which then is used up whether or
    /// not it grew. Nothing in a chunk the region does not tick.
    pub(crate) fn grow_crop(
        &mut self,
        pos: BlockPos,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if !self.ticks_chunk(pos.chunk()) {
            return false;
        }
        let Some(state) = self.own_block(pos) else {
            return false;
        };
        let Some(target) = Target::of(state) else {
            return false;
        };
        if !self.bone_meal_valid(target, pos, state, random) {
            return false;
        }
        if self.bone_meal_success(target, state, random) {
            self.perform_bone_meal(target, pos, state, random, output);
        }
        true
    }

    /// `isValidBonemealTarget` of the block `state` at `pos`.
    fn bone_meal_valid(
        &mut self,
        target: Target,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
    ) -> bool {
        let above = relative(pos, Face::Up);
        match target {
            // `!isMaxAge(state)`.
            Target::Crop => number(state, "age").unwrap_or(u8::MAX) < crop_max_age(state),
            // `canGrow` of the lower half at its age plus one.
            Target::Pitcher => self.pitcher_lower(pos, state).is_some_and(|(lower, at)| {
                let age = number(lower, "age").unwrap_or(4);
                self.pitcher_can_grow(at, lower, age.saturating_add(1))
            }),
            Target::Stem => number(state, "age") != Some(7),
            Target::Berries => number(state, "age").is_some_and(|age| age < 3),
            // n8 CocoaBlock 184–196.
            Target::Cocoa => number(state, "age").is_some_and(|age| age < 2),
            Target::Bamboo => {
                let up = self.bamboo_above(pos);
                let down = self.bamboo_below(pos);
                let top = pos.offset(0, up, 0);
                let free = top.offset(0, 1, 0);
                up + down + 1 < 16
                    && number(self.read_or_air(top), "stage") != Some(1)
                    && self.inside_build_height(free)
                    && self.read_or_air(free).props().is_air()
            }
            Target::Shoot => {
                self.read_or_air(above).props().is_air() && self.inside_build_height(above)
            }
            // `TreeGrower.canGrow`, then the shortest tree's base height inside the world.
            Target::Sapling => {
                self.tree_grower_can_grow(pos, state, random)
                    && self.inside_build_height(pos.offset(0, tree_minimum_height(state), 0))
            }
            // The propagule asks no grower: one on the ground is always a target, and a
            // hanging one until it is grown (n8 MangrovePropaguleBlock 246–258).
            Target::Propagule => !flag(state, "hanging") || number(state, "age") != Some(4),
            // The azalea grower's shortest tree plus two inside the world, and no fluid
            // above (n8 AzaleaBlock).
            Target::Azalea => {
                let height = tree_minimum_height(state);
                self.inside_build_height(pos.offset(0, height + 2, 0))
                    && self.read_or_air(above).props().fluid().is_none()
            }
            Target::Grass => {
                self.read_or_air(above).props().is_air() && self.inside_build_height(above)
            }
            // **Inferred** (`TallGrassBlock` is not among the dumps; as SteelMC's port of
            // 26.3 has it): the tall plant's lower half could stand here, with air inside
            // the world above.
            Target::TallGrass => {
                let lower = set(tall_of(state), "half", "lower");
                self.inside_build_height(above)
                    && can_survive(lower, pos, &mut RegionView(self))
                    && self.read_or_air(above).props().is_air()
            }
            // The feature is in the registry, and the mushroom's top, four plus its
            // foliage's radius above, inside the world.
            Target::Mushroom => match huge_mushroom_of(state) {
                Some((feature, radius)) => {
                    self.has_feature(feature)
                        && self.inside_build_height(pos.offset(0, 4 + radius, 0))
                }
                None => false,
            },
            // **Inferred** (`BonemealableFeaturePlacerBlock` is not among the dumps; as
            // `MossBlock` had it): air above.
            Target::Moss => self.read_or_air(above).props().is_air(),
        }
    }

    /// `isBonemealSuccess`: a sapling, the propagule on the ground and the azalea at
    /// 0.45, a mushroom at 0.4, the rest always, drawing nothing.
    fn bone_meal_success(
        &mut self,
        target: Target,
        state: BlockState,
        random: &mut Random,
    ) -> bool {
        match target {
            Target::Sapling | Target::Azalea => f64::from(random.next_float()) < 0.45,
            Target::Propagule if !flag(state, "hanging") => f64::from(random.next_float()) < 0.45,
            Target::Mushroom => f64::from(random.next_float()) < 0.4,
            _ => true,
        }
    }

    /// `performBonemeal` of the block `state` at `pos`.
    fn perform_bone_meal(
        &mut self,
        target: Target,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        match target {
            Target::Crop => {
                // `growCrops`: the age plus `getBonemealAgeIncrease`, never past the
                // oldest, with flags 2.
                let age = number(state, "age").unwrap_or(0);
                let older = (age + crop_increase(state, random)).min(crop_max_age(state));
                let grown = crop_state_for_age(state, older);
                self.set_block_updated(pos, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            Target::Pitcher => {
                if let Some((lower, at)) = self.pitcher_lower(pos, state) {
                    self.pitcher_grow(at, lower, output);
                }
            }
            Target::Stem => {
                // `min(7, age + nextInt(2, 5))` with flags 2, and at 7 the stem's random
                // tick at once, which may set its fruit.
                let age = number(state, "age").unwrap_or(7);
                let older = (age + mth_next_int(random, 2, 5)).min(7);
                let grown = set(state, "age", &older.to_string());
                self.set_block_updated(pos, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                if older == 7 {
                    self.stem_random_tick(pos, grown, random, output);
                }
            }
            // `min(3, age + 1)`, and cocoa's age plus one, each with flags 2.
            Target::Berries | Target::Cocoa => {
                let age = number(state, "age").unwrap_or(3);
                let grown = set(state, "age", &(age + 1).min(3).to_string());
                self.set_block_updated(pos, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            Target::Bamboo => self.bamboo_bone_meal(pos, random, output),
            Target::Shoot => {
                // `BambooSaplingBlock.growBamboo`: bamboo with small leaves above, with
                // every update.
                let stalk = set(default_of(blocks::BAMBOO), "leaves", "small");
                self.set_block(relative(pos, Face::Up), stalk, output);
            }
            Target::Sapling | Target::Azalea => self.advance_tree(pos, state, random, output),
            Target::Propagule => {
                if flag(state, "hanging") {
                    // `cycle(AGE)` with flags 2.
                    let age = number(state, "age").unwrap_or(0);
                    let grown = set(state, "age", &((age + 1) % 5).to_string());
                    self.set_block_updated(pos, grown, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                } else {
                    self.advance_tree(pos, state, random, output);
                }
            }
            Target::Grass => self.grass_bone_meal(pos, random, output),
            Target::TallGrass => place_tall(self, pos, state, output),
            Target::Mushroom => self.grow_mushroom(pos, state, random, output),
            Target::Moss => {
                if let Some(feature) = moss_feature_of(state) {
                    let seed = random.next_u64();
                    self.place_feature(
                        feature,
                        relative(pos, Face::Up),
                        PLANT_FEATURE_FLAGS,
                        seed,
                        output,
                    );
                }
            }
        }
    }

    /// The pitcher crop's lower half (`getLowerHalf`): `state` at `pos` where it is the
    /// lower, else the block below where that is the crop's lower half.
    fn pitcher_lower(&self, pos: BlockPos, state: BlockState) -> Option<(BlockState, BlockPos)> {
        let lower = |state: BlockState| {
            is_block(state, blocks::PITCHER_CROP) && state.property("half") == Some("lower")
        };
        if lower(state) {
            return Some((state, pos));
        }
        let below = pos.offset(0, -1, 0);
        let there = self.read_or_air(below);
        lower(there).then_some((there, below))
    }

    /// `getHeightAboveUpToMax`: how many stalks stand right above `pos`, up to 16.
    fn bamboo_above(&self, pos: BlockPos) -> i32 {
        (1..=16)
            .take_while(|up| is_block(self.read_or_air(pos.offset(0, *up, 0)), blocks::BAMBOO))
            .count() as i32
    }

    /// `BambooStalkBlock.performBonemeal` (n8 382–453): one or two new stalks on top
    /// (`1 + nextInt(2)`), each by `growBamboo` from the topmost, while the stalk stays
    /// below 16, the top is not grown out and there is air inside the world above it.
    fn bamboo_bone_meal(&mut self, pos: BlockPos, random: &mut Random, output: &mut TickOutput) {
        let above = self.bamboo_above(pos);
        let down = self.bamboo_below(pos);
        let stalks = 1 + random.next_int(2) as i32;
        for grown in 0..stalks {
            let up = above + grown;
            let height = above + down + 1 + grown;
            let top = pos.offset(0, up, 0);
            let state = self.read_or_air(top);
            let free = top.offset(0, 1, 0);
            if height >= 16
                || number(state, "stage") == Some(1)
                || !self.read_or_air(free).props().is_air()
                || !self.inside_build_height(free)
            {
                return;
            }
            self.grow_bamboo(top, state, random, height, output);
        }
    }

    /// `GrassBlock.performBonemeal` (n8 GrassBlock 41–125): 128 walks from the block
    /// above; the walk `i` takes `i / 16` steps, each one across x, y by
    /// `nextIntBetweenInclusive(-1, 1) * nextInt(3) / 2` and one across z, drawn in that
    /// order, and is given up where it stands off grass or in a full block; where it ends,
    /// the bone meal's effect (`placeBonemealEffect`).
    fn grass_bone_meal(&mut self, pos: BlockPos, random: &mut Random, output: &mut TickOutput) {
        let start = relative(pos, Face::Up);
        'walks: for walk in 0..128 {
            let mut at = start;
            for _ in 0..walk / 16 {
                let dx = random.next_int_between(-1, 1);
                // Java's division, toward nought.
                let dy = random.next_int_between(-1, 1) * random.next_int(3) as i32 / 2;
                let dz = random.next_int_between(-1, 1);
                at = at.offset(dx, dy, dz);
                let below = self.read_or_air(at.offset(0, -1, 0));
                if !is_block(below, blocks::GRASS_BLOCK)
                    || self.read_or_air(at).props().collision_is_full_block()
                {
                    continue 'walks;
                }
            }
            self.grass_bone_meal_effect(at, random, output);
        }
    }

    /// `GrassBlock.placeBonemealEffect` (n8 GrassBlock 117–229) at `at`: short grass there
    /// grows tall one time in ten; else where it is air inside the world, one time in
    /// eight one of the biome's bone meal features drawn, and otherwise the feature
    /// `grass_bonemeal` (short grass where there is air), each placed by a random seeded
    /// from the stream there.
    fn grass_bone_meal_effect(
        &mut self,
        at: BlockPos,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let here = self.read_or_air(at);
        if is_block(here, blocks::SHORT_GRASS)
            && random.next_float() < 0.1
            && self.bone_meal_valid(Target::TallGrass, at, here, random)
        {
            place_tall(self, at, here, output);
        }
        if !here.props().is_air() || !self.inside_build_height(at) {
            return;
        }
        if random.next_float() < 0.125 {
            let features = match (&self.config.features.0, self.biome_at(at)) {
                (Some(settings), Some(biome)) => settings.bone_meal_features(biome),
                _ => Vec::new(),
            };
            if features.is_empty() {
                return;
            }
            // `Util.getRandom(list, random)`.
            let len = u32::try_from(features.len()).unwrap_or(u32::MAX);
            let feature = features[random.next_int(len) as usize];
            let seed = random.next_u64();
            self.place_feature(feature, at, PLANT_FEATURE_FLAGS, seed, output);
        } else if self.has_feature(GRASS_BONEMEAL) {
            // The placed feature `grass_bonemeal` is the configured `grass` behind a
            // filter of air, which the place is.
            let seed = random.next_u64();
            self.place_feature(GRASS_BONEMEAL, at, PLANT_FEATURE_FLAGS, seed, output);
        }
    }

    /// `MushroomBlock.growMushroom` (n8 MushroomBlock 176–218): the mushroom removed
    /// (`removeBlock`, flags 3), its huge mushroom's feature placed where it stood, and
    /// where that fails the mushroom put back (`setBlockAndUpdate`).
    fn grow_mushroom(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let Some((feature, _)) = huge_mushroom_of(state) else {
            return;
        };
        if !self.has_feature(feature) {
            return;
        }
        self.set_block(pos, super::updates::fluid_block(state), output);
        let seed = random.next_u64();
        if !self.place_feature(feature, pos, FEATURE_FLAGS, seed, output) {
            self.set_block(pos, state, output);
        }
    }

    /// The block at `pos`, air where the region cannot read it.
    fn read_or_air(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(blocks::AIR)
    }

    /// `TreeGrower.canGrow(level, pos, state)` of the sapling `state`'s grower at `pos`,
    /// which draws its tree and its mega tree from `random` and wants a 2×2 where it has
    /// only mega trees.
    fn tree_grower_can_grow(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
    ) -> bool {
        TreeGrower::of(state).is_some_and(|grower| self.can_grow(grower, pos, state, random))
    }
}

/// `TreeGrower.getMinimumHeight` of the grower of the sapling or azalea `state`: the base
/// height of its shortest tree's trunk, nought where it names none (dark and pale oak).
fn tree_minimum_height(state: BlockState) -> i32 {
    TreeGrower::of(state)
        .and_then(TreeGrower::minimum_height)
        .unwrap_or(0)
}

/// The configured feature `grass_bonemeal` places.
const GRASS_BONEMEAL: &str = "minecraft:grass";

/// `getBonemealAgeIncrease` of the crop `state`: `Mth.nextInt(2, 5)`, a third of it for
/// beetroots, and 1 for the torchflower crop, which draws nothing.
fn crop_increase(state: BlockState, random: &mut Random) -> u8 {
    if is_class(state, BlockClass::TorchflowerCropBlock) {
        1
    } else if is_class(state, BlockClass::BeetrootBlock) {
        mth_next_int(random, 2, 5) / 3
    } else {
        mth_next_int(random, 2, 5)
    }
}

/// `Mth.nextInt(random, low, high)`: `nextInt(high - low + 1) + low`.
fn mth_next_int(random: &mut Random, low: u8, high: u8) -> u8 {
    let span = u32::from(high - low) + 1;
    low + random.next_int(span) as u8
}

/// The tall plant short grass or a fern grows into.
fn tall_of(state: BlockState) -> BlockState {
    if is_block(state, blocks::FERN) {
        blocks::LARGE_FERN
    } else {
        blocks::TALL_GRASS
    }
}

/// `DoublePlantBlock.placeAt(level, tall, pos, 2)` for short grass or a fern `state` at
/// `pos`: the lower half there and the upper above, each with flags 2.
fn place_tall(region: &mut Region, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
    let tall = tall_of(state);
    let lower = set(tall, "half", "lower");
    let upper = set(tall, "half", "upper");
    region.set_block_updated(pos, lower, UPDATE_CLIENTS, UPDATE_LIMIT, output);
    region.set_block_updated(
        relative(pos, Face::Up),
        upper,
        UPDATE_CLIENTS,
        UPDATE_LIMIT,
        output,
    );
}

/// The huge mushroom's configured feature of the mushroom `state`, and its foliage's
/// radius (`AbstractHugeMushroomFeature.foliageRadius`: 3 in `huge_brown_mushroom`'s
/// data, the default 2 in `huge_red_mushroom`'s).
fn huge_mushroom_of(state: BlockState) -> Option<(&'static str, i32)> {
    if is_block(state, blocks::BROWN_MUSHROOM) {
        Some(("minecraft:huge_brown_mushroom", 3))
    } else if is_block(state, blocks::RED_MUSHROOM) {
        Some(("minecraft:huge_red_mushroom", 2))
    } else {
        None
    }
}

/// The configured feature a moss block places on itself (**Inferred**, as `MossBlock`
/// placed `moss_patch_bonemeal`).
fn moss_feature_of(state: BlockState) -> Option<&'static str> {
    if is_block(state, blocks::MOSS_BLOCK) {
        Some("minecraft:moss_patch_bonemeal")
    } else if is_block(state, blocks::PALE_MOSS_BLOCK) {
        Some("minecraft:pale_moss_patch_bonemeal")
    } else {
        None
    }
}

/// The default state of the block of `state`.
fn default_of(state: BlockState) -> BlockState {
    state.block().map_or(state, |block| block.default_state)
}
