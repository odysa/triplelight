//! Differential qualification of the Turtle / TriG parser against `oxttl` 0.2 (task #45, step 3): a document is
//! accepted by both or neither, and an accepted one yields the same statements in the same order. Both parsers give
//! `[]` and collection nodes fresh random labels, so blank nodes are compared after relabelling them in order of first
//! appearance (which also checks that the emission order matches).
use triplelight_core::rdf::{BlankNode, GraphName, NamedOrBlankNode, Quad, Term};
mod ox;
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
    let quads = if trig { oxttl::TriGParser::new().for_reader(data).map(|q| q.map(ox::quad)).collect::<Result<Vec<_>, _>>().ok()? } else {
        oxttl::TurtleParser::new().for_reader(data).map(|t| t.map(|t| ox::triple(t).in_graph(GraphName::DefaultGraph))).collect::<Result<Vec<_>, _>>().ok()?
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
        "@prefix : <http://ex/> . :a :b \"x\n\" .", "@prefix : <http://ex/> . :a :b \"\"\"a\\u0041\\n\"\"\" .",
        #[cfg(not(feature = "rdf-12"))]
        "@prefix : <http://ex/> . :a :b \"x\"@en--ltr .",
        "@prefix : <http://ex/> . { :a :b :c }", "@prefix : <http://ex/> . :g { :a :b :c . :d :e :f . }", "@prefix : <http://ex/> . GRAPH :g { :a :b :c }",
        "@prefix : <http://ex/> . graph :g { :a :b :c }", "@prefix : <http://ex/> . _:g { :a :b :c }", "@prefix : <http://ex/> . [] { :a :b :c }",
        "@prefix : <http://ex/> . GRAPH [] { :a :b :c }", "@prefix : <http://ex/> . GRAPH [ :p :o ] { :a :b :c }", "@prefix : <http://ex/> . :g { } { }",
        "@prefix : <http://ex/> . :g { :a :b :c } .", "@prefix : <http://ex/> . :g { [ :p :o ] . ( :x ) :y :z }", "@prefix : <http://ex/> . :g { @prefix x: <y> . }",
        "@prefix : <http://ex/> . :g { :a :b :c .. }", "@prefix : <http://ex/> . { . }",
        #[cfg(not(feature = "rdf-12"))]
        "<< <a> <b> <c> >> <d> <e> .",
        #[cfg(not(feature = "rdf-12"))]
        "<http://a> <http://b> <http://c> {| <http://d> <http://e> |} .",
        "\u{feff}<http://a> <http://b> <http://c> .", "<http://a> <http://b> \"\u{ff}\" .",
        #[cfg(not(feature = "rdf-12"))]
        "<http://a> <http://b> <http://c> ~ .",
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
        "\"\"\"l\"\"\"", "@en", "@prefix", "^^", "^^:d", "#c\n", "\n", " ", " ", " ", "\t", ":a.b", ":a\\.", "%20", "?v",
        #[cfg(not(feature = "rdf-12"))] "<<",
        #[cfg(not(feature = "rdf-12"))] ">>",
        #[cfg(not(feature = "rdf-12"))] "~",
        #[cfg(not(feature = "rdf-12"))] "{|",
        #[cfg(not(feature = "rdf-12"))] "|}",
    ];
    let mut rng = 0x2545f4914f6cdd1du64;
    let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let (mut accepted, mut total) = (0usize, 0usize);
    const SUBJECTS: &[&str] = &[":a", "p:x", "<s>", "<http://a>", "_:b", "[]", "[ :q :r ]", "( :a 1 )", "()", "[ :q [ :r :s ] ]"];
    const PREDICATES: &[&str] = &[":p", "a", "p:y", "<http://p>"];
    const OBJECTS: &[&str] = &[":o", "p:z", "<o>", "_:c", "[]", "[ :q :r ; :s 1 ]", "( :a ( 2 ) [ :t :u ] )", "()", "\"s\"", "\"s\"@en", "'t'^^:d",
        "\"\"\"l\nl\"\"\"", "1", "-1.5", ".5e2", "true", "false", ":a.b"];
    for i in 0..150_000 {
        let mut doc = String::from("@prefix : <http://ex/> . PREFIX p: <http://ex/p/> @base <http://ex/b/> . ");
        let pick = |n: u64, xs: &[&'static str]| xs[(n % xs.len() as u64) as usize];
        let graph = i % 3 == 0;
        if graph { doc.push_str(pick(next(), &[":g { ", "GRAPH :h { ", "{ ", "_:g { "])) }
        for _ in 0..(1 + next() % 3) {
            doc.push_str(pick(next(), SUBJECTS)); doc.push(' ');
            for k in 0..(1 + next() % 2) {
                if k > 0 { doc.push_str(" ; ") }
                doc.push_str(pick(next(), PREDICATES)); doc.push(' ');
                for m in 0..(1 + next() % 2) { if m > 0 { doc.push_str(" , ") } doc.push_str(pick(next(), OBJECTS)) }
            }
            doc.push_str(" . ");
            // An occasional random piece, to exercise errors next to valid statements.
            if next() % 4 == 0 { doc.push_str(pick(next(), PIECES)); doc.push(' ') }
        }
        if graph { doc.push('}') }
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

/// Large files in lockstep plus descriptive parse timing (run with `--release --ignored --nocapture` and
/// `TL_TTL_FILES=a.ttl:b.nt`; N-Triples files are valid Turtle).
#[test]
#[ignore]
fn files_agree_and_time() {
    let Some(files) = std::env::var_os("TL_TTL_FILES") else { return };
    for path in std::env::split_paths(&files) {
        let open = || std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(&path).unwrap());
        let mut a = read_turtle(open()).map(|r| r.unwrap_or_else(|e| panic!("ours failed: {e}")));
        let mut b = oxttl::TurtleParser::new().for_reader(open()).map(|r| ox::triple(r.unwrap_or_else(|e| panic!("oxttl failed: {e}"))));
        let mut n = 0u64;
        loop {
            match (a.next(), b.next()) {
                (None, None) => break,
                (Some(x), Some(y)) => { assert_eq!(x, y, "{} statement {n}", path.display()); n += 1 }
                (x, y) => panic!("{}: one side ended at {n}: {x:?} / {y:?}", path.display()),
            }
        }
        let t = std::time::Instant::now();
        let ours_n = read_turtle(open()).count();
        let ours_s = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let theirs_n = oxttl::TurtleParser::new().for_reader(open()).count();
        let theirs_s = t.elapsed().as_secs_f64();
        assert_eq!(ours_n, theirs_n);
        eprintln!("{}: {n} statements identical; parse ours {ours_s:.2} s, oxttl {theirs_s:.2} s", path.display());
    }
}
