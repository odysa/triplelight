//! Differential qualification against `oxiri` (task #45): validation and RFC 3986 reference resolution must agree
//! on a curated edge-case list and on a large deterministic random corpus.
use triplelight_iri::Iri;

fn ours_parse(s: &str) -> Option<String> { Iri::parse(s).ok().map(|i| i.as_str().to_owned()) }
fn theirs_parse(s: &str) -> Option<String> { oxiri::Iri::parse(s.to_owned()).ok().map(|i| i.into_inner()) }
fn ours_resolve(base: &str, r: &str) -> Option<String> { Iri::parse(base).ok()?.resolve(r).ok().map(|i| i.as_str().to_owned()) }
fn theirs_resolve(base: &str, r: &str) -> Option<String> {
    oxiri::Iri::parse(base.to_owned()).ok()?.resolve(r).ok().map(|i| i.into_inner())
}
/// RFC 3986 section 5.2, transcribed literally (5.2.1 parse, 5.2.2 transform, 5.2.3 merge, 5.2.4 remove_dot_segments,
/// 5.3 recompose), independent of the crate's code. Inputs are assumed valid; the crate checks validity separately.
mod rfc {
    struct P<'a> { scheme: Option<&'a str>, authority: Option<&'a str>, path: &'a str, query: Option<&'a str>, fragment: Option<&'a str> }
    /// Appendix B's regular expression `^(([^:/?#]+):)?(//([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?`, by hand.
    fn split(s: &str) -> P<'_> {
        let mut rest = s;
        let mut scheme = None;
        if let Some(i) = rest.find(|c| ":/?#".contains(c)) && rest.as_bytes()[i] == b':' && i > 0 { scheme = Some(&rest[..i]); rest = &rest[i + 1..] }
        let mut authority = None;
        if let Some(r) = rest.strip_prefix("//") { let i = r.find(|c| "/?#".contains(c)).unwrap_or(r.len()); authority = Some(&r[..i]); rest = &r[i..] }
        let i = rest.find(|c| "?#".contains(c)).unwrap_or(rest.len());
        let (path, mut rest) = (&rest[..i], &rest[i..]);
        let mut query = None;
        if let Some(r) = rest.strip_prefix('?') { let i = r.find('#').unwrap_or(r.len()); query = Some(&r[..i]); rest = &r[i..] }
        let fragment = rest.strip_prefix('#');
        P { scheme, authority, path, query, fragment }
    }
    fn remove_dot_segments(input: &str) -> String {
        let (mut input, mut output) = (input.to_owned(), String::new());
        while !input.is_empty() {
            if input.starts_with("../") { input.drain(..3); }
            else if input.starts_with("./") { input.drain(..2); }
            else if input.starts_with("/./") { input.replace_range(..3, "/"); }
            else if input == "/." { input = "/".into(); }
            else if input.starts_with("/../") { input.replace_range(..4, "/"); output.truncate(output.rfind('/').unwrap_or(0)); }
            else if input == "/.." { input = "/".into(); output.truncate(output.rfind('/').unwrap_or(0)); }
            else if input == "." || input == ".." { input.clear(); }
            else {
                let start = usize::from(input.starts_with('/'));
                let end = input[start..].find('/').map_or(input.len(), |i| i + start);
                output.push_str(&input[..end]);
                input.drain(..end);
            }
        }
        output
    }
    pub fn resolve(base: &str, reference: &str) -> String {
        let (b, r) = (split(base), split(reference));
        let (scheme, authority, path, query);
        if r.scheme.is_some() {
            (scheme, authority, path, query) = (r.scheme, r.authority, remove_dot_segments(r.path), r.query);
        } else {
            if r.authority.is_some() {
                (authority, path, query) = (r.authority, remove_dot_segments(r.path), r.query);
            } else {
                if r.path.is_empty() {
                    path = b.path.to_owned();
                    query = if r.query.is_some() { r.query } else { b.query };
                } else {
                    path = if r.path.starts_with('/') { remove_dot_segments(r.path) } else {
                        let merged = if b.authority.is_some() && b.path.is_empty() { format!("/{}", r.path) }
                            else { match b.path.rfind('/') { Some(i) => format!("{}{}", &b.path[..=i], r.path), None => r.path.to_owned() } };
                        remove_dot_segments(&merged)
                    };
                    query = r.query;
                }
                authority = b.authority;
            }
            scheme = b.scheme;
        }
        let mut out = String::new();
        if let Some(s) = scheme { out.push_str(s); out.push(':') }
        if let Some(a) = authority { out.push_str("//"); out.push_str(a) }
        out.push_str(&path);
        if let Some(q) = query { out.push('?'); out.push_str(q) }
        if let Some(f) = r.fragment { out.push('#'); out.push_str(f) }
        out
    }
}

/// Checks one resolution: the result (or refusal) must equal `oxiri`'s exactly. Returns whether that result differs from
/// the literal RFC 3986 algorithm, which is counted and reported, not failed.
fn check_resolve(base: &str, r: &str) -> bool {
    let (ours, theirs) = (ours_resolve(base, r), theirs_resolve(base, r));
    assert_eq!(ours, theirs, "resolve {base:?} {r:?}");
    ours.is_some_and(|o| o != rfc::resolve(base, r))
}

const CURATED: &[&str] = &[
    "http://example.org/", "http://example.org", "https://a/b/c/d;p?q", "urn:isbn:0451450523", "mailto:a@b.c",
    "file:///tmp/a", "file://host/share", "a:", "a:b", "a:/", "a://", "a:///", "a:////", "http://[::1]/", "http://[::1]:80/",
    "http://[2001:db8::7]/c=GB?objectClass?one", "http://[v7.fe80::1]/", "http://[v.x]/", "http://[vg.x]/", "http://[]/",
    "http://[::1", "http://::1/", "http://a:/", "http://a:80", "http://a:8a/", "http://@a/", "http://u:p@a/", "http://u@p@a/",
    "http://a/%", "http://a/%2", "http://a/%2G", "http://a/%2f", "http://a/%C3%A9", "http://a/é", "http://a/猫",
    "http://a/\u{e000}", "http://a/?\u{e000}", "http://a/#\u{e000}", "http://a/\u{10ffff}", "http://a/\u{fffe}",
    "http://a/ b", "http://a/\"", "http://a/<", "http://a/>", "http://a/\\", "http://a/^", "http://a/`", "http://a/{", "http://a/|", "http://a/}",
    "http://a/#a#b", "http://a/??", "http://a/?#", "http://a/#?", "http://a/b//c", "http://a/./b/../c",
    "HTTP://A/", "h+t-t.p://a/", "1a:b", "+a:b", ":b", "a b:c", "http://a b/", "http://a%20b/", "http://ä/", "http://a.b-c_d~e/",
    "http://a/!$&'()*+,;=:@", "tag:yaml.org,2002:int", "urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type", "http://www.Department0.University0.edu/GraduateStudent1",
    "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/dataFromProducer1/Product1", "", "#", "?", "/", "//",
];

const BASES: &[&str] = &[
    "http://a/b/c/d;p?q", "http://a", "http://a/", "http://a/b", "file:///tmp/x/y", "urn:a:b", "a:b/c/d", "http://u@[::1]:8/x/y/?z#w",
    "http://example.org/a/b/c/", "tag:x", "a:", "a:/", "a://h",
    // Bases with dot segments of their own (kept as written by oxiri when a reference is merged in).
    "http://h/a/../b/c", "http://h/a/./b/c", "http://h/./", "http://h/../", "http://h/a/..", "http://h/a/.", "http://h/..",
    "http://h/a/b/../../c/d", "urn:a/../b", "urn:./a", "urn:..", "a:../b/c", "file:///x/./y/../z?q#f",
];

const REFS: &[&str] = &[
    "", ".", "..", "./", "../", "/", "//", "///", "g", "./g", "g/", "/g", "//g", "?y", "g?y", "#s", "g#s", "g?y#s", ";x", "g;x",
    "g;x?y#s", "../g", "../..", "../../", "../../g", "../../../g", "../../../../g", "/./g", "/../g", "g.", ".g", "g..", "..g",
    "./../g", "./g/.", "g/./h", "g/../h", "g;x=1/./y", "g;x=1/../y", "g?y/./x", "g?y/../x", "g#s/./x", "g#s/../x", "http:g",
    "http:", "x:", "x:/../g", "//h/../g", "%2e%2e/x", "a/b/../../..", "/a/b/../../..", "?", "#", "?#", "é", "猫/../x", "a b",
    "1x:y", "a:b:c", "./a:b", "//[::1]/x", "//u@h:1/p",
];

#[test]
fn curated_validation_agrees() {
    for s in CURATED { assert_eq!(ours_parse(s), theirs_parse(s), "parse {s:?}") }
}

/// `path()` as `oxiri` reports it (the Turtle parser uses it to decide when a prefixed name needs revalidation).
#[test]
fn paths_agree() {
    for s in CURATED.iter().chain(BASES) {
        if let (Ok(a), Ok(b)) = (Iri::parse(s), oxiri::Iri::parse(s.to_string())) { assert_eq!(a.path(), b.path(), "{s:?}") }
    }
}

#[test]
fn curated_resolution_agrees() {
    let mut oxiri_differs = Vec::new();
    for b in BASES {
        for r in REFS.iter().chain(CURATED) { if check_resolve(b, r) { oxiri_differs.push(format!("{b} + {r}")) } }
    }
    // Shared departures from RFC 3986 (see `Iri::resolve`).
    eprintln!("curated cases where both differ from RFC 3986: {}", oxiri_differs.len());
    for case in oxiri_differs.iter().take(20) { eprintln!("  {case}") }
}

/// Random strings over an alphabet weighted towards the characters that matter to the grammar.
#[test]
fn random_corpus_agrees() {
    const PIECES: &[&str] = &[
        "a", "b", "z", "Z", "0", "9", "-", ".", "_", "~", "!", "$", "&", "'", "(", ")", "*", "+", ",", ";", "=", ":", "@", "/",
        "?", "#", "[", "]", "%", "%2", "%2e", "%G", "%41", " ", "\"", "<", ">", "\\", "^", "`", "{", "|", "}", "é", "猫",
        "\u{a0}", "\u{e000}", "\u{fdd0}", "\u{fffd}", "\u{10000}", "\u{1fffe}", "\u{e1000}", "\u{f0000}", "\u{10fffd}",
        "http:", "urn:", "a:", "//", "../", "./", "/..", "/.", "::", "[::1]", "[v1.x]", "v", "\u{7f}", "\u{0}", "\n",
    ];
    let mut rng = 0x243f6a8885a308d3u64;
    let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let make = |next: &mut dyn FnMut() -> u64| -> String {
        let len = (next() % 10) as usize;
        (0..len).map(|_| PIECES[(next() % PIECES.len() as u64) as usize]).collect()
    };
    let (mut valid, mut resolved, mut differs) = (0usize, 0usize, 0usize);
    for i in 0..200_000 {
        let s = make(&mut next);
        let candidate = if i % 2 == 0 { format!("http://{s}") } else { s.clone() };
        let (a, b) = (ours_parse(&candidate), theirs_parse(&candidate));
        assert_eq!(a, b, "parse {candidate:?}");
        if a.is_some() { valid += 1 }
        // Half the time a random base too, so inherited dot segments and odd base paths are exercised.
        let random_base = format!("http://h/{}", make(&mut next));
        let base = if i % 4 == 1 && Iri::parse(&random_base).is_ok() { random_base.as_str() } else { BASES[(next() % BASES.len() as u64) as usize] };
        if check_resolve(base, &s) { differs += 1 }
        if ours_resolve(base, &s).is_some() { resolved += 1 }
    }
    // The corpus must exercise both outcomes, or the agreement says little.
    eprintln!("random corpus: {valid} valid, {resolved} resolved, {differs} where both differ from RFC 3986");
    assert!(valid > 10_000 && resolved > 20_000, "valid {valid}, resolved {resolved}");
}

#[test]
fn dot_segments_inherited_from_the_base_are_kept() {
    let base = Iri::parse("http://h/a/../b/c").unwrap();
    assert_eq!(base.resolve("d").unwrap().as_str(), "http://h/a/../b/d");
    assert_eq!(base.resolve("../d").unwrap().as_str(), "http://h/a/../d");
}

#[test]
fn references_with_a_scheme_or_authority_keep_their_path() {
    let base = Iri::parse("http://a/b/c/d;p?q").unwrap();
    assert_eq!(base.resolve("x:/../g").unwrap().as_str(), "x:/../g");
    assert_eq!(base.resolve("//h/../g").unwrap().as_str(), "http://h/../g");
    assert_eq!(base.resolve("g:h").unwrap().as_str(), "g:h");
}
