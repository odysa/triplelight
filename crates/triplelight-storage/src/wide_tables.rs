//! Wide tables built from a view's SPO order (#140): an in-memory `TableCatalog`.
//!
//! Subjects are grouped by their set of `rdf:type` objects. A group becomes a table when its columns (the union of
//! its subjects' predicates) are filled well enough and it holds a large enough share of the triples; untyped
//! subjects and small or sparse groups stay triples-only. Each table keeps its subjects ascending, one CSR column
//! per predicate (offsets into a value array, values ascending per subject) and a bitset for `member`.
//! Built by two SPO scans; the view's indexes are untouched, so tables are only an extra access path.
use std::collections::{BTreeMap, HashMap};

use triplelight_core::model::{Perm, TermId};
use triplelight_core::wide::{ColumnMeta, ScanCounters, TableBatch, TableCatalog, TableId, TableMeta, TableScan, TABLE_BATCH_ROWS};

use crate::{Snapshot, StorageError};

/// Which groups become tables.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// Minimum share of a table's cells (subjects x columns) that hold a value.
    pub min_fill: f64,
    /// Minimum share of the view's triples a table must hold.
    pub min_share: f64,
}

impl Default for BuildOptions {
    fn default() -> Self { BuildOptions { min_fill: 0.6, min_share: 0.01 } }
}

struct Column {
    predicate: TermId,
    /// `offsets[i]..offsets[i + 1]` are subject `i`'s values.
    offsets: Vec<u64>,
    values: Vec<TermId>,
}

struct Table {
    meta: TableMeta,
    subjects: Vec<TermId>,
    /// Sorted by predicate.
    columns: Vec<Column>,
    /// Bit `id - first` is set for every subject (`first` = the smallest), when the subjects' ID span is dense
    /// enough (at most 64 bits per subject); otherwise `member` binary-searches `subjects`.
    first: u64,
    bits: Option<Vec<u64>>,
}

impl Table {
    fn column(&self, predicate: TermId) -> Option<&Column> {
        self.columns.binary_search_by_key(&predicate, |c| c.predicate).ok().map(|i| &self.columns[i])
    }
}

/// The wide tables of one immutable view.
pub struct WideTables {
    tables: Vec<Table>,
    /// predicate -> tables having it as a column, ascending.
    by_predicate: HashMap<TermId, Vec<TableId>>,
    triples: u64,
}

/// One subject's predicate-object pairs from an SPO scan, in (p, o) order.
struct SubjectRows { subject: TermId, rows: Vec<(TermId, TermId)> }

/// Calls `f` once per subject with its rows, in SPO order.
fn for_each_subject(snap: &dyn Snapshot, mut f: impl FnMut(&SubjectRows)) -> Result<u64, StorageError> {
    let mut cur = SubjectRows { subject: TermId(0), rows: Vec::new() };
    let mut n = 0u64;
    for batch in snap.scan(Perm::Spo, &[]) {
        let b = batch?;
        for i in 0..b.len() {
            let (s, p, o) = (b.cols[0][i], b.cols[1][i], b.cols[2][i]);
            if s != cur.subject && !cur.rows.is_empty() { f(&cur); cur.rows.clear() }
            cur.subject = s;
            cur.rows.push((p, o));
            n += 1;
        }
    }
    if !cur.rows.is_empty() { f(&cur) }
    Ok(n)
}

fn type_key(rows: &[(TermId, TermId)], rdf_type: TermId) -> Vec<TermId> {
    // rows are sorted by (p, o), so the type objects are one ascending run
    rows.iter().filter(|(p, _)| *p == rdf_type).map(|(_, o)| *o).collect()
}

#[derive(Default)]
struct Group { subjects: u64, triples: u64, per_predicate: BTreeMap<TermId, u64> }

impl WideTables {
    /// Builds the tables of `snap`. `rdf_type` is the view's ID for `rdf:type` (`None`: no tables).
    pub fn build(snap: &dyn Snapshot, rdf_type: Option<TermId>, options: BuildOptions) -> Result<WideTables, StorageError> {
        let Some(rdf_type) = rdf_type else { return Ok(WideTables { tables: Vec::new(), by_predicate: HashMap::new(), triples: 0 }) };
        // Pass 1: group statistics.
        let mut groups: HashMap<Vec<TermId>, Group> = HashMap::new();
        let total = for_each_subject(snap, |s| {
            let key = type_key(&s.rows, rdf_type);
            if key.is_empty() { return }
            let g = groups.entry(key).or_default();
            g.subjects += 1;
            g.triples += s.rows.len() as u64;
            let mut last = None;
            for &(p, _) in &s.rows {
                if last != Some(p) { *g.per_predicate.entry(p).or_default() += 1; last = Some(p) }
            }
        })?;
        // Choose the tables.
        let mut chosen: Vec<(Vec<TermId>, Vec<TermId>)> = groups.iter()
            .filter(|(_, g)| {
                let cells = g.subjects as f64 * g.per_predicate.len() as f64;
                let filled: u64 = g.per_predicate.values().sum();
                cells > 0.0 && filled as f64 / cells >= options.min_fill && g.triples as f64 >= options.min_share * total as f64
            })
            .map(|(k, g)| (k.clone(), g.per_predicate.keys().copied().collect()))
            .collect();
        chosen.sort();
        let index: HashMap<Vec<TermId>, usize> = chosen.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect();
        let mut tables: Vec<Table> = chosen.iter().map(|(_, preds)| Table {
            meta: TableMeta { subjects: 0, columns: Vec::new(), subject_bytes: 0 },
            subjects: Vec::new(),
            columns: preds.iter().map(|&p| Column { predicate: p, offsets: vec![0], values: Vec::new() }).collect(),
            first: 0,
            bits: None,
        }).collect();
        // Pass 2: fill them.
        if !tables.is_empty() {
            for_each_subject(snap, |s| {
                let key = type_key(&s.rows, rdf_type);
                let Some(&t) = index.get(&key) else { return };
                let table = &mut tables[t];
                table.subjects.push(s.subject);
                let mut j = 0;
                for col in &mut table.columns {
                    while j < s.rows.len() && s.rows[j].0 < col.predicate { j += 1 }
                    while j < s.rows.len() && s.rows[j].0 == col.predicate { col.values.push(s.rows[j].1); j += 1 }
                    col.offsets.push(col.values.len() as u64);
                }
            })?;
        }
        let mut by_predicate: HashMap<TermId, Vec<TableId>> = HashMap::new();
        for (t, table) in tables.iter_mut().enumerate() {
            let (first, last) = (table.subjects.first().map_or(0, |s| s.0), table.subjects.last().map_or(0, |s| s.0));
            table.first = first;
            // IDs carry a kind tag in their top bits, so a table mixing IRIs and blank nodes spans ~2^60: only
            // allocate the bitset when it costs at most one word per subject (64 bits), else binary-search.
            let words = (last - first) / 64 + 1;
            if words <= (table.subjects.len() as u64).max(1024) {
                let mut bits = vec![0u64; words as usize];
                for s in &table.subjects { let k = s.0 - first; bits[(k / 64) as usize] |= 1 << (k % 64) }
                table.bits = Some(bits);
            }
            table.meta = TableMeta {
                subjects: table.subjects.len() as u64,
                columns: table.columns.iter().map(|c| ColumnMeta {
                    predicate: c.predicate,
                    non_null: c.offsets.windows(2).filter(|w| w[1] > w[0]).count() as u64,
                    values: c.values.len() as u64,
                    bytes: (c.offsets.len() * 8 + c.values.len() * 8) as u64,
                }).collect(),
                subject_bytes: (table.subjects.len() * 8 + table.bits.as_ref().map_or(0, Vec::len) * 8) as u64,
            };
            for c in &table.columns { by_predicate.entry(c.predicate).or_default().push(t as TableId) }
        }
        Ok(WideTables { tables, by_predicate, triples: total })
    }

    /// Number of tables.
    pub fn len(&self) -> usize { self.tables.len() }
    pub fn is_empty(&self) -> bool { self.tables.is_empty() }
    /// Triples of the view the tables were built from.
    pub fn view_triples(&self) -> u64 { self.triples }
    /// Triples stored in tables (each also stays in the indexes).
    pub fn table_triples(&self) -> u64 { self.tables.iter().map(|t| t.meta.columns.iter().map(|c| c.values).sum::<u64>()).sum() }
    /// Bytes of all tables.
    pub fn bytes(&self) -> u64 { self.tables.iter().map(|t| t.meta.subject_bytes + t.meta.columns.iter().map(|c| c.bytes).sum::<u64>()).sum() }
}

impl TableCatalog for WideTables {
    fn tables_with(&self, predicates: &[TermId]) -> Vec<TableId> {
        let Some((&first, rest)) = predicates.split_first() else { return Vec::new() };
        let Some(candidates) = self.by_predicate.get(&first) else { return Vec::new() };
        candidates.iter().copied().filter(|&t| rest.iter().all(|&p| self.tables[t as usize].column(p).is_some())).collect()
    }

    fn meta(&self, table: TableId) -> &TableMeta { &self.tables[table as usize].meta }

    fn member(&self, table: TableId, subject: TermId) -> bool {
        let t = &self.tables[table as usize];
        match &t.bits {
            Some(bits) => {
                let Some(k) = subject.0.checked_sub(t.first) else { return false };
                bits.get((k / 64) as usize).is_some_and(|w| w >> (k % 64) & 1 == 1)
            }
            None => t.subjects.binary_search(&subject).is_ok(),
        }
    }

    fn scan<'a>(&'a self, table: TableId, predicates: &[TermId], ranges: &[Option<(TermId, TermId)>]) -> Box<dyn TableScan + 'a> {
        let t = &self.tables[table as usize];
        let columns = predicates.iter().map(|&p| t.column(p).expect("scan of a predicate that is not a column")).collect();
        let ranges = (0..predicates.len()).map(|i| ranges.get(i).copied().flatten()).collect();
        Box::new(Scan { table: t, columns, ranges, next: 0, counters: ScanCounters::default(), picked: vec![Vec::new(); predicates.len()], pending: None })
    }
}

struct Scan<'a> {
    table: &'a Table,
    columns: Vec<&'a Column>,
    ranges: Vec<Option<(TermId, TermId)>>,
    next: usize,
    counters: ScanCounters,
    /// This subject's values per requested column, after the ranges.
    picked: Vec<Vec<TermId>>,
    /// A subject whose combinations did not fit the last batch: its index and the next combination (an odometer
    /// over `picked`, the last column varying fastest). A subject's rows may therefore span batches, so a huge
    /// product never makes one huge batch.
    pending: Option<(usize, Vec<usize>)>,
}

impl Scan<'_> {
    /// Loads subject `i`'s values into `picked`; whether it has at least one combination.
    fn load(&mut self, i: usize) -> bool {
        self.counters.subjects_scanned += 1;
        self.counters.bytes_read += 8;
        for (c, col) in self.columns.iter().enumerate() {
            let vals = &col.values[col.offsets[i] as usize..col.offsets[i + 1] as usize];
            self.counters.values_read += vals.len() as u64;
            self.counters.bytes_read += 8 + 8 * vals.len() as u64;
            let picked = &mut self.picked[c];
            picked.clear();
            match self.ranges[c] {
                Some((lo, hi)) => picked.extend(vals.iter().copied().filter(|v| *v >= lo && *v < hi)),
                None => picked.extend_from_slice(vals),
            }
            if picked.is_empty() { return false }
        }
        true
    }
}

impl TableScan for Scan<'_> {
    fn next_batch(&mut self) -> Option<Result<TableBatch, StorageError>> {
        let n = self.table.subjects.len();
        if self.pending.is_none() && self.next >= n { return None }
        let mut out = TableBatch { subjects: Vec::new(), values: vec![Vec::new(); self.columns.len()] };
        while out.len() < TABLE_BATCH_ROWS {
            let (i, mut idx) = match self.pending.take() {
                Some(p) => p,
                None => {
                    if self.next >= n { break }
                    let i = self.next;
                    self.next += 1;
                    if !self.load(i) { continue }
                    (i, vec![0usize; self.columns.len()])
                }
            };
            let subject = self.table.subjects[i];
            loop {
                out.subjects.push(subject);
                for (c, &k) in idx.iter().enumerate() { out.values[c].push(self.picked[c][k]) }
                self.counters.rows_emitted += 1;
                // advance the odometer; done when it wraps
                let mut c = idx.len();
                let wrapped = loop {
                    if c == 0 { break true }
                    c -= 1;
                    idx[c] += 1;
                    if idx[c] < self.picked[c].len() { break false }
                    idx[c] = 0;
                };
                if wrapped { break }
                if out.len() >= TABLE_BATCH_ROWS { self.pending = Some((i, idx)); break }
            }
        }
        if out.is_empty() { return None }
        Some(Ok(out))
    }

    fn counters(&self) -> ScanCounters { self.counters }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySnapshot;
    use crate::Dictionary;
    use triplelight_rdf::{Literal, NamedNode, Term, Triple};

    fn iri(s: &str) -> NamedNode { NamedNode::new(format!("urn:{s}")).unwrap() }
    const TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

    fn data() -> MemorySnapshot {
        let ty = NamedNode::new(TYPE).unwrap();
        let mut t = Vec::new();
        // 40 reviews: product + date, review 0 has two products (multi-valued), review 1 has no date
        for i in 0..40 {
            let r = iri(&format!("review{i}"));
            t.push(Triple::new(r.clone(), ty.clone(), iri("Review")));
            t.push(Triple::new(r.clone(), iri("reviewFor"), iri(&format!("product{}", i % 7))));
            if i == 0 { t.push(Triple::new(r.clone(), iri("reviewFor"), iri("product99"))) }
            if i != 1 { t.push(Triple::new(r.clone(), iri("date"), Literal::new_simple_literal(format!("2008-01-{:02}", i % 28 + 1)))) }
        }
        // 10 offers in another table, and 3 untyped subjects with the same predicates
        for i in 0..10 {
            let o = iri(&format!("offer{i}"));
            t.push(Triple::new(o.clone(), ty.clone(), iri("Offer")));
            t.push(Triple::new(o.clone(), iri("reviewFor"), iri(&format!("product{}", i % 3))));
            t.push(Triple::new(o.clone(), iri("date"), Literal::new_simple_literal("2008-02-01")));
        }
        for i in 0..3 {
            let u = iri(&format!("loose{i}"));
            t.push(Triple::new(u.clone(), iri("reviewFor"), iri("product1")));
            t.push(Triple::new(u.clone(), iri("date"), Literal::new_simple_literal("2008-03-01")));
        }
        MemorySnapshot::from_triples(t)
    }

    /// The star ?s <reviewFor> ?p . ?s <date> ?d over the indexes, as a sorted bag of (s, p, d).
    fn star_by_indexes(snap: &MemorySnapshot, p1: TermId, p2: TermId, keep: impl Fn(TermId) -> bool) -> Vec<[TermId; 3]> {
        let mut out = Vec::new();
        for b in snap.scan(Perm::Pso, &[p1]) {
            let b = b.unwrap();
            for i in 0..b.len() {
                let (s, o1) = (b.cols[1][i], b.cols[2][i]);
                if !keep(s) { continue }
                for b2 in snap.scan(Perm::Spo, &[s, p2]) {
                    let b2 = b2.unwrap();
                    for &o2 in &b2.cols[2] { out.push([s, o1, o2]) }
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn union_of_tables_plus_residual_equals_the_bgp_as_bags() {
        let snap = data();
        let ty = snap.lookup(&Term::from(NamedNode::new(TYPE).unwrap())).unwrap();
        let wide = WideTables::build(&snap, ty, BuildOptions { min_fill: 0.6, min_share: 0.0 }).unwrap();
        assert_eq!(wide.len(), 2, "Review and Offer");
        let p1 = snap.lookup(&iri("reviewFor").into()).unwrap().unwrap();
        let p2 = snap.lookup(&iri("date").into()).unwrap().unwrap();
        let tables = wide.tables_with(&[p1, p2]);
        assert_eq!(tables.len(), 2);
        let mut got = Vec::new();
        for &t in &tables {
            let mut scan = wide.scan(t, &[p1, p2], &[None, None]);
            while let Some(b) = scan.next_batch() {
                let b = b.unwrap();
                for r in 0..b.len() { got.push([b.subjects[r], b.values[0][r], b.values[1][r]]) }
            }
            let c = scan.counters();
            assert!(c.rows_emitted > 0 && c.subjects_scanned == wide.meta(t).subjects);
        }
        // residual: subjects in no selected table
        got.extend(star_by_indexes(&snap, p1, p2, |s| !tables.iter().any(|&t| wide.member(t, s))));
        got.sort();
        assert_eq!(got, star_by_indexes(&snap, p1, p2, |_| true), "tables + residual must equal the BGP as a bag");
        // review0 has two products, so two rows; review1 has no date, so none
        let r0 = snap.lookup(&iri("review0").into()).unwrap().unwrap();
        let r1 = snap.lookup(&iri("review1").into()).unwrap().unwrap();
        assert_eq!(got.iter().filter(|r| r[0] == r0).count(), 2);
        assert_eq!(got.iter().filter(|r| r[0] == r1).count(), 0);
    }

    #[test]
    fn ranges_filter_values_before_expansion_and_predicates_may_repeat() {
        let snap = data();
        let ty = snap.lookup(&Term::from(NamedNode::new(TYPE).unwrap())).unwrap();
        let wide = WideTables::build(&snap, ty, BuildOptions { min_fill: 0.6, min_share: 0.0 }).unwrap();
        let p1 = snap.lookup(&iri("reviewFor").into()).unwrap().unwrap();
        let t = wide.tables_with(&[p1])[0];
        // ?s <reviewFor> ?a . ?s <reviewFor> ?b on review0: 2 x 2 = 4 rows
        let mut scan = wide.scan(t, &[p1, p1], &[None, None]);
        let mut rows = 0;
        let r0 = snap.lookup(&iri("review0").into()).unwrap().unwrap();
        while let Some(b) = scan.next_batch() { rows += b.unwrap().subjects.iter().filter(|s| **s == r0).count() }
        assert_eq!(rows, 4);
        // an empty range keeps nothing
        let mut scan = wide.scan(t, &[p1], &[Some((TermId(0), TermId(0)))]);
        let mut n = 0;
        while let Some(b) = scan.next_batch() { n += b.unwrap().len() }
        assert_eq!(n, 0);
    }

    #[test]
    fn a_large_product_spans_batches_without_losing_rows() {
        let ty = NamedNode::new(TYPE).unwrap();
        let mut t = vec![Triple::new(iri("x"), ty.clone(), iri("T"))];
        for i in 0..40 { t.push(Triple::new(iri("x"), iri("a"), iri(&format!("a{i}")))) }
        for i in 0..40 { t.push(Triple::new(iri("x"), iri("b"), iri(&format!("b{i}")))) }
        let snap = MemorySnapshot::from_triples(t);
        let wide = WithTables::build(snap, BuildOptions { min_fill: 0.0, min_share: 0.0 }).unwrap();
        let (a, b) = (wide.lookup(&iri("a").into()).unwrap().unwrap(), wide.lookup(&iri("b").into()).unwrap().unwrap());
        let tbl = wide.tables.tables_with(&[a, b])[0];
        let mut scan = wide.tables.scan(tbl, &[a, b], &[None, None]);
        let (mut rows, mut batches, mut seen) = (0, 0, std::collections::HashSet::new());
        while let Some(batch) = scan.next_batch() {
            let batch = batch.unwrap();
            assert!(batch.len() <= TABLE_BATCH_ROWS);
            for r in 0..batch.len() { assert!(seen.insert((batch.values[0][r], batch.values[1][r]))) }
            rows += batch.len(); batches += 1;
        }
        assert_eq!((rows, batches), (1600, 2), "40 x 40 combinations, each once, over two batches");
        assert!(wide.wide_tables().is_some());
    }

    #[test]
    fn sparse_or_small_groups_and_untyped_subjects_get_no_table() {
        let snap = data();
        let ty = snap.lookup(&Term::from(NamedNode::new(TYPE).unwrap())).unwrap();
        let wide = WideTables::build(&snap, ty, BuildOptions { min_fill: 0.6, min_share: 0.5 }).unwrap();
        assert_eq!(wide.len(), 1, "only Review holds half of the triples");
        let loose = snap.lookup(&iri("loose0").into()).unwrap().unwrap();
        assert!(!(0..wide.len() as TableId).any(|t| wide.member(t, loose)));
        assert!(WideTables::build(&snap, None, BuildOptions::default()).unwrap().is_empty());
    }
}

/// A view with wide tables attached: every read goes to `inner`, and `wide_tables` returns `tables`. The tables must
/// have been built from `inner` itself (same contents), which is the caller's promise; this is how embedded use and
/// the A/B harness get tables before the disk format carries them.
pub struct WithTables<S> { pub inner: S, pub tables: WideTables }

impl<S: Snapshot> WithTables<S> {
    /// Builds the tables of `inner` (two SPO scans) and attaches them.
    pub fn build(inner: S, options: BuildOptions) -> Result<Self, StorageError> {
        let rdf_type = inner.lookup(&triplelight_rdf::Term::from(
            triplelight_rdf::NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type").expect("valid IRI")))?;
        let tables = WideTables::build(&inner, rdf_type, options)?;
        Ok(WithTables { inner, tables })
    }
}

impl<S: Snapshot> crate::Dictionary for WithTables<S> {
    fn lookup(&self, term: &triplelight_rdf::Term) -> Result<Option<TermId>, StorageError> { self.inner.lookup(term) }
    fn resolve(&self, id: TermId) -> Result<triplelight_rdf::Term, StorageError> { self.inner.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.inner.with_terms(ids, f) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { self.inner.literal_text_bound(text, strict) }
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> { self.inner.triple_term_parts(id) }
}

impl<S: Snapshot> Snapshot for WithTables<S> {
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<triplelight_core::model::TripleBatch, StorageError>> + 'a> { self.inner.scan(perm, prefix) }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<triplelight_core::model::TripleBatch, StorageError>> + '_> { self.inner.scan_for_stats() }
    fn first_keys(&self, perm: Perm) -> Result<Vec<TermId>, StorageError> { self.inner.first_keys(perm) }
    fn scan_from<'a>(&'a self, perm: Perm, prefix: &[TermId], from: TermId) -> Box<dyn Iterator<Item = Result<triplelight_core::model::TripleBatch, StorageError>> + 'a> { self.inner.scan_from(perm, prefix, from) }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { self.inner.count(perm, prefix) }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> { self.inner.seek(perm, prefix, target) }
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> { self.inner.matching_rows(perm, prefix) }
    fn matching_tail(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> { self.inner.matching_tail(perm, prefix) }
    fn has_rdf12_terms(&self) -> bool { self.inner.has_rdf12_terms() }
    fn text_candidates(&self, tokens: &[&str], limit: usize) -> Option<triplelight_core::storage::TextCandidates> { self.inner.text_candidates(tokens, limit) }
    fn object_memo(&self) -> Option<&triplelight_core::storage::ObjectMemo> { self.inner.object_memo() }
    fn wide_tables(&self) -> Option<&dyn TableCatalog> { Some(&self.tables) }
}
