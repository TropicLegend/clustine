//! Text components: what a client shows as text, such as the reason of a disconnect.
//!
//! In the configuration and play states a text component travels as NBT. Its simplest
//! form is a single string tag, which is shown as it is. A compound with the key
//! `translate` names a sentence of the game instead, which each client shows in its own
//! language. Only these two forms are modelled; any other component is kept as the NBT
//! it came as, so that nothing is lost in reading one.

use crate::nbt::Nbt;

/// The key of a compound whose value names a sentence of the game.
const TRANSLATE: &str = "translate";

/// A text component.
#[derive(Debug, Clone, PartialEq)]
pub enum Text {
    /// Shown as it is: a string tag.
    Literal(String),
    /// A sentence of the game by its key, without arguments: the compound
    /// `{translate: key}` and nothing else.
    Translatable(String),
    /// Any other component, as it came.
    Other(Nbt),
}

impl Text {
    /// Text shown as it is.
    pub fn literal(text: impl Into<String>) -> Self {
        Self::Literal(text.into())
    }

    /// A sentence of the game, such as `multiplayer.disconnect.kicked`.
    pub fn translatable(key: impl Into<String>) -> Self {
        Self::Translatable(key.into())
    }

    /// Reads a component. Every value is one, so this cannot fail, and [`Text::to_nbt`]
    /// gives back exactly what was read.
    pub fn from_nbt(value: Nbt) -> Self {
        match value {
            Nbt::String(text) => Self::Literal(text),
            Nbt::Compound(entries) => match <[(String, Nbt); 1]>::try_from(entries) {
                Ok([(name, Nbt::String(key))]) if name == TRANSLATE => Self::Translatable(key),
                Ok(entries) => Self::Other(Nbt::Compound(entries.into())),
                Err(entries) => Self::Other(Nbt::Compound(entries)),
            },
            other => Self::Other(other),
        }
    }

    /// The component as it goes into a packet.
    pub fn to_nbt(&self) -> Nbt {
        match self {
            Self::Literal(text) => Nbt::String(text.clone()),
            Self::Translatable(key) => {
                Nbt::Compound(vec![(TRANSLATE.to_owned(), Nbt::String(key.clone()))])
            }
            Self::Other(value) => value.clone(),
        }
    }

    /// The key of the sentence this component names, if it names one: also for a
    /// component that says more besides, such as arguments or a colour.
    pub fn translation_key(&self) -> Option<&str> {
        match self {
            Self::Literal(_) => None,
            Self::Translatable(key) => Some(key),
            Self::Other(value) => match value.get(TRANSLATE) {
                Some(Nbt::String(key)) => Some(key),
                _ => None,
            },
        }
    }
}

impl From<Text> for Nbt {
    fn from(text: Text) -> Self {
        text.to_nbt()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::codec::{Reader, Writer};

    fn written(value: &Nbt) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.put_nbt(value);
        writer.into_bytes()
    }

    #[test]
    fn a_literal_is_a_string_tag() {
        let text = Text::literal("bye");
        assert_eq!(text.to_nbt(), Nbt::String("bye".to_owned()));
        assert_eq!(Text::from_nbt(text.to_nbt()), text);
        assert_eq!(text.translation_key(), None);
    }

    #[test]
    fn a_translatable_text_known_answer() {
        let text = Text::translatable("a.b");
        // A compound with one string entry called "translate", then the end tag.
        let mut expected = vec![10, 8, 0, 9];
        expected.extend_from_slice(b"translate");
        expected.extend_from_slice(&[0, 3]);
        expected.extend_from_slice(b"a.b");
        expected.push(0);
        assert_eq!(written(&text.to_nbt()), expected);

        let read = Reader::new(&expected).nbt().unwrap().unwrap();
        assert_eq!(Text::from_nbt(read), text);
        assert_eq!(text.translation_key(), Some("a.b"));
    }

    #[test]
    fn a_component_that_says_more_is_kept_as_it_came() {
        let value = Nbt::Compound(vec![
            ("translate".to_owned(), Nbt::String("a.b".to_owned())),
            ("color".to_owned(), Nbt::String("red".to_owned())),
        ]);
        let text = Text::from_nbt(value.clone());
        assert_eq!(text, Text::Other(value.clone()));
        assert_eq!(text.translation_key(), Some("a.b"));
        assert_eq!(text.to_nbt(), value);

        // A key that is not a string names no sentence.
        let odd = Nbt::Compound(vec![("translate".to_owned(), Nbt::Int(1))]);
        assert_eq!(Text::from_nbt(odd.clone()), Text::Other(odd));
        assert_eq!(
            Text::from_nbt(Nbt::Compound(Vec::new())).translation_key(),
            None
        );
    }

    proptest! {
        #[test]
        fn texts_round_trip(literal: bool, content: String) {
            let text = if literal {
                Text::Literal(content)
            } else {
                Text::Translatable(content)
            };
            let bytes = written(&text.to_nbt());
            let mut reader = Reader::new(&bytes);
            let read = reader.nbt().unwrap().unwrap();
            prop_assert_eq!(reader.finish(), Ok(()));
            prop_assert_eq!(Text::from_nbt(read), text);
        }
    }
}
