// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
//! The leaf layer of the term readers: the errors they return and the lexical rules they share. Every term type's
//! `read` and `FromStr` (in `named_node`, `blank_node`, `literal`, `variable`, `triple`) depends on this module; it
//! depends on none of them.
use crate::LanguageTagParseError;
use std::char;
use std::fmt;
use std::str::Chars;
use triplelight_iri::InvalidIri as IriParseError;

/// The deepest nesting of triple terms a reader accepts, to avoid a stack overflow.
pub(crate) const MAX_NUMBER_OF_NESTED_TRIPLES: usize = 128;

#[derive(Debug)]
pub struct BlankNodeIdParseError;

impl fmt::Display for BlankNodeIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("The blank node identifier is invalid")
    }
}

impl std::error::Error for BlankNodeIdParseError {}

#[derive(Debug)]
pub struct VariableNameParseError;

impl fmt::Display for VariableNameParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("The variable name is invalid")
    }
}

impl std::error::Error for VariableNameParseError {}

pub(crate) fn read_hexa_char(input: &mut Chars<'_>, len: usize) -> Result<char, TermParseError> {
    let mut value = 0;
    for _ in 0..len {
        if let Some(c) = input.next() {
            value = value * 16
                + match c {
                    '0'..='9' => u32::from(c) - u32::from('0'),
                    'a'..='f' => u32::from(c) - u32::from('a') + 10,
                    'A'..='F' => u32::from(c) - u32::from('A') + 10,
                    _ => {
                        return Err(TermParseError::msg(format!(
                            "Unexpected character in a unicode escape: {c}"
                        )));
                    }
                }
        } else {
            return Err(TermParseError::msg("Unexpected literal string end"));
        }
    }
    char::from_u32(value).ok_or_else(|| TermParseError::msg("Invalid encoded unicode code point"))
}

#[derive(Debug)]
pub struct TermParseError(pub(crate) TermParseErrorKind);

#[derive(Debug)]
pub(crate) enum TermParseErrorKind {
    Iri { error: IriParseError, value: String },
    BlankNode { error: BlankNodeIdParseError, value: String },
    LanguageTag { error: LanguageTagParseError, value: String },
    Variable { error: VariableNameParseError, value: String },
    Msg(String),
}

impl std::fmt::Display for TermParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            TermParseErrorKind::Iri { error, value } => write!(f, "Error while parsing the named node '{value}': {error}"),
            TermParseErrorKind::BlankNode { error, value } => write!(f, "Error while parsing the blank node '{value}': {error}"),
            TermParseErrorKind::LanguageTag { error, value } => write!(f, "Error while parsing the language tag '{value}': {error}"),
            TermParseErrorKind::Variable { error, value } => write!(f, "Error while parsing the variable '{value}': {error}"),
            TermParseErrorKind::Msg(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for TermParseError {}

impl TermParseError {
    pub(crate) fn msg(msg: impl Into<String>) -> Self {
        Self(TermParseErrorKind::Msg(msg.into()))
    }
}
