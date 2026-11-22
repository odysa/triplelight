// Derived from oxrdf 0.3.5 (MIT); see THIRD_PARTY_NOTICES.md.
#![allow(clippy::host_endian_bytes)] // We use it to go around 16 bytes alignment of u128
use std::io::Write;
use std::{fmt, str};

#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub struct BlankNode(BlankNodeContent);

#[derive(PartialEq, Eq, Debug, Clone, Hash)]
enum BlankNodeContent {
    Named(String),
    Anonymous { id: [u8; 16], str: IdStr },
}

impl BlankNode {
    pub fn new(id: impl Into<String>) -> Result<Self, BlankNodeIdParseError> {
        let id = id.into();
        validate_blank_node_identifier(&id)?;
        Ok(Self::new_unchecked(id))
    }

    #[inline]
    pub fn new_unchecked(id: impl Into<String>) -> Self {
        let id = id.into();
        if let Some(numerical_id) = to_integer_id(&id) {
            Self::new_from_unique_id(numerical_id)
        } else {
            Self(BlankNodeContent::Named(id))
        }
    }

    #[inline]
    pub fn new_from_unique_id(id: u128) -> Self {
        Self(BlankNodeContent::Anonymous {
            id: id.to_ne_bytes(),
            str: IdStr::new(id),
        })
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            BlankNodeContent::Named(id) => id,
            BlankNodeContent::Anonymous { str, .. } => str.as_str(),
        }
    }

    #[inline]
    pub fn into_string(self) -> String {
        match self.0 {
            BlankNodeContent::Named(id) => id,
            BlankNodeContent::Anonymous { str, .. } => str.as_str().to_owned(),
        }
    }

    #[inline]
    pub fn as_ref(&self) -> BlankNodeRef<'_> {
        BlankNodeRef(match &self.0 {
            BlankNodeContent::Named(id) => BlankNodeRefContent::Named(id.as_str()),
            BlankNodeContent::Anonymous { id, str } => BlankNodeRefContent::Anonymous {
                id: *id,
                str: str.as_str(),
            },
        })
    }
}

impl fmt::Display for BlankNode {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl Default for BlankNode {
    #[inline]
    fn default() -> Self {
        // We ensure the ID does not start with a number to be also valid with RDF/XML
        loop {
            let id = random_u128();
            let str = IdStr::new(id);
            if matches!(str.as_str().as_bytes().first(), Some(b'a'..=b'f')) {
                return Self(BlankNodeContent::Anonymous {
                    id: id.to_ne_bytes(),
                    str,
                });
            }
        }
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Hash)]
pub struct BlankNodeRef<'a>(BlankNodeRefContent<'a>);

#[derive(PartialEq, Eq, Debug, Clone, Copy, Hash)]
enum BlankNodeRefContent<'a> {
    Named(&'a str),
    Anonymous { id: [u8; 16], str: &'a str },
}

impl<'a> BlankNodeRef<'a> {
    pub fn new(id: &'a str) -> Result<Self, BlankNodeIdParseError> {
        validate_blank_node_identifier(id)?;
        Ok(Self::new_unchecked(id))
    }

    #[inline]
    pub fn new_unchecked(id: &'a str) -> Self {
        if let Some(numerical_id) = to_integer_id(id) {
            Self(BlankNodeRefContent::Anonymous {
                id: numerical_id.to_ne_bytes(),
                str: id,
            })
        } else {
            Self(BlankNodeRefContent::Named(id))
        }
    }

    #[inline]
    pub const fn as_str(self) -> &'a str {
        match self.0 {
            BlankNodeRefContent::Named(id) => id,
            BlankNodeRefContent::Anonymous { str, .. } => str,
        }
    }

    #[inline]
    pub const fn unique_id(&self) -> Option<u128> {
        match self.0 {
            BlankNodeRefContent::Named(_) => None,
            BlankNodeRefContent::Anonymous { id, .. } => Some(u128::from_ne_bytes(id)),
        }
    }

    #[inline]
    pub fn into_owned(self) -> BlankNode {
        BlankNode(match self.0 {
            BlankNodeRefContent::Named(id) => BlankNodeContent::Named(id.to_owned()),
            BlankNodeRefContent::Anonymous { id, .. } => BlankNodeContent::Anonymous {
                id,
                str: IdStr::new(u128::from_ne_bytes(id)),
            },
        })
    }
}

impl fmt::Display for BlankNodeRef<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "_:{}", self.as_str())
    }
}

impl<'a> From<&'a BlankNode> for BlankNodeRef<'a> {
    #[inline]
    fn from(node: &'a BlankNode) -> Self {
        node.as_ref()
    }
}

impl<'a> From<BlankNodeRef<'a>> for BlankNode {
    #[inline]
    fn from(node: BlankNodeRef<'a>) -> Self {
        node.into_owned()
    }
}

impl PartialEq<BlankNode> for BlankNodeRef<'_> {
    #[inline]
    fn eq(&self, other: &BlankNode) -> bool {
        *self == other.as_ref()
    }
}

impl PartialEq<BlankNodeRef<'_>> for BlankNode {
    #[inline]
    fn eq(&self, other: &BlankNodeRef<'_>) -> bool {
        self.as_ref() == *other
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Hash)]
struct IdStr([u8; 32]);

impl IdStr {
    #[inline]
    fn new(id: u128) -> Self {
        let mut str = [0; 32];
        write!(&mut str[..], "{id:x}").unwrap();
        Self(str)
    }

    #[inline]
    fn as_str(&self) -> &str {
        let len = self.0.iter().position(|x| x == &0).unwrap_or(32);
        str::from_utf8(&self.0[..len]).unwrap()
    }
}

fn validate_blank_node_identifier(id: &str) -> Result<(), BlankNodeIdParseError> {
    let mut chars = id.chars();
    let front = chars.next().ok_or(BlankNodeIdParseError)?;
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
        _ => return Err(BlankNodeIdParseError),
    }
    for c in chars {
        match c {
            '.' // validated later
            | '-'
            | '0'..='9'
            | '\u{00B7}'
            | '\u{0300}'..='\u{036F}'
            | '\u{203F}'..='\u{2040}'
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
            _ => return Err(BlankNodeIdParseError),
        }
    }

    // Could not end with a dot
    if id.ends_with('.') {
        Err(BlankNodeIdParseError)
    } else {
        Ok(())
    }
}

#[inline]
fn to_integer_id(id: &str) -> Option<u128> {
    let digits = id.as_bytes();
    let mut value: u128 = 0;
    if let None | Some(b'0') = digits.first() {
        return None; // No empty string or leading zeros
    }
    for digit in digits {
        value = value.checked_mul(16)?.checked_add(
            match *digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                _ => return None,
            }
            .into(),
        )?;
    }
    Some(value)
}

#[derive(Debug)]
pub struct BlankNodeIdParseError;

impl fmt::Display for BlankNodeIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("The blank node identifier is invalid")
    }
}

impl std::error::Error for BlankNodeIdParseError {}

/// The next blank-node ID of this thread, in place of `rand::random`: a 128-bit bijective mix of a counter that
/// starts at a random seed (from the OS or the JS host), so IDs never repeat within a thread and threads, processes
/// and page loads start from independent seeds.
fn random_u128() -> u128 {
    thread_local! {
        static NEXT: std::cell::Cell<u128> = std::cell::Cell::new({
            let mut seed = [0u8; 16];
            getrandom::fill(&mut seed).expect("no random source for blank-node labels");
            u128::from_ne_bytes(seed)
        });
    }
    let mut x = NEXT.with(|next| {
        let x = next.get();
        next.set(x.wrapping_add(1));
        x
    });
    // Each step is invertible (xor-shift right, multiply by an odd constant), so distinct counters give distinct IDs.
    x ^= x >> 67;
    x = x.wrapping_mul(0x2d35_8dcc_aa6c_78a5_8bb8_4b93_962e_acc9);
    x ^= x >> 61;
    x = x.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835);
    x ^ (x >> 64)
}
