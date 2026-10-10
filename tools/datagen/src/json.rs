//! A reader of JSON that keeps every number as the text the file has.
//!
//! The game reads most numbers of its world-generation data as a `float`. Read through
//! a `double` first, a decimal can land on another `float` than read directly, and
//! ADR-0019 (section 1) has the emitter fail where the two differ rather than choose in
//! silence. `serde_json` gives a number as a `double` only, so the files of
//! `worldgen/**` are read here. An object keeps its members in the file's order, which
//! is the order the game reads lists of features and rules in.

use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number as the file writes it.
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// The members in the file's order.
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Self> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = reader.value(0)?;
        reader.skip_white_space();
        ensure!(
            reader.at == reader.bytes.len(),
            "something follows the value at byte {}",
            reader.at
        );
        Ok(value)
    }

    /// The member `key` of an object, if this is one and has it.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The member `key`, which has to be there.
    pub fn member(&self, key: &str) -> Result<&Json> {
        self.get(key)
            .with_context(|| format!("there is no {key:?} in {}", self.kind()))
    }

    pub fn members(&self) -> Result<&[(String, Json)]> {
        match self {
            Json::Object(members) => Ok(members),
            other => bail!("an object is expected, not {}", other.kind()),
        }
    }

    pub fn array(&self) -> Result<&[Json]> {
        match self {
            Json::Array(values) => Ok(values),
            other => bail!("a list is expected, not {}", other.kind()),
        }
    }

    pub fn string(&self) -> Result<&str> {
        match self {
            Json::String(text) => Ok(text),
            other => bail!("a string is expected, not {}", other.kind()),
        }
    }

    pub fn boolean(&self) -> Result<bool> {
        match self {
            Json::Bool(value) => Ok(*value),
            other => bail!("true or false is expected, not {}", other.kind()),
        }
    }

    /// A number the game reads as a `float`: the decimal read to single precision. It
    /// fails if reading it through a `double` gives another value.
    pub fn float(&self) -> Result<f32> {
        let text = self.number()?;
        let direct: f32 = text
            .parse()
            .map_err(|_| anyhow::anyhow!("{text} is not a number"))?;
        let through_double = self.double()? as f32;
        ensure!(
            direct.to_bits() == through_double.to_bits(),
            "{text} is {direct:?} read as a float and {through_double:?} read through a double; \
             which of the two the game takes has to be found out before this is emitted"
        );
        ensure!(direct.is_finite(), "{text} does not fit a float");
        Ok(direct)
    }

    /// A number the game reads as a `double`.
    pub fn double(&self) -> Result<f64> {
        let text = self.number()?;
        let value: f64 = text
            .parse()
            .map_err(|_| anyhow::anyhow!("{text} is not a number"))?;
        ensure!(value.is_finite(), "{text} does not fit a double");
        Ok(value)
    }

    /// A whole number. `3.0` is refused: the game's integer fields are written
    /// without a fraction.
    pub fn integer(&self) -> Result<i32> {
        let text = self.number()?;
        text.parse()
            .map_err(|_| anyhow::anyhow!("{text} is not a whole number that fits 32 bits"))
    }

    fn number(&self) -> Result<&str> {
        match self {
            Json::Number(text) => Ok(text),
            other => bail!("a number is expected, not {}", other.kind()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "true or false",
            Json::Number(_) => "a number",
            Json::String(_) => "a string",
            Json::Array(_) => "a list",
            Json::Object(_) => "an object",
        }
    }
}

/// How deep lists and objects may nest. The deepest of the game's files is far below.
const MAX_DEPTH: usize = 256;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn skip_white_space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        ensure!(
            self.bytes.get(self.at) == Some(&byte),
            "{:?} is expected at byte {}",
            char::from(byte),
            self.at
        );
        self.at += 1;
        Ok(())
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json> {
        ensure!(
            self.bytes[self.at..].starts_with(word.as_bytes()),
            "{word} is expected at byte {}",
            self.at
        );
        self.at += word.len();
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> Result<Json> {
        ensure!(depth <= MAX_DEPTH, "nested deeper than {MAX_DEPTH}");
        self.skip_white_space();
        match self.bytes.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let mut members: Vec<(String, Json)> = Vec::new();
                self.skip_white_space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                loop {
                    self.skip_white_space();
                    let key = self.string()?;
                    ensure!(
                        members.iter().all(|(name, _)| *name != key),
                        "the member {key:?} is there twice"
                    );
                    self.skip_white_space();
                    self.expect(b':')?;
                    members.push((key, self.value(depth + 1)?));
                    self.skip_white_space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(members));
                        }
                        _ => bail!("',' or '}}' is expected at byte {}", self.at),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut values = Vec::new();
                self.skip_white_space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(values));
                }
                loop {
                    values.push(self.value(depth + 1)?);
                    self.skip_white_space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(values));
                        }
                        _ => bail!("',' or ']' is expected at byte {}", self.at),
                    }
                }
            }
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(other) => bail!(
                "a value cannot start with {:?} (byte {})",
                char::from(*other),
                self.at
            ),
            None => bail!("the text ends where a value is expected"),
        }
    }

    fn number(&mut self) -> Result<Json> {
        let start = self.at;
        while matches!(
            self.bytes.get(self.at),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])?;
        // Rust reads more than JSON allows (`inf`, a leading `+`, `1.`); the bytes
        // taken above leave only the last two to refuse.
        let digits = text.strip_prefix('-').unwrap_or(text);
        ensure!(
            digits.starts_with(|c: char| c.is_ascii_digit())
                && !text.ends_with('.')
                && !text.contains(".e")
                && !text.contains(".E")
                && text.parse::<f64>().is_ok(),
            "{text:?} at byte {start} is not a number"
        );
        Ok(Json::Number(text.to_owned()))
    }

    fn string(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.at;
            while !matches!(self.bytes.get(self.at), Some(b'"' | b'\\') | None) {
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(&self.bytes[start..self.at])?);
            match self.bytes.get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escape = *self
                        .bytes
                        .get(self.at)
                        .context("the text ends inside a string")?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&first) {
                                // A character beyond the first 65,536 is written as two
                                // escapes.
                                self.expect(b'\\')?;
                                self.expect(b'u')?;
                                let second = self.hex4()?;
                                ensure!(
                                    (0xdc00..0xe000).contains(&second),
                                    "half an escaped character at byte {}",
                                    self.at
                                );
                                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).with_context(|| {
                                format!("an escape at byte {} is no character", self.at)
                            })?);
                        }
                        other => bail!("unknown escape \\{}", char::from(other)),
                    }
                }
                _ => bail!("the text ends inside a string"),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .context("the text ends inside an escape")?;
        self.at += 4;
        let text = std::str::from_utf8(digits)?;
        ensure!(
            text.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{text:?} is not four hexadecimal digits"
        );
        Ok(u32::from_str_radix(text, 16)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_keeps_its_members_in_the_files_order_and_numbers_as_text() {
        let json = Json::parse(
            r#" { "z": 1, "a": [true, null, "x\n\u00e9\ud83d\ude00"], "m": -0.1e-3 } "#,
        )
        .unwrap();
        let members = json.members().unwrap();
        let names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["z", "a", "m"]);
        assert_eq!(json.member("z").unwrap().integer().unwrap(), 1);
        assert_eq!(
            json.member("m").unwrap(),
            &Json::Number("-0.1e-3".to_owned())
        );
        let list = json.member("a").unwrap().array().unwrap();
        assert!(list[0].boolean().unwrap());
        assert_eq!(list[1], Json::Null);
        assert_eq!(list[2].string().unwrap(), "x\n\u{e9}\u{1f600}");
        assert!(json.get("missing").is_none());
        assert!(json.member("missing").is_err());
    }

    #[test]
    fn what_is_not_json_is_refused() {
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\":1,\"a\":2}",
            "01x",
            "+1",
            "1.",
            ".5",
            "1.e3",
            "inf",
            "NaN",
            "\"open",
            "\"\\q\"",
            "\"\\ud83d\"",
            "1 2",
            "tru",
        ] {
            assert!(Json::parse(text).is_err(), "{text:?}");
        }
        let deep = "[".repeat(300) + &"]".repeat(300);
        assert!(Json::parse(&deep).is_err());
    }

    #[test]
    fn a_float_is_read_directly_and_agrees_with_the_way_through_a_double() {
        let number = |text: &str| Json::Number(text.to_owned());
        assert_eq!(number("-0.225").float().unwrap(), -0.225_f32);
        assert_eq!(number("0.7142857142857143").double().unwrap(), 5.0 / 7.0);
        assert_eq!(number("-0.33333334").float().unwrap(), -1.0_f32 / 3.0);
        assert_eq!(number("1e40").double().unwrap(), 1e40);
        assert!(number("1e40").float().is_err(), "it does not fit a float");
        assert_eq!(number("12").integer().unwrap(), 12);
        assert!(number("12.0").integer().is_err());
        assert!(Json::String("1".to_owned()).float().is_err());
    }

    #[test]
    fn a_float_that_reads_differently_through_a_double_is_refused() {
        // Halfway between two floats lies 1 + 2^-24. A decimal a hair above it rounds
        // up when read directly; read as a double first it lands exactly on the half
        // and from there rounds to the even neighbour, down.
        let text = "1.00000005960464477550";
        let direct: f32 = text.parse().unwrap();
        let through = text.parse::<f64>().unwrap() as f32;
        assert_ne!(direct.to_bits(), through.to_bits(), "the example is one");
        let error = Json::Number(text.to_owned()).float().unwrap_err();
        assert!(format!("{error}").contains("through a double"));
    }
}
