//! What one region's tick does in land another region uses, carried to it through the
//! world store (`docs/adr/0025-effects-between-regions.md`): effects, their numbers and
//! when they are due, and the copies of the neighbours' border chunks a region reads.

use clustine_data::BlockState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BlockPos, ChunkPos, Double, Entity, EntityId, Float, ItemStack, SeamKind, Ticked};

/// What a region's tick would have done to a block in a chunk it does not have loaded,
/// addressed to that block. The effect carries the call where the target's side decides
/// (the record, section 4), and is done by the rules of the region that uses the chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    /// The block the effect acts on.
    pub at: BlockPos,
    pub what: Effected,
}

/// What an [`Effect`] does to its block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effected {
    /// `neighborChanged` or `updateShape`, as a seam entry keeps it: the direction from
    /// the block to the one that changed, by `Face`'s numbers, the block that changed as
    /// it was then, the update's flags and how much deeper a chain of shape updates may
    /// go from it.
    Update {
        direction: u8,
        kind: SeamKind,
        source: BlockState,
        flags: u32,
        depth: i32,
    },
    /// `scheduleTick`. The delay counts from the tick the effect was made in; the store
    /// says when it is due in the holder's ticks ([`Due`]).
    Tick {
        of: Ticked,
        delay: i32,
        priority: i8,
    },
    /// `FlowingFluid.spreadTo` from the neighbour in `direction` (by `Face`'s numbers,
    /// from the block to the source), whose block, with its fluid, is `source`. The
    /// holder runs the target's side whole: `canSpreadTo` and `getNewLiquid`.
    Spread { direction: u8, source: BlockState },
    /// A block set by a rule (fire spreading, a rail turned towards a placed one, a
    /// plant grown into a block, a block removed): only if the block is still
    /// `expected`, and then with the set's `flags` (`Block.UPDATE_*`), which say which
    /// updates it makes: a vine grows with 2, which tells no neighbour.
    SetIf {
        expected: BlockState,
        state: BlockState,
        flags: u32,
    },
    /// A player's hit on, or use of, the entity `key` of the chunk, which another region
    /// uses (`docs/groundwork/n5-plan.md`, decision 10 and section 7, item 9). The block
    /// it is addressed to is the entity's as the player's region last knew it; one gone
    /// by the time it is done does nothing.
    OnEntity { key: EntityId, by: EntityUse },
    /// A block an explosion broke, if it is still `expected`: the holder breaks it with
    /// its updates, spills a container, primes TNT, and lays the drops of all the blocks
    /// of one explosion taken in the tick together, merged within that explosion alone
    /// (`docs/groundwork/n6-plan.md`, decision 10, section 6 item 9 and section 7 item
    /// 11).
    Exploded { expected: BlockState, blast: Blast },
    /// Fire burnt the block, if it is still `expected`: TNT is primed (`onCaughtFire`),
    /// and the block is removed as a set with flags 3 would remove it (section 6 item 9).
    Burnt { expected: BlockState },
    /// A player of another region stands in the box `min` to `max` in this tick, which a
    /// plate or a tripwire of the block counts as an entity of its own for the tick it is
    /// due (players are not in copies; section 7 item 12 and section 8 item 10).
    PlayerInside { min: [Double; 3], max: [Double; 3] },
    /// What an animal's region gives the player whose stay is `player`, who used it from
    /// another region (a milk bucket, a stew): the player's region does the game's
    /// `ItemUtils.createFilledResult` with it, keeping out a second of one the player
    /// has where `no_duplicates` (`docs/groundwork/n7-plan.md`, section 7.3.6).
    Given {
        player: EntityId,
        stack: ItemStack,
        no_duplicates: bool,
    },
    /// The region of the vehicle `vehicle`, whose root's chunk is `chunk`, granted the
    /// player whose entity is `player` a seat on it across a line: the player's region
    /// hands them over to ride it, or declines (`docs/groundwork/n7j-mount-across-a-line.md`,
    /// section 8 item 5).
    Seated {
        player: EntityId,
        vehicle: EntityId,
        chunk: crate::ChunkPos,
    },
}

/// What an explosion's effects carry of the explosion, for the holder of a block or an
/// entity across a line to do the game's part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blast {
    /// Which explosion it is: the key of the entity that exploded and the tick of its
    /// region it exploded in, unique across regions, so that two explosions' drops are
    /// never merged.
    pub source: EntityId,
    pub tick: u64,
    /// Where it exploded, and how far it reached (`radius`).
    pub centre: [Double; 3],
    pub radius: Float,
    /// Whether its drops decay, one in `radius` kept (`DESTROY_WITH_DECAY`): TNT's do
    /// not (`tnt_explosion_drop_decay` is false).
    pub decay: bool,
    /// Whether it has a direct source, the TNT itself, which the loot sees as
    /// `this_entity`.
    pub direct: bool,
    /// Who primed it, if anyone did, and whether they are a creative player, which a
    /// vehicle and a hanging thing read of it.
    pub owner: Option<Uuid>,
    pub owner_creative: bool,
}

/// What a player did to an entity in another region's land, with what the game reads of
/// the player for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityUse {
    /// `attack`: the damage the hit deals (`base·(0.2 + 0.8·s²)`, a critical's ×1.5
    /// included), whether the attacker is a creative player and sprints, and where they
    /// stand.
    Attack {
        attacker: EntityId,
        creative: bool,
        sprinting: bool,
        damage: Float,
        from: [Double; 3],
    },
    /// `interact`: the player, whether creative, what they hold in the hand used, where
    /// on the entity the click landed, whether they sneak, and where they stand; and
    /// their edge, which a seat granted them across a line goes with
    /// (`docs/groundwork/n7j-mount-across-a-line.md`; none in a use stored before).
    Interact {
        player: EntityId,
        creative: bool,
        held: Option<ItemStack>,
        location: [Double; 3],
        sneaking: bool,
        from: [Double; 3],
        edge: Option<crate::EdgeId>,
    },
    /// The player whose entity is `player` does not take the seat the entity's region
    /// granted them across a line (`docs/groundwork/n7j-mount-across-a-line.md`,
    /// section 8 item 3): that seat goes, and no other.
    SeatDeclined { player: EntityId },
    /// An explosion's hit (`hurtEntities`), as the exploding region worked it out from
    /// its copy: the damage, the push, and of the explosion what the entity reads of
    /// its source (`docs/groundwork/n6-plan.md`, decision 10).
    Blasted {
        damage: Float,
        push: [Double; 3],
        blast: Blast,
    },
    /// An animal in love, `asker`, asks the entity to breed with it
    /// (`docs/groundwork/n7-plan.md`, sections 8.5, 9.4 and 10.7): only the higher key
    /// asks, and the partner's region breeds, as the game's `this` of `breed`, if the
    /// partner can. `number` is the asker's, which the answer names.
    BreedAsk { asker: EntityId, number: u32 },
    /// The answer to the entity's request `number` to `partner`: accepted, the asker
    /// takes its part of the breeding (its age and its love reset); refused, it is free
    /// to ask again.
    BreedAnswer {
        partner: EntityId,
        number: u32,
        accepted: bool,
    },
    /// A mob's hit on the entity (`hurtServer(mobAttack(attacker), damage)`: a goat's
    /// ram, an axolotl's bite, a pufferfish's sting), made where the attacker's region
    /// sees it only in a copy (`docs/groundwork/n7j-plan.md`, section 3 item 5): the
    /// attacker and its entity type, where its feet were, the damage, and where the hurt
    /// took, the knockback after it (strength and the direction pushed from) and the
    /// sound at the attacker's feet.
    MobHit {
        attacker: EntityId,
        kind: u16,
        from: [Double; 3],
        damage: Float,
        knockback: Option<[Double; 3]>,
        /// The iron golem's fling (`IronGolem.doHurtTarget`): upwards by this times what
        /// the target's knockback resistance leaves, added where the hurt took.
        fling: Option<Float>,
        sound: Option<i32>,
    },
}

impl Effect {
    /// What tells two effects apart where a region keeps only one of each before it
    /// sends them (the record, section 4): an update per block, direction and kind, a
    /// scheduled tick per block and what it ticks, a spread per block and direction.
    /// A set has no key and is never deduplicated.
    pub fn key(&self) -> Option<EffectKey> {
        match self.what {
            Effected::Update {
                direction, kind, ..
            } => Some(EffectKey::Update(self.at, direction, kind)),
            Effected::Tick { of, .. } => Some(EffectKey::Tick(self.at, of)),
            Effected::Spread { direction, .. } => Some(EffectKey::Spread(self.at, direction)),
            Effected::SetIf { .. }
            | Effected::OnEntity { .. }
            | Effected::Exploded { .. }
            | Effected::Burnt { .. }
            | Effected::PlayerInside { .. }
            | Effected::Given { .. }
            | Effected::Seated { .. } => None,
        }
    }
}

/// See [`Effect::key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectKey {
    Update(BlockPos, u8, SeamKind),
    Tick(BlockPos, Ticked),
    Spread(BlockPos, u8),
}

/// An effect's number: the sequence of the group of commits that made it durable and
/// its place among that group's effects. Unique for the life of the world, and
/// increasing in the order the store files effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EffectId {
    pub sequence: u64,
    pub index: u32,
}

/// When an effect is due, in the ticks of the region that takes it (the record,
/// section 8): see [`FiledEffect::due`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Due {
    /// Its maker was coupled to the holder when it was made: due at the holder's
    /// `tick`, or at the tick that takes it if that is later.
    At { tick: u64 },
    /// Its maker was not coupled to the holder: the delay counts from the tick that
    /// takes it.
    FromTaking,
}

/// An entity sent across a line, as the store hands it to the region that holds its
/// chunk (`docs/adr/0028-entities.md`, section 5; `docs/groundwork/n3-plan.md`). Numbered
/// as effects are, and taken once by number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrival {
    pub id: EffectId,
    pub chunk: ChunkPos,
    /// The entity as the sender's tick left it, with the `hops` it had: the taking raises
    /// them.
    pub entity: Entity,
    /// When it may be taken, in the holder's ticks: [`Due::At`] the holder's tick that the
    /// sender's tick was, taken in the holder's tick after it, so that a coupled crossing
    /// loses no tick and ticks the entity once; [`Due::FromTaking`] at once.
    pub due: Due,
}

/// An effect as the store hands it to the region that uses its chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiledEffect {
    pub id: EffectId,
    pub effect: Effect,
    /// For a scheduled tick, when it is due. For every other effect, [`Due::At`] names
    /// the holder's tick that the effect was made in, which the ticks it schedules
    /// count their delays from (never earlier than the tick that takes it), so that
    /// water crosses a coupled line at its own pace (the record, section 8).
    pub due: Due,
}
