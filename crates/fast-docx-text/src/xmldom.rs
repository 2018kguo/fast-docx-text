//! The XML mammoth reads: @xmldom/xmldom 0.8.15's SAX parser as mammoth 1.13.0 drives it
//! (`office-xml-reader.js`, `xml/reader.js`), for the inputs it accepts.
//!
//! What it keeps: elements named by namespace URI the way mammoth renames them (`w:p`,
//! `{uri}local`, or the bare local name when unbound), and text, entity-decoded, as one
//! node per run between markup. Comments, CDATA and processing instructions are dropped,
//! but still split the text around them (mammoth's `text()` then refuses the element).
//! End tags pop only when they name the open element, as xmldom does.
//!
//! What it refuses: everything xmldom reports to its error handler, since mammoth turns
//! any report (warnings included) into a thrown error. Malformed tags, unknown entities,
//! an element with no end tag anywhere after it, a duplicate attribute.

use crate::tree::{Builder, FxMap, Tree};

/// mammoth reads a document until its recursive reader overflows the JS stack,
/// somewhere past 1,800 levels; this reads every such document and fails cleanly
/// past 4,096, well inside the 8 MiB wasm stack (scripts/build-wasm.sh), where
/// the reader's recursion would otherwise trap near 6,000.
pub const MAX_DEPTH: usize = 4096;

fn namespace_prefix(uri: &str) -> Option<&'static str> {
    Some(match uri {
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
        | "http://purl.oclc.org/ooxml/wordprocessingml/main" => "w",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
        | "http://purl.oclc.org/ooxml/officeDocument/relationships" => "r",
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
        | "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing" => "wp",
        "http://schemas.openxmlformats.org/drawingml/2006/main"
        | "http://purl.oclc.org/ooxml/drawingml/main" => "a",
        "http://schemas.openxmlformats.org/drawingml/2006/picture"
        | "http://purl.oclc.org/ooxml/drawingml/picture" => "pic",
        "http://schemas.openxmlformats.org/package/2006/content-types" => "content-types",
        "http://schemas.openxmlformats.org/package/2006/relationships" => "relationships",
        "http://schemas.openxmlformats.org/markup-compatibility/2006" => "mc",
        "urn:schemas-microsoft-com:vml" => "v",
        "urn:schemas-microsoft-com:office:word" => "office-word",
        "http://schemas.microsoft.com/office/word/2010/wordml" => "wordml",
        _ => return None,
    })
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// xmldom's `nameStartChar` (BMP only; astral characters are two code units, neither of
/// which matches).
fn is_name_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// xmldom's `tagNamePattern`: a name, optionally `prefix:local`.
fn valid_name(name: &str) -> bool {
    let valid_part = |part: &str| {
        let mut chars = part.chars();
        chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
    };
    match name.split_once(':') {
        Some((prefix, local)) => valid_part(prefix) && valid_part(local),
        None => valid_part(name),
    }
}

/// `String.fromCharCode` of xmldom's `fixedFromCharCode(parseInt(...))`.
fn char_reference(digits: &str, out: &mut String) {
    // parseInt(k.substr(1).replace('x', '0x')): a leading "0x" reads hex, otherwise
    // decimal; either stops at the first character that is not a digit.
    let replaced = digits.replacen('x', "0x", 1);
    let (radix, body) = match replaced.strip_prefix("0x").or(replaced.strip_prefix("0X")) {
        Some(rest) => (16, rest),
        None => (10, replaced.as_str()),
    };
    let valid: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    let code = if valid.is_empty() {
        0.0 // NaN, which fromCharCode reads as 0
    } else {
        valid
            .chars()
            .fold(0f64, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap()))
    };
    let units: Vec<u16> = if code > 65535.0 {
        // (code - 0x10000) >> 10 and & 0x3ff, as int32.
        let shifted = js_to_int32(code - 65536.0);
        vec![
            (0xD800i64 + i64::from(shifted >> 10)) as u16,
            (0xDC00i64 + i64::from(shifted & 0x3FF)) as u16,
        ]
    } else {
        vec![code as u16]
    };
    out.extend(char::decode_utf16(units).map(|unit| unit.unwrap_or('\u{FFFD}')));
}

fn js_to_int32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc() % 4_294_967_296.0;
    let wrapped = if truncated < 0.0 {
        truncated + 4_294_967_296.0
    } else {
        truncated
    };
    wrapped as u32 as i32
}

/// `.replace(/&#?\w+;/g, entityReplacer)`, or an error for an unknown entity.
fn decode_entities(raw: &str, out: &mut String) -> Result<(), String> {
    let bytes = raw.as_bytes();
    let mut from = 0;
    let mut at = 0;
    while let Some(offset) = memchr::memchr(b'&', &bytes[at..]) {
        let amp = at + offset;
        let mut cursor = amp + 1;
        if bytes.get(cursor) == Some(&b'#') {
            cursor += 1;
        }
        let word_start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor == word_start || bytes.get(cursor) != Some(&b';') {
            at = amp + 1;
            continue;
        }
        out.push_str(&raw[from..amp]);
        let key = &raw[amp + 1..cursor];
        match key {
            "amp" => out.push('&'),
            "apos" => out.push('\''),
            "gt" => out.push('>'),
            "lt" => out.push('<'),
            "quot" => out.push('"'),
            _ if key.starts_with('#') => char_reference(&key[1..], out),
            _ => return Err(format!("entity not found:&{key};")),
        }
        from = cursor + 1;
        at = from;
    }
    out.push_str(&raw[from..]);
    Ok(())
}

fn is_space(c: u8) -> bool {
    c <= b' '
}

/// The source mammoth parses: TextDecoder's UTF-8 (one BOM dropped), mammoth's own BOM
/// strip, then xmldom's line-ending normalization.
pub fn decode(bytes: Vec<u8>) -> String {
    let mut text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    };
    for _ in 0..2 {
        if text.starts_with('\u{FEFF}') {
            text.drain(..'\u{FEFF}'.len_utf8());
        }
    }
    // \r, U+0085 (C2 85) and U+2028 (E2 80 A8) become \n, \r\n and \r\u{85} one \n.
    // Their lead bytes, found with memchr; everything between is copied as is.
    let bytes = text.as_bytes();
    let mut candidates = memchr::memchr3_iter(b'\r', 0xC2, 0xE2, bytes);
    let Some(first) = candidates.find(|&at| line_break(bytes, at).is_some()) else {
        return text;
    };
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..first]);
    let mut at = first;
    while at < bytes.len() {
        match line_break(bytes, at) {
            Some(len) => {
                out.push(b'\n');
                at += len;
            }
            None => {
                let next = memchr::memchr3(b'\r', 0xC2, 0xE2, &bytes[at + 1..])
                    .map_or(bytes.len(), |offset| at + 1 + offset);
                out.extend_from_slice(&bytes[at..next]);
                at = next;
            }
        }
    }
    // SAFETY: whole UTF-8 sequences were replaced by "\n"; the rest copied verbatim.
    unsafe { String::from_utf8_unchecked(out) }
}

/// `lastIndexOf('</name>')` for every name, from one backward scan shared by all of
/// them: each lookup resumes the scan where the last one stopped, remembering every
/// end tag it passes, so the whole source is read at most once.
struct EndTags<'a> {
    bytes: &'a [u8],
    /// Every `</` at or past this offset has been seen.
    scanned_to: usize,
    last: FxMap<&'a [u8], usize>,
}

impl<'a> EndTags<'a> {
    fn last(&mut self, name: &[u8]) -> Option<usize> {
        if let Some(&at) = self.last.get(name) {
            return Some(at);
        }
        while let Some(at) = memchr::memmem::rfind(&self.bytes[..self.scanned_to], b"</") {
            self.scanned_to = at;
            // A name never contains `<`, so the look for its `>` stops at the next `<`
            // and the scans together stay linear.
            let rest = &self.bytes[at + 2..];
            if let Some(len) = memchr::memchr2(b'>', b'<', rest) {
                if rest[len] == b'>' {
                    let found = &rest[..len];
                    self.last.entry(found).or_insert(at);
                    if found == name {
                        return Some(at);
                    }
                }
            }
        }
        None
    }
}

/// The byte length of the line break starting at `at`, if one does.
fn line_break(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes[at..] {
        [b'\r', b'\n', ..] => Some(2),
        [b'\r', 0xC2, 0x85, ..] => Some(3),
        [b'\r', ..] => Some(1),
        [0xC2, 0x85, ..] => Some(2),
        [0xE2, 0x80, 0xA8, ..] => Some(3),
        _ => None,
    }
}

struct Scope {
    /// (prefix, uri), innermost last.
    bindings: Vec<(String, String)>,
    /// Bumped whenever the bindings change, so cached names know they are stale.
    generation: u32,
}

impl Scope {
    fn get(&self, prefix: &str) -> Option<&str> {
        self.bindings
            .iter()
            .rev()
            .find(|(bound, _)| bound == prefix)
            .map(|(_, uri)| uri.as_str())
    }

    fn truncate(&mut self, len: usize) {
        if self.bindings.len() != len {
            self.bindings.truncate(len);
            self.generation += 1;
        }
    }
}

fn converted_name(uri: Option<&str>, local: &str) -> String {
    match uri.filter(|uri| !uri.is_empty()) {
        Some(uri) => match namespace_prefix(uri) {
            Some(prefix) => format!("{prefix}:{local}"),
            None => format!("{{{uri}}}{local}"),
        },
        None => local.to_owned(),
    }
}

fn split_qname(name: &str) -> (Option<&str>, &str) {
    match name.find(':') {
        Some(colon) if colon > 0 => (Some(&name[..colon]), &name[colon + 1..]),
        _ => (None, name),
    }
}

/// What is known about one raw element name: that it is valid (it is in the map at
/// all), xmldom's cached end-tag position for it, and its converted name's id in the
/// scope generation it was converted in.
struct ElementName {
    close: Option<i64>,
    id: u32,
    generation: u32,
}

/// Parses `source`, interning `known[i]` as name id `i`.
pub fn parse(source: String, known: &[&str]) -> Result<Tree, String> {
    let bytes = source.as_bytes();
    let mut builder = Builder::new(known);
    let mut scope = Scope {
        bindings: vec![("xml".into(), XML_NS.into())],
        generation: 0,
    };
    // Per open element: (raw tag name, scope length before it).
    let mut open: Vec<(&str, usize)> = Vec::new();
    let mut element_names: FxMap<&str, ElementName> = FxMap::default();
    let mut end_tags = EndTags {
        bytes,
        scanned_to: bytes.len(),
        last: FxMap::default(),
    };
    // Raw attribute name → (converted id, scope generation).
    let mut attribute_names: FxMap<&str, (u32, u32)> = FxMap::default();
    let mut start = 0;
    let mut scratch = String::new();
    let mut raw_attrs: Vec<(&str, usize, usize)> = Vec::new();

    loop {
        let Some(offset) = memchr::memchr(b'<', &bytes[start..]) else {
            break;
        };
        let tag_start = start + offset;
        append_text(&mut builder, &source, start, tag_start, &mut scratch)?;
        match bytes.get(tag_start + 1) {
            Some(b'/') => {
                let Some(end) = memchr::memchr(b'>', bytes.get(tag_start + 3..).unwrap_or(&[]))
                    .map(|offset| tag_start + 3 + offset)
                else {
                    return Err("end tag name is not complete".into());
                };
                let raw = &source[tag_start + 2..end];
                let name = raw.trim_end_matches([' ', '\t', '\n', '\r']);
                if name.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
                    || (!name.is_ascii() && name.contains(char::is_whitespace))
                {
                    return Err(format!("end tag name: {name} is malformed"));
                }
                if let Some(&(open_name, scope_len)) = open.last() {
                    if open_name == name {
                        scope.truncate(scope_len);
                        open.pop();
                        builder.close();
                    } else if open_name.to_lowercase() == name.to_lowercase() {
                        return Err(format!(
                            "end tag name: {name} is not match the current start tagName:{open_name}"
                        ));
                    }
                }
                start = end + 1;
            }
            Some(b'?') => {
                let Some(end) = memchr::memmem::find(&bytes[tag_start..], b"?>") else {
                    return Err("unterminated processing instruction".into());
                };
                start = tag_start + end + 2;
            }
            Some(b'!') => {
                if bytes[tag_start..].starts_with(b"<!--") {
                    let Some(end) = memchr::memmem::find(&bytes[tag_start + 4..], b"-->") else {
                        return Err("Unclosed comment".into());
                    };
                    start = tag_start + 4 + end + 3;
                } else if bytes[tag_start..].starts_with(b"<![CDATA[") {
                    let Some(end) = memchr::memmem::find(&bytes[tag_start + 9..], b"]]>") else {
                        return Err("unterminated CDATA section".into());
                    };
                    start = tag_start + 9 + end + 3;
                } else {
                    let declaration = if bytes.get(tag_start + 2) == Some(&b'-') {
                        None
                    } else {
                        doctype_end(bytes, tag_start)?
                    };
                    match declaration {
                        Some(end) => start = end,
                        None => {
                            // Not a DOCTYPE: xmldom reads the `<` as text and goes on.
                            if builder.depth() > 0 {
                                builder.text_range((tag_start as u32, tag_start as u32 + 1));
                            }
                            start = tag_start + 1;
                        }
                    }
                }
            }
            _ => {
                // The start tag: name, then `name="value"` pairs, then `>` or `/>`.
                let mut p = tag_start + 1;
                while p < bytes.len() && !is_space(bytes[p]) && bytes[p] != b'/' && bytes[p] != b'>'
                {
                    if bytes[p] == b'<' {
                        return Err("unexpected < in tag name".into());
                    }
                    p += 1;
                }
                let tag_name = &source[tag_start + 1..p];
                raw_attrs.clear();
                let closed;
                loop {
                    let separated = p < bytes.len() && is_space(bytes[p]);
                    while p < bytes.len() && is_space(bytes[p]) {
                        p += 1;
                    }
                    match bytes.get(p) {
                        None => return Err("unexpected end of input".into()),
                        Some(b'>') => {
                            closed = false;
                            break;
                        }
                        Some(b'/') => {
                            p += 1;
                            while p < bytes.len() && is_space(bytes[p]) {
                                p += 1;
                            }
                            if bytes.get(p) != Some(&b'>') {
                                return Err(
                                    "elements closed character '/' and '>' must be connected to"
                                        .into(),
                                );
                            }
                            closed = true;
                            break;
                        }
                        Some(_) if !separated => {
                            return Err("attribute space is required".into());
                        }
                        Some(_) => {}
                    }
                    let name_start = p;
                    while p < bytes.len()
                        && !is_space(bytes[p])
                        && !matches!(bytes[p], b'=' | b'>' | b'/' | b'"' | b'\'')
                    {
                        p += 1;
                    }
                    let attr_name = &source[name_start..p];
                    while p < bytes.len() && is_space(bytes[p]) {
                        p += 1;
                    }
                    if bytes.get(p) != Some(&b'=') {
                        return Err(format!("attribute \"{attr_name}\" missed value"));
                    }
                    p += 1;
                    while p < bytes.len() && is_space(bytes[p]) {
                        p += 1;
                    }
                    let quote = match bytes.get(p) {
                        Some(&q @ (b'"' | b'\'')) => q,
                        _ => return Err(format!("attribute \"{attr_name}\" missed quot")),
                    };
                    let Some(close) = memchr::memchr(quote, &bytes[p + 1..]) else {
                        return Err("attribute value no end quote match".into());
                    };
                    if raw_attrs.iter().any(|&(name, _, _)| name == attr_name) {
                        return Err(format!("Attribute {attr_name} redefined"));
                    }
                    raw_attrs.push((attr_name, p + 1, p + 1 + close));
                    p = p + 1 + close + 1;
                    if !matches!(bytes.get(p), Some(&c) if is_space(c) || c == b'>' || c == b'/') {
                        return Err("attribute space is required".into());
                    }
                }
                let tag_end = p;

                // Namespace declarations first (the first of a repeated prefix wins).
                let scope_len = scope.bindings.len();
                for &(qname, value_start, value_end) in raw_attrs.iter().rev() {
                    let prefix = match split_qname(qname) {
                        (Some("xmlns"), local) => Some(local),
                        (None, "xmlns") => Some(""),
                        _ => None,
                    };
                    if let Some(prefix) = prefix {
                        scratch.clear();
                        decode_attribute(&source[value_start..value_end], &mut scratch)?;
                        scope.bindings.push((prefix.to_owned(), scratch.clone()));
                    }
                }
                if scope.bindings.len() != scope_len {
                    scope.generation += 1;
                }

                let entry = match element_names.get_mut(tag_name) {
                    Some(entry) => entry,
                    None => {
                        if !valid_name(tag_name) {
                            return Err(format!("invalid tagName:{tag_name}"));
                        }
                        element_names.entry(tag_name).or_insert(ElementName {
                            close: None,
                            id: 0,
                            generation: u32::MAX,
                        })
                    }
                };
                if !closed {
                    // fixSelfClosed: xmldom caches, at a name's first open element,
                    // `lastIndexOf('</name>')`, or `lastIndexOf('</name')` when that is
                    // before the element's end; an element whose end is past it has no
                    // end tag after it.
                    let position = *entry.close.get_or_insert_with(|| {
                        let full = end_tags.last(tag_name.as_bytes()).map_or(-1, |at| at as i64);
                        if full < tag_end as i64 {
                            memchr::memmem::rfind(bytes, format!("</{tag_name}").as_bytes())
                                .map_or(-1, |at| at as i64)
                        } else {
                            full
                        }
                    });
                    if position < tag_end as i64 {
                        return Err("unclosed xml attribute".into());
                    }
                }
                if entry.generation != scope.generation {
                    let (prefix, local) = split_qname(tag_name);
                    let uri = scope.get(prefix.unwrap_or(""));
                    entry.id = builder.tree.intern(&converted_name(uri, local));
                    entry.generation = scope.generation;
                }
                let name_id = entry.id;

                let attr_start = builder.attr_mark();
                for &(qname, value_start, value_end) in &raw_attrs {
                    let attr_id = match attribute_names.get(qname) {
                        Some(&(id, generation)) if generation == scope.generation => id,
                        cached => {
                            if cached.is_none() && !valid_name(qname) {
                                return Err(format!("invalid attribute:{qname}"));
                            }
                            let attr_uri = match split_qname(qname) {
                                (Some("xmlns"), _) | (None, "xmlns") => Some(XMLNS_NS),
                                (Some("xml"), _) => Some(XML_NS),
                                (Some(prefix), _) => scope.get(prefix),
                                (None, _) => None,
                            };
                            let (_, attr_local) = split_qname(qname);
                            let id = builder.tree.intern(&converted_name(attr_uri, attr_local));
                            attribute_names.insert(qname, (id, scope.generation));
                            id
                        }
                    };
                    let value = &bytes[value_start..value_end];
                    if memchr::memchr3(b'&', b'\t', b'\n', value).is_none()
                        && memchr::memchr(b'\r', value).is_none()
                    {
                        builder.attr_range(attr_id, (value_start as u32, value_end as u32));
                    } else {
                        scratch.clear();
                        decode_attribute(&source[value_start..value_end], &mut scratch)?;
                        builder.attr(attr_id, &scratch);
                    }
                }
                builder.open(name_id, attr_start);
                if closed {
                    builder.close();
                    scope.truncate(scope_len);
                } else {
                    if open.len() >= MAX_DEPTH {
                        return Err(format!("XML nesting exceeds the {MAX_DEPTH}-level depth cap"));
                    }
                    open.push((tag_name, scope_len));
                }
                start = tag_end + 1;
            }
        }
    }
    append_text(&mut builder, &source, start, bytes.len(), &mut scratch)?;
    drop(open);
    drop(element_names);
    drop(attribute_names);
    drop(raw_attrs);
    Ok(builder.finish(source))
}

/// parseDCC's DOCTYPE: the tokens of `split` after the `<` up to the first `>` (or
/// `<`); when the first is `!doctype` (any case) the declaration ends after that one.
/// Ok(None) when it is not a DOCTYPE, an error when no `>` or `<` ends it.
fn doctype_end(b: &[u8], tag_start: usize) -> Result<Option<usize>, String> {
    // /'[^']+'|"[^"]+"|[^\s<>\/=]+=?|(\/?\s*>|<)/g
    let space = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let closer = |at: usize| -> Option<usize> {
        let mut k = at;
        if b.get(k) == Some(&b'/') {
            k += 1;
        }
        while k < b.len() && space(b[k]) {
            k += 1;
        }
        (b.get(k) == Some(&b'>')).then_some(k + 1)
    };
    let mut p = tag_start + 1;
    let mut first: Option<(usize, usize)> = None;
    let mut tokens = 0;
    while p < b.len() {
        let c = b[p];
        let quoted = |quote: u8| -> Option<usize> {
            let close = memchr::memchr(quote, &b[p + 1..])?;
            (close > 0).then_some(p + 1 + close + 1)
        };
        let token_end = if c == b'<' {
            tokens += 1;
            return Ok(doctype(b, first, tokens, p + 1));
        } else if c == b'>' || ((c == b'/' || space(c)) && closer(p).is_some()) {
            tokens += 1;
            return Ok(doctype(b, first, tokens, closer(p).unwrap_or(p + 1)));
        } else if c == b'/' || c == b'=' || space(c) {
            p += 1;
            continue;
        } else if let Some(end) = (c == b'\'' || c == b'"').then(|| quoted(c)).flatten() {
            end
        } else {
            let mut k = p;
            while k < b.len() && !space(b[k]) && !matches!(b[k], b'<' | b'>' | b'/' | b'=') {
                k += 1;
            }
            if b.get(k) == Some(&b'=') {
                k += 1;
            }
            k
        };
        tokens += 1;
        first.get_or_insert((p, token_end));
        p = token_end;
    }
    Err("element parse error: TypeError: Cannot read properties of undefined (reading 'length')".into())
}

fn doctype(b: &[u8], first: Option<(usize, usize)>, tokens: usize, end: usize) -> Option<usize> {
    let (start, stop) = first?;
    let token = b[start..stop].to_ascii_lowercase();
    (tokens > 1 && memchr::memmem::find(&token, b"!doctype").is_some()).then_some(end)
}

/// xmldom's `appendText`: a text node inside the root (text outside it is dropped, but
/// its entities are still decoded, and an unknown one still fails).
fn append_text(
    builder: &mut Builder,
    source: &str,
    from: usize,
    to: usize,
    scratch: &mut String,
) -> Result<(), String> {
    if to <= from {
        return Ok(());
    }
    let raw = &source[from..to];
    if memchr::memchr(b'&', raw.as_bytes()).is_none() {
        if builder.depth() > 0 {
            builder.text_range((from as u32, to as u32));
        }
        return Ok(());
    }
    scratch.clear();
    decode_entities(raw, scratch)?;
    if builder.depth() > 0 {
        builder.text(scratch);
    }
    Ok(())
}

fn decode_attribute(raw: &str, out: &mut String) -> Result<(), String> {
    if raw.contains(['\t', '\n', '\r']) {
        let normalized = raw.replace(['\t', '\n', '\r'], " ");
        decode_entities(&normalized, out)
    } else {
        decode_entities(raw, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_and_names() {
        let tree = parse(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:t>a&amp;b&#x41;&#66;</w:t></w:document>"#
                .to_owned(),
            &[],
        )
        .unwrap();
        let root = tree.top_nodes()[0];
        assert_eq!(tree.name(root), Some("w:document"));
        let t = tree.kids(root)[0];
        assert_eq!(tree.name(t), Some("w:t"));
        assert_eq!(tree.text_of(tree.kids(t)[0]), Some("a&bAB"));
    }

    #[test]
    fn refuses_unknown_entity_and_unclosed() {
        assert!(parse("<a>&nbsp;</a>".into(), &[]).is_err());
        assert!(parse("<a><b></a>".into(), &[]).is_err());
    }

    /// xmldom's line-break normalization, char by char: what `decode` must match.
    fn reference(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    if matches!(chars.peek(), Some('\n' | '\u{85}')) {
                        chars.next();
                    }
                    out.push('\n');
                }
                '\u{85}' | '\u{2028}' => out.push('\n'),
                _ => out.push(c),
            }
        }
        out
    }

    #[test]
    fn end_tags_match_last_index_of() {
        let pieces = ["</", "<", ">", "a", "b", "a:b", " ", "/"];
        let mut seed = 7u64;
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..(seed % 13) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push_str(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let bytes = text.as_bytes();
            let mut end_tags = EndTags { bytes, scanned_to: bytes.len(), last: FxMap::default() };
            for name in ["b", "a", "a:b", "ab", ""] {
                let expected = memchr::memmem::rfind(bytes, format!("</{name}>").as_bytes());
                assert_eq!(end_tags.last(name.as_bytes()), expected, "{text:?} {name}");
            }
        }
    }

    #[test]
    fn decode_normalizes_line_breaks_like_xmldom() {
        let pieces = ["\r", "\n", "\u{85}", "\u{2028}", "\u{2029}", "\u{c2}", "\u{20ac}", "a"];
        let mut seed = 1u64;
        for _ in 0..20_000 {
            let mut text = String::new();
            for _ in 0..(seed % 9) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push_str(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            assert_eq!(super::decode(text.clone().into_bytes()), reference(&text), "{text:?}");
        }
    }
}
