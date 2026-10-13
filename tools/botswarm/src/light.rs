//! The light a client's packets add up to.
//!
//! A chunk's packet carries its light, and a light update carries light for some of
//! the chunk's sections. What a client holds of a chunk's light is the first with every
//! later update laid over it:
//!
//! - **A section in a light mask is replaced** by the array that comes with it, and
//!   **a section in an empty mask becomes dark**. That is the layout's own meaning
//!   (`docs/protocol-26.3.md`, "Light masks cover sections + 2"), the same in both
//!   packets.
//! - **A section in neither mask of an update is left as it was. This is a guess**
//!   (G2 of `docs/adr/0023-light-at-the-edge.md`, section 8): the protocol document
//!   does not say it, and no client was watched. What speaks for it is from a
//!   server's side only: SteelMC's 26.3 branch builds an update from the sections
//!   whose light changed and no others
//!   (`steel-core/src/chunk/light/packet.rs`, `build_chunk_light_update_packet_for_sections`),
//!   which could not work if the client forgot the rest.
//! - **A chunk's packet begins anew**: nothing of the light held before it stays, the
//!   sections it does not name neither. For a chunk the client did not hold that is
//!   the only reading; for one it held it is a guess (G4), which no server this is
//!   used against puts to the test, since the official server sends no chunk twice
//!   and Clustine unloads one first.
//! - **What a section in no mask at all looks like is not said here.** The levels a
//!   bot reports are those its packets stated; [`ChunkLight::sky_read_from_above`] is
//!   the record's guess G3 for the rest, and is named as one.

use std::fmt;

use anyhow::{Result, ensure};
use clustine_protocol::packets::play::LightData;

/// Bytes in the light array of one section: four bits a block.
pub const LIGHT_ARRAY_LENGTH: usize = 2048;

/// Blocks along the edge of a section.
const EDGE: usize = 16;

/// The light of one section as a client's packets left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionLight {
    /// In neither mask of the chunk's packet nor of any update since.
    Unsaid,
    /// Last named in an empty mask: level 0 everywhere.
    Dark,
    /// Last named in a light mask: its array, four bits a block, indexed by
    /// `y << 8 | z << 4 | x`, the block with the even index in the low half of its
    /// byte.
    Levels(Vec<u8>),
}

impl SectionLight {
    /// The level at `x`, `y`, `z` within the section (each in `0..16`), if a packet
    /// said one.
    pub fn level(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        match self {
            Self::Unsaid => None,
            Self::Dark => Some(0),
            Self::Levels(levels) => {
                let index = (y % EDGE) << 8 | (z % EDGE) << 4 | (x % EDGE);
                Some(levels[index / 2] >> (index % 2 * 4) & 15)
            }
        }
    }

    /// The section's array as a server would have to send it to say the same: zeros
    /// for a dark one. `None` if no packet said anything of the section.
    pub fn levels(&self) -> Option<Vec<u8>> {
        match self {
            Self::Unsaid => None,
            Self::Dark => Some(vec![0; LIGHT_ARRAY_LENGTH]),
            Self::Levels(levels) => Some(levels.clone()),
        }
    }
}

/// Which light sections one packet named, as bits: bit 0 is the section below the
/// world, bit 1 the lowest section of the chunk, and so on up to the section above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Named {
    /// Sky light that came as an array.
    pub sky: u64,
    /// Sky light named as dark, in the empty mask.
    pub dark_sky: u64,
    /// Block light that came as an array.
    pub block: u64,
    /// Block light named as dark.
    pub dark_block: u64,
}

impl Named {
    /// What `light` names, for a chunk of `light_sections` light sections. Fails where
    /// the packet contradicts itself: a section beyond the chunk, one in both masks of
    /// a kind, or arrays that are not one of 2,048 bytes for every bit of their mask.
    pub fn of(light: &LightData, light_sections: usize) -> Result<Self> {
        let word = |mask: &[u64], what: &str| -> Result<u64> {
            let low = mask.first().copied().unwrap_or(0);
            ensure!(
                mask.iter().skip(1).all(|word| *word == 0)
                    && (light_sections >= 64 || low >> light_sections == 0),
                "the {what} mask {mask:x?} names a section beyond the {light_sections} \
                 light sections of the chunk"
            );
            Ok(low)
        };
        let named = Self {
            sky: word(&light.sky_mask, "sky light")?,
            dark_sky: word(&light.empty_sky_mask, "empty sky light")?,
            block: word(&light.block_mask, "block light")?,
            dark_block: word(&light.empty_block_mask, "empty block light")?,
        };
        for (mask, dark, arrays, what) in [
            (named.sky, named.dark_sky, &light.sky, "sky"),
            (named.block, named.dark_block, &light.block, "block"),
        ] {
            ensure!(
                mask & dark == 0,
                "{what} light sections {:#b} are named both with levels and as dark",
                mask & dark
            );
            ensure!(
                arrays.len() == mask.count_ones() as usize,
                "{} {what} light arrays came for the {} sections of the mask {mask:#b}",
                arrays.len(),
                mask.count_ones()
            );
            ensure!(
                arrays.iter().all(|array| array.len() == LIGHT_ARRAY_LENGTH),
                "a {what} light array is not {LIGHT_ARRAY_LENGTH} bytes long"
            );
        }
        Ok(named)
    }

    /// The sections whose sky light the packet spoke of, either way.
    pub fn any_sky(&self) -> u64 {
        self.sky | self.dark_sky
    }

    /// The sections whose block light the packet spoke of, either way.
    pub fn any_block(&self) -> u64 {
        self.block | self.dark_block
    }

    /// Whether the packet named no section at all.
    pub fn is_empty(&self) -> bool {
        self.any_sky() | self.any_block() == 0
    }
}

/// The numbers of the sections in `mask`, lowest first.
pub fn sections_of(mask: u64) -> Vec<usize> {
    (0..64).filter(|bit| mask >> bit & 1 == 1).collect()
}

impl fmt::Display for Named {
    /// "sky levels [1, 2], sky dark [0], block levels [], block dark [0, 1, 2]": light
    /// sections by number, 0 being the one below the world.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sky levels {:?}, sky dark {:?}, block levels {:?}, block dark {:?}",
            sections_of(self.sky),
            sections_of(self.dark_sky),
            sections_of(self.block),
            sections_of(self.dark_block)
        )
    }
}

/// The light of one chunk as packets left it: an entry for the section below the
/// chunk, one for each of its sections and one for the section above, bottom first,
/// for the sky and for blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    pub sky: Vec<SectionLight>,
    pub block: Vec<SectionLight>,
}

impl ChunkLight {
    /// Light of which no packet has said anything yet.
    pub fn unsaid(light_sections: usize) -> Self {
        Self {
            sky: vec![SectionLight::Unsaid; light_sections],
            block: vec![SectionLight::Unsaid; light_sections],
        }
    }

    /// The light of a chunk's packet.
    pub fn of(light: &LightData, light_sections: usize) -> Result<Self> {
        let mut said = Self::unsaid(light_sections);
        said.lay_over(light)?;
        Ok(said)
    }

    /// Lays a packet's light over this: a section in a light mask is replaced by its
    /// array, one in an empty mask becomes dark, and one in neither stays as it is
    /// (the module's note says which of these is a guess). Returns what the packet
    /// named; a packet that contradicts itself changes nothing.
    pub fn lay_over(&mut self, light: &LightData) -> Result<Named> {
        let named = Named::of(light, self.sky.len())?;
        for (sections, mask, dark, arrays) in [
            (&mut self.sky, named.sky, named.dark_sky, &light.sky),
            (&mut self.block, named.block, named.dark_block, &light.block),
        ] {
            // The arrays come in the order of their sections, lowest first.
            for (section, array) in sections_of(mask).into_iter().zip(arrays) {
                sections[section] = SectionLight::Levels(array.clone());
            }
            for section in sections_of(dark) {
                sections[section] = SectionLight::Dark;
            }
        }
        Ok(named)
    }

    /// This light with `later` laid over it, where `later` holds what packets since
    /// have named and [`SectionLight::Unsaid`] for the rest.
    pub fn with(mut self, later: &Self) -> Self {
        for (mine, theirs) in [(&mut self.sky, &later.sky), (&mut self.block, &later.block)] {
            for (section, light) in mine.iter_mut().zip(theirs) {
                if *light != SectionLight::Unsaid {
                    *section = light.clone();
                }
            }
        }
        self
    }

    /// The sky light a packet stated for the block at `x`, `z` (each in `0..16`) and
    /// `y` blocks above the bottom of the section below the chunk.
    pub fn sky(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        self.sky.get(y / EDGE)?.level(x, y, z)
    }

    /// The block light a packet stated there; see [`ChunkLight::sky`].
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        self.block.get(y / EDGE)?.level(x, y, z)
    }

    /// The sky light at a block as the record **guesses** a client reads it (G3 of
    /// `docs/adr/0023-light-at-the-edge.md`, section 8): from the section if a packet
    /// named it, else from the bottom layer of the nearest named section above, and
    /// full where none above is named.
    pub fn sky_read_from_above(&self, x: usize, y: usize, z: usize) -> u8 {
        if let Some(level) = self.sky(x, y, z) {
            return level;
        }
        self.sky
            .iter()
            .skip(y / EDGE + 1)
            .find_map(|section| section.level(x, 0, z))
            .unwrap_or(15)
    }
}

/// The light at one block as far as a client's packets stated it. `None` is a section
/// that no packet named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightAt {
    pub sky: Option<u8>,
    pub block: Option<u8>,
}

/// One light update as a bot was sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct LightUpdateSeen {
    /// The chunk it was for, as x and z.
    pub chunk: (i32, i32),
    /// Whether the bot held the chunk when the update came. A client can do nothing
    /// with one for a chunk it does not hold.
    pub held: bool,
    /// The sections it named.
    pub named: Named,
    /// How many packets of the play state had come before it: its place in what the
    /// server sent this client.
    pub packets_before: u64,
    /// How many of those were block updates, which places it among them.
    pub block_updates_before: u32,
    /// The packet's light, if the bot was asked to keep the arrays; see
    /// `Bot::keep_light_arrays`.
    pub light: Option<LightData>,
}

impl fmt::Display for LightUpdateSeen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "chunk ({}, {}){}, packet {} after {} block updates: {}",
            self.chunk.0,
            self.chunk.1,
            if self.held { "" } else { " WHICH IS NOT HELD" },
            self.packets_before + 1,
            self.block_updates_before,
            self.named
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 26 light sections of the overworld.
    const SECTIONS: usize = 26;

    fn array(level: u8) -> Vec<u8> {
        vec![level | level << 4; LIGHT_ARRAY_LENGTH]
    }

    /// The flat world's chunk as the official server sends it: sky light for the
    /// sections 1 and 2, the one below the world dark, and no block light anywhere.
    fn flat() -> LightData {
        LightData {
            sky_mask: vec![0b110],
            block_mask: vec![],
            empty_sky_mask: vec![0b001],
            empty_block_mask: vec![0b111],
            sky: vec![array(7), array(15)],
            block: vec![],
        }
    }

    #[test]
    fn a_chunk_packet_states_the_sections_of_its_masks_and_no_others() {
        let light = ChunkLight::of(&flat(), SECTIONS).unwrap();
        assert_eq!(light.sky[0], SectionLight::Dark);
        assert_eq!(light.sky[1], SectionLight::Levels(array(7)));
        assert_eq!(light.sky[2], SectionLight::Levels(array(15)));
        assert!(light.sky[3..].iter().all(|s| *s == SectionLight::Unsaid));
        assert!(light.block[..3].iter().all(|s| *s == SectionLight::Dark));
        assert!(light.block[3..].iter().all(|s| *s == SectionLight::Unsaid));

        // Heights count from the bottom of the section below the world.
        assert_eq!(light.sky(0, 5, 0), Some(0));
        assert_eq!(light.sky(3, 16, 9), Some(7));
        assert_eq!(light.sky(3, 47, 9), Some(15));
        assert_eq!(light.sky(3, 48, 9), None);
        assert_eq!(light.block(3, 20, 9), Some(0));
        assert_eq!(light.block(3, 48, 9), None);
        assert_eq!(light.sky(3, 26 * 16, 9), None);
    }

    #[test]
    fn a_level_is_read_from_the_low_half_of_its_byte_for_an_even_block() {
        let mut levels = vec![0; LIGHT_ARRAY_LENGTH];
        // The block at x 2, y 1, z 3 has the index 0x132, the low half of byte 0x99;
        // its neighbour at x 3 the high half.
        levels[0x99] = 0xA4;
        let section = SectionLight::Levels(levels);
        assert_eq!(section.level(2, 1, 3), Some(4));
        assert_eq!(section.level(3, 1, 3), Some(10));
        assert_eq!(section.level(4, 1, 3), Some(0));
    }

    #[test]
    fn an_update_replaces_the_sections_it_names_and_leaves_the_rest() {
        let mut light = ChunkLight::of(&flat(), SECTIONS).unwrap();
        let named = light
            .lay_over(&LightData {
                sky_mask: vec![0b010],
                block_mask: vec![0b1010],
                empty_sky_mask: vec![],
                empty_block_mask: vec![],
                sky: vec![array(3)],
                block: vec![array(14), array(9)],
            })
            .unwrap();
        assert_eq!(
            named,
            Named {
                sky: 0b010,
                dark_sky: 0,
                block: 0b1010,
                dark_block: 0
            }
        );
        // Named: replaced, the arrays in the order of their sections.
        assert_eq!(light.sky[1], SectionLight::Levels(array(3)));
        assert_eq!(light.block[1], SectionLight::Levels(array(14)));
        assert_eq!(light.block[3], SectionLight::Levels(array(9)));
        // In neither mask: as the chunk's packet left them.
        assert_eq!(light.sky[0], SectionLight::Dark);
        assert_eq!(light.sky[2], SectionLight::Levels(array(15)));
        assert_eq!(light.block[0], SectionLight::Dark);
        assert_eq!(light.block[2], SectionLight::Dark);
        assert_eq!(light.sky[3], SectionLight::Unsaid);
    }

    #[test]
    fn a_section_named_in_an_empty_mask_becomes_dark() {
        let mut light = ChunkLight::of(&flat(), SECTIONS).unwrap();
        let named = light
            .lay_over(&LightData {
                empty_sky_mask: vec![0b10100],
                ..LightData::default()
            })
            .unwrap();
        assert_eq!(sections_of(named.any_sky()), [2, 4]);
        assert_eq!(named.any_block(), 0);
        // One that had levels and one of which nothing had been said.
        assert_eq!(light.sky[2], SectionLight::Dark);
        assert_eq!(light.sky[4], SectionLight::Dark);
        assert_eq!(light.sky[1], SectionLight::Levels(array(7)));
        assert_eq!(light.sky(0, 40, 0), Some(0));
    }

    #[test]
    fn of_two_updates_the_later_one_stands_where_both_name_a_section() {
        let mut light = ChunkLight::of(&flat(), SECTIONS).unwrap();
        for level in [4, 5] {
            light
                .lay_over(&LightData {
                    block_mask: vec![0b10],
                    block: vec![array(level)],
                    ..LightData::default()
                })
                .unwrap();
        }
        light
            .lay_over(&LightData {
                sky_mask: vec![0b100],
                sky: vec![array(1)],
                ..LightData::default()
            })
            .unwrap();
        assert_eq!(light.block[1], SectionLight::Levels(array(5)));
        assert_eq!(light.sky[2], SectionLight::Levels(array(1)));
    }

    #[test]
    fn updates_kept_apart_from_the_chunk_add_up_to_the_same() {
        let updates = [
            LightData {
                sky_mask: vec![0b010],
                sky: vec![array(3)],
                empty_block_mask: vec![0b1000],
                ..LightData::default()
            },
            LightData {
                empty_sky_mask: vec![0b010],
                block_mask: vec![0b1],
                block: vec![array(2)],
                ..LightData::default()
            },
        ];
        let mut at_once = ChunkLight::of(&flat(), SECTIONS).unwrap();
        let mut later = ChunkLight::unsaid(SECTIONS);
        for update in &updates {
            at_once.lay_over(update).unwrap();
            later.lay_over(update).unwrap();
        }
        let summed = ChunkLight::of(&flat(), SECTIONS).unwrap().with(&later);
        assert_eq!(summed, at_once);
        assert_eq!(summed.sky[1], SectionLight::Dark);
        assert_eq!(summed.block[0], SectionLight::Levels(array(2)));
        assert_eq!(summed.block[3], SectionLight::Dark);
    }

    #[test]
    fn a_section_no_packet_named_is_read_from_the_one_above_by_the_guess_alone() {
        let mut light = ChunkLight::of(&flat(), SECTIONS).unwrap();
        // Levels for section 5, whose bottom layer is 11 and the rest 2.
        let mut levels = array(2);
        levels[..EDGE * EDGE / 2].fill(0xBB);
        light
            .lay_over(&LightData {
                sky_mask: vec![1 << 5],
                sky: vec![levels],
                ..LightData::default()
            })
            .unwrap();
        // Sections 3 and 4 were never named.
        assert_eq!(light.sky(7, 3 * 16 + 4, 7), None);
        assert_eq!(light.sky_read_from_above(7, 3 * 16 + 4, 7), 11);
        assert_eq!(light.sky_read_from_above(7, 4 * 16 + 15, 7), 11);
        // A named section is read as it is, and above every named one is full.
        assert_eq!(light.sky_read_from_above(7, 5 * 16 + 1, 7), 2);
        assert_eq!(light.sky_read_from_above(7, 6 * 16, 7), 15);
    }

    #[test]
    fn a_packet_that_contradicts_itself_is_refused_and_changes_nothing() {
        let before = ChunkLight::of(&flat(), SECTIONS).unwrap();
        let faults = [
            // An array too few.
            LightData {
                sky_mask: vec![0b110],
                sky: vec![array(1)],
                ..LightData::default()
            },
            // An array that is not a section's.
            LightData {
                block_mask: vec![0b1],
                block: vec![vec![0; 100]],
                ..LightData::default()
            },
            // A section beyond the 26.
            LightData {
                empty_sky_mask: vec![1 << 26],
                ..LightData::default()
            },
            LightData {
                empty_block_mask: vec![0, 1],
                ..LightData::default()
            },
            // Levels and dark at once.
            LightData {
                sky_mask: vec![0b10],
                empty_sky_mask: vec![0b10],
                sky: vec![array(1)],
                ..LightData::default()
            },
        ];
        for fault in faults {
            let mut light = before.clone();
            assert!(light.lay_over(&fault).is_err(), "{fault:?}");
            assert_eq!(light, before);
        }
    }

    #[test]
    fn what_a_packet_named_reads_as_section_numbers() {
        let named = Named::of(&flat(), SECTIONS).unwrap();
        assert_eq!(
            named.to_string(),
            "sky levels [1, 2], sky dark [0], block levels [], block dark [0, 1, 2]"
        );
        assert!(!named.is_empty());
        assert!(
            Named::of(&LightData::default(), SECTIONS)
                .unwrap()
                .is_empty()
        );
    }
}
