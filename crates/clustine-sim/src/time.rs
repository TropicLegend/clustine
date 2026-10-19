//! The overworld's day as the game's `day` timeline drives what a server reads of it
//! (`docs/groundwork/n7d-brief.md`, sections 1.4 and 7.4): the sky's light level and the
//! darkness a bed reads, from the overworld clock's ticks, in clear weather (Clustine has
//! no other).

/// The ticks of a day (`timeline/day.json`'s `period_ticks`).
pub const DAY: u64 = 24_000;

/// The sky light's factor's keyframes: (tick of the day, factor), linear between them and
/// round the day's end.
const SKY_LIGHT: [(i64, f32); 4] = [
    (133, 1.0),
    (11_867, 1.0),
    (13_670, 0.266_666_68),
    (22_330, 0.266_666_68),
];

/// `gameplay/sky_light_level` at the overworld clock's `day` ticks: 15 times the
/// keyframed factor, in the game's float arithmetic (`KeyframeTrackSampler`,
/// `FloatModifier.MULTIPLY`).
pub fn sky_light_level(day: u64) -> f32 {
    let t = (day % DAY) as i64;
    let period = DAY as i64;
    // The keyframes before and after `t`, the last of the day before and the first of
    // the next standing in round the ends.
    let (mut from, mut to) = (SKY_LIGHT[3], SKY_LIGHT[0]);
    from.0 -= period;
    for pair in SKY_LIGHT.windows(2) {
        if pair[0].0 <= t && t < pair[1].0 {
            (from, to) = (pair[0], pair[1]);
        }
    }
    if t >= SKY_LIGHT[3].0 {
        from = SKY_LIGHT[3];
        to = (SKY_LIGHT[0].0 + period, SKY_LIGHT[0].1);
    }
    let alpha = (t - from.0) as f32 / (to.0 - from.0) as f32;
    let factor = from.1 + alpha * (to.1 - from.1);
    (15.0_f32 * factor).clamp(0.0, 15.0)
}

/// `Level.skyDarken` (`updateSkyBrightness`): how far below full the sky's light is.
pub fn sky_darken(day: u64) -> i32 {
    (15.0_f32 - sky_light_level(day)) as i32
}

/// `Level.isDarkOutside`: what a bed's rule `when_dark` asks.
pub fn is_dark(day: u64) -> bool {
    sky_darken(day) >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bed_sleeps_from_tick_12523_to_tick_23477_of_a_clear_day() {
        let dark: Vec<u64> = (0..DAY).filter(|t| is_dark(*t)).collect();
        assert_eq!(dark.first(), Some(&12_523));
        assert_eq!(dark.last(), Some(&23_477));
        assert_eq!(dark.len() as u64, 23_477 - 12_523 + 1);
        // Every day alike.
        assert!(is_dark(5 * DAY + 12_523) && !is_dark(5 * DAY + 12_522));
    }

    #[test]
    fn the_sky_darkens_a_level_every_164_ticks_from_12031_and_is_darkest_in_the_night() {
        assert_eq!(sky_darken(12_030), 0);
        assert_eq!(sky_darken(12_031), 1);
        assert_eq!(sky_darken(12_195), 2);
        assert_eq!(sky_darken(12_359), 3);
        assert_eq!(sky_darken(12_523), 4);
        assert_eq!(sky_darken(13_670), 11);
        assert_eq!(sky_darken(22_330), 11);
        assert_eq!(sky_darken(22_331), 10);
        assert_eq!(sky_darken(23_478), 3);
        assert_eq!(sky_darken(23_970), 0);
        // The morning a sleep moves to is bright.
        assert_eq!(sky_darken(0), 0);
        assert_eq!(sky_light_level(18_000), 4.0);
    }
}
