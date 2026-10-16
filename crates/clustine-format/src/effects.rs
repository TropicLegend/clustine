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
//!
//! When it is due is a u8, 0 for counted from its taking or 1 followed by the holder's
//! tick u64. An effect's number is its sequence u64 and its index u32.

use clustine_data::{BLOCK_STATE_COUNT, BlockState};
use clustine_world::{BlockPos, Due, Effect, EffectId, Effected, FiledEffect, SeamKind, Ticked};

use crate::FormatError;
use crate::bytes::Input;

const UPDATE: u8 = 1;
const TICK: u8 = 2;
const SPREAD: u8 = 3;
const SET_IF: u8 = 4;

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
    }
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
        kind[12] = 9;
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
