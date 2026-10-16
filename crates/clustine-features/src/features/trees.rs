//! The kinds of configured features that step F3 of `docs/groundwork/f-plan.md` builds
//! (`crate::dispatch::KINDS`): trees with every placer and decorator the data uses,
//! fallen trees, huge mushrooms and the azalea's root system. Each kind is a file of
//! `trees/`, with the parts trees share beside them.

use crate::feature::Feature;
use crate::reader::{Read, Reader};

pub mod block_entities;
pub mod common;
pub mod decorators;
pub mod fallen_tree;
pub mod foliage;
pub mod huge_mushroom;
pub mod leaves;
pub mod root_system;
pub mod roots;
pub mod size;
pub mod tree;
pub mod trunk;

#[cfg(test)]
mod tests;

/// Reads a configured feature of one of this step's kinds.
pub fn read(kind: &'static str, reader: &Reader) -> Read<Box<dyn Feature>> {
    use huge_mushroom::{Cap, HugeMushroom};
    Ok(match kind {
        "minecraft:tree" => Box::new(tree::Tree::read(reader)?),
        "minecraft:fallen_tree" => Box::new(fallen_tree::FallenTree::read(reader)?),
        "minecraft:huge_brown_mushroom" => Box::new(HugeMushroom::read(Cap::Brown, reader)?),
        "minecraft:huge_red_mushroom" => Box::new(HugeMushroom::read(Cap::Red, reader)?),
        "minecraft:root_system" => Box::new(root_system::RootSystem::read(reader)?),
        other => return Err(reader.error(format!("{other} is not a kind of step F3"))),
    })
}
