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

/// Forwards everything to `inner` and counts the triples its scans return.
struct Counting<'a> { inner: &'a dyn Snapshot, rows: AtomicUsize }
fn counting(inner: &dyn Snapshot) -> Counting<'_> { Counting { inner, rows: AtomicUsize::new(0) } }
impl Dictionary for Counting<'_> {
    fn lookup(&self, t: &Term) -> Result<Option<TermId>, StorageError> { self.inner.lookup(t) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { self.inner.literal_text_bound(text, strict) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.inner.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_terms(ids, f) }
}
impl Snapshot for Counting<'_> {
    fn scan<'a>(&'a self, p: Perm, k: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
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
        format!("?d >= \"2007-10-15\"^^<urn:mydate>"),
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
        d("2007-10-29"), d(&form(263)), d("2007-10-13Z"),
    )).unwrap();
    // A base triple in the range deleted in the delta.
    store.update(&format!("DELETE DATA {{ <urn:r262> <urn:date> {} }}", d(&form(262)))).unwrap();
    let cond = format!("?d >= {} && ?d <= {}", d("2007-10-10"), d("2007-10-31"));
    let q = select(&cond);
    let check = |snap: &dyn Snapshot, pushed: bool| {
        let (got, on, off) = on_off(snap, &q);
        for s in ["<urn:new1>", "<urn:new2>", "<urn:new3>"] { assert!(got.iter().any(|r| r.starts_with(s)), "{s} missing: {got:?}") }
        for s in ["<urn:new4>", "<urn:r262>"] { assert!(!got.iter().any(|r| r.starts_with(s)), "{s} present: {got:?}") }
        assert_eq!(on < off, pushed, "pushed down: {on} rows vs {off}");
        let joined = format!("SELECT ?r ?p WHERE {{ ?r <urn:date> ?d . ?r <urn:for> ?p FILTER({cond}) }}");
        assert!(on_off(snap, &joined).0.iter().any(|r| r.starts_with("<urn:new1>")));
        got
    };
    let before = check(&*store.snapshot(), true);
    // After compaction "2007-10-13Z" is a base literal: no pushdown, same answers.
    store.compact().unwrap();
    assert_eq!(check(&*store.snapshot(), false), before);
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
        let q = match trial % 4 {
            0 => format!("SELECT ?s ?d WHERE {{ ?s <urn:date> ?d FILTER({c}) }}"),
            1 => format!("SELECT ?s ?d ?p WHERE {{ ?s <urn:date> ?d . ?s <urn:for> ?p FILTER({c}) }}"),
            2 => { let e = cond("?e", &mut rng); format!("SELECT ?s ?d ?e WHERE {{ ?s <urn:date> ?d . ?s <urn:date2> ?e FILTER({c} && {e}) }}") }
            _ => format!("SELECT ?p (COUNT(?s) AS ?n) WHERE {{ ?s <urn:for> ?p . ?s <urn:date> ?d FILTER({c}) }} GROUP BY ?p"),
        };
        let (got, on, off) = on_off(&base.snap, &q);
        if on < off { pushed += 1 }
        assert_eq!(got, answers(&reference, &q, &opts(true)), "{q}");
        let with_stats = |on: bool| Options { stats: Some(&stats), ..opts(on) };
        assert_eq!(answers(&base.snap, &q, &with_stats(true)), got, "{q} (planner)");
        assert_eq!(answers(&base.snap, &q, &with_stats(false)), got, "{q} (planner, off)");
    }
    assert!(pushed > 50, "the pushdown was taken in only {pushed} trials");
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
