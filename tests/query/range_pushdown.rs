//! Range-filter pushdown (`Options::range_pushdown`, crates/triplelight-exec/src/range.rs): a pattern `?s <p> ?d` under
//! FILTER bounds on `?d` with `xsd:date` constants reads only the object IDs that can pass them. Every test checks
//! that answers are identical with the switch on and off (and to the in-memory store, which never pushes down), and
//! the scan-row counter shows when the pushdown is (or must not be) taken.
use triplelight::exec::{query_with, Options, QueryResult};
use triplelight::model::{Perm, TermId, TripleBatch};
use triplelight::opt::Stats;
use triplelight::storage::{write_disk_snapshot, Dictionary, DiskBase, DiskSnapshot, MemorySnapshot, Snapshot, StorageError};
use triplelight::store::{Store, StoreUpdate};
use triplelight_rdf::{Term, TermRef};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const DATE: &str = "http://www.w3.org/2001/XMLSchema#date";

/// An `xsd:date` literal in N-Triples / SPARQL syntax.
fn d(form: &str) -> String { format!("\"{form}\"^^<{DATE}>") }

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("triplelight-range-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A unified-dictionary disk base (the only layout with text-ordered literal IDs), removed on drop.
struct Disk { dir: PathBuf, snap: DiskSnapshot }
impl Drop for Disk { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); } }
fn disk(name: &str, nt: &str) -> Disk {
    let dir = tmp(name);
    write_disk_snapshot(&dir, &memory(nt)).unwrap();
    let snap = DiskSnapshot::open(&dir).unwrap();
    Disk { dir, snap }
}
fn memory(nt: &str) -> MemorySnapshot { MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap() }

/// Forwards everything to `inner` and counts its scans and the triples they return.
struct Counting<'a> { inner: &'a dyn Snapshot, rows: AtomicUsize, scans: AtomicUsize }
fn counting(inner: &dyn Snapshot) -> Counting<'_> { Counting { inner, rows: AtomicUsize::new(0), scans: AtomicUsize::new(0) } }
impl Dictionary for Counting<'_> {
    fn lookup(&self, t: &Term) -> Result<Option<TermId>, StorageError> { self.inner.lookup(t) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { self.inner.literal_text_bound(text, strict) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.inner.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_terms(ids, f) }
}
impl Snapshot for Counting<'_> {
    fn scan<'a>(&'a self, p: Perm, k: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        Box::new(self.inner.scan(p, k).inspect(|b| if let Ok(b) = b { self.rows.fetch_add(b.len(), Ordering::Relaxed); }))
    }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> { self.inner.scan_for_stats() }
    fn count(&self, p: Perm, k: &[TermId]) -> Result<u64, StorageError> { self.inner.count(p, k) }
    fn seek(&self, p: Perm, k: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.inner.seek(p, k, t) }
    fn has_rdf12_terms(&self) -> bool { self.inner.has_rdf12_terms() }
}

fn opts(range_pushdown: bool) -> Options<'static> { Options { range_pushdown, ..Default::default() } }

/// Solutions as sorted display strings.
fn answers<S: Snapshot>(s: &S, q: &str, o: &Options<'_>) -> Vec<String> {
    match query_with(s, q, o).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows.iter().map(|r| r.iter().map(|t| t.as_ref().map_or("-".into(), |t| t.to_string())).collect::<Vec<_>>().join(" ")).collect();
            v.sort();
            v
        }
        QueryResult::Boolean(b) => vec![b.to_string()],
        r => panic!("{r:?}"),
    }
}

/// Solutions as display strings in result order.
fn ordered<S: Snapshot>(s: &S, q: &str, o: &Options<'_>) -> Vec<String> {
    match query_with(s, q, o).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryResult::Solutions { rows, .. } => rows.iter().map(|r| r.iter().map(|t| t.as_ref().map_or("-".into(), |t| t.to_string())).collect::<Vec<_>>().join(" ")).collect(),
        QueryResult::Boolean(b) => vec![b.to_string()],
        r => panic!("{r:?}"),
    }
}

/// Answers with the pushdown on and off (which must agree) and the triples scanned by each.
fn on_off(s: &dyn Snapshot, q: &str) -> (Vec<String>, usize, usize) {
    let (on, off) = (counting(s), counting(s));
    let a = answers(&on, q, &opts(true));
    let b = answers(&off, q, &opts(false));
    assert_eq!(a, b, "pushdown changed the answers of {q}");
    (a, on.rows.load(Ordering::Relaxed), off.rows.load(Ordering::Relaxed))
}

/// Review `i`'s date: 2007-01-01 .. 2007-11-20, 28 days a month.
fn form(i: usize) -> String { format!("2007-{:02}-{:02}", 1 + (i / 28) % 12, 1 + i % 28) }
const REVIEWS: usize = 300;

/// Reviews with canonical dates on `<urn:date>`, plus objects of other kinds and datatypes whose text sorts among
/// them, and a non-canonical date on another predicate (which must not disable the pushdown for `<urn:date>`).
fn base_nt() -> String {
    let mut nt = String::new();
    for i in 0..REVIEWS {
        nt += &format!("<urn:r{i}> <urn:date> {} .\n<urn:r{i}> <urn:for> <urn:p{}> .\n", d(&form(i)), i % 7);
    }
    nt += "<urn:x1> <urn:date> \"2007-10-15\" .\n";
    nt += "<urn:x2> <urn:date> \"2007-10-15T10:00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime> .\n";
    nt += "<urn:x3> <urn:date> \"2007-10-15\"@en .\n";
    nt += "<urn:x4> <urn:date> <urn:2007-10-15> .\n";
    nt += "<urn:x5> <urn:date> \"2007-10-15\"^^<urn:mydate> .\n";
    nt += "<urn:x6> <urn:date> \"20071015\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n";
    nt += &format!("<urn:x7> <urn:other> {} .\n", d("2007-10-15Z"));
    nt
}

fn select(cond: &str) -> String { format!("SELECT ?r ?d WHERE {{ ?r <urn:date> ?d FILTER({cond}) }}") }

#[test]
fn literal_text_bound_is_the_text_order_lower_bound() {
    let base = disk("bound", &base_nt());
    // Every stored literal with its text, in ID order; within the literal tag that must be text order.
    let mut literals: Vec<(TermId, String)> = Vec::new();
    for b in base.snap.scan(Perm::Pos, &[]) {
        for &o in &b.unwrap().cols[1] {
            if o.0 >> 60 == 3 && literals.last().is_none_or(|l| l.0 != o) { literals.push((o, base.snap.resolve(o).unwrap().to_string())) }
        }
    }
    literals.sort();
    literals.dedup();
    assert!(literals.windows(2).all(|w| w[0].1 < w[1].1), "literal IDs are not in text order");
    let end = TermId(4 << 60);
    let probes = [d("2007-10-10"), d("2007-10-29"), d("1990-01-01"), d("2100-01-01"), "\"2007-10-15\"".into(), "\"".into(), "\"\u{10FFFF}".into()];
    for text in probes {
        for strict in [false, true] {
            let want = literals.iter().find(|(_, t)| if strict { *t > text } else { *t >= text }).map_or(end, |l| l.0);
            assert_eq!(base.snap.literal_text_bound(&text, strict).unwrap(), Some(want), "{text} strict={strict}");
        }
    }
    // Stores without text-ordered IDs make no promise.
    assert_eq!(memory(&base_nt()).literal_text_bound(&d("2007-10-10"), false).unwrap(), None);
}

#[test]
fn each_operator_and_boundary_matches_the_filter() {
    let base = disk("ops", &base_nt());
    let reference = memory(&base_nt());
    let c = "2007-10-10";
    let forms: Vec<String> = (0..REVIEWS).map(form).collect();
    let expect = |f: &dyn Fn(&str) -> bool| forms.iter().filter(|x| f(x.as_str())).count();
    let cases: Vec<(String, usize)> = vec![
        (format!("?d >= {}", d(c)), expect(&|x| x >= c)),
        (format!("?d > {}", d(c)), expect(&|x| x > c)),
        (format!("?d <= {}", d(c)), expect(&|x| x <= c)),
        (format!("?d < {}", d(c)), expect(&|x| x < c)),
        // Mirrored comparisons.
        (format!("{} <= ?d", d(c)), expect(&|x| x >= c)),
        (format!("{} < ?d", d(c)), expect(&|x| x > c)),
        (format!("{} >= ?d", d(c)), expect(&|x| x <= c)),
        (format!("{} > ?d", d(c)), expect(&|x| x < c)),
        // Both bounds, as in BSBM BI q3; a bound absent from the dictionary (no day 29 is stored).
        (format!("?d >= {} && ?d < {}", d(c), d("2007-11-07")), expect(&|x| x >= c && x < "2007-11-07")),
        (format!("?d > {} && ?d <= {}", d("2007-10-29"), d("2007-11-03")), expect(&|x| x > "2007-10-29" && x <= "2007-11-03")),
        (format!("?d >= {} && ?d <= {}", d("2007-10-15"), d("2007-10-15")), 1),
        // Interval ends at the first and last stored date.
        (format!("?d >= {}", d(&form(0))), REVIEWS),
        (format!("?d <= {}", d(&form(REVIEWS - 1))), REVIEWS),
        (format!("?d < {}", d(&form(0))), 0),
        (format!("?d > {}", d(&form(REVIEWS - 1))), 0),
    ];
    for (cond, n) in cases {
        let q = select(&cond);
        let (got, _, _) = on_off(&base.snap, &q);
        assert_eq!(got.len(), n, "{q}");
        assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}");
        // Only xsd:date literals can satisfy a comparison with an xsd:date constant.
        assert!(got.iter().all(|r| r.starts_with("<urn:r") && r.ends_with(&format!("^^<{DATE}>"))), "{q}: {got:?}");
    }
}

#[test]
fn pushdown_reads_only_the_range() {
    let base = disk("reads", &base_nt());
    let q = select(&format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-15")));
    let (got, on, off) = on_off(&base.snap, &q);
    assert_eq!(got.len(), 5);
    // Five dates, plus the string and language-tagged "2007-10-15" whose text sorts inside the ID interval (removed
    // by the residual filter); without the pushdown every object of <urn:date> is read.
    assert!(on <= 10, "pushdown read {on} rows");
    assert!(off >= REVIEWS, "full scan read only {off} rows");
    // In a join, with the ranged pattern ordered first.
    let q = format!("SELECT ?r ?p WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER(?d >= {} && ?d < {}) }}", d("2007-10-10"), d("2007-10-15"));
    let (got, on, off) = on_off(&base.snap, &q);
    assert_eq!(got.len(), 5);
    assert!(on * 10 < off, "join read {on} rows with pushdown, {off} without");
}

#[test]
fn other_datatypes_and_negations_are_not_pushed() {
    let base = disk("other", &base_nt());
    let reference = memory(&base_nt());
    let dt = "<http://www.w3.org/2001/XMLSchema#dateTime>";
    for cond in [
        // Not order-preserving constants: answered by the filter alone.
        "?d >= \"2007-10-10\"".to_string(),
        format!("?d >= \"2007-10-15T00:00:00\"^^{dt}"),
        format!("?d >= {}", d("2007-10-10Z")),
        format!("?d >= {}", d("-2007-10-10")),
        "?d >= \"2007-10-15\"^^<urn:mydate>".to_string(),
        // Negation and disjunction are not bounds.
        format!("!(?d < {})", d("2007-10-10")),
        format!("?d < {} || ?d > {}", d("2007-02-01"), d("2007-10-10")),
        // A bound on a variable no pattern has as object.
        format!("?r >= {}", d("2007-10-10")),
    ] {
        let q = select(&cond);
        let (got, on, off) = on_off(&base.snap, &q);
        assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}");
        assert_eq!(on, off, "{q} was pushed down");
    }
    // A pushable bound next to one that is not: the range comes from the date bound alone, the other conjunct is
    // only a filter.
    let q = select(&format!("?d >= {} && ?d < \"2008-01-01T00:00:00\"^^{dt}", d("2007-10-10")));
    assert_eq!(on_off(&base.snap, &q).0, answers(&reference, &q, &opts(true)), "{q}");
    // `!(?d < c)` keeps exactly the dates `>= c`: every other object is a type error, and ! of an error is an error.
    let neg = on_off(&base.snap, &select(&format!("!(?d < {})", d("2007-10-10")))).0;
    assert_eq!(neg, on_off(&base.snap, &select(&format!("?d >= {}", d("2007-10-10")))).0);
}

#[test]
fn empty_ranges() {
    let base = disk("empty", &base_nt());
    for cond in [
        format!("?d > {} && ?d < {}", d("2007-10-10"), d("2007-10-01")),
        format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-10")),
        format!("?d > {} && ?d < {}", d("2007-10-10"), d("2007-10-11")),
        format!("?d >= {} && ?d <= {}", d("1990-01-01"), d("1990-12-31")),
        format!("?d >= {}", d("2100-01-01")),
        format!("?d < {}", d("1000-01-01")),
    ] {
        let q = select(&cond);
        let (got, on, _) = on_off(&base.snap, &q);
        assert!(got.is_empty(), "{q}: {got:?}");
        assert!(on <= 2, "{q}: an empty range read {on} rows");
        // In a join and an aggregate, the empty pattern empties everything.
        let q = format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER({cond}) }} GROUP BY ?p");
        assert_eq!(on_off(&base.snap, &q).0, answers(&memory(&base_nt()), &q, &opts(true)), "{q}");
    }
}

#[test]
fn a_non_canonical_date_disables_the_pushdown() {
    let q_lower = select(&format!("?d >= {}", d("2007-10-10")));
    let q_upper = select(&format!("?d < {}", d("2007-10-11")));
    let q_both = select(&format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-11-07")));
    // N-Triples text of each lexical form; `\n` is an escaped newline.
    for (i, bad) in ["2007-10-15Z", " 2007-10-15", "2007-10-10 ", "-2007-10-15", "12007-10-15", "2007-10-1\\n", "2007-10-15+01:00", "2007-10-15 "].into_iter().enumerate() {
        let nt = format!("{}<urn:bad> <urn:date> \"{bad}\"^^<{DATE}> .\n", base_nt());
        let base = disk(&format!("bad{i}"), &nt);
        let reference = memory(&nt);
        for q in [&q_lower, &q_upper, &q_both] {
            let (got, on, off) = on_off(&base.snap, q);
            assert_eq!(got, answers(&reference, q, &opts(true)), "{bad:?}: {q}");
            assert_eq!(on, off, "{bad:?}: {q} was pushed down");
        }
    }
    // The forms whose text sorts outside the interval although the filter keeps them: the reason for the check.
    let nt = format!("{}<urn:bad> <urn:date> \"2007-10-10 \"^^<{DATE}> .\n", base_nt());
    assert!(on_off(&disk("trailing", &nt).snap, &q_lower).0.iter().any(|r| r.starts_with("<urn:bad>")));
    let nt = format!("{}<urn:bad> <urn:date> \"2007-10-1\\n\"^^<{DATE}> .\n", base_nt());
    assert!(on_off(&disk("escaped", &nt).snap, &q_upper).0.iter().any(|r| r.starts_with("<urn:bad>")));
}

#[test]
fn updates_in_the_delta_are_returned() {
    let dir = tmp("delta");
    std::fs::create_dir_all(&dir).unwrap();
    let init = dir.join("init.nt");
    std::fs::write(&init, base_nt()).unwrap();
    let store = Store::open_with(dir.join("db"), Some(&init), Box::new(DiskBase { workers: 1 })).unwrap();
    store.update(&format!(
        "INSERT DATA {{ <urn:new1> <urn:date> {} . <urn:new2> <urn:date> {} . <urn:new3> <urn:date> {} . <urn:new4> <urn:date> \"2007-10-30\" . <urn:new1> <urn:for> <urn:p0> }}",
        // A date first seen in the delta (no day 29 is in the base), one already in the base dictionary, and a
        // non-canonical one in the delta (delta IDs are always scanned, so it does not need the check).
        d("2007-10-29"), d(&form(272)), d("2007-10-23Z"),
    )).unwrap();
    // A base triple in the range deleted in the delta.
    store.update(&format!("DELETE DATA {{ <urn:r273> <urn:date> {} }}", d(&form(273)))).unwrap();
    // Narrow enough (8 stored dates and 3 delta objects) to be read by one probe per object.
    let cond = format!("?d >= {} && ?d <= {}", d("2007-10-20"), d("2007-10-31"));
    let q = select(&cond);
    let check = |snap: &dyn Snapshot, pushed: bool| {
        let (got, on, off) = on_off(snap, &q);
        for s in ["<urn:new1>", "<urn:new2>", "<urn:new3>"] { assert!(got.iter().any(|r| r.starts_with(s)), "{s} missing: {got:?}") }
        for s in ["<urn:new4>", "<urn:r273>"] { assert!(!got.iter().any(|r| r.starts_with(s)), "{s} present: {got:?}") }
        assert_eq!(on < off, pushed, "pushed down: {on} rows vs {off}");
        let joined = format!("SELECT ?r ?p WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({cond}) }}");
        assert!(on_off(snap, &joined).0.iter().any(|r| r.starts_with("<urn:new1>")));
        got
    };
    let before = check(&*store.snapshot(), true);
    // After compaction "2007-10-23Z" is a base literal: no pushdown, same answers.
    store.compact().unwrap();
    assert_eq!(check(&*store.snapshot(), false), before);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

/// A dense range (almost every date) in front of a join whose other pattern has many more rows than the range: the
/// shape of BSBM BI q3 on its dense windows, where reading the range first and then probing once per review was
/// slower than scanning both patterns. Work counts each scan call (one index probe) as `PROBE_ROWS` = 20 rows,
/// the executor's own cost model; the ranged plan must do no more work than the plan without the range.
#[test]
fn a_dense_range_costs_no_more_than_no_range() {
    let reviews = 3000;
    let mut nt = String::new();
    for i in 0..reviews { nt += &format!("<urn:r{i}> <urn:date> {} .\n<urn:r{i}> <urn:for> <urn:p{}> .\n", d(&form(i)), i % 50); }
    // Many more <urn:for> triples than reviews, so joining by probes and joining by one scan really differ.
    for i in 0..3 * reviews { nt += &format!("<urn:o{i}> <urn:for> <urn:p{}> .\n", i % 50); }
    let base = disk("dense", &nt);
    let reference = memory(&nt);
    let work = |q: &str, on: bool| {
        let s = counting(&base.snap);
        let got = answers(&s, q, &opts(on));
        (got, s.rows.load(Ordering::Relaxed) + 20 * s.scans.load(Ordering::Relaxed))
    };
    for cond in [format!("?d >= {}", d("2007-01-05")), format!("?d > {} && ?d <= {}", d("2007-01-01"), d("2008-01-01")), format!("?d < {}", d("2007-11-20"))] {
        for q in [
            format!("SELECT ?r ?p WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER({cond}) }}"),
            format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({cond}) }} GROUP BY ?p"),
        ] {
            let ((a, on), (b, off)) = (work(&q, true), work(&q, false));
            assert_eq!(a, b, "{q}");
            assert_eq!(a, answers(&reference, &q, &opts(true)), "{q}");
            assert!(on <= off, "{q}: work {on} with the range, {off} without");
        }
    }
}

/// The selectivity gate: a range holding more than 15% of the predicate's triples is not pushed, so the scans are
/// exactly those of the unranged plan; at or below it, the range is pushed.
#[test]
fn a_range_over_the_fraction_takes_the_unranged_plan() {
    let base = disk("fraction", &base_nt());
    let scans = |q: &str, on: bool| {
        let s = counting(&base.snap);
        let got = answers(&s, q, &opts(on));
        (got, s.rows.load(Ordering::Relaxed), s.scans.load(Ordering::Relaxed))
    };
    // <urn:date> has 306 triples (300 dates and 6 others), so the gate is 45 rows. From 2007-06-01 on is 160 dates
    // plus 5 other literals in the ID interval, March-April and January-February 56 dates each: not pushed.
    // 2007-10-10 to 2007-10-14 is 5 dates (under 2%): pushed.
    for (cond, pushed) in [
        (format!("?d >= {}", d("2007-06-01")), false),
        (format!("?d >= {} && ?d < {}", d("2007-03-01"), d("2007-05-01")), false),
        (format!("?d < {}", d("2007-03-01")), false),
        (format!("?d >= {} && ?d <= {}", d("2007-10-10"), d("2007-10-14")), true),
    ] {
        for q in [select(&cond), format!("SELECT ?r ?p WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER({cond}) }}")] {
            let ((a, on_rows, on_scans), (b, off_rows, off_scans)) = (scans(&q, true), scans(&q, false));
            assert_eq!(a, b, "{q}");
            if pushed {
                assert!(on_rows < off_rows, "{q}: not pushed ({on_rows} rows vs {off_rows})");
            } else {
                assert_eq!((on_rows, on_scans), (off_rows, off_scans), "{q}: a dense range changed the plan");
            }
        }
    }
}

/// Jianlin's fixture (PR #202 review): in subject order the three selected values are MAX, 1, -1 (the checked SUM
/// overflows: unbound); in date order MAX, -1, 1 (the SUM is MAX). SUM is order-sensitive, so the BGP under it is not
/// pushed and the answer is the unranged plan's. (Jianlin's 20 filler dates are 200 here, so that the pushed COUNT
/// reads its three rows by object rather than through the dense-range scan.)
#[test]
fn an_order_sensitive_sum_is_not_pushed() {
    let mut nt = String::new();
    for (s, date) in [("a", "2007-01-01"), ("b", "2007-01-03"), ("c", "2007-01-02")] { nt += &format!("<urn:{s}> <urn:date> {} .\n", d(date)); }
    for i in 0..200 { nt += &format!("<urn:z{i:03}> <urn:date> {} .\n", d("2008-01-01")); }
    let q = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT (SUM(IF(?s = <urn:a>, 9223372036854775807, IF(?s = <urn:b>, 1, -1))) AS ?total)
WHERE { ?s <urn:date> ?d . FILTER(?d < \"2008-01-01\"^^xsd:date) }";
    let base = disk("ordered-sum", &nt);
    let (got, on, off) = on_off(&base.snap, q);
    assert_eq!((on, off), (203, 203), "the SUM's BGP was pushed down");
    assert_eq!(got, vec!["-".to_string()], "the checked SUM over MAX, 1, -1 overflows");
    assert_eq!(ordered(&base.snap, q, &opts(true)), ordered(&memory(&nt), q, &opts(true)));
    // The same BGP under COUNT is pushed (three rows read).
    let count = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:date> ?d . FILTER(?d < \"2008-01-01\"^^xsd:date) }";
    let (got, on, _) = on_off(&base.snap, count);
    assert_eq!(on, 3);
    assert_eq!(got.len(), 1);
}

/// Which consumers allow the pushdown: plain SELECT and COUNT (through a subquery too) do; SUM, AVG, MIN, MAX,
/// SAMPLE and GROUP_CONCAT do not, in a multi-pattern BGP with statistics on; nor a LIMIT without ORDER BY, or an
/// ORDER BY that can tie (a plain variable, or a group key bound to literals).
#[test]
fn only_order_insensitive_consumers_are_pushed() {
    let mut nt = base_nt();
    for i in 0..REVIEWS {
        nt += &format!("<urn:r{i}> <urn:n> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i % 5);
        // A group key that is 1 for some reviews and 1.0 for others: distinct keys that ORDER BY ties.
        let k = if i % 2 == 0 { "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>" } else { "\"1.0\"^^<http://www.w3.org/2001/XMLSchema#decimal>" };
        nt += &format!("<urn:r{i}> <urn:key> {k} .\n");
    }
    let base = disk("consumers", &nt);
    let stats = Stats::from_snapshot(&base.snap).unwrap();
    let range = format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-15"));
    let bgp = format!("?r <urn:for> ?p . ?r <urn:date> ?d . ?r <urn:n> ?n FILTER({range})");
    let run = |q: &str, on: bool, stats_on: bool| {
        let s = counting(&base.snap);
        let o = Options { stats: stats_on.then_some(&stats), ..opts(on) };
        let got = ordered(&s, q, &o);
        (got, s.rows.load(Ordering::Relaxed), s.scans.load(Ordering::Relaxed))
    };
    let q3 = format!(
        "SELECT ?p ?a ?b WHERE {{ {{ SELECT ?p (COUNT(?r) AS ?a) WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER({range}) }} GROUP BY ?p }} \
         {{ SELECT ?p (COUNT(?r) AS ?b) WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER(?d >= {} && ?d < {}) }} GROUP BY ?p HAVING (COUNT(?r) > 0) }} }} \
         ORDER BY DESC(?a / ?b) ?p LIMIT 10", d("2007-10-01"), d("2007-10-10"));
    let pushed = [
        format!("SELECT ?r ?p ?n WHERE {{ {bgp} }}"),
        format!("SELECT ?p (COUNT(?r) AS ?c) (COUNT(DISTINCT ?n) AS ?k) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (COUNT(*) AS ?c) WHERE {{ {bgp} }} GROUP BY ?p ORDER BY ?p LIMIT 2"),
        // BSBM BI q3's shape: two COUNT subqueries joined on their key, ordered by the key under LIMIT.
        q3,
    ];
    let not_pushed = [
        format!("SELECT ?p (SUM(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (AVG(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (MIN(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (MAX(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (SAMPLE(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (GROUP_CONCAT(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        format!("SELECT ?p (COUNT(?r) AS ?c) (SUM(?n) AS ?x) WHERE {{ {bgp} }} GROUP BY ?p"),
        // SUM over a COUNT subquery: the subquery's group order reaches the fold.
        format!("SELECT (SUM(?c) AS ?x) WHERE {{ {{ SELECT ?p (COUNT(?r) AS ?c) WHERE {{ {bgp} }} GROUP BY ?p }} }}"),
        // LIMIT without ORDER BY, and ORDER BY with ties under LIMIT.
        format!("SELECT ?r WHERE {{ {bgp} }} LIMIT 1"),
        // ASK is a LIMIT 1 without ORDER BY in the algebra; its answer is order-free, but the gate does not prove that yet.
        format!("ASK {{ {bgp} }}"),
        format!("SELECT ?r WHERE {{ {bgp} }} ORDER BY ?d LIMIT 1"),
        // Group key bound to literals (1 and 1.0 are distinct groups that ORDER BY ties).
        format!("SELECT ?k (COUNT(?r) AS ?c) WHERE {{ ?r <urn:date> ?d . ?r <urn:key> ?k FILTER({range}) }} GROUP BY ?k ORDER BY ?k LIMIT 1"),
    ];
    for stats_on in [false, true] {
        for q in &pushed {
            let ((a, on, _), (b, off, _)) = (run(q, true, stats_on), run(q, false, stats_on));
            assert_eq!(sorted(a), sorted(b), "{q}");
            assert!(on < off, "{q} (stats {stats_on}) was not pushed: {on} rows vs {off}");
        }
        for q in &not_pushed {
            let ((a, on_rows, on_scans), (b, off_rows, off_scans)) = (run(q, true, stats_on), run(q, false, stats_on));
            assert_eq!(a, b, "{q}");
            assert_eq!((on_rows, on_scans), (off_rows, off_scans), "{q} (stats {stats_on}) was pushed down");
        }
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> { v.sort(); v }

/// The tie-free proof needs every grouping key bound in every group. Grouped by an OPTIONAL key, the left subquery has
/// the groups UNBOUND and <urn:t0>; both join the right subquery's <urn:t0> group, so two rows share ?k = <urn:t0>
/// and `ORDER BY ?k LIMIT 1` picks by input order: not pushed. The same shape with a mandatory key is pushed.
#[test]
fn an_unbound_group_key_is_not_a_uniqueness_key() {
    let mut nt = base_nt();
    for i in 0..REVIEWS {
        nt += &format!("<urn:r{i}> <urn:tag> <urn:t{}> .\n", i % 3);
        if i % 2 == 0 { nt += &format!("<urn:r{i}> <urn:opt> <urn:t0> .\n"); }
    }
    let base = disk("unbound-key", &nt);
    let range = format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-15"));
    let run = |q: &str, on: bool| {
        let s = counting(&base.snap);
        let got = ordered(&s, q, &opts(on));
        (got, s.rows.load(Ordering::Relaxed), s.scans.load(Ordering::Relaxed))
    };
    let optional = format!(
        "SELECT ?k ?a ?b WHERE {{ {{ SELECT ?k (COUNT(?r) AS ?a) WHERE {{ {{ ?r <urn:date> ?d FILTER({range}) }} OPTIONAL {{ ?r <urn:opt> ?k }} }} GROUP BY ?k }} \
         {{ SELECT ?k (COUNT(?x) AS ?b) WHERE {{ ?x <urn:opt> ?k }} GROUP BY ?k }} }} ORDER BY ?k LIMIT 1");
    let ((a, on_rows, on_scans), (b, off_rows, off_scans)) = (run(&optional, true), run(&optional, false));
    assert_eq!(a, b, "{optional}");
    assert_eq!((on_rows, on_scans), (off_rows, off_scans), "the OPTIONAL-keyed join was pushed down");
    let mandatory = format!(
        "SELECT ?k ?a ?b WHERE {{ {{ SELECT ?k (COUNT(?r) AS ?a) WHERE {{ ?r <urn:date> ?d . ?r <urn:tag> ?k FILTER({range}) }} GROUP BY ?k }} \
         {{ SELECT ?k (COUNT(?x) AS ?b) WHERE {{ ?x <urn:tag> ?k }} GROUP BY ?k }} }} ORDER BY ?k LIMIT 1");
    let ((a, on, _), (b, off, _)) = (run(&mandatory, true), run(&mandatory, false));
    assert_eq!(a, b, "{mandatory}");
    assert!(on < off, "the mandatory-keyed join was not pushed: {on} rows vs {off}");
}

/// The IRI-only proof for an object key is checked on the default graph. Under GRAPH the key comes from a named
/// graph, where the same predicate has the literal objects 1 and 1.0 (distinct groups that ORDER BY ties): not pushed.
#[test]
fn a_key_from_a_named_graph_is_not_proven() {
    let dir = tmp("graph-key");
    std::fs::create_dir_all(&dir).unwrap();
    let init = dir.join("init.nt");
    std::fs::write(&init, format!("{}<urn:y> <urn:g> <urn:v1> .\n", base_nt())).unwrap();
    let store = Store::open_with(dir.join("db"), Some(&init), Box::new(DiskBase { workers: 1 })).unwrap();
    store.update("INSERT DATA { GRAPH <urn:G> { <urn:x1> <urn:g> 1 . <urn:x2> <urn:g> 1.0 } }").unwrap();
    let snap = store.snapshot();
    let range = format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-15"));
    let q = format!(
        "SELECT ?k ?c WHERE {{ {{ SELECT ?k (COUNT(?r) AS ?c) WHERE {{ GRAPH <urn:G> {{ ?x <urn:g> ?k }} {{ ?r <urn:date> ?d FILTER({range}) }} }} GROUP BY ?k }} }} \
         ORDER BY ?k LIMIT 1");
    let run = |on: bool| {
        let s = counting(&*snap);
        let got = match triplelight::exec::query_dataset_with(&s, &*snap, &q, &opts(on)).unwrap_or_else(|e| panic!("{q}: {e}")) {
            QueryResult::Solutions { rows, .. } => rows.iter().map(|r| r.iter().map(|t| t.as_ref().map_or("-".into(), |t| t.to_string())).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>(),
            r => panic!("{r:?}"),
        };
        (got, s.rows.load(Ordering::Relaxed), s.scans.load(Ordering::Relaxed))
    };
    let ((a, on_rows, on_scans), (b, off_rows, off_scans)) = (run(true), run(false));
    assert_eq!(a, b, "{q}");
    assert_eq!(a.len(), 1, "{q}");
    assert_eq!((on_rows, on_scans), (off_rows, off_scans), "{q} was pushed down");
    drop(snap);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

/// xorshift64*, as in the storage tests: deterministic and dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 { self.next() % n }
    fn date(&mut self) -> String {
        let y = match self.below(10) { 0 => 1990, 1 => 2050, _ => 1998 + self.below(14) };
        format!("{y:04}-{:02}-{:02}", 1 + self.below(12), 1 + self.below(28))
    }
}

#[test]
fn differential_random_ranges() {
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut nt = String::new();
    for s in 0..400 {
        for p in ["date", "date2"] {
            for _ in 0..rng.below(3) {
                let o = match rng.below(12) {
                    0 => format!("\"{}\"", rng.date()),
                    1 => format!("\"{}\"@en", rng.date()),
                    2 => format!("\"{}T12:00:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime>", rng.date()),
                    3 => format!("<urn:{}>", rng.date()),
                    _ => d(&rng.date()),
                };
                nt += &format!("<urn:s{s}> <urn:{p}> {o} .\n");
            }
        }
        nt += &format!("<urn:s{s}> <urn:for> <urn:p{}> .\n", rng.below(9));
        // Values whose SUM and AVG depend on the fold order: an integer overflow near i64::MAX, floats that cancel.
        let v = ["9223372036854775807", "9223372036854775806", "1", "-1", "-2", "3"][rng.below(6) as usize];
        let f = ["1e16", "1.0e0", "-1e16", "0.5e0"][rng.below(4) as usize];
        nt += &format!("<urn:s{s}> <urn:val> \"{v}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n");
        nt += &format!("<urn:s{s}> <urn:fval> \"{f}\"^^<http://www.w3.org/2001/XMLSchema#double> .\n");
    }
    let base = disk("random", &nt);
    let reference = memory(&nt);
    let stats = Stats::from_snapshot(&base.snap).unwrap();
    let ops = [">", ">=", "<", "<="];
    let mut pushed = 0;
    for trial in 0..300 {
        let bound = |v: &str, rng: &mut Rng| {
            let (op, c) = (ops[rng.below(4) as usize], d(&rng.date()));
            if rng.below(2) == 0 { format!("{v} {op} {c}") } else {
                let mirrored = match op { ">" => "<", ">=" => "<=", "<" => ">", _ => ">=" };
                format!("{c} {mirrored} {v}")
            }
        };
        let cond = |v: &str, rng: &mut Rng| match rng.below(3) {
            0 => bound(v, rng),
            _ => { let (a, b) = (bound(v, rng), bound(v, rng)); format!("{a} && {b}") }
        };
        let c = cond("?d", &mut rng);
        let q = match trial % 6 {
            0 => format!("SELECT ?s ?d WHERE {{ ?s <urn:date> ?d FILTER({c}) }}"),
            1 => format!("SELECT ?s ?d ?p WHERE {{ ?s <urn:date> ?d . ?s <urn:for> ?p FILTER({c}) }}"),
            2 => { let e = cond("?e", &mut rng); format!("SELECT ?s ?d ?e WHERE {{ ?s <urn:date> ?d . ?s <urn:date2> ?e FILTER({c} && {e}) }}") }
            3 => format!("SELECT ?p (COUNT(?s) AS ?n) WHERE {{ ?s <urn:for> ?p . ?s <urn:date> ?d FILTER({c}) }} GROUP BY ?p"),
            4 => format!("SELECT ?p (MIN(?d) AS ?lo) (MAX(?d) AS ?hi) (COUNT(*) AS ?n) WHERE {{ ?s <urn:for> ?p . ?s <urn:date> ?d FILTER({c}) }} GROUP BY ?p"),
            _ => format!("SELECT ?p (SUM(?v) AS ?t) (AVG(?f) AS ?a) WHERE {{ ?s <urn:date> ?d . ?s <urn:val> ?v . ?s <urn:fval> ?f . ?s <urn:for> ?p FILTER({c}) }} GROUP BY ?p"),
        };
        let (got, on, off) = on_off(&base.snap, &q);
        if on < off { pushed += 1 }
        if trial % 6 >= 4 {
            // Order-sensitive aggregates are never pushed: the same scans, and the same values in the same order.
            assert_eq!(on, off, "{q} was pushed down");
            assert_eq!(ordered(&base.snap, &q, &opts(true)), ordered(&base.snap, &q, &opts(false)), "{q}");
        }
        // The memory store folds in its own order, so SUM/AVG are compared with it only on the same plan.
        if trial % 6 != 5 { assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}"); }
        let with_stats = |on: bool| Options { stats: Some(&stats), ..opts(on) };
        assert_eq!(answers(&base.snap, &q, &with_stats(true)), got, "{q} (planner)");
        assert_eq!(answers(&base.snap, &q, &with_stats(false)), got, "{q} (planner, off)");
    }
    assert!(pushed > 30, "the pushdown was taken in only {pushed} trials");
}

/// Large enough for the cost-based planner (estimated greedy work >= 100,000), which must plan the ranged pattern
/// with its range's count and so read it first, whichever order the patterns are written in.
#[test]
fn planner_orders_the_ranged_pattern_first() {
    let n = 45_000;
    let mut nt = String::with_capacity(n * 120);
    for i in 0..n { nt += &format!("<urn:r{i}> <urn:date> {} .\n<urn:r{i}> <urn:for> <urn:p{}> .\n", d(&form(i)), i % 50); }
    let base = disk("planner", &nt);
    let stats = Stats::from_snapshot(&base.snap).unwrap();
    let range = format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-13"));
    for q in [
        format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <urn:for> ?p . ?r <urn:date> ?d FILTER({range}) }} GROUP BY ?p"),
        format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({range}) }} GROUP BY ?p"),
    ] {
        let run = |on: bool| {
            let s = counting(&base.snap);
            let got = answers(&s, &q, &Options { stats: Some(&stats), ..opts(on) });
            (got, s.rows.load(Ordering::Relaxed))
        };
        let ((a, on), (b, off)) = (run(true), run(false));
        assert_eq!(a, b, "{q}");
        assert!(!a.is_empty());
        assert!(on * 10 < off, "{q}: {on} rows with pushdown, {off} without");
    }
}

/// RDF 1.2 terms among the objects (a directional literal and a triple term holding a date) are never range
/// matches and do not change any answer.
#[cfg(feature = "rdf-12")]
#[test]
fn rdf12_terms_are_unaffected() {
    let nt = format!(
        "{}<urn:y1> <urn:date> \"2007-10-15\"@en--ltr .\n<urn:y2> <urn:date> <<( <urn:a> <urn:b> {} )>> .\n<urn:y3> <urn:says> <<( <urn:r261> <urn:date> {} )>> .\n",
        base_nt(), d("2007-10-15"), d(&form(261)),
    );
    let base = disk("rdf12", &nt);
    let reference = memory(&nt);
    let range = format!("?d >= {} && ?d < {}", d("2007-10-10"), d("2007-10-20"));
    for q in [
        select(&range),
        format!("SELECT ?r ?d ?y WHERE {{ ?y <urn:says> <<( ?r <urn:date> ?d )>> . ?r <urn:date> ?d FILTER({range}) }}"),
    ] {
        let got = answers(&base.snap, &q, &opts(true));
        assert_eq!(got, answers(&base.snap, &q, &opts(false)), "{q}");
        assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}");
        assert!(!got.iter().any(|r| r.contains("urn:y1") || r.contains("urn:y2")), "{q}: {got:?}");
    }
    assert_eq!(answers(&base.snap, &select(&range), &opts(true)).len(), 10);
}

/// The sort-merge join behind a pushed range (`extend_sort_merge`, #105): a selective date range over many
/// subjects that are not all reviews (as BSBM's `dc:date` covers reviews, products and offers), joined to
/// `?r <urn:for> ?p` whose range is too large for the whole-range hash table. Some reviews have two dates in the range
/// (duplicate keys on the left) and some have several products (several matches on the right); every combination
/// must appear exactly as often as without the range, and as in the memory store.
#[test]
fn a_sort_merge_after_a_range_keeps_every_multiplicity() {
    let mut nt = String::new();
    let days = 200; // each day about 0.5% of the dates, so a short window is pushed
    for i in 0..60_000usize {
        let day = 1 + i % days;
        let form = format!("2007-{:02}-{:02}", 1 + (day - 1) / 28 % 12, 1 + (day - 1) % 28);
        nt += &format!("<urn:s{i}> <urn:date> {} .\n", d(&form));
        if i % 3 == 0 {
            nt += &format!("<urn:s{i}> <urn:for> <urn:p{}> .\n", i % 97);
            if i % 9 == 0 { nt += &format!("<urn:s{i}> <urn:for> <urn:q{}> .\n", i % 89) }
        }
    }
    // Reviews with a second, different in-range date (a duplicate key on the left after the range).
    for i in (0..60_000usize).step_by(60) {
        let day = 1 + i % days;
        let form = format!("2007-{:02}-{:02}", 1 + (day - 1) / 28 % 12, 2 + (day - 1) % 27);
        nt += &format!("<urn:s{i}> <urn:date> {} .\n", d(&form));
    }
    // Many `for` triples on other subjects, so the right side is larger than twice the range's rows.
    for i in 0..40_000usize { nt += &format!("<urn:x{i}> <urn:for> <urn:p{}> .\n", i % 97) }
    let base = disk("sortmerge", &nt);
    let reference = memory(&nt);
    for cond in [format!("?d >= {} && ?d < {}", d("2007-01-01"), d("2007-01-15")), format!("?d >= {} && ?d <= {}", d("2007-03-01"), d("2007-03-20"))] {
        for q in [
            format!("SELECT ?r ?p WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({cond}) }}"),
            format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({cond}) }} GROUP BY ?p"),
        ] {
            let (got, _, _) = on_off(&base.snap, &q);
            assert!(!got.is_empty(), "{q}");
            assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}");
        }
    }
}
