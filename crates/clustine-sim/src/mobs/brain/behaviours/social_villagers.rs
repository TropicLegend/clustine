//! The villager's social behaviours (`docs/groundwork/n10-plan.md`, section 7): gossip,
//! trade between villagers, love, golems, bells, hiding and celebrating. Part V3 fills
//! it; the contract step N10c gave it the hooks other code calls, and each behaviour's
//! constructor with the game's arguments, inert, for part V1's packages to name.

#![allow(dead_code, reason = "part V1 names these in the villager's packages")]

use crate::mobs::Mob;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::brain::{Behaviour, MemoryKind, inert};
use crate::mobs::tick::MobTick;

/// `VillagerMakeLove`'s write into the partner at a birth, where the partner is held by
/// another region (`VillagerUse::Born`): it eats and digests its food, and its age
/// becomes 6000. `lived` is its record's.
pub(crate) fn born_as_partner(mob: &mut Mob, lived: u32) {
    let _ = (mob, lived);
}

/// After the villager's brain in `customServerAiStep`: a trade's `TRADE` reputation event
/// about the last player who traded, entity event 14, and the offers' prices again.
pub(crate) fn after_brain(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// `Villager.die`: its witnesses (the living it sees within 16) gossip `major_negative`
/// about a murderer, and its points of interest are given back, by its locals.
pub(crate) fn died(struck: &mut Struck<'_>, source: &DamageSource) {
    let _ = (struck, source);
}

/// `setLastHurtByMob`'s `VILLAGER_HURT` reputation event about a living attacker.
pub(crate) fn hurt(struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
    let _ = (struck, source, took);
}

/// `TradeWithVillager` (meet 3, idle 3, in a gate erasing `interaction_target`).
pub(crate) fn trade_with_villager() -> Box<dyn Behaviour> {
    inert()
}

/// `SocializeAtBell.create()` (meet 2).
pub(crate) fn socialize_at_bell() -> Box<dyn Behaviour> {
    inert()
}

/// `InteractWith.of(kind, maxDistance, memory, speed, close)`: an entity of the type
/// `kind` within `max_distance` becomes the `memory` target, walked to.
pub(crate) fn interact_with(
    kind: u16,
    max_distance: i32,
    memory: MemoryKind,
    speed: f32,
    close: i32,
) -> Box<dyn Behaviour> {
    let _ = (kind, max_distance, memory, speed, close);
    inert()
}

/// `InteractWith.of(VILLAGER, 8, Villager::canBreed, AgeableMob::canBreed, breed_target,
/// speed, close)` (idle 2): a partner for love.
pub(crate) fn interact_with_breedable(
    max_distance: i32,
    speed: f32,
    close: i32,
) -> Box<dyn Behaviour> {
    let _ = (max_distance, speed, close);
    inert()
}

/// `SetLookAndInteract.create(kind, maxDistance)`.
pub(crate) fn set_look_and_interact(kind: u16, max_distance: i32) -> Box<dyn Behaviour> {
    let _ = (kind, max_distance);
    inert()
}

/// `ShowTradesToPlayer(minDuration, maxDuration)`.
pub(crate) fn show_trades_to_player(min: i32, max: i32) -> Box<dyn Behaviour> {
    let _ = (min, max);
    inert()
}

/// `LookAndFollowTradingPlayerSink(speed)` (core 3).
pub(crate) fn look_and_follow_trading_player(speed: f32) -> Box<dyn Behaviour> {
    let _ = speed;
    inert()
}

/// `VillagerMakeLove` (idle 3, in a gate erasing `breed_target`).
pub(crate) fn villager_make_love() -> Box<dyn Behaviour> {
    inert()
}

/// `GiveGiftToHero(duration)`.
pub(crate) fn give_gift_to_hero(duration: i32) -> Box<dyn Behaviour> {
    let _ = duration;
    inert()
}

/// `RingBell.create()` (pre-raid 0).
pub(crate) fn ring_bell() -> Box<dyn Behaviour> {
    inert()
}

/// `ReactToBell.create()` (core 0).
pub(crate) fn react_to_bell() -> Box<dyn Behaviour> {
    inert()
}

/// `SetHiddenState.create(seconds, close)` (hide 0).
pub(crate) fn set_hidden_state(seconds: i32, close: i32) -> Box<dyn Behaviour> {
    let _ = (seconds, close);
    inert()
}

/// `LocateHidingPlace.create(radius, speed, close)`.
pub(crate) fn locate_hiding_place(radius: i32, speed: f32, close: i32) -> Box<dyn Behaviour> {
    let _ = (radius, speed, close);
    inert()
}

/// `CelebrateVillagersSurvivedRaid(min, max)` (raid 0).
pub(crate) fn celebrate_villagers_survived_raid(min: i32, max: i32) -> Box<dyn Behaviour> {
    let _ = (min, max);
    inert()
}
