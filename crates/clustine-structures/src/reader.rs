//! Reading the game's data trees (`clustine_worldgen_data::feature::Data`) into the
//! types that place templates, as the game's codecs read the files.
//!
//! The same discipline as the features' reader (`clustine-features/src/reader.rs`), of
//! which this is a copy cut to what processors need: the data is not typed by the
//! emitter, so a reader is where a member the code does not know would be passed over.
//! A [`Reader`] remembers which members of its object were read, and
//! [`Reader::finish`] fails if one was not. The copy is here because the features will
//! build on this crate (fossils and the template feature place templates), so this
//! crate cannot use theirs.

use std::cell::RefCell;
use std::fmt;

use clustine_worldgen_data::feature::Data;

/// Why the data could not be read, with where in the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataError {
    pub at: String,
    pub what: String,
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.what)
    }
}

impl std::error::Error for DataError {}

pub type Read<T> = Result<T, DataError>;

/// A value of the data, with the path that led to it.
pub struct Reader {
    data: &'static Data,
    at: String,
    /// For an object, whether each member has been read.
    read: RefCell<Vec<bool>>,
}

impl fmt::Debug for Reader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Reader({})", self.at)
    }
}

impl Reader {
    /// A reader of `data`, which stands at `at` (a name, as `minecraft:fossil_rot`).
    pub fn new(data: &'static Data, at: impl Into<String>) -> Self {
        let members = match data {
            Data::Object(members) => members.len(),
            _ => 0,
        };
        Self {
            data,
            at: at.into(),
            read: RefCell::new(vec![false; members]),
        }
    }

    pub fn at(&self) -> &str {
        &self.at
    }

    pub fn data(&self) -> &'static Data {
        self.data
    }

    pub fn error(&self, what: impl Into<String>) -> DataError {
        DataError {
            at: self.at.clone(),
            what: what.into(),
        }
    }

    /// The member `key` of this object, if it has one; it counts as read.
    pub fn optional(&self, key: &str) -> Option<Reader> {
        let Data::Object(members) = self.data else {
            return None;
        };
        let index = members.iter().position(|(name, _)| *name == key)?;
        self.read.borrow_mut()[index] = true;
        Some(Reader::new(&members[index].1, format!("{}.{key}", self.at)))
    }

    /// The member `key` of this object.
    pub fn field(&self, key: &str) -> Read<Reader> {
        if !matches!(self.data, Data::Object(_)) {
            return Err(self.error(format!("is not an object, so has no member {key:?}")));
        }
        self.optional(key)
            .ok_or_else(|| self.error(format!("has no member {key:?}")))
    }

    pub fn is_object(&self) -> bool {
        matches!(self.data, Data::Object(_))
    }

    pub fn is_list(&self) -> bool {
        matches!(self.data, Data::List(_))
    }

    pub fn is_number(&self) -> bool {
        matches!(self.data, Data::Number(_))
    }

    pub fn is_text(&self) -> bool {
        matches!(self.data, Data::Text(_))
    }

    pub fn text(&self) -> Read<&'static str> {
        match self.data {
            Data::Text(text) => Ok(text),
            _ => Err(self.error("is not a string")),
        }
    }

    fn number(&self) -> Read<&'static str> {
        match self.data {
            Data::Number(text) => Ok(text),
            _ => Err(self.error("is not a number")),
        }
    }

    /// An integer, as a codec of `int` reads it: a number with a fraction is refused.
    pub fn int(&self) -> Read<i32> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| self.error(format!("{text} is not an int")))
    }

    /// A 32-bit float, as a codec of `float` reads it: parsed once to the nearest.
    pub fn float(&self) -> Read<f32> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| self.error(format!("{text} is not a float")))
    }

    /// The items of a list.
    pub fn list(&self) -> Read<Vec<Reader>> {
        match self.data {
            Data::List(items) => Ok(items
                .iter()
                .enumerate()
                .map(|(index, item)| Reader::new(item, format!("{}[{index}]", self.at)))
                .collect()),
            _ => Err(self.error("is not a list")),
        }
    }

    /// Fails if a member of this object was not read.
    pub fn finish(&self) -> Read<()> {
        let Data::Object(members) = self.data else {
            return Ok(());
        };
        let read = self.read.borrow();
        let unread: Vec<&str> = members
            .iter()
            .zip(read.iter())
            .filter(|(_, read)| !**read)
            .map(|((name, _), _)| *name)
            .collect();
        if unread.is_empty() {
            Ok(())
        } else {
            Err(self.error(format!("has members no code reads: {unread:?}")))
        }
    }
}
