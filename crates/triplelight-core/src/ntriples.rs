//! N-Triples and N-Quads parsing. The default implements RDF 1.1. The `rdf-12` feature adds triple terms,
//! directional literals and document version announcements.
//! Each statement occupies its own line (`\n`, `\r` or `\r\n`). Triples and quads end in a dot. A trailing
//! `#` comment is allowed. Comments are not checked for UTF-8; every token is.
//! Derived in part from oxttl (MIT); see THIRD_PARTY_NOTICES.md.
use crate::rdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, ReadError, Term, Triple};
use std::io::BufRead;
#[cfg(feature = "rdf-12")]
use triplelight_rdf::BaseDirection;

const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// Why a line was refused.
enum LineError { Syntax(String), TooLarge }
impl From<String> for LineError { fn from(m: String) -> Self { LineError::Syntax(m) } }
impl From<&str> for LineError { fn from(m: &str) -> Self { LineError::Syntax(m.to_owned()) } }
impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { LineError::Syntax(m) => f.write_str(m), LineError::TooLarge => write!(f, "token larger than {MAX_TOKEN} bytes") }
    }
}

/// One parsed line: a quad, or a line that emits no quad.
type LineResult = Result<Option<Quad>, LineError>;

/// `capped`: tokens over `MAX_TOKEN` are refused, as `oxttl`'s streaming readers did; its slice parsers had no cap.
struct Line<'a> { data: &'a [u8], pos: usize, with_graph: bool, capped: bool }

impl<'a> Line<'a> {
    fn peek(&self) -> u8 { self.data.get(self.pos).copied().unwrap_or(b'\n') }
    fn at(&self, i: usize) -> u8 { self.data.get(self.pos + i).copied().unwrap_or(b'\n') }
    fn skip_ws(&mut self) { while matches!(self.peek(), b' ' | b'\t') { self.pos += 1 } }
    /// End of the statement area: the line end or a comment.
    fn at_end(&self) -> bool { matches!(self.peek(), b'\n' | b'#') }

    fn subject_predicate(&mut self) -> Result<(NamedOrBlankNode, NamedNode), LineError> {
        let subject = match self.peek() {
            b'<' if self.at(1) != b'<' => NamedNode::new_unchecked(self.iri()?).into(),
            b'_' if self.at(1) == b':' => BlankNode::new_unchecked(self.blank_node()?).into(),
            _ => return Err("The subject of a triple must be an IRI or a blank node".into()),
        };
        self.skip_ws();
        let predicate = match self.peek() {
            b'<' if self.at(1) != b'<' => NamedNode::new_unchecked(self.iri()?),
            _ => return Err("The predicate of a triple must be an IRI".into()),
        };
        self.skip_ws();
        Ok((subject, predicate))
    }

    fn simple_object(&mut self) -> Result<Term, LineError> {
        let object = match self.peek() {
            b'<' if self.at(1) != b'<' => NamedNode::new_unchecked(self.iri()?).into(),
            b'_' if self.at(1) == b':' => BlankNode::new_unchecked(self.blank_node()?).into(),
            b'"' => {
                let value = self.string()?;
                self.skip_ws();
                match (self.peek(), self.at(1)) {
                    (b'@', _) => self.language_literal(value)?.into(),
                    (b'^', b'^') => {
                        self.pos += 2;
                        self.skip_ws();
                        if !(self.peek() == b'<' && self.at(1) != b'<') { return Err("A literal datatype must be an IRI".into()) }
                        let datatype = self.iri()?;
                        if datatype == RDF_LANG_STRING {
                            return Err("The datatype of a literal without a language tag must not be rdf:langString".into());
                        }
                        #[cfg(feature = "rdf-12")]
                        if datatype == triplelight_rdf::vocab::rdf::DIR_LANG_STRING.as_str() {
                            return Err("The datatype of a literal without a base direction must not be rdf:dirLangString".into());
                        }
                        Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype)).into()
                    }
                    _ => Literal::new_simple_literal(value).into(),
                }
            }
            _ => return Err("The object of a triple must be an IRI, a blank node or a literal".into()),
        };
        Ok(object)
    }

    #[cfg(feature = "rdf-12")]
    fn starts_with(&self, bytes: &[u8]) -> bool {
        self.data[self.pos..].starts_with(bytes)
    }

    #[cfg(feature = "rdf-12")]
    fn triple_term(&mut self) -> Result<Term, LineError> {
        // Only the object can be a triple term. Read its left-hand terms iteratively and
        // validate all closing delimiters before constructing the nested value.
        let mut parts = Vec::new();
        while self.starts_with(b"<<(") {
            self.pos += 3;
            self.skip_ws();
            parts.push(self.subject_predicate()?);
        }
        let object = self.simple_object()?;
        for _ in &parts {
            self.skip_ws();
            if !self.starts_with(b")>>") {
                return Err("A triple term must end with )>>".into());
            }
            self.pos += 3;
        }
        Ok(parts.into_iter().rev().fold(object, |object, (subject, predicate)| {
            Triple::new(subject, predicate, object).into()
        }))
    }

    fn statement(&mut self) -> LineResult {
        self.skip_ws();
        if self.at_end() { return Ok(None) }
        let (subject, predicate) = self.subject_predicate()?;
        #[cfg(feature = "rdf-12")]
        let object = if self.starts_with(b"<<(") { self.triple_term()? } else { self.simple_object()? };
        #[cfg(not(feature = "rdf-12"))]
        let object = self.simple_object()?;
        self.skip_ws();
        let graph_name = if !self.with_graph {
            GraphName::DefaultGraph
        } else {
            match self.peek() {
                b'<' if self.at(1) != b'<' => GraphName::NamedNode(NamedNode::new_unchecked(self.iri()?)),
                b'_' if self.at(1) == b':' => GraphName::BlankNode(BlankNode::new_unchecked(self.blank_node()?)),
                _ => GraphName::DefaultGraph,
            }
        };
        self.skip_ws();
        // A '.' directly followed by a digit starts a number token, not the end of the statement.
        if !(self.peek() == b'.' && !self.at(1).is_ascii_digit()) { return Err("Quads must be followed by a dot".into()) }
        self.pos += 1;
        self.skip_ws();
        if !self.at_end() { return Err("Only a single triple or quad can be written in a line".into()) }
        Ok(Some(Quad::new(subject, predicate, object, graph_name)))
    }

    /// `IRIREF`: `<`, then any bytes but `>` with `\u`/`\U` escapes, then `>`; the result must be an absolute IRI.
    fn iri(&mut self) -> Result<String, LineError> {
        let mut bytes = Vec::new();
        let mut i = self.pos + 1;
        loop {
            let rest = &self.data[i..];
            let Some(end) = rest.iter().position(|&b| b == b'>' || b == b'\\') else { return Err("Unterminated IRI".into()) };
            bytes.extend_from_slice(&rest[..end]);
            i += end;
            if self.data[i] == b'>' { break }
            let (c, len) = escape(&self.data[i..], false)?;
            let mut buf = [0; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            i += len;
        }
        if self.capped && i + 1 - self.pos > MAX_TOKEN { return Err(LineError::TooLarge) }
        self.pos = i + 1;
        let iri = String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {e}"))?;
        triplelight_iri::Iri::validate(&iri).map_err(|e| format!("Invalid IRI <{iri}>: {e}"))?;
        Ok(iri)
    }

    /// `BLANK_NODE_LABEL`: `_:` (PN_CHARS_U | digit) ((PN_CHARS | '.')* PN_CHARS)?; trailing dots are not part of it.
    fn blank_node(&mut self) -> Result<String, LineError> {
        let start = self.pos + 2;
        let mut i = start;
        loop {
            let (c, len) = match utf8_char(&self.data[i..]) { Ok(Some(r)) => r, Ok(None) => break, Err(e) => return Err(e.into()) };
            let ok = if i == start { pn_chars_u(c) || c.is_ascii_digit() } else { pn_chars(c) || c == '.' };
            if !ok { break }
            i += len;
        }
        while i > start && self.data[i - 1] == b'.' { i -= 1 }
        if i == start { return Err("A blank node ID cannot be empty".into()) }
        if self.capped && i - self.pos > MAX_TOKEN { return Err(LineError::TooLarge) }
        self.pos = i;
        Ok(std::str::from_utf8(&self.data[start..i]).map_err(|e| format!("Invalid UTF-8: {e}"))?.to_owned())
    }

    /// `STRING_LITERAL_QUOTE`, with `ECHAR` and `UCHAR` escapes; raw line breaks are not allowed.
    fn string(&mut self) -> Result<String, LineError> {
        let mut out = String::new();
        let mut i = self.pos + 1;
        loop {
            let rest = &self.data[i..];
            let end = rest.iter().position(|&b| matches!(b, b'"' | b'\\' | b'\n' | b'\r')).ok_or("Unterminated string literal")?;
            out.push_str(std::str::from_utf8(&rest[..end]).map_err(|e| format!("Invalid UTF-8: {e}"))?);
            i += end;
            match self.data[i] {
                b'"' => { if self.capped && i + 1 - self.pos > MAX_TOKEN { return Err(LineError::TooLarge) } self.pos = i + 1; return Ok(out) }
                b'\\' => { let (c, len) = escape(&self.data[i..], true)?; out.push(c); i += len }
                _ => return Err("Line jumps are not allowed in string literals, use \\n".into()),
            }
        }
    }

    fn language_literal(&mut self, value: String) -> Result<Literal, LineError> {
        let mut language = self.lang_tag()?;
        language.make_ascii_lowercase();
        #[cfg(feature = "rdf-12")]
        if self.starts_with(b"--") {
            let direction = match self.data.get(self.pos + 2..self.pos + 5) {
                Some(b"ltr") => BaseDirection::Ltr,
                Some(b"rtl") => BaseDirection::Rtl,
                _ => return Err("A literal base direction must be ltr or rtl".into()),
            };
            if self.at(5).is_ascii_alphanumeric() || self.at(5) == b'-' {
                return Err("A literal base direction must be ltr or rtl".into());
            }
            self.pos += 5;
            if self.capped && language.len() + 6 > MAX_TOKEN { return Err(LineError::TooLarge) }
            return Ok(Literal::new_directional_language_tagged_literal_unchecked(value, language, direction));
        }
        Ok(Literal::new_language_tagged_literal_unchecked(value, language))
    }

    /// Scan the BCP 47 tag, leaving an RDF 1.2 base-direction suffix for the caller.
    fn lang_tag(&mut self) -> Result<String, LineError> {
        let data = &self.data[self.pos..];
        let mut is_last_block_empty = true;
        let mut digits_allowed = false;
        for (i, &c) in data[1..].iter().enumerate() {
            if (digits_allowed && c.is_ascii_alphanumeric()) || (!digits_allowed && c.is_ascii_alphabetic()) {
                is_last_block_empty = false;
            } else if i == 0 {
                return Err("A language code should always start with a letter".into());
            } else if is_last_block_empty {
                if c != b'-' { return self.finish_lang(&data[1..i], i) }
                #[cfg(feature = "rdf-12")]
                return self.finish_lang(&data[1..i], i);
                #[cfg(not(feature = "rdf-12"))]
                return Err("Literal base direction are only allowed in RDF 1.2".into());
            } else if c == b'-' {
                is_last_block_empty = true;
                digits_allowed = true;
            } else {
                return self.finish_lang(&data[1..=i], i + 1);
            }
        }
        // The end of the line acts as the terminator that follows it in the stream.
        let i = data.len() - 1;
        if i == 0 { return Err("A language code should always start with a letter".into()) }
        if is_last_block_empty { self.finish_lang(&data[1..i], i) } else { self.finish_lang(&data[1..=i], i + 1) }
    }

    fn finish_lang(&mut self, tag: &[u8], consumed: usize) -> Result<String, LineError> {
        let tag = std::str::from_utf8(tag).map_err(|e| format!("Invalid UTF-8: {e}"))?;
        if self.capped && consumed > MAX_TOKEN { return Err(LineError::TooLarge) }
        if !crate::langtag::is_well_formed(tag) { return Err(format!("Invalid language tag {tag:?}").into()) }
        self.pos += consumed;
        Ok(tag.to_owned())
    }
}

/// `UCHAR` (`\uXXXX`, `\UXXXXXXXX`), and with `echar` also `\t \b \n \r \f \" \' \\`. Returns the character and the
/// escape's length in bytes.
fn escape(data: &[u8], echar: bool) -> Result<(char, usize), String> {
    let hex = |len: usize| -> Result<char, String> {
        let digits = data.get(2..2 + len).ok_or("Incomplete escape sequence")?;
        let mut v = 0u32;
        for &d in digits {
            v = v * 16 + match d { b'0'..=b'9' => d - b'0', b'a'..=b'f' => d - b'a' + 10, b'A'..=b'F' => d - b'A' + 10, _ => return Err("Invalid hexadecimal escape".into()) } as u32;
        }
        char::from_u32(v).ok_or_else(|| format!("The escape sequence encodes {v:X}, which is not a valid unicode character"))
    };
    match data.get(1).copied().unwrap_or(b'\n') {
        b'u' => Ok((hex(4)?, 6)),
        b'U' => Ok((hex(8)?, 10)),
        b't' if echar => Ok(('\t', 2)),
        b'b' if echar => Ok(('\x08', 2)),
        b'n' if echar => Ok(('\n', 2)),
        b'r' if echar => Ok(('\r', 2)),
        b'f' if echar => Ok(('\x0C', 2)),
        b'"' if echar => Ok(('"', 2)),
        b'\'' if echar => Ok(('\'', 2)),
        b'\\' if echar => Ok(('\\', 2)),
        c => Err(format!("Unexpected escape character '\\{}'", char::from(c))),
    }
}

/// One UTF-8 character at the start of `data`, or `None` at the end; invalid encodings are errors.
fn utf8_char(data: &[u8]) -> Result<Option<(char, usize)>, String> {
    let Some(&b) = data.first() else { return Ok(None) };
    if b.is_ascii() { return Ok(Some((char::from(b), 1))) }
    let len = match b { 0x00..=0x7F => 1, 0xC2..=0xDF => 2, 0xE0..=0xEF => 3, 0xF0..=0xF4 => 4, _ => return Err("Invalid UTF-8 character encoding".into()) };
    let bytes = data.get(..len).ok_or("Invalid UTF-8 character encoding")?;
    let s = std::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 character encoding")?;
    Ok(s.chars().next().map(|c| (c, len)))
}

/// The PN_CHARS_BASE, PN_CHARS_U and PN_CHARS productions shared by the N-Triples and Turtle grammars.
pub(crate) fn pn_chars_base(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '\u{00C0}'..='\u{00D6}' | '\u{00D8}'..='\u{00F6}' | '\u{00F8}'..='\u{02FF}'
        | '\u{0370}'..='\u{037D}' | '\u{037F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}
pub(crate) fn pn_chars_u(c: char) -> bool { pn_chars_base(c) || c == '_' }
pub(crate) fn pn_chars(c: char) -> bool {
    pn_chars_u(c) || matches!(c, '-' | '0'..='9' | '\u{00B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

/// Parses one line (without its terminator) of N-Quads (`with_graph`) or N-Triples. Reading past the end of the
/// line sees a line terminator, as in the original stream.
fn parse_line(line: &[u8], with_graph: bool, capped: bool) -> LineResult { Line { data: line, pos: 0, with_graph, capped }.statement() }

fn parse_document_line(line: &[u8], with_graph: bool, capped: bool) -> LineResult {
    #[cfg(feature = "rdf-12")]
    {
        let mut parser = Line { data: line, pos: 0, with_graph, capped };
        parser.skip_ws();
        if parser.starts_with(b"VERSION") {
            parser.pos += b"VERSION".len();
            parser.skip_ws();
            if parser.peek() != b'"' {
                return Err("VERSION must be followed by a string literal".into());
            }
            // The grammar treats VERSION as a hint and emits no statement.
            parser.string()?;
            parser.skip_ws();
            if !parser.at_end() {
                return Err("Only a single statement can be written in a line".into());
            }
            return Ok(None);
        }
        parser.statement()
    }
    #[cfg(not(feature = "rdf-12"))]
    parse_line(line, with_graph, capped)
}

/// Parses one N-Quads statement line, as the WAL stores them (task #36): `None` if it is not exactly one valid quad.
pub fn parse_nquads_line(line: &str) -> Option<Quad> {
    // `\r` and `\n` break lines as in a document: the text must hold exactly one statement and nothing invalid.
    let mut found = None;
    for piece in line.as_bytes().split(|&b| matches!(b, b'\n' | b'\r')) {
        // Uncapped, as `oxttl`'s `for_slice` was: the WAL may hold large records the old binary accepted.
        match parse_line(piece, true, false) {
            Ok(Some(q)) if found.is_none() => found = Some(q),
            Ok(None) => {}
            _ => return None,
        }
    }
    found
}

/// The largest token `oxttl` 0.2 buffers (16 MiB); a longer one is an I/O error (`OutOfMemory`) there and here.
pub const MAX_TOKEN: usize = 4096 * 4096;
/// The longest line read into memory. Bounded so input that never breaks a line cannot exhaust memory; four tokens of
/// the maximum size fit. (Unlike `oxttl`, a longer line made only of whitespace or a comment is refused too.)
const MAX_LINE: usize = 5 * MAX_TOKEN;

fn too_large(what: &str) -> ReadError {
    ReadError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, format!("{what} larger than {MAX_TOKEN} bytes")))
}

/// Streams the statements of an N-Triples (`with_graph = false`) or N-Quads document, line by line, with lines broken
/// at `\n`, `\r` or `\r\n` as they are read. A syntax error is reported with its line number and ends the stream.
pub struct StatementReader<R> { input: R, with_graph: bool, capped: bool, line: u64, buf: Vec<u8>, done: bool }

impl<R: BufRead> StatementReader<R> {
    /// A streaming reader, with `oxttl`'s 16 MiB token limit and a line limit that bounds memory.
    pub fn new(input: R, with_graph: bool) -> Self { Self { input, with_graph, capped: true, line: 0, buf: Vec::new(), done: false } }
    /// A reader over input already in memory: no token or line limit, as `oxttl`'s slice parsers.
    fn uncapped(input: R, with_graph: bool) -> Self { Self { input, with_graph, capped: false, line: 0, buf: Vec::new(), done: false } }

    /// Reads the next line into `buf` (without its break). `Ok(false)` at the end of the input.
    fn read_line(&mut self) -> Result<bool, ReadError> {
        self.buf.clear();
        let mut any = false;
        loop {
            let available = match self.input.fill_buf() { Ok(b) => b, Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue, Err(e) => return Err(ReadError::Io(e)) };
            if available.is_empty() { return Ok(any) }
            any = true;
            if let Some(i) = available.iter().position(|&b| b == b'\n' || b == b'\r') {
                let cr = available[i] == b'\r';
                if self.capped && self.buf.len() + i > MAX_LINE { return Err(too_large("line")) }
                self.buf.extend_from_slice(&available[..i]);
                self.input.consume(i + 1);
                // `\r\n` is one line break.
                if cr {
                    loop {
                        match self.input.fill_buf() {
                            Ok(b) => { if b.first() == Some(&b'\n') { self.input.consume(1) } break }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => return Err(ReadError::Io(e)),
                        }
                    }
                }
                return Ok(true);
            }
            let n = available.len();
            if self.capped && self.buf.len() + n > MAX_LINE { return Err(too_large("line")) }
            self.buf.extend_from_slice(available);
            self.input.consume(n);
        }
    }
}

impl<R: BufRead> Iterator for StatementReader<R> {
    type Item = Result<Quad, ReadError>;
    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            match self.read_line() {
                Ok(false) => self.done = true,
                Ok(true) => {
                    self.line += 1;
                    match parse_document_line(&self.buf, self.with_graph, self.capped) {
                        Ok(Some(q)) => return Some(Ok(q)),
                        Ok(None) => {}
                        Err(LineError::Syntax(e)) => { self.done = true; return Some(Err(ReadError::Syntax(format!("line {}: {e}", self.line)))) }
                        Err(LineError::TooLarge) => { self.done = true; return Some(Err(too_large("token"))) }
                    }
                }
                Err(e) => { self.done = true; return Some(Err(e)) }
            }
        }
        None
    }
}

/// The triples of an N-Triples document.
pub fn read_ntriples<R: BufRead>(input: R) -> impl Iterator<Item = Result<Triple, ReadError>> {
    StatementReader::new(input, false).map(|q| q.map(|q| Triple::new(q.subject, q.predicate, q.object)))
}

/// The triples of an N-Triples document held in memory, without the streaming limits (as `oxttl`'s `for_slice`).
pub fn read_ntriples_slice(data: &[u8]) -> impl Iterator<Item = Result<Triple, ReadError>> + '_ {
    StatementReader::uncapped(data, false).map(|q| q.map(|q| Triple::new(q.subject, q.predicate, q.object)))
}

/// The quads of an N-Quads document.
pub fn read_nquads<R: BufRead>(input: R) -> impl Iterator<Item = Result<Quad, ReadError>> { StatementReader::new(input, true) }

/// The offset just after the first line break (`\n`, `\r` or `\r\n`) at or after `from`, or the file size. Scans with
/// a fixed 4 KiB buffer, so a file without breaks is not read into memory.
fn next_line_start(file: &mut std::fs::File, from: u64, size: u64) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(from))?;
    let mut buf = [0u8; 4096];
    let mut pos = from;
    let mut after_cr = false;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 { return Ok(size) }
        for (i, &b) in buf[..n].iter().enumerate() {
            if after_cr { return Ok(if b == b'\n' { pos + i as u64 + 1 } else { pos + i as u64 }) }
            match b { b'\n' => return Ok(pos + i as u64 + 1), b'\r' => after_cr = true, _ => {} }
        }
        pos += n as u64;
    }
}

/// Splits an N-Triples file into at most `parts` readers at line breaks, for parallel parsing. Their triples,
/// concatenated in order, are the file's.
pub fn split_ntriples_file(path: &std::path::Path, parts: usize) -> std::io::Result<Vec<impl Iterator<Item = Result<Triple, ReadError>> + use<>>> {
    use std::io::{Read, Seek, SeekFrom};
    let size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut bounds = vec![0u64];
    for i in 1..parts.max(1) as u64 {
        let at = next_line_start(&mut file, size * i / parts as u64, size)?;
        if at > *bounds.last().unwrap() && at < size { bounds.push(at) }
    }
    bounds.push(size);
    bounds.dedup();
    bounds.windows(2).map(|w| {
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(w[0]))?;
        Ok(read_ntriples(std::io::BufReader::new(file.take(w[1] - w[0]))))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Line breaks across buffer edges: every small buffer size, for `\n`, `\r` and `\r\n` documents, gives the
    /// same statements and the same line number for a trailing error.
    #[test]
    fn line_breaks_across_buffer_edges() {
        let line = "<http://a> <http://b> \"x\" .";
        for brk in ["\n", "\r", "\r\n"] {
            let doc = format!("{line}{brk}{line}{brk}# c{brk}{brk}<bad>{brk}");
            for cap in 1..40 {
                let r = std::io::BufReader::with_capacity(cap, doc.as_bytes());
                let got: Vec<_> = StatementReader::new(r, false).collect();
                assert_eq!(got.len(), 3, "{brk:?} cap {cap}");
                assert!(got[..2].iter().all(Result::is_ok));
                match &got[2] { Err(ReadError::Syntax(m)) => assert!(m.starts_with("line 5:"), "{brk:?} cap {cap}: {m}"), e => panic!("{e:?}") }
            }
        }
    }
    /// The shard planner cuts only at line breaks, including `\r` and `\r\n`, and the shards together are the file.
    #[test]
    fn shards_cut_at_line_breaks() {
        let dir = std::env::temp_dir().join(format!("tl-nt-shards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for brk in ["\n", "\r", "\r\n"] {
            let doc: String = (0..500).map(|i| format!("<http://a/{i}> <http://b> \"{i}\" .{brk}")).collect();
            let path = dir.join("doc.nt");
            std::fs::write(&path, &doc).unwrap();
            let whole: Vec<Triple> = read_ntriples(doc.as_bytes()).map(Result::unwrap).collect();
            for parts in [1, 2, 3, 7, 64] {
                let shards: Vec<Triple> = split_ntriples_file(&path, parts).unwrap().into_iter().flatten().map(Result::unwrap).collect();
                assert_eq!(shards, whole, "{brk:?} parts {parts}");
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// Streaming readers refuse tokens over 16 MiB as `oxttl`'s did; slice parsing and WAL lines accept them, as
    /// `oxttl`'s `for_slice` did, so a WAL written by an older binary still replays.
    #[test]
    fn oversized_tokens_follow_oxttl_by_path() {
        let big = "a".repeat(MAX_TOKEN);
        let doc = format!("<http://a> <http://b> \"{big}\" .\n");
        assert!(matches!(read_ntriples(doc.as_bytes()).next(), Some(Err(ReadError::Io(e))) if e.kind() == std::io::ErrorKind::OutOfMemory));
        assert!(matches!(read_ntriples_slice(doc.as_bytes()).next(), Some(Ok(_))));
        assert!(parse_nquads_line(doc.trim_end()).is_some());
        assert!(matches!(oxttl::NTriplesParser::new().for_slice(doc.as_bytes()).next(), Some(Ok(_))));
    }
    #[cfg(feature = "rdf-12")]
    #[test]
    fn rdf12_wal_line_round_trip() {
        for line in [
            r#"<urn:s> <urn:p> "text"@EN-us--ltr ."#,
            r#"<urn:s> <urn:p> <<( _:b <urn:p> <<( <urn:s> <urn:p> "نص"@ar--rtl )>> )>> <urn:g> ."#,
        ] {
            let quad = parse_nquads_line(line).unwrap();
            assert_eq!(parse_nquads_line(&format!("{quad} .")), Some(quad));
        }
        let quad = parse_nquads_line(r#"<urn:s> <urn:p> <<(<urn:a> <urn:p> "text"@EN-us--ltr)>>."#).unwrap();
        let Term::Triple(triple) = quad.object else { panic!("expected a triple term") };
        let Term::Literal(literal) = triple.object else { panic!("expected a directional literal") };
        assert_eq!(literal.language(), Some("en-us"));
        assert_eq!(literal.direction(), Some(BaseDirection::Ltr));

        for bad in [
            r#"<urn:s> <urn:p> <<( <urn:a> <urn:p> <urn:b> >> ."#,
            r#"<<( <urn:a> <urn:p> <urn:b> )>> <urn:p> <urn:b> ."#,
            r#"<urn:s> <urn:p> "text"@ar--RTL ."#,
            r#"<urn:s> <urn:p> "text"@ar---rtl ."#,
            r#"<urn:s> <urn:p> "text"@ar--rtl0 ."#,
            r#"<urn:s> <urn:p> "text"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString> ."#,
            "VERSION \"1.2\"\n<urn:s> <urn:p> <urn:o> .",
        ] {
            assert!(parse_nquads_line(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn rdf12_features_follow_the_build_configuration() {
        let line = r#"<urn:s> <urn:p> <<( <urn:a> <urn:p> "text"@en--ltr )>> ."#;
        let doc = format!("VERSION \"1.2\" # hint\r\n{line}\r");
        for quads in [false, true] {
            let rows = StatementReader::new(doc.as_bytes(), quads).collect::<Result<Vec<_>, _>>();
            if cfg!(feature = "rdf-12") {
                assert_eq!(rows.unwrap(), vec![parse_nquads_line(line).unwrap()]);
            } else {
                assert!(rows.is_err());
                assert!(parse_nquads_line(line).is_none());
                assert!(parse_nquads_line(r#"<urn:s> <urn:p> "text"@en--ltr ."#).is_none());
            }
        }
        for bad in [
            r#"version "1.2""#,
            r#"VERSION 1.2"#,
            r#"VERSION <urn:version>"#,
            r#"VERSION "1.2" ."#,
            r#"VERSION "1.2" <urn:s> <urn:p> <urn:o> ."#,
        ] {
            assert!(read_ntriples(bad.as_bytes()).next().unwrap().is_err(), "{bad}");
            assert!(read_nquads(bad.as_bytes()).next().unwrap().is_err(), "{bad}");
        }
    }

    #[test]
    fn statements_and_errors() {
        let ok = |s: &str, g| parse_line(s.as_bytes(), g, true).unwrap_or_else(|e| panic!("{s:?}: {e}"));
        assert!(ok("", false).is_none());
        assert!(ok("  # just a comment \u{ff}", false).is_none());
        assert_eq!(ok("<http://a> <http://b> \"x\"@EN-us . # c", false).unwrap().object.to_string(), "\"x\"@en-us");
        assert_eq!(ok("_:b1 <http://b> \"\\u00e9\\t\" <http://g> .", true).unwrap().graph_name.to_string(), "<http://g>");
        for bad in ["<a> <http://b> <http://c> .", "<http://a> <http://b> <http://c>", "<http://a> <http://b> <http://c> . <http://a> <http://b> <http://c> .",
            "<http://a> <http://b> \"x\"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString> .",
            #[cfg(not(feature = "rdf-12"))]
            "<http://a> <http://b> \"x\"@en--ltr .",
            "<http://a> <http://b> \"x\"@1 .", "_: <http://b> <http://c> .", "<http://a> <http://b> \"x\n\" ."] {
            assert!(parse_line(bad.as_bytes(), false, true).is_err(), "{bad:?}");
        }
    }
}
