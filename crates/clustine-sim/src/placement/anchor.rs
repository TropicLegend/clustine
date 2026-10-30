//! A click on a respawn anchor (`RespawnAnchorBlock.useItemOn`, `useWithoutItem`;
//! `docs/groundwork/p-brief.md`, section 5.4): whether the anchor takes it, which the
//! region then does by its rule (`region/anchors.rs`), or passes it on to the item in
//! hand.
//!
//! Named difference: with glowstone in the off hand, a chargeable anchor passes the main
//! hand's click on to its item (`useItemOn` 34–64, `InteractionResult.PASS`), so that the
//! off hand charges it next. A [`Click`] does not say what the off hand holds, and the
//! off hand's click uses no block (`placement::click_with`), so that rule is not kept:
//! the main hand's click uses a charged anchor as it would without the glowstone.

use clustine_blocks::data::number;
use clustine_data::{BlockState, items};

use super::Click;

/// The most charges an anchor holds (`RespawnAnchorBlock.MAX_CHARGES`).
pub(crate) const MAX_CHARGES: u8 = 4;

/// The charges of the anchor `state`.
pub(crate) fn charges(state: BlockState) -> u8 {
    number(state, "charges").unwrap_or(0)
}

/// Whether the anchor `state` takes the click `click` with the main hand
/// (d RespawnAnchorBlock.txt 31–152): glowstone charges one that is not full
/// (`useItemOn`); anything else, an empty hand, or glowstone on a full one, tries the
/// empty hand (`TRY_WITH_EMPTY_HAND`), whose use (`useWithoutItem`) takes a charged
/// anchor and passes on an empty one.
pub(crate) fn takes(click: &Click, state: BlockState) -> bool {
    let charges = charges(state);
    if click.held == Some(items::GLOWSTONE) && charges < MAX_CHARGES {
        return true;
    }
    charges > 0
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::{BlockPos, Vec3};

    use super::*;
    use crate::api::Face;

    fn anchor(charges: u8) -> BlockState {
        clustine_blocks::data::set(blocks::RESPAWN_ANCHOR, "charges", &charges.to_string())
    }

    fn click(held: Option<i32>) -> Click {
        Click {
            position: BlockPos::new(0, 64, 0),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            held,
            feet: Vec3::new(0.5, 65.0, 2.5),
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    #[test]
    fn glowstone_is_taken_by_an_anchor_that_is_not_full() {
        for charges in 0..MAX_CHARGES {
            assert!(takes(&click(Some(items::GLOWSTONE)), anchor(charges)));
        }
    }

    #[test]
    fn glowstone_on_a_full_anchor_uses_it_as_an_empty_hand_would() {
        assert!(takes(&click(Some(items::GLOWSTONE)), anchor(MAX_CHARGES)));
    }

    #[test]
    fn an_empty_hand_or_another_item_uses_a_charged_anchor_and_passes_an_empty_one() {
        for held in [None, Some(items::STONE)] {
            assert!(!takes(&click(held), anchor(0)), "{held:?}");
            for charges in 1..=MAX_CHARGES {
                assert!(takes(&click(held), anchor(charges)), "{held:?} {charges}");
            }
        }
    }
}
