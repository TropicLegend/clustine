//! A raid's bar at the edge (`docs/groundwork/n10-plan.md`, sections 9.4 and 11): each
//! change the region reports as a `boss_event`, red and notched in ten. Part E1 fills it;
//! the contract step N10c put the call where the event arrives.

use bytes::Bytes;
use clustine_sim::api::RaidBarChange;
use uuid::Uuid;

/// The `boss_event` of a change of the raid's bar `id`, if it is sent.
pub(crate) fn raid_bar(id: Uuid, change: RaidBarChange) -> Option<Bytes> {
    let _ = (id, change);
    None
}
