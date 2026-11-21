//! Turtle and TriG (RDF 1.1) parsing (task #45, step 3). It accepts and rejects exactly what `oxttl` 0.2's strict
//! parsers do and emits the same statements in the same order; blank nodes for `[]` and collections are fresh, as
//! there. Input is read through a refillable buffer: a statement is parsed as soon as its tokens are available, and a
//! streaming reader refuses a token longer than `oxttl`'s 16 MiB buffer, as it did (slices have no limit).
use crate::ntriples::MAX_TOKEN;
use crate::rdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, ReadError, Term};
use std::collections::{HashMap, VecDeque};
use std::io::BufRead;
use triplelight_iri::Iri;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Iri(String),
    PName { prefix: String, local: String, might_be_invalid: bool },
    BNode(String),
    Lang(String),
    Str(String),
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
            let n = chunk.len();
            self.buf.extend_from_slice(chunk);
            self.input.consume(n);
        }
        Ok(Some(self.buf[self.pos + i]))
    }
    fn byte(&mut self, i: usize) -> R<u8> { Ok(self.at(i)?.unwrap_or(0)) }
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

fn pn_chars_base(c: char) -> bool {
    matches!(c, 'A'..='Z' | 'a'..='z' | '\u{00C0}'..='\u{00D6}' | '\u{00D8}'..='\u{00F6}' | '\u{00F8}'..='\u{02FF}'
        | '\u{0370}'..='\u{037D}' | '\u{037F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}
fn pn_chars_u(c: char) -> bool { pn_chars_base(c) || c == '_' }
fn pn_chars(c: char) -> bool { pn_chars_u(c) || matches!(c, '-' | '0'..='9' | '\u{00B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}') }
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
    fn skip_ws(&mut self) -> R<()> {
        loop {
            match self.src.at(0)? {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.src.pos += 1,
                Some(b'#') => {
                    // Comment bytes are not decoded.
                    loop { match self.src.at(0)? { None => break, Some(b'\n' | b'\r') => break, Some(_) => self.src.pos += 1 } }
                }
                _ => return Ok(()),
            }
        }
    }
    fn peek(&mut self) -> R<&Tok> {
        if self.peeked.is_none() { let t = self.lex()?; self.peeked = Some(t) }
        Ok(self.peeked.as_ref().unwrap())
    }
    fn next(&mut self) -> R<Tok> { match self.peeked.take() { Some(t) => Ok(t), None => self.lex() } }

    fn lex(&mut self) -> R<Tok> {
        self.skip_ws()?;
        self.src.compact();
        let Some(b) = self.src.at(0)? else { return Ok(Tok::Eof) };
        let (tok, len) = match b {
            b'<' => match self.src.byte(1)? {
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
            match self.src.at(i)?.ok_or("Unterminated IRI")? {
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
                b => { bytes.push(b); i += 1 }
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
            loop {
                match self.src.at(i)?.ok_or("Unterminated string literal")? {
                    b if b == delimiter || b == b'\\' || b == b'\n' || b == b'\r' => break,
                    _ => i += 1,
                }
            }
            out.push_str(std::str::from_utf8(self.src.slice(start, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?);
            match self.src.byte(i)? {
                b if b == delimiter => return Ok((Tok::Str(out), i + 1)),
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
            loop {
                match self.src.at(i)?.ok_or("Unterminated long string literal")? {
                    b if b == delimiter || b == b'\\' => break,
                    _ => i += 1,
                }
            }
            out.push_str(std::str::from_utf8(self.src.slice(start, i)).map_err(|e| format!("Invalid UTF-8: {e}"))?);
            if self.src.byte(i)? == delimiter {
                if self.src.at(i + 1)?.ok_or("Unterminated long string literal")? == delimiter
                    && self.src.at(i + 2)?.ok_or("Unterminated long string literal")? == delimiter {
                    return Ok((Tok::Str(out), i + 3));
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
        let tag = std::str::from_utf8(self.src.slice(from, to)).map_err(|e| format!("Invalid UTF-8: {e}"))?.to_owned();
        if !crate::langtag::is_well_formed(&tag) { return Err(Fail::Syntax(format!("Invalid language tag {tag:?}"))) }
        Ok((Tok::Lang(tag), consumed))
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
        loop {
            let Some((c, len)) = char_at(&mut self.src, i)? else { break };
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

/// The Turtle / TriG grammar over the lexer, emitting statements as `oxttl` does.
struct Parser<I> { lex: Lexer<I>, prefixes: HashMap<String, Iri>, with_graph: bool, graph: GraphName, out: VecDeque<Quad> }

fn rdf(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{RDF}{local}")) }

impl<I: BufRead> Parser<I> {
    fn emit(&mut self, s: &NamedOrBlankNode, p: &NamedNode, o: Term) { self.out.push_back(Quad::new(s.clone(), p.clone(), o, self.graph.clone())) }

    fn resolve_pname(&self, prefix: &str, local: &str, might_be_invalid: bool) -> R<NamedNode> {
        let Some(start) = self.prefixes.get(prefix) else { return Err(Fail::Syntax(format!("The prefix {prefix}: has not been declared"))) };
        let iri = format!("{start}{local}");
        if (might_be_invalid || start.path().is_empty()) && Iri::validate(&iri).is_err() {
            return Err(Fail::Syntax(format!("The prefixed name {prefix}:{local} builds IRI {iri} that is invalid")));
        }
        Ok(NamedNode::new_unchecked(iri))
    }
    /// An IRI token (`IRIREF` or prefixed name), if `t` is one.
    fn iri_of(&self, t: &Tok) -> Option<R<NamedNode>> {
        match t {
            Tok::Iri(i) => Some(Ok(NamedNode::new_unchecked(i.clone()))),
            Tok::PName { prefix, local, might_be_invalid } => Some(self.resolve_pname(prefix, local, *might_be_invalid)),
            _ => None,
        }
    }
    fn expect(&mut self, p: &'static str, message: &str) -> R<()> {
        if self.lex.next()? == Tok::Punct(p) { Ok(()) } else { Err(Fail::Syntax(message.to_owned())) }
    }

    /// One top-level statement (directive, triples or graph block); `false` at the end of the document.
    fn statement(&mut self) -> R<bool> {
        self.graph = GraphName::DefaultGraph;
        let t = self.lex.next()?;
        match &t {
            Tok::Eof => return Ok(false),
            Tok::Keyword(k) if k.eq_ignore_ascii_case("base") => self.base()?,
            Tok::Keyword(k) if k.eq_ignore_ascii_case("prefix") => self.prefix()?,
            Tok::Lang(l) if l == "prefix" => { self.prefix()?; self.expect(".", "A dot is expected at the end of statements")? }
            Tok::Lang(l) if l == "base" => { self.base()?; self.expect(".", "A dot is expected at the end of statements")? }
            Tok::Keyword(k) if k.eq_ignore_ascii_case("graph") && self.with_graph => { self.graph_name()?; self.wrapped_graph()? }
            Tok::Punct("{") if self.with_graph => { self.lex.peeked = Some(t); self.wrapped_graph()? }
            _ => self.triples_or_graph(t)?,
        }
        Ok(true)
    }
    fn base(&mut self) -> R<()> {
        match self.lex.next()? {
            Tok::Iri(i) => { self.lex.base = Some(Iri::parse(&i).map_err(|e| e.to_string())?); Ok(()) }
            _ => Err("The BASE keyword should be followed by an IRI".into()),
        }
    }
    fn prefix(&mut self) -> R<()> {
        let name = match self.lex.next()? {
            Tok::PName { prefix, local, .. } if local.is_empty() => prefix,
            _ => return Err("The PREFIX keyword should be followed by a prefix like 'ex:'".into()),
        };
        match self.lex.next()? {
            Tok::Iri(i) => { self.prefixes.insert(name, Iri::parse(&i).map_err(|e| e.to_string())?); Ok(()) }
            _ => Err("The PREFIX declaration should be followed by a prefix and its value as an IRI".into()),
        }
    }

    fn triples_or_graph(&mut self, t: Tok) -> R<()> {
        let term: NamedOrBlankNode = match &t {
            Tok::BNode(l) => BlankNode::new_unchecked(l.clone()).into(),
            Tok::Punct("[") => {
                if self.lex.peek()? == &Tok::Punct("]") {
                    self.lex.next()?;
                    BlankNode::default().into()
                } else {
                    let s: NamedOrBlankNode = BlankNode::default().into();
                    self.predicate_object_list(&s)?;
                    if self.lex.peek()? == &Tok::Punct("]") { self.lex.next()?; } else { return Err("blank node property lists should end with a ']'".into()) }
                    if !matches!(self.lex.peek()?, Tok::Punct("." | "}")) { self.predicate_object_list(&s)? }
                    return self.expect(".", "A dot is expected at the end of statements");
                }
            }
            Tok::Punct("(") => {
                let s = self.collection_subject()?;
                self.predicate_object_list(&s)?;
                return self.expect(".", "A dot is expected at the end of statements");
            }
            t => match self.iri_of(t) { Some(i) => i?.into(), None => return Err("TOKEN is not a valid subject or graph name".into()) },
        };
        if self.with_graph && self.lex.peek()? == &Tok::Punct("{") {
            self.graph = match term { NamedOrBlankNode::NamedNode(n) => GraphName::NamedNode(n), NamedOrBlankNode::BlankNode(b) => GraphName::BlankNode(b) };
            return self.wrapped_graph();
        }
        self.predicate_object_list(&term)?;
        self.expect(".", "A dot is expected at the end of statements")
    }

    fn graph_name(&mut self) -> R<()> {
        let t = self.lex.next()?;
        self.graph = match &t {
            Tok::BNode(l) => GraphName::BlankNode(BlankNode::new_unchecked(l.clone())),
            Tok::Punct("[") => {
                if self.lex.next()? != Tok::Punct("]") { return Err("Anonymous blank node with a property list are not allowed as graph name".into()) }
                GraphName::BlankNode(BlankNode::default())
            }
            t => match self.iri_of(t) { Some(i) => GraphName::NamedNode(i?), None => return Err("TOKEN is not a valid graph name".into()) },
        };
        Ok(())
    }

    fn wrapped_graph(&mut self) -> R<()> {
        if self.lex.next()? != Tok::Punct("{") { return Err("The GRAPH keyword should be followed by a graph name and a value in '{'".into()) }
        loop {
            self.triples()?;
            match self.lex.next()? {
                Tok::Punct("}") => return Ok(()),
                Tok::Punct(".") => continue,
                _ => return Err("A '}' or a '.' is expected at the end of a graph block".into()),
            }
        }
    }

    /// `triples` inside a graph block; returns at once before a `}`.
    fn triples(&mut self) -> R<()> {
        let t = self.lex.next()?;
        match &t {
            Tok::Punct("}") => { self.lex.peeked = Some(t); Ok(()) }
            Tok::Punct("[") => {
                let s: NamedOrBlankNode = BlankNode::default().into();
                if self.lex.peek()? == &Tok::Punct("]") {
                    self.lex.next()?;
                    return self.predicate_object_list(&s);
                }
                self.predicate_object_list(&s)?;
                if self.lex.peek()? == &Tok::Punct("]") { self.lex.next()?; } else { return Err("blank node property lists should end with a ']'".into()) }
                if !matches!(self.lex.peek()?, Tok::Punct("." | "}")) { self.predicate_object_list(&s)? }
                Ok(())
            }
            Tok::BNode(l) => { let s = BlankNode::new_unchecked(l.clone()).into(); self.predicate_object_list(&s) }
            Tok::Punct("(") => { let s = self.collection_subject()?; self.predicate_object_list(&s) }
            t => match self.iri_of(t) { Some(i) => { let s = i?.into(); self.predicate_object_list(&s) } None => Err("TOKEN is not a valid RDF subject".into()) },
        }
    }

    fn predicate_object_list(&mut self, s: &NamedOrBlankNode) -> R<()> {
        loop {
            let p = self.verb()?;
            self.objects_list(s, &p)?;
            if self.lex.peek()? != &Tok::Punct(";") { return Ok(()) }
            while self.lex.peek()? == &Tok::Punct(";") { self.lex.next()?; }
            if matches!(self.lex.peek()?, Tok::Punct("." | "}" | "]" | "|}")) { return Ok(()) }
        }
    }
    fn verb(&mut self) -> R<NamedNode> {
        let t = self.lex.next()?;
        if t == Tok::Keyword("a".into()) { return Ok(rdf("type")) }
        match self.iri_of(&t) { Some(i) => i, None => Err("TOKEN is not a valid predicate".into()) }
    }
    fn objects_list(&mut self, s: &NamedOrBlankNode, p: &NamedNode) -> R<()> {
        loop {
            self.object(s, p)?;
            if self.lex.peek()? != &Tok::Punct(",") { return Ok(()) }
            self.lex.next()?;
        }
    }

    /// Reads one object and emits `(s, p, object)`, with any nested statements in `oxttl`'s order: a property list's
    /// own statements before the triple that uses it, a collection's head triple before its list statements.
    fn object(&mut self, s: &NamedOrBlankNode, p: &NamedNode) -> R<()> {
        let t = self.lex.next()?;
        let o: Term = match t {
            Tok::BNode(l) => BlankNode::new_unchecked(l).into(),
            Tok::Punct("[") => {
                if self.lex.peek()? == &Tok::Punct("]") {
                    self.lex.next()?;
                    BlankNode::default().into()
                } else {
                    let b: NamedOrBlankNode = BlankNode::default().into();
                    self.predicate_object_list(&b)?;
                    if self.lex.next()? != Tok::Punct("]") { return Err("blank node property lists should end with a ']'".into()) }
                    Term::from(b)
                }
            }
            Tok::Punct("(") => {
                if self.lex.peek()? == &Tok::Punct(")") {
                    self.lex.next()?;
                    rdf("nil").into()
                } else {
                    let root = BlankNode::default();
                    self.emit(s, p, root.clone().into());
                    return self.collection_items(root);
                }
            }
            Tok::Str(v) => self.literal_suffix(v)?,
            Tok::Integer(v) => Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("{XSD}integer"))).into(),
            Tok::Decimal(v) => Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("{XSD}decimal"))).into(),
            Tok::Double(v) => Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("{XSD}double"))).into(),
            Tok::Keyword(k) if k == "true" || k == "false" => Literal::new_typed_literal(k, NamedNode::new_unchecked(format!("{XSD}boolean"))).into(),
            t => match self.iri_of(&t) { Some(i) => i?.into(), None => return Err("TOKEN is not a valid RDF object".into()) },
        };
        self.emit(s, p, o);
        Ok(())
    }

    /// The items of a collection whose first node is `node`, as `rdf:first` / `rdf:rest` statements.
    fn collection_items(&mut self, mut node: BlankNode) -> R<()> {
        let first = rdf("first");
        let rest = rdf("rest");
        loop {
            let n: NamedOrBlankNode = node.clone().into();
            self.object(&n, &first)?;
            if self.lex.peek()? == &Tok::Punct(")") {
                self.lex.next()?;
                self.emit(&n, &rest, rdf("nil").into());
                return Ok(());
            }
            let next = BlankNode::default();
            self.emit(&n, &rest, next.clone().into());
            node = next;
        }
    }
    /// A collection used as a subject: its list statements, and its head node (or `rdf:nil`).
    fn collection_subject(&mut self) -> R<NamedOrBlankNode> {
        if self.lex.peek()? == &Tok::Punct(")") { self.lex.next()?; return Ok(rdf("nil").into()) }
        let root = BlankNode::default();
        self.collection_items(root.clone())?;
        Ok(root.into())
    }

    fn literal_suffix(&mut self, value: String) -> R<Term> {
        match self.lex.peek()? {
            Tok::Lang(_) => { let Tok::Lang(l) = self.lex.next()? else { unreachable!() }; Ok(Literal::new_language_tagged_literal_unchecked(value, l.to_ascii_lowercase()).into()) }
            Tok::Punct("^^") => {
                self.lex.next()?;
                let t = self.lex.next()?;
                let d = match self.iri_of(&t) { Some(i) => i?, None => return Err("A literal datatype must be an IRI".into()) };
                if d.as_str() == format!("{RDF}langString") { return Err("The datatype of a literal without a language tag must not be rdf:langString".into()) }
                Ok(Literal::new_typed_literal(value, d).into())
            }
            _ => Ok(Literal::new_simple_literal(value).into()),
        }
    }
}

/// Streams the statements of a Turtle (`with_graph = false`) or TriG document. The first error ends the stream.
pub struct TurtleReader<I> { parser: Parser<I>, done: bool }

impl<I: BufRead> TurtleReader<I> {
    fn new(input: I, with_graph: bool, capped: bool) -> Self {
        let src = Src { input, buf: Vec::new(), pos: 0, eof: false, capped, token_start: 0 };
        TurtleReader { parser: Parser { lex: Lexer { src, base: None, peeked: None }, prefixes: HashMap::new(), with_graph, graph: GraphName::DefaultGraph, out: VecDeque::new() }, done: false }
    }
}

impl<I: BufRead> Iterator for TurtleReader<I> {
    type Item = Result<Quad, ReadError>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(q) = self.parser.out.pop_front() { return Some(Ok(q)) }
            if self.done { return None }
            match self.parser.statement() {
                Ok(true) => {}
                Ok(false) => self.done = true,
                Err(e) => {
                    self.done = true;
                    // Statements completed before the error are still yielded first, as `oxttl` does.
                    let e = match e {
                        Fail::Syntax(m) => ReadError::Syntax(m),
                        Fail::Io(e) => ReadError::Io(e),
                        Fail::TooLarge => ReadError::Io(std::io::Error::new(std::io::ErrorKind::OutOfMemory, format!("token larger than {MAX_TOKEN} bytes"))),
                    };
                    self.parser.out.clear();
                    return Some(Err(e));
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
