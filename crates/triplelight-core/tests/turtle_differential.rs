//! Differential qualification of the Turtle / TriG parser against `oxttl` 0.2 (task #45, step 3): a document is
//! accepted by both or neither, and an accepted one yields the same statements in the same order. Both parsers give
//! `[]` and collection nodes fresh random labels, so blank nodes are compared after relabelling them in order of first
//! appearance (which also checks that the emission order matches).
use oxrdf::{BlankNode, GraphName, NamedOrBlankNode, Quad, Term};
use std::collections::HashMap;
use triplelight_core::turtle::{read_trig, read_turtle};

fn canonical(quads: Vec<Quad>) -> Vec<String> {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut label = |b: &BlankNode| { let n = map.len(); map.entry(b.as_str().to_owned()).or_insert_with(|| format!("b{n}")).clone() };
    quads.into_iter().map(|q| {
        let s = match &q.subject { NamedOrBlankNode::BlankNode(b) => format!("_:{}", label(b)), s => s.to_string() };
        let o = match &q.object { Term::BlankNode(b) => format!("_:{}", label(b)), o => o.to_string() };
        let g = match &q.graph_name { GraphName::BlankNode(b) => format!("_:{}", label(b)), g => g.to_string() };
        format!("{s} {} {o} {g}", q.predicate)
    }).collect()
}
fn ours(data: &[u8], trig: bool) -> Option<Vec<String>> {
    let quads = if trig { read_trig(data).collect::<Result<Vec<_>, _>>().ok()? } else {
        read_turtle(data).map(|t| t.map(|t| t.in_graph(GraphName::DefaultGraph))).collect::<Result<Vec<_>, _>>().ok()?
    };
    Some(canonical(quads))
}
fn theirs(data: &[u8], trig: bool) -> Option<Vec<String>> {
    let quads = if trig { oxttl::TriGParser::new().for_reader(data).collect::<Result<Vec<_>, _>>().ok()? } else {
        oxttl::TurtleParser::new().for_reader(data).map(|t| t.map(|t| t.in_graph(GraphName::DefaultGraph))).collect::<Result<Vec<_>, _>>().ok()?
    };
    Some(canonical(quads))
}

/// The W3C RDF 1.1 Turtle and TriG suites (also run each Turtle file as TriG), when `TL_RDF_TESTS` is set.
#[test]
fn w3c_syntax_suites_agree() {
    let Some(root) = std::env::var_os("TL_RDF_TESTS") else { eprintln!("TL_RDF_TESTS not set; skipped"); return };
    let mut seen = 0;
    for (dir, ext, trig) in [("rdf/rdf11/rdf-turtle", "ttl", false), ("rdf/rdf11/rdf-turtle", "ttl", true), ("rdf/rdf11/rdf-trig", "trig", true)] {
        for entry in std::fs::read_dir(std::path::Path::new(&root).join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some(ext) { continue }
            let data = std::fs::read(&path).unwrap();
            // Suite files use relative IRIs against their own location; give both parsers that base.
            let base = format!("http://www.w3.org/2013/TurtleTests/{}", path.file_name().unwrap().to_string_lossy());
            let doc = [format!("@base <{base}> .\n").into_bytes(), data].concat();
            assert_eq!(ours(&doc, trig), theirs(&doc, trig), "{} (trig: {trig})", path.display());
            seen += 1;
        }
    }
    assert!(seen > 500, "only {seen} suite files found");
    eprintln!("W3C files compared: {seen}");
}

#[test]
fn curated_documents_agree() {
    let docs: &[&str] = &[
        "", "# c", "@prefix : <http://ex/> . :a :b :c .", "@prefix : <http://ex/> .\n:a :b :c ; :d :e , :f ; .", "PREFIX : <http://ex/> :a :b :c .",
        "prefix : <http://ex/> :a :b :c .", "@PREFIX : <http://ex/> . :a :b :c .", "@prefix : <http://ex/> :a :b :c .", "BASE <http://ex/> <a> <b> <c> .",
        "@base <http://ex/a/> . <b> <c> <../d> .", "@base <http://ex/a/> . @base <b/> . <c> <d> <e> .", "<a> <b> <c> .", "@prefix : <http://ex/> . :a a :c .",
        "@prefix : <http://ex/> . :a A :c .", "@prefix : <http://ex/> . :a :b true, false, TRUE .", "@prefix : <http://ex/> . :a :b 1, -2, +3, 1.5, .5, 1e5, 1.E-3, 1. .",
        "@prefix : <http://ex/> . :a :b 1.:c .", "@prefix : <http://ex/> . :a :b \"x\"@EN, 'y', \"\"\"long\n\"q\"\"\"\", '''z''' .",
        "@prefix : <http://ex/> . :a :b \"x\"^^:d, \"y\"^^<http://ex/e> .", "@prefix : <http://ex/> . :a :b [ :c :d ; :e [ :f :g ] ] .",
        "@prefix : <http://ex/> . [ :c :d ] :e :f .", "@prefix : <http://ex/> . [ :c :d ] .", "@prefix : <http://ex/> . [] :c :d .", "@prefix : <http://ex/> . [] .",
        "@prefix : <http://ex/> . :a :b ( :c ( :d ) [ :e :f ] () 1 ) .", "@prefix : <http://ex/> . ( :c :d ) :e :f .", "@prefix : <http://ex/> . () :e :f .",
        "@prefix : <http://ex/> . ( :c ) .", "@prefix : <http://ex/> . :a :b :c,, :d .", "@prefix : <http://ex/> . :a :b :c ;; :d :e ;.",
        "@prefix : <http://ex/> . :a.b :c :d .", "@prefix : <http://ex/> . :a\\.b :c :d .", "@prefix : <http://ex/> . :a%20b :c :d .", "@prefix : <http://ex/> . :a%2 :c :d .",
        "@prefix : <http://ex/> . :a\\/b :c :d .", "@prefix : <http://ex/> . :a\\x :c :d .", "@prefix : <http://ex/> . :a:b :c :d .", "@prefix x. : <http://ex/> .",
        "@prefix : <urn:> . :a :b :c .", "@prefix : <http://ex> . :a :b :c .", "@prefix : <http://ex/> . :  :b :c .", "@prefix : <http://ex/> . : :b :c .",
        "@prefix : <http://ex/> . :a :b ?x .", "@prefix : <http://ex/> . :a :b :c", "@prefix : <http://ex/> . :a :b :c .. ", "@prefix : <http://ex/> . :a :b :c } ",
        "@prefix : <http://ex/> . _:x :b _:y . _:y. :b _:x.", "@prefix : <http://ex/> . :a :b \"x\"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString> .",
        "@prefix : <http://ex/> . :a :b \"x\n\" .", "@prefix : <http://ex/> . :a :b \"\"\"a\\u0041\\n\"\"\" .", "@prefix : <http://ex/> . :a :b \"x\"@en--ltr .",
        "@prefix : <http://ex/> . { :a :b :c }", "@prefix : <http://ex/> . :g { :a :b :c . :d :e :f . }", "@prefix : <http://ex/> . GRAPH :g { :a :b :c }",
        "@prefix : <http://ex/> . graph :g { :a :b :c }", "@prefix : <http://ex/> . _:g { :a :b :c }", "@prefix : <http://ex/> . [] { :a :b :c }",
        "@prefix : <http://ex/> . GRAPH [] { :a :b :c }", "@prefix : <http://ex/> . GRAPH [ :p :o ] { :a :b :c }", "@prefix : <http://ex/> . :g { } { }",
        "@prefix : <http://ex/> . :g { :a :b :c } .", "@prefix : <http://ex/> . :g { [ :p :o ] . ( :x ) :y :z }", "@prefix : <http://ex/> . :g { @prefix x: <y> . }",
        "@prefix : <http://ex/> . :g { :a :b :c .. }", "@prefix : <http://ex/> . { . }", "<< <a> <b> <c> >> <d> <e> .", "<http://a> <http://b> <http://c> {| <http://d> <http://e> |} .",
        "\u{feff}<http://a> <http://b> <http://c> .", "<http://a> <http://b> \"\u{ff}\" .", "<http://a> <http://b> <http://c> ~ .",
    ];
    for doc in docs { for trig in [false, true] { assert_eq!(ours(doc.as_bytes(), trig), theirs(doc.as_bytes(), trig), "{doc:?} (trig: {trig})") } }
    for doc in [&b"# \xff\n<http://a> <http://b> <http://c> .\n"[..], b"<http://a\xff> <http://b> <http://c> .", b"<http://a> <http://b> \"\xff\" .", b"@prefix \xc3: <http://e/> ."] {
        for trig in [false, true] { assert_eq!(ours(doc, trig), theirs(doc, trig), "{doc:?}") }
    }
}

/// Random documents over grammar-relevant pieces.
#[test]
fn random_documents_agree() {
    const PIECES: &[&str] = &[
        "@prefix : <http://ex/> .", "PREFIX p: <http://ex/p/>", "@base <http://ex/b/> .", "BASE <http://ex/c>", ":a", ":b", "p:x", "p:", ":", "<x>", "<http://a>",
        "_:b", "_:b.", "[", "]", "[]", "(", ")", "()", "{", "}", "GRAPH", "a", "A", ";", ",", ".", "..", "1", "-1.5", ".5e2", "1.", "true", "\"s\"", "'t'",
        "\"\"\"l\"\"\"", "@en", "@prefix", "^^", "^^:d", "#c\n", "\n", " ", " ", " ", "\t", ":a.b", ":a\\.", "%20", "?v", "<<", ">>", "~", "{|", "|}",
    ];
    let mut rng = 0x2545f4914f6cdd1du64;
    let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let (mut accepted, mut total) = (0usize, 0usize);
    for i in 0..150_000 {
        let mut doc = String::from(if i % 2 == 0 { "@prefix : <http://ex/> . PREFIX p: <http://ex/p/> " } else { "" });
        for _ in 0..(3 + next() % 10) { doc.push_str(PIECES[(next() % PIECES.len() as u64) as usize]); doc.push(' '); }
        for trig in [false, true] {
            let (a, b) = (ours(doc.as_bytes(), trig), theirs(doc.as_bytes(), trig));
            assert_eq!(a, b, "{doc:?} (trig: {trig})");
            total += 1;
            if a.is_some_and(|v| !v.is_empty()) { accepted += 1 }
        }
    }
    eprintln!("random documents: {total} compared, {accepted} yielded statements");
    assert!(accepted > 5_000, "too few valid documents ({accepted})");
}
