//! The evoker's fangs: their warmup and their bite (`docs/groundwork/n9-plan.md`, section
//! 4.8; `EvokerFangs.tick`, n9 EvokerFangs 109–280, and `dealDamageTo`, 281–336, as D9 1.2
//! row 6 settles them). The evoker's spell makes them (`make_fangs`).
//!
//! Their warmup and life are counted from `lived` (A28 section 3's derived counters): in
//! the tick whose `tickCount` is `n`, the warmup is `warmup - n`. At the first tick it is
//! below nothing they rise (entity event 4, once); at -8 every living entity in their
//! box grown by 0.2 across is bitten by `indirectMagic(fangs, owner)` 6 (`magic` 6 with no
//! owner), but the dead, their owner and its allies; 23 ticks after the warmup ran out
//! (`lifeTicks` 22 counted below nothing) they are gone. No draws on the server.
//!
//! Named differences: an owner the region does not have is none, as the game's
//! `getOwner()` is null for one its level has not loaded (its bite is then `magic`); an
//! ally is an illager's friend (`#illager_friends`) of an illager owner, as
//! `AbstractIllager.isAlliedTo` is taken to be (**Inferred**: not dumped; there are no
//! teams); no mob is `Invulnerable` (the flag of commands); a player is creative and
//! refuses the bite, an armour stand is not bitten (the hurt of a stand by magic is
//! not built); the entity event of rising is sent at that tick only, so fangs taken in
//! after it do not send it again as the game's would after a load.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{ChunkPos, Double, Entity, EntityData, EntityId, Float, HurtBy};

use super::Region;
use super::thrown::Living;
use crate::api::{RegionEvent, TickOutput};

/// The entity event of the fangs rising.
const RISE: u8 = 4;

/// The tick of the warmup at which they bite.
const BITE: i64 = -8;

/// `lifeTicks`: they are gone when it falls below nothing, counted from the first tick
/// the warmup is below nothing.
const LIFE: i64 = 22;

/// The hurt of a bite.
const DAMAGE: f32 = 6.0;

impl Region {
    /// The tick of the fangs `entity` of the loaded chunk `position` (`EvokerFangs.tick`);
    /// it commits through `write_projectile`.
    pub(crate) fn tick_fangs(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Fangs(fangs) = entity.data else {
            return;
        };
        let key = entity.key;
        let tick_count = i64::from(entity.lived) + 1;
        let warmup = i64::from(fangs.warmup) - tick_count;
        if warmup < 0 {
            if warmup == BITE {
                self.bite(&entity, fangs.owner, output);
            }
            if warmup == -1 {
                output.events.push(RegionEvent::EntityEvent {
                    entity: key,
                    chunk: position,
                    event: RISE,
                });
            }
            // `--lifeTicks < 0`: the life counted down from the first tick below nothing.
            let life = LIFE - (-warmup);
            if life < 0 {
                self.remove_entity(key, entity.hops, position, output);
                return;
            }
        }
        let after = Entity {
            lived: entity.lived.saturating_add(1),
            ..entity.clone()
        };
        self.write_projectile(&entity, after, position, output);
    }

    /// The fangs' bite of every living entity in their box grown by 0.2 across
    /// (`dealDamageTo`).
    fn bite(&mut self, fangs: &Entity, owner: Option<EntityId>, output: &mut TickOutput) {
        let info = EntityTypeInfo::of(entity_types::EVOKER_FANGS);
        let bounds = crate::physics::aabb::Aabb::of_entity(fangs.position, info.width, info.height)
            .inflate([0.2, 0.0, 0.2]);
        // The owner as the region has it, a living entity.
        let owner = owner.and_then(|owner| {
            let causer = self.causer_of(owner)?;
            Some((owner, causer))
        });
        let direct = clustine_world::Causer {
            key: fangs.key,
            kind: fangs.kind,
            position: fangs.position.map(Double),
            player: None,
            living: false,
        };
        for living in self.living_in(&bounds, fangs.key) {
            if !living.target.alive {
                continue;
            }
            if owner
                .as_ref()
                .is_some_and(|(key, _)| *key == living.target.key)
            {
                continue;
            }
            let hurt = match &owner {
                None => bite_by("minecraft:magic", None, None),
                Some((owner, causer)) => {
                    if allied(causer.kind, &living) || self.allied_vex(*owner, causer.kind, &living)
                    {
                        continue;
                    }
                    bite_by(
                        "minecraft:indirect_magic",
                        Some(causer.clone()),
                        Some(direct.clone()),
                    )
                }
            };
            self.give_hurt(&living, hurt, output);
        }
    }

    /// `Evoker.considersEntityAsAlly(vex)` (n7 Evoker, part S8's): a vex whose owner is
    /// the fangs' owner `owner` (of the type `kind`), or an illager's friend of an
    /// illager owner.
    fn allied_vex(&self, owner: EntityId, kind: u16, living: &Living) -> bool {
        let key = living.target.key;
        let record = self
            .find_entity(key)
            .map(|(_, record)| record.clone())
            .or_else(|| self.copied_entity(key).map(|(_, record)| record));
        let Some(vex_owner) = record.as_ref().and_then(crate::mobs::types::vex::owner_of) else {
            return false;
        };
        vex_owner == owner
            || self.causer_of(vex_owner).is_some_and(|root| {
                entity_in_tag(i32::from(kind), "minecraft:illager")
                    && entity_in_tag(i32::from(root.kind), "minecraft:illager_friends")
            })
    }
}

/// A bite of the damage type `kind`, caused by `cause` and dealt by `direct`.
fn bite_by(
    kind: &str,
    cause: Option<clustine_world::Causer>,
    direct: Option<clustine_world::Causer>,
) -> HurtBy {
    HurtBy {
        damage_type: kind.to_owned(),
        cause,
        direct,
        position: None,
        projectile_motion: None,
        amount: Float(DAMAGE),
        knockback: None,
        fire_before: 0,
        fire_after: 0,
        effects: Vec::new(),
        arrow: false,
        extras: Vec::new(),
        sound: None,
        hit: None,
    }
}

/// `owner.isAlliedTo(target)` for an owner of the type `owner`: an illager's friends are
/// its allies (**Inferred**, the module's note).
fn allied(owner: u16, living: &Living) -> bool {
    let Some(kind) = living.kind else {
        return false;
    };
    entity_in_tag(i32::from(owner), "minecraft:illager")
        && entity_in_tag(i32::from(kind), "minecraft:illager_friends")
}
