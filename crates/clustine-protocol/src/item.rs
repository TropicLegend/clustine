//! Item stacks as they appear inside packets.
//!
//! An item stack is a count, an item and a patch of "components" that deviate from the
//! item's defaults, such as a potion's contents or enchantments. The components of the
//! set Clustine reads travel by the game's stream codecs (`crate::components`): a
//! server writes them with nothing between them, so a stack from a server with a
//! component outside the set cannot be read, and a client puts a length before each,
//! so those outside the set are stepped over and dropped.

use crate::codec::{DecodeError, Reader, Writer};
use crate::components::WireComponents;

/// A number of items of one kind, and the components it differs in from the item's
/// defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
    pub components: WireComponents,
}

impl ItemStack {
    /// A stack with no components of its own.
    pub const fn new(item: i32, count: i32) -> Self {
        Self {
            item,
            count,
            components: WireComponents {
                added: Vec::new(),
                removed: Vec::new(),
            },
        }
    }
}

impl Writer {
    /// Writes an item stack with its components, or an empty slot
    /// (`ItemStack.OPTIONAL_STREAM_CODEC`).
    pub fn put_item_stack(&mut self, stack: Option<&ItemStack>) {
        match stack {
            Some(stack) if stack.count > 0 => {
                self.put_var_int(stack.count);
                self.put_var_int(stack.item);
                self.put_components(&stack.components);
            }
            _ => self.put_var_int(0),
        }
    }

    /// Writes an item stack the way clients send it, each component after its length:
    /// for the bots and the tests.
    pub fn put_item_stack_serverbound(&mut self, stack: Option<&ItemStack>) {
        match stack {
            Some(stack) if stack.count > 0 => {
                self.put_var_int(stack.count);
                self.put_var_int(stack.item);
                self.put_components_serverbound(&stack.components);
            }
            _ => self.put_var_int(0),
        }
    }
}

impl Reader<'_> {
    /// Reads an item stack the way servers send it. `None` is an empty slot.
    ///
    /// Fails for a stack with a component outside the set: servers send components
    /// without lengths, so one cannot be stepped over without knowing its format.
    pub fn item_stack(&mut self) -> Result<Option<ItemStack>, DecodeError> {
        let Some((item, count)) = self.item_and_count()? else {
            return Ok(None);
        };
        let components = self.components_clientbound()?;
        Ok(Some(ItemStack {
            item,
            count,
            components,
        }))
    }

    /// Reads an item stack the way clients send it. `None` is an empty slot. The
    /// components of the set are read; those outside it are stepped over by their
    /// lengths and dropped.
    pub fn untrusted_item_stack(&mut self) -> Result<Option<ItemStack>, DecodeError> {
        let Some((item, count)) = self.item_and_count()? else {
            return Ok(None);
        };
        let components = self.components_serverbound()?.components;
        Ok(Some(ItemStack {
            item,
            count,
            components,
        }))
    }

    fn item_and_count(&mut self) -> Result<Option<(i32, i32)>, DecodeError> {
        let count = self.var_int()?;
        if count <= 0 {
            return Ok(None);
        }
        Ok(Some((self.var_int()?, count)))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::components::{WireComponent, data_component_type};

    fn written(stack: Option<&ItemStack>) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.put_item_stack(stack);
        writer.into_bytes()
    }

    fn damaged(item: i32, count: i32, damage: i32) -> ItemStack {
        let mut stack = ItemStack::new(item, count);
        stack.components.added.push(WireComponent::Damage(damage));
        stack
    }

    #[test]
    fn known_answers() {
        assert_eq!(written(None), [0]);
        let stone = ItemStack::new(1, 64);
        assert_eq!(written(Some(&stone)), [64, 1, 0, 0]);
        // A stack of nothing is an empty slot.
        assert_eq!(written(Some(&ItemStack::new(1, 0))), [0]);
        // One component added, none removed: the kind's id and its value, with no
        // length between them.
        let id = u8::try_from(data_component_type::DAMAGE).unwrap();
        assert_eq!(
            written(Some(&damaged(800, 1, 5))),
            [1, 0xA0, 6, 1, 0, id, 5]
        );
    }

    #[test]
    fn plain_stacks_read_the_same_either_way() {
        let stone = ItemStack::new(1, 64);
        for bytes in [written(Some(&stone)), written(None)] {
            let trusted = Reader::new(&bytes).item_stack().unwrap();
            let untrusted = Reader::new(&bytes).untrusted_item_stack().unwrap();
            assert_eq!(trusted, untrusted);
        }
        assert_eq!(Reader::new(&[64, 1, 0, 0]).item_stack(), Ok(Some(stone)));
    }

    #[test]
    fn a_stack_with_components_reads_back_from_either_side() {
        let sword = damaged(800, 1, 5);
        let mut bytes = written(Some(&sword));
        assert_eq!(Reader::new(&bytes).item_stack(), Ok(Some(sword.clone())));
        let mut writer = Writer::new();
        writer.put_item_stack_serverbound(Some(&sword));
        bytes.clear();
        bytes.extend(writer.into_bytes());
        assert_eq!(Reader::new(&bytes).untrusted_item_stack(), Ok(Some(sword)));
    }

    #[test]
    fn components_from_a_client_outside_the_set_are_dropped() {
        // One stone with two added components outside the set (type 5 with three
        // bytes, type 6 with none) and one removed component (type 12), followed by
        // another field.
        let bytes = [1, 1, 2, 1, 5, 3, 0xAA, 0xBB, 0xCC, 6, 0, 12, 0x7F];
        let mut reader = Reader::new(&bytes);
        let mut stone = ItemStack::new(1, 1);
        stone.components.removed.push(12);
        assert_eq!(reader.untrusted_item_stack(), Ok(Some(stone)));
        assert_eq!(reader.u8(), Ok(0x7F));
    }

    #[test]
    fn components_from_a_server_outside_the_set_are_refused() {
        assert!(Reader::new(&[1, 1, 1, 0, 5, 0]).item_stack().is_err());
    }

    proptest! {
        #[test]
        fn stacks_round_trip(item in 0..2000i32, count in 1..100i32, damage in 0..2000i32) {
            for stack in [ItemStack::new(item, count), damaged(item, count, damage)] {
                let bytes = written(Some(&stack));
                let mut reader = Reader::new(&bytes);
                prop_assert_eq!(reader.item_stack(), Ok(Some(stack)));
                prop_assert_eq!(reader.finish(), Ok(()));
            }
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = Reader::new(&bytes).item_stack();
            let _ = Reader::new(&bytes).untrusted_item_stack();
        }
    }
}
