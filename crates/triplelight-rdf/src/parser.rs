// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
#[cfg(feature = "rdf-12")]
use crate::BaseDirection;
use crate::vocab::xsd;
use crate::{
    BlankNode, BlankNodeIdParseError, GraphName, IriParseError, LanguageTagParseError, Literal,
    NamedNode, Quad, Term, Triple, Variable, VariableNameParseError,
};
use std::borrow::Cow;
use std::char;
use std::str::{Chars, FromStr};

const MAX_NUMBER_OF_NESTED_TRIPLES: usize = 128;

impl FromStr for NamedNode {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = read_named_node(s)?;
        if !left.is_empty() {
            return Err(Self::Err::msg(
                "Named node serialization should end with a >",
            ));
        }
        Ok(term)
    }
}

impl FromStr for BlankNode {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = read_blank_node(s)?;
        if !left.is_empty() {
            return Err(Self::Err::msg(
                "Blank node serialization should not contain whitespaces",
            ));
        }
        Ok(term)
    }
}

impl FromStr for Literal {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = read_literal(s)?;
        if !left.is_empty() {
            return Err(Self::Err::msg("Invalid literal serialization"));
        }
        Ok(term)
    }
}

impl FromStr for Term {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = read_term(s, 0)?;
        if !left.is_empty() {
            return Err(Self::Err::msg("Invalid term serialization"));
        }
        Ok(term)
    }
}

impl FromStr for Triple {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (triple, left) = read_triple(s, 0)?;
        if !matches!(left.trim(), "" | ".") {
            return Err(Self::Err::msg("Invalid triple serialization"));
        }
        Ok(triple)
    }
}

impl FromStr for Quad {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (triple, left) = read_triple(s, 0)?;
        if matches!(left.trim(), "" | ".") {
            return Ok(triple.in_graph(GraphName::DefaultGraph));
        }
        let (graph_name, left) = read_term(left, 0)?;
        if !matches!(left.trim(), "" | ".") {
            return Err(Self::Err::msg("Invalid triple serialization"));
        }
        Ok(triple.in_graph(match graph_name {
            Term::NamedNode(graph_name) => GraphName::from(graph_name),
            Term::BlankNode(graph_name) => GraphName::from(graph_name),
            Term::Literal(_) => {
                return Err(TermParseError::msg(
                    "Literals are not allowed in graph name position",
                ));
            }
            #[cfg(feature = "rdf-12")]
            Term::Triple(_) => {
                return Err(TermParseError::msg(
                    "Triple terms are not allowed in graph name position",
                ));
            }
        }))
    }
}

impl FromStr for Variable {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with('?') && !s.starts_with('$') {
            return Err(Self::Err::msg(
                "Variable serialization should start with ? or $",
            ));
        }
        Self::new(&s[1..]).map_err(|error| {
            TermParseError(TermParseErrorKind::Variable {
                value: s.to_owned(),
                error,
            })
        })
    }
}

fn read_named_node(s: &str) -> Result<(NamedNode, &str), TermParseError> {
    let s = s.trim();
    if let Some(remain) = s.strip_prefix('<') {
        let end = remain
            .find('>')
            .ok_or_else(|| TermParseError::msg("Named node serialization should end with a >"))?;
        let (value, remain) = remain.split_at(end);
        let remain = &remain[1..];
        let value = if value.contains('\\') {
            let mut escaped = String::with_capacity(value.len());
            let mut chars = value.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('u') => escaped.push(read_hexa_char(&mut chars, 4)?),
                        Some('U') => escaped.push(read_hexa_char(&mut chars, 8)?),
                        Some(c) => {
                            escaped.push('\\');
                            escaped.push(c);
                        }
                        None => escaped.push('\\'),
                    }
                } else {
                    escaped.push(c);
                }
            }
            Cow::Owned(escaped)
        } else {
            Cow::Borrowed(value)
        };
        let term = NamedNode::new(value.as_ref()).map_err(|error| {
            TermParseError(TermParseErrorKind::Iri {
                value: value.into_owned(),
                error,
            })
        })?;
        Ok((term, remain))
    } else {
        Err(TermParseError::msg(
            "Named node serialization should start with a <",
        ))
    }
}

fn read_blank_node(s: &str) -> Result<(BlankNode, &str), TermParseError> {
    let s = s.trim();
    if let Some(remain) = s.strip_prefix("_:") {
        let mut end = remain
            .find(|v: char| {
                v.is_whitespace()
                    || matches!(
                        v,
                        '<' | '?'
                            | '$'
                            | '"'
                            | '\''
                            | '>'
                            | '@'
                            | '^'
                            | ':'
                            | '('
                            | ')'
                            | '{'
                            | '}'
                            | '['
                            | ']'
                    )
            })
            .unwrap_or(remain.len());
        if let Some(pos) = remain[..end].find("..") {
            end = pos;
        }
        if remain[..end].ends_with('.') {
            // It can't end with '.'
            end -= 1;
        }
        let (value, remain) = remain.split_at(end);
        let term = BlankNode::new(value).map_err(|error| {
            TermParseError(TermParseErrorKind::BlankNode {
                value: value.to_owned(),
                error,
            })
        })?;
        Ok((term, remain))
    } else {
        Err(TermParseError::msg(
            "Blank node serialization should start with '_:'",
        ))
    }
}

fn read_literal(s: &str) -> Result<(Literal, &str), TermParseError> {
    let s = s.trim();
    if let Some(s) = s.strip_prefix('"') {
        let mut value = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let remain = chars.as_str().trim();
                    return if let Some(remain) = remain.strip_prefix('@') {
                        let end = remain
                            .find(|v: char| !v.is_ascii_alphanumeric() && v != '-')
                            .unwrap_or(remain.len());
                        let (language, remain) = remain.split_at(end);
                        #[cfg(feature = "rdf-12")]
                        if let Some((language, direction)) = language.split_once("--") {
                            return Ok((
                                Literal::new_directional_language_tagged_literal(value, language, match direction {
                                    "ltr" => BaseDirection::Ltr,
                                    "rtl" => BaseDirection::Rtl,
                                    _ => return Err(TermParseError(TermParseErrorKind::Msg(format!("The only two possible base directions are 'rtl' and 'ltr', found '{direction}'"))))
                                }).map_err(
                                    |error| {
                                        TermParseError(TermParseErrorKind::LanguageTag {
                                            value: language.to_owned(),
                                            error,
                                        })
                                    },
                                )?,
                                remain,
                            ));
                        }
                        Ok((
                            Literal::new_language_tagged_literal(value, language).map_err(
                                |error| {
                                    TermParseError(TermParseErrorKind::LanguageTag {
                                        value: language.to_owned(),
                                        error,
                                    })
                                },
                            )?,
                            remain,
                        ))
                    } else if let Some(remain) = remain.strip_prefix("^^") {
                        let (datatype, remain) = read_named_node(remain)?;
                        Ok((Literal::new_typed_literal(value, datatype), remain))
                    } else {
                        Ok((Literal::new_simple_literal(value), remain))
                    };
                }
                '\\' => {
                    if let Some(c) = chars.next() {
                        value.push(match c {
                            't' => '\t',
                            'b' => '\u{08}',
                            'n' => '\n',
                            'r' => '\r',
                            'f' => '\u{0C}',
                            '"' => '"',
                            '\'' => '\'',
                            '\\' => '\\',
                            'u' => read_hexa_char(&mut chars, 4)?,
                            'U' => read_hexa_char(&mut chars, 8)?,
                            _ => return Err(TermParseError::msg("Unexpected escaped char")),
                        })
                    } else {
                        return Err(TermParseError::msg("Unexpected literal end"));
                    }
                }
                _ => value.push(c),
            }
        }
        Err(TermParseError::msg("Unexpected literal end"))
    } else if let Some(remain) = s.strip_prefix("true") {
        Ok((Literal::new_typed_literal("true", xsd::BOOLEAN), remain))
    } else if let Some(remain) = s.strip_prefix("false") {
        Ok((Literal::new_typed_literal("false", xsd::BOOLEAN), remain))
    } else {
        let input = s.as_bytes();
        if input.is_empty() {
            return Err(TermParseError::msg("Empty term serialization"));
        }

        let mut cursor = match input.first() {
            Some(b'+' | b'-') => 1,
            _ => 0,
        };
        let mut with_dot = false;

        let mut count_before: usize = 0;
        while cursor < input.len() && b'0' <= input[cursor] && input[cursor] <= b'9' {
            count_before += 1;
            cursor += 1;
        }

        let mut count_after: usize = 0;
        if cursor < input.len() && input[cursor] == b'.' {
            with_dot = true;
            cursor += 1;
            while cursor < input.len() && b'0' <= input[cursor] && input[cursor] <= b'9' {
                count_after += 1;
                cursor += 1;
            }
        }

        if cursor < input.len() && (input[cursor] == b'e' || input[cursor] == b'E') {
            cursor += 1;
            cursor += match input.get(cursor) {
                Some(b'+' | b'-') => 1,
                _ => 0,
            };
            let mut count_exponent = 0;
            while cursor < input.len() && b'0' <= input[cursor] && input[cursor] <= b'9' {
                count_exponent += 1;
                cursor += 1;
            }
            if count_exponent > 0 {
                Ok((
                    Literal::new_typed_literal(&s[..cursor], xsd::DOUBLE),
                    &s[cursor..],
                ))
            } else {
                Err(TermParseError::msg(
                    "Double serialization with an invalid exponent",
                ))
            }
        } else if with_dot {
            if count_after > 0 {
                Ok((
                    Literal::new_typed_literal(&s[..cursor], xsd::DECIMAL),
                    &s[cursor..],
                ))
            } else {
                Err(TermParseError::msg(
                    "Decimal serialization without floating part",
                ))
            }
        } else if count_before > 0 {
            Ok((
                Literal::new_typed_literal(&s[..cursor], xsd::INTEGER),
                &s[cursor..],
            ))
        } else {
            Err(TermParseError::msg("Empty integer serialization"))
        }
    }
}

fn read_term(s: &str, number_of_recursive_calls: usize) -> Result<(Term, &str), TermParseError> {
    if number_of_recursive_calls == MAX_NUMBER_OF_NESTED_TRIPLES {
        return Err(TermParseError::msg(
            "Too many nested triples. The parser fails here to avoid a stack overflow.",
        ));
    }
    let s = s.trim();
    #[allow(unused_variables, clippy::allow_attributes)]
    if let Some(remain) = s.strip_prefix("<<(") {
        #[cfg(feature = "rdf-12")]
        {
            let (triple, remain) = read_triple(remain, number_of_recursive_calls + 1)?;
            let remain = remain.trim_start();
            if let Some(remain) = remain.strip_prefix(")>>") {
                Ok((triple.into(), remain))
            } else {
                Err(TermParseError::msg(
                    "Triple term serialization must be enclosed between <<( and )>>",
                ))
            }
        }
        #[cfg(not(feature = "rdf-12"))]
        {
            Err(TermParseError::msg("RDF 1.2 is not supported"))
        }
    } else if s.starts_with('<') {
        let (term, remain) = read_named_node(s)?;
        Ok((term.into(), remain))
    } else if s.starts_with('_') {
        let (term, remain) = read_blank_node(s)?;
        Ok((term.into(), remain))
    } else {
        let (term, remain) = read_literal(s)?;
        Ok((term.into(), remain))
    }
}

fn read_triple(
    s: &str,
    number_of_recursive_calls: usize,
) -> Result<(Triple, &str), TermParseError> {
    let s = s.trim();
    let (subject, remain) = read_term(s, number_of_recursive_calls + 1)?;
    let (predicate, remain) = read_named_node(remain)?;
    let (object, remain) = read_term(remain, number_of_recursive_calls + 1)?;
    Ok((
        Triple {
            subject: match subject {
                Term::NamedNode(s) => s.into(),
                Term::BlankNode(s) => s.into(),
                Term::Literal(_) => {
                    return Err(TermParseError::msg(
                        "Literals are not allowed in subject position",
                    ));
                }
                #[cfg(feature = "rdf-12")]
                Term::Triple(_) => {
                    return Err(TermParseError::msg(
                        "Triple terms are not allowed in subject position",
                    ));
                }
            },
            predicate,
            object,
        },
        remain,
    ))
}

fn read_hexa_char(input: &mut Chars<'_>, len: usize) -> Result<char, TermParseError> {
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
pub struct TermParseError(TermParseErrorKind);

#[derive(Debug)]
enum TermParseErrorKind {
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
