//! What spawning reads of blocks and entity types (`generated/spawning.rs`), as the game
//! answers it (`docs/groundwork/n7l-plan.md`, section 5.1, L0).

use crate::generated::spawning::{
    DANGER_SETS, DANGEROUS, PLACEMENTS, VALID_FOR_ALL, VALID_FOR_SOME,
};

/// `SpawnPlacementTypes`: where a type may be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    OnGround,
    InWater,
    InLava,
    NoRestrictions,
}

/// The height map a type is placed by (`SpawnPlacements.getHeightmapType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heightmap {
    MotionBlocking,
    MotionBlockingNoLeaves,
    WorldSurface,
    OceanFloor,
}

/// `BlockStateBase.isValidSpawn(level, pos, type)` of the state `state` (the block below
/// the mob) for the entity type `kind`.
pub fn is_valid_spawn(state: u32, kind: u16) -> bool {
    let run = VALID_FOR_ALL.partition_point(|(first, _)| *first <= state);
    if run > 0 && state < VALID_FOR_ALL[run - 1].1 {
        return true;
    }
    VALID_FOR_SOME
        .binary_search_by_key(&state, |(id, _)| *id)
        .is_ok_and(|index| VALID_FOR_SOME[index].1.contains(&kind))
}

/// `EntityType.isBlockDangerous(state)` of the entity type `kind`.
pub fn is_block_dangerous(kind: u16, state: u32) -> bool {
    let Some(&set) = DANGEROUS.get(usize::from(kind)) else {
        return false;
    };
    DANGER_SETS[usize::from(set)]
        .iter()
        .any(|(first, end)| (*first..*end).contains(&state))
}

/// `SpawnPlacements`' placement and height map of the entity type `kind`; a type not
/// registered is placed without restriction by `MOTION_BLOCKING_NO_LEAVES`.
pub fn placement(kind: u16) -> (Placement, Heightmap) {
    PLACEMENTS
        .get(usize::from(kind))
        .copied()
        .unwrap_or((Placement::NoRestrictions, Heightmap::MotionBlockingNoLeaves))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{blocks, entity_types};

    fn kind(id: i32) -> u16 {
        u16::try_from(id).expect("an entity type")
    }

    fn state(block: crate::BlockState) -> u32 {
        u32::from(block.0)
    }

    /// The brief's 8.5: a mob may spawn on a block whose top is sturdy and that does not
    /// shine too bright; leaves take only ocelots and parrots, ice only polar bears.
    #[test]
    fn grass_takes_every_mob_leaves_only_ocelots_and_parrots() {
        let grass = state(blocks::GRASS_BLOCK);
        assert!(is_valid_spawn(grass, kind(entity_types::COW)));
        assert!(is_valid_spawn(grass, kind(entity_types::ZOMBIE)));
        let leaves = state(blocks::OAK_LEAVES);
        assert!(is_valid_spawn(leaves, kind(entity_types::PARROT)));
        assert!(is_valid_spawn(leaves, kind(entity_types::OCELOT)));
        assert!(!is_valid_spawn(leaves, kind(entity_types::COW)));
        assert!(!is_valid_spawn(state(blocks::AIR), kind(entity_types::COW)));
    }

    /// `EntityType.isBlockDangerous`: magma burns what is not fire immune, and a type's
    /// `immuneTo` spares it (the fox and berry bushes).
    #[test]
    fn magma_is_dangerous_to_a_cow_and_not_to_a_strider() {
        let magma = state(blocks::MAGMA_BLOCK);
        assert!(is_block_dangerous(kind(entity_types::COW), magma));
        assert!(!is_block_dangerous(kind(entity_types::STRIDER), magma));
        let bush = state(crate::BlockState::parse("minecraft:sweet_berry_bush[age=0]").unwrap());
        assert!(is_block_dangerous(kind(entity_types::COW), bush));
        assert!(!is_block_dangerous(kind(entity_types::FOX), bush));
        assert!(!is_block_dangerous(
            kind(entity_types::COW),
            state(blocks::GRASS_BLOCK)
        ));
    }

    /// The brief's table of registrations (8.5).
    #[test]
    fn the_placements_are_the_games_registrations() {
        use Heightmap::*;
        use Placement::*;
        assert_eq!(
            placement(kind(entity_types::COW)),
            (OnGround, MotionBlockingNoLeaves)
        );
        assert_eq!(
            placement(kind(entity_types::OCELOT)),
            (OnGround, MotionBlocking)
        );
        assert_eq!(
            placement(kind(entity_types::PARROT)),
            (OnGround, MotionBlocking)
        );
        assert_eq!(
            placement(kind(entity_types::FOX)),
            (NoRestrictions, MotionBlockingNoLeaves)
        );
        assert_eq!(
            placement(kind(entity_types::PANDA)),
            (NoRestrictions, MotionBlockingNoLeaves)
        );
        assert_eq!(
            placement(kind(entity_types::STRIDER)),
            (InLava, MotionBlockingNoLeaves)
        );
        assert_eq!(placement(kind(entity_types::COD)).0, InWater);
    }
}
