//! A small XML reader, enough for the lensfun database.
//!
//! It accepts well-formed documents with elements, attributes, text, comments,
//! CDATA, processing instructions and a DOCTYPE (which is skipped, not
//! interpreted). The five predefined entities and numeric character
//! references are decoded. Anything else is reported with a line number
//! instead of being guessed at, because user-supplied files go through here.

use std::fmt;

/// Deepest nesting accepted; lensfun files use four levels.
const MAX_DEPTH: usize = 64;

/// An element with its attributes, child elements and direct text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// Text directly inside this element (not inside children), concatenated.
    pub text: String,
}

impl Element {
    /// The value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Child elements called `name`, in document order.
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The first child element called `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlError {
    /// 1-based line of the problem.
    pub line: usize,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for XmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for XmlError {}

/// Parses a whole document and returns its root element.
pub(crate) fn parse(input: &str) -> Result<Element, XmlError> {
    let mut p = Parser { s: input, pos: 0 };
    p.skip_bom();
    p.skip_misc()?;
    if p.peek() != Some('<') {
        return Err(p.error("expected the root element"));
    }
    let root = p.element(0)?;
    p.skip_misc()?;
    if p.pos < p.s.len() {
        return Err(p.error("unexpected content after the root element"));
    }
    Ok(root)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.s[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn error(&self, message: impl Into<String>) -> XmlError {
        let line = self.s[..self.pos].matches('\n').count() + 1;
        XmlError { line, message: message.into() }
    }

    fn skip_bom(&mut self) {
        if self.rest().starts_with('\u{feff}') {
            self.pos += '\u{feff}'.len_utf8();
        }
    }

    fn skip_whitespace(&mut self) {
        let trimmed = self.rest().trim_start_matches(is_xml_space);
        self.pos = self.s.len() - trimmed.len();
    }

    /// Moves past `end`, which must occur later in the input.
    fn skip_past(&mut self, end: &str, what: &str) -> Result<(), XmlError> {
        match self.rest().find(end) {
            Some(i) => {
                self.pos += i + end.len();
                Ok(())
            }
            None => Err(self.error(format!("unterminated {what}"))),
        }
    }

    /// Skips whitespace, comments, processing instructions and DOCTYPE.
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_whitespace();
            if self.starts_with("<!--") {
                self.skip_past("-->", "comment")?;
            } else if self.starts_with("<?") {
                self.skip_past("?>", "processing instruction")?;
            } else if self.starts_with("<!DOCTYPE") {
                self.skip_doctype()?;
            } else {
                return Ok(());
            }
        }
    }

    /// Skips `<!DOCTYPE …>`, including an internal subset in brackets.
    fn skip_doctype(&mut self) -> Result<(), XmlError> {
        let mut depth = 0_usize;
        let mut quote: Option<char> = None;
        for (i, c) in self.rest().char_indices() {
            match (quote, c) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, '"' | '\'') => quote = Some(c),
                (None, '[') => depth += 1,
                (None, ']') => depth = depth.saturating_sub(1),
                (None, '>') if depth == 0 => {
                    self.pos += i + 1;
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(self.error("unterminated DOCTYPE"))
    }

    fn name(&mut self) -> Result<String, XmlError> {
        let end = self
            .rest()
            .find(|c: char| is_xml_space(c) || matches!(c, '>' | '/' | '=' | '<'))
            .unwrap_or(self.rest().len());
        if end == 0 {
            return Err(self.error("expected a name"));
        }
        let name = self.rest()[..end].to_owned();
        self.pos += end;
        Ok(name)
    }

    /// Parses an element starting at `<`.
    fn element(&mut self, depth: usize) -> Result<Element, XmlError> {
        if depth > MAX_DEPTH {
            return Err(self.error("elements nested too deeply"));
        }
        self.pos += 1; // '<'
        let name = self.name()?;
        let mut el = Element { name, attrs: Vec::new(), children: Vec::new(), text: String::new() };

        // Attributes, up to '>' or '/>'.
        loop {
            self.skip_whitespace();
            if self.starts_with("/>") {
                self.pos += 2;
                return Ok(el);
            }
            if self.starts_with(">") {
                self.pos += 1;
                break;
            }
            let key = self.name()?;
            self.skip_whitespace();
            if !self.starts_with("=") {
                return Err(self.error(format!("attribute '{key}' has no value")));
            }
            self.pos += 1;
            self.skip_whitespace();
            let quote = match self.peek() {
                Some(q @ ('"' | '\'')) => q,
                _ => return Err(self.error(format!("value of attribute '{key}' is not quoted"))),
            };
            self.pos += 1;
            let end = self.rest().find(quote).ok_or_else(|| self.error("unterminated attribute value"))?;
            let raw = &self.rest()[..end];
            if raw.contains('<') {
                return Err(self.error(format!("'<' in the value of attribute '{key}'")));
            }
            let value = decode_entities(raw).map_err(|m| self.error(m))?;
            self.pos += end + 1;
            if el.attrs.iter().any(|(k, _)| *k == key) {
                return Err(self.error(format!("attribute '{key}' appears twice")));
            }
            el.attrs.push((key, value));
        }

        // Content, up to the matching end tag.
        loop {
            let next =
                self.rest().find('<').ok_or_else(|| self.error(format!("element <{}> is not closed", el.name)))?;
            if next > 0 {
                let raw = &self.rest()[..next];
                el.text.push_str(&decode_entities(raw).map_err(|m| self.error(m))?);
                self.pos += next;
            }
            if self.starts_with("</") {
                self.pos += 2;
                let closing = self.name()?;
                if closing != el.name {
                    return Err(self.error(format!("<{}> is closed by </{closing}>", el.name)));
                }
                self.skip_whitespace();
                if !self.starts_with(">") {
                    return Err(self.error(format!("malformed end tag </{closing}>")));
                }
                self.pos += 1;
                return Ok(el);
            } else if self.starts_with("<!--") {
                self.skip_past("-->", "comment")?;
            } else if self.starts_with("<![CDATA[") {
                self.pos += "<![CDATA[".len();
                let end = self.rest().find("]]>").ok_or_else(|| self.error("unterminated CDATA section"))?;
                el.text.push_str(&self.rest()[..end]);
                self.pos += end + 3;
            } else if self.starts_with("<?") {
                self.skip_past("?>", "processing instruction")?;
            } else if self.starts_with("<!") {
                return Err(self.error("unexpected declaration inside an element"));
            } else {
                let child = self.element(depth + 1)?;
                el.children.push(child);
            }
        }
    }
}

fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// Replaces `&amp;` and friends and `&#…;` references.
fn decode_entities(raw: &str) -> Result<String, String> {
    if !raw.contains('&') {
        return Ok(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').ok_or_else(|| "'&' without ';'".to_owned())?;
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = entity.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    None
                };
                code.and_then(char::from_u32).ok_or_else(|| format!("unknown entity '&{entity};'"))?
            }
        };
        out.push(c);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_elements_attributes_text_and_skips_the_rest() {
        let doc = r#"<?xml version="1.0"?>
<!DOCTYPE db SYSTEM "x.dtd" [ <!ELEMENT a (b)> ]>
<!-- top -->
<db version="2">
    <lens>
        <maker>Fish &amp; Chips</maker>
        <!-- a comment -->
        <model lang='en'>Lens &#x41;&#66;</model>
        <calib focal="24" a="-0.1"/>
        <note><![CDATA[a < b]]></note>
    </lens>
</db>
"#;
        let root = parse(doc).expect("parses");
        assert_eq!(root.name, "db");
        assert_eq!(root.attr("version"), Some("2"));
        let lens = root.child("lens").expect("lens");
        assert_eq!(lens.child("maker").map(|m| m.text.as_str()), Some("Fish & Chips"));
        let model = lens.child("model").expect("model");
        assert_eq!((model.text.as_str(), model.attr("lang")), ("Lens AB", Some("en")));
        let calib = lens.child("calib").expect("calib");
        assert_eq!((calib.attr("focal"), calib.attr("a")), (Some("24"), Some("-0.1")));
        assert!(calib.children.is_empty());
        assert_eq!(lens.child("note").map(|n| n.text.as_str()), Some("a < b"));
        assert_eq!(lens.children_named("maker").count(), 1);
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        let e = parse("<a>\n<b>\n</a>").expect_err("mismatched");
        assert_eq!(e.line, 3);
        assert!(e.message.contains("closed by"), "{e}");
        assert!(parse("<a x=1/>").is_err());
        assert!(parse("<a x='1' x='2'/>").is_err());
        assert!(parse("<a>&bogus;</a>").is_err());
        assert!(parse("<a></a><b/>").is_err());
        assert!(parse("<a>").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn deep_nesting_is_refused_instead_of_overflowing_the_stack() {
        let doc = "<a>".repeat(100) + &"</a>".repeat(100);
        let e = parse(&doc).expect_err("too deep");
        assert!(e.message.contains("deeply"), "{e}");
    }
}
