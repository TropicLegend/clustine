//! How a server ends a connection, and the reason it gives.
//!
//! A reason is a text component. In the login state it travels as JSON, afterwards as
//! NBT, and in either it is a plain string or says more: the official server names a
//! sentence of the game by its key, which each client shows in its own language. A bot
//! reads all of these and keeps what was on the wire beside what it made of it, since a
//! comparison with the official server is of the bytes.

use std::fmt;

use anyhow::{Context, Result};
use clustine_protocol::codec::Reader;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::text::Text;
use serde_json::Value;

/// A reason as it was sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Wire {
    /// From the configuration or the play state: the bytes of the packet after its id,
    /// and the value they hold.
    Nbt { bytes: Vec<u8>, value: Nbt },
    /// From the login state.
    Json(String),
}

impl fmt::Display for Wire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nbt { value, .. } => write!(f, "{value}"),
            Self::Json(json) => f.write_str(json),
        }
    }
}

/// The reason a server gave for ending a connection.
#[derive(Debug, Clone, PartialEq)]
pub struct Reason {
    pub wire: Wire,
    /// What the bot read: a literal, the key of a sentence, or a component that says
    /// more than either.
    pub text: Text,
}

impl Reason {
    /// Reads the reason from a disconnect packet of the configuration or the play
    /// state, which have the same layout. `frame` is the unframed packet: its id, then
    /// its body.
    pub fn from_frame(frame: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(frame);
        reader.var_int()?;
        let bytes = frame[frame.len() - reader.remaining()..].to_vec();
        let value = reader
            .nbt()?
            .context("the disconnect packet has no reason")?;
        reader.finish()?;
        Ok(Self {
            text: Text::from_nbt(value.clone()),
            wire: Wire::Nbt { bytes, value },
        })
    }

    /// Reads the reason of a disconnect packet of the login state. Text that is not
    /// JSON is taken as it is.
    pub fn from_json(json: &str) -> Self {
        let text = match serde_json::from_str::<Value>(json) {
            Ok(value) => Text::from_nbt(nbt_from_json(&value)),
            Err(_) => Text::literal(json),
        };
        Self {
            wire: Wire::Json(json.to_owned()),
            text,
        }
    }

    /// The key of the sentence the reason names, if it names one.
    pub fn translation_key(&self) -> Option<&str> {
        self.text.translation_key()
    }
}

/// What was read, then what was sent, with the bytes in hexadecimal.
impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.text {
            Text::Literal(text) => write!(f, "the literal text {text:?}")?,
            Text::Translatable(key) => write!(f, "the sentence with the key {key:?}")?,
            Text::Other(_) => match self.translation_key() {
                Some(key) => write!(f, "a component with more than the key {key:?}")?,
                None => f.write_str("a component that is neither a string nor a key")?,
            },
        }
        match &self.wire {
            Wire::Json(json) => write!(f, "; sent as the JSON {json}"),
            Wire::Nbt { bytes, value } => {
                write!(f, "; sent as the NBT {value}, which is the bytes")?;
                for byte in bytes {
                    write!(f, " {byte:02x}")?;
                }
                Ok(())
            }
        }
    }
}

/// How a bot's connection ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// The server sent a disconnect packet.
    Disconnected(Reason),
    /// The connection ended without one; the text says how.
    Closed(String),
    /// The bot gave up: the server sent something a client cannot make sense of.
    Failed(String),
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected(reason) => write!(f, "a disconnect packet with {reason}"),
            Self::Closed(how) => write!(f, "no disconnect packet: {how}"),
            Self::Failed(why) => write!(f, "the bot gave up: {why}"),
        }
    }
}

/// A text component of the login state in the shape the later states use, so that one
/// reading serves both. Numbers become the narrowest of int, long and double.
fn nbt_from_json(value: &Value) -> Nbt {
    match value {
        Value::Null => Nbt::String(String::new()),
        Value::Bool(flag) => Nbt::Byte(i8::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(whole) => i32::try_from(whole).map_or(Nbt::Long(whole), Nbt::Int),
            None => Nbt::Double(number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(text) => Nbt::String(text.clone()),
        Value::Array(items) => Nbt::List(items.iter().map(nbt_from_json).collect()),
        Value::Object(entries) => Nbt::Compound(
            entries
                .iter()
                .map(|(name, value)| (name.clone(), nbt_from_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use clustine_protocol::packets::configuration::Disconnect as ConfigurationDisconnect;
    use clustine_protocol::packets::encode;
    use clustine_protocol::packets::play::{ClientboundPlay, Disconnect};

    use super::*;

    const KEY: &str = "multiplayer.disconnect.duplicate_login";

    /// What a bot makes of a disconnect packet of the play state, as `Bot::step` does.
    fn read(packet: &Disconnect) -> Reason {
        let frame = encode(packet);
        assert!(matches!(
            ClientboundPlay::decode(&frame),
            Ok(ClientboundPlay::Disconnect(_))
        ));
        Reason::from_frame(&frame).unwrap()
    }

    #[test]
    fn a_bot_reads_a_reason_that_is_a_plain_string() {
        let packet = Disconnect::from(Text::literal("You are logged in already"));
        let reason = read(&packet);
        assert_eq!(reason.text, Text::literal("You are logged in already"));
        assert_eq!(reason.translation_key(), None);
        let Wire::Nbt { bytes, value } = &reason.wire else {
            panic!("not NBT: {reason:?}");
        };
        assert_eq!(*value, packet.reason);
        // The string tag, its length and its text.
        assert_eq!(bytes[..3], [8, 0, 25]);
        assert_eq!(
            reason.to_string(),
            "the literal text \"You are logged in already\"; sent as the NBT \
             \"You are logged in already\", which is the bytes 08 00 19 59 6f 75 20 61 72 65 \
             20 6c 6f 67 67 65 64 20 69 6e 20 61 6c 72 65 61 64 79"
        );
    }

    #[test]
    fn a_bot_reads_a_reason_that_names_a_sentence_of_the_game() {
        let packet = Disconnect::from(Text::translatable(KEY));
        let reason = read(&packet);
        assert_eq!(reason.text, Text::translatable(KEY));
        assert_eq!(reason.translation_key(), Some(KEY));
        let Wire::Nbt { bytes, value } = &reason.wire else {
            panic!("not NBT: {reason:?}");
        };
        assert_eq!(*value, packet.reason);
        // A compound that begins with a string entry and ends with the end tag.
        assert_eq!((bytes[0], bytes[1], bytes[bytes.len() - 1]), (10, 8, 0));
        assert!(reason.to_string().starts_with(&format!(
            "the sentence with the key {KEY:?}; sent as the NBT"
        )));
    }

    #[test]
    fn a_bot_reads_both_forms_in_the_configuration_state_too() {
        for text in [Text::literal("bye"), Text::translatable(KEY)] {
            let frame = encode(&ConfigurationDisconnect {
                reason: text.to_nbt(),
            });
            assert_eq!(Reason::from_frame(&frame).unwrap().text, text);
        }
    }

    #[test]
    fn a_reason_that_says_more_than_a_key_still_gives_the_key() {
        let component = Nbt::Compound(vec![
            ("translate".to_owned(), Nbt::String(KEY.to_owned())),
            ("with".to_owned(), Nbt::List(vec![Nbt::Int(1)])),
        ]);
        let reason = read(&Disconnect {
            reason: component.clone(),
        });
        assert_eq!(reason.text, Text::Other(component));
        assert_eq!(reason.translation_key(), Some(KEY));
    }

    #[test]
    fn a_bot_reads_both_forms_as_json_in_the_login_state() {
        let plain = Reason::from_json(r#""Server is full""#);
        assert_eq!(plain.text, Text::literal("Server is full"));
        assert_eq!(plain.wire.to_string(), r#""Server is full""#);

        let sentence = Reason::from_json(&format!(r#"{{"translate":"{KEY}"}}"#));
        assert_eq!(sentence.text, Text::translatable(KEY));

        let more = Reason::from_json(r#"{"translate":"a.b","with":[3,true,1.5]}"#);
        assert_eq!(more.translation_key(), Some("a.b"));
        assert!(matches!(more.text, Text::Other(_)));

        assert_eq!(
            Reason::from_json("not json").text,
            Text::literal("not json")
        );
    }

    #[test]
    fn a_disconnect_without_a_reason_is_not_read() {
        // An id, then the lone end tag that stands for "no value".
        assert!(Reason::from_frame(&[0, 0]).is_err());
    }
}
