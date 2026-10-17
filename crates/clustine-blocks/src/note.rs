//! Note blocks' instruments (`docs/adr/0026-redstone.md`, step R3b): the game's
//! `NoteBlockInstrument`, which instrument a block gives the note block above it
//! (`BlockBehaviour.Properties.instrument`), and `NoteBlock.setInstrument`, which a
//! note block is placed by and takes its shape again by when the block above or below
//! it changes.
//!
//! `clustine-data` does not carry a block's instrument, so the table in
//! `note/instruments.rs` was read from the 26.3 jar's own `Blocks.<clinit>`: each
//! block's last `.instrument(…)`, or that of the block whose properties it copies
//! (`ofFullCopy`, `ofLegacyCopy`, and the stairs, slabs and walls registered from a
//! base block), or of the colour and weathering families' own property makers. A
//! block not in it plays the harp, as the game's default is.

mod instruments;

use clustine_data::{BlockState, Face, sounds};
use clustine_world::BlockPos;

use crate::BlockView;
use crate::data::{is_class, relative, set};

/// `NoteBlockInstrument`, in the game's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instrument {
    Harp,
    Basedrum,
    Snare,
    Hat,
    Bass,
    Flute,
    Bell,
    Guitar,
    Chime,
    Xylophone,
    IronXylophone,
    CowBell,
    Didgeridoo,
    Bit,
    Banjo,
    Pling,
    Trumpet,
    TrumpetExposed,
    TrumpetOxidized,
    TrumpetWeathered,
    Zombie,
    Skeleton,
    Creeper,
    Dragon,
    WitherSkeleton,
    Piglin,
    CustomHead,
}

impl Instrument {
    /// The name the `instrument` property of a note block gives it.
    pub fn name(self) -> &'static str {
        use Instrument as I;
        match self {
            I::Harp => "harp",
            I::Basedrum => "basedrum",
            I::Snare => "snare",
            I::Hat => "hat",
            I::Bass => "bass",
            I::Flute => "flute",
            I::Bell => "bell",
            I::Guitar => "guitar",
            I::Chime => "chime",
            I::Xylophone => "xylophone",
            I::IronXylophone => "iron_xylophone",
            I::CowBell => "cow_bell",
            I::Didgeridoo => "didgeridoo",
            I::Bit => "bit",
            I::Banjo => "banjo",
            I::Pling => "pling",
            I::Trumpet => "trumpet",
            I::TrumpetExposed => "trumpet_exposed",
            I::TrumpetOxidized => "trumpet_oxidized",
            I::TrumpetWeathered => "trumpet_weathered",
            I::Zombie => "zombie",
            I::Skeleton => "skeleton",
            I::Creeper => "creeper",
            I::Dragon => "dragon",
            I::WitherSkeleton => "wither_skeleton",
            I::Piglin => "piglin",
            I::CustomHead => "custom_head",
        }
    }

    /// The instrument a note block of `state` plays.
    pub fn of_note_block(state: BlockState) -> Instrument {
        let name = state.property("instrument").unwrap_or("harp");
        ALL.into_iter()
            .find(|instrument| instrument.name() == name)
            .unwrap_or(Instrument::Harp)
    }

    /// `NoteBlockInstrument.isTunable`: an instrument of a block below, which plays
    /// the note; a head's sound has no note.
    pub fn is_tunable(self) -> bool {
        !self.works_above_note_block()
    }

    /// `NoteBlockInstrument.worksAboveNoteBlock`: a mob's head or a player's, which a
    /// note block takes from above it.
    pub fn works_above_note_block(self) -> bool {
        use Instrument as I;
        matches!(
            self,
            I::Zombie
                | I::Skeleton
                | I::Creeper
                | I::Dragon
                | I::WitherSkeleton
                | I::Piglin
                | I::CustomHead
        )
    }

    /// `NoteBlockInstrument.getSoundEvent`; `None` for a player's head, whose sound
    /// is the head's own (`hasCustomSound`).
    pub fn sound(self) -> Option<i32> {
        use Instrument as I;
        use sounds as s;
        Some(match self {
            I::Harp => s::BLOCK_NOTE_BLOCK_HARP,
            I::Basedrum => s::BLOCK_NOTE_BLOCK_BASEDRUM,
            I::Snare => s::BLOCK_NOTE_BLOCK_SNARE,
            I::Hat => s::BLOCK_NOTE_BLOCK_HAT,
            I::Bass => s::BLOCK_NOTE_BLOCK_BASS,
            I::Flute => s::BLOCK_NOTE_BLOCK_FLUTE,
            I::Bell => s::BLOCK_NOTE_BLOCK_BELL,
            I::Guitar => s::BLOCK_NOTE_BLOCK_GUITAR,
            I::Chime => s::BLOCK_NOTE_BLOCK_CHIME,
            I::Xylophone => s::BLOCK_NOTE_BLOCK_XYLOPHONE,
            I::IronXylophone => s::BLOCK_NOTE_BLOCK_IRON_XYLOPHONE,
            I::CowBell => s::BLOCK_NOTE_BLOCK_COW_BELL,
            I::Didgeridoo => s::BLOCK_NOTE_BLOCK_DIDGERIDOO,
            I::Bit => s::BLOCK_NOTE_BLOCK_BIT,
            I::Banjo => s::BLOCK_NOTE_BLOCK_BANJO,
            I::Pling => s::BLOCK_NOTE_BLOCK_PLING,
            I::Trumpet => s::BLOCK_NOTE_BLOCK_TRUMPET,
            I::TrumpetExposed => s::BLOCK_NOTE_BLOCK_TRUMPET_EXPOSED,
            I::TrumpetOxidized => s::BLOCK_NOTE_BLOCK_TRUMPET_OXIDIZED,
            I::TrumpetWeathered => s::BLOCK_NOTE_BLOCK_TRUMPET_WEATHERED,
            I::Zombie => s::BLOCK_NOTE_BLOCK_IMITATE_ZOMBIE,
            I::Skeleton => s::BLOCK_NOTE_BLOCK_IMITATE_SKELETON,
            I::Creeper => s::BLOCK_NOTE_BLOCK_IMITATE_CREEPER,
            I::Dragon => s::BLOCK_NOTE_BLOCK_IMITATE_ENDER_DRAGON,
            I::WitherSkeleton => s::BLOCK_NOTE_BLOCK_IMITATE_WITHER_SKELETON,
            I::Piglin => s::BLOCK_NOTE_BLOCK_IMITATE_PIGLIN,
            I::CustomHead => return None,
        })
    }
}

/// Every instrument, in the game's order.
const ALL: [Instrument; 27] = {
    use Instrument as I;
    [
        I::Harp,
        I::Basedrum,
        I::Snare,
        I::Hat,
        I::Bass,
        I::Flute,
        I::Bell,
        I::Guitar,
        I::Chime,
        I::Xylophone,
        I::IronXylophone,
        I::CowBell,
        I::Didgeridoo,
        I::Bit,
        I::Banjo,
        I::Pling,
        I::Trumpet,
        I::TrumpetExposed,
        I::TrumpetOxidized,
        I::TrumpetWeathered,
        I::Zombie,
        I::Skeleton,
        I::Creeper,
        I::Dragon,
        I::WitherSkeleton,
        I::Piglin,
        I::CustomHead,
    ]
};

/// `BlockState.instrument`: what a block of `state` gives a note block, the harp for
/// a block the table does not name.
pub fn instrument(state: BlockState) -> Instrument {
    let Some(block) = state.block() else {
        return Instrument::Harp;
    };
    instruments::INSTRUMENTS
        .binary_search_by(|(name, _)| (*name).cmp(block.name))
        .map_or(Instrument::Harp, |found| instruments::INSTRUMENTS[found].1)
}

/// Whether `state` is a note block.
pub fn is_note_block(state: BlockState) -> bool {
    is_class(state, clustine_data::BlockClass::NoteBlock)
}

/// `NoteBlock.setInstrument`: the note block `state` at `pos` takes the instrument of
/// a head above it, or else of the block below it; a head below plays the harp.
pub fn with_instrument<V: BlockView + ?Sized>(
    view: &mut V,
    pos: BlockPos,
    state: BlockState,
) -> BlockState {
    let above = instrument(view.block(relative(pos, Face::Up)));
    if above.works_above_note_block() {
        return set(state, "instrument", above.name());
    }
    let below = instrument(view.block(relative(pos, Face::Down)));
    let played = if below.works_above_note_block() {
        Instrument::Harp
    } else {
        below
    };
    set(state, "instrument", played.name())
}

/// `NoteBlock.getPitchFromNote`: `2^((note - 12) / 12)`.
pub fn pitch(note: u8) -> f32 {
    2f64.powf((f64::from(note) - 12.0) / 12.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::blocks;

    fn of(name: &str) -> Instrument {
        instrument(BlockState::parse(name).expect("a block"))
    }

    #[test]
    fn the_table_is_sorted_so_that_it_can_be_searched() {
        let names: Vec<&str> = instruments::INSTRUMENTS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
        for name in names {
            assert!(
                clustine_data::block_id_by_name(name).is_some(),
                "{name} is a block"
            );
        }
    }

    #[test]
    fn blocks_give_the_instruments_the_game_gives_them() {
        assert_eq!(instrument(blocks::STONE), Instrument::Basedrum);
        assert_eq!(instrument(blocks::AIR), Instrument::Harp);
        assert_eq!(instrument(blocks::DIRT), Instrument::Harp);
        assert_eq!(of("minecraft:oak_planks"), Instrument::Bass);
        assert_eq!(of("minecraft:oak_stairs"), Instrument::Bass);
        assert_eq!(of("minecraft:sand"), Instrument::Snare);
        assert_eq!(of("minecraft:glass"), Instrument::Hat);
        assert_eq!(of("minecraft:red_stained_glass"), Instrument::Hat);
        assert_eq!(of("minecraft:white_wool"), Instrument::Guitar);
        assert_eq!(of("minecraft:clay"), Instrument::Flute);
        assert_eq!(of("minecraft:gold_block"), Instrument::Bell);
        assert_eq!(of("minecraft:packed_ice"), Instrument::Chime);
        assert_eq!(of("minecraft:bone_block"), Instrument::Xylophone);
        assert_eq!(of("minecraft:iron_block"), Instrument::IronXylophone);
        assert_eq!(of("minecraft:soul_sand"), Instrument::CowBell);
        assert_eq!(of("minecraft:pumpkin"), Instrument::Didgeridoo);
        assert_eq!(of("minecraft:emerald_block"), Instrument::Bit);
        assert_eq!(of("minecraft:hay_block"), Instrument::Banjo);
        assert_eq!(of("minecraft:glowstone"), Instrument::Pling);
        assert_eq!(of("minecraft:copper_block"), Instrument::Trumpet);
        assert_eq!(
            of("minecraft:waxed_oxidized_cut_copper"),
            Instrument::TrumpetOxidized
        );
        assert_eq!(of("minecraft:zombie_head"), Instrument::Zombie);
        assert_eq!(of("minecraft:black_concrete_powder"), Instrument::Snare);
    }

    #[test]
    fn the_pitch_doubles_over_twelve_notes() {
        assert_eq!(pitch(12), 1.0);
        assert_eq!(pitch(0), 0.5);
        assert_eq!(pitch(24), 2.0);
    }
}
