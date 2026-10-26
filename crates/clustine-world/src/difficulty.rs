//! The world's difficulty (`docs/groundwork/n7k-plan.md`, section 10): easy, as a new
//! dedicated server of the game starts, and fixed. It decides the special multiplier of
//! every `finalizeSpawn`, which at easy is always 0, so that nothing a mob is made with is
//! ever enchanted; another difficulty would need the enchantment providers, and so none is
//! accepted until they are built. When one is, the difficulty becomes a setting the world
//! keeps beside its generator's identity, so that every worker reads the same.

/// The game's `Difficulty`, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Difficulty {
    Peaceful = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    /// The world's.
    pub const WORLD: Difficulty = Difficulty::Easy;

    /// Its id (`Difficulty.getId`), which the game's rules compute with.
    pub fn id(self) -> i32 {
        self as i32
    }
}
