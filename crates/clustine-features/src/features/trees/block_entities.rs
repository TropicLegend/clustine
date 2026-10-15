//! The data of the block entities trees make, as the game saves them
//! (`saveWithoutMetadata`: no id and no position) and in NBT's network form: one
//! nameless compound, its type byte first.
//!
//! The game keeps a compound's members in a `HashMap` and writes them in its order,
//! so they are written here in the order JDK 25's `HashMap` of those few names has:
//! by the bucket of each name's spread `String.hashCode` in a table of 16, and in the
//! order they were put within a bucket.

use clustine_data::BlockState;
use clustine_world::BlockEntity;

const TAG_END: u8 = 0;
const TAG_INT: u8 = 3;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;

/// The block entity a fresh block of `state` has, with `data`.
fn entity(state: BlockState, data: Vec<u8>) -> BlockEntity {
    let kind = state
        .block_info()
        .and_then(|info| info.block_entity_type)
        .expect("the block keeps a block entity");
    BlockEntity { kind, data }
}

fn name(out: &mut Vec<u8>, tag: u8, name: &str) {
    out.push(tag);
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
}

fn int(out: &mut Vec<u8>, key: &str, value: i32) {
    name(out, TAG_INT, key);
    out.extend_from_slice(&value.to_be_bytes());
}

/// A bee as `BeehiveBlockEntity.Occupant.create` makes one: a bee's entity data with
/// nothing but its type, the ticks it has been in the hive and the 600 it must stay.
/// In the order of the names' buckets: `ticks_in_hive` (10), `entity_data` (11),
/// `min_ticks_in_hive` (12); and `id` alone in the entity data.
fn bee(out: &mut Vec<u8>, ticks_in_hive: i32) {
    int(out, "ticks_in_hive", ticks_in_hive);
    name(out, TAG_COMPOUND, "entity_data");
    name(out, TAG_STRING, "id");
    let id = "minecraft:bee";
    out.extend_from_slice(&(id.len() as u16).to_be_bytes());
    out.extend_from_slice(id.as_bytes());
    out.push(TAG_END);
    int(out, "min_ticks_in_hive", 600);
    out.push(TAG_END);
}

/// A beehive or bee nest with these bees, each by its ticks in the hive: `{bees:
/// [...]}`, with no flower position.
pub fn beehive(state: BlockState, bees: &[i32]) -> BlockEntity {
    let mut out = vec![TAG_COMPOUND];
    name(&mut out, TAG_LIST, "bees");
    out.push(if bees.is_empty() {
        TAG_END
    } else {
        TAG_COMPOUND
    });
    out.extend_from_slice(&(bees.len() as i32).to_be_bytes());
    for ticks in bees {
        bee(&mut out, *ticks);
    }
    out.push(TAG_END);
    entity(state, out)
}

/// A block entity that saves nothing while it is fresh, as a creaking heart with no
/// creaking: an empty compound.
pub fn empty(state: BlockState) -> BlockEntity {
    entity(state, vec![TAG_COMPOUND, TAG_END])
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;

    /// `String.hashCode` spread as `HashMap.hash` spreads it, in a table of 16.
    fn bucket(name: &str) -> u32 {
        let hash = name.bytes().fold(0_u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        });
        (hash ^ (hash >> 16)) & 15
    }

    #[test]
    fn a_bee_s_members_are_in_the_order_of_java_s_buckets() {
        assert!(bucket("ticks_in_hive") < bucket("entity_data"));
        assert!(bucket("entity_data") < bucket("min_ticks_in_hive"));
    }

    #[test]
    fn a_nest_with_two_bees_is_one_compound_with_a_list_of_two() {
        let entity = beehive(blocks::BEE_NEST, &[5, 598]);
        let data = &entity.data;
        assert_eq!(data[0], TAG_COMPOUND);
        assert_eq!(data[1], TAG_LIST);
        assert_eq!(&data[2..4], &[0, 4]);
        assert_eq!(&data[4..8], b"bees");
        assert_eq!(data[8], TAG_COMPOUND);
        assert_eq!(&data[9..13], &2_i32.to_be_bytes());
        // The first bee starts with its ticks in the hive.
        assert_eq!(data[13], TAG_INT);
        assert_eq!(&data[14..16], &[0, 13]);
        assert_eq!(&data[16..29], b"ticks_in_hive");
        assert_eq!(&data[29..33], &5_i32.to_be_bytes());
        // Two bees of the same length, the list's end and the compound's.
        let bee_length = (data.len() - 13 - 1) / 2;
        assert_eq!(data[13 + bee_length], TAG_INT);
        assert_eq!(&data[data.len() - 1..], &[TAG_END]);
        assert_eq!(data[13 + bee_length - 1], TAG_END);
        let windows = data.windows(13).filter(|w| *w == b"minecraft:bee").count();
        assert_eq!(windows, 2);
        assert_eq!(
            Some(entity.kind),
            blocks::BEE_NEST.block_info().unwrap().block_entity_type
        );
    }

    #[test]
    fn a_fresh_creaking_heart_saves_an_empty_compound() {
        let entity = empty(blocks::CREAKING_HEART);
        assert_eq!(entity.data, vec![TAG_COMPOUND, TAG_END]);
    }
}
