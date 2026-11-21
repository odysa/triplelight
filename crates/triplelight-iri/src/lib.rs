//! RFC 3987 absolute IRI validation and RFC 3986 reference resolution (task #45). Used by the SPARQL parser and the
//! `IRI()` function; `tests/differential.rs` qualifies it against `oxiri`, which is a dev-only comparator.
//! Derived in part from oxiri (MIT); see THIRD_PARTY_NOTICES.md.
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
    /// `parse` without building the value: whether `input` is a valid absolute IRI.
    pub fn validate(input: &str) -> Result<(), InvalidIri> { validate_absolute(input) }
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn into_string(self) -> String { self.0 }
    /// The path component (RFC 3986 section 3.3): after the scheme and any authority, before `?` or `#`.
    pub fn path(&self) -> &str {
        let s = self.as_str();
        let rest = &s[s.find(':').map_or(0, |i| i + 1)..];
        let end = rest.find(['?', '#']).unwrap_or(rest.len());
        let rest = &rest[..end];
        match rest.strip_prefix("//") { Some(r) => r.find('/').map_or("", |i| &r[i..]), None => rest }
    }
    /// Resolve an IRI reference against this absolute base (RFC 3986 section 5.2). The reference must itself be a valid
    /// IRI reference. Resolution follows `oxiri` exactly so answers stay identical, which departs from RFC 3986 in
    /// three ways: a reference with a scheme or an authority keeps its path as written (`x:/../g`); dot segments
    /// inherited from the base path are kept (`http://h/a/../b/c` + `d` is `http://h/a/../b/d`); and without an
    /// authority a `..` that empties a rootless path drops the following slash (`urn:a:b` + `/../g` is `urn:g`).
    pub fn resolve(&self, reference: &str) -> Result<Self, InvalidIri> {
        resolve_reference(self.as_str(), reference)
    }
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
fn unreserved(c: char) -> bool { c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') || (!c.is_ascii() && ucschar(c)) }
fn sub_delim(c: char) -> bool { matches!(c, '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=') }

/// Validate a component and its percent escapes in one pass.
fn component(input: &str, allowed: impl Fn(char) -> bool) -> Result<(), InvalidIri> {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            if !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)) { return Err(InvalidIri) }
            i += 3;
        } else if b.is_ascii() {
            if !allowed(char::from(b)) { return Err(InvalidIri) }
            i += 1;
        } else {
            let c = input[i..].chars().next().ok_or(InvalidIri)?;
            if !allowed(c) { return Err(InvalidIri) }
            i += c.len_utf8();
        }
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

#[derive(Clone, Copy)]
struct Parts<'a> {
    scheme: Option<&'a str>, authority: Option<&'a str>, path: &'a str,
    query: Option<&'a str>, fragment: Option<&'a str>,
}
fn parts(input: &str) -> Result<Parts<'_>, InvalidIri> {
    let (prefix, fragment) = match input.split_once('#') { Some((a, b)) => (a, Some(b)), None => (input, None) };
    if fragment.is_some_and(|f| f.contains('#')) { return Err(InvalidIri) }
    let (prefix, query) = match prefix.split_once('?') { Some((a, b)) => (a, Some(b)), None => (prefix, None) };
    let colon = prefix.find(':');
    let slash = prefix.find('/');
    let scheme = colon.filter(|&c| slash.is_none_or(|s| c < s));
    let (scheme, remainder) = if let Some(i) = scheme {
        let candidate = &prefix[..i];
        let mut chars = candidate.chars();
        if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) ||
            !chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) { return Err(InvalidIri) }
        (Some(candidate), &prefix[i + 1..])
    } else { (None, prefix) };
    let (authority, path) = if let Some(rest) = remainder.strip_prefix("//") {
        match rest.find('/') { Some(i) => (Some(&rest[..i]), &rest[i..]), None => (Some(rest), "") }
    } else { (None, remainder) };
    Ok(Parts { scheme, authority, path, query, fragment })
}

/// Removes the last segment of a merged path, keeping its slash (`oxiri`'s rule): `/a/b` becomes `/a/`. Without a
/// slash the path empties, and gets a root slash only when there is an authority.
fn pop_segment(out: &mut String, has_authority: bool) {
    if let Some(i) = out.rfind('/') { out.truncate(i + 1) } else { out.clear(); if has_authority { out.push('/') } }
}

/// Appends a reference's relative path to `out` (the base directory) segment by segment, applying the dot-segment
/// rules only to segments read from the reference, the way `oxiri` does. Dot segments inherited from the base stay
/// as written (`http://h/a/../b/c` + `d` is `http://h/a/../b/d`); RFC 3986 section 5.2.4 would normalize the whole
/// merged path. Without an authority a popped rootless path loses its slash (`urn:a:b` + `/../g` is `urn:g`).
fn append_reference_path(out: &mut String, reference: &str, has_authority: bool) {
    let mut rest = reference;
    loop {
        let (segment, more) = match rest.find('/') { Some(i) => (&rest[..i], Some(&rest[i + 1..])), None => (rest, None) };
        out.push_str(segment);
        if out.ends_with("/..") || out == ".." {
            out.truncate(out.len() - 2);
            if out.ends_with('/') { out.pop(); }
            pop_segment(out, has_authority);
        } else if out.ends_with("/.") || out == "." {
            out.pop();
        } else if more.is_some() {
            out.push('/');
        }
        match more { Some(r) => rest = r, None => break }
    }
}
/// Validate an IRI reference (RFC 3987 `IRI-reference`): an absolute IRI, or a relative reference whose parts use
/// only their allowed characters. Checked before resolution, because removing dot segments can drop invalid text.
fn validate_reference(reference: &str, r: &Parts<'_>) -> Result<(), InvalidIri> {
    if r.scheme.is_some() { return validate_absolute(reference) }
    if let Some(auth) = r.authority { authority(auth)? }
    component(r.path, |c| unreserved(c) || sub_delim(c) || ":@/".contains(c))?;
    if let Some(query) = r.query { component(query, |c| unreserved(c) || iprivate(c) || sub_delim(c) || ":@/?".contains(c))? }
    if let Some(fragment) = r.fragment { component(fragment, |c| unreserved(c) || sub_delim(c) || ":@/?".contains(c))? }
    Ok(())
}
fn resolve_reference(base: &str, reference: &str) -> Result<Iri, InvalidIri> {
    let b = parts(base)?;
    let r = parts(reference)?;
    validate_reference(reference, &r)?;
    let scheme = r.scheme.or(b.scheme).ok_or(InvalidIri)?;
    // A reference with a scheme or an authority keeps its path as written (as `oxiri` does, so answers stay identical
    // after the switch); RFC 3986 section 5.2.2 would remove its dot segments too.
    let (auth, path, query) = if r.scheme.is_some() || r.authority.is_some() {
        (r.authority, r.path.to_owned(), r.query)
    } else if r.path.is_empty() {
        (b.authority, b.path.to_owned(), r.query.or(b.query))
    } else if let Some(after_root) = r.path.strip_prefix('/') {
        let mut path = String::from("/");
        append_reference_path(&mut path, after_root, b.authority.is_some());
        (b.authority, path, r.query)
    } else {
        let mut path = b.path.to_owned();
        pop_segment(&mut path, b.authority.is_some());
        append_reference_path(&mut path, r.path, b.authority.is_some());
        (b.authority, path, r.query)
    };
    // Without an authority, a path starting with "//" would be read back as one (RFC 3986 section 3.3): refuse it.
    if auth.is_none() && path.starts_with("//") { return Err(InvalidIri) }
    let mut out = format!("{scheme}:");
    if let Some(auth) = auth { out.push_str("//"); out.push_str(auth); }
    out.push_str(&path);
    if let Some(query) = query { out.push('?'); out.push_str(query); }
    if let Some(fragment) = r.fragment { out.push('#'); out.push_str(fragment); }
    Iri::parse(&out)
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

#[cfg(test)]
mod resolution_tests {
    use super::*;
    #[test]
    fn rfc3986_reference_resolution_examples() {
        let base = Iri::parse("http://a/b/c/d;p?q").unwrap();
        for (reference, expected) in [
            ("g", "http://a/b/c/g"), ("./g", "http://a/b/c/g"), ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"), ("//g", "http://g"), ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"), ("#s", "http://a/b/c/d;p?q#s"),
            ("", "http://a/b/c/d;p?q"), (".", "http://a/b/c/"), ("..", "http://a/b/"),
            ("../g", "http://a/b/g"), ("../../g", "http://a/g"), ("g/../h", "http://a/b/c/h"),
        ] { assert_eq!(base.resolve(reference).unwrap().as_str(), expected, "{reference}"); }
    }
}

#[cfg(test)]
mod rfc3986_abnormal_examples {
    use super::Iri;
    #[test]
    fn reference_resolution_abnormal_examples() {
        let base = Iri::parse("http://a/b/c/d;p?q").unwrap();
        for (reference, expected) in [
            ("../../../g", "http://a/g"), ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"), ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."), (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."), ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"), ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"), ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("g?y/./x", "http://a/b/c/g?y/./x"),
            ("g?y/../x", "http://a/b/c/g?y/../x"),
            ("g#s/./x", "http://a/b/c/g#s/./x"),
            ("g#s/../x", "http://a/b/c/g#s/../x"),
        ] {
            assert_eq!(base.resolve(reference).unwrap().as_str(), expected, "reference {reference}");
        }
    }
}

#[cfg(test)]
mod unicode_and_escape_tests {
    use super::Iri;
    #[test]
    fn unicode_and_percent_encoding_are_validated_without_normalizing() {
        for iri in ["urn:é", "urn:猫", "urn:a%20b", "urn:a%2fb", "http://example/?x=\u{e000}"] {
            assert_eq!(Iri::parse(iri).unwrap().as_str(), iri);
        }
        for iri in ["urn:a%", "urn:a%0", "urn:a%xz", "urn:a|b", "urn:a<b", "urn:a\u{e000}", "urn:a#\u{e000}"] {
            assert!(Iri::parse(iri).is_err(), "{iri:?}");
        }
    }
    #[test]
    fn reference_resolution_retains_encoded_dots() {
        let base = Iri::parse("http://example/a/b/").unwrap();
        assert_eq!(base.resolve("%2e%2e/x").unwrap().as_str(), "http://example/a/b/%2e%2e/x");
        assert!(base.resolve("1invalid:relative").is_err());
    }
}
