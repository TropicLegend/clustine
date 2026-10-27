//! The composter's rule (`ComposterBlock`, n6 ComposterBlock.txt 159–175, 265–479;
//! `docs/groundwork/n10-plan.md`, section 6): what an item put in does to its level
//! (`insertItem`, `addLayer`), the bone meal it gives at level 8 (`extractProduce`,
//! `empty`), and its own tick from 7 to 8 (`onPlace`'s schedule and `tick`); and the
//! growth bone meal gives a crop (`CropBlock.performBonemeal`), which a farmer villager
//! uses in its tick.
//!
//! The rules are pure functions of a state and a stream, so that the farmer's behaviour
//! (`mobs/brain/behaviours/work.rs`, `WorkAtComposter` and `UseBonemeal`) draws them from
//! its own stream at the game's place and applies what they say by its set-block local,
//! as the game's level random would draw them inside its tick. The region keeps only the
//! composter's tick. **Not built, named**: a player's use of a composter and a hopper's
//! (`region/hoppers.rs` keeps it a container nothing goes into); they would read
//! [`insert_item`] and [`extract_produce`] too.

use clustine_blocks::data::{is_block, is_class, number, set};
use clustine_data::{BlockClass, BlockState, ITEMS, blocks, sounds};
use clustine_world::{BlockPos, Ticked};

use super::random_ticks::growth::{crop_max_age, crop_state_for_age};
use super::{Random, Region};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};

/// `ComposterBlock.READY`: the level at which it holds bone meal.
pub(crate) const READY: u8 = 8;
/// `ComposterBlock.MAX_LEVEL`: the level it is filled to by items.
pub(crate) const FULL: u8 = 7;
/// The ticks between a composter's filling and its being ready (`scheduleTick(…, 20)`).
const READYING_TICKS: i32 = 20;

/// `LevelEvent.COMPOSTER_FILL`: the particles and the sound of an item put in, its data 1
/// where the level rose.
pub(crate) const COMPOSTER_FILL: i32 = 1500;

/// The flags of a block set by these rules, for a mob's set-block local: every update
/// (`setBlockAndUpdate`), and the clients' only (a crop's growth).
pub(crate) const UPDATE_ALL: u32 = super::updates::UPDATE_ALL;
pub(crate) const UPDATE_CLIENTS: u32 = super::updates::UPDATE_CLIENTS;

/// Bone meal's sparks and their data (`region/bone_meal.rs`).
pub(crate) const PLANT_GROWTH: i32 = super::bone_meal::PLANT_GROWTH;
pub(crate) const PLANT_GROWTH_DATA: i32 = super::bone_meal::PLANT_GROWTH_DATA;

/// The chance in a hundred that an item adds a layer to a composter not empty: the
/// `layers` of its `compostable` component, a weighted list of 1 and 0 (the datagen's
/// `context_int_provider/compostable/*`): `low` 30, `low_medium` 50, `medium` 65,
/// `medium_high` 85; `always_add_one` adds one always and draws nothing (100 here).
/// Sorted by name, for a search.
const COMPOSTABLE: [(&str, u8); 122] = [
    ("minecraft:acacia_leaves", 30),
    ("minecraft:acacia_sapling", 30),
    ("minecraft:allium", 65),
    ("minecraft:apple", 65),
    ("minecraft:azalea", 65),
    ("minecraft:azalea_leaves", 30),
    ("minecraft:azure_bluet", 65),
    ("minecraft:baked_potato", 85),
    ("minecraft:beetroot", 65),
    ("minecraft:beetroot_seeds", 30),
    ("minecraft:big_dripleaf", 65),
    ("minecraft:birch_leaves", 30),
    ("minecraft:birch_sapling", 30),
    ("minecraft:blue_orchid", 65),
    ("minecraft:bread", 85),
    ("minecraft:brown_mushroom", 65),
    ("minecraft:brown_mushroom_block", 85),
    ("minecraft:bush", 30),
    ("minecraft:cactus", 50),
    ("minecraft:cactus_flower", 30),
    ("minecraft:cake", 100),
    ("minecraft:carrot", 65),
    ("minecraft:carved_pumpkin", 65),
    ("minecraft:cherry_leaves", 30),
    ("minecraft:cherry_sapling", 30),
    ("minecraft:closed_eyeblossom", 65),
    ("minecraft:cocoa_beans", 65),
    ("minecraft:cookie", 85),
    ("minecraft:cornflower", 65),
    ("minecraft:crimson_fungus", 65),
    ("minecraft:crimson_roots", 65),
    ("minecraft:dandelion", 65),
    ("minecraft:dark_oak_leaves", 30),
    ("minecraft:dark_oak_sapling", 30),
    ("minecraft:dried_kelp", 30),
    ("minecraft:dried_kelp_block", 50),
    ("minecraft:fern", 65),
    ("minecraft:firefly_bush", 30),
    ("minecraft:flowering_azalea", 85),
    ("minecraft:flowering_azalea_leaves", 50),
    ("minecraft:glow_berries", 30),
    ("minecraft:glow_lichen", 50),
    ("minecraft:hanging_roots", 30),
    ("minecraft:hay_block", 85),
    ("minecraft:jungle_leaves", 30),
    ("minecraft:jungle_sapling", 30),
    ("minecraft:kelp", 30),
    ("minecraft:large_fern", 65),
    ("minecraft:leaf_litter", 30),
    ("minecraft:lilac", 65),
    ("minecraft:lily_of_the_valley", 65),
    ("minecraft:lily_pad", 65),
    ("minecraft:mangrove_leaves", 30),
    ("minecraft:mangrove_propagule", 30),
    ("minecraft:mangrove_roots", 30),
    ("minecraft:melon", 65),
    ("minecraft:melon_seeds", 30),
    ("minecraft:melon_slice", 50),
    ("minecraft:moss_block", 65),
    ("minecraft:moss_carpet", 30),
    ("minecraft:mushroom_stem", 65),
    ("minecraft:nether_sprouts", 50),
    ("minecraft:nether_wart", 65),
    ("minecraft:nether_wart_block", 85),
    ("minecraft:oak_leaves", 30),
    ("minecraft:oak_sapling", 30),
    ("minecraft:open_eyeblossom", 65),
    ("minecraft:orange_poplar_leaves", 30),
    ("minecraft:orange_tulip", 65),
    ("minecraft:oxeye_daisy", 65),
    ("minecraft:pale_hanging_moss", 30),
    ("minecraft:pale_moss_block", 65),
    ("minecraft:pale_moss_carpet", 30),
    ("minecraft:pale_oak_leaves", 30),
    ("minecraft:pale_oak_sapling", 30),
    ("minecraft:peony", 65),
    ("minecraft:pink_petals", 30),
    ("minecraft:pink_tulip", 65),
    ("minecraft:pitcher_plant", 85),
    ("minecraft:pitcher_pod", 30),
    ("minecraft:poplar_sapling", 30),
    ("minecraft:poppy", 65),
    ("minecraft:potato", 65),
    ("minecraft:pumpkin", 65),
    ("minecraft:pumpkin_pie", 100),
    ("minecraft:pumpkin_seeds", 30),
    ("minecraft:red_mushroom", 65),
    ("minecraft:red_mushroom_block", 85),
    ("minecraft:red_poplar_leaves", 30),
    ("minecraft:red_shrub", 30),
    ("minecraft:red_tulip", 65),
    ("minecraft:rose_bush", 65),
    ("minecraft:sea_pickle", 65),
    ("minecraft:seagrass", 30),
    ("minecraft:shelf_mushroom", 65),
    ("minecraft:short_dry_grass", 30),
    ("minecraft:short_grass", 30),
    ("minecraft:shroomlight", 65),
    ("minecraft:small_dripleaf", 30),
    ("minecraft:spore_blossom", 65),
    ("minecraft:spruce_leaves", 30),
    ("minecraft:spruce_sapling", 30),
    ("minecraft:straw_bed", 85),
    ("minecraft:sugar_cane", 50),
    ("minecraft:sunflower", 65),
    ("minecraft:sweet_berries", 30),
    ("minecraft:tall_dry_grass", 30),
    ("minecraft:tall_grass", 50),
    ("minecraft:torchflower", 85),
    ("minecraft:torchflower_seeds", 30),
    ("minecraft:twisting_vines", 50),
    ("minecraft:vine", 50),
    ("minecraft:warped_fungus", 65),
    ("minecraft:warped_roots", 65),
    ("minecraft:warped_wart_block", 85),
    ("minecraft:weeping_vines", 50),
    ("minecraft:wheat", 65),
    ("minecraft:wheat_seeds", 30),
    ("minecraft:white_tulip", 65),
    ("minecraft:wildflowers", 30),
    ("minecraft:wither_rose", 65),
    ("minecraft:yellow_poplar_leaves", 30),
];

/// The chance in a hundred that the item `item` adds a layer (its `compostable`
/// component), none for an item that is not compostable.
pub(crate) fn compost_chance(item: i32) -> Option<u8> {
    let name = ITEMS.get(usize::try_from(item).ok()?)?.name;
    COMPOSTABLE
        .binary_search_by(|(known, _)| known.cmp(&name))
        .ok()
        .map(|at| COMPOSTABLE[at].1)
}

/// Whether `state` is a composter.
pub(crate) fn is_composter(state: BlockState) -> bool {
    is_block(state, blocks::COMPOSTER)
}

/// A composter's level, none for another block.
pub(crate) fn level(state: BlockState) -> Option<u8> {
    if is_composter(state) {
        number(state, "level")
    } else {
        None
    }
}

/// The composter `state` at the level `level`.
fn at_level(state: BlockState, level: u8) -> BlockState {
    set(state, "level", &level.to_string())
}

/// `ComposterBlock.addLayer`'s draw of the item's `layers` (a `number_dispatcher`): one
/// layer and no draw at level 0; else the weighted list's one `nextInt(100)`, a layer
/// below the item's chance; none and no draw for one that always adds a layer.
fn layers(state: BlockState, chance: u8, random: &mut Random) -> u8 {
    if level(state) == Some(0) || chance >= 100 {
        return 1;
    }
    u8::from(random.next_int(100) < u32::from(chance))
}

/// What an item put into a composter did (`ComposterBlock.insertItem`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Inserted {
    /// The composter after it: a level higher, or as it was.
    pub(crate) state: BlockState,
    /// Whether the item was used up (`stack.shrink(1)`): it is compostable and the level
    /// was below 7, whether or not it added a layer.
    pub(crate) used: bool,
}

/// `ComposterBlock.insertItem(entity, state, level, stack, pos)` of the composter `state`
/// and one of the item `item`, drawing from `random`: below level 7 and compostable, the
/// item is used and `addLayer` draws whether the level rises (by one, at most to 7).
/// The caller sets the new state with every update and its `block_change` game event
/// where it changed; the composter's own `onPlace` waits 20 ticks at 7 ([`Region::
/// composter_on_place`]).
pub(crate) fn insert_item(state: BlockState, item: i32, random: &mut Random) -> Inserted {
    let unchanged = Inserted { state, used: false };
    let Some(level) = level(state) else {
        return unchanged;
    };
    let Some(chance) = compost_chance(item) else {
        return unchanged;
    };
    if level >= FULL {
        return unchanged;
    }
    let added = layers(state, chance, random);
    let state = if added > 0 {
        at_level(state, (level + added).min(FULL))
    } else {
        state
    };
    Inserted { state, used: true }
}

/// What `ComposterBlock.extractProduce` does at a composter at `pos` that is ready,
/// drawing from `random`: where its bone meal is dropped
/// (`atLowerCornerWithOffset(0.5, 1.01, 0.5).offsetRandomXZ(random, 0.7)`, two
/// `nextFloat`s), and the empty composter (`empty`), set with every update, its
/// `block_change` game event and `composter_empty`'s sound at the block's centre.
pub(crate) fn extract_produce(
    state: BlockState,
    pos: BlockPos,
    random: &mut Random,
) -> ([f64; 3], BlockState) {
    let x = f64::from((random.next_float() - 0.5) * 0.7);
    let z = f64::from((random.next_float() - 0.5) * 0.7);
    let at = [
        f64::from(pos.x) + 0.5 + x,
        f64::from(pos.y) + 1.01,
        f64::from(pos.z) + 0.5 + z,
    ];
    (at, at_level(state, 0))
}

/// The sound of a composter emptied (`SoundEvents.COMPOSTER_EMPTY`).
pub(crate) const EMPTY_SOUND: i32 = sounds::BLOCK_COMPOSTER_EMPTY;

/// `CropBlock.performBonemeal` of the crop `state` that is not grown out
/// (`isValidBonemealTarget`), drawing from `random` as `growCrops` does: its age plus
/// `Mth.nextInt(2, 5)` (a third of it for beetroots, 1 and no draw for the torchflower
/// crop), at most its oldest; the state to set with flags 2. None for a block that is
/// not such a crop, which takes no bone meal and draws nothing.
pub(crate) fn bone_meal_crop(state: BlockState, random: &mut Random) -> Option<BlockState> {
    if !is_class(state, BlockClass::CropBlock) || is_class(state, BlockClass::PitcherCropBlock) {
        return None;
    }
    let age = number(state, "age")?;
    let most = crop_max_age(state);
    if age >= most {
        return None;
    }
    let increase = if is_class(state, BlockClass::TorchflowerCropBlock) {
        1
    } else {
        let drawn = 2 + random.next_int(4) as u8;
        if is_class(state, BlockClass::BeetrootBlock) {
            drawn / 3
        } else {
            drawn
        }
    };
    Some(crop_state_for_age(state, (age + increase).min(most)))
}

/// `CropBlock.isMaxAge(state)` of a crop (`CropBlock` and its kin), none for another
/// block.
pub(crate) fn crop_grown_out(state: BlockState) -> Option<bool> {
    if !is_class(state, BlockClass::CropBlock) {
        return None;
    }
    Some(number(state, "age").unwrap_or(0) >= crop_max_age(state))
}

impl Region {
    /// `ComposterBlock.onPlace`: a composter come to level 7 is ready 20 ticks on.
    pub(super) fn composter_on_place(&mut self, pos: BlockPos, state: BlockState) {
        if level(state) == Some(FULL) {
            let block = clustine_blocks::data::tick_id(state);
            self.schedule_tick(pos, Ticked::Block(block), READYING_TICKS, 0);
        }
    }

    /// `ComposterBlock.tick`: one at level 7 goes to 8 with every update and
    /// `composter_ready`'s sound at its centre. Whether the block was a composter.
    pub(super) fn composter_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        let Some(level) = level(state) else {
            return false;
        };
        if level == FULL {
            self.set_block(pos, at_level(state, READY), output);
            output.events.push(RegionEvent::Sound {
                sound: sounds::BLOCK_COMPOSTER_READY,
                source: SOUND_SOURCE_BLOCKS,
                position: clustine_world::Vec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        true
    }
}
