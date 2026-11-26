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
    offsets: Vec<u32>,
    values: Vec<TermId>,
}

struct Table {
    meta: TableMeta,
    subjects: Vec<TermId>,
    /// Sorted by predicate.
    columns: Vec<Column>,
    /// Bit `id - first` is set for every subject; `first` is the smallest subject.
    first: u64,
    bits: Vec<u64>,
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
            bits: Vec::new(),
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
                    col.offsets.push(col.values.len() as u32);
                }
            })?;
        }
        let mut by_predicate: HashMap<TermId, Vec<TableId>> = HashMap::new();
        for (t, table) in tables.iter_mut().enumerate() {
            let (first, last) = (table.subjects.first().map_or(0, |s| s.0), table.subjects.last().map_or(0, |s| s.0));
            table.first = first;
            table.bits = vec![0u64; ((last - first) / 64 + 1) as usize];
            for s in &table.subjects { let k = s.0 - first; table.bits[(k / 64) as usize] |= 1 << (k % 64) }
            table.meta = TableMeta {
                subjects: table.subjects.len() as u64,
                columns: table.columns.iter().map(|c| ColumnMeta {
                    predicate: c.predicate,
                    non_null: c.offsets.windows(2).filter(|w| w[1] > w[0]).count() as u64,
                    values: c.values.len() as u64,
                    bytes: (c.offsets.len() * 4 + c.values.len() * 8) as u64,
                }).collect(),
                subject_bytes: (table.subjects.len() * 8 + table.bits.len() * 8) as u64,
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
        let Some(k) = subject.0.checked_sub(t.first) else { return false };
        t.bits.get((k / 64) as usize).is_some_and(|w| w >> (k % 64) & 1 == 1)
    }

    fn scan<'a>(&'a self, table: TableId, predicates: &[TermId], ranges: &[Option<(TermId, TermId)>]) -> Box<dyn TableScan + 'a> {
        let t = &self.tables[table as usize];
        let columns = predicates.iter().map(|&p| t.column(p).expect("scan of a predicate that is not a column")).collect();
        let ranges = (0..predicates.len()).map(|i| ranges.get(i).copied().flatten()).collect();
        Box::new(Scan { table: t, columns, ranges, next: 0, counters: ScanCounters::default(), picked: vec![Vec::new(); predicates.len()] })
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
}

impl TableScan for Scan<'_> {
    fn next_batch(&mut self) -> Option<Result<TableBatch, StorageError>> {
        let n = self.table.subjects.len();
        if self.next >= n { return None }
        let mut out = TableBatch { subjects: Vec::new(), values: vec![Vec::new(); self.columns.len()] };
        while self.next < n && out.len() < TABLE_BATCH_ROWS {
            let i = self.next;
            self.next += 1;
            self.counters.subjects_scanned += 1;
            self.counters.bytes_read += 8;
            let mut rows = 1usize;
            for (c, col) in self.columns.iter().enumerate() {
                let vals = &col.values[col.offsets[i] as usize..col.offsets[i + 1] as usize];
                self.counters.values_read += vals.len() as u64;
                self.counters.bytes_read += 4 + 8 * vals.len() as u64;
                let picked = &mut self.picked[c];
                picked.clear();
                match self.ranges[c] {
                    Some((lo, hi)) => picked.extend(vals.iter().copied().filter(|v| *v >= lo && *v < hi)),
                    None => picked.extend_from_slice(vals),
                }
                rows = rows.saturating_mul(picked.len());
                if rows == 0 { break }
            }
            if rows == 0 { continue }
            // every combination, the last column varying fastest (ascending values per column)
            let mut idx = vec![0usize; self.columns.len()];
            for _ in 0..rows {
                out.subjects.push(self.table.subjects[i]);
                for (c, &k) in idx.iter().enumerate() { out.values[c].push(self.picked[c][k]) }
                for c in (0..idx.len()).rev() {
                    idx[c] += 1;
                    if idx[c] < self.picked[c].len() { break }
                    idx[c] = 0;
                }
            }
            self.counters.rows_emitted += rows as u64;
        }
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
