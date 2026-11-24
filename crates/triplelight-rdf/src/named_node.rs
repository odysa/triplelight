// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
use crate::syntax::{read_hexa_char, TermParseError, TermParseErrorKind};
use std::borrow::Cow;
use std::str::FromStr;
use triplelight_iri::Iri;

/// An invalid IRI (`oxiri::IriParseError` in oxrdf).
pub type IriParseError = triplelight_iri::InvalidIri;
use std::cmp::Ordering;
use std::fmt;

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone, Hash)]
pub struct NamedNode {
    iri: String,
}

impl NamedNode {
    pub fn new(iri: impl Into<String>) -> Result<Self, IriParseError> {
        let iri = iri.into();
        Iri::validate(&iri)?;
        Ok(Self::new_unchecked(iri))
    }

    #[inline]
    pub fn new_unchecked(iri: impl Into<String>) -> Self {
        Self { iri: iri.into() }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        self.iri.as_str()
    }

    #[inline]
    pub fn into_string(self) -> String {
        self.iri
    }

    #[inline]
    pub fn as_ref(&self) -> NamedNodeRef<'_> {
        NamedNodeRef::new_unchecked(&self.iri)
    }
}

impl fmt::Display for NamedNode {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl PartialEq<str> for NamedNode {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<NamedNode> for str {
    #[inline]
    fn eq(&self, other: &NamedNode) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<&str> for NamedNode {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<NamedNode> for &str {
    #[inline]
    fn eq(&self, other: &NamedNode) -> bool {
        *self == other
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone, Copy, Hash)]
pub struct NamedNodeRef<'a> {
    iri: &'a str,
}

impl<'a> NamedNodeRef<'a> {
    pub fn new(iri: &'a str) -> Result<Self, IriParseError> {
        Iri::validate(iri)?;
        Ok(Self::new_unchecked(iri))
    }

    #[inline]
    pub const fn new_unchecked(iri: &'a str) -> Self {
        Self { iri }
    }

    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.iri
    }

    #[inline]
    pub fn into_owned(self) -> NamedNode {
        NamedNode::new_unchecked(self.iri)
    }
}

impl fmt::Display for NamedNodeRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.as_str())
    }
}

impl From<NamedNodeRef<'_>> for NamedNode {
    #[inline]
    fn from(node: NamedNodeRef<'_>) -> Self {
        node.into_owned()
    }
}

impl<'a> From<&'a NamedNode> for NamedNodeRef<'a> {
    #[inline]
    fn from(node: &'a NamedNode) -> Self {
        node.as_ref()
    }
}

impl PartialEq<NamedNode> for NamedNodeRef<'_> {
    #[inline]
    fn eq(&self, other: &NamedNode) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<NamedNodeRef<'_>> for NamedNode {
    #[inline]
    fn eq(&self, other: &NamedNodeRef<'_>) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<str> for NamedNodeRef<'_> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<NamedNodeRef<'_>> for str {
    #[inline]
    fn eq(&self, other: &NamedNodeRef<'_>) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<&str> for NamedNodeRef<'_> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl PartialEq<NamedNodeRef<'_>> for &str {
    #[inline]
    fn eq(&self, other: &NamedNodeRef<'_>) -> bool {
        *self == other
    }
}

impl PartialOrd<NamedNode> for NamedNodeRef<'_> {
    #[inline]
    fn partial_cmp(&self, other: &NamedNode) -> Option<Ordering> {
        self.partial_cmp(&other.as_ref())
    }
}

impl PartialOrd<NamedNodeRef<'_>> for NamedNode {
    #[inline]
    fn partial_cmp(&self, other: &NamedNodeRef<'_>) -> Option<Ordering> {
        self.as_ref().partial_cmp(other)
    }
}

impl FromStr for NamedNode {
    type Err = TermParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (term, left) = NamedNode::read(s)?;
        if !left.is_empty() {
            return Err(Self::Err::msg(
                "Named node serialization should end with a >",
            ));
        }
        Ok(term)
    }
}

impl NamedNode {
    pub(crate) fn read(s: &str) -> Result<(NamedNode, &str), TermParseError> {
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
}
