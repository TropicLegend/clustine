//! What placing a template reads of blocks: a block state as the data writes one, the
//! game's tags of blocks, and block entities' data as a chunk keeps it.
//!
//! The helpers follow `clustine-features/src/blocks.rs` and the block entities' data of
//! `clustine-features/src/features/ground/shared.rs`, which this crate cannot use (the
//! features will build on it).

use clustine_data::{BLOCK_ENTITY_TYPES, BLOCKS, BlockClass, BlockState, TAGS, block_id_by_name};
use clustine_protocol::codec::Writer;
use clustine_protocol::nbt::Nbt;

use crate::reader::{Read, Reader};

/// Reads a block state as 26.3's codec reads one: a block's name for its default
/// state, or `{"id": ..., "properties": {...}}` with the properties given and the
/// rest as in the default state.
pub fn read_state(reader: &Reader) -> Read<BlockState> {
    if reader.is_text() {
        let name = reader.text()?;
        return block_default(name).ok_or_else(|| reader.error(format!("{name} is not a block")));
    }
    let name = reader.field("id")?.text()?;
    let mut pairs = Vec::new();
    if let Some(properties) = reader.optional("properties") {
        let clustine_worldgen_data::feature::Data::Object(members) = properties.data() else {
            return Err(properties.error("the properties of a block state are an object"));
        };
        for (property, value) in members.iter() {
            let clustine_worldgen_data::feature::Data::Text(value) = value else {
                return Err(properties.error(format!("{property} is not a string")));
            };
            pairs.push((*property, *value));
        }
    }
    reader.finish()?;
    BlockState::from_name_and_properties(name, pairs.iter().copied())
        .ok_or_else(|| reader.error(format!("{name} with {pairs:?} is not a block state")))
}

/// The default state of the block called `name`.
pub fn block_default(name: &str) -> Option<BlockState> {
    Some(BLOCKS[block_id_by_name(name)?].default_state)
}

/// The default state of a block of the pinned jar, by a name written in the code.
pub fn known(name: &str) -> BlockState {
    block_default(name).unwrap_or_else(|| panic!("{name} is a block of the pinned jar"))
}

/// The id of the block of `state`. Every state a template holds is a block's.
pub fn block_id(state: BlockState) -> usize {
    state
        .block_id()
        .expect("every state a template holds is a block's state")
}

/// Whether `state` is of the block called `name` (`state.is(block)`).
pub fn is_block(state: BlockState, name: &str) -> bool {
    state.block().is_some_and(|block| block.name == name)
}

/// Whether the block of `state` is of `class` or of a class made from it.
pub fn is_class(state: BlockState, class: BlockClass) -> bool {
    state.block_info().is_some_and(|info| info.is(class))
}

/// The blocks of the block tag `name` (with its namespace, without `#`), in the game's
/// order.
pub fn block_tag(name: &str) -> Option<&'static [i32]> {
    TAGS.iter()
        .find(|tags| tags.registry == "minecraft:block")
        .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
        .map(|tag| tag.entries)
}

/// Whether the block of `state` is in the block tag `name`. A tag the game does not
/// have holds nothing.
pub fn in_block_tag(state: BlockState, name: &str) -> bool {
    let id = block_id(state) as i32;
    block_tag(name).is_some_and(|entries| entries.contains(&id))
}

/// A set of blocks as the data names one (`HolderSet<Block>`): a block, a tag with
/// `#`, or a list of blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSet {
    blocks: Vec<usize>,
}

impl BlockSet {
    pub fn read(reader: &Reader) -> Read<Self> {
        let names: Vec<&str> = if reader.is_list() {
            reader
                .list()?
                .iter()
                .map(Reader::text)
                .collect::<Read<_>>()?
        } else {
            vec![reader.text()?]
        };
        let mut blocks = Vec::new();
        for name in names {
            if let Some(tag) = name.strip_prefix('#') {
                let entries =
                    block_tag(tag).ok_or_else(|| reader.error(format!("{tag} is not a tag")))?;
                blocks.extend(entries.iter().map(|id| *id as usize));
            } else {
                blocks.push(
                    block_id_by_name(name)
                        .ok_or_else(|| reader.error(format!("{name} is not a block")))?,
                );
            }
        }
        Ok(Self { blocks })
    }

    /// The set of these blocks, by name.
    pub fn of(names: &[&str]) -> Self {
        Self {
            blocks: names
                .iter()
                .map(|name| {
                    block_id_by_name(name)
                        .unwrap_or_else(|| panic!("{name} is a block of the pinned jar"))
                })
                .collect(),
        }
    }

    pub fn contains(&self, state: BlockState) -> bool {
        self.blocks.contains(&block_id(state))
    }
}

/// The index of a block entity type in [`BLOCK_ENTITY_TYPES`].
pub fn block_entity_type(name: &str) -> Option<u8> {
    let index = BLOCK_ENTITY_TYPES.iter().position(|kind| *kind == name)?;
    Some(u8::try_from(index).expect("the game has fewer than 256 types of block entity"))
}

/// A compound in the nameless form of the network, as a chunk keeps a block entity's
/// data.
pub fn nbt_bytes(value: &Nbt) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.put_nbt(value);
    writer.into_bytes()
}

/// `value` with the members of each of its compounds, nested ones too, in the order the
/// game's `CompoundTag` writes them: it holds them in a Java `HashMap`, which writes
/// its keys in the order of their buckets and, within a bucket, in the order they were
/// put. They are taken to have been put in the order they have here.
pub fn java_ordered(value: Nbt) -> Nbt {
    match value {
        Nbt::Compound(members) => {
            let mut capacity = 16_usize;
            // The map doubles once it holds more than three quarters of its capacity.
            while members.len() > capacity / 4 * 3 {
                capacity *= 2;
            }
            let mut keyed: Vec<(usize, (String, Nbt))> = members
                .into_iter()
                .map(|(key, value)| (java_bucket(&key, capacity), (key, java_ordered(value))))
                .collect();
            // A stable sort keeps the order of putting within a bucket.
            keyed.sort_by_key(|(bucket, _)| *bucket);
            Nbt::Compound(keyed.into_iter().map(|(_, member)| member).collect())
        }
        Nbt::List(items) => Nbt::List(items.into_iter().map(java_ordered).collect()),
        other => other,
    }
}

/// The bucket of a Java `HashMap` of `capacity` that a string key falls in:
/// `String.hashCode` over its UTF-16 units, spread by its upper half.
fn java_bucket(key: &str, capacity: usize) -> usize {
    let hash = key.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    let hash = hash as u32;
    ((hash ^ (hash >> 16)) as usize) & (capacity - 1)
}

/// Whether two values are equal as the game's tags are (`Tag.equals`): a compound is a
/// map, so the order of its members does not count.
pub fn nbt_equal(a: &Nbt, b: &Nbt) -> bool {
    match (a, b) {
        (Nbt::Compound(left), Nbt::Compound(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .iter()
                        .find(|(other, _)| other == key)
                        .is_some_and(|(_, other)| nbt_equal(value, other))
                })
        }
        (Nbt::List(left), Nbt::List(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(a, b)| nbt_equal(a, b))
        }
        _ => a == b,
    }
}

/// Puts `value` at `key` of a compound, where a member of that name is replaced
/// (`CompoundTag.put`).
pub fn put(members: &mut Vec<(String, Nbt)>, key: &str, value: Nbt) {
    match members.iter_mut().find(|(name, _)| name == key) {
        Some((_, old)) => *old = value,
        None => members.push((key.to_string(), value)),
    }
}

/// Removes the member `key` of a compound, if it has one.
pub fn remove(members: &mut Vec<(String, Nbt)>, key: &str) {
    members.retain(|(name, _)| name != key);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compound_is_equal_to_itself_in_another_order_and_a_list_is_not() {
        let a = Nbt::Compound(vec![("a".into(), Nbt::Int(1)), ("b".into(), Nbt::Int(2))]);
        let b = Nbt::Compound(vec![("b".into(), Nbt::Int(2)), ("a".into(), Nbt::Int(1))]);
        assert!(nbt_equal(&a, &b));
        let c = Nbt::Compound(vec![("b".into(), Nbt::Int(3)), ("a".into(), Nbt::Int(1))]);
        assert!(!nbt_equal(&a, &c));
        assert!(!nbt_equal(
            &Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)]),
            &Nbt::List(vec![Nbt::Int(2), Nbt::Int(1)])
        ));
    }

    #[test]
    fn members_come_out_in_the_order_of_a_java_hash_map() {
        // "LootTable" falls in bucket 0 of 16, "components" in 2 and "LootTableSeed"
        // in 15 (worked out by hand from `String.hashCode`), whatever the order of
        // putting.
        let ordered = java_ordered(Nbt::Compound(vec![
            ("components".into(), Nbt::Compound(Vec::new())),
            ("LootTableSeed".into(), Nbt::Long(5)),
            ("LootTable".into(), Nbt::String("x".into())),
        ]));
        let Nbt::Compound(members) = ordered else {
            panic!("a compound stays one");
        };
        let keys: Vec<&str> = members.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["LootTable", "components", "LootTableSeed"]);
    }
}
