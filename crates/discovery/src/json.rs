//! Minimal strict JSON parser (RFC 8259), standard library only.
//!
//! Why this exists: package installs were unavailable when the GitHub
//! adapter was written, so `serde_json` could not be added. Replace this
//! module with `serde_json` once dependencies can be fetched.
//!
//! The parser is deliberately strict (no comments, no trailing commas, no
//! leading zeros, no lone surrogates) and bounded (nesting depth), because
//! it reads data from a network peer.

use std::collections::BTreeMap;

/// Maximum nesting of arrays and objects.
pub(crate) const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// The number exactly as written. It is validated but not converted.
    Number(String),
    String(String),
    Array(Vec<Json>),
    /// For duplicate keys the last one wins.
    Object(BTreeMap<String, Json>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError {
    pub(crate) offset: usize,
    pub(crate) message: &'static str,
}

impl Json {
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub(crate) fn is_object(&self) -> bool {
        matches!(self, Json::Object(_))
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub(crate) fn as_number_str(&self) -> Option<&str> {
        match self {
            Json::Number(n) => Some(n),
            _ => None,
        }
    }
}

pub(crate) fn parse(input: &str) -> Result<Json, ParseError> {
    let mut parser = Parser {
        src: input,
        bytes: input.as_bytes(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.value(0)?;
    parser.skip_ws();
    if parser.pos != parser.bytes.len() {
        return Err(parser.err("unexpected trailing characters"));
    }
    Ok(value)
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, message: &'static str) -> ParseError {
        ParseError {
            offset: self.pos,
            message,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8, message: &'static str) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(message))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn literal(&mut self, word: &'static [u8], value: Json) -> Result<Json, ParseError> {
        if self.bytes[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.pos += 1; // '{'
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key"));
            }
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':', "expected ':' after key")?;
            self.skip_ws();
            let value = self.value(depth + 1)?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(map));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        // Start of the current run of plain characters. Runs end only at
        // ASCII bytes, so slicing the source there is always on a char boundary.
        let mut run = self.pos;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.err("unterminated string"));
            };
            match byte {
                b'"' => {
                    out.push_str(&self.src[run..self.pos]);
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.src[run..self.pos]);
                    self.pos += 1;
                    let ch = self.escape()?;
                    out.push(ch);
                    run = self.pos;
                }
                0x00..=0x1f => return Err(self.err("control character in string")),
                _ => self.pos += 1,
            }
        }
    }

    fn escape(&mut self) -> Result<char, ParseError> {
        let Some(byte) = self.peek() else {
            return Err(self.err("unterminated escape"));
        };
        self.pos += 1;
        Ok(match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(),
            _ => {
                return Err(ParseError {
                    offset: self.pos - 1,
                    message: "invalid escape",
                })
            }
        })
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let end = self.pos + 4;
        let Some(digits) = self.bytes.get(self.pos..end) else {
            return Err(self.err("truncated unicode escape"));
        };
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.err("invalid unicode escape"));
        }
        let value = digits.iter().fold(0u32, |acc, d| {
            acc * 16 + char::from(*d).to_digit(16).unwrap_or(0)
        });
        self.pos = end;
        Ok(value)
    }

    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let first = self.hex4()?;
        if (0xD800..0xDC00).contains(&first) {
            // High surrogate: it must be followed by \uDC00..\uDFFF.
            if self.bytes.get(self.pos) != Some(&b'\\')
                || self.bytes.get(self.pos + 1) != Some(&b'u')
            {
                return Err(self.err("unpaired surrogate"));
            }
            self.pos += 2;
            let second = self.hex4()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(self.err("invalid surrogate pair"));
            }
            let code = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
            char::from_u32(code).ok_or_else(|| self.err("invalid code point"))
        } else if (0xDC00..0xE000).contains(&first) {
            Err(self.err("unpaired surrogate"))
        } else {
            char::from_u32(first).ok_or_else(|| self.err("invalid code point"))
        }
    }

    fn digits(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.digits() {
                return Err(self.err("digits required after decimal point"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.digits() {
                return Err(self.err("digits required in exponent"));
            }
        }
        Ok(Json::Number(self.src[start..self.pos].to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Json {
        parse(s).unwrap_or_else(|e| panic!("{s:?} should parse, got {} at {}", e.message, e.offset))
    }

    fn err(s: &str) -> ParseError {
        parse(s).expect_err("input should be rejected")
    }

    fn nested(levels: usize) -> String {
        format!("{}{}", "[".repeat(levels), "]".repeat(levels))
    }

    #[test]
    fn parses_scalars() {
        assert_eq!(ok("null"), Json::Null);
        assert_eq!(ok(" true "), Json::Bool(true));
        assert_eq!(ok("false"), Json::Bool(false));
        assert_eq!(ok("0"), Json::Number("0".into()));
        assert_eq!(ok("-12.5e+3"), Json::Number("-12.5e+3".into()));
        assert_eq!(ok("\"hi\""), Json::String("hi".into()));
    }

    #[test]
    fn parses_containers() {
        let v = ok(r#"{"a":[1,{"b":null}],"c":"d"}"#);
        assert_eq!(v.get("c").and_then(Json::as_str), Some("d"));
        let a = v.get("a").and_then(Json::as_array).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].as_number_str(), Some("1"));
        assert_eq!(a[1].get("b"), Some(&Json::Null));
        for empty in ["[]", "{}", "[ ]", "{ }"] {
            ok(empty);
        }
    }

    #[test]
    fn accessors_return_none_for_the_wrong_type() {
        let v = ok(r#"{"a":1}"#);
        assert!(v.is_object());
        assert_eq!(v.as_str(), None);
        assert_eq!(v.as_bool(), None);
        assert_eq!(v.as_array(), None);
        assert_eq!(v.as_number_str(), None);
        assert_eq!(ok("1").get("a"), None);
        assert!(!ok("[]").is_object());
    }

    #[test]
    fn decodes_escapes_and_unicode() {
        assert_eq!(
            ok(r#""a\"b\\c\/d\n\tA""#),
            Json::String("a\"b\\c/d\n\tA".into())
        );
        assert_eq!(ok("\"héllo ✓\""), Json::String("héllo ✓".into()));
        assert_eq!(ok(r#""😀""#), Json::String("😀".into()));
    }

    #[test]
    fn last_duplicate_key_wins() {
        let v = ok(r#"{"a":1,"a":2}"#);
        assert_eq!(v.get("a").and_then(Json::as_number_str), Some("2"));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "   ",
            "[1,]",
            "[,1]",
            r#"{"a":1,}"#,
            "{a:1}",
            r#"{"a" 1}"#,
            "01",
            "1.",
            "1e",
            "-",
            "+1",
            "'a'",
            "nul",
            "truee",
            "[] x",
            "\"abc",
            "\"a\nb\"",
            r#""\x""#,
            r#""\ud800""#,
            r#""\udc00""#,
            r#""\ud800A""#,
            r#""\u12""#,
            r#""\u12G4""#,
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn reports_the_offset_of_the_problem() {
        assert_eq!(err("[1,]").offset, 3);
        assert_eq!(err("[1,]").message, "unexpected character");
    }

    #[test]
    fn nesting_is_bounded() {
        ok(&nested(MAX_DEPTH));
        let e = err(&nested(MAX_DEPTH + 1));
        assert_eq!(e.message, "nesting too deep");
    }
}
