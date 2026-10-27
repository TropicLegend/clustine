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
//! | 5, on an entity | its key u64, then 1, an attack: the attacker's key u64, flags u8 (bit 0 creative, bit 1 sprinting), the damage f32, where they stand 3 × f64; or 2, a use: the player's key u64, flags u8 (bit 0 creative, bit 1 sneaking), u8 0 or 1 and the stack held (as `stacks.rs` writes a stack), where the click landed 3 × f64, where they stand 3 × f64; or 3, an explosion's hit: the damage f32, the push 3 × f64, the blast |
//! | 6, exploded | the block expected u16, the blast |
//! | 7, burnt | the block expected u16 |
//! | 8, a player inside | the box's lower corner 3 × f64 and its upper corner 3 × f64 |
//! | 9, given | the player's key u64, the stack (as `stacks.rs` writes one), u8 0 or 1 whether no duplicates |
//! | 10, seated | the player's key u64, the vehicle's key u64, its root's chunk x i32 and z i32 |
//! | 11, on a player | the player's UUID u128, their entity's key u64, then a use as kind 5 writes one after its key |
//!
//! The uses of kind 5 since: 4, a request to breed; 5, its answer; 6, a mob's hit; 7, a
//! use as 2 followed by the player's edge u64; 8, a seat declined, the player's key u64;
//! 9, a mob's hit as 6 followed by the iron golem's fling f32; 11, a guardian's spikes, a
//! mob's hit of `thorns` as 6; 10, a thrown item's hit: the projectile's key u64, its type u16, where it was 3 × f64,
//! its motion 3 × f64, the damage f32, and u8 0 where nobody threw it, or u8 1 and the
//! thrower's key u64, type u16 and feet 3 × f64, then u8 0, or u8 1 and the player's
//! UUID u128 where a player threw it.
//!
//! N9's uses (`docs/groundwork/n9-plan.md`, section 8.3). An option is u8 0, or u8 1 and
//! the value; a list is a count u32 and its elements; a string a length u32 and its UTF-8
//! bytes; an effect instance as `stacks.rs` writes one in `potion_contents`; a causer its
//! key u64, type u16, feet 3 × f64, an option of the player's UUID u128, and u8 0 or 1
//! whether it is living.
//!
//! - 12, a hurt: the damage type (a string), the cause and the direct entity (each an
//!   option of a causer), the position and the projectile's motion (each an option of
//!   3 × f64), the amount f32, the knockback (an option of 3 × f64), the fire before i32
//!   and after i32, the effects (a list of instances), u8 0 or 1 an arrow, the extras (a
//!   list, each u8 1 a zombie's fire, 2 a husk's hunger, or 3 a fling and its f32), the
//!   sound (an option of i32, the volume f32 and the pitch f32) and the hit (an option of
//!   the attacker's key u64 and its `lived` u32).
//! - 13, an alert: the attacker's key u64, its type u16, the class u16.
//! - 14, effects: the instances (a list), the instant factor f32, the source and the owner
//!   (each an option of a causer), u8 0 or 1 water.
//! - 15, a pearl landed: where 3 × f64, u8 0 or 1 an endermite.
//! - 16, a patrol's target: x i32, y i32, z i32.
//!
//! A blast (`clustine_world::Blast`) is the exploding entity's key u64, the tick u64, the
//! centre 3 × f64, the radius f32, flags u8 (bit 0 its drops decay, bit 1 it has a direct
//! source, bit 2 it has an owner, bit 3 the owner is a creative player; the others 0) and,
//! with an owner, the owner's UUID u128.
//!
//! When it is due is a u8, 0 for counted from its taking or 1 followed by the holder's
//! tick u64. An effect's number is its sequence u64 and its index u32.

use clustine_data::{BLOCK_STATE_COUNT, BlockState};
use clustine_world::components::EffectInstance;
use clustine_world::{
    Blast, BlockPos, Causer, Double, Due, Effect, EffectId, Effected, EntityId, EntityUse,
    FiledEffect, Float, GivenEffects, HitExtra, HurtBy, PlayerId, SeamKind, Thrower, Ticked,
};
use uuid::Uuid;

use crate::FormatError;
use crate::bytes::Input;
use crate::stacks::{
    put_effect as put_effect_instance, put_optional_stack, put_stack, put_string,
    take_effect as take_effect_instance, take_optional_stack, take_stack, take_string,
};

const UPDATE: u8 = 1;
const TICK: u8 = 2;
const SPREAD: u8 = 3;
const SET_IF: u8 = 4;
const ON_ENTITY: u8 = 5;
const EXPLODED: u8 = 6;
const BURNT: u8 = 7;
const PLAYER_INSIDE: u8 = 8;
const GIVEN: u8 = 9;
const SEATED: u8 = 10;
/// An entity use addressed to a player wherever their stay is held (N9, a new tag
/// without a raise of the format, as the uses 12 to 16 below).
const ON_PLAYER: u8 = 11;
/// A ticket of a point of interest given back (`docs/groundwork/n10-plan.md`, decision 2).
const RELEASE: u8 = 12;

const USE_ATTACK: u8 = 1;
const USE_INTERACT: u8 = 2;
const USE_BLASTED: u8 = 3;
const USE_BREED_ASK: u8 = 4;
const USE_BREED_ANSWER: u8 = 5;
const USE_MOB_HIT: u8 = 6;
/// A use with the player's edge (`USE_INTERACT` is one stored before, with none).
const USE_INTERACT_FROM_EDGE: u8 = 7;
const USE_SEAT_DECLINED: u8 = 8;
/// A mob's hit with the iron golem's fling, which follows its sound.
const USE_MOB_HIT_FLUNG: u8 = 9;
/// A thrown item's hit (a snowball's).
const USE_THROWN: u8 = 10;
/// A guardian's spikes: a mob's hit of `thorns`, which carries no fling.
const USE_MOB_HIT_THORNS: u8 = 11;
// N9's entity effects (`docs/groundwork/n9-plan.md`, section 8.3), and the effect
// addressed to a player. New tags need no raise of the format: a world made before them
// is refused anyway (the plan's finding 55), and none of the old tags changes meaning.
/// A hurt and what follows it if it took (`clustine_world::HurtBy`).
const USE_HURT: u8 = 12;
/// A mob told of an attacker (`alertOthers`).
const USE_ALERT: u8 = 13;
/// Effects given by a splash or a cloud (`clustine_world::GivenEffects`).
const USE_EFFECTS: u8 = 14;
/// A player's ender pearl landed.
const USE_PEARL_LANDED: u8 = 15;
/// A patrol's leader names its target.
const USE_PATROL_TARGET: u8 = 16;

/// What an attacker's type adds to a hurt (`clustine_world::HitExtra`), a tag each.
const EXTRA_ZOMBIE_FIRE: u8 = 1;
const EXTRA_HUSK_HUNGER: u8 = 2;
const EXTRA_FLING: u8 = 3;

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
        Effected::OnEntity { key, ref by } => {
            out.push(ON_ENTITY);
            out.extend_from_slice(&key.0.to_be_bytes());
            put_use(out, by);
        }
        Effected::OnPlayer {
            player,
            entity,
            ref by,
        } => {
            out.push(ON_PLAYER);
            out.extend_from_slice(&player.0.as_u128().to_be_bytes());
            out.extend_from_slice(&entity.0.to_be_bytes());
            put_use(out, by);
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
            ref stack,
            no_duplicates,
        } => {
            out.push(GIVEN);
            out.extend_from_slice(&player.0.to_be_bytes());
            put_stack(out, stack);
            out.push(u8::from(no_duplicates));
        }
        Effected::Seated {
            player,
            vehicle,
            chunk,
        } => {
            out.push(SEATED);
            out.extend_from_slice(&player.0.to_be_bytes());
            out.extend_from_slice(&vehicle.0.to_be_bytes());
            out.extend_from_slice(&chunk.x.to_be_bytes());
            out.extend_from_slice(&chunk.z.to_be_bytes());
        }
        Effected::Release { kind } => {
            out.push(RELEASE);
            out.push(kind.id());
        }
    }
}

/// Appends a use of an entity: its kind, then what that kind carries. An effect on a
/// chunk's entity and one addressed to a player write it alike.
fn put_use(out: &mut Vec<u8>, by: &EntityUse) {
    match *by {
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
            ref held,
            location,
            sneaking,
            from,
            edge,
        } => {
            out.push(if edge.is_some() {
                USE_INTERACT_FROM_EDGE
            } else {
                USE_INTERACT
            });
            out.extend_from_slice(&player.0.to_be_bytes());
            out.push(u8::from(creative) | (u8::from(sneaking) << 1));
            put_optional_stack(out, held.as_ref());
            put_doubles(out, location);
            put_doubles(out, from);
            if let Some(edge) = edge {
                out.extend_from_slice(&edge.0.to_be_bytes());
            }
        }
        EntityUse::SeatDeclined { player } => {
            out.push(USE_SEAT_DECLINED);
            out.extend_from_slice(&player.0.to_be_bytes());
        }
        EntityUse::Blasted {
            damage,
            push,
            ref blast,
        } => {
            out.push(USE_BLASTED);
            out.extend_from_slice(&damage.0.to_be_bytes());
            put_doubles(out, push);
            put_blast(out, blast);
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
            fling,
            sound,
            thorns,
        } => {
            // A flung hit and a guardian's spikes are kinds of their own, so that
            // the hits the log keeps from before them are read as they were
            // written. The spikes push nothing and fling nothing.
            out.push(if thorns {
                USE_MOB_HIT_THORNS
            } else if fling.is_some() {
                USE_MOB_HIT_FLUNG
            } else {
                USE_MOB_HIT
            });
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
            if let Some(fling) = fling.filter(|_| !thorns) {
                out.extend_from_slice(&fling.0.to_be_bytes());
            }
        }
        EntityUse::Thrown {
            projectile,
            kind,
            at,
            motion,
            damage,
            owner,
        } => {
            out.push(USE_THROWN);
            out.extend_from_slice(&projectile.0.to_be_bytes());
            out.extend_from_slice(&kind.to_be_bytes());
            put_doubles(out, at);
            put_doubles(out, motion);
            out.extend_from_slice(&damage.0.to_be_bytes());
            out.push(u8::from(owner.is_some()));
            if let Some(owner) = owner {
                out.extend_from_slice(&owner.key.0.to_be_bytes());
                out.extend_from_slice(&owner.kind.to_be_bytes());
                put_doubles(out, owner.from);
                out.push(u8::from(owner.player.is_some()));
                if let Some(player) = owner.player {
                    out.extend_from_slice(&player.as_u128().to_be_bytes());
                }
            }
        }
        EntityUse::Hurt(ref hurt) => {
            out.push(USE_HURT);
            put_string(out, &hurt.damage_type);
            put_optional_causer(out, hurt.cause.as_ref());
            put_optional_causer(out, hurt.direct.as_ref());
            put_optional_doubles(out, hurt.position);
            put_optional_doubles(out, hurt.projectile_motion);
            out.extend_from_slice(&hurt.amount.0.to_be_bytes());
            put_optional_doubles(out, hurt.knockback);
            out.extend_from_slice(&hurt.fire_before.to_be_bytes());
            out.extend_from_slice(&hurt.fire_after.to_be_bytes());
            put_instances(out, &hurt.effects);
            out.push(u8::from(hurt.arrow));
            out.extend_from_slice(&count_u32(hurt.extras.len()).to_be_bytes());
            for extra in &hurt.extras {
                match *extra {
                    HitExtra::ZombieFire => out.push(EXTRA_ZOMBIE_FIRE),
                    HitExtra::HuskHunger => out.push(EXTRA_HUSK_HUNGER),
                    HitExtra::Fling(strength) => {
                        out.push(EXTRA_FLING);
                        out.extend_from_slice(&strength.0.to_be_bytes());
                    }
                }
            }
            out.push(u8::from(hurt.sound.is_some()));
            if let Some((sound, volume, pitch)) = hurt.sound {
                out.extend_from_slice(&sound.to_be_bytes());
                out.extend_from_slice(&volume.0.to_be_bytes());
                out.extend_from_slice(&pitch.0.to_be_bytes());
            }
            out.push(u8::from(hurt.hit.is_some()));
            if let Some((attacker, lived)) = hurt.hit {
                out.extend_from_slice(&attacker.0.to_be_bytes());
                out.extend_from_slice(&lived.to_be_bytes());
            }
        }
        EntityUse::Alert {
            attacker,
            kind,
            class,
        } => {
            out.push(USE_ALERT);
            out.extend_from_slice(&attacker.0.to_be_bytes());
            out.extend_from_slice(&kind.to_be_bytes());
            out.extend_from_slice(&class.to_be_bytes());
        }
        EntityUse::Effects(ref given) => {
            out.push(USE_EFFECTS);
            put_instances(out, &given.instances);
            out.extend_from_slice(&given.instant_factor.0.to_be_bytes());
            put_optional_causer(out, given.source.as_ref());
            put_optional_causer(out, given.owner.as_ref());
            out.push(u8::from(given.water));
        }
        EntityUse::PearlLanded { at, endermite } => {
            out.push(USE_PEARL_LANDED);
            put_doubles(out, at);
            out.push(u8::from(endermite));
        }
        EntityUse::PatrolTarget { pos } => {
            out.push(USE_PATROL_TARGET);
            out.extend_from_slice(&pos.x.to_be_bytes());
            out.extend_from_slice(&pos.y.to_be_bytes());
            out.extend_from_slice(&pos.z.to_be_bytes());
        }
    }
}

/// Reads what [`put_use`] wrote.
fn take_use(input: &mut Input<'_>) -> Result<EntityUse, FormatError> {
    let use_kind = input.u8()?;
    Ok(match use_kind {
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
        USE_INTERACT | USE_INTERACT_FROM_EDGE => {
            let player = EntityId(input.u64()?);
            let (creative, sneaking) = take_two_flags(input)?;
            let held = take_optional_stack(input, "an entity use's held flag")?;
            let location = take_doubles(input)?;
            let from = take_doubles(input)?;
            let edge = if use_kind == USE_INTERACT_FROM_EDGE {
                Some(clustine_world::EdgeId(input.u64()?))
            } else {
                None
            };
            EntityUse::Interact {
                player,
                creative,
                held,
                location,
                sneaking,
                from,
                edge,
            }
        }
        USE_SEAT_DECLINED => EntityUse::SeatDeclined {
            player: EntityId(input.u64()?),
        },
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
            accepted: take_flag(input, "a breeding answer's flag")?,
        },
        USE_MOB_HIT | USE_MOB_HIT_FLUNG | USE_MOB_HIT_THORNS => {
            let attacker = EntityId(input.u64()?);
            let kind = input.u16()?;
            let from = take_doubles(input)?;
            let damage = Float(input.f32()?);
            let knockback = match input.u8()? {
                0 => None,
                1 => Some(take_doubles(input)?),
                _ => return Err(FormatError::Corrupt("a mob's hit's knockback flag")),
            };
            let sound = match input.u8()? {
                0 => None,
                1 => Some(input.i32()?),
                _ => return Err(FormatError::Corrupt("a mob's hit's sound flag")),
            };
            let fling = if use_kind == USE_MOB_HIT_FLUNG {
                Some(Float(input.f32()?))
            } else {
                None
            };
            EntityUse::MobHit {
                attacker,
                kind,
                from,
                damage,
                knockback,
                fling,
                sound,
                thorns: use_kind == USE_MOB_HIT_THORNS,
            }
        }
        USE_THROWN => EntityUse::Thrown {
            projectile: EntityId(input.u64()?),
            kind: input.u16()?,
            at: take_doubles(input)?,
            motion: take_doubles(input)?,
            damage: Float(input.f32()?),
            owner: match input.u8()? {
                0 => None,
                1 => Some(Thrower {
                    key: EntityId(input.u64()?),
                    kind: input.u16()?,
                    from: take_doubles(input)?,
                    player: match input.u8()? {
                        0 => None,
                        1 => Some(Uuid::from_u128(input.u128()?)),
                        _ => {
                            return Err(FormatError::Corrupt("a thrown item's hit's player flag"));
                        }
                    },
                }),
                _ => return Err(FormatError::Corrupt("a thrown item's hit's owner flag")),
            },
        },
        USE_HURT => {
            let damage_type = take_string(input)?;
            let cause = take_optional_causer(input)?;
            let direct = take_optional_causer(input)?;
            let position = take_optional_doubles(input, "a hurt's position flag")?;
            let projectile_motion = take_optional_doubles(input, "a hurt's motion flag")?;
            let amount = Float(input.f32()?);
            let knockback = take_optional_doubles(input, "a hurt's knockback flag")?;
            let fire_before = input.i32()?;
            let fire_after = input.i32()?;
            let effects = take_instances(input)?;
            let arrow = take_flag(input, "a hurt's arrow flag")?;
            let count = take_count(input)?;
            let mut extras = Vec::with_capacity(count);
            for _ in 0..count {
                extras.push(match input.u8()? {
                    EXTRA_ZOMBIE_FIRE => HitExtra::ZombieFire,
                    EXTRA_HUSK_HUNGER => HitExtra::HuskHunger,
                    EXTRA_FLING => HitExtra::Fling(Float(input.f32()?)),
                    _ => return Err(FormatError::Corrupt("a hit's extra")),
                });
            }
            let sound = if take_flag(input, "a hurt's sound flag")? {
                Some((input.i32()?, Float(input.f32()?), Float(input.f32()?)))
            } else {
                None
            };
            let hit = if take_flag(input, "a hurt's hit flag")? {
                Some((EntityId(input.u64()?), input.u32()?))
            } else {
                None
            };
            EntityUse::Hurt(Box::new(HurtBy {
                damage_type,
                cause,
                direct,
                position,
                projectile_motion,
                amount,
                knockback,
                fire_before,
                fire_after,
                effects,
                arrow,
                extras,
                sound,
                hit,
            }))
        }
        USE_ALERT => EntityUse::Alert {
            attacker: EntityId(input.u64()?),
            kind: input.u16()?,
            class: input.u16()?,
        },
        USE_EFFECTS => EntityUse::Effects(Box::new(GivenEffects {
            instances: take_instances(input)?,
            instant_factor: Float(input.f32()?),
            source: take_optional_causer(input)?,
            owner: take_optional_causer(input)?,
            water: take_flag(input, "given effects' water flag")?,
        })),
        USE_PEARL_LANDED => EntityUse::PearlLanded {
            at: take_doubles(input)?,
            endermite: take_flag(input, "a pearl's endermite flag")?,
        },
        USE_PATROL_TARGET => EntityUse::PatrolTarget {
            pos: BlockPos::new(input.i32()?, input.i32()?, input.i32()?),
        },
        _ => return Err(FormatError::Corrupt("an entity use's kind")),
    })
}

/// A causer is its key u64, its type u16, its feet 3 × f64, u8 0 or u8 1 and the player's
/// UUID u128, and u8 0 or 1 whether it is living.
fn put_causer(out: &mut Vec<u8>, causer: &Causer) {
    out.extend_from_slice(&causer.key.0.to_be_bytes());
    out.extend_from_slice(&causer.kind.to_be_bytes());
    put_doubles(out, causer.position);
    out.push(u8::from(causer.player.is_some()));
    if let Some(player) = causer.player {
        out.extend_from_slice(&player.as_u128().to_be_bytes());
    }
    out.push(u8::from(causer.living));
}

fn take_causer(input: &mut Input<'_>) -> Result<Causer, FormatError> {
    Ok(Causer {
        key: EntityId(input.u64()?),
        kind: input.u16()?,
        position: take_doubles(input)?,
        player: if take_flag(input, "a causer's player flag")? {
            Some(Uuid::from_u128(input.u128()?))
        } else {
            None
        },
        living: take_flag(input, "a causer's living flag")?,
    })
}

fn put_optional_causer(out: &mut Vec<u8>, causer: Option<&Causer>) {
    out.push(u8::from(causer.is_some()));
    if let Some(causer) = causer {
        put_causer(out, causer);
    }
}

fn take_optional_causer(input: &mut Input<'_>) -> Result<Option<Causer>, FormatError> {
    Ok(if take_flag(input, "a causer's flag")? {
        Some(take_causer(input)?)
    } else {
        None
    })
}

fn put_optional_doubles(out: &mut Vec<u8>, values: Option<[Double; 3]>) {
    out.push(u8::from(values.is_some()));
    if let Some(values) = values {
        put_doubles(out, values);
    }
}

fn take_optional_doubles(
    input: &mut Input<'_>,
    what: &'static str,
) -> Result<Option<[Double; 3]>, FormatError> {
    Ok(if take_flag(input, what)? {
        Some(take_doubles(input)?)
    } else {
        None
    })
}

/// A count u32, then each effect instance as a stack's `potion_contents` writes one.
fn put_instances(out: &mut Vec<u8>, instances: &[EffectInstance]) {
    out.extend_from_slice(&count_u32(instances.len()).to_be_bytes());
    for instance in instances {
        put_effect_instance(out, instance);
    }
}

fn take_instances(input: &mut Input<'_>) -> Result<Vec<EffectInstance>, FormatError> {
    let count = take_count(input)?;
    let mut instances = Vec::with_capacity(count);
    for _ in 0..count {
        instances.push(take_effect_instance(input)?);
    }
    Ok(instances)
}

/// A list's count, refused where it is beyond the bytes left (each element takes one at
/// the least), so that a forged count reserves nothing.
fn take_count(input: &mut Input<'_>) -> Result<usize, FormatError> {
    let count = usize::try_from(input.u32()?).map_err(|_| FormatError::Corrupt("a count"))?;
    if count > input.0.len() {
        return Err(FormatError::Corrupt("a list longer than its record"));
    }
    Ok(count)
}

fn count_u32(count: usize) -> u32 {
    u32::try_from(count).expect("a hurt carries fewer than 2^32 effects or extras")
}

fn take_flag(input: &mut Input<'_>, what: &'static str) -> Result<bool, FormatError> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(FormatError::Corrupt(what)),
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
        ON_ENTITY => Effected::OnEntity {
            key: EntityId(input.u64()?),
            by: take_use(input)?,
        },
        ON_PLAYER => Effected::OnPlayer {
            player: PlayerId(Uuid::from_u128(input.u128()?)),
            entity: EntityId(input.u64()?),
            by: take_use(input)?,
        },
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
            stack: take_stack(input)?,
            no_duplicates: match input.u8()? {
                0 => false,
                1 => true,
                _ => return Err(FormatError::Corrupt("a given stack's flag")),
            },
        },
        SEATED => Effected::Seated {
            player: EntityId(input.u64()?),
            vehicle: EntityId(input.u64()?),
            chunk: clustine_world::ChunkPos::new(input.i32()?, input.i32()?),
        },
        RELEASE => Effected::Release {
            kind: clustine_world::poi::PoiType::from_id(input.u8()?)
                .ok_or(FormatError::Corrupt("a point of interest of no type"))?,
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

    /// A release of a ticket is its block, its kind and the point of interest's type id;
    /// a type there is none of is corrupt.
    #[test]
    fn a_release_is_read_back_and_one_of_no_type_is_corrupt() {
        let release = Effect {
            at: BlockPos::new(-7, 64, 12),
            what: Effected::Release {
                kind: clustine_world::poi::PoiType::Meeting,
            },
        };
        let bytes = bytes_of(&release);
        assert_eq!(bytes[12..], [RELEASE, 14]);
        assert_eq!(take_effect(&mut Input(&bytes)), Ok(release));
        let mut unknown = bytes.clone();
        unknown[13] = 21;
        assert_eq!(
            take_effect(&mut Input(&unknown)),
            Err(FormatError::Corrupt("a point of interest of no type"))
        );
    }

    #[test]
    fn an_effect_with_a_direction_a_kind_or_a_block_out_of_range_is_corrupt() {
        let good = bytes_of(&effect());
        assert_eq!(take_effect(&mut Input(&good)), Ok(effect()));
        // The kind is after the block, three i32; the direction after the kind.
        let mut kind = good.clone();
        kind[12] = RELEASE + 1;
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
    use clustine_world::ItemStack;

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
        for held in [
            None,
            Some(ItemStack::new(7, 3)),
            Some(crate::stacks::tests::with_every_component()),
        ] {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(1),
                    by: EntityUse::Interact {
                        edge: None,
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
            for stack in [
                ItemStack::new(1134, 1),
                crate::stacks::tests::with_every_component(),
            ] {
                round_trip(Effect {
                    at: BlockPos::new(-3, 70, 11),
                    what: Effected::Given {
                        player: EntityId(3 << 32 | 1),
                        stack,
                        no_duplicates,
                    },
                });
            }
        }
        let mut out = Vec::new();
        put_effect(
            &mut out,
            &Effect {
                at: BlockPos::new(0, 0, 0),
                what: Effected::Given {
                    player: EntityId(1),
                    stack: ItemStack::new(1, 1),
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
            fling: None,
            sound,
            thorns: false,
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

    #[test]
    fn a_flung_hit_is_read_back_with_its_fling_and_an_old_hit_without() {
        let at = BlockPos::new(15, -60, 2);
        let hit = |fling| EntityUse::MobHit {
            attacker: EntityId(3),
            kind: 71,
            from: [Double(0.5), Double(-60.0), Double(0.5)],
            damage: Float(13.5),
            knockback: None,
            fling,
            sound: Some(5),
            thorns: false,
        };
        for by in [hit(Some(Float(0.4))), hit(None)] {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by,
                },
            });
        }
        // A hit without a fling keeps the old layout; a flung one is its fling longer.
        let bytes = |by| {
            let mut out = Vec::new();
            put_effect(
                &mut out,
                &Effect {
                    at,
                    what: Effected::OnEntity {
                        key: EntityId(9),
                        by,
                    },
                },
            );
            out
        };
        let (plain, flung) = (bytes(hit(None)), bytes(hit(Some(Float(0.4)))));
        assert_eq!(flung.len(), plain.len() + 4);
        assert_eq!(&flung[flung.len() - 4..], &0.4_f32.to_be_bytes());
    }

    #[test]
    fn a_thrown_items_hit_is_read_back_as_written_by_a_player_a_mob_or_nobody() {
        let at = BlockPos::new(15, -60, 2);
        let hit = |owner| EntityUse::Thrown {
            projectile: EntityId(u64::MAX - 4),
            kind: 1 << 15,
            at: [Double(0.5), Double(-58.25), Double(-1024.5)],
            motion: [Double(1.25), Double(-0.03), Double(-0.75)],
            damage: Float(0.0),
            owner,
        };
        let thrower = |player: Option<u128>| Thrower {
            key: EntityId(1 << 33),
            kind: 155,
            from: [Double(2.0), Double(-60.0), Double(3.5)],
            player: player.map(Uuid::from_u128),
        };
        for by in [
            hit(None),
            hit(Some(thrower(None))),
            hit(Some(thrower(Some(u128::MAX - 9)))),
        ] {
            round_trip(Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by,
                },
            });
        }
        // The owner's flag is 0 or 1, and so is the player's.
        let mut out = Vec::new();
        put_effect(
            &mut out,
            &Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9),
                    by: hit(Some(thrower(None))),
                },
            },
        );
        // The block (12), the kind (1), the key (8), the use's kind (1), the projectile
        // (8), its type (2), where it was (24), its motion (24) and the damage (4).
        let owner_flag = 12 + 1 + 8 + 1 + 8 + 2 + 24 + 24 + 4;
        assert_eq!(out[owner_flag], 1);
        let mut bad = out.clone();
        bad[owner_flag] = 2;
        assert!(take_effect(&mut Input(&bad)).is_err());
        let player_flag = owner_flag + 1 + 8 + 2 + 24;
        assert_eq!(out[player_flag], 0);
        let mut bad = out.clone();
        bad[player_flag] = 2;
        assert!(take_effect(&mut Input(&bad)).is_err());
    }

    #[test]
    fn a_guardian_s_spikes_are_read_back_as_thorns_in_a_plain_hit_s_layout() {
        let at = BlockPos::new(15, -60, 2);
        let hit = |thorns| EntityUse::MobHit {
            attacker: EntityId(3),
            kind: 64,
            from: [Double(0.5), Double(-60.0), Double(0.5)],
            damage: Float(2.0),
            knockback: None,
            fling: None,
            sound: None,
            thorns,
        };
        let bytes = |by| {
            let mut out = Vec::new();
            put_effect(
                &mut out,
                &Effect {
                    at,
                    what: Effected::OnEntity {
                        key: EntityId(9),
                        by,
                    },
                },
            );
            out
        };
        round_trip(Effect {
            at,
            what: Effected::OnEntity {
                key: EntityId(9),
                by: hit(true),
            },
        });
        let (plain, spikes) = (bytes(hit(false)), bytes(hit(true)));
        assert_eq!(plain.len(), spikes.len());
        assert_ne!(plain, spikes, "a kind of its own");
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

    fn causer(player: Option<u128>, living: bool) -> Causer {
        Causer {
            key: EntityId(u64::MAX - 7),
            kind: 1 << 15 | 3,
            position: [Double(-0.0), Double(-63.5), Double(1e300)],
            player: player.map(Uuid::from_u128),
            living,
        }
    }

    fn instance(effect: &str, amplifier: i32, duration: i32) -> EffectInstance {
        EffectInstance {
            effect: effect.to_owned(),
            amplifier,
            duration,
            ambient: true,
            visible: false,
            show_icon: true,
        }
    }

    /// A hurt with every part set: a cause and a direct entity, a projectile's motion,
    /// knockback, fire before and after, two effects, an arrow, every extra, a sound and
    /// the hit's key.
    fn full_hurt() -> EntityUse {
        EntityUse::Hurt(Box::new(HurtBy {
            damage_type: "minecraft:arrow".to_owned(),
            cause: Some(causer(
                Some(0x0123_4567_89AB_CDEF_FEDC_BA98_7654_3210),
                true,
            )),
            direct: Some(causer(None, false)),
            position: Some([Double(3.5), Double(70.0), Double(-8.25)]),
            projectile_motion: Some([Double(0.75), Double(-0.125), Double(f64::MIN_POSITIVE)]),
            amount: Float(6.5),
            knockback: Some([Double(0.4), Double(-1.0), Double(0.5)]),
            fire_before: 100,
            fire_after: -1,
            effects: vec![
                instance("minecraft:poison", 1, 100),
                instance("minecraft:slowness", 0, -1),
            ],
            arrow: true,
            extras: vec![
                HitExtra::ZombieFire,
                HitExtra::HuskHunger,
                HitExtra::Fling(Float(0.4)),
            ],
            sound: Some((i32::MIN, Float(1.0), Float(0.875))),
            hit: Some((EntityId(1 << 48 | 5), u32::MAX)),
        }))
    }

    /// A hurt of nothing but its type and amount.
    fn bare_hurt() -> EntityUse {
        EntityUse::Hurt(Box::new(HurtBy {
            damage_type: "minecraft:generic".to_owned(),
            cause: None,
            direct: None,
            position: None,
            projectile_motion: None,
            amount: Float(1.0),
            knockback: None,
            fire_before: 0,
            fire_after: 0,
            effects: Vec::new(),
            arrow: false,
            extras: Vec::new(),
            sound: None,
            hit: None,
        }))
    }

    fn full_effects() -> EntityUse {
        EntityUse::Effects(Box::new(GivenEffects {
            instances: vec![
                instance("minecraft:instant_damage", 1, 1),
                instance("minecraft:speed", 2, 3600),
            ],
            instant_factor: Float(0.625),
            source: Some(causer(None, false)),
            owner: Some(causer(Some(u128::MAX), true)),
            water: true,
        }))
    }

    fn bare_effects() -> EntityUse {
        EntityUse::Effects(Box::new(GivenEffects {
            instances: Vec::new(),
            instant_factor: Float(1.0),
            source: None,
            owner: None,
            water: false,
        }))
    }

    /// One of each of N9's uses, with its parts set and without.
    fn n9_uses() -> Vec<EntityUse> {
        vec![
            full_hurt(),
            bare_hurt(),
            EntityUse::Alert {
                attacker: EntityId(1 << 40 | 2),
                kind: 145,
                class: u16::MAX,
            },
            full_effects(),
            bare_effects(),
            EntityUse::PearlLanded {
                at: [Double(0.5), Double(-64.0), Double(-30_000_000.5)],
                endermite: true,
            },
            EntityUse::PearlLanded {
                at: [Double(0.0); 3],
                endermite: false,
            },
            EntityUse::PatrolTarget {
                pos: BlockPos::new(i32::MIN, -64, i32::MAX),
            },
        ]
    }

    fn n9_effects() -> Vec<Effect> {
        let at = BlockPos::new(-17, 64, 3);
        let mut effects: Vec<Effect> = n9_uses()
            .into_iter()
            .map(|by| Effect {
                at,
                what: Effected::OnEntity {
                    key: EntityId(9 << 32 | 4),
                    by,
                },
            })
            .collect();
        for by in [full_hurt(), bare_hurt(), full_effects(), bare_effects()] {
            effects.push(Effect {
                at,
                what: Effected::OnPlayer {
                    player: PlayerId(Uuid::from_u128(0xFEED_0000_0000_0000_0000_0000_0000_BEEF)),
                    entity: EntityId(3 << 32 | 1),
                    by,
                },
            });
        }
        effects.push(Effect {
            at,
            what: Effected::OnPlayer {
                player: PlayerId(Uuid::from_u128(7)),
                entity: EntityId(1),
                by: EntityUse::PearlLanded {
                    at: [Double(1.5), Double(2.0), Double(3.5)],
                    endermite: false,
                },
            },
        });
        effects
    }

    #[test]
    fn n9_s_uses_and_an_effect_on_a_player_are_read_back_as_written() {
        for effect in n9_effects() {
            round_trip(effect);
        }
    }

    #[test]
    fn every_cut_of_n9_s_uses_is_refused() {
        for effect in n9_effects() {
            let out = bytes_of(&effect);
            for length in 0..out.len() {
                assert!(
                    take_effect(&mut Input(&out[..length])).is_err(),
                    "{effect:?} cut to {length} of {} bytes",
                    out.len()
                );
            }
        }
    }

    fn bytes_of(effect: &Effect) -> Vec<u8> {
        let mut out = Vec::new();
        put_effect(&mut out, effect);
        out
    }

    #[test]
    fn an_alert_is_written_byte_by_byte_as_the_format_says() {
        let effect = Effect {
            at: BlockPos::new(1, -2, 3),
            what: Effected::OnEntity {
                key: EntityId(0x0102_0304_0506_0708),
                by: EntityUse::Alert {
                    attacker: EntityId(0x1112_1314_1516_1718),
                    kind: 0x2122,
                    class: 0x3132,
                },
            },
        };
        let expected: Vec<u8> = [
            &[0, 0, 0, 1][..],                                 // x i32
            &[0xff, 0xff, 0xff, 0xfe],                         // y i32
            &[0, 0, 0, 3],                                     // z i32
            &[5],                                              // on an entity
            &[1, 2, 3, 4, 5, 6, 7, 8],                         // its key u64
            &[13],                                             // an alert
            &[0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18], // the attacker u64
            &[0x21, 0x22],                                     // its type u16
            &[0x31, 0x32],                                     // the class u16
        ]
        .concat();
        assert_eq!(bytes_of(&effect), expected);
        round_trip(effect);
    }

    #[test]
    fn a_pearl_s_landing_on_a_player_is_written_byte_by_byte_as_the_format_says() {
        let effect = Effect {
            at: BlockPos::new(0, 0, -1),
            what: Effected::OnPlayer {
                player: PlayerId(Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10)),
                entity: EntityId(0x2122_2324_2526_2728),
                by: EntityUse::PearlLanded {
                    at: [Double(1.0), Double(-2.0), Double(0.5)],
                    endermite: true,
                },
            },
        };
        let expected: Vec<u8> = [
            &[0, 0, 0, 0][..],                                        // x i32
            &[0, 0, 0, 0],                                            // y i32
            &[0xff, 0xff, 0xff, 0xff],                                // z i32
            &[11],                                                    // on a player
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16], // their UUID u128
            &[0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28],        // their entity's key u64
            &[15],                                                    // a pearl landed
            &[0x3f, 0xf0, 0, 0, 0, 0, 0, 0],                          // x 1.0 f64
            &[0xc0, 0, 0, 0, 0, 0, 0, 0],                             // y -2.0 f64
            &[0x3f, 0xe0, 0, 0, 0, 0, 0, 0],                          // z 0.5 f64
            &[1],                                                     // an endermite comes
        ]
        .concat();
        assert_eq!(bytes_of(&effect), expected);
        round_trip(effect);
    }

    #[test]
    fn a_hurt_s_flags_and_extras_out_of_range_are_refused() {
        let at = BlockPos::new(0, 0, 0);
        let on = |by| Effect {
            at,
            what: Effected::OnEntity {
                key: EntityId(1),
                by,
            },
        };
        let hurt = |extras| {
            EntityUse::Hurt(Box::new(HurtBy {
                extras,
                ..match bare_hurt() {
                    EntityUse::Hurt(hurt) => *hurt,
                    _ => unreachable!(),
                }
            }))
        };
        // The block (12), the kind (1), the key (8), the use's kind (1), then the damage
        // type: its length (4) and "minecraft:generic" (17); then the cause's flag.
        let cause_flag = 12 + 1 + 8 + 1 + 4 + 17;
        let out = bytes_of(&on(hurt(vec![HitExtra::HuskHunger])));
        assert_eq!(out[cause_flag], 0);
        let mut bad = out.clone();
        bad[cause_flag] = 2;
        assert_eq!(
            take_effect(&mut Input(&bad)),
            Err(FormatError::Corrupt("a causer's flag"))
        );
        // The extra's tag is the byte before the sound's flag and the hit's.
        let extra = out.len() - 3;
        assert_eq!(out[extra], EXTRA_HUSK_HUNGER);
        let mut bad = out.clone();
        bad[extra] = 4;
        assert_eq!(
            take_effect(&mut Input(&bad)),
            Err(FormatError::Corrupt("a hit's extra"))
        );
        // A forged count of extras is refused before anything is reserved for it.
        let count = extra - 4;
        let mut bad = out;
        bad[count..count + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            take_effect(&mut Input(&bad)),
            Err(FormatError::Corrupt("a list longer than its record"))
        );
        // A use's kind beyond the last is refused.
        let mut out = bytes_of(&on(EntityUse::PatrolTarget {
            pos: BlockPos::new(1, 2, 3),
        }));
        out[21] = USE_PATROL_TARGET + 1;
        assert_eq!(
            take_effect(&mut Input(&out)),
            Err(FormatError::Corrupt("an entity use's kind"))
        );
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
