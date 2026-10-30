//! A click on a respawn anchor (`RespawnAnchorBlock.useItemOn`, `useWithoutItem`;
//! `docs/groundwork/p-brief.md`, section 5.4): whether the anchor takes it, which the
//! region then does by its rule (`region/anchors.rs`), or passes it on to the item in
//! hand. Family C of `docs/groundwork/p-plan.md`, section 17, fills this.

use clustine_data::BlockState;

use super::Click;

/// Whether the anchor `state` takes the click `click`: glowstone charges one that is
/// not full; an empty hand, or anything else, uses a charged one. Until family C builds
/// it, an anchor takes nothing.
pub(crate) fn takes(click: &Click, state: BlockState) -> bool {
    let _ = (click, state);
    false
}
