// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/mod.rs`.

//! `tree` (`TreeFeature`): roots, a trunk and foliage where there is room for them,
//! then the decorators, then the leaves' distances and the shapes at the tree's
//! edge.

use clustine_world::BlockPos;

use super::common::{Placement, is_block, sorted_by_y};
use super::decorators::{Decorating, Decorator, decorate};
use super::foliage::FoliagePlacer;
use super::leaves::{Bounds, update_edges, update_leaves};
use super::roots::RootPlacer;
use super::size::FeatureSize;
use super::trunk::TrunkPlacer;
use crate::context::Context;
use crate::feature::Feature;
use crate::providers::Provider;
use crate::reader::{Read, Reader};

/// The providers of a tree's blocks.
#[derive(Debug, Clone)]
pub struct TreeBlocks {
    pub trunk: Provider,
    pub below_trunk: Provider,
    pub foliage: Provider,
}

#[derive(Debug)]
pub struct Tree {
    blocks: TreeBlocks,
    trunk: TrunkPlacer,
    foliage: FoliagePlacer,
    roots: Option<RootPlacer>,
    minimum_size: FeatureSize,
    decorators: Vec<Decorator>,
    ignore_vines: bool,
}

impl Tree {
    pub fn read(reader: &Reader) -> Read<Self> {
        let decorators = match reader.optional("decorators") {
            Some(list) => list
                .list()?
                .iter()
                .map(Decorator::read)
                .collect::<Read<_>>()?,
            None => Vec::new(),
        };
        let tree = Self {
            blocks: TreeBlocks {
                trunk: Provider::read(&reader.field("trunk_provider")?)?,
                below_trunk: Provider::read(&reader.field("below_trunk_provider")?)?,
                foliage: Provider::read(&reader.field("foliage_provider")?)?,
            },
            trunk: TrunkPlacer::read(&reader.field("trunk_placer")?)?,
            foliage: FoliagePlacer::read(&reader.field("foliage_placer")?)?,
            roots: match reader.optional("root_placer") {
                Some(roots) => Some(RootPlacer::read(&roots)?),
                None => None,
            },
            minimum_size: FeatureSize::read(&reader.field("minimum_size")?)?,
            decorators,
            ignore_vines: match reader.optional("ignore_vines") {
                Some(ignore) => ignore.boolean()?,
                None => false,
            },
        };
        reader.finish()?;
        Ok(tree)
    }

    /// `doPlace`: false, having placed nothing, where the tree does not fit.
    fn do_place(
        &self,
        context: &mut Context<'_, '_>,
        origin: BlockPos,
        placement: &mut Placement,
    ) -> bool {
        let tree_height = self.trunk.tree_height(&mut *context.random);
        let foliage_height = self
            .foliage
            .foliage_height(&mut *context.random, tree_height);
        let trunk_height = tree_height - foliage_height;
        let leaf_radius = self
            .foliage
            .foliage_radius(&mut *context.random, trunk_height);
        let trunk_origin = match &self.roots {
            Some(roots) => roots.trunk_origin(origin, &mut *context.random),
            None => origin,
        };
        let min_y = origin.y.min(trunk_origin.y);
        let max_y = origin.y.max(trunk_origin.y) + tree_height + 1;
        if min_y < context.level.min_y() + 1 || max_y > context.level.top_y() {
            return false;
        }
        let free_height = self.max_free_height(context, tree_height, trunk_origin);
        if free_height < tree_height
            && self
                .minimum_size
                .min_clipped_height()
                .is_none_or(|min| free_height < min)
        {
            return false;
        }
        if let Some(roots) = &self.roots
            && !roots.place(context, placement, origin, trunk_origin)
        {
            return false;
        }
        let attachments =
            self.trunk
                .place(context, &self.blocks, placement, free_height, trunk_origin);
        for attachment in attachments {
            self.foliage.create(
                context,
                &self.blocks,
                placement,
                attachment,
                foliage_height,
                leaf_radius,
            );
        }
        true
    }

    /// `getMaxFreeTreeHeight`: how tall the tree can grow before something is in its
    /// way, two less than where that is.
    fn max_free_height(&self, context: &mut Context<'_, '_>, height: i32, pos: BlockPos) -> i32 {
        for y in 0..=height + 1 {
            let radius = self.minimum_size.size_at_height(height, y);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let at = pos.offset(x, y, z);
                    if !self.trunk.is_free(context.level, at)
                        || !self.ignore_vines
                            && is_block(context.level.block(at), clustine_data::blocks::VINE)
                    {
                        return y - 2;
                    }
                }
            }
        }
        height
    }
}

impl Feature for Tree {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut placement = Placement::default();
        if !self.do_place(context, origin, &mut placement)
            || placement.trunks.is_empty() && placement.foliage.is_empty()
        {
            return false;
        }
        if !self.decorators.is_empty() {
            let mut with = Decorating {
                logs: sorted_by_y(&placement.trunks),
                leaves: sorted_by_y(&placement.foliage),
                roots: sorted_by_y(&placement.roots),
                decorations: Some(&mut placement.decorations),
            };
            decorate(context, &self.decorators, &mut with);
        }
        let Some(bounds) = Bounds::around(&placement) else {
            return false;
        };
        let shape = update_leaves(context.level, bounds, &placement);
        update_edges(context.level, &shape);
        true
    }
}
