//! The end crystal (`EndCrystal`, d EndCrystal.txt; `docs/groundwork/p-brief.md`, section
//! 6.3): family E7 of the End's milestone (`docs/groundwork/p-plan.md`, sections 19.2,
//! 19.5 item 13 and 19.6). Its item is `end_crystal_item.rs`.
//!
//! **Its tick** (`tick`, 61–101): its age goes on (`time + 1`; the game's `time` starts
//! at a random number, is neither saved nor sent and is read only by the client's
//! renderer, so the record's `lived` stands for it), and in a level with a dragon fight,
//! the End, it sets fire (`BaseFireBlock.getState`) in its own block where that is air,
//! with `setBlockAndUpdate`. `applyEffectsFromBlocks` does nothing to it: the fire it
//! stands in cannot hurt it, as its type is fire immune; `handlePortal` is the shared
//! entities' phase's (`portals.rs`).
//!
//! **Its hurt** (`hurtServer`, 158–214): nothing that is fire hurts it (`isInvulnerableTo
//! Base` of a fire-immune type: a fireball's hit), nor anything the dragon causes. Any
//! other hurt removes it (`KILLED`) at once; unless the hurt is an explosion's, it then
//! explodes where it stood at a power of 6, without fire, breaking blocks with their drops
//! decaying (`ExplosionInteraction.BLOCK` and `block_explosion_drop_decay`), its damage
//! source caused by what hurt it (`damageSources().explosion(this, cause)`, none where
//! nothing caused it); and last the fight is told (`onDestroyedBy` →
//! `EnderDragonFight.onCrystalDestroyed`), which is the fight's step's
//! ([`Region::end_crystal_destroyed`]). A hit across a line comes as an effect to the
//! region that holds the crystal, which does all of it there.
//!
//! **Shown**: its beam's target and whether it shows its bedrock base
//! (`DATA_BEAM_TARGET`, `DATA_SHOW_BOTTOM`), read from its record by
//! [`crate::shown::end_crystal_values`].
//!
//! Named differences: a player's hit on it plays none of the attacker's sounds (as for
//! the other kinds that are not mobs); an explosion names only a player as its cause, so
//! a crystal a mob or a mob's arrow destroys explodes as if nothing caused it; the game's
//! source is never in water, as the crystal's tick never runs `baseTick`, and so it is
//! here. An explosion's source is the crystal, already removed, as in the game.

use clustine_data::entity_types;
use clustine_world::{
    BlockPos, ChunkPos, Dimension, EndCrystal, Entity, EntityData, EntityField, EntityId,
    EntityUse, PlayerId,
};
use uuid::Uuid;

use super::Region;
use super::explosions::Explosion;
use super::items::tracked;
use super::thrown::Hit;
use super::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT};
use crate::api::{RegionEvent, TickOutput};

/// `hurtServer`'s `explode(…, 6.0f, false, BLOCK)`.
const EXPLOSION_POWER: f32 = 6.0;

/// What caused a crystal's hurt (`DamageSource.getEntity()`): the entity, its type, and
/// the player's UUID where it is a player, as the fight reads it (`onCrystalDestroyed`
/// asks whether the cause is a player).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CrystalCause {
    /// The cause's key, where the region knows it.
    pub(crate) key: Option<EntityId>,
    pub(crate) kind: u16,
    pub(crate) player: Option<Uuid>,
}

impl CrystalCause {
    fn is_dragon(&self) -> bool {
        i32::from(self.kind) == entity_types::ENDER_DRAGON
    }
}

/// What the fight's hooks were called with in this thread, for the tests: a crystal
/// destroyed (its key and cause) or a crystal placed in the End.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FightCall {
    Destroyed {
        crystal: EntityId,
        cause: Option<CrystalCause>,
    },
    TryRespawn,
}

#[cfg(test)]
thread_local! {
    pub(crate) static FIGHT_CALLS: std::cell::RefCell<Vec<FightCall>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Takes what the fight's hooks were called with in this thread so far.
#[cfg(test)]
pub(crate) fn take_fight_calls() -> Vec<FightCall> {
    FIGHT_CALLS.with(|calls| std::mem::take(&mut *calls.borrow_mut()))
}

/// Whether a hit by the projectile of the type `kind` is fire's (`#is_fire`): a
/// fireball's, large or small (`fireball`, `unattributed_fireball`), which a fire-immune
/// type is invulnerable to.
fn hit_is_fire(kind: u16) -> bool {
    matches!(
        i32::from(kind),
        entity_types::FIREBALL | entity_types::SMALL_FIREBALL
    )
}

impl Region {
    /// The tick of an end crystal (`EndCrystal.tick`, d EndCrystal.txt 61–101): its age
    /// on, set in place as a counter is (`docs/groundwork/n2-plan.md`, section 11), then,
    /// in the End, fire in its block where that is air.
    pub(crate) fn tick_end_crystal(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let lived = entity.lived.saturating_add(1);
        self.age_entity(
            position,
            entity.key,
            vec![EntityField::Lived(lived)],
            output,
        );
        // `level.getDragonFight() != null`: the End's level has one, and only it.
        if self.config.dimension != Dimension::End {
            return;
        }
        let pos = crate::physics::containing(entity.position);
        if !self.block(pos).is_some_and(|state| state.props().is_air()) {
            return;
        }
        let fire = clustine_blocks::shape::fire_state(&mut RegionView(self), pos);
        self.set_block_updated(pos, fire, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// A player's or another region's use of an end crystal: an attack, or a mob's hit,
    /// is a hurt (`EndCrystal.hurtServer`), caused by the attacker; nothing else does
    /// anything to it. An explosion's and a projectile's come by their own hooks.
    pub(crate) fn end_crystal_used(
        &mut self,
        entity: &Entity,
        _chunk: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let cause = match by {
            EntityUse::Attack {
                attacker, player, ..
            } => CrystalCause {
                key: Some(attacker),
                kind: super::mob_bodies::player_kind(),
                player: player.or_else(|| self.player_with_entity(attacker).map(|id| id.0)),
            },
            EntityUse::MobHit { attacker, kind, .. } => CrystalCause {
                key: Some(attacker),
                kind,
                player: None,
            },
            _ => return,
        };
        self.end_crystal_hurt(entity.key, Some(cause), false, output);
    }

    /// An explosion's hurt of an end crystal (`ServerExplosion.hurtEntities` →
    /// `hurtServer` of an `#is_explosion` source), here or across a line: it is removed
    /// without an explosion of its own (the plan, section 19.5 item 13), caused by the
    /// explosion's cause.
    pub(crate) fn end_crystal_blasted(
        &mut self,
        record: &Entity,
        explosion: &Explosion,
        output: &mut TickOutput,
    ) {
        let cause = explosion.owner.map(|uuid| CrystalCause {
            key: self
                .players
                .get(&PlayerId(uuid))
                .map(|player| player.entity_id),
            kind: super::mob_bodies::player_kind(),
            player: Some(uuid),
        });
        self.end_crystal_hurt(record.key, cause, true, output);
    }

    /// A projectile's hit of an end crystal (a thrown item's, an arrow's, a fireball's),
    /// here or across a line: a hurt caused by its owner, none from fire.
    pub(crate) fn end_crystal_hit(&mut self, record: &Entity, hit: Hit, output: &mut TickOutput) {
        if hit_is_fire(hit.kind) {
            return;
        }
        let cause = hit.owner.map(|owner| CrystalCause {
            key: Some(owner.key),
            kind: owner.kind,
            player: owner.player,
        });
        self.end_crystal_hurt(record.key, cause, false, output);
    }

    /// `EndCrystal.hurtServer` of the region's crystal `key` (d EndCrystal.txt 158–214):
    /// refused where the dragon caused it; else the crystal is removed, explodes unless
    /// `by_explosion`, and the fight is told.
    fn end_crystal_hurt(
        &mut self,
        key: EntityId,
        cause: Option<CrystalCause>,
        by_explosion: bool,
        output: &mut TickOutput,
    ) {
        if cause.is_some_and(|cause| cause.is_dragon()) {
            return;
        }
        // `isRemoved()`: a crystal another hurt of this tick removed is hurt no more.
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        if !matches!(record.data, EntityData::EndCrystal(_)) {
            return;
        }
        // The explosion draws from the crystal's own stream, taken while it is there.
        let mut random = self.entity_stream(key).random;
        let start = random.clone();
        self.remove_entity(key, record.hops, chunk, output);
        if !by_explosion {
            let owner = cause.and_then(|cause| cause.player);
            let explosion = Explosion {
                source: key,
                centre: record.position,
                radius: EXPLOSION_POWER,
                owner,
                // `Explosion.getIndirectSourceEntity` of a crystal: none (n6
                // Explosion.txt 13–60), so the blocks are not broken by a player.
                indirect: None,
                // `ExplosionInteraction.BLOCK`: `block_explosion_drop_decay`, on.
                decay: true,
                rails_pass: false,
                source_in_water: false,
                fire: false,
                hurts: true,
                resistance_at: None,
            };
            self.explode(&explosion, &mut random, output);
            self.entity_drew(key, random.draws_since(&start));
        }
        self.end_crystal_destroyed(&record, cause, output);
    }

    /// `onDestroyedBy` → `EnderDragonFight.onCrystalDestroyed(crystal, source)` (d
    /// EndCrystal.txt 228–239; the brief's 6.3): in the End, the fight's step's, which
    /// fills this hook. A crystal of another level tells no fight.
    pub(crate) fn end_crystal_destroyed(
        &mut self,
        crystal: &Entity,
        cause: Option<CrystalCause>,
        _output: &mut TickOutput,
    ) {
        if self.config.dimension != Dimension::End {
            return;
        }
        #[cfg(test)]
        FIGHT_CALLS.with(|calls| {
            calls.borrow_mut().push(FightCall::Destroyed {
                crystal: crystal.key,
                cause,
            });
        });
        let _ = (crystal, cause);
    }

    /// `EnderDragonFight.tryRespawn()`, which `EndCrystalItem.useOn` asks for after it
    /// places a crystal in the End (d EndCrystalItem.txt 197–211): the fight's step's,
    /// which fills this hook.
    pub(crate) fn end_crystal_placed_in_the_end(&mut self, _output: &mut TickOutput) {
        #[cfg(test)]
        FIGHT_CALLS.with(|calls| calls.borrow_mut().push(FightCall::TryRespawn));
    }

    /// `setBeamTarget(target)` of the region's crystal `key`: its beam's target committed
    /// and sent to the clients that show it (`DATA_BEAM_TARGET`), as the fight's respawn
    /// and the spikes' feature set it. Whether it was there to set.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the fight's step (the dragon's respawn) points the crystals' beams"
        )
    )]
    pub(crate) fn set_end_crystal_beam(
        &mut self,
        key: EntityId,
        target: Option<BlockPos>,
        output: &mut TickOutput,
    ) -> bool {
        let Some((chunk, record)) = self.find_entity(key) else {
            return false;
        };
        let EntityData::EndCrystal(crystal) = record.data else {
            return false;
        };
        if crystal.beam_target == target {
            return true;
        }
        let mut after = record.clone();
        after.data = EntityData::EndCrystal(EndCrystal {
            beam_target: target,
            ..crystal
        });
        self.commit_entity(
            chunk,
            key,
            vec![EntityField::Data(after.data.clone())],
            output,
        );
        let mut report = tracked(&after, 0, true);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
        true
    }
}

#[cfg(test)]
#[path = "end_crystal_tests.rs"]
mod tests;
