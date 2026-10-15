//! Reading a feature's data (`clustine_worldgen_data::feature::Data`) into the types
//! that place it, as the game's codecs read the files.
//!
//! The data is a tree that mirrors the jar's files and is not typed by the emitter, so
//! a reader is where a member the code does not know would be passed over. A
//! [`Reader`] remembers which members of its object were read, and
//! [`Reader::finish`] fails if one was not: every type's parser ends with it.

use std::cell::RefCell;
use std::fmt;

use clustine_worldgen_data::feature::Data;

/// Why a feature's data could not be read, with where in the data.
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

/// A value of a feature's data, with the path that led to it.
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
    /// A reader of `data`, which stands at `at` (a name, as `minecraft:oak`).
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

    /// Where this value stands, for errors.
    pub fn at(&self) -> &str {
        &self.at
    }

    /// The value itself.
    pub fn data(&self) -> &'static Data {
        self.data
    }

    /// An error at this value.
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

    /// Whether this object has the member `key`, without reading it.
    pub fn has(&self, key: &str) -> bool {
        self.data.get(key).is_some()
    }

    /// What the member `type` names; it counts as read.
    pub fn kind(&self) -> Read<&'static str> {
        self.field("type")?.text()
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

    pub fn boolean(&self) -> Read<bool> {
        match self.data {
            Data::Bool(value) => Ok(*value),
            _ => Err(self.error("is not true or false")),
        }
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

    pub fn long(&self) -> Read<i64> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| self.error(format!("{text} is not a long")))
    }

    /// A 32-bit float, as a codec of `float` reads it: parsed once to the nearest.
    pub fn float(&self) -> Read<f32> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| self.error(format!("{text} is not a float")))
    }

    /// A 64-bit float, as a codec of `double` reads it.
    pub fn double(&self) -> Read<f64> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| self.error(format!("{text} is not a double")))
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

    /// The items of a list, or this value alone where a codec takes either (the
    /// game's `ExtraCodecs.compactListCodec` and the like).
    pub fn one_or_list(&self) -> Vec<Reader> {
        match self.list() {
            Ok(items) => items,
            Err(_) => vec![Reader::new(self.data, self.at.clone())],
        }
    }

    /// Marks every member of this object as read: for a kind whose code is not built
    /// yet, which reads nothing and places nothing.
    pub fn skip_rest(&self) {
        self.read.borrow_mut().fill(true);
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

#[cfg(test)]
mod tests {
    use super::*;

    static ORE: Data = Data::Object(&[
        ("type", Data::Text("minecraft:ore")),
        ("size", Data::Number("9")),
        ("discard_chance_on_air_exposure", Data::Number("0.5")),
        ("flag", Data::Bool(true)),
        ("targets", Data::List(&[Data::Text("a"), Data::Text("b")])),
    ]);

    #[test]
    fn members_are_read_as_the_codecs_read_them() {
        let reader = Reader::new(&ORE, "minecraft:ore_iron");
        assert_eq!(reader.kind(), Ok("minecraft:ore"));
        assert_eq!(reader.field("size").and_then(|size| size.int()), Ok(9));
        assert_eq!(
            reader
                .field("discard_chance_on_air_exposure")
                .and_then(|chance| chance.float()),
            Ok(0.5)
        );
        assert_eq!(
            reader.field("flag").and_then(|flag| flag.boolean()),
            Ok(true)
        );
        let targets = reader
            .field("targets")
            .and_then(|targets| targets.list())
            .unwrap();
        assert_eq!(targets[1].text(), Ok("b"));
        assert_eq!(targets[1].at(), "minecraft:ore_iron.targets[1]");
        assert_eq!(reader.finish(), Ok(()));
    }

    #[test]
    fn a_member_left_unread_fails_the_reading() {
        let reader = Reader::new(&ORE, "minecraft:ore_iron");
        reader.kind().unwrap();
        reader.field("size").unwrap();
        let error = reader.finish().unwrap_err();
        assert!(
            error.what.contains("discard_chance_on_air_exposure"),
            "{error}"
        );
        assert!(error.what.contains("targets"), "{error}");
        reader.skip_rest();
        assert_eq!(reader.finish(), Ok(()));
    }

    #[test]
    fn a_number_with_a_fraction_is_not_an_int_and_a_missing_member_is_named() {
        let reader = Reader::new(&ORE, "x");
        assert!(
            reader
                .field("discard_chance_on_air_exposure")
                .unwrap()
                .int()
                .is_err()
        );
        assert!(reader.field("radius").unwrap_err().what.contains("radius"));
        assert!(reader.optional("radius").is_none());
    }
}
