//! RDF 1.2 differential tests for `triplelight-rdf` against oxrdf, both with their `rdf-12` features on.
//! A separate package (its own `[workspace]`), because oxrdf's `rdf-12` feature would otherwise unify into the
//! workspace's dev-dependencies and break oxttl and core's comparators. The RDF 1.1 cases from
//! `crates/triplelight-rdf/tests/differential.rs` are repeated here and must still agree with the feature on.
//! Differential qualification of the term types against oxrdf 0.3.5 (task #45, step 4): every constructor accepts
#![allow(deprecated)] // `is_plain` is deprecated in oxrdf too; it is compared like every other accessor.
//! and rejects the same input, and accessors, `Display`, equality and ordering agree, on curated and random input.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use triplelight_rdf as tl;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn below(&mut self, n: usize) -> usize { (self.next() % n as u64) as usize }
    /// A short string over `alphabet`, with an occasional random code point.
    fn string(&mut self, alphabet: &[&str], max: usize) -> String {
        let mut s = String::new();
        for _ in 0..self.below(max + 1) {
            if self.below(12) == 0 {
                let c = loop { if let Some(c) = char::from_u32((self.next() % 0x11_0000) as u32) { break c } };
                s.push(c)
            } else {
                s.push_str(alphabet[self.below(alphabet.len())])
            }
        }
        s
    }
}

const LABEL: &[&str] = &["a", "b", "f", "0", "1", "9", "A", "_", "-", ".", ":", "é", "·", "\u{300}", "‿", " ", "x", "Z", "00", "ab12"];
const LANG: &[&str] = &["en", "-", "GB", "x", "X", "1", "a", "zh", "Hant", "419", "i", "lojban", "art", "abcdefghi", "_"];
const IRI: &[&str] = &["http:", "//", "a", "/", "?", "#", "%20", "%", "é", " ", "<", ">", "\"", "{", "|", "\\", "^", "`", "[", "]", "@", ":", "1", "urn:", "x"];
const TEXT: &[&str] = &["a", "\"", "\\", "\n", "\r", "\t", "\u{8}", "\u{c}", "\u{0}", "\u{1f}", "\u{7f}", "\u{80}", "\u{fffe}", "\u{ffff}", "é", "😀", " ", "'"];

fn hash<T: Hash>(t: &T) -> u64 { let mut h = DefaultHasher::new(); t.hash(&mut h); h.finish() }

#[test]
fn named_nodes_agree() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let curated = ["http://a/b", "urn:x", "http://a/b c", "", "a", "http://[::1]/", "http://a/%zz", "HTTP://A", "http://例.jp/é"];
    let mut nodes = Vec::new();
    for s in curated.iter().map(|s| s.to_string()).chain((0..200_000).map(|_| rng.string(IRI, 8))) {
        let (ours, theirs) = (tl::NamedNode::new(s.clone()), oxrdf::NamedNode::new(s.clone()));
        assert_eq!(ours.is_ok(), theirs.is_ok(), "{s:?}");
        assert_eq!(tl::NamedNodeRef::new(&s).is_ok(), theirs.is_ok(), "{s:?}");
        if let (Ok(a), Ok(b)) = (ours, theirs) {
            assert_eq!((a.as_str(), a.to_string()), (b.as_str(), b.to_string()));
            if nodes.len() < 2000 { nodes.push((a, b)) }
        }
    }
    for (a, b) in &nodes {
        for (c, d) in nodes.iter().take(200) { assert_eq!(a.cmp(c), b.cmp(d)); assert_eq!(a == c, b == d) }
    }
}

#[test]
fn blank_nodes_agree() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let curated = ["a", "abc", "0", "00", "1a", "ff", "0f", "a.", "a.b", ":", "-a", "_a", "a-", "é", "a b", "", "deadbeef", "DEADBEEF",
        "ffffffffffffffffffffffffffffffff", "1ffffffffffffffffffffffffffffffff", "123456789abcdef0123456789abcdef0"];
    let mut nodes = Vec::new();
    for s in curated.iter().map(|s| s.to_string()).chain((0..200_000).map(|_| rng.string(LABEL, 6))) {
        let (ours, theirs) = (tl::BlankNode::new(s.clone()), oxrdf::BlankNode::new(s.clone()));
        assert_eq!(ours.is_ok(), theirs.is_ok(), "{s:?}");
        let (a, b) = (tl::BlankNode::new_unchecked(s.clone()), oxrdf::BlankNode::new_unchecked(s.clone()));
        assert_eq!((a.as_str(), a.to_string(), a.as_ref().unique_id()), (b.as_str(), b.to_string(), b.as_ref().unique_id()), "{s:?}");
        if nodes.len() < 3000 { nodes.push((a, b)) }
    }
    for id in [0u128, 1, 15, 16, 0xabc, u128::MAX, 1 << 127] {
        let (a, b) = (tl::BlankNode::new_from_unique_id(id), oxrdf::BlankNode::new_from_unique_id(id));
        assert_eq!((a.as_str(), a.as_ref().unique_id()), (b.as_str(), b.as_ref().unique_id()));
        nodes.push((a, b));
    }
    // Equality is on the label in both, whichever representation a node has.
    for (a, b) in &nodes { for (c, d) in nodes.iter().take(300) { assert_eq!(a == c, b == d, "{a} {c}") } }
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10_000 {
        let b = tl::BlankNode::default();
        let s = b.as_str();
        assert!(matches!(s.as_bytes()[0], b'a'..=b'f') && s.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{s}");
        assert!(oxrdf::BlankNode::new(s).is_ok() && b.as_ref().unique_id().is_some());
        assert!(seen.insert(s.to_owned()), "repeated default label {s}");
    }
}

#[test]
fn variables_agree() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut vars = Vec::new();
    for s in ["x", ":x", "1", "_", "a.b", "a-b", "é", "", "a b", "·a", "a·"].iter().map(|s| s.to_string()).chain((0..200_000).map(|_| rng.string(LABEL, 5))) {
        let (ours, theirs) = (tl::Variable::new(s.clone()), oxrdf::Variable::new(s.clone()));
        assert_eq!(ours.is_ok(), theirs.is_ok(), "{s:?}");
        if let (Ok(a), Ok(b)) = (ours, theirs) {
            assert_eq!((a.as_str(), a.to_string()), (b.as_str(), b.to_string()));
            if vars.len() < 2000 { vars.push((a, b)) }
        }
    }
    for (a, b) in &vars { for (c, d) in vars.iter().take(200) { assert_eq!(a.cmp(c), b.cmp(d)) } }
}

fn same(a: &tl::Literal, b: &oxrdf::Literal) {
    assert_eq!((a.value(), a.language(), a.datatype().as_str(), a.is_plain(), a.to_string()),
        (b.value(), b.language(), b.datatype().as_str(), b.is_plain(), b.to_string()));
}

#[test]
fn literals_agree() {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    let mut text: Vec<String> = (0..0x100u32).chain([0xfffe, 0xffff, 0x10ffff, 0x2028]).filter_map(char::from_u32).map(String::from).collect();
    text.extend((0..100_000).map(|_| rng.string(TEXT, 6)));
    let xsd_string = "http://www.w3.org/2001/XMLSchema#string";
    let mut lits = Vec::new();
    for v in &text {
        let pairs = [
            (tl::Literal::new_simple_literal(v.clone()), oxrdf::Literal::new_simple_literal(v.clone())),
            (tl::Literal::new_typed_literal(v.clone(), tl::NamedNode::new_unchecked(xsd_string)), oxrdf::Literal::new_typed_literal(v.clone(), oxrdf::NamedNode::new_unchecked(xsd_string))),
            (tl::Literal::new_typed_literal(v.clone(), tl::NamedNode::new_unchecked("http://e/d")), oxrdf::Literal::new_typed_literal(v.clone(), oxrdf::NamedNode::new_unchecked("http://e/d"))),
            (tl::Literal::new_language_tagged_literal_unchecked(v.clone(), "en"), oxrdf::Literal::new_language_tagged_literal_unchecked(v.clone(), "en")),
        ];
        for (a, b) in pairs { same(&a, &b); same(&a.as_ref().into_owned(), &b); if lits.len() < 3000 { lits.push((a, b)) } }
    }
    let langs = ["en", "EN-gb", "en-GB-oed", "i-KLINGON", "x-a", "X-A", "zh-Hant-TW", "de-419", "", "e", "en-", "a-b-c", "abcdefghi", "en_US", "sgn-BE-FR"];
    for tag in langs.iter().map(|s| s.to_string()).chain((0..100_000).map(|_| rng.string(LANG, 5))) {
        let (a, b) = (tl::Literal::new_language_tagged_literal("v", tag.clone()), oxrdf::Literal::new_language_tagged_literal("v", tag.clone()));
        assert_eq!(a.is_ok(), b.is_ok(), "{tag:?}");
        if let (Ok(a), Ok(b)) = (a, b) { same(&a, &b); if lits.len() < 4000 { lits.push((a, b)) } }
    }
    for (a, b) in &lits { for (c, d) in lits.iter().take(300) { assert_eq!(a == c, b == d) } }
    assert_eq!(hash(&lits[0].0) == hash(&lits[1].0), lits[0].0 == lits[1].0);
}

#[test]
fn literal_conversions_agree() {
    for v in [true, false] { same(&v.into(), &v.into()) }
    for v in [0i64, -1, i64::MIN, i64::MAX, 42] { same(&v.into(), &v.into()) }
    for v in [0i32, i32::MIN] { same(&v.into(), &v.into()) }
    for v in [0i16, i16::MIN] { same(&v.into(), &v.into()) }
    for v in [0u64, u64::MAX] { same(&v.into(), &v.into()) }
    for v in [0u32, u32::MAX] { same(&v.into(), &v.into()) }
    for v in [0u16, u16::MAX] { same(&v.into(), &v.into()) }
    for v in [0i128, i128::MIN, i128::MAX] { same(&v.into(), &v.into()) }
    for v in [0.0f64, -0.0, 1.0, 1.5, 1e21, 1e-7, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MIN_POSITIVE, f64::MAX, 0.1 + 0.2] { same(&v.into(), &v.into()) }
    for v in [0.0f32, -0.0, 1e21, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 0.1] { same(&v.into(), &v.into()) }
    for v in ["", "a\"b"] { same(&v.into(), &v.into()); same(&v.to_owned().into(), &v.to_owned().into()) }
}

#[test]
fn statements_display_agree() {
    let (s, bs) = (tl::NamedNode::new_unchecked("http://s"), tl::BlankNode::new_unchecked("b1"));
    let (os, obs) = (oxrdf::NamedNode::new_unchecked("http://s"), oxrdf::BlankNode::new_unchecked("b1"));
    let lit = tl::Literal::new_language_tagged_literal_unchecked("a\nb", "en");
    let olit = oxrdf::Literal::new_language_tagged_literal_unchecked("a\nb", "en");
    let t = tl::Triple::new(bs.clone(), s.clone(), lit.clone());
    let ot = oxrdf::Triple::new(obs.clone(), os.clone(), olit.clone());
    assert_eq!(t.to_string(), ot.to_string());
    assert_eq!(t.as_ref().to_string(), ot.as_ref().to_string());
    for (g, og) in [(tl::GraphName::DefaultGraph, oxrdf::GraphName::DefaultGraph), (s.clone().into(), os.clone().into()), (bs.clone().into(), obs.clone().into())] {
        assert_eq!(g.to_string(), og.to_string());
        let (q, oq) = (t.clone().in_graph(g), ot.clone().in_graph(og));
        assert_eq!((q.to_string(), q.as_ref().to_string()), (oq.to_string(), oq.as_ref().to_string()));
        assert_eq!(tl::Triple::from(q).to_string(), oxrdf::Triple::from(oq).to_string());
    }
    let terms: [(tl::Term, oxrdf::Term); 3] = [(s.into(), os.into()), (bs.into(), obs.into()), (lit.into(), olit.into())];
    for (a, b) in &terms {
        assert_eq!((a.to_string(), a.as_ref().to_string(), a.is_named_node(), a.is_blank_node(), a.is_literal()),
            (b.to_string(), b.as_ref().to_string(), b.is_named_node(), b.is_blank_node(), b.is_literal()));
        assert_eq!(tl::NamedOrBlankNode::try_from(a.clone()).is_ok(), oxrdf::NamedOrBlankNode::try_from(b.clone()).is_ok());
        if let (Err(e), Err(f)) = (tl::NamedNode::try_from(a.clone()), oxrdf::NamedNode::try_from(b.clone())) { assert_eq!(e.to_string(), f.to_string()) }
    }
}

const TERM_TEXT: &[&str] = &["<", ">", "http://a/", "_:", "b1", "\"", "\\", "\\n", "\\\"", "\\u00E9", "\\U0001F600", "\\uD800", "\\x", "@", "en", "-GB", "^^", "<http://www.w3.org/2001/XMLSchema#string>",
    "<http://www.w3.org/2001/XMLSchema#integer>", "1", "-2.5", "1e3", "true", "false", " ", "\n", "\t", ".", "?", "$", "x", "é", "<<", ">>", "'", "#"];

/// The term text parser (`FromStr`): the dictionaries store every term as its `Display` text and read it back with
/// it, so it must accept and build exactly what oxrdf's does.
#[test]
fn term_parsing_agrees() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut inputs: Vec<String> = vec!["<http://a>", "_:b", "\"x\"", "\"x\"@EN", "\"x\"^^<http://www.w3.org/2001/XMLSchema#string>", "1", "+1", "-0.5", "1.", ".5", "1e5", "1E-5",
        "true", "false", "\"a\nb\"", "\"\\u0000\"", "<http://a b>", "_:a.", "\"x\"@", "\"x\"^^", "?x", "$x", "?", "<>", "\"\\U00110000\"", " <http://a>", "<http://a> "]
        .into_iter().map(String::from).collect();
    inputs.extend((0..300_000).map(|_| rng.string(TERM_TEXT, 6)));
    for s in &inputs {
        let (a, b) = (s.parse::<tl::Term>(), s.parse::<oxrdf::Term>());
        assert_eq!(a.as_ref().map(ToString::to_string).ok(), b.as_ref().map(ToString::to_string).ok(), "{s:?}");
        // Same message, except the detail of an IRI or language-tag error, which comes from oxiri/oxilangtag there.
        let head = |e: String| e.split("': ").next().unwrap_or_default().to_owned();
        if let (Err(a), Err(b)) = (&a, &b) { assert_eq!(head(a.to_string()), head(b.to_string()), "{s:?}") }
        assert_eq!(s.parse::<tl::Literal>().map(|l| l.to_string()).ok(), s.parse::<oxrdf::Literal>().map(|l| l.to_string()).ok(), "{s:?}");
        assert_eq!(s.parse::<tl::NamedNode>().map(|l| l.to_string()).ok(), s.parse::<oxrdf::NamedNode>().map(|l| l.to_string()).ok(), "{s:?}");
        assert_eq!(s.parse::<tl::BlankNode>().map(|l| l.to_string()).ok(), s.parse::<oxrdf::BlankNode>().map(|l| l.to_string()).ok(), "{s:?}");
        assert_eq!(s.parse::<tl::Variable>().map(|l| l.to_string()).ok(), s.parse::<oxrdf::Variable>().map(|l| l.to_string()).ok(), "{s:?}");
        let t = format!("<http://s> <http://p> {s} .");
        assert_eq!(t.parse::<tl::Triple>().map(|l| l.to_string()).ok(), t.parse::<oxrdf::Triple>().map(|l| l.to_string()).ok(), "{t:?}");
        let q = format!("<http://s> <http://p> <http://o> {s} .");
        assert_eq!(q.parse::<tl::Quad>().map(|l| l.to_string()).ok(), q.parse::<oxrdf::Quad>().map(|l| l.to_string()).ok(), "{q:?}");
    }
    // Every term's text reads back as the same term (the dictionary round trip).
    for v in ["a", "\u{0}\u{7f}\u{fffe}\"\\\n", "é😀"] {
        for t in [tl::Term::from(tl::Literal::new_simple_literal(v)), tl::Literal::new_language_tagged_literal_unchecked(v, "en-gb").into(),
            tl::Literal::new_typed_literal(v, tl::NamedNode::new_unchecked("http://d")).into(), tl::BlankNode::default().into()] {
            assert_eq!(t.to_string().parse::<tl::Term>().unwrap(), t);
            assert_eq!(t.to_string().parse::<oxrdf::Term>().unwrap().to_string(), t.to_string());
        }
    }
}

/// RDF 1.2 (the `rdf-12` feature): triple terms and directional language-tagged literals, against oxrdf with its own
/// `rdf-12` feature: constructors, accessors, `Display`, and the `FromStr` parser accept, reject (with the same message)
/// and build the same terms.
mod rdf_12 {
    use super::*;

    fn dirs() -> [(tl::BaseDirection, oxrdf::BaseDirection); 2] {
        [(tl::BaseDirection::Ltr, oxrdf::BaseDirection::Ltr), (tl::BaseDirection::Rtl, oxrdf::BaseDirection::Rtl)]
    }

    fn same_dir(a: &tl::Literal, b: &oxrdf::Literal) {
        same(a, b);
        assert_eq!(a.direction().map(|d| (d.to_string(), format!("{d:?}"))), b.direction().map(|d| (d.to_string(), format!("{d:?}"))));
        assert_eq!(a.as_ref().direction().map(|d| d.to_string()), b.as_ref().direction().map(|d| d.to_string()));
        let ((v, dt, l, d), (ov, odt, ol, od)) = (a.clone().destruct(), b.clone().destruct());
        assert_eq!((v, dt.map(|n| n.as_str().to_owned()), l, d.map(|d| d.to_string())),
            (ov, odt.map(|n| n.as_str().to_owned()), ol, od.map(|d| d.to_string())));
    }

    #[test]
    fn directional_literals_agree() {
        let mut rng = Rng(0x5851_F42D_4C95_7F2D);
        let mut lits = Vec::new();
        let langs = ["en", "EN-gb", "en-GB-oed", "i-KLINGON", "x-a", "X-A", "zh-Hant-TW", "de-419", "", "e", "en-", "en--ltr", "a-b-c", "abcdefghi", "en_US"];
        for tag in langs.iter().map(|s| s.to_string()).chain((0..50_000).map(|_| rng.string(LANG, 5))) {
            for (d, od) in dirs() {
                let (a, b) = (tl::Literal::new_directional_language_tagged_literal("v\n\"", tag.clone(), d),
                    oxrdf::Literal::new_directional_language_tagged_literal("v\n\"", tag.clone(), od));
                assert_eq!(a.is_ok(), b.is_ok(), "{tag:?}");
                if let (Ok(a), Ok(b)) = (a, b) { same_dir(&a, &b); same_dir(&a.as_ref().into_owned(), &b); if lits.len() < 3000 { lits.push((a, b)) } }
            }
        }
        for v in ["", "a", "\u{0}\u{7f}\u{fffe}\"\\\n", "é😀"] {
            for (d, od) in dirs() {
                let (a, b) = (tl::Literal::new_directional_language_tagged_literal_unchecked(v, "en-gb", d),
                    oxrdf::Literal::new_directional_language_tagged_literal_unchecked(v, "en-gb", od));
                same_dir(&a, &b);
                let (r, or) = (tl::LiteralRef::new_directional_language_tagged_literal_unchecked(v, "en-gb", d),
                    oxrdf::LiteralRef::new_directional_language_tagged_literal_unchecked(v, "en-gb", od));
                assert_eq!((r.to_string(), r.value(), r.language(), r.datatype().as_str(), r == a, r.into_owned() == a),
                    (or.to_string(), or.value(), or.language(), or.datatype().as_str(), or == b, or.into_owned() == b));
                lits.push((a, b));
            }
            // A directional literal never equals the language-tagged one with the same tag, nor the other direction.
            lits.push((tl::Literal::new_language_tagged_literal_unchecked(v, "en-gb"), oxrdf::Literal::new_language_tagged_literal_unchecked(v, "en-gb")));
        }
        for (a, b) in &lits { for (c, d) in lits.iter().rev().take(300) { assert_eq!(a == c, b == d, "{a} {c}") } }
    }

    /// `<<( _:b <http://p> "x"@en--rtl )>>` wrapped `depth` times as the object of `<http://s> <http://p>`.
    fn nested(depth: usize) -> (tl::Triple, oxrdf::Triple) {
        let (s, p) = (tl::NamedNode::new_unchecked("http://s"), tl::NamedNode::new_unchecked("http://p"));
        let (os, op) = (oxrdf::NamedNode::new_unchecked("http://s"), oxrdf::NamedNode::new_unchecked("http://p"));
        let mut t = (
            tl::Triple::new(tl::BlankNode::new_unchecked("b"), p.clone(), tl::Literal::new_directional_language_tagged_literal_unchecked("x", "en", tl::BaseDirection::Rtl)),
            oxrdf::Triple::new(oxrdf::BlankNode::new_unchecked("b"), op.clone(), oxrdf::Literal::new_directional_language_tagged_literal_unchecked("x", "en", oxrdf::BaseDirection::Rtl)),
        );
        for _ in 0..depth { t = (tl::Triple::new(s.clone(), p.clone(), t.0), oxrdf::Triple::new(os.clone(), op.clone(), t.1)) }
        t
    }

    #[test]
    fn triple_terms_agree() {
        for depth in [0, 1, 2, 5, 40] {
            let (t, ot) = nested(depth);
            assert_eq!((t.to_string(), t.as_ref().to_string()), (ot.to_string(), ot.as_ref().to_string()));
            let (a, b) = (tl::Term::from(t.clone()), oxrdf::Term::from(ot.clone()));
            assert_eq!((a.to_string(), a.as_ref().to_string(), a.is_triple(), a.as_ref().is_triple(), a.is_named_node(), a.is_blank_node(), a.is_literal()),
                (b.to_string(), b.as_ref().to_string(), b.is_triple(), b.as_ref().is_triple(), b.is_named_node(), b.is_blank_node(), b.is_literal()));
            assert_eq!(a.as_ref().into_owned(), a);
            assert_eq!(tl::Term::from(Box::new(t.clone())), a);
            assert_eq!(tl::Term::from(t.as_ref()), a);
            assert_eq!(tl::TermRef::from(&t), a.as_ref());
            assert_eq!(tl::NamedOrBlankNode::try_from(a.clone()).unwrap_err().to_string(), oxrdf::NamedOrBlankNode::try_from(b.clone()).unwrap_err().to_string());
            assert_eq!(tl::NamedNode::try_from(a.clone()).unwrap_err().to_string(), oxrdf::NamedNode::try_from(b.clone()).unwrap_err().to_string());
            assert_eq!(tl::BlankNode::try_from(a.clone()).unwrap_err().to_string(), oxrdf::BlankNode::try_from(b.clone()).unwrap_err().to_string());
            assert_eq!(tl::Literal::try_from(a.clone()).unwrap_err().to_string(), oxrdf::Literal::try_from(b.clone()).unwrap_err().to_string());
            // The text reads back as the same term, triple and quad.
            assert_eq!(a.to_string().parse::<tl::Term>().unwrap(), a);
            assert_eq!(a.to_string().parse::<oxrdf::Term>().unwrap(), b);
            assert_eq!(t.to_string().parse::<tl::Triple>().unwrap(), t);
            assert_eq!(t.to_string().parse::<oxrdf::Triple>().unwrap(), ot);
            let (q, oq) = (t.clone().in_graph(tl::GraphName::DefaultGraph), ot.clone().in_graph(oxrdf::GraphName::DefaultGraph));
            assert_eq!((q.to_string(), q.as_ref().to_string()), (oq.to_string(), oq.as_ref().to_string()));
            assert_eq!(q.to_string().parse::<tl::Quad>().unwrap(), q);
        }
    }

    const TEXT_12: &[&str] = &["<<(", ")>>", "<<", ">>", "(", ")", " ", "<http://s>", "<http://p>", "_:b", "\"x\"", "@", "en", "EN", "-GB", "--", "-", "ltr", "rtl",
        "LTR", "up", "^^", "<http://d>", "1", ".", "x-a"];

    #[test]
    fn rdf_12_parsing_agrees() {
        let mut inputs: Vec<String> = vec![
            "<<( <http://s> <http://p> <http://o> )>>", "<<(<http://s> <http://p> <http://o>)>>", " <<( <http://s> <http://p> <http://o> )>> ",
            "<<( _:b <http://p> \"x\"@en--ltr )>>", "<<( <http://s> <http://p> <<( _:b <http://q> \"x\"@en-GB--rtl )>> )>>",
            "<<( <http://s> <http://p> <<( <http://s> <http://p> <<( <http://s> <http://p> 1 )>> )>> )>>",
            // Malformed triple terms.
            "<<( <<( <http://s> <http://p> <http://o> )>> <http://p> <http://o> )>>", "<<( \"x\" <http://p> <http://o> )>>", "<<( <http://s> _:p <http://o> )>>",
            "<<( <http://s> <http://p> )>>", "<<( <http://s> <http://p> <http://o> )>", "<<( <http://s> <http://p> <http://o> >>", "<<( <http://s> <http://p> <http://o>",
            "<<( <http://s> <http://p> <http://o> ) >>", "<<( <http://s> <http://p> <http://o> . )>>", "<<( <http://s> <http://p> <http://o> )>> <http://x>",
            "<<()>>", "<<( )>>", "<<(", "<< <http://s> <http://p> <http://o> >>", "<<( <http://s> <http://p> \"x\"@en--up )>>",
            // Directional language-tagged literals, valid and not.
            "\"x\"@en--ltr", "\"x\"@en--rtl", "\"x\"@EN-gb--rtl", "\"a\\nb\"@en--rtl", "\"x\"@x-a--rtl", "\"x\"@i-klingon--ltr", "\"x\" @en--ltr",
            "\"x\"@en--LTR", "\"x\"@en--Rtl", "\"x\"@--ltr", "\"x\"@en--up", "\"x\"@en--", "\"x\"@en---ltr", "\"x\"@en--ltr--rtl", "\"x\"@en-ltr", "\"x\"@ltr",
            "\"x\"@en_US--ltr", "\"x\"@en--ltr^^<http://d>", "\"x\"@en-- ltr", "\"x\"@en--ltr.", "\"x\"@en--l",
        ].into_iter().map(String::from).collect();
        // Nesting depth around the parser's recursion limit.
        let deep = |n: usize| format!("{}<http://o>{}", "<<( <http://s> <http://p> ".repeat(n), " )>>".repeat(n));
        inputs.extend([1, 30, 62, 63, 64, 65, 200].map(deep));
        let mut rng = Rng(0x94D0_49BB_1331_11EB);
        inputs.extend((0..200_000).map(|_| rng.string(TEXT_12, 8)));
        // Same message, except the detail of an IRI or language-tag error, which comes from oxiri/oxilangtag there.
        let head = |e: String| e.split("': ").next().unwrap_or_default().to_owned();
        for s in &inputs {
            let (a, b) = (s.parse::<tl::Term>(), s.parse::<oxrdf::Term>());
            assert_eq!(a.as_ref().map(ToString::to_string).ok(), b.as_ref().map(ToString::to_string).ok(), "{s:?}");
            if let (Err(a), Err(b)) = (&a, &b) { assert_eq!(head(a.to_string()), head(b.to_string()), "{s:?}") }
            if let (Ok(a), Ok(b)) = (&a, &b) {
                assert_eq!(a.to_string().parse::<tl::Term>().ok().as_ref() == Some(a), b.to_string().parse::<oxrdf::Term>().ok().as_ref() == Some(b), "{s:?}");
            }
            let (a, b) = (s.parse::<tl::Literal>(), s.parse::<oxrdf::Literal>());
            assert_eq!(a.as_ref().map(ToString::to_string).ok(), b.as_ref().map(ToString::to_string).ok(), "{s:?}");
            if let (Err(a), Err(b)) = (&a, &b) { assert_eq!(head(a.to_string()), head(b.to_string()), "{s:?}") }
            if let (Ok(a), Ok(b)) = (&a, &b) { same_dir(a, b) }
            for t in [format!("<http://s> <http://p> {s} ."), format!("{s} <http://p> <http://o>"), format!("<http://s> <http://p> {s}")] {
                let (a, b) = (t.parse::<tl::Triple>(), t.parse::<oxrdf::Triple>());
                assert_eq!(a.as_ref().map(ToString::to_string).ok(), b.as_ref().map(ToString::to_string).ok(), "{t:?}");
                if let (Err(a), Err(b)) = (&a, &b) { assert_eq!(head(a.to_string()), head(b.to_string()), "{t:?}") }
            }
            let q = format!("<http://s> <http://p> <http://o> {s} .");
            let (a, b) = (q.parse::<tl::Quad>(), q.parse::<oxrdf::Quad>());
            assert_eq!(a.as_ref().map(ToString::to_string).ok(), b.as_ref().map(ToString::to_string).ok(), "{q:?}");
            if let (Err(a), Err(b)) = (&a, &b) { assert_eq!(head(a.to_string()), head(b.to_string()), "{q:?}") }
        }
    }
}

