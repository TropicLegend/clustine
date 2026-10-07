//! Item stacks as they appear inside packets.
//!
//! An item stack is a count, an item and a patch of "components" that deviate from the
//! item's defaults, such as a custom name or enchantments. Clustine does not model
//! components yet. It writes stacks without any, and when reading it keeps only the
//! item and the count.

use crate::codec::{DecodeError, Reader, Writer};

/// A number of items of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
}

impl Writer {
    /// Writes an item stack without components, or an empty slot.
    pub fn put_item_stack(&mut self, stack: Option<ItemStack>) {
        match stack {
            Some(stack) if stack.count > 0 => {
                self.put_var_int(stack.count);
                self.put_var_int(stack.item);
                // No components added and none removed.
                self.put_var_int(0);
                self.put_var_int(0);
            }
            _ => self.put_var_int(0),
        }
    }
}

impl Reader<'_> {
    /// Reads an item stack the way servers send it. `None` is an empty slot.
    ///
    /// Fails for a stack with components: servers send them without lengths, so they
    /// cannot be skipped without knowing every component's format.
    pub fn item_stack(&mut self) -> Result<Option<ItemStack>, DecodeError> {
        let Some(stack) = self.item_and_count()? else {
            return Ok(None);
        };
        let (added, removed) = (self.var_int()?, self.var_int()?);
        if added != 0 || removed != 0 {
            return Err(DecodeError::InvalidValue {
                what: "item stack without components",
                value: i64::from(added) + i64::from(removed),
            });
        }
        Ok(Some(stack))
    }

    /// Reads an item stack the way clients send it, ignoring its components. `None` is
    /// an empty slot.
    ///
    /// Clients prefix every component with its length, so that a server can skip those
    /// it does not understand.
    pub fn untrusted_item_stack(&mut self) -> Result<Option<ItemStack>, DecodeError> {
        let Some(stack) = self.item_and_count()? else {
            return Ok(None);
        };
        let added = self.length()?;
        let removed = self.length()?;
        for _ in 0..added {
            self.var_int()?;
            let length = self.length()?;
            self.bytes(length)?;
        }
        for _ in 0..removed {
            self.var_int()?;
        }
        Ok(Some(stack))
    }

    fn item_and_count(&mut self) -> Result<Option<ItemStack>, DecodeError> {
        let count = self.var_int()?;
        if count <= 0 {
            return Ok(None);
        }
        Ok(Some(ItemStack {
            item: self.var_int()?,
            count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn written(stack: Option<ItemStack>) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.put_item_stack(stack);
        writer.into_bytes()
    }

    #[test]
    fn known_answers() {
        assert_eq!(written(None), [0]);
        let stone = ItemStack { item: 1, count: 64 };
        assert_eq!(written(Some(stone)), [64, 1, 0, 0]);
        // A stack of nothing is an empty slot.
        assert_eq!(written(Some(ItemStack { item: 1, count: 0 })), [0]);
    }

    #[test]
    fn plain_stacks_read_the_same_either_way() {
        let stone = ItemStack { item: 1, count: 64 };
        for bytes in [written(Some(stone)), written(None)] {
            let trusted = Reader::new(&bytes).item_stack().unwrap();
            let untrusted = Reader::new(&bytes).untrusted_item_stack().unwrap();
            assert_eq!(trusted, untrusted);
        }
        assert_eq!(Reader::new(&[64, 1, 0, 0]).item_stack(), Ok(Some(stone)));
    }

    #[test]
    fn components_from_a_client_are_skipped() {
        // One stone with two added components (type 5 with three bytes, type 9 with
        // none) and one removed component (type 12), followed by another field.
        let bytes = [1, 1, 2, 1, 5, 3, 0xAA, 0xBB, 0xCC, 9, 0, 12, 0x7F];
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            reader.untrusted_item_stack(),
            Ok(Some(ItemStack { item: 1, count: 1 }))
        );
        assert_eq!(reader.u8(), Ok(0x7F));
    }

    #[test]
    fn components_from_a_server_are_refused() {
        assert!(Reader::new(&[1, 1, 1, 0, 5, 0]).item_stack().is_err());
    }

    proptest! {
        #[test]
        fn stacks_round_trip(item in 0..2000i32, count in 1..100i32) {
            let stack = ItemStack { item, count };
            let bytes = written(Some(stack));
            let mut reader = Reader::new(&bytes);
            prop_assert_eq!(reader.item_stack(), Ok(Some(stack)));
            prop_assert_eq!(reader.finish(), Ok(()));
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = Reader::new(&bytes).item_stack();
            let _ = Reader::new(&bytes).untrusted_item_stack();
        }
    }
}
