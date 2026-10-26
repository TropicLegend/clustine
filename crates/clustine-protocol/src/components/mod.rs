//! Item components as the wire carries them: the game's `DataComponentPatch` over the
//! kinds Clustine reads (`docs/groundwork/n9-plan.md`, sections 3.1 and 3.3), each value
//! by its stream codec, byte for byte the game's.
//!
//! Registry values (a potion, an effect, an enchantment, a banner pattern, a chicken
//! variant, a component kind) are kept here as the numbers the wire gives them; the edge
//! turns them into the names the world keeps, and checks them against the registries.
//! This crate checks only the shape: what the game's stream codecs themselves refuse
//! (a list over its maximum, a string over its length, an id outside a registry the
//! protocol fixes, a value cut short).
//!
//! A server writes a patch's values one after another with nothing between them, so a
//! component Clustine does not know cannot be stepped over: [`Reader::components_clientbound`]
//! refuses it, naming its id. A client puts a length before every value, so
//! [`Reader::components_serverbound`] steps over those and says which it dropped.
//!
//! The patch's own framing (the two counts, then the added, then the removed) is
//! `DataComponentPatch$3`, and the length before a client's value `$2`, whose maximum is
//! `Integer.MAX_VALUE` (n9 DataComponentPatch$2 12–15): no length is refused for its
//! size, here as there. The mechanics of a length-prefixed value are `ByteBufCodecs$30`:
//! the value is read from a slice of its length and what is left of it is let be. The
//! game reads the added components into a map (`Reference2ObjectArrayMap.put`,
//! n9 DataComponentPatch$3 27–96), so a kind added twice is one, the last value in the
//! first one's place; this reader keeps both as read, and the world's patch, set in the
//! order read, comes to the same.

mod values;

#[cfg(test)]
mod tests;

pub use crate::generated::component_ids::{data_component_type, mob_effect, potion};
pub use values::*;

use crate::codec::{DecodeError, Reader, Writer};

/// The `what` of the [`DecodeError::InvalidValue`] for a component outside the set, whose
/// `value` is the component's id.
pub const OUTSIDE_THE_SET: &str = "item component Clustine does not read";

/// How deeply stacks may nest in stacks (a crossbow's projectiles) and effects in
/// effects (an effect's hidden one). The game's stream codecs set no bound and would
/// run out of stack; Clustine refuses before it does.
pub const MAX_NESTING: usize = 16;

/// A stack's components beyond its item's defaults, as the wire has them: the
/// components added, in the order read, and the ids of the kinds removed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WireComponents {
    pub added: Vec<WireComponent>,
    /// Ids in the `data_component_type` registry, of any kind, in the set or not.
    pub removed: Vec<i32>,
}

impl WireComponents {
    /// Whether the patch adds nothing and removes nothing.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// What a client's patch gave: the components of the set it carried, and the ids of
/// those outside it, which were stepped over by their lengths.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerboundComponents {
    pub components: WireComponents,
    /// Ids of added components that were dropped, in the order read: kinds outside
    /// the set, and kinds of the set whose value holds a stack with a kind outside it.
    pub dropped: Vec<i32>,
}

impl Writer {
    /// Writes a patch as a server sends it: the counts of added and removed kinds, each
    /// added component's kind id and its value with nothing before it, then the removed
    /// kinds' ids (`DataComponentPatch.STREAM_CODEC`).
    pub fn put_components(&mut self, components: &WireComponents) {
        self.put_patch(components, false);
    }

    /// Writes a patch as a client sends it, with each value's length before it
    /// (`DataComponentPatch.DELIMITED_STREAM_CODEC`): for the bots and the tests.
    pub fn put_components_serverbound(&mut self, components: &WireComponents) {
        self.put_patch(components, true);
    }

    fn put_patch(&mut self, components: &WireComponents, delimited: bool) {
        self.put_length(components.added.len());
        self.put_length(components.removed.len());
        for component in &components.added {
            self.put_var_int(component.id());
            if delimited {
                let mut value = Writer::new();
                value.put_component(component);
                let value = value.into_bytes();
                self.put_length(value.len());
                self.put_bytes(&value);
            } else {
                self.put_component(component);
            }
        }
        for id in &components.removed {
            self.put_var_int(*id);
        }
    }
}

impl Reader<'_> {
    /// Reads a patch as a server sends it. A component outside the set is refused, with
    /// [`OUTSIDE_THE_SET`] and its id: its value has no length, so what follows it could
    /// not be found.
    pub fn components_clientbound(&mut self) -> Result<WireComponents, DecodeError> {
        read_patch(self, 0)
    }

    /// Reads a patch as a client sends it, each value after its length. The components
    /// of the set are decoded and those outside it are stepped over and named as dropped.
    /// A value is read from its length's bytes alone, and what is left of them is let
    /// be, as the game lets it be.
    pub fn components_serverbound(&mut self) -> Result<ServerboundComponents, DecodeError> {
        let (added, removed) = (self.var_int()?, self.var_int()?);
        let mut read = ServerboundComponents::default();
        for _ in 0..count(added)? {
            let id = component_id(self)?;
            let length = self.length()?;
            let mut value = Reader::new(self.bytes(length)?);
            if !is_in_the_set(id) {
                read.dropped.push(id);
                continue;
            }
            match value.component(id, 0) {
                Ok(component) => read.components.added.push(component),
                Err(DecodeError::InvalidValue { what, .. }) if what == OUTSIDE_THE_SET => {
                    read.dropped.push(id);
                }
                Err(error) => return Err(error),
            }
        }
        for _ in 0..count(removed)? {
            read.components.removed.push(component_id(self)?);
        }
        Ok(read)
    }
}

/// `DataComponentPatch.STREAM_CODEC`'s decoding, `depth` stacks deep.
pub(crate) fn read_patch(r: &mut Reader<'_>, depth: usize) -> Result<WireComponents, DecodeError> {
    let (added, removed) = (r.var_int()?, r.var_int()?);
    let mut patch = WireComponents::default();
    for _ in 0..count(added)? {
        let id = component_id(r)?;
        patch.added.push(r.component(id, depth)?);
    }
    for _ in 0..count(removed)? {
        patch.removed.push(component_id(r)?);
    }
    Ok(patch)
}

/// A patch's count of added or of removed kinds. The game reads it as a plain VarInt
/// and sizes its map by it, which a negative count fails. Every kind takes at least a
/// byte, so a count beyond the input fails at the first kind that is not there, before
/// anything is allocated for it.
fn count(raw: i32) -> Result<usize, DecodeError> {
    usize::try_from(raw).map_err(|_| DecodeError::NegativeLength(raw))
}

/// A kind's id, which the game's `ByteBufCodecs.registry` refuses outside the registry.
fn component_id(r: &mut Reader<'_>) -> Result<i32, DecodeError> {
    let id = r.var_int()?;
    if usize::try_from(id).map_or(true, |id| id >= data_component_type::NAMES.len()) {
        return Err(DecodeError::InvalidValue {
            what: "item component kind",
            value: id.into(),
        });
    }
    Ok(id)
}

/// Whether Clustine reads the component kind `id`.
pub fn is_in_the_set(id: i32) -> bool {
    WireComponent::IDS.contains(&id)
}
