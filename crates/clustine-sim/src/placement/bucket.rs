// Adapted from SteelMC's `steel-core/src/behavior/items/bucket.rs` and
// `items/place_on_water_block_item.rs`.

//! The use of a bucket or a lily pad, which aims by a ray from the player's eyes
//! rather than at a clicked block (section 5).

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, ItemClass, blocks, items};
use clustine_noise::trig;
use clustine_world::{BlockPos, Vec3};

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
    let hit = ray(item_use, blocks, true).ok()??;
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
    // the inventory if it holds none, which here is the hotbar.
    let has_one = item_use
        .hotbar
        .iter()
        .flatten()
        .any(|stack| stack.item == filled);
    let hotbar = if has_one {
        None
    } else {
        item_use
            .hotbar
            .iter()
            .position(Option::is_none)
            .map(|slot| {
                let stack = ItemStack {
                    item: filled,
                    count: 1,
                };
                (slot as u8, Some(stack))
            })
    };
    Some(UsedItem {
        changes: vec![(hit.pos, taken)],
        hotbar,
    })
}

/// A water or lava bucket empties into the block before the one it is aimed at, or
/// into that block itself if it can hold water. In creative mode it stays full.
fn filled_bucket(item_use: &ItemUse, blocks: &dyn Blocks, fluid: BlockState) -> Option<UsedItem> {
    let hit = ray(item_use, blocks, false).ok()??;
    let water = fluid == blocks::WATER;
    let hit_state = blocks.block(hit.pos)?;
    let beyond = hit.face.relative(hit.pos);
    let first = if water && hit_state.property("waterlogged").is_some() {
        hit.pos
    } else {
        beyond
    };
    // Vanilla's `emptyContents` tries the first target with the hit, where sneaking
    // asks for air, and then the block before the hit without it, where sneaking does
    // not matter.
    let changes = match empty_into(first, true, item_use.sneaking, blocks, fluid)? {
        Some(changes) => changes,
        None => empty_into(beyond, false, item_use.sneaking, blocks, fluid)??,
    };
    (!changes.is_empty()).then_some(UsedItem {
        changes,
        hotbar: None,
    })
}

/// What emptying a bucket of `fluid` into `pos` changes: `Some` of the changes when it
/// goes in (possibly none, where the fluid is there already), `Some(None)` when it does
/// not, and `None` when `pos` is in a chunk the region cannot see.
fn empty_into(
    pos: BlockPos,
    with_hit: bool,
    sneaking: bool,
    blocks: &dyn Blocks,
    fluid: BlockState,
) -> Option<Option<Vec<(BlockPos, BlockState)>>> {
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
    if container {
        if data::flag(state, "waterlogged") {
            return Some(Some(Vec::new()));
        }
        return Some(Some(vec![(
            pos,
            data::set_flag(state, "waterlogged", true),
        )]));
    }
    let same_source = props.fluid().is_some_and(|held| held.source)
        && if water {
            data::has_water(state)
        } else {
            data::has_lava(state)
        };
    if same_source {
        return Some(Some(Vec::new()));
    }
    Some(Some(vec![(pos, fluid)]))
}

/// A lily pad or frogspawn goes onto the block above what the ray hits, through
/// fluids to a source, as a block placed by a click on that block's top.
fn on_water(item_use: &ItemUse, blocks: &dyn Blocks, bodies: &[[f64; 6]]) -> Option<UsedItem> {
    let hit = ray(item_use, blocks, true).ok()??;
    let position = Dir::Up.relative(hit.pos);
    let clicked = blocks.block(position)?;
    let cursor = [
        (hit.location[0] - f64::from(position.x)) as f32,
        (hit.location[1] - f64::from(position.y)) as f32,
        (hit.location[2] - f64::from(position.z)) as f32,
    ];
    let click = Click {
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
        hotbar: None,
    })
}

/// The first block a ray from the player's eyes along their view meets within reach,
/// by the blocks' outline shapes and, with `sources`, the shapes of fluid sources.
/// `Ok(None)` when it meets nothing, `Err` when it reaches a block the caller cannot
/// see first. Vanilla's `BlockGetter.traverseBlocks` and `Level.clip`.
fn ray(item_use: &ItemUse, blocks: &dyn Blocks, sources: bool) -> Result<Option<Hit>, ()> {
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

/// Where the ray meets the block at `pos`: the nearer of its outline and, with
/// `sources`, the shape of a fluid source in it.
fn clip_block(
    pos: BlockPos,
    from: [f64; 3],
    to: [f64; 3],
    blocks: &dyn Blocks,
    sources: bool,
) -> Result<Option<Hit>, ()> {
    let state = blocks.block(pos).ok_or(())?;
    let outline = geometry::moved(state.props().outline_shape(), pos);
    let mut best = geometry::clip_boxes(&outline, from, to);
    if sources && state.props().fluid().is_some_and(|fluid| fluid.source) {
        let fluid_boxes = fluid_shape(state, pos, blocks);
        if let Some(fluid_hit) = geometry::clip_boxes(&fluid_boxes, from, to) {
            if best.is_none_or(|(t, _)| fluid_hit.0 < t) {
                best = Some(fluid_hit);
            }
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
