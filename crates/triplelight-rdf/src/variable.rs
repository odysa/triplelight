// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
use crate::syntax::{TermParseError, TermParseErrorKind, VariableNameParseError};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone, Hash)]
pub struct Variable {
    name: String,
}

impl Variable {
    pub fn new(name: impl Into<String>) -> Result<Self, VariableNameParseError> {
        let name = name.into();
        validate_variable_identifier(&name)?;
        Ok(Self::new_unchecked(name))
    }

    #[inline]
    pub fn new_unchecked(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.name
    }

    #[inline]
    pub fn into_string(self) -> String {
        self.name
    }

    #[inline]
    pub fn as_ref(&self) -> VariableRef<'_> {
        VariableRef { name: &self.name }
    }
}

impl fmt::Display for Variable {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Debug, Clone, Copy, Hash)]
pub struct VariableRef<'a> {
    name: &'a str,
}

impl<'a> VariableRef<'a> {
    pub fn new(name: &'a str) -> Result<Self, VariableNameParseError> {
        validate_variable_identifier(name)?;
        Ok(Self::new_unchecked(name))
    }

    #[inline]
    pub const fn new_unchecked(name: &'a str) -> Self {
        Self { name }
    }

    #[inline]
    pub const fn as_str(self) -> &'a str {
        self.name
    }

    #[inline]
    pub fn into_string(self) -> String {
        self.name.to_owned()
    }

    #[inline]
    pub fn into_owned(self) -> Variable {
        Variable {
            name: self.name.to_owned(),
        }
    }
}

impl fmt::Display for VariableRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?{}", self.name)
    }
}

impl<'a> From<&'a Variable> for VariableRef<'a> {
    #[inline]
    fn from(variable: &'a Variable) -> Self {
        variable.as_ref()
    }
}

impl<'a> From<VariableRef<'a>> for Variable {
    #[inline]
    fn from(variable: VariableRef<'a>) -> Self {
        variable.into_owned()
    }
}

impl PartialEq<Variable> for VariableRef<'_> {
    #[inline]
    fn eq(&self, other: &Variable) -> bool {
        *self == other.as_ref()
    }
}

impl PartialEq<VariableRef<'_>> for Variable {
    #[inline]
    fn eq(&self, other: &VariableRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

impl PartialOrd<Variable> for VariableRef<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Variable) -> Option<Ordering> {
        self.partial_cmp(&other.as_ref())
    }
}

impl PartialOrd<VariableRef<'_>> for Variable {
    #[inline]
    fn partial_cmp(&self, other: &VariableRef<'_>) -> Option<Ordering> {
        self.as_ref().partial_cmp(other)
    }
}

fn validate_variable_identifier(id: &str) -> Result<(), VariableNameParseError> {
    let mut chars = id.chars();
    let front = chars.next().ok_or(VariableNameParseError)?;
    match front {
        '0'..='9'
        | '_'
        | ':'
        | 'A'..='Z'
        | 'a'..='z'
        | '\u{00C0}'..='\u{00D6}'
        | '\u{00D8}'..='\u{00F6}'
        | '\u{00F8}'..='\u{02FF}'
        | '\u{0370}'..='\u{037D}'
        | '\u{037F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}' => (),
        _ => return Err(VariableNameParseError),
    }
    for c in chars {
        match c {
            '0'..='9'
            | '\u{00B7}'
            | '\u{0300}'..='\u{036F}'
            | '\u{203F}'..='\u{2040}'
            | '_'
            | 'A'..='Z'
            | 'a'..='z'
            | '\u{00C0}'..='\u{00D6}'
            | '\u{00D8}'..='\u{00F6}'
            | '\u{00F8}'..='\u{02FF}'
            | '\u{0370}'..='\u{037D}'
            | '\u{037F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}'
            | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}' => (),
            _ => return Err(VariableNameParseError),
        }
    }
    Ok(())
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
