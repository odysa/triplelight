//! Differential qualification of the N-Triples / N-Quads parser against `oxttl` 0.2 (task #45, step 2): a document
//! is accepted by both or by neither, and an accepted document yields the same statements in the same order.
use triplelight_core::ntriples::{parse_nquads_line, read_nquads, read_ntriples};
use triplelight_core::rdf::{GraphName, Quad};
mod ox;

fn ours(data: &[u8], quads: bool) -> Option<Vec<Quad>> {
    if quads { read_nquads(data).collect::<Result<Vec<_>, _>>().ok() } else {
        read_ntriples(data).map(|t| t.map(|t| t.in_graph(GraphName::DefaultGraph))).collect::<Result<Vec<_>, _>>().ok()
    }
}
fn theirs(data: &[u8], quads: bool) -> Option<Vec<Quad>> {
    if quads { oxttl::NQuadsParser::new().for_slice(data).map(|q| q.map(ox::quad)).collect::<Result<Vec<_>, _>>().ok() } else {
        oxttl::NTriplesParser::new().for_slice(data).map(|t| t.map(|t| ox::triple(t).in_graph(GraphName::DefaultGraph))).collect::<Result<Vec<_>, _>>().ok()
    }
}
fn check(data: &[u8], quads: bool) {
    assert_eq!(ours(data, quads), theirs(data, quads), "{:?} (quads: {quads})", String::from_utf8_lossy(data));
}

/// The W3C RDF 1.1 N-Triples and N-Quads test suites, when `TL_RDF_TESTS` names an rdf-tests checkout.
#[test]
fn w3c_syntax_suites_agree() {
    let Some(root) = std::env::var_os("TL_RDF_TESTS") else { eprintln!("TL_RDF_TESTS not set; skipped"); return };
    let mut seen = 0;
    for (dir, ext, quads) in [("rdf/rdf11/rdf-n-triples", "nt", false), ("rdf/rdf11/rdf-n-quads", "nq", true), ("rdf/rdf11/rdf-n-quads", "nt", true)] {
        let dir = std::path::Path::new(&root).join(dir);
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some(ext) { continue }
            let data = std::fs::read(&path).unwrap();
            assert_eq!(ours(&data, quads), theirs(&data, quads), "{}", path.display());
            seen += 1;
        }
    }
    assert!(seen > 150, "only {seen} suite files found");
    eprintln!("W3C files compared: {seen}");
}

#[test]
fn curated_documents_agree() {
    let docs: &[&str] = &[
        "", "\n", "\r\n", "\r", "# c", "# c\n", "#\u{ff}x\n", "<http://a> <http://b> <http://c> .", "<http://a> <http://b> <http://c> .\n",
        "<http://a> <http://b> <http://c> . # c\n", "<http://a> <http://b> <http://c>.\n", "<http://a><http://b><http://c>.\n",
        "<http://a> <http://b> <http://c> .\r\n<http://a> <http://b> <http://d> .\r\n", "<http://a> <http://b> <http://c> .\r<http://a> <http://b> <http://d> .",
        "\t<http://a>\t<http://b>\t<http://c>\t.\t\n", " <http://a> <http://b> <http://c> . <http://a> <http://b> <http://d> .\n",
        "<http://a> <http://b> \"x\" .\n", "<http://a> <http://b> \"x\"@en .\n", "<http://a> <http://b> \"x\" @en .\n", "<http://a> <http://b> \"x\"@EN-Us .\n",
        "<http://a> <http://b> \"x\"@en-1 .\n", "<http://a> <http://b> \"x\"@en- .\n", "<http://a> <http://b> \"x\"@1en .\n", "<http://a> <http://b> \"x\"@en--ltr .\n",
        "<http://a> <http://b> \"x\"@i-klingon .\n", "<http://a> <http://b> \"x\"@x-private .\n", "<http://a> <http://b> \"x\"@en-a-bbb-x-a .\n",
        "<http://a> <http://b> \"x\"@en-a .\n", "<http://a> <http://b> \"x\"@zh-Hant-TW .\n", "<http://a> <http://b> \"x\"@abcdefghi .\n",
        "<http://a> <http://b> \"x\"^^<http://www.w3.org/2001/XMLSchema#string> .\n", "<http://a> <http://b> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
        "<http://a> <http://b> \"x\" ^^ <http://d> .\n", "<http://a> <http://b> \"x\"^<http://d> .\n", "<http://a> <http://b> \"x\"^^\"y\" .\n",
        "<http://a> <http://b> \"x\"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString> .\n",
        "<http://a> <http://b> \"\\t\\b\\n\\r\\f\\\"\\'\\\\\" .\n", "<http://a> <http://b> \"\\u00e9\\U0001F600\" .\n", "<http://a> <http://b> \"\\uD800\" .\n",
        "<http://a> <http://b> \"\\uD83D\\uDE00\" .\n", "<http://a> <http://b> \"\\x\" .\n", "<http://a> <http://b> \"\\u00\" .\n", "<http://a> <http://b> \"\\u0000\" .\n",
        "<http://a> <http://b> \"a\nb\" .\n", "<http://a> <http://b> \"a\rb\" .\n", "<http://a> <http://b> \"unterminated .\n", "<http://a> <http://b> 'x' .\n",
        "<http://a> <http://b> \"\"\"x\"\"\" .\n", "<http://a> <http://b> \"x\" <http://g> .\n", "<http://a> <http://b> \"x\" _:g .\n",
        "<http://a\\u0020b> <http://b> <http://c> .\n", "<http://a\\u00E9> <http://b> <http://c> .\n", "<http://a\\t> <http://b> <http://c> .\n",
        "<rel> <http://b> <http://c> .\n", "<http://a b> <http://b> <http://c> .\n", "<http://a\n> <http://b> <http://c> .\n", "<> <http://b> <http://c> .\n",
        "<http://a> <http://b> <http://c\n", "<<http://a> <http://b> <http://c>>> <http://b> <http://c> .\n",
        "_:a <http://b> _:b .\n", "_:a. <http://b> _:b .\n", "_:a.b <http://b> _:b.. .\n", "_:1a <http://b> _:-a .\n", "_:_a <http://b> _:a-b·c .\n",
        "_: <http://b> <http://c> .\n", "_:a <http://b> <http://c> _:g .\n", "_:é <http://b> <http://c> .\n", "_:a\u{37e} <http://b> <http://c> .\n",
        "<http://a> _:b <http://c> .\n", "\"x\" <http://b> <http://c> .\n", "<http://a> <http://b> <http://c> .5\n", "<http://a> <http://b> <http://c> ..\n",
        "<http://a> <http://b> <http://c> . .\n", "<http://a> <http://b> <http://c> <http://g> .\n", "<http://a> <http://b> <http://c> <http://g> <http://h> .\n",
        "\u{feff}<http://a> <http://b> <http://c> .\n", "<http://a> <http://b> \"\u{ff}\" .\n",
    ];
    for doc in docs { for quads in [false, true] { check(doc.as_bytes(), quads) } }
    // Invalid UTF-8 in a comment, a token and a string.
    for doc in [&b"# \xff\n<http://a> <http://b> <http://c> .\n"[..], b"<http://a\xff> <http://b> <http://c> .\n", b"<http://a> <http://b> \"\xff\" .\n", b"_:a\xff <http://b> <http://c> .\n"] {
        for quads in [false, true] { check(doc, quads) }
    }
}

/// Random lines over grammar-relevant pieces, alone and as two-line documents.
#[test]
fn random_lines_agree() {
    const PIECES: &[&str] = &[
        "<http://a>", "<http://b/c>", "<urn:x>", "<rel>", "<http://a b>", "<http://\\u00e9>", "<http://\\u0020>", "<", ">", "_:a", "_:b.c", "_:1", "_:", "_:-",
        "\"x\"", "\"\"", "\"\\n\"", "\"\\u00\"", "\"\\uD800\"", "\"é\"", "\"", "@en", "@EN-us", "@en-", "@1", "@x-a", "@i-klingon", "@en--ltr", "^^", "^",
        "<http://www.w3.org/2001/XMLSchema#integer>", "<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString>", ".", "..", ".5", "#", "# c", " ", " ", " ",
        "\t", "\r", "'x'", "<<", ">>", "(", "a", ":", "\u{feff}",
    ];
    let mut rng = 0x9e3779b97f4a7c15u64;
    let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
    let (mut accepted, mut total) = (0usize, 0usize);
    for _ in 0..300_000 {
        // A plausible statement shape with random substitutions, so that a fair share is valid.
        let mut line = String::new();
        let shape = next() % 3;
        for slot in 0..(4 + next() % 4) {
            let piece = if shape == 0 && slot < 6 {
                ["<http://a>", " ", "<http://b>", " ", "\"x\"@en", " ."][slot as usize]
            } else { PIECES[(next() % PIECES.len() as u64) as usize] };
            line.push_str(piece);
        }
        for quads in [false, true] {
            let doc = format!("{line}\n");
            let (a, b) = (ours(doc.as_bytes(), quads), theirs(doc.as_bytes(), quads));
            assert_eq!(a, b, "{doc:?} (quads: {quads})");
            total += 1;
            if a.is_some_and(|v| !v.is_empty()) { accepted += 1 }
        }
    }
    eprintln!("random lines: {total} compared, {accepted} yielded statements");
    assert!(accepted > 10_000, "too few valid lines ({accepted}) for the comparison to mean much");
}

/// The WAL reader: a line is accepted exactly when `oxttl` yields a single quad without an error from it.
#[test]
fn wal_lines_agree() {
    for line in [
        "<http://a> <http://b> <http://c> .", "<http://a> <http://b> <http://c> <http://g> .", "_:a <http://b> \"x\"@en <http://g> .",
        "<http://a> <http://b> \"x\\ny\"^^<http://d> .", "<http://a> <http://b> <http://c>", "<http://a> <http://b> <http://c> . <http://a> <http://b> <http://c> .",
        "", "# c", "<http://a> <http://b> <http://c> _:g .", "<http://a> <http://b> \"x", "<http://a> <http://b> <http://c> .\r",
        "<http://a> <http://b> <http://c> .\r<http://a> <http://b> <http://d> .", "\r<http://a> <http://b> <http://c> .", "<http://a> <http://b> <http://c> .\n# c",
        "<http://a> <http://b> <http://c> .\n<bad>",
    ] {
        let mut q = oxttl::NQuadsParser::new().for_slice(line.as_bytes());
        let theirs = match (q.next(), q.next()) { (Some(Ok(quad)), None) => Some(ox::quad(quad)), _ => None };
        assert_eq!(parse_nquads_line(line), theirs, "{line:?}");
    }
}

/// Whole files in lockstep (run with `--ignored` and `TL_NT_FILES=a.nt:b.nt`); for the benchmark datasets.
#[test]
#[ignore]
fn files_agree() {
    let Some(files) = std::env::var_os("TL_NT_FILES") else { return };
    for path in std::env::split_paths(&files) {
        let quads = path.extension().is_some_and(|e| e == "nq");
        let open = || std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(&path).unwrap());
        // Both sides must succeed on every statement: an error on either side fails, it never compares equal.
        let ours_it: Box<dyn Iterator<Item = Quad>> = if quads {
            Box::new(read_nquads(open()).map(|r| r.unwrap_or_else(|e| panic!("ours failed: {e}"))))
        } else {
            Box::new(read_ntriples(open()).map(|r| r.unwrap_or_else(|e| panic!("ours failed: {e}")).in_graph(GraphName::DefaultGraph)))
        };
        let theirs_it: Box<dyn Iterator<Item = Quad>> = if quads {
            Box::new(oxttl::NQuadsParser::new().for_reader(open()).map(|r| ox::quad(r.unwrap_or_else(|e| panic!("oxttl failed: {e}")))))
        } else {
            Box::new(oxttl::NTriplesParser::new().for_reader(open()).map(|r| ox::triple(r.unwrap_or_else(|e| panic!("oxttl failed: {e}"))).in_graph(GraphName::DefaultGraph)))
        };
        let (mut a, mut b) = (ours_it, theirs_it);
        let mut n = 0u64;
        loop {
            match (a.next(), b.next()) {
                (None, None) => break,
                (Some(x), Some(y)) => { assert_eq!(x, y, "{} statement {n}", path.display()); n += 1 }
                (x, y) => panic!("{}: one side ended at statement {n}: ours {x:?}, oxttl {y:?}", path.display()),
            }
        }
        eprintln!("{}: {n} statements identical", path.display());
    }
}

/// Descriptive parse-only timing of both parsers on `TL_NT_FILES` (run with `--release --ignored --nocapture`).
#[test]
#[ignore]
fn parse_time() {
    let Some(files) = std::env::var_os("TL_NT_FILES") else { return };
    for path in std::env::split_paths(&files) {
        let data = std::fs::read(&path).unwrap();
        for round in 0..3 {
            let t = std::time::Instant::now();
            let n = read_ntriples(&data[..]).filter(|r| r.is_ok()).count();
            let ours = t.elapsed();
            let t = std::time::Instant::now();
            let m = oxttl::NTriplesParser::new().for_slice(&data).filter(|r| r.is_ok()).count();
            let theirs = t.elapsed();
            assert_eq!(n, m);
            eprintln!("{} round {round}: ours {:.2} s, oxttl {:.2} s ({n} triples)", path.display(), ours.as_secs_f64(), theirs.as_secs_f64());
        }
    }
}

/// Invalid UTF-8 in a token is a syntax error, as with `oxttl` (so loads report `Syntax`, not `Storage`).
#[test]
fn invalid_utf8_is_a_syntax_error() {
    use triplelight_core::rdf::ReadError;
    for doc in [&b"<http://a\xff> <http://b> <http://c> .\n"[..], b"<http://a> <http://b> \"\xff\" .\n", b"_:a\xff <http://b> <http://c> .\n"] {
        assert!(matches!(read_ntriples(doc).find_map(Result::err), Some(ReadError::Syntax(_))), "{doc:?}");
        assert!(oxttl::NTriplesParser::new().for_slice(doc).any(|r| r.is_err()), "{doc:?}");
    }
}
