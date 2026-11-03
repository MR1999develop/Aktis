//! A small JSON reader and writer: enough for the stored edits, with no
//! dependency. Numbers keep their text, so an `f32` is read back from the
//! digits it was written with (exactly the same value).

use std::fmt::Write as _;

/// Deepest nesting accepted when reading (the format needs about fifteen).
const MAX_DEPTH: usize = 128;

/// A JSON value. Objects keep their keys in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    /// The number as written, already checked against the JSON grammar.
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// An object from `(key, value)` pairs.
    pub(crate) fn object<const N: usize>(fields: [(&str, Value); N]) -> Self {
        Self::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    /// A text value.
    pub(crate) fn text(s: &str) -> Self {
        Self::String(s.to_owned())
    }

    /// An `f32`, written with the fewest digits that read back as the same
    /// value. Non-finite values (not valid JSON numbers) are written as the
    /// strings `"nan"`, `"inf"` and `"-inf"`.
    pub(crate) fn f32(v: f32) -> Self {
        if v.is_nan() {
            Self::text("nan")
        } else if v.is_infinite() {
            Self::text(if v > 0.0 { "inf" } else { "-inf" })
        } else {
            Self::Number(format!("{v}"))
        }
    }

    /// An unsigned integer.
    pub(crate) fn uint(v: impl Into<u64>) -> Self {
        Self::Number(v.into().to_string())
    }

    /// The value as compact JSON text.
    pub(crate) fn to_text(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out);
        out
    }

    /// The fields of an object.
    pub(crate) fn as_object(&self, what: &str) -> Result<&[(String, Value)], String> {
        match self {
            Self::Object(fields) => Ok(fields),
            _ => Err(format!("{what}: expected an object")),
        }
    }

    /// The items of an array.
    pub(crate) fn as_array(&self, what: &str) -> Result<&[Value], String> {
        match self {
            Self::Array(items) => Ok(items),
            _ => Err(format!("{what}: expected a list")),
        }
    }

    /// A string.
    pub(crate) fn as_str(&self, what: &str) -> Result<&str, String> {
        match self {
            Self::String(s) => Ok(s),
            _ => Err(format!("{what}: expected a text")),
        }
    }

    /// A boolean.
    pub(crate) fn as_bool(&self, what: &str) -> Result<bool, String> {
        match self {
            Self::Bool(b) => Ok(*b),
            _ => Err(format!("{what}: expected true or false")),
        }
    }

    /// An `f32` written by [`Self::f32`].
    pub(crate) fn as_f32(&self, what: &str) -> Result<f32, String> {
        match self {
            Self::Number(n) => n.parse::<f32>().map_err(|_| format!("{what}: not a number")),
            Self::String(s) if s == "nan" => Ok(f32::NAN),
            Self::String(s) if s == "inf" => Ok(f32::INFINITY),
            Self::String(s) if s == "-inf" => Ok(f32::NEG_INFINITY),
            _ => Err(format!("{what}: expected a number")),
        }
    }

    /// A whole number that fits a `u64`.
    pub(crate) fn as_u64(&self, what: &str) -> Result<u64, String> {
        match self {
            Self::Number(n) => n.parse::<u64>().map_err(|_| format!("{what}: expected a whole number")),
            _ => Err(format!("{what}: expected a whole number")),
        }
    }

    /// A whole number that fits a `u32`.
    pub(crate) fn as_u32(&self, what: &str) -> Result<u32, String> {
        u32::try_from(self.as_u64(what)?).map_err(|_| format!("{what}: number too large"))
    }
}

/// The value of `key` in an object's fields.
pub(crate) fn field<'a>(fields: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// The value of `key`, which must be there.
pub(crate) fn required<'a>(fields: &'a [(String, Value)], key: &str) -> Result<&'a Value, String> {
    field(fields, key).ok_or_else(|| format!("'{key}' is missing"))
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(fields) => {
            out.push('{');
            for (i, (k, item)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Reads one JSON value; nothing but white space may follow it.
///
/// # Errors
/// A message saying what is wrong and at which byte.
pub(crate) fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { bytes: text.as_bytes(), pos: 0 };
    let value = p.value(0)?;
    p.skip_space();
    if p.pos != p.bytes.len() {
        return Err(p.error("unexpected text after the end"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        format!("{what} at byte {}", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        self.skip_space();
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected '{}'", char::from(b))))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error("unknown word"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        self.skip_space();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected character")),
            None => Err(self.error("unexpected end")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1; // '{'
        let mut fields: Vec<(String, Value)> = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return Err(self.error("expected a key"));
            }
            let key = self.string()?;
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(self.error(&format!("key '{key}' appears twice")));
            }
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            fields.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("bad number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(self.error("bad number"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("bad number"));
            }
        }
        // Only ASCII digits and signs were accepted.
        let text = String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned();
        Ok(Value::Number(text))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.bytes.get(self.pos..self.pos + 4).ok_or_else(|| self.error("short \\u escape"))?;
        let text = std::str::from_utf8(digits).map_err(|_| self.error("bad \\u escape"))?;
        let code = u32::from_str_radix(text, 16).map_err(|_| self.error("bad \\u escape"))?;
        self.pos += 4;
        Ok(code)
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy the run of plain characters at once (valid UTF-8: the input is a str).
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            let run = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| self.error("bad text"))?;
            out.push_str(run);
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = self.peek().ok_or_else(|| self.error("unexpected end"))?;
                    self.pos += 1;
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
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                // A high surrogate must be followed by a low one.
                                if self.bytes.get(self.pos..self.pos + 2) != Some(b"\\u".as_slice()) {
                                    return Err(self.error("lone surrogate"));
                                }
                                self.pos += 2;
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return Err(self.error("lone surrogate"));
                                }
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            }
                            out.push(char::from_u32(code).ok_or_else(|| self.error("bad \\u escape"))?);
                        }
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                Some(_) => return Err(self.error("control character in a text")),
                None => return Err(self.error("unterminated text")),
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_through_text() {
        let v = Value::object([
            ("a", Value::Array(vec![Value::Null, Value::Bool(true), Value::Bool(false), Value::uint(7_u32)])),
            ("text", Value::text("quote \" slash \\ tab \t line \n bell \u{7} é ✓ 🙂")),
            ("empty", Value::Object(vec![])),
            ("list", Value::Array(vec![])),
            ("f", Value::f32(-0.1)),
        ]);
        let text = v.to_text();
        assert_eq!(parse(&text).unwrap(), v);
    }

    #[test]
    fn floats_read_back_exactly() {
        for v in [0.1_f32, -0.0, 1.0 / 3.0, f32::MAX, f32::MIN_POSITIVE, 1e-40, 123_456.79, -5.0] {
            let back = parse(&Value::f32(v).to_text()).unwrap().as_f32("v").unwrap();
            assert_eq!(back.to_bits(), v.to_bits(), "{v}");
        }
        for v in [f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(parse(&Value::f32(v).to_text()).unwrap().as_f32("v").unwrap(), v);
        }
        assert!(parse(&Value::f32(f32::NAN).to_text()).unwrap().as_f32("v").unwrap().is_nan());
    }

    #[test]
    fn standard_json_from_elsewhere_is_read() {
        let v = parse(" { \"k\" : [ 1 , -2.5e3 , \"\\u00e9\\ud83d\\ude42\\/\" ] } ").unwrap();
        let fields = v.as_object("v").unwrap();
        let list = required(fields, "k").unwrap().as_array("k").unwrap();
        assert_eq!(list[0].as_u32("n").unwrap(), 1);
        assert_eq!(list[1].as_f32("n").unwrap(), -2500.0);
        assert_eq!(list[2].as_str("s").unwrap(), "é🙂/");
    }

    #[test]
    fn broken_json_is_refused() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\":1 \"b\":2}",
            "01",
            "1.",
            "-",
            "1e",
            "tru",
            "\"open",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"a\u{1}\"",
            "{\"a\":1,\"a\":2}",
            "[] []",
            "{1:2}",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deep_nesting_is_refused_without_overflowing_the_stack() {
        let deep = "[".repeat(100_000);
        assert!(parse(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn numbers_are_checked_by_kind() {
        let v = parse("[300, -1, 1.5, 4294967296]").unwrap();
        let items = v.as_array("v").unwrap();
        assert_eq!(items[0].as_u32("n").unwrap(), 300);
        assert!(items[1].as_u32("n").is_err());
        assert!(items[2].as_u32("n").is_err());
        assert!(items[3].as_u32("n").is_err());
        assert!(Value::text("x").as_f32("n").is_err());
    }
}
