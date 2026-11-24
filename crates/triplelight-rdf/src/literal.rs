// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
use crate::named_node::{NamedNode, NamedNodeRef};
use crate::syntax::{read_hexa_char, TermParseError, TermParseErrorKind};
use crate::vocab::{rdf, xsd};
use crate::langtag::{self, LanguageTagParseError};
use std::borrow::Cow;
use std::str::FromStr;
use std::fmt;
use std::fmt::Write;

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub struct Literal(LiteralContent);

#[derive(PartialEq, Eq, Debug, Clone, Hash)]
enum LiteralContent {
    String(String),
    LanguageTaggedString {
        value: String,
        language: String,
    },
    #[cfg(feature = "rdf-12")]
    DirectionalLanguageTaggedString {
        value: String,
        language: String,
        direction: BaseDirection,
    },
    TypedLiteral {
        value: String,
        datatype: NamedNode,
    },
}

impl Literal {
    #[inline]
    pub fn new_simple_literal(value: impl Into<String>) -> Self {
        Self(LiteralContent::String(value.into()))
    }

    #[inline]
    pub fn new_typed_literal(value: impl Into<String>, datatype: impl Into<NamedNode>) -> Self {
        let value = value.into();
        let datatype = datatype.into();
        Self(if datatype == xsd::STRING {
            LiteralContent::String(value)
        } else {
            LiteralContent::TypedLiteral { value, datatype }
        })
    }

    #[inline]
    pub fn new_language_tagged_literal(
        value: impl Into<String>,
        language: impl Into<String>,
    ) -> Result<Self, LanguageTagParseError> {
        let mut language = language.into();
        language.make_ascii_lowercase();
        Ok(Self::new_language_tagged_literal_unchecked(
            value,
            langtag::parse(language)?,
        ))
    }

    #[inline]
    pub fn new_language_tagged_literal_unchecked(
        value: impl Into<String>,
        language: impl Into<String>,
    ) -> Self {
        Self(LiteralContent::LanguageTaggedString {
            value: value.into(),
            language: language.into(),
        })
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub fn new_directional_language_tagged_literal(
        value: impl Into<String>,
        language: impl Into<String>,
        direction: impl Into<BaseDirection>,
    ) -> Result<Self, LanguageTagParseError> {
        let mut language = language.into();
        language.make_ascii_lowercase();
        Ok(Self::new_directional_language_tagged_literal_unchecked(
            value,
            langtag::parse(language)?,
            direction,
        ))
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub fn new_directional_language_tagged_literal_unchecked(
        value: impl Into<String>,
        language: impl Into<String>,
        direction: impl Into<BaseDirection>,
    ) -> Self {
        Self(LiteralContent::DirectionalLanguageTaggedString {
            value: value.into(),
            language: language.into(),
            direction: direction.into(),
        })
    }

    #[inline]
    pub fn value(&self) -> &str {
        self.as_ref().value()
    }

    #[inline]
    pub fn language(&self) -> Option<&str> {
        self.as_ref().language()
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub fn direction(&self) -> Option<BaseDirection> {
        self.as_ref().direction()
    }

    #[inline]
    pub fn datatype(&self) -> NamedNodeRef<'_> {
        self.as_ref().datatype()
    }

    #[inline]
    #[deprecated(note = "Plain literal concept is removed in RDF 1.1", since = "0.3.0")]
    pub fn is_plain(&self) -> bool {
        #[expect(deprecated)]
        self.as_ref().is_plain()
    }

    #[inline]
    pub fn as_ref(&self) -> LiteralRef<'_> {
        LiteralRef(match &self.0 {
            LiteralContent::String(value) => LiteralRefContent::String(value),
            LiteralContent::LanguageTaggedString { value, language } => {
                LiteralRefContent::LanguageTaggedString { value, language }
            }
            #[cfg(feature = "rdf-12")]
            LiteralContent::DirectionalLanguageTaggedString {
                value,
                language,
                direction,
            } => LiteralRefContent::DirectionalLanguageTaggedString {
                value,
                language,
                direction: *direction,
            },
            LiteralContent::TypedLiteral { value, datatype } => LiteralRefContent::TypedLiteral {
                value,
                datatype: datatype.as_ref(),
            },
        })
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub fn destruct(
        self,
    ) -> (
        String,
        Option<NamedNode>,
        Option<String>,
        Option<BaseDirection>,
    ) {
        match self.0 {
            LiteralContent::String(s) => (s, None, None, None),
            LiteralContent::LanguageTaggedString { value, language } => {
                (value, None, Some(language), None)
            }
            LiteralContent::DirectionalLanguageTaggedString {
                value,
                language,
                direction,
            } => (value, None, Some(language), Some(direction)),
            LiteralContent::TypedLiteral { value, datatype } => (value, Some(datatype), None, None),
        }
    }

    #[cfg(not(feature = "rdf-12"))]
    #[inline]
    pub fn destruct(self) -> (String, Option<NamedNode>, Option<String>) {
        match self.0 {
            LiteralContent::String(s) => (s, None, None),
            LiteralContent::LanguageTaggedString { value, language } => {
                (value, None, Some(language))
            }
            LiteralContent::TypedLiteral { value, datatype } => (value, Some(datatype), None),
        }
    }
}

impl fmt::Display for Literal {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<'a> From<&'a str> for Literal {
    #[inline]
    fn from(value: &'a str) -> Self {
        Self(LiteralContent::String(value.into()))
    }
}

impl From<String> for Literal {
    #[inline]
    fn from(value: String) -> Self {
        Self(LiteralContent::String(value))
    }
}

impl<'a> From<Cow<'a, str>> for Literal {
    #[inline]
    fn from(value: Cow<'a, str>) -> Self {
        Self(LiteralContent::String(value.into()))
    }
}

impl From<bool> for Literal {
    #[inline]
    fn from(value: bool) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::BOOLEAN.into(),
        })
    }
}

impl From<i128> for Literal {
    #[inline]
    fn from(value: i128) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<i64> for Literal {
    #[inline]
    fn from(value: i64) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<i32> for Literal {
    #[inline]
    fn from(value: i32) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<i16> for Literal {
    #[inline]
    fn from(value: i16) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<u64> for Literal {
    #[inline]
    fn from(value: u64) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<u32> for Literal {
    #[inline]
    fn from(value: u32) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<u16> for Literal {
    #[inline]
    fn from(value: u16) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: value.to_string(),
            datatype: xsd::INTEGER.into(),
        })
    }
}

impl From<f32> for Literal {
    #[inline]
    fn from(value: f32) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: if value == f32::INFINITY {
                "INF".to_owned()
            } else if value == f32::NEG_INFINITY {
                "-INF".to_owned()
            } else {
                value.to_string()
            },
            datatype: xsd::FLOAT.into(),
        })
    }
}

impl From<f64> for Literal {
    #[inline]
    fn from(value: f64) -> Self {
        Self(LiteralContent::TypedLiteral {
            value: if value == f64::INFINITY {
                "INF".to_owned()
            } else if value == f64::NEG_INFINITY {
                "-INF".to_owned()
            } else {
                value.to_string()
            },
            datatype: xsd::DOUBLE.into(),
        })
    }
}

















#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub struct LiteralRef<'a>(LiteralRefContent<'a>);

#[derive(PartialEq, Eq, Debug, Clone, Copy, Hash)]
enum LiteralRefContent<'a> {
    String(&'a str),
    LanguageTaggedString {
        value: &'a str,
        language: &'a str,
    },
    #[cfg(feature = "rdf-12")]
    DirectionalLanguageTaggedString {
        value: &'a str,
        language: &'a str,
        direction: BaseDirection,
    },
    TypedLiteral {
        value: &'a str,
        datatype: NamedNodeRef<'a>,
    },
}

impl<'a> LiteralRef<'a> {
    #[inline]
    pub const fn new_simple_literal(value: &'a str) -> Self {
        LiteralRef(LiteralRefContent::String(value))
    }

    #[inline]
    pub fn new_typed_literal(value: &'a str, datatype: impl Into<NamedNodeRef<'a>>) -> Self {
        let datatype = datatype.into();
        LiteralRef(if datatype == xsd::STRING {
            LiteralRefContent::String(value)
        } else {
            LiteralRefContent::TypedLiteral { value, datatype }
        })
    }

    #[inline]
    pub const fn new_language_tagged_literal_unchecked(value: &'a str, language: &'a str) -> Self {
        LiteralRef(LiteralRefContent::LanguageTaggedString { value, language })
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub const fn new_directional_language_tagged_literal_unchecked(
        value: &'a str,
        language: &'a str,
        direction: BaseDirection,
    ) -> Self {
        LiteralRef(LiteralRefContent::DirectionalLanguageTaggedString {
            value,
            language,
            direction,
        })
    }

    #[inline]
    pub const fn value(self) -> &'a str {
        match self.0 {
            LiteralRefContent::String(value)
            | LiteralRefContent::LanguageTaggedString { value, .. }
            | LiteralRefContent::TypedLiteral { value, .. } => value,
            #[cfg(feature = "rdf-12")]
            LiteralRefContent::DirectionalLanguageTaggedString { value, .. } => value,
        }
    }

    #[inline]
    pub const fn language(self) -> Option<&'a str> {
        match self.0 {
            LiteralRefContent::LanguageTaggedString { language, .. } => Some(language),
            #[cfg(feature = "rdf-12")]
            LiteralRefContent::DirectionalLanguageTaggedString { language, .. } => Some(language),
            _ => None,
        }
    }

    #[cfg(feature = "rdf-12")]
    #[inline]
    pub const fn direction(self) -> Option<BaseDirection> {
        match self.0 {
            LiteralRefContent::DirectionalLanguageTaggedString { direction, .. } => Some(direction),
            _ => None,
        }
    }

    #[inline]
    pub const fn datatype(self) -> NamedNodeRef<'a> {
        match self.0 {
            LiteralRefContent::String(_) => xsd::STRING,
            LiteralRefContent::LanguageTaggedString { .. } => rdf::LANG_STRING,
            #[cfg(feature = "rdf-12")]
            LiteralRefContent::DirectionalLanguageTaggedString { .. } => rdf::DIR_LANG_STRING,
            LiteralRefContent::TypedLiteral { datatype, .. } => datatype,
        }
    }

    #[inline]
    #[deprecated(note = "Plain literal concept is removed in RDF 1.1", since = "0.3.0")]
    pub const fn is_plain(self) -> bool {
        matches!(
            self.0,
            LiteralRefContent::String(_) | LiteralRefContent::LanguageTaggedString { .. }
        )
    }

    #[inline]
    pub fn into_owned(self) -> Literal {
        Literal(match self.0 {
            LiteralRefContent::String(value) => LiteralContent::String(value.to_owned()),
            LiteralRefContent::LanguageTaggedString { value, language } => {
                LiteralContent::LanguageTaggedString {
                    value: value.to_owned(),
                    language: language.to_owned(),
                }
            }
            #[cfg(feature = "rdf-12")]
            LiteralRefContent::DirectionalLanguageTaggedString {
                value,
                language,
                direction,
            } => LiteralContent::DirectionalLanguageTaggedString {
                value: value.to_owned(),
                language: language.to_owned(),
                direction,
            },
            LiteralRefContent::TypedLiteral { value, datatype } => LiteralContent::TypedLiteral {
                value: value.to_owned(),
                datatype: datatype.into_owned(),
            },
        })
    }

    #[cfg(not(feature = "rdf-12"))]
    #[inline]
    #[deprecated(
        note = "Use directly .value(), .datatype() and .language()",
        since = "0.3.0"
    )]
    pub const fn destruct(self) -> (&'a str, Option<NamedNodeRef<'a>>, Option<&'a str>) {
        match self.0 {
            LiteralRefContent::String(s) => (s, None, None),
            LiteralRefContent::LanguageTaggedString { value, language } => {
                (value, None, Some(language))
            }
            LiteralRefContent::TypedLiteral { value, datatype } => (value, Some(datatype), None),
        }
    }
}

impl fmt::Display for LiteralRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            LiteralRefContent::String(value) => print_quoted_str(value, f),
            LiteralRefContent::LanguageTaggedString { value, language } => {
                print_quoted_str(value, f)?;
                write!(f, "@{language}")
            }
            #[cfg(feature = "rdf-12")]
            LiteralRefContent::DirectionalLanguageTaggedString {
                value,
                language,
                direction,
            } => {
                print_quoted_str(value, f)?;
                write!(f, "@{language}--{direction}")
            }
            LiteralRefContent::TypedLiteral { value, datatype } => {
                print_quoted_str(value, f)?;
                write!(f, "^^{datatype}")
            }
        }
    }
}

impl<'a> From<&'a Literal> for LiteralRef<'a> {
    #[inline]
    fn from(node: &'a Literal) -> Self {
        node.as_ref()
    }
}

impl<'a> From<LiteralRef<'a>> for Literal {
    #[inline]
    fn from(node: LiteralRef<'a>) -> Self {
        node.into_owned()
    }
}

impl<'a> From<&'a str> for LiteralRef<'a> {
    #[inline]
    fn from(value: &'a str) -> Self {
        LiteralRef(LiteralRefContent::String(value))
    }
}

impl PartialEq<Literal> for LiteralRef<'_> {
    #[inline]
    fn eq(&self, other: &Literal) -> bool {
        *self == other.as_ref()
    }
}

impl PartialEq<LiteralRef<'_>> for Literal {
    #[inline]
    fn eq(&self, other: &LiteralRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

#[inline]
pub fn print_quoted_str(string: &str, f: &mut impl Write) -> fmt::Result {
    f.write_char('"')?;
    for c in string.chars() {
        match c {
            '\u{8}' => f.write_str("\\b"),
            '\t' => f.write_str("\\t"),
            '\n' => f.write_str("\\n"),
            '\u{C}' => f.write_str("\\f"),
            '\r' => f.write_str("\\r"),
            '"' => f.write_str("\\\""),
            '\\' => f.write_str("\\\\"),
            '\0'..='\u{1F}' | '\u{7F}' | '\u{FFFE}' | '\u{FFFF}' => {
                write!(f, "\\u{:04X}", u32::from(c))
            }
            _ => f.write_char(c),
        }?;
    }
    f.write_char('"')
}

#[cfg(feature = "rdf-12")]
#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub enum BaseDirection {
    Ltr,
    Rtl,
}

#[cfg(feature = "rdf-12")]
impl fmt::Display for BaseDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        })
    }
}

impl FromStr for Literal {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = Literal::read(s)?;
        if !left.is_empty() {
            return Err(Self::Err::msg("Invalid literal serialization"));
        }
        Ok(term)
    }
}

impl Literal {
    pub(crate) fn read(s: &str) -> Result<(Literal, &str), TermParseError> {
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
                            let (datatype, remain) = NamedNode::read(remain)?;
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
}
