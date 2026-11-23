//! Tokenizer for SPARQL 1.1 (grammar §19.8, terminals). Keywords are recognized case-insensitively by the parser
//! from `Name` tokens; this layer only splits the text.
//!
//! With the `sparql-12` feature it also produces the SPARQL 1.2 terminals (§19.7): `<<(`, `)>>`, `<<`, `>>`, `{|`,
//! `|}`, `~`, and `LANG_DIR` (`@en--ltr`), whose `LangTag` text then keeps the `--dir` suffix for the parser to check.
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// `<…>` with `\u` escapes resolved, not yet resolved against BASE.
    Iri(String),
    /// `prefix:local` with the local part's escapes resolved (`prefix` may be empty).
    PName(String, String),
    /// `_:label`.
    BNode(String),
    /// `?x` or `$x`, without the sigil.
    Var(String),
    /// `@en-us`, without the `@` (with `sparql-12`, also `@en-us--rtl`: the text then contains `--`).
    LangTag(String),
    /// Numbers keep their exact lexical form.
    Integer(String),
    Decimal(String),
    Double(String),
    /// A string literal's value, escapes resolved.
    Str(String),
    /// Keywords, `a`, `true`/`false` and function names: letters, digits and `_`.
    Name(String),
    /// `[]` with only whitespace inside.
    Anon,
    /// `()` with only whitespace inside.
    Nil,
    Punct(&'static str),
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Iri(i) => write!(f, "<{i}>"),
            Tok::PName(p, l) => write!(f, "{p}:{l}"),
            Tok::BNode(b) => write!(f, "_:{b}"),
            Tok::Var(v) => write!(f, "?{v}"),
            Tok::LangTag(l) => write!(f, "@{l}"),
            Tok::Integer(n) | Tok::Decimal(n) | Tok::Double(n) => f.write_str(n),
            Tok::Str(s) => write!(f, "{s:?}"),
            Tok::Name(n) => f.write_str(n),
            Tok::Anon => f.write_str("[]"),
            Tok::Nil => f.write_str("()"),
            Tok::Punct(p) => f.write_str(p),
            Tok::Eof => f.write_str("end of input"),
        }
    }
}

/// A token with its byte offset (for error messages).
#[derive(Clone, Debug)]
pub struct Spanned { pub tok: Tok, pub pos: usize }

#[derive(Debug, Clone)]
pub struct LexError { pub message: String, pub pos: usize }

const PUNCT: &[&str] = &["^^", "||", "&&", "!=", "<=", ">=", "{", "}", "(", ")", "[", "]", ".", ",", ";", "*", "+", "-", "/",
    "!", "^", "|", "=", "<", ">", "?"];

/// SPARQL 1.2 punctuation, tried before `PUNCT` so the longest match wins (`<<(` over `<<` over `<`, `)>>` over `)`,
/// `>>` over `>`, `{|` over `{`, `|}` over `|`).
#[cfg(feature = "sparql-12")]
const PUNCT_12: &[&str] = &["<<(", ")>>", "<<", ">>", "{|", "|}", "~"];

pub fn tokenize(src: &str) -> Result<Vec<Spanned>, LexError> {
    let mut lx = Lexer { s: src, i: 0 };
    let mut out = Vec::new();
    loop {
        lx.skip_ws();
        let pos = lx.i;
        let tok = lx.next()?;
        let end = tok == Tok::Eof;
        out.push(Spanned { tok, pos });
        if end { return Ok(out) }
    }
}

struct Lexer<'a> { s: &'a str, i: usize }

fn is_pn_chars_base(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(c as u32, 0xC0..=0xD6 | 0xD8..=0xF6 | 0xF8..=0x2FF | 0x370..=0x37D | 0x37F..=0x1FFF | 0x200C..=0x200D
            | 0x2070..=0x218F | 0x2C00..=0x2FEF | 0x3001..=0xD7FF | 0xF900..=0xFDCF | 0xFDF0..=0xFFFD | 0x10000..=0xEFFFF)
}
fn is_pn_chars_u(c: char) -> bool { is_pn_chars_base(c) || c == '_' }
fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c) || c == '-' || c.is_ascii_digit() || matches!(c as u32, 0xB7 | 0x300..=0x36F | 0x203F..=0x2040)
}
fn is_varname_start(c: char) -> bool { is_pn_chars_u(c) || c.is_ascii_digit() }
fn is_varname_char(c: char) -> bool { is_varname_start(c) || matches!(c as u32, 0xB7 | 0x300..=0x36F | 0x203F..=0x2040) }

impl<'a> Lexer<'a> {
    fn err<T>(&self, m: impl Into<String>) -> Result<T, LexError> { Err(LexError { message: m.into(), pos: self.i }) }
    fn peek(&self) -> Option<char> { self.s[self.i..].chars().next() }
    fn peek_at(&self, n: usize) -> Option<char> { self.s[self.i..].chars().nth(n) }
    fn bump(&mut self) -> Option<char> { let c = self.peek()?; self.i += c.len_utf8(); Some(c) }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() { self.bump(); } else if c == '#' {
                while let Some(c) = self.bump() { if c == '\n' || c == '\r' { break } }
            } else { break }
        }
    }

    fn next(&mut self) -> Result<Tok, LexError> {
        let Some(c) = self.peek() else { return Ok(Tok::Eof) };
        match c {
            '<' => if let Some(iri) = self.try_iri()? { return Ok(Tok::Iri(iri)) },
            '?' | '$' => if self.peek_at(1).is_some_and(is_varname_start) {
                self.bump();
                let start = self.i;
                while self.peek().is_some_and(is_varname_char) { self.bump(); }
                return Ok(Tok::Var(self.s[start..self.i].to_string()));
            },
            '"' | '\'' => return self.string(),
            '@' => {
                self.bump();
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) { self.bump(); }
                if self.i == start { return self.err("expected a language tag after '@'") }
                while self.peek() == Some('-') && self.peek_at(1).is_some_and(|c| c.is_ascii_alphanumeric()) {
                    self.bump();
                    while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) { self.bump(); }
                }
                // LANG_DIR (§19.7 [165]): `('--' [a-zA-Z]+)?` after the tag.
                #[cfg(feature = "sparql-12")]
                if self.peek() == Some('-') && self.peek_at(1) == Some('-') && self.peek_at(2).is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.i += 2;
                    while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) { self.bump(); }
                }
                return Ok(Tok::LangTag(self.s[start..self.i].to_string()));
            }
            '_' if self.peek_at(1) == Some(':') => {
                self.i += 2;
                let start = self.i;
                match self.peek() { Some(c) if is_pn_chars_u(c) || c.is_ascii_digit() => { self.bump(); } _ => return self.err("expected a blank node label") }
                self.name_tail(is_pn_chars);
                return Ok(Tok::BNode(self.s[start..self.i].to_string()));
            }
            '[' => if let Some(n) = self.empty_pair(']') { self.i = n; return Ok(Tok::Anon) },
            '(' => if let Some(n) = self.empty_pair(')') { self.i = n; return Ok(Tok::Nil) },
            c if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) => return self.number(),
            _ => {}
        }
        if c == ':' || is_pn_chars_base(c) { return self.name_or_pname() }
        #[cfg(feature = "sparql-12")]
        for p in PUNCT_12 { if self.s[self.i..].starts_with(p) { self.i += p.len(); return Ok(Tok::Punct(p)) } }
        for p in PUNCT { if self.s[self.i..].starts_with(p) { self.i += p.len(); return Ok(Tok::Punct(p)) } }
        self.err(format!("unexpected character {c:?}"))
    }

    /// End offset of `[ ws* ]` / `( ws* )` starting here, if it is one.
    fn empty_pair(&self, close: char) -> Option<usize> {
        let mut j = self.i + 1;
        let rest = &self.s[j..];
        for c in rest.chars() {
            if c == close { return Some(j + 1) }
            if !c.is_whitespace() { return None }
            j += c.len_utf8();
        }
        None
    }

    /// `<…>` if the text here is an IRIREF; `None` if `<` is the comparison operator.
    fn try_iri(&mut self) -> Result<Option<String>, LexError> {
        let mut j = self.i + 1;
        let mut out = String::new();
        let rest = self.s;
        while j < rest.len() {
            let c = rest[j..].chars().next().unwrap();
            match c {
                '>' => { self.i = j + 1; return Ok(Some(out)) }
                '\\' => {
                    let (ch, len) = match unicode_escape(&rest[j..]) { Some(x) => x, None => return Ok(None) };
                    if (ch as u32) <= 0x20 || "<>\"{}|^`\\".contains(ch) { return Err(LexError { message: "invalid character in IRI escape".into(), pos: j }) }
                    out.push(ch);
                    j += len;
                }
                c if (c as u32) <= 0x20 || "<\"{}|^`".contains(c) => return Ok(None),
                c => { out.push(c); j += c.len_utf8(); }
            }
        }
        Ok(None)
    }

    fn string(&mut self) -> Result<Tok, LexError> {
        let q = self.bump().unwrap();
        let long = self.peek() == Some(q) && self.peek_at(1) == Some(q);
        if long { self.i += 2 * q.len_utf8() }
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else { return self.err("unterminated string") };
            if c == q {
                if !long { self.bump(); return Ok(Tok::Str(out)) }
                if self.peek_at(1) == Some(q) && self.peek_at(2) == Some(q) {
                    // In a long string a run of quotes ends at the last three.
                    if self.peek_at(3) == Some(q) { out.push(q); self.bump(); continue }
                    self.i += 3 * q.len_utf8();
                    return Ok(Tok::Str(out));
                }
                out.push(c); self.bump(); continue;
            }
            if !long && (c == '\n' || c == '\r') { return self.err("newline in a short string") }
            if c == '\\' {
                let rest = &self.s[self.i..];
                if let Some((ch, len)) = unicode_escape(rest) { out.push(ch); self.i += len; continue }
                self.bump();
                let e = self.bump();
                out.push(match e {
                    Some('t') => '\t', Some('b') => '\u{8}', Some('n') => '\n', Some('r') => '\r', Some('f') => '\u{c}',
                    Some('"') => '"', Some('\'') => '\'', Some('\\') => '\\',
                    _ => return self.err("invalid escape in string"),
                });
                continue;
            }
            out.push(c);
            self.bump();
        }
    }

    fn number(&mut self) -> Result<Tok, LexError> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) { self.bump(); }
        let mut kind = 0; // 0 integer, 1 decimal, 2 double
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) { self.bump(); }
            kind = 1;
        } else if self.peek() == Some('.') && matches!(self.peek_at(1), Some('e' | 'E')) && self.i > start {
            // `1.e5`
            self.bump();
            kind = 1;
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let save = self.i;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) { self.bump(); }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) { self.bump(); }
                kind = 2;
            } else { self.i = save; if kind == 1 && self.s[..self.i].ends_with('.') { return self.err("invalid number") } }
        }
        let text = self.s[start..self.i].to_string();
        Ok(match kind { 0 => Tok::Integer(text), 1 => Tok::Decimal(text), _ => Tok::Double(text) })
    }

    /// Consumes PN_CHARS / '.' with the rule that a name never ends with '.'.
    fn name_tail(&mut self, ok: fn(char) -> bool) {
        loop {
            match self.peek() {
                Some(c) if ok(c) => { self.bump(); }
                Some('.') => {
                    // A dot belongs to the name only if more name characters follow.
                    let mut k = 1;
                    while self.peek_at(k) == Some('.') { k += 1; }
                    if self.peek_at(k).is_some_and(ok) { for _ in 0..k { self.bump(); } } else { return }
                }
                _ => return,
            }
        }
    }

    fn name_or_pname(&mut self) -> Result<Tok, LexError> {
        let start = self.i;
        // PN_PREFIX (or a keyword / function name).
        if self.peek() != Some(':') {
            self.bump();
            self.name_tail(is_pn_chars);
        }
        let prefix = self.s[start..self.i].to_string();
        if self.peek() != Some(':') {
            // Keywords and names have no dots; a name ending before ':' is a prefix.
            if prefix.contains('.') { return Err(LexError { message: format!("unexpected name {prefix:?}"), pos: start }) }
            return Ok(Tok::Name(prefix));
        }
        self.bump(); // ':'
        let local = self.pn_local()?;
        Ok(Tok::PName(prefix, local))
    }

    fn pn_local(&mut self) -> Result<String, LexError> {
        let mut out = String::new();
        let first = |c: char| is_pn_chars_u(c) || c == ':' || c.is_ascii_digit();
        let mid = |c: char| is_pn_chars(c) || c == ':';
        let mut started = false;
        while let Some(c) = self.peek() {
            if c == '%' {
                let hex = |x: Option<char>| x.is_some_and(|c| c.is_ascii_hexdigit());
                if !(hex(self.peek_at(1)) && hex(self.peek_at(2))) { return self.err("invalid %-escape in local name") }
                for _ in 0..3 { out.push(self.bump().unwrap()); }
            } else if c == '\\' {
                match self.peek_at(1) {
                    Some(e) if "_~.-!$&'()*+,;=/?#@%".contains(e) => { self.i += 2; out.push(e); }
                    _ => return self.err("invalid escape in local name"),
                }
            } else if if started { mid(c) } else { first(c) } {
                out.push(c); self.bump();
            } else if c == '.' && started {
                let mut k = 1;
                while self.peek_at(k) == Some('.') { k += 1; }
                let next = self.peek_at(k);
                if next.is_some_and(|n| mid(n) || n == '%' || n == '\\') { for _ in 0..k { out.push('.'); self.bump(); } } else { break }
            } else { break }
            started = true;
        }
        Ok(out)
    }
}

/// `\uXXXX` / `\UXXXXXXXX` at the start of `s`: the character and the escape's length.
fn unicode_escape(s: &str) -> Option<(char, usize)> {
    let b = s.as_bytes();
    if b.len() < 2 || b[0] != b'\\' { return None }
    let n = match b[1] { b'u' => 4, b'U' => 8, _ => return None };
    let hex = s.get(2..2 + n)?;
    if !hex.bytes().all(|c| c.is_ascii_hexdigit()) { return None }
    Some((char::from_u32(u32::from_str_radix(hex, 16).ok()?)?, 2 + n))
}
