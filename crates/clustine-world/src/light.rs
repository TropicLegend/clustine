//! Light levels.
//!
//! There is no lighting engine yet. Sky light is approximated per column: full light
//! above the highest block and darkness from there down. That is exact for terrain
//! without overhangs or transparent blocks, and too dark below overhangs, where light
//! would really spread sideways. Blocks that emit light are not taken into account.

use serde::{Deserialize, Serialize};

use crate::{Chunk, SECTION_SIZE};

/// Bytes in the light array of one section: 4 bits per block.
pub const LIGHT_ARRAY_LENGTH: usize = 2048;

/// The light of one section.
///
/// Light that is 0 everywhere is always [`SectionLight::Dark`] and light that is 15
/// everywhere always [`SectionLight::Full`], so that equal light is equal; reading
/// one that travelled makes it so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SectionLightRepr", into = "SectionLightRepr")]
pub enum SectionLight {
    /// The client's packet lists it in neither mask. For sky light above everything
    /// that could cast a shadow, the client assumes full light.
    Absent,
    /// Level 0 everywhere.
    Dark,
    /// Level 15 everywhere. Written as an array only in the client's packet.
    Full,
    /// Levels per block, 4 bits each, indexed by `y << 8 | z << 4 | x`; the block with
    /// the even index is in the low half of its byte.
    Levels(Box<[u8; LIGHT_ARRAY_LENGTH]>),
}

impl SectionLight {
    /// The light of `levels`, in the one form equal light has.
    pub fn of(levels: Box<[u8; LIGHT_ARRAY_LENGTH]>) -> Self {
        if levels.iter().all(|byte| *byte == 0) {
            Self::Dark
        } else if levels.iter().all(|byte| *byte == 0xff) {
            Self::Full
        } else {
            Self::Levels(levels)
        }
    }
}

#[derive(Serialize, Deserialize)]
enum SectionLightRepr {
    Absent,
    Dark,
    Full,
    Levels(#[serde(with = "crate::bytes")] Vec<u8>),
}

impl From<SectionLight> for SectionLightRepr {
    fn from(light: SectionLight) -> Self {
        match light {
            SectionLight::Absent => Self::Absent,
            SectionLight::Dark => Self::Dark,
            SectionLight::Full => Self::Full,
            SectionLight::Levels(levels) => Self::Levels(levels.to_vec()),
        }
    }
}

impl TryFrom<SectionLightRepr> for SectionLight {
    type Error = String;

    fn try_from(repr: SectionLightRepr) -> Result<Self, Self::Error> {
        Ok(match repr {
            SectionLightRepr::Absent => Self::Absent,
            SectionLightRepr::Dark => Self::Dark,
            SectionLightRepr::Full => Self::Full,
            SectionLightRepr::Levels(levels) => {
                let levels: Box<[u8; LIGHT_ARRAY_LENGTH]> = levels
                    .into_boxed_slice()
                    .try_into()
                    .map_err(|levels: Box<[u8]>| {
                        format!(
                            "a section's light is {LIGHT_ARRAY_LENGTH} bytes, not {}",
                            levels.len()
                        )
                    })?;
                Self::of(levels)
            }
        })
    }
}

/// The light a chunk carries: what the light rules of the version `made_by` give for
/// its blocks beside the neighbours it was made beside. `sky` and `block` each have
/// an entry for the section below the chunk, one for each section and one for the
/// section above, bottom first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Light {
    pub made_by: u16,
    /// The version of the generator that made the nine chunks the light was made
    /// from, or 0 where whoever made the light did not say. Whoever lights a chunk
    /// it has generated says so before it gives the chunk the light. It stays with
    /// the light through every change that keeps the light, also when the chunk is
    /// no longer as generated: an edge reads it to tell whether the chunks beside
    /// this one are still what the light was made beside
    /// (`docs/adr/0023-light-at-the-edge.md`, section 7).
    pub beside: u16,
    pub sky: Vec<SectionLight>,
    pub block: Vec<SectionLight>,
}

impl Light {
    /// The value of `made_by` for light that the official server made, in a world
    /// that was brought in from it.
    pub const IMPORTED: u16 = u16::MAX;
}

/// The sky light of `chunk`: one entry for the section below the column, one per
/// section, and one for the section above the column, from bottom to top.
pub fn sky_light(chunk: &Chunk) -> Vec<SectionLight> {
    let heights = chunk.surface_heights();
    let highest = usize::from(heights.iter().copied().max().unwrap_or(0));
    // Sections are listed up to one above the highest block, as the official server does.
    let lit_sections = if highest == 0 {
        0
    } else {
        (highest - 1) / SECTION_SIZE + 2
    };

    let mut light = vec![SectionLight::Dark];
    for section in 0..chunk.sections().len() + 1 {
        if section >= lit_sections {
            light.push(SectionLight::Absent);
            continue;
        }
        let bottom = section * SECTION_SIZE;
        let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
        let mut any = false;
        for y in 0..SECTION_SIZE {
            for (column, height) in heights.iter().enumerate() {
                if bottom + y >= usize::from(*height) {
                    let index = y << 8 | column;
                    levels[index / 2] |= 15 << (index % 2 * 4);
                    any = true;
                }
            }
        }
        light.push(if any {
            SectionLight::of(levels)
        } else {
            SectionLight::Dark
        });
    }
    light
}

#[cfg(test)]
mod tests {
    use clustine_data::{DIMENSION_TYPES, blocks};

    use super::*;
    use crate::Biome;

    fn empty() -> Chunk {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        Chunk::empty(overworld, Biome(0))
    }

    fn level(light: &SectionLight, x: usize, y: usize, z: usize) -> u8 {
        let SectionLight::Levels(levels) = light else {
            panic!("expected light levels, got {light:?}");
        };
        let index = y << 8 | z << 4 | x;
        levels[index / 2] >> (index % 2 * 4) & 15
    }

    #[test]
    fn empty_chunk_is_open_sky() {
        let light = sky_light(&empty());
        assert_eq!(light.len(), 26);
        assert_eq!(light[0], SectionLight::Dark);
        assert!(
            light[1..]
                .iter()
                .all(|light| *light == SectionLight::Absent)
        );
    }

    /// The shape the official server sends for a superflat world: darkness below the
    /// world, levels for the section with the ground and for the one above it.
    #[test]
    fn flat_ground_matches_the_official_layout() {
        let mut chunk = empty();
        for y in -64..-60 {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, y, z, blocks::STONE);
                }
            }
        }
        let light = sky_light(&chunk);
        assert_eq!(light[0], SectionLight::Dark);
        assert_eq!(level(&light[1], 7, 3, 7), 0);
        assert_eq!(level(&light[1], 7, 4, 7), 15);
        assert_eq!(light[2], SectionLight::Full);
        assert!(
            light[3..]
                .iter()
                .all(|light| *light == SectionLight::Absent)
        );
    }

    #[test]
    fn a_pillar_shades_only_its_own_column() {
        let mut chunk = empty();
        for y in -64..0 {
            chunk.set(3, y, 5, blocks::STONE);
        }
        let light = sky_light(&chunk);
        // Sections 0 to 3 hold the pillar; index 1 is section 0.
        for section in &light[1..=4] {
            assert_eq!(level(section, 3, 8, 5), 0);
            assert_eq!(level(section, 4, 8, 5), 15);
            assert_eq!(level(section, 3, 8, 6), 15);
        }
        assert_eq!(light[5], SectionLight::Full);
        assert_eq!(light[6], SectionLight::Absent);
    }

    #[test]
    fn a_full_section_below_the_surface_is_dark() {
        let mut chunk = empty();
        for y in -64..-32 {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, y, z, blocks::STONE);
                }
            }
        }
        let light = sky_light(&chunk);
        assert_eq!(light[1], SectionLight::Dark);
        assert_eq!(light[2], SectionLight::Dark);
        // Lit throughout, which is the one form of an array of fifteens.
        assert_eq!(light[3], SectionLight::Full);
        assert_eq!(light[4], SectionLight::Absent);
    }
}
