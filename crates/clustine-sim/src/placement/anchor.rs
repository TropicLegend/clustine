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
///
/// With `off_hand`, the click is the off hand's, which uses an anchor only to charge it;
/// a click of the main hand that holds no glowstone, while the off hand does and the
/// anchor can be charged, passes on to the item in hand, so that the off hand's click
/// that follows charges it (`useItemOn`'s first test, d RespawnAnchorBlock.txt 31–65).
pub(crate) fn takes(click: &Click, state: BlockState, off_hand: bool) -> bool {
    let charges = charges(state);
    let glowstone = Some(items::GLOWSTONE);
    let chargeable = charges < MAX_CHARGES;
    if off_hand {
        return click.held == glowstone && chargeable;
    }
    if click.held == glowstone && chargeable {
        return true;
    }
    if click.held != glowstone && click.other_hand == glowstone && chargeable {
        return false;
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
            other_hand: None,
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
            assert!(takes(
                &click(Some(items::GLOWSTONE)),
                anchor(charges),
                false
            ));
        }
    }

    #[test]
    fn glowstone_on_a_full_anchor_uses_it_as_an_empty_hand_would() {
        assert!(takes(
            &click(Some(items::GLOWSTONE)),
            anchor(MAX_CHARGES),
            false
        ));
    }

    #[test]
    fn an_empty_hand_or_another_item_uses_a_charged_anchor_and_passes_an_empty_one() {
        for held in [None, Some(items::STONE)] {
            assert!(!takes(&click(held), anchor(0), false), "{held:?}");
            for charges in 1..=MAX_CHARGES {
                assert!(
                    takes(&click(held), anchor(charges), false),
                    "{held:?} {charges}"
                );
            }
        }
    }

    /// `useItemOn`'s first test: a main hand without glowstone passes where the off hand
    /// holds it and the anchor can be charged; the off hand's click then charges it, and
    /// an off hand uses an anchor for nothing else.
    #[test]
    fn glowstone_in_the_off_hand_makes_the_main_hand_pass_and_charges_from_there() {
        let main_empty = Click {
            other_hand: Some(items::GLOWSTONE),
            ..click(None)
        };
        assert!(!takes(&main_empty, anchor(1), false), "passes");
        assert!(takes(&main_empty, anchor(MAX_CHARGES), false), "full: used");
        let off_glowstone = Click {
            other_hand: None,
            ..click(Some(items::GLOWSTONE))
        };
        assert!(takes(&off_glowstone, anchor(1), true));
        assert!(!takes(&off_glowstone, anchor(MAX_CHARGES), true));
        assert!(!takes(&click(None), anchor(2), true), "an empty off hand");
    }
}
