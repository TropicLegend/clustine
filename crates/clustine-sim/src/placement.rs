//! What a click on a block or the use of an item does to blocks, by vanilla's rules:
//! using a door, placing a block with the state vanilla gives it, buckets. See
//! `docs/groundwork/w6-plan.md`, sections 3 to 5, which this module follows.
//!
//! Every function here is pure: it reads blocks through [`Blocks`] and says what is
//! to change, and the region applies that all or nothing. A rule reads blocks of
//! chunks the region holds and has loaded; the region decides which region a target
//! belongs to and passes a placement on when it is another's (section 4.6).

use clustine_data::BlockState;
use clustine_world::{BlockPos, Vec3};
use serde::{Deserialize, Serialize};

use crate::api::{Face, HOTBAR_SLOTS, ItemStack};

/// The blocks a rule reads.
pub trait Blocks {
    /// The block at `at`, or `None` if it is in a chunk the caller has not loaded or
    /// outside the world's height. A rule never takes `None` for air: what it would
    /// change or stand on there is not placed (section 4.3), and what it only looks at
    /// beside it is taken as air (section 4.5).
    fn block(&self, at: BlockPos) -> Option<BlockState>;
}

/// A click on a block with the use button.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Click {
    /// The clicked block, `P`.
    pub position: BlockPos,
    /// The clicked face, `F`.
    pub face: Face,
    /// Where on the clicked block the click landed, each from 0 to 1.
    pub cursor: [f32; 3],
    /// Whether the player held the sneak key.
    pub sneaking: bool,
    /// The id of the item in the main hand, `None` for an empty hand.
    pub held: Option<i32>,
    /// Where the player's feet are.
    pub feet: Vec3,
    /// The player's rotation, in degrees as the protocol has it.
    pub yaw: f32,
    pub pitch: f32,
}

/// What a click comes to where the clicked block is held (section 3.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Clicked {
    /// Nothing changes. The click is acknowledged all the same.
    Nothing,
    /// The clicked block was used, and these blocks change, all or none.
    Used(Vec<(BlockPos, BlockState)>),
    /// A block is to be placed at `target` (`T`), which the region that holds it does
    /// with [`place`]. `replace_clicked` is whether `T` is the clicked block itself,
    /// and `clicked` the clicked block's state as read here (section 4.5).
    Place {
        target: BlockPos,
        replace_clicked: bool,
        clicked: BlockState,
    },
}

/// What a click on `click.position` does, read where that block is held: a use, or
/// where a block is to be placed (sections 3.2, 4.1 and 4.2).
pub fn click(_click: &Click, _blocks: &dyn Blocks) -> Clicked {
    Clicked::Nothing
}

/// The blocks that change when the item of `click` is placed at `target`, or `None`
/// when nothing is placed (sections 4.1 to 4.4). `clicked` is the state of the
/// clicked block as the region that holds it read it; if `blocks` has the clicked
/// block, what it has wins. `bodies` are the boxes of the players other than the one
/// who clicks, `[min x, min y, min z, max x, max y, max z]`, which a placed block may
/// not overlap; the clicking player's own box is made from `click`.
pub fn place(
    _click: &Click,
    _target: BlockPos,
    _replace_clicked: bool,
    _clicked: BlockState,
    _blocks: &dyn Blocks,
    _bodies: &[[f64; 6]],
) -> Option<Vec<(BlockPos, BlockState)>> {
    None
}

/// The use of the item in the main hand without a block it uses on: a bucket or a
/// lily pad (section 5).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemUse {
    /// The id of the item in the main hand, `None` for an empty hand.
    pub held: Option<i32>,
    /// Where the player's feet are.
    pub feet: Vec3,
    /// The rotation the client sent with the use, which is the player's from then on.
    pub yaw: f32,
    pub pitch: f32,
    pub sneaking: bool,
    /// What the player's hotbar holds, for an empty bucket's filled one.
    pub hotbar: [Option<ItemStack>; HOTBAR_SLOTS],
}

/// What the use of an item changes.
#[derive(Debug, Clone, PartialEq)]
pub struct UsedItem {
    /// The blocks that change, all or none.
    pub changes: Vec<(BlockPos, BlockState)>,
    /// A hotbar slot that changes, and what it holds then.
    pub hotbar: Option<(u8, Option<ItemStack>)>,
}

/// What the use of `item_use`'s item changes, or `None` if nothing does.
pub fn use_item(
    _item_use: &ItemUse,
    _blocks: &dyn Blocks,
    _bodies: &[[f64; 6]],
) -> Option<UsedItem> {
    None
}

/// The box of a player whose feet are at `feet`: 0.6 wide, 1.8 high, or 1.5 while
/// sneaking (section 4.3).
pub fn body(feet: Vec3, sneaking: bool) -> [f64; 6] {
    let height = if sneaking { 1.5 } else { 1.8 };
    [
        feet.x - 0.3,
        feet.y,
        feet.z - 0.3,
        feet.x + 0.3,
        feet.y + height,
        feet.z + 0.3,
    ]
}
