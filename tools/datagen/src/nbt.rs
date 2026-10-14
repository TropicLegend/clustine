// Written for Clustine from the format's public description (the game's `NbtIo`
// reads the same bytes).

//! Reads the game's binary tags (NBT), as far as a structure template needs: the
//! whole tree of a file that is compressed with gzip, kept in the file's order.

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};

/// One tag. Arrays of numbers are kept as lists of numbers.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Integer(i64),
    Float(f64),
    Text(String),
    List(Vec<Tag>),
    /// The members in the file's order.
    Compound(Vec<(String, Tag)>),
}

impl Tag {
    /// The member `name` of a compound, if it has one.
    pub fn get(&self, name: &str) -> Option<&Tag> {
        match self {
            Self::Compound(members) => members
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The member `name`, which has to be there.
    pub fn member(&self, name: &str) -> Result<&Tag> {
        self.get(name)
            .with_context(|| format!("a tag has no member {name:?}"))
    }

    pub fn list(&self) -> Result<&[Tag]> {
        match self {
            Self::List(items) => Ok(items),
            _ => bail!("a tag is no list"),
        }
    }

    pub fn text(&self) -> Result<&str> {
        match self {
            Self::Text(text) => Ok(text),
            _ => bail!("a tag is no text"),
        }
    }

    pub fn integer(&self) -> Result<i64> {
        match self {
            Self::Integer(number) => Ok(*number),
            _ => bail!("a tag is no whole number"),
        }
    }
}

/// How deep a file's tags may nest: the game's own limit.
const DEPTH: usize = 512;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8]> {
        ensure!(
            self.bytes.len() - self.at >= count,
            "the file ends inside a tag"
        );
        let taken = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(taken)
    }

    fn number(&mut self, bytes: usize) -> Result<i64> {
        let mut value: i64 = 0;
        for (index, byte) in self.take(bytes)?.iter().enumerate() {
            value = if index == 0 {
                i64::from(*byte as i8)
            } else {
                value << 8 | i64::from(*byte)
            };
        }
        Ok(value)
    }

    fn length(&mut self) -> Result<usize> {
        usize::try_from(self.number(4)?).context("a length below zero")
    }

    fn text(&mut self) -> Result<String> {
        let length = usize::try_from(self.number(2)? & 0xffff)?;
        // The game writes Java's modified UTF-8, which is UTF-8 for everything a
        // template names.
        Ok(std::str::from_utf8(self.take(length)?)
            .context("a text is not UTF-8")?
            .to_owned())
    }

    fn payload(&mut self, kind: u8, depth: usize) -> Result<Tag> {
        ensure!(depth < DEPTH, "the tags nest deeper than {DEPTH}");
        Ok(match kind {
            1 => Tag::Integer(self.number(1)?),
            2 => Tag::Integer(self.number(2)?),
            3 => Tag::Integer(self.number(4)?),
            4 => Tag::Integer(self.number(8)?),
            5 => {
                let bits = u32::try_from(self.number(4)? & 0xffff_ffff)?;
                Tag::Float(f64::from(f32::from_bits(bits)))
            }
            6 => Tag::Float(f64::from_bits(self.number(8)? as u64)),
            7 | 11 | 12 => {
                let width = match kind {
                    7 => 1,
                    11 => 4,
                    _ => 8,
                };
                let length = self.length()?;
                let mut items = Vec::new();
                for _ in 0..length {
                    items.push(Tag::Integer(self.number(width)?));
                }
                Tag::List(items)
            }
            8 => Tag::Text(self.text()?),
            9 => {
                let of = self.number(1)? as u8;
                let length = self.length()?;
                let mut items = Vec::new();
                for _ in 0..length {
                    items.push(self.payload(of, depth + 1)?);
                }
                Tag::List(items)
            }
            10 => {
                let mut members = Vec::new();
                loop {
                    let kind = self.number(1)? as u8;
                    if kind == 0 {
                        break;
                    }
                    let name = self.text()?;
                    members.push((name, self.payload(kind, depth + 1)?));
                }
                Tag::Compound(members)
            }
            other => bail!("a tag of the kind {other} is not known"),
        })
    }
}

/// The root compound of a file of tags that is compressed with gzip.
pub fn read_gzip(bytes: &[u8]) -> Result<Tag> {
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut plain)
        .context("unpacking a file of tags")?;
    read(&plain)
}

/// The root compound of a file of tags.
pub fn read(bytes: &[u8]) -> Result<Tag> {
    let mut reader = Reader { bytes, at: 0 };
    let kind = reader.number(1)? as u8;
    ensure!(kind == 10, "a file of tags starts with a compound");
    reader.text()?;
    let root = reader.payload(10, 0)?;
    ensure!(reader.at == bytes.len(), "bytes follow the root tag");
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compound `{size: [I; 3, -2], name: "a", inner: [{b: 1b}]}` by hand.
    fn sample() -> Vec<u8> {
        let mut bytes = vec![10, 0, 0];
        bytes.extend([11, 0, 4]);
        bytes.extend(b"size");
        bytes.extend([0, 0, 0, 2, 0, 0, 0, 3, 0xff, 0xff, 0xff, 0xfe]);
        bytes.extend([8, 0, 4]);
        bytes.extend(b"name");
        bytes.extend([0, 1, b'a']);
        bytes.extend([9, 0, 5]);
        bytes.extend(b"inner");
        bytes.extend([10, 0, 0, 0, 1, 1, 0, 1, b'b', 1, 0]);
        bytes.push(0);
        bytes
    }

    #[test]
    fn a_file_of_tags_reads_as_its_tree_in_the_files_order() {
        let root = read(&sample()).unwrap();
        assert_eq!(
            root.member("size").unwrap().list().unwrap(),
            [Tag::Integer(3), Tag::Integer(-2)]
        );
        assert_eq!(root.member("name").unwrap().text().unwrap(), "a");
        let inner = root.member("inner").unwrap().list().unwrap();
        assert_eq!(inner[0].member("b").unwrap().integer().unwrap(), 1);
        let Tag::Compound(members) = &root else {
            panic!("the root is a compound");
        };
        let names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["size", "name", "inner"]);
    }

    #[test]
    fn a_file_that_is_cut_short_or_goes_on_is_refused() {
        let bytes = sample();
        assert!(read(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(read(&longer).is_err());
        assert!(read(&[8, 0, 0]).is_err());
    }
}
