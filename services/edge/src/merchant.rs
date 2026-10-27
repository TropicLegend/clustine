//! A merchant's window at the edge (`docs/groundwork/n10-plan.md`, sections 8.2 and 11):
//! its offers as `merchant_offers` for the window the player's menu of a serial is shown
//! in. Part E1 fills it; the contract step N10c put the call where the event arrives.

use bytes::Bytes;
use clustine_world::trading::Offers;

use crate::menus::Menus;

/// What the region's offers of the menu `serial` are sent as: `merchant_offers` for the
/// window that shows it, or nothing where no such window is shown.
pub(crate) fn offers(
    menus: &Menus,
    serial: u64,
    offers: &Offers,
    level: i32,
    xp: i32,
    show_progress: bool,
    can_restock: bool,
) -> Option<Bytes> {
    let _ = (menus, serial, offers, level, xp, show_progress, can_restock);
    None
}
