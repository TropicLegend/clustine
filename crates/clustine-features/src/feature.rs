//! A configured feature (`ConfiguredFeature` in the game): a kind of feature with its
//! settings, which places itself at a position; and how one feature names another.

use std::fmt;

use clustine_world::BlockPos;
use clustine_worldgen_data::feature as data;

use crate::context::Context;
use crate::dispatch;
use crate::placement::PlacedFeature;
use crate::reader::{Read, Reader};

/// A configured feature.
pub trait Feature: fmt::Debug + Send + Sync {
    /// Places the feature at `origin`, as `ConfiguredFeature.place` does, and says
    /// whether it did: selectors and some kinds pass that on, and decide by it.
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool;
}

/// A configured feature as another names it, by name or written out in place.
#[derive(Debug)]
pub enum FeatureRef {
    /// An index into the configured features of the data.
    Named(usize),
    Inline(Box<dyn Feature>),
}

impl FeatureRef {
    /// Reads a configured feature where the game's codec takes a `Holder` of one: a
    /// name, or the feature written out.
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_text() {
            let name = reader.text()?;
            let (index, _) = data::configured(name)
                .ok_or_else(|| reader.error(format!("{name} is not a configured feature")))?;
            return Ok(FeatureRef::Named(index));
        }
        Ok(FeatureRef::Inline(read_configured(reader)?))
    }

    pub fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        match self {
            FeatureRef::Named(index) => {
                let feature = context.settings.configured(*index);
                feature.place(context, origin)
            }
            FeatureRef::Inline(feature) => feature.place(context, origin),
        }
    }
}

/// A placed feature as another names it, by name or written out in place.
#[derive(Debug)]
pub enum PlacedRef {
    /// An index into the placed features of the data.
    Named(usize),
    Inline(Box<PlacedFeature>),
}

impl PlacedRef {
    /// Reads a placed feature where the game's codec takes a `Holder` of one.
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_text() {
            let name = reader.text()?;
            let (index, _) = data::placed(name)
                .ok_or_else(|| reader.error(format!("{name} is not a placed feature")))?;
            return Ok(PlacedRef::Named(index));
        }
        Ok(PlacedRef::Inline(Box::new(PlacedFeature::read(reader)?)))
    }

    /// `PlacedFeature.place`: the modifiers and then the feature, with no
    /// placed feature on top for the modifier `biome` to ask about.
    pub fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        match self {
            PlacedRef::Named(index) => {
                let placed = context.settings.placed(*index);
                placed.place(context, origin)
            }
            PlacedRef::Inline(placed) => placed.place(context, origin),
        }
    }
}

/// Reads a configured feature written out: its `type` says which kind.
pub fn read_configured(reader: &Reader) -> Read<Box<dyn Feature>> {
    let kind = reader.kind()?;
    let feature = dispatch::read(kind, reader)?;
    reader.finish()?;
    Ok(feature)
}
