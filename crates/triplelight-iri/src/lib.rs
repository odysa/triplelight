//! Independent RFC 3987 absolute IRI validation. Resolution is not implemented yet.
//! This work-in-progress crate is not integrated into TripleLight.
use std::{fmt, net::Ipv6Addr, str::FromStr};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Iri(String);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidIri;
impl fmt::Display for InvalidIri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("invalid absolute IRI") }
}
impl std::error::Error for InvalidIri {}

impl Iri {
    /// Validate the RFC 3987 IRI syntax without normalization or percent-decoding.
    pub fn parse(input: &str) -> Result<Self, InvalidIri> {
        validate_absolute(input)?;
        Ok(Self(input.to_owned()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl fmt::Display for Iri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
}
impl FromStr for Iri {
    type Err = InvalidIri;
    fn from_str(value: &str) -> Result<Self, Self::Err> { Self::parse(value) }
}

fn ucschar(c: char) -> bool {
    let n = c as u32;
    matches!(n, 0xa0..=0xd7ff | 0xf900..=0xfdcf | 0xfdf0..=0xffef | 0xe1000..=0xefffd)
        || (0x10000..=0xdfffd).contains(&n) && (n & 0xffff) <= 0xfffd
}
fn iprivate(c: char) -> bool { matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd) }
fn unreserved(c: char) -> bool { c.is_ascii_alphanumeric() || "-._~".contains(c) || ucschar(c) }
fn sub_delim(c: char) -> bool { "!$&'()*+,;=".contains(c) }

/// Validate a component and its percent escapes in one pass.
fn component(input: &str, allowed: impl Fn(char) -> bool) -> Result<(), InvalidIri> {
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            if !chars.next().is_some_and(|c| c.is_ascii_hexdigit()) ||
               !chars.next().is_some_and(|c| c.is_ascii_hexdigit()) { return Err(InvalidIri) }
        } else if !allowed(c) { return Err(InvalidIri) }
    }
    Ok(())
}
fn authority(input: &str) -> Result<(), InvalidIri> {
    let hostport = if let Some((user, hostport)) = input.rsplit_once('@') {
        component(user, |c| unreserved(c) || sub_delim(c) || c == ':')?;
        hostport
    } else { input };
    if let Some(rest) = hostport.strip_prefix('[') {
        let (address, suffix) = rest.split_once(']').ok_or(InvalidIri)?;
        if address.starts_with('v') || address.starts_with('V') {
            let (version, suffix) = address[1..].split_once('.').ok_or(InvalidIri)?;
            if version.is_empty() || !version.chars().all(|c| c.is_ascii_hexdigit()) || suffix.is_empty() ||
                !suffix.chars().all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c) || sub_delim(c) || c == ':') {
                return Err(InvalidIri);
            }
        } else if address.parse::<Ipv6Addr>().is_err() { return Err(InvalidIri) }
        if !suffix.is_empty() && !suffix.strip_prefix(':').is_some_and(|p| p.chars().all(|c| c.is_ascii_digit())) {
            return Err(InvalidIri);
        }
    } else {
        let (host, port) = match hostport.split_once(':') { Some((h, p)) => (h, Some(p)), None => (hostport, None) };
        if port.is_some_and(|p| !p.chars().all(|c| c.is_ascii_digit())) { return Err(InvalidIri) }
        component(host, |c| unreserved(c) || sub_delim(c))?;
    }
    Ok(())
}
fn validate_absolute(input: &str) -> Result<(), InvalidIri> {
    let (scheme, remainder) = input.split_once(':').ok_or(InvalidIri)?;
    let mut chars = scheme.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) ||
        !chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) { return Err(InvalidIri) }
    let (before_fragment, fragment) = match remainder.split_once('#') { Some((a, b)) => (a, Some(b)), None => (remainder, None) };
    if let Some(fragment) = fragment { component(fragment, |c| unreserved(c) || sub_delim(c) || ":@/?".contains(c))?; }
    let (before_query, query) = match before_fragment.split_once('?') { Some((a, b)) => (a, Some(b)), None => (before_fragment, None) };
    if let Some(query) = query { component(query, |c| unreserved(c) || iprivate(c) || sub_delim(c) || ":@/?".contains(c))?; }
    let path = if let Some(rest) = before_query.strip_prefix("//") {
        let (auth, path) = match rest.find('/') { Some(i) => (&rest[..i], &rest[i..]), None => (rest, "") };
        authority(auth)?;
        path
    } else { before_query };
    component(path, |c| unreserved(c) || sub_delim(c) || ":@/".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_valid_iris_without_normalization() {
        for iri in ["urn:example:猫", "https://user:pass@[2001:db8::1]:443/a%20b?x=1#frag", "file:///tmp/a", "a:", "https://example.org/?a?b"] {
            assert_eq!(Iri::parse(iri).unwrap().as_str(), iri);
        }
    }
    #[test]
    fn rejects_bad_iris() {
        for iri in ["relative/path", "1http:x", "https://a:port/", "https://a/%0g", "https://[::bad::]/", "urn:a#b#c", "http://a/b c", "urn:\u{e000}"] {
            assert!(Iri::parse(iri).is_err(), "{iri:?}");
        }
    }
}
