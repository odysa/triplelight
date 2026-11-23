//! Turtle and TriG parsing, with RDF 1.2 syntax behind the `rdf-12` feature. The RDF 1.1 parser follows
//! `oxttl` 0.2's strict parsers, including fresh blank nodes for `[]` and collections.
//! Input is read through a refillable buffer: a statement is parsed as soon as its tokens are available, and a
//! streaming reader refuses a token longer than `oxttl`'s 16 MiB buffer, as it did (slices have no limit).
//! Derived in part from oxttl (MIT); see THIRD_PARTY_NOTICES.md.
use crate::ntriples::{pn_chars, pn_chars_base, pn_chars_u, MAX_TOKEN};
use crate::rdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, ReadError, Term};
use std::collections::{HashMap, VecDeque};
use std::io::BufRead;
use triplelight_iri::Iri;
#[cfg(feature = "rdf-12")]
use triplelight_rdf::{BaseDirection, MAX_TRIPLE_TERM_DEPTH, Triple, triple_term_depth};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// The most input appended to the buffer per refill.
const REFILL: usize = 64 * 1024;
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Iri(String),
    PName { prefix: String, local: String, might_be_invalid: bool },
    BNode(String),
    Lang(String),
    #[cfg(feature = "rdf-12")]
    LangDir(String, BaseDirection),
    Str {
        value: String,
        #[cfg(feature = "rdf-12")]
        long: bool,
    },
    Integer(String),
    Decimal(String),
    Double(String),
    Keyword(String),
    Punct(&'static str),
    Var,
    Eof,
}

enum Fail { Syntax(String), Io(std::io::Error), TooLarge }
impl From<String> for Fail { fn from(m: String) -> Self { Fail::Syntax(m) } }
impl From<&str> for Fail { fn from(m: &str) -> Self { Fail::Syntax(m.to_owned()) } }
type R<T> = Result<T, Fail>;

/// The input: bytes from `pos` on, refilled from the reader on demand.
struct Src<I> { input: I, buf: Vec<u8>, pos: usize, eof: bool, capped: bool, token_start: usize }

impl<I: BufRead> Src<I> {
    /// The byte at `pos + i`, reading more input if needed; `None` at the end of the input.
    fn at(&mut self, i: usize) -> R<Option<u8>> {
        while self.pos + i >= self.buf.len() {
            if self.eof { return Ok(None) }
            if self.capped && self.buf.len() - self.token_start >= MAX_TOKEN { return Err(Fail::TooLarge) }
            let chunk = match self.input.fill_buf() { Ok(c) => c, Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue, Err(e) => return Err(Fail::Io(e)) };
            if chunk.is_empty() { self.eof = true; return Ok(None) }
            // Append a bounded piece, so a reader that exposes a whole document (a slice) is not copied at once and
            // the token limit is checked at a fine grain.
            let n = chunk.len().min(REFILL);
            self.buf.extend_from_slice(&chunk[..n]);
            self.input.consume(n);
        }
        Ok(Some(self.buf[self.pos + i]))
    }
    fn byte(&mut self, i: usize) -> R<u8> { Ok(self.at(i)?.unwrap_or(0)) }
    /// The offset (from `pos`) of the first byte at or after `from` that matches `stop`, reading more input as
    /// needed; `None` if the input ends first. Scans whole buffered spans instead of byte by byte.
    fn find(&mut self, from: usize, stop: impl Fn(u8) -> bool) -> R<Option<usize>> {
        let mut i = from;
        loop {
            if self.at(i)?.is_none() { return Ok(None) }
            let span = &self.buf[self.pos + i..];
            if let Some(k) = span.iter().position(|&b| stop(b)) { return Ok(Some(i + k)) }
            i += span.len();
        }
    }
    /// Advances `pos` past bytes matching `skip`, releasing them as it goes (no token limit applies).
    fn skip_while(&mut self, skip: impl Fn(u8) -> bool) -> R<()> {
        loop {
            self.compact();
            if self.at(0)?.is_none() { return Ok(()) }
            let span = &self.buf[self.pos..];
            match span.iter().position(|&b| !skip(b)) { Some(k) => { self.pos += k; return Ok(()) } None => self.pos += span.len() }
        }
    }
    /// Drops consumed input once enough has accumulated.
    fn compact(&mut self) {
        if self.pos > (1 << 20) { self.buf.drain(..self.pos); self.pos = 0; }
        self.token_start = self.pos;
    }
    fn slice(&self, a: usize, b: usize) -> &[u8] { &self.buf[self.pos + a..self.pos + b] }
}

/// One UTF-8 character at `pos + i`: `(char, len)`, `None` at the end; invalid encodings are errors.
fn char_at<I: BufRead>(s: &mut Src<I>, i: usize) -> R<Option<(char, usize)>> {
    let Some(b) = s.at(i)? else { return Ok(None) };
    if b.is_ascii() { return Ok(Some((char::from(b), 1))) }
    let len = match b { 0xC2..=0xDF => 2, 0xE0..=0xEF => 3, 0xF0..=0xF4 => 4, _ => return Err("Invalid UTF-8 character encoding".into()) };
    for k in 1..len { if s.at(i + k)?.is_none() { return Err("Invalid UTF-8 character encoding".into()) } }
    let c = std::str::from_utf8(s.slice(i, i + len)).map_err(|_| "Invalid UTF-8 character encoding")?.chars().next().unwrap();
    Ok(Some((c, len)))
}

fn pn_chars_base_but_not_valid_iri(c: char) -> bool {
    matches!(c, '\u{FFF0}'..='\u{FFFD}') || u32::from(c) % u32::from('\u{FFFE}') == 0 || u32::from(c) % u32::from('\u{FFFF}') == 0
}

/// `\u`/`\U` escapes, and with `echar` the ECHAR set; `(char, length)`.
fn escape<I: BufRead>(s: &mut Src<I>, echar: bool) -> R<(char, usize)> {
    let hex = |s: &mut Src<I>, len: usize| -> R<char> {
        let mut v = 0u32;
        for k in 0..len {
            let d = s.at(2 + k)?.ok_or("Incomplete escape sequence")?;
            v = v * 16 + match d { b'0'..=b'9' => d - b'0', b'a'..=b'f' => d - b'a' + 10, b'A'..=b'F' => d - b'A' + 10, _ => return Err("Invalid hexadecimal escape".into()) } as u32;
        }
        char::from_u32(v).ok_or_else(|| Fail::Syntax(format!("The escape sequence encodes {v:X}, which is not a valid unicode character")))
    };
    match s.at(1)?.ok_or("Incomplete escape sequence")? {
        b'u' => Ok((hex(s, 4)?, 6)),
        b'U' => Ok((hex(s, 8)?, 10)),
        b't' if echar => Ok(('\t', 2)),
        b'b' if echar => Ok(('\x08', 2)),
        b'n' if echar => Ok(('\n', 2)),
        b'r' if echar => Ok(('\r', 2)),
        b'f' if echar => Ok(('\x0C', 2)),
        b'"' if echar => Ok(('"', 2)),
        b'\'' if echar => Ok(('\'', 2)),
        b'\\' if echar => Ok(('\\', 2)),
        c => Err(Fail::Syntax(format!("Unexpected escape character '\\{}'", char::from(c)))),
    }
}

struct Lexer<I> { src: Src<I>, base: Option<Iri>, peeked: Option<Tok> }

impl<I: BufRead> Lexer<I> {
    /// Skips whitespace and comments, releasing the skipped input as it goes (as `oxttl` advances past it), so a long
    /// run of whitespace is neither held in memory nor counted against the next token's size limit.
    fn skip_ws(&mut self) -> R<()> {
        loop {
            self.src.compact();
            match self.src.at(0)? {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.src.skip_while(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))?,
                // Comment bytes are not decoded.
                Some(b'#') => self.src.skip_while(|b| b != b'\n' && b != b'\r')?,
                _ => return Ok(()),
            }
        }
    }
    fn next(&mut self) -> R<Tok> { match self.peeked.take() { Some(t) => Ok(t), None => self.lex() } }

    fn lex(&mut self) -> R<Tok> {
        self.skip_ws()?;
        self.src.compact();
        let Some(b) = self.src.at(0)? else { return Ok(Tok::Eof) };
        let (tok, len) = match b {
            b'<' => match self.src.byte(1)? {
                #[cfg(feature = "rdf-12")]
                b'<' if self.src.byte(2)? == b'(' => (Tok::Punct("<<("), 3),
                b'<' => (Tok::Punct("<<"), 2),
                _ => self.iri()?,
            },
            b'>' => if self.src.byte(1)? == b'>' { (Tok::Punct(">>"), 2) } else { (Tok::Punct(">"), 1) },
            b'_' => match self.src.byte(1)? { b':' => self.blank_node()?, c => return Err(Fail::Syntax(format!("Unexpected character '{}'", char::from(c)))) },
            b'"' | b'\'' => if self.src.byte(1)? == b && self.src.byte(2)? == b { self.long_string(b)? } else { self.string(b)? },
            b'@' => self.lang_tag()?,
            b'.' => if self.src.byte(1)?.is_ascii_digit() { self.number()? } else { (Tok::Punct("."), 1) },
            b'^' => if self.src.byte(1)? == b'^' { (Tok::Punct("^^"), 2) } else { (Tok::Punct("^"), 1) },
            b'(' => (Tok::Punct("("), 1),
            b')' => if self.src.byte(1)? == b'>' && self.src.byte(2)? == b'>' { (Tok::Punct(")>>"), 3) } else { (Tok::Punct(")"), 1) },
            b'[' => (Tok::Punct("["), 1),
            b']' => (Tok::Punct("]"), 1),
            b'{' => if self.src.byte(1)? == b'|' { (Tok::Punct("{|"), 2) } else { (Tok::Punct("{"), 1) },
            b'}' => (Tok::Punct("}"), 1),
            b',' => (Tok::Punct(","), 1),
            b';' => (Tok::Punct(";"), 1),
            b'!' => (Tok::Punct("!"), 1),
            b'|' => if self.src.byte(1)? == b'}' { (Tok::Punct("|}"), 2) } else { (Tok::Punct("|"), 1) },
            b'=' => if self.src.byte(1)? == b'>' { (Tok::Punct("=>"), 2) } else { (Tok::Punct("="), 1) },
            b'~' => (Tok::Punct("~"), 1),
            b'0'..=b'9' | b'+' | b'-' => self.number()?,
            b'?' => { self.src.pos += 1; let (_, n) = self.pn_local()?; self.src.pos += n; return Ok(Tok::Var) }
            _ => self.pname_or_keyword()?,
        };
        self.src.pos += len;
        Ok(tok)
    }

    /// `IRIREF`, resolved against the base (or required to be absolute without one).
    fn iri(&mut self) -> R<(Tok, usize)> {
        let mut bytes = Vec::new();
        let mut i = 1;
        loop {
            let end = self.src.find(i, |b| b == b'>' || b == b'\\')?.ok_or("Unterminated IRI")?;
            bytes.extend_from_slice(self.src.slice(i, end));
            i = end;
            match self.src.byte(i)? {
                b'>' => break,
                b'\\' => {
                    let save = self.src.pos;
                    self.src.pos += i;
                    let r = escape(&mut self.src, false);
                    self.src.pos = save;
                    let (c, len) = r?;
                    let mut b = [0; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                    i += len;
                }
                _ => unreachable!(),
            }
        }
        let iri = String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8: {e}"))?;
        let iri = match &self.base {
            Some(base) => base.resolve(&iri).map_err(|e| format!("Invalid IRI <{iri}>: {e}"))?.into_string(),
            None => { Iri::validate(&iri).map_err(|e| format!("Invalid IRI <{iri}>: {e}"))?; iri }
        };
        Ok((Tok::Iri(iri), i + 1))
    }

    fn blank_node(&mut self) -> R<(Tok, usize)> {
        let mut i = 2;
        while let Some((c, len)) = char_at(&mut self.src, i)? {
            let ok = if i == 2 { pn_chars_u(c) || c.is_ascii_digit() } else { pn_chars(c) || c == '.' };
            if !ok { break }
            i += len;
        }
        while i > 2 && self.src.slice(0, i)[i - 1] == b'.' { i -= 1 }
        if i == 2 { return Err("A blank node ID cannot be empty".into()) }
        let label = std::str::from_utf8(self.src.slice(2, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?.to_owned();
        Ok((Tok::BNode(label), i))
    }

    fn string(&mut self, delimiter: u8) -> R<(Tok, usize)> {
        let mut out = String::new();
        let mut i = 1;
        loop {
            let start = i;
            i = self.src.find(i, |b| b == delimiter || b == b'\\' || b == b'\n' || b == b'\r')?.ok_or("Unterminated string literal")?;
            out.push_str(std::str::from_utf8(self.src.slice(start, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?);
            match self.src.byte(i)? {
                b if b == delimiter => return Ok((Tok::Str { value: out, #[cfg(feature = "rdf-12")] long: false }, i + 1)),
                b'\\' => {
                    let save = self.src.pos;
                    self.src.pos += i;
                    let r = escape(&mut self.src, true);
                    self.src.pos = save;
                    let (c, len) = r?;
                    out.push(c);
                    i += len;
                }
                _ => return Err("Line jumps are not allowed in string literals, use \\n".into()),
            }
        }
    }

    fn long_string(&mut self, delimiter: u8) -> R<(Tok, usize)> {
        let mut out = String::new();
        let mut i = 3;
        loop {
            let start = i;
            i = self.src.find(i, |b| b == delimiter || b == b'\\')?.ok_or("Unterminated long string literal")?;
            out.push_str(std::str::from_utf8(self.src.slice(start, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?);
            if self.src.byte(i)? == delimiter {
                if self.src.at(i + 1)?.ok_or("Unterminated long string literal")? == delimiter
                    && self.src.at(i + 2)?.ok_or("Unterminated long string literal")? == delimiter {
                    return Ok((Tok::Str { value: out, #[cfg(feature = "rdf-12")] long: true }, i + 3));
                }
                out.push(char::from(delimiter));
                i += 1;
            } else {
                let save = self.src.pos;
                self.src.pos += i;
                let r = escape(&mut self.src, true);
                self.src.pos = save;
                let (c, len) = r?;
                out.push(c);
                i += len;
            }
        }
    }

    /// `LANGTAG`, scanned as `oxttl` does; `@prefix` and `@base` lex as tags too, and the parser tells them apart.
    fn lang_tag(&mut self) -> R<(Tok, usize)> {
        let mut is_last_block_empty = true;
        let mut digits_allowed = false;
        let mut i = 0usize; // index into the bytes after '@'
        loop {
            let c = self.src.at(1 + i)?;
            let Some(c) = c else { return Err("Unterminated language tag".into()) };
            if (digits_allowed && c.is_ascii_alphanumeric()) || (!digits_allowed && c.is_ascii_alphabetic()) {
                is_last_block_empty = false;
            } else if i == 0 {
                return Err("A language code should always start with a letter".into());
            } else if is_last_block_empty {
                if c != b'-' { return self.finish_lang(1, i, i) }
                #[cfg(feature = "rdf-12")]
                {
                    // i is the index of the second dash after the '@'.
                    let from = i + 2;
                    let direction = match (self.src.byte(from)?, self.src.byte(from + 1)?, self.src.byte(from + 2)?) {
                        (b'l', b't', b'r') => BaseDirection::Ltr,
                        (b'r', b't', b'l') => BaseDirection::Rtl,
                        _ => return Err("A literal base direction must be ltr or rtl".into()),
                    };
                    if matches!(self.src.byte(from + 3)?, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-') {
                        return Err("A literal base direction must be ltr or rtl".into());
                    }
                    return Ok((Tok::LangDir(self.validated_lang(1, i)?, direction), from + 3));
                }
                #[cfg(not(feature = "rdf-12"))]
                return Err("Literal base direction are only allowed in RDF 1.2".into());
            } else if c == b'-' {
                is_last_block_empty = true;
                digits_allowed = true;
            } else {
                return self.finish_lang(1, 1 + i, 1 + i);
            }
            i += 1;
        }
    }
    fn finish_lang(&mut self, from: usize, to: usize, consumed: usize) -> R<(Tok, usize)> {
        Ok((Tok::Lang(self.validated_lang(from, to)?), consumed))
    }
    fn validated_lang(&self, from: usize, to: usize) -> R<String> {
        let tag = std::str::from_utf8(self.src.slice(from, to)).map_err(|e| format!("Invalid UTF-8: {e}"))?;
        if !crate::langtag::is_well_formed(tag) { return Err(Fail::Syntax(format!("Invalid language tag {tag:?}"))) }
        Ok(tag.to_owned())
    }

    fn number(&mut self) -> R<(Tok, usize)> {
        let mut i = 0;
        if matches!(self.src.byte(0)?, b'+' | b'-') { i += 1 }
        let digits = |s: &mut Src<I>, mut i: usize| -> R<usize> { let st = i; while s.at(i)?.is_some_and(|b| b.is_ascii_digit()) { i += 1 } Ok(i - st) };
        let before = digits(&mut self.src, i)?;
        i += before;
        let mut after = None;
        if self.src.at(i)? == Some(b'.') {
            i += 1;
            let n = digits(&mut self.src, i)?;
            i += n;
            after = Some(n);
        }
        let text = |s: &Src<I>, n: usize| -> String { String::from_utf8_lossy(s.slice(0, n)).into_owned() };
        if matches!(self.src.at(i)?, Some(b'e' | b'E')) {
            i += 1;
            if matches!(self.src.at(i)?, Some(b'+' | b'-')) { i += 1 }
            let exp = digits(&mut self.src, i)?;
            i += exp;
            if exp == 0 { return Err("A double exponent cannot be empty".into()) }
            if before == 0 && after.unwrap_or(0) == 0 { return Err("A double should not be empty".into()) }
            return Ok((Tok::Double(text(&self.src, i)), i));
        }
        match after {
            Some(0) => { i -= 1; if before == 0 { return Err("An integer should not be empty".into()) } Ok((Tok::Integer(text(&self.src, i)), i)) }
            Some(_) => Ok((Tok::Decimal(text(&self.src, i)), i)),
            None => { if before == 0 { return Err("An integer should not be empty".into()) } Ok((Tok::Integer(text(&self.src, i)), i)) }
        }
    }

    fn pname_or_keyword(&mut self) -> R<(Tok, usize)> {
        let mut i = 0;
        loop {
            match char_at(&mut self.src, i)? {
                Some((':', len)) => { i += len; break }
                Some((c, len)) if i == 0 => {
                    if !pn_chars_base(c) { return Err(Fail::Syntax(format!("'{c}' is not allowed at the beginning of a prefix name"))) }
                    i += len;
                }
                Some((c, len)) if pn_chars(c) || c == '.' => i += len,
                Some(_) | None => {
                    while i > 0 && self.src.slice(0, i)[i - 1] == b'.' { i -= 1 }
                    if i == 0 { return Err("Unexpected byte".into()) }
                    let k = std::str::from_utf8(self.src.slice(0, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?.to_owned();
                    return Ok((Tok::Keyword(k), i));
                }
            }
        }
        let prefix = std::str::from_utf8(self.src.slice(0, i - 1)).map_err(|e| format!("Invalid UTF-8: {e}"))?.to_owned();
        if prefix.ends_with('.') { return Err(Fail::Syntax(format!("'{prefix}' is not a valid prefix: prefixes are not allowed to end with '.'"))) }
        let save = self.src.pos;
        self.src.pos += i;
        let r = self.pn_local();
        self.src.pos = save;
        let ((local, might_be_invalid), n) = r?;
        Ok((Tok::PName { prefix, local, might_be_invalid }, i + n))
    }

    /// `PN_LOCAL` at the current position: the local name (escapes applied), whether it may make an invalid IRI,
    /// and its length. Trailing unescaped dots are not part of it.
    fn pn_local(&mut self) -> R<((String, bool), usize)> {
        let mut out = String::new();
        let mut i = 0;
        let mut might_be_invalid = false;
        let mut trailing_dots = 0usize;
        while let Some((c, len)) = char_at(&mut self.src, i)? {
            if c == '%' {
                let a = self.src.at(i + 1)?.ok_or("Incomplete escape")?;
                let b = self.src.at(i + 2)?.ok_or("Incomplete escape")?;
                if !a.is_ascii_hexdigit() || !b.is_ascii_hexdigit() {
                    return Err(Fail::Syntax(format!("escapes in IRIs should be % followed by two hexadecimal characters, found '%{}{}'", char::from(a), char::from(b))));
                }
                out.push('%'); out.push(char::from(a)); out.push(char::from(b));
                i += 3;
                trailing_dots = 0;
            } else if c == '\\' {
                let a = char::from(self.src.at(i + 1)?.ok_or("Incomplete escape")?);
                if matches!(a, '_' | '~' | '.' | '-' | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=') {
                } else if matches!(a, '/' | '?' | '#' | '@' | '%') {
                    might_be_invalid = true;
                } else {
                    return Err(Fail::Syntax(format!("The character that are allowed to be escaped in IRIs are _~.-!$&'()*+,;=/?#@%, found '{a}'")));
                }
                out.push(a);
                i += 2;
                trailing_dots = 0;
            } else if i == 0 {
                if !(pn_chars_u(c) || c == ':' || c.is_ascii_digit()) { return Ok(((String::new(), false), 0)) }
                might_be_invalid |= pn_chars_base_but_not_valid_iri(c) || c == ':';
                out.push(c);
                i += len;
            } else if pn_chars(c) || c == ':' {
                might_be_invalid |= pn_chars_base_but_not_valid_iri(c) || c == ':';
                out.push(c);
                i += len;
                trailing_dots = 0;
            } else if c == '.' {
                out.push(c);
                i += len;
                trailing_dots += 1;
            } else {
                break;
            }
        }
        for _ in 0..trailing_dots { out.pop(); }
        Ok(((out, might_be_invalid), i - trailing_dots))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectMode {
    Asserted,
    #[cfg(feature = "rdf-12")]
    TripleTerm,
    #[cfg(feature = "rdf-12")]
    Reified,
}

#[cfg(feature = "rdf-12")]
#[derive(Clone, Copy)]
enum ReifiedTarget {
    Subject,
    Object(ObjectMode),
}

#[cfg(feature = "rdf-12")]
enum ReifierTarget {
    Quoted(ReifiedTarget),
    Annotation,
}

/// The Turtle / TriG grammar over the lexer, emitting statements as `oxttl` does.
/// The Turtle / TriG grammar as `oxttl`'s token-driven state machine (`terse.rs`, RDF 1.1 states): grammar state
/// lives on heap stacks, so nesting depth is bounded by memory, not the call stack, and each token is processed on
/// its own. The statements one token produces are released last-first, as `oxttl` returns them (its results are a
/// stack), as soon as that token is processed.
enum State {
    TriGDoc,
    ExpectDot,
    BaseExpectIri,
    #[cfg(feature = "rdf-12")]
    Version { expect_dot: bool },
    #[cfg(feature = "rdf-12")]
    VersionDot,
    PrefixExpectPrefix,
    PrefixExpectIri { name: String },
    TriplesOrGraph,
    WrappedGraphBlankNodePropertyListCurrent,
    SubjectBlankNodePropertyListEnd,
    SubjectBlankNodePropertyListAfter,
    SubjectCollectionBeginning,
    SubjectCollectionPossibleEnd,
    WrappedGraphOrPredicateObjectList { term: NamedOrBlankNode },
    WrappedGraph,
    WrappedGraphPossibleEnd,
    GraphName,
    GraphNameAnonEnd,
    Triples,
    TriplesBlankNodePropertyListCurrent,
    PredicateObjectList,
    PredicateObjectListEnd,
    PredicateObjectListPossibleContinuation,
    ObjectsList,
    AnnotationBlock {
        #[cfg(feature = "rdf-12")]
        reifier: Option<NamedOrBlankNode>,
    },
    #[cfg(feature = "rdf-12")]
    AnnotationEnd,
    Verb,
    Object(ObjectMode),
    #[cfg(feature = "rdf-12")]
    QuotedSubject(ObjectMode),
    #[cfg(feature = "rdf-12")]
    AnonEnd,
    #[cfg(feature = "rdf-12")]
    TripleTermEnd(ObjectMode),
    #[cfg(feature = "rdf-12")]
    ReifiedBeforeReifier(ReifiedTarget),
    #[cfg(feature = "rdf-12")]
    ReifiedEnd { target: ReifiedTarget, triple: Triple, reifier: NamedOrBlankNode },
    #[cfg(feature = "rdf-12")]
    Reifier { target: ReifierTarget, triple: Triple },
    #[cfg(feature = "rdf-12")]
    ReifierAnonEnd { target: ReifierTarget, triple: Triple },
    ObjectBlankNodePropertyListCurrent,
    ObjectBlankNodePropertyListEnd,
    ObjectCollectionBeginning,
    ObjectCollectionPossibleEnd,
    LiteralPossibleSuffix { value: String, mode: ObjectMode },
    LiteralExpectDatatype { value: String, mode: ObjectMode },
}

impl State {
    fn annotation() -> Self {
        Self::AnnotationBlock {
            #[cfg(feature = "rdf-12")]
            reifier: None,
        }
    }
}

fn rdf(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{RDF}{local}")) }
fn xsd(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{XSD}{local}")) }

struct Parser {
    stack: Vec<State>,
    subjects: Vec<NamedOrBlankNode>,
    predicates: Vec<NamedNode>,
    objects: Vec<Term>,
    graph: GraphName,
    prefixes: HashMap<String, Iri>,
    #[cfg(feature = "rdf-12")]
    triple_term_depth: usize,
    with_graph: bool,
    /// The statements of the token being processed, in production order.
    batch: Vec<Quad>,
}

impl Parser {
    fn new(with_graph: bool) -> Self {
        Parser {
            stack: vec![State::TriGDoc],
            subjects: Vec::new(),
            predicates: Vec::new(),
            objects: Vec::new(),
            graph: GraphName::DefaultGraph,
            prefixes: HashMap::new(),
            #[cfg(feature = "rdf-12")]
            triple_term_depth: 0,
            with_graph,
            batch: Vec::new(),
        }
    }
    fn emit(&mut self) {
        let q = Quad::new(self.subjects.last().unwrap().clone(), self.predicates.last().unwrap().clone(), self.objects.last().unwrap().clone(), self.graph.clone());
        self.batch.push(q);
    }
    fn finish_object(&mut self, object: Term, mode: ObjectMode) {
        self.objects.push(object);
        if mode == ObjectMode::Asserted { self.emit() }
    }
    #[cfg(feature = "rdf-12")]
    fn quoted_triple(&mut self) -> Triple {
        Triple::new(self.subjects.pop().unwrap(), self.predicates.pop().unwrap(), self.objects.pop().unwrap())
    }
    #[cfg(feature = "rdf-12")]
    fn begin_quoted(&mut self, mode: ObjectMode) {
        self.stack.push(State::Object(mode));
        self.stack.push(State::Verb);
        self.stack.push(State::QuotedSubject(mode));
    }
    #[cfg(feature = "rdf-12")]
    fn emit_reification(&mut self, reifier: NamedOrBlankNode, triple: Triple) -> R<()> {
        // Reification adds one stored triple-term node around the parsed object.
        if triple_term_depth(triple.object.as_ref()) >= MAX_TRIPLE_TERM_DEPTH {
            return Err(format!("Triple term depth exceeds {MAX_TRIPLE_TERM_DEPTH}").into());
        }
        self.batch.push(Quad::new(reifier, rdf("reifies"), triple, self.graph.clone()));
        Ok(())
    }
    #[cfg(feature = "rdf-12")]
    fn finish_reifier(&mut self, target: ReifierTarget, triple: Triple, reifier: NamedOrBlankNode) -> R<()> {
        match target {
            ReifierTarget::Quoted(target) => self.stack.push(State::ReifiedEnd { target, triple, reifier }),
            ReifierTarget::Annotation => {
                self.emit_reification(reifier.clone(), triple)?;
                self.stack.push(State::AnnotationBlock { reifier: Some(reifier) });
            }
        }
        Ok(())
    }
    fn resolve_pname(&self, prefix: &str, local: &str, might_be_invalid: bool) -> R<NamedNode> {
        let Some(start) = self.prefixes.get(prefix) else { return Err(Fail::Syntax(format!("The prefix {prefix}: has not been declared"))) };
        let iri = format!("{start}{local}");
        if (might_be_invalid || start.path().is_empty()) && Iri::validate(&iri).is_err() {
            return Err(Fail::Syntax(format!("The prefixed name {prefix}:{local} builds IRI {iri} that is invalid")));
        }
        Ok(NamedNode::new_unchecked(iri))
    }
    /// `IRIREF` or prefixed name, if `t` is one.
    fn iri_of(&self, t: &Tok) -> Option<R<NamedNode>> {
        match t {
            Tok::Iri(i) => Some(Ok(NamedNode::new_unchecked(i.clone()))),
            Tok::PName { prefix, local, might_be_invalid } => Some(self.resolve_pname(prefix, local, *might_be_invalid)),
            _ => None,
        }
    }

    /// Processes one token (`oxttl`'s `recognize_next`, with its re-dispatches as a loop). `base` receives a new base
    /// IRI from BASE / @base, which the lexer applies to the following tokens.
    fn recognize(&mut self, token: Tok, base: &mut Option<Iri>) -> R<()> {
        let mut pending = Some(token);
        while let Some(token) = pending.take() {
            let Some(state) = self.stack.pop() else { return Err("Unexpected token after an error".into()) };
            // `again` re-dispatches the same token to the new top state.
            let again = |p: &mut Option<Tok>, t: Tok| *p = Some(t);
            match state {
                State::TriGDoc => {
                    self.graph = GraphName::DefaultGraph;
                    self.stack.push(State::TriGDoc);
                    match &token {
                        #[cfg(feature = "rdf-12")]
                        Tok::Keyword(k) if k.eq_ignore_ascii_case("version") => self.stack.push(State::Version { expect_dot: false }),
                        #[cfg(feature = "rdf-12")]
                        Tok::Lang(l) if l == "version" => self.stack.push(State::Version { expect_dot: true }),
                        Tok::Keyword(k) if k.eq_ignore_ascii_case("base") => self.stack.push(State::BaseExpectIri),
                        Tok::Keyword(k) if k.eq_ignore_ascii_case("prefix") => self.stack.push(State::PrefixExpectPrefix),
                        Tok::Lang(l) if l == "prefix" => { self.stack.push(State::ExpectDot); self.stack.push(State::PrefixExpectPrefix) }
                        Tok::Lang(l) if l == "base" => { self.stack.push(State::ExpectDot); self.stack.push(State::BaseExpectIri) }
                        Tok::Keyword(k) if k.eq_ignore_ascii_case("graph") && self.with_graph => { self.stack.push(State::WrappedGraph); self.stack.push(State::GraphName) }
                        Tok::Punct("{") if self.with_graph => { self.stack.push(State::WrappedGraph); again(&mut pending, token) }
                        _ => { self.stack.push(State::TriplesOrGraph); again(&mut pending, token) }
                    }
                }
                State::ExpectDot => {
                    self.subjects.pop();
                    if token != Tok::Punct(".") { return Err("A dot is expected at the end of statements".into()) }
                }
                #[cfg(feature = "rdf-12")]
                State::Version { expect_dot } => {
                    if !matches!(token, Tok::Str { long: false, .. }) {
                        return Err("VERSION must be followed by a short string literal".into());
                    }
                    if expect_dot { self.stack.push(State::VersionDot); }
                }
                #[cfg(feature = "rdf-12")]
                State::VersionDot => {
                    if token != Tok::Punct(".") { return Err("@version must end with a dot".into()); }
                }
                State::BaseExpectIri => match token {
                    Tok::Iri(i) => *base = Some(Iri::parse(&i).map_err(|e| e.to_string())?),
                    _ => return Err("The BASE keyword should be followed by an IRI".into()),
                },
                State::PrefixExpectPrefix => match token {
                    Tok::PName { prefix, local, .. } if local.is_empty() => self.stack.push(State::PrefixExpectIri { name: prefix }),
                    _ => return Err("The PREFIX keyword should be followed by a prefix like 'ex:'".into()),
                },
                State::PrefixExpectIri { name } => match token {
                    Tok::Iri(i) => { self.prefixes.insert(name, Iri::parse(&i).map_err(|e| e.to_string())?); }
                    _ => return Err("The PREFIX declaration should be followed by a prefix and its value as an IRI".into()),
                },
                State::TriplesOrGraph => match &token {
                    #[cfg(feature = "rdf-12")]
                    Tok::Punct("<<") => {
                        self.stack.push(State::ExpectDot);
                        self.stack.push(State::SubjectBlankNodePropertyListAfter);
                        self.stack.push(State::ReifiedBeforeReifier(ReifiedTarget::Subject));
                        self.begin_quoted(ObjectMode::Reified);
                    }
                    Tok::BNode(l) => self.stack.push(State::WrappedGraphOrPredicateObjectList { term: BlankNode::new_unchecked(l.clone()).into() }),
                    Tok::Punct("[") => self.stack.push(State::WrappedGraphBlankNodePropertyListCurrent),
                    Tok::Punct("(") => { self.stack.push(State::ExpectDot); self.stack.push(State::PredicateObjectList); self.stack.push(State::SubjectCollectionBeginning) }
                    t => match self.iri_of(t) {
                        Some(i) => self.stack.push(State::WrappedGraphOrPredicateObjectList { term: i?.into() }),
                        None => return Err("TOKEN is not a valid subject or graph name".into()),
                    },
                },
                State::WrappedGraphOrPredicateObjectList { term } => {
                    if token == Tok::Punct("{") && self.with_graph {
                        self.graph = match term { NamedOrBlankNode::NamedNode(n) => GraphName::NamedNode(n), NamedOrBlankNode::BlankNode(b) => GraphName::BlankNode(b) };
                        self.stack.push(State::WrappedGraph);
                    } else {
                        self.subjects.push(term);
                        self.stack.push(State::ExpectDot);
                        self.stack.push(State::PredicateObjectList);
                    }
                    again(&mut pending, token);
                }
                State::WrappedGraphBlankNodePropertyListCurrent => {
                    if token == Tok::Punct("]") {
                        self.stack.push(State::WrappedGraphOrPredicateObjectList { term: BlankNode::default().into() });
                    } else {
                        self.subjects.push(BlankNode::default().into());
                        self.stack.push(State::ExpectDot);
                        self.stack.push(State::SubjectBlankNodePropertyListEnd);
                        self.stack.push(State::PredicateObjectList);
                        again(&mut pending, token);
                    }
                }
                State::SubjectBlankNodePropertyListEnd => {
                    if token != Tok::Punct("]") { return Err("blank node property lists should end with a ']'".into()) }
                    self.stack.push(State::SubjectBlankNodePropertyListAfter);
                }
                State::SubjectBlankNodePropertyListAfter => {
                    if !matches!(token, Tok::Punct("." | "}")) { self.stack.push(State::PredicateObjectList) }
                    again(&mut pending, token);
                }
                State::SubjectCollectionBeginning => {
                    if token == Tok::Punct(")") {
                        self.subjects.push(rdf("nil").into());
                    } else {
                        let root = BlankNode::default();
                        self.subjects.push(root.clone().into());
                        self.subjects.push(root.into());
                        self.predicates.push(rdf("first"));
                        self.stack.push(State::SubjectCollectionPossibleEnd);
                        self.stack.push(State::Object(ObjectMode::Asserted));
                        again(&mut pending, token);
                    }
                }
                // `oxttl` continues a subject collection with the object-collection state too.
                State::SubjectCollectionPossibleEnd | State::ObjectCollectionPossibleEnd => {
                    let old = self.subjects.pop().unwrap();
                    self.objects.pop();
                    if token == Tok::Punct(")") {
                        self.predicates.pop();
                        self.batch.push(Quad::new(old, rdf("rest"), rdf("nil"), self.graph.clone()));
                    } else {
                        let new = BlankNode::default();
                        self.batch.push(Quad::new(old, rdf("rest"), new.clone(), self.graph.clone()));
                        self.subjects.push(new.into());
                        self.stack.push(State::ObjectCollectionPossibleEnd);
                        self.stack.push(State::Object(ObjectMode::Asserted));
                        again(&mut pending, token);
                    }
                }
                State::WrappedGraph => {
                    if token != Tok::Punct("{") { return Err("The GRAPH keyword should be followed by a graph name and a value in '{'".into()) }
                    self.stack.push(State::WrappedGraphPossibleEnd);
                    self.stack.push(State::Triples);
                }
                State::WrappedGraphPossibleEnd => {
                    self.subjects.pop();
                    match token {
                        Tok::Punct("}") => {}
                        Tok::Punct(".") => { self.stack.push(State::WrappedGraphPossibleEnd); self.stack.push(State::Triples) }
                        _ => return Err("A '}' or a '.' is expected at the end of a graph block".into()),
                    }
                }
                State::Triples => match &token {
                    #[cfg(feature = "rdf-12")]
                    Tok::Punct("<<") => {
                        self.stack.push(State::SubjectBlankNodePropertyListAfter);
                        self.stack.push(State::ReifiedBeforeReifier(ReifiedTarget::Subject));
                        self.begin_quoted(ObjectMode::Reified);
                    }
                    Tok::Punct("}") => again(&mut pending, token),
                    Tok::Punct("[") => { self.subjects.push(BlankNode::default().into()); self.stack.push(State::TriplesBlankNodePropertyListCurrent) }
                    Tok::BNode(l) => { self.subjects.push(BlankNode::new_unchecked(l.clone()).into()); self.stack.push(State::PredicateObjectList) }
                    Tok::Punct("(") => { self.stack.push(State::PredicateObjectList); self.stack.push(State::SubjectCollectionBeginning) }
                    t => match self.iri_of(t) {
                        Some(i) => { self.subjects.push(i?.into()); self.stack.push(State::PredicateObjectList) }
                        None => return Err("TOKEN is not a valid RDF subject".into()),
                    },
                },
                State::TriplesBlankNodePropertyListCurrent => {
                    if token == Tok::Punct("]") {
                        self.stack.push(State::PredicateObjectList);
                    } else {
                        self.stack.push(State::SubjectBlankNodePropertyListEnd);
                        self.stack.push(State::PredicateObjectList);
                        again(&mut pending, token);
                    }
                }
                State::GraphName => match &token {
                    Tok::BNode(l) => self.graph = GraphName::BlankNode(BlankNode::new_unchecked(l.clone())),
                    Tok::Punct("[") => self.stack.push(State::GraphNameAnonEnd),
                    t => match self.iri_of(t) {
                        Some(i) => self.graph = GraphName::NamedNode(i?),
                        None => return Err("TOKEN is not a valid graph name".into()),
                    },
                },
                State::GraphNameAnonEnd => {
                    if token != Tok::Punct("]") { return Err("Anonymous blank node with a property list are not allowed as graph name".into()) }
                    self.graph = GraphName::BlankNode(BlankNode::default());
                }
                State::PredicateObjectList => {
                    self.stack.push(State::PredicateObjectListEnd);
                    self.stack.push(State::ObjectsList);
                    self.stack.push(State::Verb);
                    again(&mut pending, token);
                }
                State::PredicateObjectListEnd => {
                    self.predicates.pop();
                    if token == Tok::Punct(";") { self.stack.push(State::PredicateObjectListPossibleContinuation) } else { again(&mut pending, token) }
                }
                State::PredicateObjectListPossibleContinuation => {
                    if token == Tok::Punct(";") {
                        self.stack.push(State::PredicateObjectListPossibleContinuation);
                    } else if matches!(token, Tok::Punct("." | "}" | "]" | "|}")) {
                        again(&mut pending, token);
                    } else {
                        self.stack.push(State::PredicateObjectListEnd);
                        self.stack.push(State::ObjectsList);
                        self.stack.push(State::Verb);
                        again(&mut pending, token);
                    }
                }
                State::ObjectsList => {
                    self.stack.push(State::annotation());
                    self.stack.push(State::Object(ObjectMode::Asserted));
                    again(&mut pending, token);
                }
                State::AnnotationBlock {
                    #[cfg(feature = "rdf-12")]
                    reifier,
                } => {
                    #[cfg(feature = "rdf-12")]
                    match token {
                        Tok::Punct("~") => {
                            let triple = Triple::new(self.subjects.last().unwrap().clone(), self.predicates.last().unwrap().clone(), self.objects.last().unwrap().clone());
                            self.stack.push(State::Reifier { target: ReifierTarget::Annotation, triple });
                            continue;
                        }
                        Tok::Punct("{|") => {
                            let reifier = if let Some(reifier) = reifier {
                                reifier
                            } else {
                                let reifier = NamedOrBlankNode::from(BlankNode::default());
                                let triple = Triple::new(self.subjects.last().unwrap().clone(), self.predicates.last().unwrap().clone(), self.objects.last().unwrap().clone());
                                self.emit_reification(reifier.clone(), triple)?;
                                reifier
                            };
                            self.subjects.push(reifier);
                            self.stack.push(State::AnnotationEnd);
                            self.stack.push(State::PredicateObjectList);
                            continue;
                        }
                        _ => {}
                    }
                    self.objects.pop();
                    if token == Tok::Punct(",") {
                        self.stack.push(State::annotation());
                        self.stack.push(State::Object(ObjectMode::Asserted));
                    } else {
                        again(&mut pending, token);
                    }
                }
                #[cfg(feature = "rdf-12")]
                State::AnnotationEnd => {
                    if token != Tok::Punct("|}") { return Err("An annotation must end with |}".into()); }
                    self.subjects.pop();
                    self.stack.push(State::annotation());
                }
                State::Verb => {
                    if token == Tok::Keyword("a".into()) {
                        self.predicates.push(rdf("type"));
                    } else {
                        match self.iri_of(&token) { Some(i) => self.predicates.push(i?), None => return Err("TOKEN is not a valid predicate".into()) }
                    }
                }
                State::Object(mode) => match token {
                    Tok::BNode(l) => self.finish_object(BlankNode::new_unchecked(l).into(), mode),
                    Tok::Punct("[") if mode == ObjectMode::Asserted => self.stack.push(State::ObjectBlankNodePropertyListCurrent),
                    Tok::Punct("(") if mode == ObjectMode::Asserted => self.stack.push(State::ObjectCollectionBeginning),
                    #[cfg(feature = "rdf-12")]
                    Tok::Punct("[") => {
                        self.finish_object(BlankNode::default().into(), mode);
                        self.stack.push(State::AnonEnd);
                    }
                    #[cfg(feature = "rdf-12")]
                    Tok::Punct("<<(") => {
                        if self.triple_term_depth == MAX_TRIPLE_TERM_DEPTH {
                            return Err(format!("Triple term depth exceeds {MAX_TRIPLE_TERM_DEPTH}").into());
                        }
                        self.triple_term_depth += 1;
                        self.stack.push(State::TripleTermEnd(mode));
                        self.begin_quoted(ObjectMode::TripleTerm);
                    }
                    #[cfg(feature = "rdf-12")]
                    Tok::Punct("<<") if mode != ObjectMode::TripleTerm => {
                        self.stack.push(State::ReifiedBeforeReifier(ReifiedTarget::Object(mode)));
                        self.begin_quoted(ObjectMode::Reified);
                    }
                    Tok::Str { value, .. } => self.stack.push(State::LiteralPossibleSuffix { value, mode }),
                    Tok::Integer(v) => self.finish_object(Literal::new_typed_literal(v, xsd("integer")).into(), mode),
                    Tok::Decimal(v) => self.finish_object(Literal::new_typed_literal(v, xsd("decimal")).into(), mode),
                    Tok::Double(v) => self.finish_object(Literal::new_typed_literal(v, xsd("double")).into(), mode),
                    Tok::Keyword(k) if k == "true" || k == "false" => self.finish_object(Literal::new_typed_literal(k, xsd("boolean")).into(), mode),
                    t => match self.iri_of(&t) {
                        Some(i) => self.finish_object(i?.into(), mode),
                        None => return Err("TOKEN is not a valid RDF object".into()),
                    },
                },
                #[cfg(feature = "rdf-12")]
                State::QuotedSubject(mode) => match token {
                    Tok::BNode(label) => self.subjects.push(BlankNode::new_unchecked(label).into()),
                    Tok::Punct("[") => {
                        self.subjects.push(BlankNode::default().into());
                        self.stack.push(State::AnonEnd);
                    }
                    Tok::Punct("<<") if mode == ObjectMode::Reified => {
                        self.stack.push(State::ReifiedBeforeReifier(ReifiedTarget::Subject));
                        self.begin_quoted(ObjectMode::Reified);
                    }
                    t => match self.iri_of(&t) {
                        Some(iri) => self.subjects.push(iri?.into()),
                        None => return Err("A quoted triple's subject must denote an IRI or blank node".into()),
                    },
                },
                #[cfg(feature = "rdf-12")]
                State::AnonEnd => {
                    if token != Tok::Punct("]") { return Err("Only an empty blank node property list is allowed inside a quoted triple".into()); }
                }
                #[cfg(feature = "rdf-12")]
                State::TripleTermEnd(mode) => {
                    if token != Tok::Punct(")>>") { return Err("A triple term must end with )>>".into()); }
                    self.triple_term_depth -= 1;
                    let triple = self.quoted_triple();
                    self.finish_object(triple.into(), mode);
                }
                #[cfg(feature = "rdf-12")]
                State::ReifiedBeforeReifier(target) => {
                    let triple = self.quoted_triple();
                    if token == Tok::Punct("~") {
                        self.stack.push(State::Reifier { target: ReifierTarget::Quoted(target), triple });
                    } else {
                        self.finish_reifier(ReifierTarget::Quoted(target), triple, BlankNode::default().into())?;
                        again(&mut pending, token);
                    }
                }
                #[cfg(feature = "rdf-12")]
                State::ReifiedEnd { target, triple, reifier } => {
                    if token != Tok::Punct(">>") { return Err("A reified triple must end with >>".into()); }
                    self.emit_reification(reifier.clone(), triple)?;
                    match target {
                        ReifiedTarget::Subject => self.subjects.push(reifier),
                        ReifiedTarget::Object(mode) => self.finish_object(reifier.into(), mode),
                    }
                }
                #[cfg(feature = "rdf-12")]
                State::Reifier { target, triple } => match token {
                    Tok::Punct("[") => self.stack.push(State::ReifierAnonEnd { target, triple }),
                    Tok::BNode(label) => self.finish_reifier(target, triple, BlankNode::new_unchecked(label).into())?,
                    t => {
                        if let Some(iri) = self.iri_of(&t) {
                            self.finish_reifier(target, triple, iri?.into())?;
                        } else {
                            self.finish_reifier(target, triple, BlankNode::default().into())?;
                            again(&mut pending, t);
                        }
                    }
                },
                #[cfg(feature = "rdf-12")]
                State::ReifierAnonEnd { target, triple } => {
                    if token != Tok::Punct("]") { return Err("A reifier accepts an empty blank node property list".into()); }
                    self.finish_reifier(target, triple, BlankNode::default().into())?;
                }
                State::ObjectBlankNodePropertyListCurrent => {
                    if token == Tok::Punct("]") {
                        self.objects.push(BlankNode::default().into());
                        self.emit();
                    } else {
                        self.subjects.push(BlankNode::default().into());
                        self.stack.push(State::ObjectBlankNodePropertyListEnd);
                        self.stack.push(State::PredicateObjectList);
                        again(&mut pending, token);
                    }
                }
                State::ObjectBlankNodePropertyListEnd => {
                    if token != Tok::Punct("]") { return Err("blank node property lists should end with a ']'".into()) }
                    let b = self.subjects.pop().unwrap();
                    self.objects.push(b.into());
                    self.emit();
                }
                State::ObjectCollectionBeginning => {
                    if token == Tok::Punct(")") {
                        self.objects.push(rdf("nil").into());
                        self.emit();
                    } else {
                        let root = BlankNode::default();
                        self.objects.push(root.clone().into());
                        self.emit();
                        self.subjects.push(root.into());
                        self.predicates.push(rdf("first"));
                        self.stack.push(State::ObjectCollectionPossibleEnd);
                        self.stack.push(State::Object(ObjectMode::Asserted));
                        again(&mut pending, token);
                    }
                }
                State::LiteralPossibleSuffix { value, mode } => match token {
                    Tok::Lang(l) => self.finish_object(Literal::new_language_tagged_literal_unchecked(value, l.to_ascii_lowercase()).into(), mode),
                    #[cfg(feature = "rdf-12")]
                    Tok::LangDir(l, direction) => self.finish_object(Literal::new_directional_language_tagged_literal_unchecked(value, l.to_ascii_lowercase(), direction).into(), mode),
                    Tok::Punct("^^") => self.stack.push(State::LiteralExpectDatatype { value, mode }),
                    t => { self.finish_object(Literal::new_simple_literal(value).into(), mode); again(&mut pending, t) }
                },
                State::LiteralExpectDatatype { value, mode } => match self.iri_of(&token) {
                    Some(d) => {
                        let d = d?;
                        if d.as_str() == format!("{RDF}langString") { return Err("The datatype of a literal without a language tag must not be rdf:langString".into()) }
                        #[cfg(feature = "rdf-12")]
                        if d.as_str() == triplelight_rdf::vocab::rdf::DIR_LANG_STRING.as_str() {
                            return Err("The datatype of a literal without a base direction must not be rdf:dirLangString".into());
                        }
                        self.finish_object(Literal::new_typed_literal(value, d).into(), mode);
                    }
                    None => return Err("Expecting a datatype IRI after ^^, found TOKEN".into()),
                },
            }
        }
        Ok(())
    }

    /// The end of the input: only a complete document (the state stack back to the document level) is accepted.
    fn end(&self) -> R<()> {
        match self.stack.as_slice() { [] | [State::TriGDoc] => Ok(()), _ => Err("Unexpected end of the document".into()) }
    }
}

/// Streams the statements of a Turtle (`with_graph = false`) or TriG document, token by token. On an error the
/// statements of earlier tokens have been yielded; the error ends the stream.
pub struct TurtleReader<I> { lex: Lexer<I>, parser: Parser, out: VecDeque<Quad>, done: bool }

impl<I: BufRead> TurtleReader<I> {
    fn new(input: I, with_graph: bool, capped: bool) -> Self {
        let src = Src { input, buf: Vec::new(), pos: 0, eof: false, capped, token_start: 0 };
        TurtleReader { lex: Lexer { src, base: None, peeked: None }, parser: Parser::new(with_graph), out: VecDeque::new(), done: false }
    }
    fn step(&mut self) -> R<bool> {
        let token = self.lex.next()?;
        if token == Tok::Eof { self.parser.end()?; return Ok(false) }
        self.parser.recognize(token, &mut self.lex.base)?;
        // Released last-first, as `oxttl` pops its results.
        while let Some(q) = self.parser.batch.pop() { self.out.push_back(q) }
        Ok(true)
    }
}

impl<I: BufRead> Iterator for TurtleReader<I> {
    type Item = Result<Quad, ReadError>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(q) = self.out.pop_front() { return Some(Ok(q)) }
            if self.done { return None }
            match self.step() {
                Ok(true) => {}
                Ok(false) => self.done = true,
                Err(e) => {
                    self.done = true;
                    self.parser.batch.clear();
                    return Some(Err(match e {
                        Fail::Syntax(m) => ReadError::Syntax(m),
                        Fail::Io(e) => ReadError::Io(e),
                        Fail::TooLarge => ReadError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, format!("token larger than {MAX_TOKEN} bytes"))),
                    }));
                }
            }
        }
    }
}

/// The triples of a streamed Turtle document (16 MiB token limit, as `oxttl`'s readers).
pub fn read_turtle<I: BufRead>(input: I) -> impl Iterator<Item = Result<crate::rdf::Triple, ReadError>> {
    TurtleReader::new(input, false, true).map(|q| q.map(|q| crate::rdf::Triple::new(q.subject, q.predicate, q.object)))
}
/// The triples of a Turtle document in memory (no token limit, as `oxttl`'s `for_slice`).
pub fn read_turtle_slice(data: &[u8]) -> impl Iterator<Item = Result<crate::rdf::Triple, ReadError>> + '_ {
    TurtleReader::new(data, false, false).map(|q| q.map(|q| crate::rdf::Triple::new(q.subject, q.predicate, q.object)))
}
/// The quads of a streamed TriG document.
pub fn read_trig<I: BufRead>(input: I) -> impl Iterator<Item = Result<Quad, ReadError>> { TurtleReader::new(input, true, true) }

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "rdf-12")]
    #[test]
    fn rdf12_annotations_keep_scope_and_allocate_fresh_reifiers() {
        let input = b"PREFIX : <urn:test:>
            :g { :s :p :o ~ :r {| :a :v |} {| :b :v |} .
                 << :s :p :n ~ :r2 >> . }
            :s :p :default .";
        let quads = read_trig(&input[..]).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(quads.len(), 7);
        assert_eq!(quads.iter().filter(|q| q.graph_name.is_default_graph()).count(), 1);
        assert!(quads.iter().filter(|q| !q.graph_name.is_default_graph()).all(|q| {
            q.graph_name.to_string() == "<urn:test:g>"
        }));
        let find = |predicate: &str| quads.iter().find(|q| q.predicate.as_str() == predicate).unwrap();
        let a = find("urn:test:a");
        let b = find("urn:test:b");
        assert_eq!(a.subject.to_string(), "<urn:test:r>");
        assert!(matches!(b.subject, NamedOrBlankNode::BlankNode(_)));
        assert!(quads.iter().any(|q| q.subject == b.subject && q.predicate.as_str() == format!("{RDF}reifies")));
        assert!(!quads.iter().any(|q| q.predicate.as_str() == "urn:test:p" && q.object.to_string() == "<urn:test:n>"));
    }

    #[cfg(feature = "rdf-12")]
    #[test]
    fn rdf12_quoted_grammar_and_version_boundaries() {
        for bad in [
            r#"@Version "1.2" . <urn:s> <urn:p> <urn:o> ."#,
            r#"@version "1.2" <urn:s> <urn:p> <urn:o> ."#,
            r#"VERSION """1.2""" <urn:s> <urn:p> <urn:o> ."#,
            r#"VERSION "1.2" . <urn:s> <urn:p> <urn:o> ."#,
            r#"<<( <urn:s> <urn:p> <urn:o> )>> <urn:p> <urn:o> ."#,
            r#"<urn:s> <urn:p> <<( <urn:a> <urn:p> << <urn:s> <urn:p> <urn:o> >> )>> ."#,
            r#"<urn:s> <urn:p> <<( [ <urn:p> <urn:o> ] <urn:p> <urn:o> )>> ."#,
            r#"<urn:s> <urn:p> "x"@en--Long ."#,
            r#"<urn:s> <urn:p> <urn:o> {| |} ."#,
        ] {
            assert!(read_turtle_slice(bad.as_bytes()).any(|row| row.is_err()), "{bad}");
        }
    }

    #[cfg(feature = "rdf-12")]
    #[test]
    fn rdf12_terse_depth_matches_dictionary() {
        let object = |depth| format!("{}<urn:o>{}", "<<( <urn:s> <urn:p> ".repeat(depth), " )>>".repeat(depth));
        let documents = [
            (MAX_TRIPLE_TERM_DEPTH, "<urn:s> <urn:p> ", " ."),
            (MAX_TRIPLE_TERM_DEPTH - 1, "<< <urn:s> <urn:p> ", " ~ <urn:r> >> ."),
            (MAX_TRIPLE_TERM_DEPTH - 1, "<urn:s> <urn:p> ", " {| <urn:q> <urn:o> |} ."),
            (MAX_TRIPLE_TERM_DEPTH - 1, "<urn:s> <urn:p> ", " ~ <urn:r> ."),
        ];
        for (limit, prefix, suffix) in documents {
            for depth in [limit, limit + 1] {
                let text = format!("{prefix}{}{suffix}", object(depth));
                let dataset = format!("<urn:g> {{ {text} }}");
                let turtle = read_turtle_slice(text.as_bytes())
                    .map(|row| row.map(|triple| triple.in_graph(GraphName::DefaultGraph)))
                    .collect::<Result<Vec<_>, _>>();
                let trig = read_trig(dataset.as_bytes()).collect::<Result<Vec<_>, _>>();
                for result in [turtle, trig] {
                    if depth == limit {
                        let rows = result.unwrap();
                        let depth = rows.iter().map(|q| triple_term_depth(q.object.as_ref())).max();
                        assert_eq!(depth, Some(MAX_TRIPLE_TERM_DEPTH));
                        assert!(rows.iter().all(|q| q.object.to_string().parse::<Term>().is_ok_and(|term| term == q.object)));
                    } else {
                        assert!(matches!(result, Err(ReadError::Syntax(message)) if message.contains("Triple term depth")));
                    }
                }
            }
        }
    }

    /// Statements are yielded as their tokens arrive: the first quads of a graph block come out before the rest of
    /// the block can be read (the reader fails after the first part).
    #[test]
    fn graph_blocks_stream() {
        struct Part { data: &'static [u8], served: bool }
        impl std::io::Read for Part { fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> { unreachable!() } }
        impl BufRead for Part {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                if self.served && self.data.is_empty() { return Err(std::io::Error::other("the rest of the block is not readable yet")) }
                self.served = true;
                Ok(self.data)
            }
            fn consume(&mut self, n: usize) { self.data = &self.data[n..] }
        }
        let input = Part { data: b"@prefix : <http://ex/> . GRAPH :g { :a :b :c . :a :b :d , :e . ", served: false };
        let got: Vec<_> = read_trig(input).collect();
        assert!(got.len() >= 3, "{got:?}");
        assert!(got[..got.len() - 1].iter().all(Result::is_ok));
        assert!(matches!(got.last(), Some(Err(ReadError::Io(_)))));
    }
    /// More than 16 MiB of whitespace and comments between two tokens is skipped, not counted as one token, in a
    /// streamed document, and the buffer stays small while skipping.
    #[test]
    fn long_whitespace_runs_stream() {
        let gap = format!("{}# comment\n{}", " ".repeat(9 << 20), "\t\n".repeat(5 << 20));
        let doc = format!("<http://a> <http://b> {gap} <http://c> .");
        let r = std::io::BufReader::with_capacity(1 << 16, doc.as_bytes());
        let mut reader = TurtleReader::new(r, false, true);
        let q = reader.next().expect("a statement").expect("parsed");
        assert_eq!(q.object.to_string(), "<http://c>");
        assert!(reader.lex.src.buf.len() < 4 << 20, "buffer grew to {}", reader.lex.src.buf.len());
        assert!(reader.next().is_none());
    }
    /// Deep nesting is limited by memory, not the call stack: 100,000 nested property lists and collections parse,
    /// on a thread with a small (512 KiB) stack.
    #[test]
    fn deep_nesting_does_not_use_the_call_stack() {
        let depth = 100_000;
        let lists = format!("@prefix : <http://ex/> . :s :p {} :o {} .", "[ :p ".repeat(depth), "]".repeat(depth));
        let collections = format!("@prefix : <http://ex/> . :s :p {} :o {} .", "( ".repeat(depth), ")".repeat(depth));
        let handle = std::thread::Builder::new().stack_size(512 * 1024).spawn(move || {
            let a = read_turtle_slice(lists.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap().len();
            let b = read_turtle_slice(collections.as_bytes()).collect::<Result<Vec<_>, _>>().unwrap().len();
            (a, b)
        }).unwrap();
        let (a, b) = handle.join().expect("no stack overflow");
        assert_eq!(a, depth + 1);
        assert_eq!(b, 2 * depth + 1);
    }
}
