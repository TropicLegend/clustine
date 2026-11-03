//! How a block breaks: the hardness and the rule about tools the game's `Block` keeps
//! from its properties (`docs/groundwork/v-plan.md`, step V2). The time a player takes
//! is reckoned from these and the held item's `minecraft:tool`.

use crate::BlockState;
use crate::generated::block_breaking::{BLOCK_BREAKING, BREAKING};

/// The fields of a block that breaking reads, as the game holds them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlockBreaking {
    /// `defaultDestroyTime`: 1.5 for stone, 0 for what breaks at once, -1 for what
    /// does not break (bedrock, a portal).
    pub hardness: f32,
    /// `requiresCorrectToolForDrops`: only the tool the block's tags name makes it
    /// drop (stone, ores), and without it a player breaks it a third as fast.
    pub requires_correct_tool: bool,
}

/// How the block of this id breaks. Panics if there is no such block.
pub fn block_breaking(block_id: usize) -> &'static BlockBreaking {
    &BREAKING[usize::from(BLOCK_BREAKING[block_id])]
}

impl BlockState {
    /// How the block of this state breaks, or `None` if the id is not a block state.
    pub fn breaking(self) -> Option<&'static BlockBreaking> {
        Some(block_breaking(self.block_id()?))
    }
}

#[cfg(test)]
mod tests {
    use crate::{BlockState, blocks};

    fn breaking(name: &str) -> &'static super::BlockBreaking {
        BlockState::parse(name).unwrap().breaking().unwrap()
    }

    #[test]
    fn stone_obsidian_bedrock_and_a_flower_break_as_the_game_says() {
        assert_eq!(breaking("minecraft:stone").hardness, 1.5);
        assert!(breaking("minecraft:stone").requires_correct_tool);
        assert_eq!(breaking("minecraft:obsidian").hardness, 50.0);
        assert_eq!(breaking("minecraft:bedrock").hardness, -1.0);
        assert_eq!(breaking("minecraft:poppy").hardness, 0.0);
        assert!(!breaking("minecraft:dirt").requires_correct_tool);
        assert_eq!(breaking("minecraft:dirt").hardness, 0.5);
    }

    #[test]
    fn air_breaks_at_once() {
        assert_eq!(
            blocks::AIR.breaking().map(|breaking| breaking.hardness),
            Some(0.0)
        );
    }
}
