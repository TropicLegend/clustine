//! How effects are written in the log (`docs/adr/0025-effects-between-regions.md`,
//! sections 4 and 5).
//!
//! An effect is the block it acts on as i32 x, i32 y and i32 z, then a kind u8 and what
//! that kind carries:
//!
//! | Kind | What follows |
//! |---|---|
//! | 1, an update | direction u8 (0 to 5), kind u8 (0 `updateShape`, 1 `neighborChanged`), the block that changed u16, flags u32, depth i32 |
//! | 2, a scheduled tick | 1 and a block u16, or 2 and a fluid u16; delay i32, priority i8 |
//! | 3, a spread | direction u8 (0 to 5), the source's block u16 |
//! | 4, a set if still | the block expected u16, the block to set u16, flags u32 |
//! | 5, on an entity | its key u64, then 1, an attack: the attacker's key u64, flags u8 (bit 0 creative, bit 1 sprinting), the damage f32, where they stand 3 × f64; or 2, a use: the player's key u64, flags u8 (bit 0 creative, bit 1 sneaking), u8 0 or 1 and the stack held (item i32, count i32), where the click landed 3 × f64, where they stand 3 × f64; or 3, an explosion's hit: the damage f32, the push 3 × f64, the blast |
//! | 6, exploded | the block expected u16, the blast |
//! | 7, burnt | the block expected u16 |
//! | 8, a player inside | the box's lower corner 3 × f64 and its upper corner 3 × f64 |
//!
//! A blast (`clustine_world::Blast`) is the exploding entity's key u64, the tick u64, the
//! centre 3 × f64, the radius f32, flags u8 (bit 0 its drops decay, bit 1 it has a direct
//! source, bit 2 it has an owner, bit 3 the owner is a creative player; the others 0) and,
//! with an owner, the owner's UUID u128.
//!
//! When it is due is a u8, 0 for counted from its taking or 1 followed by the holder's
//! tick u64. An effect's number is its sequence u64 and its index u32.

use clustine_data::{BLOCK_STATE_COUNT, BlockState};
use clustine_world::{
    Blast, BlockPos, Double, Due, Effect, EffectId, Effected, EntityId, EntityUse, FiledEffect,
    Float, ItemStack, SeamKind, Ticked,
};
use uuid::Uuid;

use crate::FormatError;
use crate::bytes::Input;

const UPDATE: u8 = 1;
const TICK: u8 = 2;
const SPREAD: u8 = 3;
const SET_IF: u8 = 4;
const ON_ENTITY: u8 = 5;
const EXPLODED: u8 = 6;
const BURNT: u8 = 7;
const PLAYER_INSIDE: u8 = 8;
const GIVEN: u8 = 9;

const USE_ATTACK: u8 = 1;
const USE_INTERACT: u8 = 2;
const USE_BLASTED: u8 = 3;
const USE_BREED_ASK: u8 = 4;
const USE_BREED_ANSWER: u8 = 5;
const USE_MOB_HIT: u8 = 6;

const BLAST_DECAY: u8 = 1;
const BLAST_DIRECT: u8 = 2;
const BLAST_OWNED: u8 = 4;
const BLAST_BY_CREATIVE: u8 = 8;

/// Bytes an effect takes at the least: its block and its kind.
const EFFECT_LENGTH: usize = 13;

pub(crate) fn put_effect(out: &mut Vec<u8>, effect: &Effect) {
    out.extend_from_slice(&effect.at.x.to_be_bytes());
    out.extend_from_slice(&effect.at.y.to_be_bytes());
    out.extend_from_slice(&effect.at.z.to_be_bytes());
    match effect.what {
        Effected::Update {
            direction,
            kind,
            source,
            flags,
            depth,
        } => {
            out.push(UPDATE);
            out.push(direction);
            out.push(match kind {
                SeamKind::Shape => 0,
                SeamKind::Neighbour => 1,
            });
            out.extend_from_slice(&source.0.to_be_bytes());
            out.extend_from_slice(&flags.to_be_bytes());
            out.extend_from_slice(&depth.to_be_bytes());
        }
        Effected::Tick {
            of,
            delay,
            priority,
        } => {
            out.push(TICK);
            let (kind, id) = match of {
                Ticked::Block(block) => (1_u8, block),
                Ticked::Fluid(fluid) => (2, u16::from(fluid)),
            };
            out.push(kind);
            out.extend_from_slice(&id.to_be_bytes());
            out.extend_from_slice(&delay.to_be_bytes());
            out.extend_from_slice(&priority.to_be_bytes());
        }
        Effected::Spread { direction, source } => {
            out.push(SPREAD);
            out.push(direction);
            out.extend_from_slice(&source.0.to_be_bytes());
        }
        Effected::SetIf {
            expected,
            state,
            flags,
        } => {
            out.push(SET_IF);
            out.extend_from_slice(&expected.0.to_be_bytes());
            out.extend_from_slice(&state.0.to_be_bytes());
            out.extend_from_slice(&flags.to_be_bytes());
        }
        Effected::OnEntity { key, by } => {
            out.push(ON_ENTITY);
            out.extend_from_slice(&key.0.to_be_bytes());
            match by {
                EntityUse::Attack {
                    attacker,
                    creative,
                    sprinting,
                    damage,
                    from,
                } => {
                    out.push(USE_ATTACK);
                    out.extend_from_slice(&attacker.0.to_be_bytes());
                    out.push(u8::from(creative) | (u8::from(sprinting) << 1));
                    out.extend_from_slice(&damage.0.to_be_bytes());
                    put_doubles(out, from);
                }
                EntityUse::Interact {
                    player,
                    creative,
                    held,
                    location,
                    sneaking,
                    from,
                } => {
                    out.push(USE_INTERACT);
                    out.extend_from_slice(&player.0.to_be_bytes());
                    out.push(u8::from(creative) | (u8::from(sneaking) << 1));
                    match held {
                        None => out.push(0),
                        Some(stack) => {
                            out.push(1);
                            out.extend_from_slice(&stack.item.to_be_bytes());
                            out.extend_from_slice(&stack.count.to_be_bytes());
                        }
                    }
                    put_doubles(out, location);
                    put_doubles(out, from);
                }
                EntityUse::Blasted {
                    damage,
                    push,
                    blast,
                } => {
                    out.push(USE_BLASTED);
                    out.extend_from_slice(&damage.0.to_be_bytes());
                    put_doubles(out, push);
                    put_blast(out, &blast);
                }
                EntityUse::BreedAsk { asker, number } => {
                    out.push(USE_BREED_ASK);
                    out.extend_from_slice(&asker.0.to_be_bytes());
                    out.extend_from_slice(&number.to_be_bytes());
                }
                EntityUse::BreedAnswer {
                    partner,
                    number,
                    accepted,
                } => {
                    out.push(USE_BREED_ANSWER);
                    out.extend_from_slice(&partner.0.to_be_bytes());
                    out.extend_from_slice(&number.to_be_bytes());
                    out.push(u8::from(accepted));
                }
                EntityUse::MobHit {
                    attacker,
                    kind,
                    from,
                    damage,
                    knockback,
                    sound,
                } => {
                    out.push(USE_MOB_HIT);
                    out.extend_from_slice(&attacker.0.to_be_bytes());
                    out.extend_from_slice(&kind.to_be_bytes());
                    put_doubles(out, from);
                    out.extend_from_slice(&damage.0.to_be_bytes());
                    // Each optional part after a flag of its own.
                    out.push(u8::from(knockback.is_some()));
                    if let Some(knockback) = knockback {
                        put_doubles(out, knockback);
                    }
                    out.push(u8::from(sound.is_some()));
                    if let Some(sound) = sound {
                        out.extend_from_slice(&sound.to_be_bytes());
                    }
                }
            }
        }
        Effected::Exploded { expected, blast } => {
            out.push(EXPLODED);
            out.extend_from_slice(&expected.0.to_be_bytes());
            put_blast(out, &blast);
        }
        Effected::Burnt { expected } => {
            out.push(BURNT);
            out.extend_from_slice(&expected.0.to_be_bytes());
        }
        Effected::PlayerInside { min, max } => {
            out.push(PLAYER_INSIDE);
            put_doubles(out, min);
            put_doubles(out, max);
        }
        Effected::Given {
            player,
            stack,
            no_duplicates,
        } => {
            out.push(GIVEN);
            out.extend_from_slice(&player.0.to_be_bytes());
            out.extend_from_slice(&stack.item.to_be_bytes());
            out.extend_from_slice(&stack.count.to_be_bytes());
            out.push(u8::from(no_duplicates));
        }
    }
}

fn put_blast(out: &mut Vec<u8>, blast: &Blast) {
    out.extend_from_slice(&blast.source.0.to_be_bytes());
    out.extend_from_slice(&blast.tick.to_be_bytes());
    put_doubles(out, blast.centre);
    out.extend_from_slice(&blast.radius.0.to_be_bytes());
    let mut flags = 0;
    if blast.decay {
        flags |= BLAST_DECAY;
    }
    if blast.direct {
        flags |= BLAST_DIRECT;
    }
    if blast.owner.is_some() {
        flags |= BLAST_OWNED;
    }
    if blast.owner_creative {
        flags |= BLAST_BY_CREATIVE;
    }
    out.push(flags);
    if let Some(owner) = blast.owner {
        out.extend_from_slice(&owner.as_u128().to_be_bytes());
    }
}

fn take_blast(input: &mut Input<'_>) -> Result<Blast, FormatError> {
    let source = EntityId(input.u64()?);
    let tick = input.u64()?;
    let centre = take_doubles(input)?;
    let radius = Float(input.f32()?);
    let flags = input.u8()?;
    if flags & !(BLAST_DECAY | BLAST_DIRECT | BLAST_OWNED | BLAST_BY_CREATIVE) != 0 {
        return Err(FormatError::Corrupt("a blast's flags"));
    }
    let owner = if flags & BLAST_OWNED != 0 {
        Some(Uuid::from_u128(input.u128()?))
    } else {
        None
    };
    Ok(Blast {
        source,
        tick,
        centre,
        radius,
        decay: flags & BLAST_DECAY != 0,
        direct: flags & BLAST_DIRECT != 0,
        owner,
        owner_creative: flags & BLAST_BY_CREATIVE != 0,
    })
}

fn put_doubles(out: &mut Vec<u8>, values: [Double; 3]) {
    for value in values {
        out.extend_from_slice(&value.0.to_be_bytes());
    }
}

fn take_doubles(input: &mut Input<'_>) -> Result<[Double; 3], FormatError> {
    Ok([
        Double(input.f64()?),
        Double(input.f64()?),
        Double(input.f64()?),
    ])
}

fn take_two_flags(input: &mut Input<'_>) -> Result<(bool, bool), FormatError> {
    let flags = input.u8()?;
    if flags & !3 != 0 {
        return Err(FormatError::Corrupt("an entity use's flags"));
    }
    Ok((flags & 1 != 0, flags & 2 != 0))
}

pub(crate) fn take_effect(input: &mut Input<'_>) -> Result<Effect, FormatError> {
    let at = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
    let what = match input.u8()? {
        UPDATE => {
            let direction = take_direction(input)?;
            let kind = match input.u8()? {
                0 => SeamKind::Shape,
                1 => SeamKind::Neighbour,
                _ => return Err(FormatError::Corrupt("an effect's kind of update")),
            };
            Effected::Update {
                direction,
                kind,
                source: take_block(input)?,
                flags: input.u32()?,
                depth: input.i32()?,
            }
        }
        TICK => {
            let kind = input.u8()?;
            let id = input.u16()?;
            let of = match kind {
                1 => Ticked::Block(id),
                2 => Ticked::Fluid(u8::try_from(id).map_err(|_| FormatError::Corrupt("fluid"))?),
                _ => return Err(FormatError::Corrupt("an effect's tick kind")),
            };
            Effected::Tick {
                of,
                delay: input.i32()?,
                priority: input.i8()?,
            }
        }
        SPREAD => Effected::Spread {
            direction: take_direction(input)?,
            source: take_block(input)?,
        },
        SET_IF => Effected::SetIf {
            expected: take_block(input)?,
            state: take_block(input)?,
            flags: input.u32()?,
        },
        ON_ENTITY => {
            let key = EntityId(input.u64()?);
            let by = match input.u8()? {
                USE_ATTACK => {
                    let attacker = EntityId(input.u64()?);
                    let (creative, sprinting) = take_two_flags(input)?;
                    EntityUse::Attack {
                        attacker,
                        creative,
                        sprinting,
                        damage: Float(input.f32()?),
                        from: take_doubles(input)?,
                    }
                }
                USE_INTERACT => {
                    let player = EntityId(input.u64()?);
                    let (creative, sneaking) = take_two_flags(input)?;
                    let held = match input.u8()? {
                        0 => None,
                        1 => Some(ItemStack {
                            item: input.i32()?,
                            count: input.i32()?,
                        }),
                        _ => return Err(FormatError::Corrupt("an entity use's held flag")),
                    };
                    EntityUse::Interact {
                        player,
                        creative,
                        held,
                        location: take_doubles(input)?,
                        sneaking,
                        from: take_doubles(input)?,
                    }
                }
                USE_BLASTED => EntityUse::Blasted {
                    damage: Float(input.f32()?),
                    push: take_doubles(input)?,
                    blast: take_blast(input)?,
                },
                USE_BREED_ASK => EntityUse::BreedAsk {
                    asker: EntityId(input.u64()?),
                    number: input.u32()?,
                },
                USE_BREED_ANSWER => EntityUse::BreedAnswer {
                    partner: EntityId(input.u64()?),
                    number: input.u32()?,
                    accepted: match input.u8()? {
                        0 => false,
                        1 => true,
                        _ => return Err(FormatError::Corrupt("a breeding answer's flag")),
                    },
                },
                USE_MOB_HIT => EntityUse::MobHit {
                    attacker: EntityId(input.u64()?),
                    kind: input.u16()?,
                    from: take_doubles(input)?,
                    damage: Float(input.f32()?),
                    knockback: match input.u8()? {
                        0 => None,
                        1 => Some(take_doubles(input)?),
                        _ => return Err(FormatError::Corrupt("a mob's hit's knockback flag")),
                    },
                    sound: match input.u8()? {
                        0 => None,
                        1 => Some(input.i32()?),
                        _ => return Err(FormatError::Corrupt("a mob's hit's sound flag")),
                    },
                },
                _ => return Err(FormatError::Corrupt("an entity use's kind")),
            };
            Effected::OnEntity { key, by }
        }
        EXPLODED => Effected::Exploded {
            expected: take_block(input)?,
            blast: take_blast(input)?,
        },
        BURNT => Effected::Burnt {
            expected: take_block(input)?,
        },
        PLAYER_INSIDE => Effected::PlayerInside {
            min: take_doubles(input)?,
            max: take_doubles(input)?,
        },
        GIVEN => Effected::Given {
            player: EntityId(input.u64()?),
            stack: ItemStack {
                item: input.i32()?,
                count: input.i32()?,
            },
            no_duplicates: match input.u8()? {
                0 => false,
                1 => true,
                _ => return Err(FormatError::Corrupt("a given stack's flag")),
            },
        },
        _ => return Err(FormatError::Corrupt("an effect's kind")),
    };
    Ok(Effect { at, what })
}

fn take_direction(input: &mut Input<'_>) -> Result<u8, FormatError> {
    let direction = input.u8()?;
    if direction > 5 {
        return Err(FormatError::Corrupt("an effect's direction"));
    }
    Ok(direction)
}

fn take_block(input: &mut Input<'_>) -> Result<BlockState, FormatError> {
    let block = input.u16()?;
    if u32::from(block) >= BLOCK_STATE_COUNT {
        return Err(FormatError::Corrupt("an effect's block"));
    }
    Ok(BlockState(block))
}

pub(crate) fn put_due(out: &mut Vec<u8>, due: Due) {
    match due {
        Due::FromTaking => out.push(0),
        Due::At { tick } => {
            out.push(1);
            out.extend_from_slice(&tick.to_be_bytes());
        }
    }
}

pub(crate) fn take_due(input: &mut Input<'_>) -> Result<Due, FormatError> {
    match input.u8()? {
        0 => Ok(Due::FromTaking),
        1 => Ok(Due::At { tick: input.u64()? }),
        _ => Err(FormatError::Corrupt("when an effect is due")),
    }
}

pub(crate) fn put_id(out: &mut Vec<u8>, id: EffectId) {
    out.extend_from_slice(&id.sequence.to_be_bytes());
    out.extend_from_slice(&id.index.to_be_bytes());
}

pub(crate) fn take_id(input: &mut Input<'_>) -> Result<EffectId, FormatError> {
    Ok(EffectId {
        sequence: input.u64()?,
        index: input.u32()?,
    })
}

/// A count, then each effect with when it is due.
pub(crate) fn put_effects(out: &mut Vec<u8>, effects: &[(Effect, Due)]) {
    out.extend_from_slice(&(effects.len() as u32).to_be_bytes());
    for (effect, due) in effects {
        put_effect(out, effect);
        put_due(out, *due);
    }
}

pub(crate) fn take_effects(input: &mut Input<'_>) -> Result<Vec<(Effect, Due)>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the payload can hold.
    let mut effects = Vec::with_capacity(count.min(input.0.len() / (EFFECT_LENGTH + 1)));
    for _ in 0..count {
        let effect = take_effect(input)?;
        effects.push((effect, take_due(input)?));
    }
    Ok(effects)
}

/// A count, then each number, in ascending order.
pub(crate) fn put_ids(out: &mut Vec<u8>, ids: &[EffectId]) {
    out.extend_from_slice(&(ids.len() as u32).to_be_bytes());
    for id in ids {
        put_id(out, *id);
    }
}

pub(crate) fn take_ids(input: &mut Input<'_>) -> Result<Vec<EffectId>, FormatError> {
    let count = input.u32()? as usize;
    let mut ids: Vec<EffectId> = Vec::with_capacity(count.min(input.0.len() / 12));
    for _ in 0..count {
        let id = take_id(input)?;
        if ids.last().is_some_and(|last| *last >= id) {
            return Err(FormatError::Corrupt("effects' numbers out of order"));
        }
        ids.push(id);
    }
    Ok(ids)
}

/// A count, then each effect with its number and when it is due, in ascending order of
/// the numbers.
pub(crate) fn put_filed(out: &mut Vec<u8>, effects: &[FiledEffect]) {
    out.extend_from_slice(&(effects.len() as u32).to_be_bytes());
    for filed in effects {
        put_id(out, filed.id);
        put_effect(out, &filed.effect);
        put_due(out, filed.due);
    }
}

pub(crate) fn take_filed(input: &mut Input<'_>) -> Result<Vec<FiledEffect>, FormatError> {
    let count = input.u32()? as usize;
    let mut filed: Vec<FiledEffect> =
        Vec::with_capacity(count.min(input.0.len() / (EFFECT_LENGTH + 13)));
    for _ in 0..count {
        let id = take_id(input)?;
        if filed.last().is_some_and(|last| last.id >= id) {
            return Err(FormatError::Corrupt("effects' numbers out of order"));
        }
        let effect = take_effect(input)?;
        filed.push(FiledEffect {
            id,
            effect,
            due: take_due(input)?,
        });
    }
    Ok(filed)
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;

    fn effect() -> Effect {
        Effect {
            at: BlockPos::new(1, 2, 3),
            what: Effected::Spread {
                direction: 2,
                source: blocks::WATER,
            },
        }
    }

    fn bytes_of(effect: &Effect) -> Vec<u8> {
        let mut out = Vec::new();
        put_effect(&mut out, effect);
        out
    }

    #[test]
    fn an_effect_with_a_direction_a_kind_or_a_block_out_of_range_is_corrupt() {
        let good = bytes_of(&effect());
        assert_eq!(take_effect(&mut Input(&good)), Ok(effect()));
        // The kind is after the block, three i32; the direction after the kind.
        let mut kind = good.clone();
        kind[12] = 10;
        assert_eq!(
            take_effect(&mut Input(&kind)),
            Err(FormatError::Corrupt("an effect's kind"))
        );
        let mut direction = good.clone();
        direction[13] = 6;
        assert_eq!(
            take_effect(&mut Input(&direction)),
            Err(FormatError::Corrupt("an effect's direction"))
        );
        let mut block = good;
        block[14] = 0xff;
        block[15] = 0xff;
        assert_eq!(
            take_effect(&mut Input(&block)),
            Err(FormatError::Corrupt("an effect's block"))
        );
    }

    #[test]
    fn numbers_out_of_order_or_twice_are_corrupt() {
        for ids in [
            [
                EffectId {
                    sequence: 2,
                    index: 0,
                },
                EffectId {
                    sequence: 1,
                    index: 5,
                },
            ],
            [
                EffectId {
                    sequence: 2,
                    index: 1,
                },
                EffectId {
                    sequence: 2,
                    index: 1,
                },
            ],
        ] {
            let mut out = Vec::new();
            put_ids(&mut out, &ids);
            assert_eq!(
                take_ids(&mut Input(&out)),
                Err(FormatError::Corrupt("effects' numbers out of order"))
            );
        }
    }

    #[test]
    fn a_forged_count_reserves_no_more_than_the_input_holds() {
        let mut out = u32::MAX.to_be_bytes().to_vec();
        out.extend_from_slice(&bytes_of(&effect()));
        out.push(0);
        assert_eq!(take_effects(&mut Input(&out)), Err(FormatError::Truncated));
        let forged = u32::MAX.to_be_bytes();
        assert_eq!(take_ids(&mut Input(&forged)), Err(FormatError::Truncated));
        assert_eq!(take_filed(&mut Input(&forged)), Err(FormatError::Truncated));
    }

    #[test]
    fn when_an_effect_is_due_has_two_forms_and_no_third() {
        for due in [Due::FromTaking, Due::At { tick: u64::MAX }] {
            let mut out = Vec::new();
            put_due(&mut out, due);
            assert_eq!(take_due(&mut Input(&out)), Ok(due));
        }
        assert_eq!(
            take_due(&mut Input(&[2])),
            Err(FormatError::Corrupt("when an effect is due"))
        );
    }
}

#[cfg(test)]
mod entity_uses {
    use super::*;

    fn round_trip(effect: Effect) {
        let mut out = Vec::new();
        put_effect(&mut out, &effect);
        let mut input = Input(&out);
        assert_eq!(take_effect(&mut input), Ok(effect));
        assert!(input.0.is_empty());
    }

    #[test]
    fn a_hit_and_a_use_of_an_entity_are_read_back_as_written() {
        let at = BlockPos::new(-17, 64, 3);
        round_trip(Effect {
            at,
            what: Effected::OnEntity {
                key: EntityId(9 << 32 | 4),
                by: EntityUse::Attack {
                    attacker: EntityId(2 << 32),
                    creative: true,
                    sprinting: false,
                    damage: Float(1.5),
                    from: [Double(-16.5), Double(64.0), Double(2.25)],
                },
            },
        });
        for held in [None, Some(ItemStack { item: 7, count: 3 })] {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(1),
                    by: EntityUse::Interact {
                        player: EntityId(5),
                        creative: false,
                        held,
                        location: [Double(0.25), Double(1.5), Double(-0.125)],
                        sneaking: true,
                        from: [Double(-15.0), Double(63.0), Double(4.0)],
                    },
                },
            });
        }
    }

    #[test]
    fn a_use_of_an_entity_with_flags_out_of_range_is_refused() {
        let mut out = Vec::new();
        put_effect(
            &mut out,
            &Effect {
                at: BlockPos::new(0, 0, 0),
                what: Effected::OnEntity {
                    key: EntityId(1),
                    by: EntityUse::Attack {
                        attacker: EntityId(2),
                        creative: false,
                        sprinting: false,
                        damage: Float(1.0),
                        from: [Double(0.0); 3],
                    },
                },
            },
        );
        // The flags follow the block (12), the kind (1), the key (8), the use's kind (1)
        // and the attacker (8).
        out[30] = 4;
        assert!(take_effect(&mut Input(&out)).is_err());
    }

    #[test]
    fn a_stack_given_to_a_player_is_read_back_as_written_and_a_bad_flag_refused() {
        for no_duplicates in [false, true] {
            round_trip(Effect {
                at: BlockPos::new(-3, 70, 11),
                what: Effected::Given {
                    player: EntityId(3 << 32 | 1),
                    stack: ItemStack {
                        item: 1134,
                        count: 1,
                    },
                    no_duplicates,
                },
            });
        }
        let mut out = Vec::new();
        put_effect(
            &mut out,
            &Effect {
                at: BlockPos::new(0, 0, 0),
                what: Effected::Given {
                    player: EntityId(1),
                    stack: ItemStack { item: 1, count: 1 },
                    no_duplicates: true,
                },
            },
        );
        // The flag is the last byte, after the block, the kind, the player and the stack.
        *out.last_mut().expect("written") = 2;
        assert!(take_effect(&mut Input(&out)).is_err());
    }

    #[test]
    fn a_request_to_breed_and_its_answer_are_read_back_as_written() {
        let at = BlockPos::new(15, -60, 2);
        let uses = [
            EntityUse::BreedAsk {
                asker: EntityId(u64::MAX - 1),
                number: u32::MAX,
            },
            EntityUse::BreedAnswer {
                partner: EntityId(1 << 40),
                number: 3,
                accepted: true,
            },
            EntityUse::BreedAnswer {
                partner: EntityId(7),
                number: 0,
                accepted: false,
            },
        ];
        for by in uses {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by,
                },
            });
        }
    }

    #[test]
    fn a_mobs_hit_is_read_back_as_written_with_and_without_its_parts() {
        let at = BlockPos::new(15, -60, 2);
        let hit = |knockback, sound| EntityUse::MobHit {
            attacker: EntityId(u64::MAX - 2),
            kind: 1 << 15,
            from: [Double(0.5), Double(-58.25), Double(-1024.5)],
            damage: Float(2.5),
            knockback,
            sound,
        };
        let pushed = Some([Double(1.75), Double(-0.5), Double(0.0)]);
        for by in [
            hit(None, None),
            hit(pushed, None),
            hit(None, Some(-7)),
            hit(pushed, Some(i32::MAX)),
        ] {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by,
                },
            });
        }
    }

    fn blast(owner: Option<u128>) -> Blast {
        Blast {
            source: EntityId(u64::MAX - 3),
            tick: 1 << 40,
            centre: [Double(0.5), Double(-58.0), Double(-1024.5)],
            radius: Float(4.0),
            decay: false,
            direct: true,
            owner: owner.map(Uuid::from_u128),
            owner_creative: owner.is_some(),
        }
    }

    #[test]
    fn an_explosions_effects_are_read_back_as_written() {
        let at = BlockPos::new(15, -60, 2);
        for owner in [None, Some(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF)] {
            round_trip(Effect {
                at,
                what: Effected::Exploded {
                    expected: clustine_data::blocks::TNT,
                    blast: blast(owner),
                },
            });
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by: EntityUse::Blasted {
                        damage: Float(56.5),
                        push: [Double(0.25), Double(1.0), Double(-0.5)],
                        blast: Blast {
                            decay: true,
                            direct: false,
                            ..blast(owner)
                        },
                    },
                },
            });
        }
        round_trip(Effect {
            at,
            what: Effected::Burnt {
                expected: clustine_data::blocks::TNT,
            },
        });
        round_trip(Effect {
            at,
            what: Effected::PlayerInside {
                min: [Double(15.2), Double(-60.0), Double(2.2)],
                max: [Double(15.8), Double(-58.2), Double(2.8)],
            },
        });
    }

    #[test]
    fn a_blast_with_flags_out_of_range_is_refused() {
        let mut out = Vec::new();
        put_effect(
            &mut out,
            &Effect {
                at: BlockPos::new(0, 0, 0),
                what: Effected::Exploded {
                    expected: clustine_data::blocks::AIR,
                    blast: blast(None),
                },
            },
        );
        // The flags follow the block (12), the kind (1), the block expected (2), the
        // source (8), the tick (8), the centre (24) and the radius (4).
        out[59] |= 16;
        assert!(take_effect(&mut Input(&out)).is_err());
    }
}
