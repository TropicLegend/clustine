//! A placed feature (`PlacedFeature` in the game): a configured feature with the
//! modifiers that choose where it is placed.

use std::fmt;

use clustine_world::BlockPos;

use crate::context::Context;
use crate::feature::FeatureRef;
use crate::reader::{Read, Reader};

/// A placement modifier (`PlacementModifier`).
pub trait Modifier: fmt::Debug + Send + Sync {
    /// The positions the modifier gives for `origin`, in order, as its
    /// `getPositions` makes them. It is called when the game's stream would call
    /// `getPositions`, so the random numbers it draws are drawn at the same moment;
    /// see [`PlacedFeature::place`].
    fn positions(&self, context: &mut Context<'_, '_>, origin: BlockPos, out: &mut Vec<BlockPos>);
}

/// A configured feature and its modifiers.
#[derive(Debug)]
pub struct PlacedFeature {
    pub feature: FeatureRef,
    pub modifiers: Vec<Box<dyn Modifier>>,
}

impl PlacedFeature {
    /// Reads a placed feature written out: `feature` and `placement`.
    pub fn read(reader: &Reader) -> Read<Self> {
        let feature = FeatureRef::read(&reader.field("feature")?)?;
        let modifiers = reader
            .field("placement")?
            .list()?
            .iter()
            .map(read_modifier)
            .collect::<Read<Vec<_>>>()?;
        reader.finish()?;
        Ok(Self { feature, modifiers })
    }

    /// Places the feature at every position its modifiers give, as
    /// `PlacedFeature.placeWithContext` does, and says whether it was placed at any.
    ///
    /// The game chains the modifiers through lazy streams (`Stream.flatMap`), and a
    /// stream ending in `forEach` takes each position all the way to the feature
    /// before the modifier that gave it is asked for its next. So the order is depth
    /// first: each position a modifier gives goes through the modifiers after it and
    /// is placed before the next is taken, and a modifier is asked about a position
    /// only when it arrives. The random numbers are drawn in that order.
    pub fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        self.through(context, 0, origin)
    }

    fn through(&self, context: &mut Context<'_, '_>, modifier: usize, at: BlockPos) -> bool {
        let Some(next) = self.modifiers.get(modifier) else {
            return self.feature.place(context, at);
        };
        let mut positions = Vec::new();
        next.positions(context, at, &mut positions);
        let mut placed = false;
        for position in positions {
            placed |= self.through(context, modifier + 1, position);
        }
        placed
    }
}

/// Reads a placement modifier; its `type` says which.
pub fn read_modifier(reader: &Reader) -> Read<Box<dyn Modifier>> {
    let kind = reader.kind()?;
    let modifier = read_kind(kind, reader)?;
    reader.finish()?;
    Ok(modifier)
}

/// The modifiers are step F1b's. Until then a modifier gives no position.
fn read_kind(kind: &'static str, reader: &Reader) -> Read<Box<dyn Modifier>> {
    reader.skip_rest();
    Ok(Box::new(Unbuilt(kind)))
}

/// A modifier whose code is not built: it gives no position.
struct Unbuilt(&'static str);

impl fmt::Debug for Unbuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unbuilt({})", self.0)
    }
}

impl Modifier for Unbuilt {
    fn positions(&self, _: &mut Context<'_, '_>, _: BlockPos, _: &mut Vec<BlockPos>) {}
}
