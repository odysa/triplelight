//! RFC 3987 absolute IRI validation and RFC 3986 reference resolution (task #45). Used by the SPARQL parser and the
//! `IRI()` function; `tests/differential.rs` qualifies it against `oxiri`, which is a dev-only comparator.
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
    pub fn into_string(self) -> String { self.0 }
    /// Resolve an IRI reference against this absolute base (RFC 3986 section 5.2). The reference must itself be a valid
    /// IRI reference. One deliberate departure, shared with `oxiri`: a reference with a scheme or an authority keeps
    /// its path as written (`x:/../g` stays `x:/../g`), where the RFC would remove its dot segments.
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

fn remove_last_segment(out: &mut String) {
    if let Some(i) = out.rfind('/') { out.truncate(i) } else { out.clear() }
}
/// RFC 3986 section 5.2.4, with one rule shared with `oxiri` so answers stay identical: without an authority, a `..`
/// that empties a rootless output also drops the slash that follows, so `urn:a:b` + `/../g` is `urn:g` (the RFC
/// gives `urn:/g`). A rooted path keeps its root.
fn remove_dot_segments(mut input: &str, has_authority: bool) -> String {
    let mut output = String::new();
    let pop = |output: &mut String, rest: &'static str| -> &'static str {
        let rooted = output.starts_with('/');
        remove_last_segment(output);
        if !has_authority && !rooted && output.is_empty() { "" } else { rest }
    };
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") { input = rest; }
        else if let Some(rest) = input.strip_prefix("./") { input = rest; }
        else if input.starts_with("/./") { input = &input[2..]; }
        else if input == "/." { input = "/"; }
        else if let Some(rest) = input.strip_prefix("/../") {
            input = if pop(&mut output, "/").is_empty() { rest } else { &input[3..] };
        }
        else if input == "/.." { input = pop(&mut output, "/"); }
        else if input == "." || input == ".." { input = ""; }
        else {
            let after_initial_slash = usize::from(input.starts_with('/'));
            let end = input[after_initial_slash..].find('/').map_or(input.len(), |i| i + after_initial_slash);
            output.push_str(&input[..end]); input = &input[end..];
        }
    }
    output
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
    } else if r.path.starts_with('/') {
        (b.authority, remove_dot_segments(r.path, b.authority.is_some()), r.query)
    } else {
        let merged = if b.authority.is_some() && b.path.is_empty() { format!("/{}", r.path) }
            else if let Some(i) = b.path.rfind('/') { format!("{}{}", &b.path[..=i], r.path) }
            else { r.path.to_owned() };
        (b.authority, remove_dot_segments(&merged, b.authority.is_some()), r.query)
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
