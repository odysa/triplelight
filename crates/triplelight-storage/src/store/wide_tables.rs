//! Wide tables built from a view's SPO order (#140): an in-memory `TableCatalog`.
//!
//! Subjects are grouped by their set of `rdf:type` objects. A group becomes a table when its columns (the union of
//! its subjects' predicates) are filled well enough and it holds a large enough share of the triples; untyped
//! subjects and small or sparse groups stay triples-only. Each table keeps its subjects ascending, one CSR column
//! per predicate (offsets into a value array, values ascending per subject) and a bitset for `member`. Subjects,
//! offsets and values are frame-of-reference bit-packed (`PackedIds`); a column whose every subject has exactly
//! one value stores no offsets.
//! Built by two SPO scans; the view's indexes are untouched, so tables are only an extra access path.
use std::collections::{BTreeMap, HashMap};

use triplelight_core::model::{Perm, TermId};
use triplelight_core::wide::{ColumnMeta, ScanCounters, TableBatch, TableCatalog, TableId, TableMeta, TableScan, TABLE_BATCH_ROWS};

use super::packed_ids::{PackedIds, BLOCK};

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
    /// `offsets[i]..offsets[i + 1]` are subject `i`'s values; `None` when the column is dense (`values[i]` is
    /// subject `i`'s only value, for every subject).
    offsets: Option<PackedIds>,
    values: PackedIds,
}

impl Column {
    fn dense(&self) -> bool { self.offsets.is_none() }
    /// Subject `i`'s values as an index range into `values`.
    #[inline]
    fn span(&self, i: usize) -> (usize, usize) {
        match &self.offsets { None => (i, i + 1), Some(o) => (o.get(i) as usize, o.get(i + 1) as usize) }
    }
    #[inline]
    fn value(&self, k: usize) -> TermId { TermId(self.values.get(k)) }
    fn bytes(&self) -> u64 { self.values.bytes() + self.offsets.as_ref().map_or(0, PackedIds::bytes) }
}

/// A column while the tables fill.
struct Filling { offsets: Vec<u64>, values: Vec<u64> }

struct Table {
    meta: TableMeta,
    subjects: PackedIds,
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
    /// Predicates every one of whose subjects is in a table.
    covered: std::collections::HashSet<TermId>,
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
        let Some(rdf_type) = rdf_type else { return Ok(WideTables { tables: Vec::new(), by_predicate: HashMap::new(), triples: 0, covered: Default::default() }) };
        // Pass 1: group statistics.
        let mut groups: HashMap<Vec<TermId>, Group> = HashMap::new();
        // subjects per predicate over the whole view, for the exact coverage test
        let mut subjects_with: HashMap<TermId, u64> = HashMap::new();
        let total = for_each_subject(snap, |s| {
            let mut last = None;
            for &(p, _) in &s.rows { if last != Some(p) { *subjects_with.entry(p).or_default() += 1; last = Some(p) } }
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
        // Pass 2: fill them (plain arrays), then pack.
        let mut filling: Vec<(Vec<u64>, Vec<Filling>)> = chosen.iter()
            .map(|(_, preds)| (Vec::new(), preds.iter().map(|_| Filling { offsets: vec![0], values: Vec::new() }).collect()))
            .collect();
        if !filling.is_empty() {
            for_each_subject(snap, |s| {
                let key = type_key(&s.rows, rdf_type);
                let Some(&t) = index.get(&key) else { return };
                let (subjects, cols) = &mut filling[t];
                subjects.push(s.subject.0);
                let mut j = 0;
                for (col, &p) in cols.iter_mut().zip(&chosen[t].1) {
                    while j < s.rows.len() && s.rows[j].0 < p { j += 1 }
                    while j < s.rows.len() && s.rows[j].0 == p { col.values.push(s.rows[j].1.0); j += 1 }
                    col.offsets.push(col.values.len() as u64);
                }
            })?;
        }
        let mut tables: Vec<Table> = Vec::with_capacity(filling.len());
        for ((subjects, cols), (_, preds)) in filling.into_iter().zip(&chosen) {
            let n = subjects.len();
            let (first, last) = (subjects.first().copied().unwrap_or(0), subjects.last().copied().unwrap_or(0));
            // IDs carry a kind tag in their top bits, so a table mixing IRIs and blank nodes spans ~2^60: only
            // allocate the bitset when it costs at most one word per subject (64 bits), else binary-search.
            let words = (last - first) / 64 + 1;
            let bits = (words <= (n as u64).max(1024)).then(|| {
                let mut bits = vec![0u64; words as usize];
                for s in &subjects { let k = s - first; bits[(k / 64) as usize] |= 1 << (k % 64) }
                bits
            });
            let mut metas = Vec::with_capacity(cols.len());
            let columns = cols.into_iter().zip(preds).map(|(c, &p)| {
                let non_null = c.offsets.windows(2).filter(|w| w[1] > w[0]).count() as u64;
                // every subject exactly one value: no offsets
                let dense = c.values.len() == n && c.offsets.windows(2).all(|w| w[1] - w[0] == 1);
                let col = Column { predicate: p, offsets: (!dense).then(|| PackedIds::from_slice(&c.offsets)), values: PackedIds::from_slice(&c.values) };
                metas.push(ColumnMeta { predicate: p, non_null, values: c.values.len() as u64, bytes: col.bytes() });
                col
            }).collect();
            let subjects = PackedIds::from_slice(&subjects);
            let meta = TableMeta { subjects: n as u64, columns: metas, subject_bytes: subjects.bytes() + bits.as_ref().map_or(0, Vec::len) as u64 * 8 };
            tables.push(Table { meta, subjects, columns, first, bits });
        }
        let mut by_predicate: HashMap<TermId, Vec<TableId>> = HashMap::new();
        for (t, table) in tables.iter().enumerate() {
            for c in &table.columns { by_predicate.entry(c.predicate).or_default().push(t as TableId) }
        }
        let mut in_tables: HashMap<TermId, u64> = HashMap::new();
        for t in &tables { for c in &t.meta.columns { *in_tables.entry(c.predicate).or_default() += c.non_null } }
        let covered = in_tables.into_iter().filter(|(p, n)| subjects_with.get(p) == Some(n)).map(|(p, _)| p).collect();
        Ok(WideTables { tables, by_predicate, triples: total, covered })
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

    fn covers_all_subjects(&self, predicate: TermId) -> bool { self.covered.contains(&predicate) }

    fn member(&self, table: TableId, subject: TermId) -> bool {
        let t = &self.tables[table as usize];
        match &t.bits {
            Some(bits) => {
                let Some(k) = subject.0.checked_sub(t.first) else { return false };
                bits.get((k / 64) as usize).is_some_and(|w| w >> (k % 64) & 1 == 1)
            }
            None => {
                let i = t.subjects.partition_point(subject.0);
                i < t.subjects.len() && t.subjects.get(i) == subject.0
            }
        }
    }

    fn scan<'a>(&'a self, table: TableId, predicates: &[TermId], ranges: &[Option<(TermId, TermId)>]) -> Box<dyn TableScan + 'a> {
        let t = &self.tables[table as usize];
        let columns = predicates.iter().map(|&p| t.column(p).expect("scan of a predicate that is not a column")).collect();
        let ranges = (0..predicates.len()).map(|i| ranges.get(i).copied().flatten()).collect();
        Box::new(Scan { table: t, columns, ranges, next: 0, counters: ScanCounters::default(), picked: vec![Vec::new(); predicates.len()], pending: None, scratch: Vec::with_capacity(BLOCK) })
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
    /// A decoded block, reused across batches.
    scratch: Vec<u64>,
}

impl Scan<'_> {
    /// Loads subject `i`'s values into `picked`; whether it has at least one combination.
    fn load(&mut self, i: usize) -> bool {
        self.counters.subjects_scanned += 1;
        self.counters.bytes_read += 8;
        for (c, col) in self.columns.iter().enumerate() {
            let (a, b) = col.span(i);
            self.counters.values_read += (b - a) as u64;
            self.counters.bytes_read += 8 + 8 * (b - a) as u64;
            let picked = &mut self.picked[c];
            picked.clear();
            let vals = (a..b).map(|k| col.value(k));
            match self.ranges[c] {
                Some((lo, hi)) => picked.extend(vals.filter(|v| *v >= lo && *v < hi)),
                None => picked.extend(vals),
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
        let k = self.columns.len();
        if self.pending.is_none() && self.columns.iter().all(|c| c.dense()) { return self.next_dense_batch() }
        let mut out = TableBatch { subjects: Vec::with_capacity(TABLE_BATCH_ROWS), values: (0..k).map(|_| Vec::with_capacity(TABLE_BATCH_ROWS)).collect() };
        'subjects: while out.len() < TABLE_BATCH_ROWS {
            let (i, mut idx) = match self.pending.take() {
                Some(p) => p,
                None => {
                    if self.next >= n { break }
                    let i = self.next;
                    self.next += 1;
                    // Fast path: every requested column has exactly one value for this subject (most cells in a
                    // well-filled table), so the subject yields at most one row and nothing is copied twice.
                    if self.columns.iter().all(|col| { let (a, b) = col.span(i); b - a == 1 }) {
                        self.counters.subjects_scanned += 1;
                        self.counters.values_read += k as u64;
                        self.counters.bytes_read += 8 + 16 * k as u64;
                        for (c, col) in self.columns.iter().enumerate() {
                            let v = col.value(col.span(i).0);
                            if let Some((lo, hi)) = self.ranges[c] && !(v >= lo && v < hi) {
                                // undo the values pushed for this subject's earlier columns
                                for done in &mut out.values[..c] { done.pop(); }
                                continue 'subjects;
                            }
                            out.values[c].push(v);
                        }
                        out.subjects.push(TermId(self.table.subjects.get(i)));
                        self.counters.rows_emitted += 1;
                        continue;
                    }
                    if !self.load(i) { continue }
                    (i, vec![0usize; k])
                }
            };
            let subject = TermId(self.table.subjects.get(i));
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

impl Scan<'_> {
    /// Every requested column is dense: subject `i`'s row is `values[c][i]`, so a batch is a block of subjects
    /// filtered column by column (ranged columns first, into a selection vector) and then gathered. Same rows,
    /// order and counters as the per-subject path.
    fn next_dense_batch(&mut self) -> Option<Result<TableBatch, StorageError>> {
        let n = self.table.subjects.len();
        let k = self.columns.len();
        let mut sel: Vec<u32> = Vec::with_capacity(TABLE_BATCH_ROWS);
        while self.next < n {
            let (lo, hi) = (self.next, (self.next + TABLE_BATCH_ROWS).min(n));
            self.next = hi;
            self.counters.subjects_scanned += (hi - lo) as u64;
            self.counters.bytes_read += (hi - lo) as u64 * (8 + 16 * k as u64);
            sel.clear();
            sel.extend(0..(hi - lo) as u32);
            // batches start at multiples of TABLE_BATCH_ROWS, which is the packing block, so batch = block
            let blk = lo / BLOCK;
            for (c, col) in self.columns.iter().enumerate() {
                let Some((a, b)) = self.ranges[c] else { continue };
                col.values.select_in_range(blk, a.0, b.0, &mut self.scratch, &mut sel);
                if sel.is_empty() { break }
            }
            // gather from decoded blocks: one sequential decode per column beats a random `get` per row
            self.counters.values_read += (hi - lo) as u64 * k as u64;
            if sel.is_empty() { continue }
            let scratch = &mut self.scratch;
            let mut gather = |p: &PackedIds| -> Vec<TermId> {
                p.decode_block(blk, scratch);
                sel.iter().map(|&r| TermId(scratch[r as usize])).collect()
            };
            let subjects = gather(&self.table.subjects);
            let values = self.columns.iter().map(|col| gather(&col.values)).collect();
            let out = TableBatch { subjects, values };
            self.counters.rows_emitted += out.len() as u64;
            return Some(Ok(out));
        }
        None
    }
}

/// A view with wide tables attached: every read goes to `inner`, and `wide_tables` returns `tables`. The tables must
/// have been built from `inner` itself (same contents), which is the caller's promise; this is how embedded use and
/// the A/B harness get tables before the disk format carries them.
pub struct WithTables<S> { inner: S, tables: WideTables }

impl<S: Snapshot> WithTables<S> {
    /// Catalog size for diagnostics, without exposing the mutable access-path representation.
    pub fn table_count(&self) -> usize { self.tables.len() }
    pub fn table_bytes(&self) -> u64 { self.tables.bytes() }
    pub fn table_triples(&self) -> u64 { self.tables.table_triples() }
    pub fn view_triples(&self) -> u64 { self.tables.view_triples() }

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
        // the untyped `loose` subjects have reviewFor and date but no table, so neither predicate is covered
        let (rf, dt) = (snap.lookup(&iri("reviewFor").into()).unwrap().unwrap(), snap.lookup(&iri("date").into()).unwrap().unwrap());
        assert!(!wide.covers_all_subjects(rf) && !wide.covers_all_subjects(dt));
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
    fn dense_columns_drop_offsets_and_scan_by_blocks_with_the_same_rows_and_counters() {
        // 2500 single-valued reviews (three blocks) plus one with two products, in a second type so that the
        // first table is dense and the second is not
        let ty = NamedNode::new(TYPE).unwrap();
        let mut t = Vec::new();
        for i in 0..2500 {
            let r = iri(&format!("r{i:05}"));
            t.push(Triple::new(r.clone(), ty.clone(), iri("Dense")));
            t.push(Triple::new(r.clone(), iri("reviewFor"), iri(&format!("product{}", i % 13))));
            t.push(Triple::new(r.clone(), iri("date"), Literal::new_simple_literal(format!("2008-{:02}-01", i % 12 + 1))));
        }
        t.push(Triple::new(iri("m"), ty.clone(), iri("Multi")));
        t.push(Triple::new(iri("m"), iri("reviewFor"), iri("product1")));
        t.push(Triple::new(iri("m"), iri("reviewFor"), iri("product2")));
        t.push(Triple::new(iri("m"), iri("date"), Literal::new_simple_literal("2008-05-01")));
        let snap = MemorySnapshot::from_triples(t);
        let tyid = snap.lookup(&Term::from(ty)).unwrap();
        let wide = WideTables::build(&snap, tyid, BuildOptions { min_fill: 0.0, min_share: 0.0 }).unwrap();
        let (p1, p2) = (snap.lookup(&iri("reviewFor").into()).unwrap().unwrap(), snap.lookup(&iri("date").into()).unwrap().unwrap());
        let tables = wide.tables_with(&[p1, p2]);
        assert_eq!(tables.len(), 2);
        let dense: Vec<bool> = tables.iter().map(|&t| wide.tables[t as usize].columns.iter().all(Column::dense)).collect();
        assert_eq!(dense.iter().filter(|d| **d).count(), 1, "one dense table, one with a multi-valued column");
        // a date range by ID: the IDs of the March..June literals
        let mut dates: Vec<TermId> = (3..=6).map(|m| snap.lookup(&Literal::new_simple_literal(format!("2008-{m:02}-01")).into()).unwrap().unwrap()).collect();
        dates.sort();
        let (lo, hi) = (dates[0], TermId(dates[3].0 + 1));
        let keep = |d: TermId| d >= lo && d < hi;
        for (&t, &is_dense) in tables.iter().zip(&dense) {
            let mut got = Vec::new();
            let mut scan = wide.scan(t, &[p1, p2], &[None, Some((lo, hi))]);
            while let Some(b) = scan.next_batch() {
                let b = b.unwrap();
                assert!(b.len() <= TABLE_BATCH_ROWS);
                for r in 0..b.len() { got.push([b.subjects[r], b.values[0][r], b.values[1][r]]) }
            }
            assert!(got.windows(2).all(|w| w[0][0] <= w[1][0]), "subjects ascending");
            let mut want: Vec<[TermId; 3]> = star_by_indexes(&snap, p1, p2, |s| wide.member(t, s)).into_iter().filter(|r| keep(r[2])).collect();
            want.sort();
            let mut sorted = got.clone();
            sorted.sort();
            assert_eq!(sorted, want, "dense={is_dense}");
            let c = scan.counters();
            let n = wide.meta(t).subjects;
            assert_eq!((c.subjects_scanned, c.values_read, c.rows_emitted), (n, if is_dense { 2 * n } else { c.values_read }, got.len() as u64));
            if is_dense { assert!(wide.meta(t).columns.iter().map(|c| c.bytes).sum::<u64>() < 2 * n * 8, "packed, no offsets") }
        }
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
        assert!(wide.tables.covers_all_subjects(a) && wide.tables.covers_all_subjects(b), "x is the only subject with a and b, and it is in a table");
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
