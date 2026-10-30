// Adapted from SteelMC's `steel-core/src/behavior/items/bucket.rs` and
// `items/place_on_water_block_item.rs`.

//! The use of a bucket or a lily pad, which aims by a ray from the player's eyes
//! rather than at a clicked block (section 5).

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, ItemClass, blocks, items};
use clustine_noise::trig;
use clustine_world::{ARMOUR_START, BlockPos, Vec3};

use super::geometry::{self, DirExt};
use super::{Blocks, Click, ItemUse, UsedItem, data};

use crate::api::ItemStack;

/// How far a creative-mode player reaches with an item, vanilla's block interaction
/// range.
const REACH: f64 = 5.0;

/// Where a ray hit: the block, the side it entered by, and the point.
struct Hit {
    pos: BlockPos,
    face: Dir,
    location: [f64; 3],
}

/// The use of a bucket or of a block placed on water.
pub(crate) fn use_item(
    item_use: &ItemUse,
    blocks: &dyn Blocks,
    bodies: &[[f64; 6]],
) -> Option<super::UsedItem> {
    let held = item_use.held?;
    let item = data::item(held)?;
    match (item.class, held) {
        (ItemClass::BucketItem, items::BUCKET) => empty_bucket(item_use, blocks),
        (ItemClass::BucketItem, items::WATER_BUCKET) => {
            filled_bucket(item_use, blocks, blocks::WATER)
        }
        (ItemClass::BucketItem, items::LAVA_BUCKET) => {
            filled_bucket(item_use, blocks, blocks::LAVA)
        }
        (ItemClass::PlaceOnWaterBlockItem, _) => on_water(item_use, blocks, bodies),
        _ => None,
    }
}

/// An empty bucket takes the source it is aimed at, the water of a waterlogged block,
/// or powder snow.
fn empty_bucket(item_use: &ItemUse, blocks: &dyn Blocks) -> Option<UsedItem> {
    let hit = ray(item_use, blocks, Fluids::Sources).ok()??;
    let state = blocks.block(hit.pos)?;
    let (taken, filled) = if state.props().is_liquid() {
        let fluid = state.props().fluid()?;
        if !fluid.source {
            return None;
        }
        let filled = if data::has_lava(state) {
            items::LAVA_BUCKET
        } else {
            items::WATER_BUCKET
        };
        (blocks::AIR, filled)
    } else if data::flag(state, "waterlogged") {
        (
            data::set_flag(state, "waterlogged", false),
            items::WATER_BUCKET,
        )
    } else if data::same_block(state, blocks::POWDER_SNOW) {
        (blocks::AIR, items::POWDER_SNOW_BUCKET)
    } else {
        return None;
    };
    // In creative mode the bucket in hand stays empty, and the filled one is added to
    // the inventory if no slot of it holds one (`ItemUtils.createFilledResult`): into
    // the first empty slot of the hotbar and the main inventory, which is where the
    // game's `Inventory.add` puts a stack of one that can have nothing added to it.
    let has_one = item_use
        .inventory
        .0
        .iter()
        .flatten()
        .any(|stack| stack.item == filled);
    let slot = if has_one {
        None
    } else {
        item_use.inventory.0[..ARMOUR_START]
            .iter()
            .position(Option::is_none)
            .map(|slot| {
                let stack = ItemStack::new(filled, 1);
                let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
                (slot, Some(stack))
            })
    };
    Some(UsedItem {
        changes: vec![(hit.pos, taken)],
        slot,
    })
}

/// A water or lava bucket empties into the block before the one it is aimed at, or
/// into that block itself if it can hold water. In creative mode it stays full. Where
/// its water evaporates it changes nothing here: see [`evaporated`].
fn filled_bucket(item_use: &ItemUse, blocks: &dyn Blocks, fluid: BlockState) -> Option<UsedItem> {
    let Emptied::Changes(changes) = pour(item_use, blocks, fluid)?.1 else {
        return None;
    };
    (!changes.is_empty()).then_some(UsedItem {
        changes,
        slot: None,
    })
}

/// Where the water of the bucket in hand evaporates instead of going in, as
/// `BucketItem.emptyContents` lets it where the level's `water_evaporates` holds (n7
/// BucketItem.txt 184-320): with the fizz and nothing placed, the creative player's
/// bucket kept full. For a water bucket or a bucket of a mob that holds water (a mob's
/// bucket evaporates its water too, and its mob is made all the same, as
/// `MobBucketItem.checkExtraContent` makes it); `None` for any other item, where water
/// does not evaporate, or where it goes nowhere.
pub(crate) fn evaporated(item_use: &ItemUse, blocks: &dyn Blocks) -> Option<BlockPos> {
    let held = item_use.held?;
    let holds_water = held == items::WATER_BUCKET
        || (data::item(held)?.class == ItemClass::MobBucketItem
            && held != items::SULFUR_CUBE_BUCKET);
    if !holds_water {
        return None;
    }
    match pour(item_use, blocks, blocks::WATER)?.1 {
        Emptied::Evaporated(pos) => Some(pos),
        Emptied::Changes(_) => None,
    }
}

/// Where a filled bucket of `fluid` aimed by `item_use` empties: the block
/// `BucketItem.use` names (the hit block where it takes water, else the one before it),
/// and what emptying there or, failing that, into the block before the hit comes to.
/// Vanilla's `emptyContents` tries the first target with the hit, where sneaking asks
/// for air, and then the block before the hit without it, where sneaking does not
/// matter. `None` where it goes nowhere or reaches a block the region cannot see.
fn pour(item_use: &ItemUse, blocks: &dyn Blocks, fluid: BlockState) -> Option<(BlockPos, Emptied)> {
    let hit = ray(item_use, blocks, Fluids::None).ok()??;
    let water = fluid == blocks::WATER;
    let hit_state = blocks.block(hit.pos)?;
    let beyond = hit.face.relative(hit.pos);
    let first = if water && hit_state.property("waterlogged").is_some() {
        hit.pos
    } else {
        beyond
    };
    let (sneaking, evaporates) = (item_use.sneaking, item_use.water_evaporates);
    let emptied = match empty_into(first, true, sneaking, blocks, fluid, evaporates)? {
        Some(emptied) => emptied,
        None => empty_into(beyond, false, sneaking, blocks, fluid, evaporates)??,
    };
    Some((first, emptied))
}

/// A bucket of a mob (`MobBucketItem`, which aims as a water bucket does): the block
/// the water goes into, which its mob is made at, and the changes, none where water is
/// there already or where it evaporates; `None` where the water goes nowhere.
pub(crate) fn mob_bucket(
    item_use: &ItemUse,
    blocks: &dyn Blocks,
) -> Option<(BlockPos, Vec<(BlockPos, BlockState)>)> {
    // `BucketItem.use`'s position, which `checkExtraContent` makes the mob at even where
    // the water went into the block before the hit instead, or evaporated.
    let (first, emptied) = pour(item_use, blocks, blocks::WATER)?;
    let changes = match emptied {
        Emptied::Changes(changes) => changes,
        Emptied::Evaporated(_) => Vec::new(),
    };
    Some((first, changes))
}

/// What emptying a bucket into a block that takes it comes to.
enum Emptied {
    /// The blocks that change, none where the fluid is there already.
    Changes(Vec<(BlockPos, BlockState)>),
    /// The water evaporated at the block, which keeps what it was.
    Evaporated(BlockPos),
}

/// What emptying a bucket of `fluid` into `pos` comes to: `Some` of it when it goes in
/// (possibly no change, where the fluid is there already), `Some(None)` when it does
/// not, and `None` when `pos` is in a chunk the region cannot see. Water that goes in
/// where `evaporates` holds evaporates before a block that takes water is asked to take
/// it, as `emptyContents` asks the level before `LiquidBlockContainer.placeLiquid` (n7
/// BucketItem.txt 184-201 against 321-363): a block that could be waterlogged in the
/// Nether is left as it is.
fn empty_into(
    pos: BlockPos,
    with_hit: bool,
    sneaking: bool,
    blocks: &dyn Blocks,
    fluid: BlockState,
    evaporates: bool,
) -> Option<Option<Emptied>> {
    let state = blocks.block(pos)?;
    let water = fluid == blocks::WATER;
    let props = state.props();
    let container = water && state.property("waterlogged").is_some();
    let never =
        data::is(state, BlockClass::EndPortalBlock) || data::is(state, BlockClass::EndGatewayBlock);
    let may_replace = !never && (props.is_air() || props.can_be_replaced() || !props.is_solid());
    let goes_in = props.is_air() || (may_replace || container) && !(sneaking && with_hit);
    if !goes_in {
        return Some(None);
    }
    if water && evaporates {
        return Some(Some(Emptied::Evaporated(pos)));
    }
    if container {
        if data::flag(state, "waterlogged") {
            return Some(Some(Emptied::Changes(Vec::new())));
        }
        return Some(Some(Emptied::Changes(vec![(
            pos,
            data::set_flag(state, "waterlogged", true),
        )])));
    }
    let same_source = props.fluid().is_some_and(|held| held.source)
        && if water {
            data::has_water(state)
        } else {
            data::has_lava(state)
        };
    if same_source {
        return Some(Some(Emptied::Changes(Vec::new())));
    }
    Some(Some(Emptied::Changes(vec![(pos, fluid)])))
}

/// A lily pad or frogspawn goes onto the block above what the ray hits, through
/// fluids to a source, as a block placed by a click on that block's top.
fn on_water(item_use: &ItemUse, blocks: &dyn Blocks, bodies: &[[f64; 6]]) -> Option<UsedItem> {
    let hit = ray(item_use, blocks, Fluids::Sources).ok()??;
    let position = Dir::Up.relative(hit.pos);
    let clicked = blocks.block(position)?;
    let cursor = [
        (hit.location[0] - f64::from(position.x)) as f32,
        (hit.location[1] - f64::from(position.y)) as f32,
        (hit.location[2] - f64::from(position.z)) as f32,
    ];
    let click = Click {
        other_hand: None,
        position,
        face: geometry::to_api(hit.face),
        cursor,
        sneaking: item_use.sneaking,
        held: item_use.held,
        feet: item_use.feet,
        yaw: item_use.yaw,
        pitch: item_use.pitch,
    };
    let (target, replace_clicked) = super::target_of(&click, clicked, blocks);
    let changes = super::place(&click, target, replace_clicked, clicked, blocks, bodies)?;
    Some(UsedItem {
        changes,
        slot: None,
    })
}

/// Which fluids a ray meets beside the blocks' outlines (`ClipContext.Fluid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fluids {
    None,
    Sources,
    Any,
}

/// Where a ray from the player's eyes along their view first meets a block's outline
/// or any fluid's surface within reach, as `BoatItem.use` aims
/// (`Item.getPlayerPOVHitResult` with `ClipContext.Fluid.ANY`): `Ok(None)` when it
/// meets nothing, `Err` when it reaches a block the caller cannot see first.
pub(crate) fn boat_hit(item_use: &ItemUse, blocks: &dyn Blocks) -> Result<Option<[f64; 3]>, ()> {
    Ok(ray(item_use, blocks, Fluids::Any)?.map(|hit| hit.location))
}

/// The block where a ray from the player's eyes along their view first meets a block's
/// outline or a fluid's source within reach, as `SpawnEggItem.use` aims
/// (`Item.getPlayerPOVHitResult` with `ClipContext.Fluid.SOURCE_ONLY`): `Ok(None)` when
/// it meets nothing, `Err` when it reaches a block the caller cannot see first.
pub(crate) fn source_hit(item_use: &ItemUse, blocks: &dyn Blocks) -> Result<Option<BlockPos>, ()> {
    Ok(ray(item_use, blocks, Fluids::Sources)?.map(|hit| hit.pos))
}

/// The first block a ray from the player's eyes along their view meets within reach,
/// by the blocks' outline shapes and the shapes of the fluids `fluids` names.
/// `Ok(None)` when it meets nothing, `Err` when it reaches a block the caller cannot
/// see first. Vanilla's `BlockGetter.traverseBlocks` and `Level.clip`.
fn ray(item_use: &ItemUse, blocks: &dyn Blocks, sources: Fluids) -> Result<Option<Hit>, ()> {
    let from = eyes(item_use.feet, item_use.sneaking);
    let view = view_vector(item_use.yaw, item_use.pitch);
    let to = [
        from[0] + view[0] * REACH,
        from[1] + view[1] * REACH,
        from[2] + view[2] * REACH,
    ];
    if from == to {
        return Ok(None);
    }
    let lerp = |delta: f64, a: f64, b: f64| a + delta * (b - a);
    let end: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
    let start: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
    let mut current: [i32; 3] = std::array::from_fn(|i| start[i].floor() as i32);
    if let Some(hit) = clip_block(block_pos(current), from, to, blocks, sources)? {
        return Ok(Some(hit));
    }
    let delta: [f64; 3] = std::array::from_fn(|i| end[i] - start[i]);
    let sign: [i32; 3] = std::array::from_fn(|i| {
        if delta[i] > 0.0 {
            1
        } else if delta[i] < 0.0 {
            -1
        } else {
            0
        }
    });
    let step: [f64; 3] = std::array::from_fn(|i| {
        if sign[i] == 0 {
            f64::MAX
        } else {
            f64::from(sign[i]) / delta[i]
        }
    });
    let mut t: [f64; 3] = std::array::from_fn(|i| {
        let fraction = start[i] - start[i].floor();
        step[i]
            * if sign[i] > 0 {
                1.0 - fraction
            } else {
                fraction
            }
    });
    while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
        let axis = if t[0] < t[1] {
            if t[0] < t[2] { 0 } else { 2 }
        } else if t[1] < t[2] {
            1
        } else {
            2
        };
        current[axis] += sign[axis];
        t[axis] += step[axis];
        if let Some(hit) = clip_block(block_pos(current), from, to, blocks, sources)? {
            return Ok(Some(hit));
        }
    }
    Ok(None)
}

fn block_pos([x, y, z]: [i32; 3]) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// Where the ray meets the block at `pos`: the nearer of its outline and the shape of
/// the fluid in it, where `sources` names that fluid.
fn clip_block(
    pos: BlockPos,
    from: [f64; 3],
    to: [f64; 3],
    blocks: &dyn Blocks,
    sources: Fluids,
) -> Result<Option<Hit>, ()> {
    let state = blocks.block(pos).ok_or(())?;
    let outline = geometry::moved(state.props().outline_shape(), pos);
    let mut best = geometry::clip_boxes(&outline, from, to);
    let picked = state.props().fluid().is_some_and(|fluid| match sources {
        Fluids::None => false,
        Fluids::Sources => fluid.source,
        Fluids::Any => true,
    });
    if picked {
        let fluid_boxes = fluid_shape(state, pos, blocks);
        if let Some(fluid_hit) = geometry::clip_boxes(&fluid_boxes, from, to)
            && best.is_none_or(|(t, _)| fluid_hit.0 < t)
        {
            best = Some(fluid_hit);
        }
    }
    Ok(best.map(|(t, face)| Hit {
        pos,
        face,
        location: std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t),
    }))
}

/// The shape of the fluid in `state`: as high as its amount, or the whole block under
/// the same fluid.
fn fluid_shape(state: BlockState, pos: BlockPos, blocks: &dyn Blocks) -> Vec<[f64; 6]> {
    let above = blocks.block(Dir::Up.relative(pos)).unwrap_or(blocks::AIR);
    let same_above = if data::has_water(state) {
        data::has_water(above)
    } else {
        data::has_lava(above)
    };
    let amount = state.props().fluid().map_or(0, |fluid| fluid.amount);
    let height = if same_above {
        1.0
    } else {
        f64::from(f32::from(amount) / 9.0)
    };
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    vec![[x, y, z, x + 1.0, y + height, z + 1.0]]
}

/// Where a player's eyes are: 1.62 above the feet, 1.27 while sneaking.
fn eyes(feet: Vec3, sneaking: bool) -> [f64; 3] {
    let height = if sneaking { 1.27 } else { 1.62 };
    [feet.x, feet.y + height, feet.z]
}

/// Vanilla's `Entity.calculateViewVector`, with the game's table of sines.
fn view_vector(yaw: f32, pitch: f32) -> [f64; 3] {
    const RADIANS_PER_DEGREE: f32 = std::f64::consts::PI as f32 / 180.0;
    let pitch = pitch * RADIANS_PER_DEGREE;
    let yaw = -yaw * RADIANS_PER_DEGREE;
    let yaw_cos = trig::cos(f64::from(yaw));
    let yaw_sin = trig::sin(f64::from(yaw));
    let pitch_cos = trig::cos(f64::from(pitch));
    let pitch_sin = trig::sin(f64::from(pitch));
    [
        f64::from(yaw_sin * pitch_cos),
        f64::from(-pitch_sin),
        f64::from(yaw_cos * pitch_cos),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use clustine_world::Dimension;

    use super::*;
    use crate::api::Inventory;

    /// A world of air with the blocks set in it, loaded for x below 16.
    #[derive(Default)]
    struct World(BTreeMap<BlockPos, BlockState>);

    impl World {
        fn with(mut self, pos: BlockPos, state: BlockState) -> Self {
            self.0.insert(pos, state);
            self
        }

        /// A floor of stone at y = 63.
        fn floor() -> Self {
            let mut world = Self::default();
            for x in -4..8 {
                for z in -4..8 {
                    world.0.insert(BlockPos::new(x, 63, z), blocks::STONE);
                }
            }
            world
        }
    }

    impl Blocks for World {
        fn block(&self, at: BlockPos) -> Option<BlockState> {
            if at.x >= 16 || !(-64..320).contains(&at.y) {
                return None;
            }
            Some(*self.0.get(&at).unwrap_or(&blocks::AIR))
        }
    }

    /// The use of `held` by a player standing on the floor and looking down ahead, in
    /// `dimension`.
    fn used_in(dimension: Dimension, held: i32) -> ItemUse {
        ItemUse {
            held: Some(held),
            feet: Vec3::new(0.5, 64.0, 0.5),
            yaw: 0.0,
            pitch: 60.0,
            sneaking: false,
            inventory: Inventory::default(),
            water_evaporates: dimension.rules().water_evaporates,
        }
    }

    /// Where a water bucket's water goes from `used_in` on the bare floor: above the
    /// stone the player looks at.
    fn target() -> BlockPos {
        let used = use_item(
            &used_in(Dimension::Overworld, items::WATER_BUCKET),
            &World::floor(),
            &[],
        )
        .expect("water goes onto the floor");
        used.changes[0].0
    }

    #[test]
    fn a_water_bucket_places_water_where_water_does_not_evaporate_and_fizzes_in_the_nether() {
        let world = World::floor();
        let pos = target();
        for dimension in Dimension::ALL {
            let item_use = used_in(dimension, items::WATER_BUCKET);
            let used = use_item(&item_use, &world, &[]);
            if dimension == Dimension::Nether {
                assert_eq!(used, None, "nothing is placed");
                assert_eq!(evaporated(&item_use, &world), Some(pos));
            } else {
                let used = used.expect("water is placed");
                assert_eq!(used.changes, vec![(pos, blocks::WATER)], "{dimension:?}");
                assert_eq!(used.slot, None, "the creative player's bucket stays full");
                assert_eq!(evaporated(&item_use, &world), None, "{dimension:?}");
            }
        }
    }

    #[test]
    fn water_evaporates_before_a_block_that_takes_it_is_waterlogged() {
        let pos = target();
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        let slab = BlockState::parse("minecraft:oak_slab[type=bottom]").expect("a slab");
        let world = World::floor().with(below, slab);
        for dimension in Dimension::ALL {
            let item_use = used_in(dimension, items::WATER_BUCKET);
            if dimension == Dimension::Nether {
                assert_eq!(use_item(&item_use, &world, &[]), None);
                assert_eq!(evaporated(&item_use, &world), Some(below));
            } else {
                let used = use_item(&item_use, &world, &[]).expect("the slab takes water");
                assert_eq!(
                    used.changes,
                    vec![(below, data::set_flag(slab, "waterlogged", true))],
                    "{dimension:?}"
                );
            }
        }
    }

    #[test]
    fn water_evaporates_even_into_a_block_already_full_of_it() {
        // `emptyContents` asks the level before it looks at what is there.
        let pos = target();
        let world = World::floor().with(pos, blocks::WATER);
        let nether = used_in(Dimension::Nether, items::WATER_BUCKET);
        assert_eq!(evaporated(&nether, &world), Some(pos));
        for dimension in [Dimension::Overworld, Dimension::End] {
            let item_use = used_in(dimension, items::WATER_BUCKET);
            assert_eq!(use_item(&item_use, &world, &[]), None, "no change to make");
            assert_eq!(evaporated(&item_use, &world), None);
        }
    }

    #[test]
    fn lava_is_poured_in_every_dimension() {
        let world = World::floor();
        let pos = target();
        for dimension in Dimension::ALL {
            let item_use = used_in(dimension, items::LAVA_BUCKET);
            let used = use_item(&item_use, &world, &[]).expect("lava is placed");
            assert_eq!(used.changes, vec![(pos, blocks::LAVA)], "{dimension:?}");
            assert_eq!(evaporated(&item_use, &world), None);
        }
    }

    #[test]
    fn a_mob_bucket_s_water_evaporates_in_the_nether_and_its_mob_is_still_made_there() {
        let world = World::floor();
        let pos = target();
        for dimension in Dimension::ALL {
            let item_use = used_in(dimension, items::COD_BUCKET);
            let (at, changes) = mob_bucket(&item_use, &world).expect("the bucket is emptied");
            assert_eq!(at, pos, "{dimension:?}");
            if dimension == Dimension::Nether {
                assert_eq!(changes, vec![]);
                assert_eq!(evaporated(&item_use, &world), Some(pos));
            } else {
                assert_eq!(changes, vec![(pos, blocks::WATER)], "{dimension:?}");
                assert_eq!(evaporated(&item_use, &world), None);
            }
        }
    }

    #[test]
    fn only_buckets_of_water_evaporate() {
        let world = World::floor();
        let nether = |held| used_in(Dimension::Nether, held);
        for held in [
            items::LAVA_BUCKET,
            items::BUCKET,
            items::POWDER_SNOW_BUCKET,
            items::SULFUR_CUBE_BUCKET,
            items::LILY_PAD,
        ] {
            assert_eq!(evaporated(&nether(held), &world), None, "{held}");
        }
        for held in [
            items::WATER_BUCKET,
            items::AXOLOTL_BUCKET,
            items::TADPOLE_BUCKET,
        ] {
            assert!(evaporated(&nether(held), &world).is_some(), "{held}");
        }
    }

    #[test]
    fn a_water_bucket_aimed_at_nothing_evaporates_nowhere() {
        let mut item_use = used_in(Dimension::Nether, items::WATER_BUCKET);
        item_use.pitch = -90.0;
        assert_eq!(evaporated(&item_use, &World::floor()), None);
        assert_eq!(use_item(&item_use, &World::floor(), &[]), None);
    }
}
