//! What a click on a block or the use of an item does to blocks, by vanilla's rules:
//! using a door, placing a block with the state vanilla gives it, buckets. See
//! `docs/groundwork/w6-plan.md`, sections 3 to 5, which this module follows.
//!
//! Every function here is pure: it reads blocks through [`Blocks`] and says what is
//! to change, and the region applies that all or nothing. A rule reads blocks of
//! chunks the region holds and has loaded; the region decides which region a target
//! belongs to and passes a placement on when it is another's (section 4.6).

mod bucket;
mod context;
mod data;
mod geometry;
mod replace;
mod rules;
mod survive;
#[cfg(test)]
mod tests;

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, ItemClass};
use clustine_world::{BlockPos, Vec3};
use serde::{Deserialize, Serialize};

use crate::api::{Face, Inventory, ItemStack};
use context::{Context, World};
use geometry::DirExt;

/// The blocks a rule reads.
pub trait Blocks {
    /// The block at `at`, or `None` if it is in a chunk the caller has not loaded or
    /// outside the world's height. A rule never takes `None` for air: what it would
    /// change or stand on there is not placed (section 4.3), and what it only looks at
    /// beside it is taken as air (section 4.5).
    fn block(&self, at: BlockPos) -> Option<BlockState>;

    /// The output the comparator's block entity at `at` keeps, which a powered
    /// comparator gives as its signal; 0 where there is none or the caller keeps no
    /// block entities.
    fn comparator_output(&self, _at: BlockPos) -> u8 {
        0
    }
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
    /// The clicked block was used: these blocks are set, all or none, in order, each
    /// with the game's `flags` for that use (`Level.setBlock`'s), and what else
    /// changes follows from the region's rules (`docs/adr/0026-redstone.md`, section
    /// 7).
    Used {
        changes: Vec<(BlockPos, BlockState)>,
        flags: u32,
    },
    /// The clicked block's own rule is its use (`useWithoutItem`), which the region
    /// runs inside its updater, reading the blocks as the rule reads them: a lever
    /// toggled, a button pressed, wire turned from a dot to a cross, a note block
    /// tuned, a bell rung (`docs/adr/0026-redstone.md`, section 7). `face` is the
    /// clicked face, which a bell rings away from.
    ByRule { position: BlockPos, face: Dir },
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
pub fn click(click: &Click, blocks: &dyn Blocks) -> Clicked {
    let Some(clicked) = blocks.block(click.position) else {
        return Clicked::Nothing;
    };
    // Sneaking with something in hand passes the block by and places against it.
    if !(click.sneaking && click.held.is_some()) && is_used(clicked, click.held) {
        let used = use_block(click, clicked, blocks);
        // Wire that is neither a dot nor a cross, or would not change, a bell hit
        // where it does not ring, and a note block clicked on top with a head pass the
        // click on to the item in hand (`InteractionResult.PASS`).
        let passes = [
            BlockClass::RedstoneWireBlock,
            BlockClass::BellBlock,
            BlockClass::NoteBlock,
        ]
        .into_iter()
        .any(|class| data::is(clicked, class));
        if !(used == Clicked::Nothing && passes) {
            return used;
        }
    }
    if placing(click.held, true).is_none() {
        return Clicked::Nothing;
    }
    let (target, replace_clicked) = target_of(click, clicked, blocks);
    Clicked::Place {
        target,
        replace_clicked,
        clicked,
    }
}

/// Where a click puts a block, `T`, and whether that is the clicked block itself:
/// vanilla's `BlockPlaceContext.replaceClicked` (section 4.1). Whether the block at `T`
/// gives way when it is not the clicked one is for [`place`] to judge.
fn target_of(click: &Click, clicked: BlockState, blocks: &dyn Blocks) -> (BlockPos, bool) {
    let context = context(click, click.position, true, clicked, blocks);
    if replace::can_be_replaced(clicked, &context) {
        (click.position, true)
    } else {
        (click.face.neighbour(click.position), false)
    }
}

/// Whether a click on `state` with `held` in hand is taken by the block (section 3.2).
fn is_used(state: BlockState, held: Option<i32>) -> bool {
    if !data::info(state).used_by_click {
        return false;
    }
    // A fence is used only to tie a lead, which Clustine has none of, and an empty
    // jukebox only to take a disc: other clicks go on to placing.
    if data::is(state, BlockClass::FenceBlock) {
        return false;
    }
    if data::is(state, BlockClass::JukeboxBlock) && !data::flag(state, "has_record") {
        return held
            .and_then(data::item)
            .is_some_and(|item| item.name.starts_with("minecraft:music_disc_"));
    }
    true
}

/// `Block.UPDATE_CLIENTS | Block.UPDATE_IMMEDIATE`, with which a door and a gate set
/// themselves when used: the neighbours are not told.
const USED_DOOR: u32 = 2 | 8;
/// `Block.UPDATE_CLIENTS`, a trapdoor's.
const USED_TRAPDOOR: u32 = 2;
/// `Block.UPDATE_ALL`, a flower pot's.
const USED_POT: u32 = 1 | 2;

/// What using `state` does: doors, trapdoors and fence gates open and close, a flower
/// goes into an empty pot, each set as the game's `useWithoutItem` and `useItemOn` set
/// it; a lever, a button and wire that changes are used by their own rules; and every
/// other used block does nothing yet.
fn use_block(click: &Click, state: BlockState, blocks: &dyn Blocks) -> Clicked {
    let position = click.position;
    let flip_open = |state: BlockState| data::set_flag(state, "open", !data::flag(state, "open"));
    let used = |state, flags| Clicked::Used {
        changes: vec![(position, state)],
        flags,
    };
    // The game sets the clicked half, and the other follows from its shape. A shape
    // asked again waits where the block's neighbours cannot all be read, as at a line
    // whose other side the region has no copy of, which would leave a door open by
    // half; so the other half is set here too, after the clicked one, as its shape
    // would set it.
    if data::is(state, BlockClass::DoorBlock) {
        let mut changes = vec![(position, flip_open(state))];
        let (other_pos, other_half) = if state.property("half") == Some("upper") {
            (Dir::Down.relative(position), "lower")
        } else {
            (Dir::Up.relative(position), "upper")
        };
        if let Some(other) = blocks.block(other_pos)
            && data::same_block(other, state)
            && other.property("half") == Some(other_half)
        {
            changes.push((other_pos, flip_open(other)));
        }
        return Clicked::Used {
            changes,
            flags: USED_DOOR,
        };
    }
    if data::is(state, BlockClass::TrapDoorBlock) {
        return used(flip_open(state), USED_TRAPDOOR);
    }
    if data::is(state, BlockClass::FenceGateBlock) {
        let changed = if data::flag(state, "open") {
            data::set_flag(state, "open", false)
        } else {
            let facing = geometry::from_y_rot(click.yaw);
            let state = if survive::facing(state) == Some(facing.opposite()) {
                data::set(state, "facing", facing.name())
            } else {
                state
            };
            data::set_flag(state, "open", true)
        };
        return used(changed, USED_DOOR);
    }
    // A lever, a button and wire use themselves by rules that tell their neighbours,
    // which the region runs.
    let face = geometry::from_api(click.face);
    // A repeater's delay and a comparator's mode, which every Clustine player may
    // change, as creative players may build (`Abilities.mayBuild`).
    if data::is(state, BlockClass::LeverBlock)
        || data::is(state, BlockClass::ButtonBlock)
        || data::is(state, BlockClass::DiodeBlock)
    {
        return Clicked::ByRule { position, face };
    }
    // `NoteBlock.useItemOn`: a head clicked onto the top is placed there; anything
    // else tunes the block (`useWithoutItem`).
    if data::is(state, BlockClass::NoteBlock) {
        let head = click.held.is_some_and(|held| {
            clustine_blocks::data::item_in_tag(held, "minecraft:noteblock_top_instruments")
        });
        if head && face == Dir::Up {
            return Clicked::Nothing;
        }
        return Clicked::ByRule { position, face };
    }
    // `BellBlock.onHit`: a bell rings only when it is hit on a side it swings from,
    // below its top (`isProperHit`).
    if data::is(state, BlockClass::BellBlock) {
        let height = f64::from(click.cursor[1]);
        if bell_hit_properly(state, face, height) {
            return Clicked::ByRule { position, face };
        }
        return Clicked::Nothing;
    }
    if data::is(state, BlockClass::RedstoneWireBlock) {
        let world = World::new(blocks, position, state);
        let changes =
            clustine_blocks::signal::wire_used(&mut context::View(&world), state, position);
        return match changes {
            Some(_) => Clicked::ByRule { position, face },
            None => Clicked::Nothing,
        };
    }
    if data::is(state, BlockClass::FlowerPotBlock) && data::info(state).potted == Some(0) {
        let plant = click.held.and_then(data::item).and_then(|item| item.places);
        if let Some(pot) = plant.and_then(data::pot_holding) {
            return used(pot, USED_POT);
        }
    }
    Clicked::Nothing
}

/// `BellBlock.isProperHit`: a hit on a side, no higher than 0.8124 up the block, on
/// the bell's own axis for one on the floor, across it for one on a wall, anywhere for
/// one under a ceiling.
fn bell_hit_properly(state: BlockState, face: Dir, height: f64) -> bool {
    if !face.is_horizontal() || height > 0.812_399_983_406_066_9 {
        return false;
    }
    let Some(facing) = survive::facing(state) else {
        return false;
    };
    match state.property("attachment") {
        Some("floor") => facing.axis() == face.axis(),
        Some("single_wall" | "double_wall") => facing.axis() != face.axis(),
        Some("ceiling") => true,
        _ => false,
    }
}

/// The sound of using `state`, which a door, a trapdoor or a gate has just become:
/// its open or its close sound, by its block set or its wood, as the game's
/// `BlockSetType` and `WoodType` give them.
pub fn use_sound(state: BlockState) -> Option<i32> {
    use clustine_data::sounds as s;
    let name = state.block().map(|block| block.name)?;
    let open = data::flag(state, "open");
    let of = |(opened, closed): (i32, i32)| Some(if open { opened } else { closed });
    let wood = |copper, iron, bamboo, cherry, nether, plain| {
        if name.contains("copper") {
            copper
        } else if name.contains("iron") {
            iron
        } else if name.contains("bamboo") {
            bamboo
        } else if name.contains("cherry") {
            cherry
        } else if name.contains("crimson") || name.contains("warped") {
            nether
        } else {
            plain
        }
    };
    if data::is(state, BlockClass::DoorBlock) {
        return of(wood(
            (s::BLOCK_COPPER_DOOR_OPEN, s::BLOCK_COPPER_DOOR_CLOSE),
            (s::BLOCK_IRON_DOOR_OPEN, s::BLOCK_IRON_DOOR_CLOSE),
            (
                s::BLOCK_BAMBOO_WOOD_DOOR_OPEN,
                s::BLOCK_BAMBOO_WOOD_DOOR_CLOSE,
            ),
            (
                s::BLOCK_CHERRY_WOOD_DOOR_OPEN,
                s::BLOCK_CHERRY_WOOD_DOOR_CLOSE,
            ),
            (
                s::BLOCK_NETHER_WOOD_DOOR_OPEN,
                s::BLOCK_NETHER_WOOD_DOOR_CLOSE,
            ),
            (s::BLOCK_WOODEN_DOOR_OPEN, s::BLOCK_WOODEN_DOOR_CLOSE),
        ));
    }
    if data::is(state, BlockClass::TrapDoorBlock) {
        return of(wood(
            (
                s::BLOCK_COPPER_TRAPDOOR_OPEN,
                s::BLOCK_COPPER_TRAPDOOR_CLOSE,
            ),
            (s::BLOCK_IRON_TRAPDOOR_OPEN, s::BLOCK_IRON_TRAPDOOR_CLOSE),
            (
                s::BLOCK_BAMBOO_WOOD_TRAPDOOR_OPEN,
                s::BLOCK_BAMBOO_WOOD_TRAPDOOR_CLOSE,
            ),
            (
                s::BLOCK_CHERRY_WOOD_TRAPDOOR_OPEN,
                s::BLOCK_CHERRY_WOOD_TRAPDOOR_CLOSE,
            ),
            (
                s::BLOCK_NETHER_WOOD_TRAPDOOR_OPEN,
                s::BLOCK_NETHER_WOOD_TRAPDOOR_CLOSE,
            ),
            (
                s::BLOCK_WOODEN_TRAPDOOR_OPEN,
                s::BLOCK_WOODEN_TRAPDOOR_CLOSE,
            ),
        ));
    }
    if data::is(state, BlockClass::FenceGateBlock) {
        let plain = (s::BLOCK_FENCE_GATE_OPEN, s::BLOCK_FENCE_GATE_CLOSE);
        return of(wood(
            plain,
            plain,
            (
                s::BLOCK_BAMBOO_WOOD_FENCE_GATE_OPEN,
                s::BLOCK_BAMBOO_WOOD_FENCE_GATE_CLOSE,
            ),
            (
                s::BLOCK_CHERRY_WOOD_FENCE_GATE_OPEN,
                s::BLOCK_CHERRY_WOOD_FENCE_GATE_CLOSE,
            ),
            (
                s::BLOCK_NETHER_WOOD_FENCE_GATE_OPEN,
                s::BLOCK_NETHER_WOOD_FENCE_GATE_CLOSE,
            ),
            plain,
        ));
    }
    None
}

/// How the held item places a block (section 3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placing {
    /// The block's own rule, by its class.
    Block(u16),
    /// Vanilla's `StandingAndWallBlockItem`: torches, signs, banners, heads, coral
    /// fans.
    StandingOrWall {
        standing: u16,
        wall: u16,
        attach: Dir,
    },
}

/// How `held` places a block, if it does. `from_click` leaves out the items that place
/// only by their use, on water.
fn placing(held: Option<i32>, from_click: bool) -> Option<Placing> {
    let item = data::item(held?)?;
    match item.class {
        // Only an operator places these, and Clustine has none.
        ItemClass::GameMasterBlockItem => return None,
        ItemClass::PlaceOnWaterBlockItem if from_click => return None,
        _ => {}
    }
    let block = item.places?;
    Some(match item.wall {
        Some((wall, attach)) => Placing::StandingOrWall {
            standing: block,
            wall,
            attach,
        },
        None => Placing::Block(block),
    })
}

/// The context of a click placing at `target`.
fn context<'a>(
    click: &Click,
    target: BlockPos,
    replace_clicked: bool,
    clicked: BlockState,
    blocks: &'a dyn Blocks,
) -> Context<'a> {
    let p = click.position;
    Context {
        world: World::new(blocks, p, clicked),
        face: geometry::from_api(click.face),
        location: [
            f64::from(p.x) + f64::from(click.cursor[0]),
            f64::from(p.y) + f64::from(click.cursor[1]),
            f64::from(p.z) + f64::from(click.cursor[2]),
        ],
        target,
        replace_clicked,
        held: click.held,
        sneaking: click.sneaking,
        yaw: click.yaw,
        pitch: click.pitch,
    }
}

/// Whether `state` at `target` leaves room for the players: its collision boxes
/// overlap none of `bodies`.
fn unobstructed(state: BlockState, target: BlockPos, bodies: &[[f64; 6]]) -> bool {
    let boxes = geometry::moved(state.props().collision_shape(), target);
    boxes
        .iter()
        .all(|placed| bodies.iter().all(|body| !geometry::overlaps(placed, body)))
}

/// Vanilla's `StandingAndWallBlockItem.getPlacementState`: the wall block's state is
/// worked out once; then along the looking directions, but for the one away from the
/// attachment, the attachment direction gives the standing block and a side the wall
/// one, and the first that can stay and is in no player's way wins.
fn standing_or_wall(
    standing: u16,
    wall: u16,
    attach: Dir,
    context: &Context,
    bodies: &[[f64; 6]],
) -> Option<BlockState> {
    let wall_state = rules::state_for_placement(data::default_state(wall), context);
    for dir in context.nearest_looking_directions() {
        if dir == attach.opposite() {
            continue;
        }
        let candidate = if dir == attach {
            rules::state_for_placement(data::default_state(standing), context)
        } else {
            wall_state
        };
        let Some(state) = candidate else {
            continue;
        };
        let holds = survive::can_survive(state, context.target, &context.world)
            && (rules::rule_for(state) != rules::Rule::WallHangingSign
                || rules::wall_hanging_sign_holds(state, context));
        if holds && unobstructed(state, context.target, bodies) {
            return Some(state);
        }
    }
    None
}

/// The blocks that change when the item of `click` is placed at `target`, or `None`
/// when nothing is placed (sections 4.1 to 4.4). `clicked` is the state of the
/// clicked block as the region that holds it read it; if `blocks` has the clicked
/// block, what it has wins. `bodies` are the boxes of the players other than the one
/// who clicks, `[min x, min y, min z, max x, max y, max z]`, which a placed block may
/// not overlap; the clicking player's own box is made from `click`.
pub fn place(
    click: &Click,
    target: BlockPos,
    replace_clicked: bool,
    clicked: BlockState,
    blocks: &dyn Blocks,
    bodies: &[[f64; 6]],
) -> Option<Vec<(BlockPos, BlockState)>> {
    let placing = placing(click.held, false)?;
    let clicked = blocks.block(click.position).unwrap_or(clicked);
    let context = context(click, target, replace_clicked, clicked, blocks);
    let at_target = context.world.known(target)?;
    if !replace_clicked && !replace::can_be_replaced(at_target, &context) {
        return None;
    }
    let mut players = Vec::with_capacity(bodies.len() + 1);
    players.push(body(click.feet, click.sneaking));
    players.extend_from_slice(bodies);
    let state = match placing {
        Placing::Block(block) => {
            let state = rules::state_for_placement(data::default_state(block), &context)?;
            if !survive::can_survive(state, target, &context.world)
                || !unobstructed(state, target, &players)
            {
                return None;
            }
            state
        }
        Placing::StandingOrWall {
            standing,
            wall,
            attach,
        } => standing_or_wall(standing, wall, attach, &context, &players)?,
    };
    let mut changes = vec![(target, state)];
    for (pos, more) in rules::more_blocks(state, &context)? {
        context.world.known(pos)?;
        changes.push((pos, more));
    }
    Some(changes)
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
    /// What the player carries, for an empty bucket's filled one.
    pub inventory: Inventory,
}

/// What the use of an item changes.
#[derive(Debug, Clone, PartialEq)]
pub struct UsedItem {
    /// The blocks that change, all or none.
    pub changes: Vec<(BlockPos, BlockState)>,
    /// A slot of the inventory that changes, numbered as [`Inventory`] numbers them,
    /// and what it holds then.
    pub slot: Option<(u8, Option<ItemStack>)>,
}

/// What the use of `item_use`'s item changes, or `None` if nothing does.
pub fn use_item(item_use: &ItemUse, blocks: &dyn Blocks, bodies: &[[f64; 6]]) -> Option<UsedItem> {
    bucket::use_item(item_use, blocks, bodies)
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
