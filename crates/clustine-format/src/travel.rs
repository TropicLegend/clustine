//! Travels and travellers as the log has them (`docs/groundwork/p-plan.md`, section 15).

use clustine_world::{
    BlockPos, ChunkPos, Dimension, Double, LevelChunk, PortalAxis, Travel, Traveller, Way,
};

use crate::FormatError;
use crate::bytes::Input;

/// The most entities one traveller carries: a root and its riders.
const MAX_TRAVELLING: usize = 64;

pub(crate) fn put_dimension(out: &mut Vec<u8>, dimension: Dimension) {
    out.push(dimension.number());
}

pub(crate) fn take_dimension(input: &mut Input<'_>) -> Result<Dimension, FormatError> {
    Dimension::numbered(input.u8()?).ok_or(FormatError::Corrupt("a dimension"))
}

fn put_block(out: &mut Vec<u8>, position: BlockPos) {
    for coordinate in [position.x, position.y, position.z] {
        out.extend_from_slice(&coordinate.to_be_bytes());
    }
}

fn take_block(input: &mut Input<'_>) -> Result<BlockPos, FormatError> {
    Ok(BlockPos::new(input.i32()?, input.i32()?, input.i32()?))
}

pub(crate) fn put_travel(out: &mut Vec<u8>, travel: &Travel) {
    put_dimension(out, travel.to);
    put_block(out, travel.target);
    match travel.way {
        Way::NetherPortal { axis, relative } => {
            out.push(match axis {
                None => 0,
                Some(PortalAxis::X) => 1,
                Some(PortalAxis::Z) => 2,
            });
            for value in relative {
                out.extend_from_slice(&value.0.to_be_bytes());
            }
        }
        Way::EndPortal => out.push(3),
        Way::Gateway { from, exact } => {
            out.push(4);
            put_block(out, from);
            out.push(u8::from(exact));
        }
        Way::Credits => out.push(5),
        Way::Respawn => out.push(6),
    }
}

pub(crate) fn take_travel(input: &mut Input<'_>) -> Result<Travel, FormatError> {
    let to = take_dimension(input)?;
    let target = take_block(input)?;
    let way = match input.u8()? {
        tag @ 0..=2 => Way::NetherPortal {
            axis: [None, Some(PortalAxis::X), Some(PortalAxis::Z)][usize::from(tag)],
            relative: [
                Double(input.f64()?),
                Double(input.f64()?),
                Double(input.f64()?),
            ],
        },
        3 => Way::EndPortal,
        4 => Way::Gateway {
            from: take_block(input)?,
            exact: match input.u8()? {
                0 => false,
                1 => true,
                _ => return Err(FormatError::Corrupt("a gateway's exact flag")),
            },
        },
        5 => Way::Credits,
        6 => Way::Respawn,
        _ => return Err(FormatError::Corrupt("a travel's way")),
    };
    Ok(Travel { to, target, way })
}

pub(crate) fn put_optional_travel(out: &mut Vec<u8>, travel: Option<&Travel>) {
    match travel {
        None => out.push(0),
        Some(travel) => {
            out.push(1);
            put_travel(out, travel);
        }
    }
}

pub(crate) fn take_optional_travel(input: &mut Input<'_>) -> Result<Option<Travel>, FormatError> {
    match input.u8()? {
        0 => Ok(None),
        1 => Ok(Some(take_travel(input)?)),
        _ => Err(FormatError::Corrupt("a travel's flag")),
    }
}

pub(crate) fn put_traveller(out: &mut Vec<u8>, traveller: &Traveller) {
    put_travel(out, &traveller.travel);
    put_dimension(out, traveller.chunk.dimension);
    out.extend_from_slice(&traveller.chunk.position.x.to_be_bytes());
    out.extend_from_slice(&traveller.chunk.position.z.to_be_bytes());
    let count = u8::try_from(traveller.entities.len())
        .ok()
        .filter(|count| usize::from(*count) <= MAX_TRAVELLING)
        .expect("a traveller carries at most 64 entities");
    out.push(count);
    for entity in &traveller.entities {
        crate::entities::put_entity(out, entity);
    }
}

pub(crate) fn take_traveller(input: &mut Input<'_>) -> Result<Traveller, FormatError> {
    let travel = take_travel(input)?;
    let dimension = take_dimension(input)?;
    let position = ChunkPos::new(input.i32()?, input.i32()?);
    let count = usize::from(input.u8()?);
    if count == 0 || count > MAX_TRAVELLING {
        return Err(FormatError::Corrupt("a traveller's count of entities"));
    }
    let mut entities = Vec::with_capacity(count);
    for _ in 0..count {
        entities.push(crate::entities::take_entity(input)?);
    }
    Ok(Traveller {
        travel,
        chunk: LevelChunk::new(dimension, position),
        entities,
    })
}

pub(crate) fn put_travellers(out: &mut Vec<u8>, travellers: &[Traveller]) {
    out.extend_from_slice(&(travellers.len() as u32).to_be_bytes());
    for traveller in travellers {
        put_traveller(out, traveller);
    }
}

pub(crate) fn take_travellers(input: &mut Input<'_>) -> Result<Vec<Traveller>, FormatError> {
    let count = input.u32()? as usize;
    // Each is far longer than a byte: no forged count reserves more.
    let mut travellers = Vec::with_capacity(count.min(input.0.len()));
    for _ in 0..count {
        travellers.push(take_traveller(input)?);
    }
    Ok(travellers)
}
