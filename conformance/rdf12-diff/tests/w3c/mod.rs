use oxrdf::{Graph, NamedNodeRef, NamedOrBlankNodeRef, TermRef};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const COMMIT: &str = "56076c0efec4c353efcae2971be8f6005294ac64";
const ORIGIN: &str = "https://w3c.github.io/rdf-tests/";
pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";

pub fn fixture(root: &Path, iri: &str) -> Vec<u8> {
    let relative = iri.strip_prefix(ORIGIN).expect("W3C fixture IRI");
    fs::read(root.join(relative)).unwrap_or_else(|error| panic!("{iri}: {error}"))
}

pub fn property<'a>(
    graph: &'a Graph,
    subject: NamedOrBlankNodeRef<'a>,
    prefix: &str,
    local: &str,
) -> TermRef<'a> {
    let predicate = format!("{prefix}{local}");
    let mut matches =
        graph.objects_for_subject_predicate(subject, NamedNodeRef::new_unchecked(&predicate));
    let value = matches
        .next()
        .unwrap_or_else(|| panic!("missing {local} on {subject}"));
    assert!(
        matches.next().is_none(),
        "multiple {local} values on {subject}"
    );
    value
}

pub fn node(term: TermRef<'_>) -> NamedOrBlankNodeRef<'_> {
    match term {
        TermRef::NamedNode(value) => value.into(),
        TermRef::BlankNode(value) => value.into(),
        _ => panic!("expected IRI or blank node: {term}"),
    }
}

pub fn iri(term: TermRef<'_>) -> &str {
    match term {
        TermRef::NamedNode(value) => value.as_str(),
        _ => panic!("expected an IRI: {term}"),
    }
}

pub fn pinned_checkout() -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("TL_RDF_TESTS").expect("set TL_RDF_TESTS to the pinned checkout"),
    );
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(head.status.success());
    assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), COMMIT);
    root
}

pub fn load_manifest(root: &Path, relative: &str) -> Graph {
    let path = format!("{ORIGIN}{relative}");
    let data = fixture(root, &path);
    oxttl::TurtleParser::new()
        .with_base_iri(path)
        .unwrap()
        .for_slice(&data)
        .collect::<Result<_, _>>()
        .unwrap()
}

pub fn entries(graph: &Graph) -> Vec<NamedOrBlankNodeRef<'_>> {
    let predicate = NamedNodeRef::new_unchecked(
        "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#entries",
    );
    let mut heads = graph
        .triples_for_predicate(predicate)
        .map(|triple| triple.object);
    let mut next = heads.next().expect("manifest entry list");
    assert!(heads.next().is_none());
    let mut visited_cells = HashSet::new();
    let mut visited_entries = HashSet::new();
    let mut entries = Vec::new();
    while next != TermRef::NamedNode(oxrdf::vocab::rdf::NIL) {
        let cell = node(next);
        assert!(
            visited_cells.insert(cell),
            "cycle in manifest list at {cell}"
        );
        let entry = node(property(graph, cell, RDF, "first"));
        next = property(graph, cell, RDF, "rest");
        assert!(visited_entries.insert(entry), "repeated case {entry}");
        entries.push(entry);
    }
    entries
}
