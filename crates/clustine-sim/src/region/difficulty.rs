//! The effective difficulty (`DifficultyInstance`; `docs/groundwork/n9-plan.md`, section
//! 7): the world's difficulty, raised by the day's clock and the moon. At easy it lies
//! in [0.75, 1.125]; the inhabited time of a chunk is not kept (named, decision 8).

use clustine_world::{BlockPos, Difficulty};

use super::Region;

impl Region {
    /// `level.getCurrentDifficultyAt(pos).getEffectiveDifficulty()`: `id · (0.75 + t + c)`
    /// with `t` the clock past its first hour over 1 440 000 ticks, at most a quarter,
    /// and `c` the moon's brightness over four, at most `t`, halved at easy; 0 in
    /// peace. The position reads nothing but would the inhabited time.
    pub(crate) fn difficulty_at(&self, _pos: BlockPos) -> f32 {
        let base = Difficulty::WORLD;
        if base == Difficulty::Peaceful {
            return 0.0;
        }
        let day = self.clock.day;
        let level_time = day as f32;
        let t = ((level_time - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
        let mut c = (crate::mobs::making::moon_brightness(day) * 0.25).clamp(0.0, t);
        if base == Difficulty::Easy {
            c *= 0.5;
        }
        base.id() as f32 * (0.75 + t + c)
    }
}
