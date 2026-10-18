// Adapted from SteelMC's `canBeReplaced` of the blocks under
// `steel-core/src/behavior/blocks/` (snow layers, candles, sea pickles, turtle eggs,
// segmentable plants, multiface blocks, the bodies of growing plants).

//! Vanilla's `canBeReplaced(state, context)`: whether a block gives way to the one
//! placed (section 4.2).

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState};

use super::context::Context;
use super::data;

/// The classes with a rule of their own for giving way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Replace {
    SnowLayer,
    Slab,
    /// Candles, sea pickles, turtle eggs, flower beds and leaf litter: one more of the
    /// same kind goes in, up to the most the block holds.
    Additive,
    Multiface,
    /// The body of a weeping or twisting vine, kelp or cave vines: the item of its
    /// head does not replace it.
    GrowingPlantBody,
    Fluid,
}

/// The rule of the nearest class that has one.
pub(crate) fn replace_rule(state: BlockState) -> Option<Replace> {
    data::nearest(data::info(state), |class| match class {
        BlockClass::SnowLayerBlock => Some(Replace::SnowLayer),
        BlockClass::SlabBlock => Some(Replace::Slab),
        BlockClass::CandleBlock
        | BlockClass::SeaPickleBlock
        | BlockClass::TurtleEggBlock
        | BlockClass::FlowerBedBlock
        | BlockClass::LeafLitterBlock => Some(Replace::Additive),
        BlockClass::MultifaceBlock => Some(Replace::Multiface),
        BlockClass::GrowingPlantBodyBlock => Some(Replace::GrowingPlantBody),
        BlockClass::LiquidBlock => Some(Replace::Fluid),
        _ => None,
    })
}

/// The property that counts how many an additive block holds, and the most it holds.
pub(crate) fn additive_count(state: BlockState) -> Option<(&'static str, u8)> {
    [
        "candles",
        "pickles",
        "eggs",
        "flower_amount",
        "segment_amount",
    ]
    .into_iter()
    .find(|name| state.property(name).is_some())
    .map(|name| (name, 4))
}

/// Whether `state` at the context's target gives way to what the context places.
pub(crate) fn can_be_replaced(state: BlockState, context: &Context) -> bool {
    let own_item = data::is_item_of(context.held, state);
    match replace_rule(state) {
        None => default_rule(state, context),
        Some(Replace::Fluid) => true,
        Some(Replace::SnowLayer) => {
            let layers = data::number(state, "layers").unwrap_or(1);
            if !own_item || layers >= 8 {
                layers == 1
            } else if context.replace_clicked {
                context.face == Dir::Up
            } else {
                true
            }
        }
        Some(Replace::Slab) => {
            let kind = state.property("type");
            if kind == Some("double") || !own_item {
                return false;
            }
            if !context.replace_clicked {
                return true;
            }
            let upper = context.cursor_height() > 0.5;
            let side = context.face != Dir::Up && context.face != Dir::Down;
            if kind == Some("bottom") {
                context.face == Dir::Up || upper && side
            } else {
                context.face == Dir::Down || !upper && side
            }
        }
        Some(Replace::Additive) => {
            let room = additive_count(state)
                .is_some_and(|(name, most)| data::number(state, name).unwrap_or(most) < most);
            (!context.sneaking && own_item && room) || default_rule(state, context)
        }
        Some(Replace::Multiface) => {
            let vacant = Dir::ALL
                .into_iter()
                .any(|dir| state.property(super::geometry::DirExt::name(dir)) == Some("false"));
            !own_item || vacant
        }
        Some(Replace::GrowingPlantBody) => {
            default_rule(state, context) && !holds_head_item(state, context.held)
        }
    }
}

/// `canBeReplaced` with an empty hand, as a falling block asks it where it lands
/// (`DirectionalPlaceContext` with `ItemStack.EMPTY`): every rule of a class that asks
/// whether the hand holds the block's own item takes the answer for an empty hand.
pub(crate) fn can_be_replaced_empty_handed(state: BlockState) -> bool {
    match replace_rule(state) {
        None | Some(Replace::Additive | Replace::GrowingPlantBody) => {
            state.props().can_be_replaced()
        }
        Some(Replace::Fluid | Replace::Multiface) => true,
        Some(Replace::SnowLayer) => data::number(state, "layers").unwrap_or(1) == 1,
        Some(Replace::Slab) => false,
    }
}

/// Vanilla's default: the state's own `canBeReplaced()`, unless the item in hand is the
/// block's own.
fn default_rule(state: BlockState, context: &Context) -> bool {
    state.props().can_be_replaced() && !data::is_item_of(context.held, state)
}

/// Whether `held` places the head of the growing plant whose body `state` is: its name
/// without `_plant`.
fn holds_head_item(state: BlockState, held: Option<i32>) -> bool {
    let Some(name) = state
        .block()
        .and_then(|block| block.name.strip_suffix("_plant"))
    else {
        return false;
    };
    BlockState::parse(name).is_some_and(|head| data::is_item_of(held, head))
}
