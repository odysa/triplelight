//! Read-only disk-backed snapshot with an in-memory dictionary and bounded index caches.
//! Snapshot files must remain immutable while open.
use super::disk_index::{DiskIndex, write_index};
use super::{Dictionary, MemorySnapshot, Snapshot, StorageError};
use crate::model::{Perm, TermId, TripleBatch};
use triplelight_rdf::Term;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

pub(super) const FILES: [&str; 7] = ["dictionary", "spo", "sop", "pso", "pos", "osp", "ops"];
/// Which triple (and quad) orders a base keeps, as bits of their `Perm::ALL` positions (task #73).
pub const ALL_ORDERS: u8 = 0b11_1111;
/// SPO, PSO, POS and OSP: kept by every base. SOP and OPS are served from them when absent (`derived_order`).
pub const REQUIRED_ORDERS: u8 = 0b01_1101;
/// The graph-first quad orders of an `RDFSNAP5`/`RDFSNAP6` base (task #36), in `Perm::ALL` order of their triple part.
pub(super) const QUAD_FILES: [&str; 6] = ["gspo", "gsop", "gpso", "gpos", "gosp", "gops"];
/// The graph catalog of an `RDFSNAP5` base: every named graph's ID and triple count, empty graphs included.
pub(super) const CATALOG_FILE: &str = "graphs";
fn bad(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
pub(super) fn digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = BufReader::new(File::open(path)?);
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize().into())
}
/// Reads and checks the graph catalog: checksum equal to the manifest identity, strictly ascending nonzero IDs,
/// and counts that add up to the quad orders' row count.
fn read_catalog(path: &Path, identity: [u8; 32], quads: u64) -> io::Result<Vec<(TermId, u64)>> {
    let bytes = fs::read(path)?;
    if bytes.len() < 48 || &bytes[..8] != b"RDFGRA01" {
        return Err(bad("bad graph catalog"));
    }
    let (body, sum) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(body).as_slice() != sum || sum != identity {
        return Err(bad("graph catalog checksum mismatch"));
    }
    let n = u64::from_le_bytes(body[8..16].try_into().unwrap());
    if (body.len() as u64 - 16) != n.checked_mul(16).ok_or_else(|| bad("bad graph catalog"))? {
        return Err(bad("bad graph catalog length"));
    }
    let mut out: Vec<(TermId, u64)> = Vec::with_capacity(n as usize);
    let mut total = 0u64;
    for entry in body[16..].as_chunks::<16>().0 {
        let g = TermId(u64::from_le_bytes(entry[..8].try_into().unwrap()));
        let count = u64::from_le_bytes(entry[8..].try_into().unwrap());
        if g.0 == 0 || out.last().is_some_and(|&(prev, _)| prev >= g) {
            return Err(bad("graph catalog IDs not ascending"));
        }
        total = total.checked_add(count).ok_or_else(|| bad("bad graph catalog"))?;
        out.push((g, count));
    }
    if total != quads {
        return Err(bad("graph catalog counts disagree with the quad orders"));
    }
    Ok(out)
}
fn number(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn write_dictionary(source: &MemorySnapshot, path: &Path) -> io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    out.write_all(b"RDFDIC01")?;
    let entries = source.terms();
    out.write_all(&(entries.len() as u64).to_le_bytes())?;
    for (id, term) in entries {
        let text = term.to_string();
        out.write_all(&id.0.to_le_bytes())?;
        out.write_all(&(text.len() as u64).to_le_bytes())?;
        out.write_all(text.as_bytes())?;
    }
    out.flush()?;
    out.get_ref().sync_all()
}
fn read_dictionary(path: &Path) -> io::Result<(HashMap<Term, TermId>, HashMap<TermId, Term>)> {
    let mut input = BufReader::new(File::open(path)?);
    let mut header = [0; 8];
    input.read_exact(&mut header)?;
    if &header != b"RDFDIC01" {
        return Err(bad("bad dictionary magic"));
    }
    let n = number(&mut input)?;
    if n > 100_000_000 {
        return Err(bad("dictionary count too large"));
    }
    let (mut forward, mut reverse) = (HashMap::new(), HashMap::new());
    for _ in 0..n {
        let id = TermId(number(&mut input)?);
        let len = number(&mut input)?;
        if id.0 == 0 || len > 16_777_216 {
            return Err(bad("invalid dictionary entry"));
        }
        let mut bytes = vec![0; len as usize];
        input.read_exact(&mut bytes)?;
        let term: Term = std::str::from_utf8(&bytes)
            .map_err(|_| bad("invalid term UTF-8"))?
            .parse()
            .map_err(|_| bad("invalid term"))?;
        if forward.insert(term.clone(), id).is_some() || reverse.insert(id, term).is_some() {
            return Err(bad("duplicate dictionary entry"));
        }
    }
    let mut extra = [0];
    if input.read(&mut extra)? != 0 {
        return Err(bad("dictionary trailing bytes"));
    }
    Ok((forward, reverse))
}
/// Build a new immutable disk snapshot from a consistent read snapshot using scratch-backed sorting.
pub fn write_disk_snapshot(dir: impl AsRef<Path>, source: &dyn Snapshot) -> io::Result<()> {
    super::external_build::from_snapshot(dir.as_ref(), source)
}
/// Build a base of a whole dataset (task #36): the default graph plus every named graph as `RDFSNAP5`, or a plain
/// `RDFSNAP4` base when there are no named graphs.
pub fn write_disk_dataset(dir: impl AsRef<Path>, source: &dyn triplelight_core::storage::DatasetView) -> io::Result<()> {
    super::external_build::from_dataset(dir.as_ref(), source)
}
/// Build a new immutable disk snapshot from N-Triples using scratch-backed sorting.
pub fn build_disk_snapshot(dir: impl AsRef<Path>, source: Option<&Path>) -> io::Result<()> {
    super::external_build::adaptive(
        dir.as_ref(),
        source,
        std::thread::available_parallelism().map_or(1, |n| n.get()),
    )
}
/// Build from N-Triples with an explicit number of ingestion workers.
pub fn build_disk_snapshot_with_workers(dir: impl AsRef<Path>, source: Option<&Path>, workers: usize) -> io::Result<()> {
    if workers == 0 { return Err(io::Error::new(io::ErrorKind::InvalidInput, "workers must be positive")); }
    super::external_build::adaptive(dir.as_ref(), source, workers)
}
/// The orders the base in `dir` keeps (bits of `Perm::ALL` positions): its RDFSNAP6 order set, or all six.
pub(super) fn kept_orders(dir: &Path) -> io::Result<u8> {
    let mut head = [0u8; 9];
    let mut manifest = File::open(dir.join("MANIFEST"))?;
    manifest.read_exact(&mut head[..8])?;
    if &head[..8] != b"RDFSNAP6" && &head[..8] != b"RDFSNAP7" { return Ok(ALL_ORDERS) }
    manifest.read_exact(&mut head[8..])?;
    Ok(head[8])
}
/// Persistent Store base format backed by immutable checksummed disk indexes.
pub struct DiskBase {
    pub workers: usize,
}
impl crate::store::BaseFormat for DiskBase {
    fn write(&self, dir: &Path, src: &dyn Snapshot) -> io::Result<()> {
        write_disk_snapshot(dir, src)
    }
    fn import(&self, dir: &Path, ntriples: Option<&Path>) -> io::Result<()> {
        super::external_build::adaptive(dir, ntriples, self.workers)
    }
    fn open(&self, dir: &Path) -> io::Result<std::sync::Arc<dyn Snapshot>> {
        Ok(std::sync::Arc::new(DiskSnapshot::open(dir)?))
    }
    fn holds_named_graphs(&self) -> bool { true }
    fn write_dataset(&self, dir: &Path, src: &crate::store::Overlay) -> io::Result<()> {
        write_disk_dataset(dir, src)
    }
    fn rewrite(&self, previous: &Path, dir: &Path, src: &crate::store::Overlay, dataset: bool) -> io::Result<()> {
        // The new base keeps the previous base's orders, whatever this process's build setting.
        let orders = kept_orders(previous)?;
        if dataset {
            super::external_build::from_dataset_keeping(dir, src, orders)
        } else {
            super::external_build::from_snapshot_keeping(dir, src, orders)
        }
    }
    fn open_dataset(&self, dir: &Path) -> io::Result<(std::sync::Arc<dyn Snapshot>, Vec<(TermId, std::sync::Arc<dyn Snapshot>)>)> {
        let snap = std::sync::Arc::new(DiskSnapshot::open(dir)?);
        let named = snap.graph_names().into_iter().map(|g| {
            let view = snap.graph_view(g).expect("a listed graph has a view");
            (g, std::sync::Arc::new(view) as std::sync::Arc<dyn Snapshot>)
        }).collect();
        Ok((snap, named))
    }
}
enum DictionaryBackend {
    Legacy {
        forward: HashMap<Term, TermId>,
        reverse: HashMap<TermId, Term>,
    },
    Lazy {
        by_term: super::lazy_dictionary::LazyIndex,
        by_id: super::lazy_dictionary::LazyIndex,
    },
    /// One file in ID order serving both lookups (manifest `RDFSNAP4`).
    Unified(super::lazy_dictionary::LazyIndex),
}
pub struct DiskSnapshot {
    dictionary: DictionaryBackend,
    /// The six triple orders in `Perm::ALL` order; `None` for an order this base does not keep (SOP and OPS are then
    /// served from the others, see `derived_order`).
    indexes: Vec<Option<DiskIndex>>,
    /// Named graphs (`RDFSNAP5` only): the six graph-first quad orders and the catalog `(graph ID, triples)`,
    /// ascending by ID.
    named: Option<NamedBase>,
    seek_memo: super::derived_order::SeekMemo,
    /// The base holds an RDF 1.2 term (manifest `RDFSNAP7`).
    rdf12: bool,
    /// `RDFSNAP7` only: rows `(triple term, s, p, o)` sorted by term ID (`Dictionary::triple_term_parts`).
    #[cfg_attr(not(feature = "rdf-12"), allow(dead_code))] // only RDF 1.2 builds open RDFSNAP7
    triple_terms: Option<DiskIndex<4>>,
}
/// File name of the triple-term parts table of an `RDFSNAP7` base.
pub(super) const TRIPLE_TERMS_FILE: &str = "triple_terms";
struct NamedBase {
    /// The graph-first quad orders, in `Perm::ALL` order of their triple part; `None` as for `indexes`.
    quads: Vec<Option<DiskIndex<4>>>,
    catalog: Vec<(TermId, u64)>,
}
impl DiskSnapshot {
    /// Writes a new snapshot; the manifest is written last and old paths are never replaced.
    pub fn create(dir: impl AsRef<Path>, source: &MemorySnapshot) -> io::Result<()> {
        let dir = dir.as_ref();
        fs::create_dir(dir)?;
        write_dictionary(source, &dir.join(FILES[0]))?;
        let (term_identity, id_identity) = super::lazy_dictionary::convert_legacy(dir)?;
        let mut identities = vec![term_identity, id_identity];
        for (i, name) in FILES[1..].iter().enumerate() {
            let file = File::create(dir.join(name))?;
            let mut out = BufWriter::new(file);
            write_index(&mut out, source.index_rows(i))?;
            out.flush()?;
            out.get_ref().sync_all()?;
            let index = DiskIndex::<3>::open(dir.join(name))?;
            index.validate()?;
            identities.push(index.identity());
        }
        let mut manifest = BufWriter::new(File::create(dir.join("MANIFEST"))?);
        // Deliberately RDFSNAP3: this path writes a legacy dictionary in memory-ID order, which is not term-text
        // order, so it cannot become the unified layout without a sort. Production builds (external) write RDFSNAP4.
        manifest.write_all(b"RDFSNAP3")?;
        for identity in identities {
            manifest.write_all(&identity)?;
        }
        manifest.flush()?;
        manifest.get_ref().sync_all()?;
        File::open(dir)?.sync_all()?;
        File::open(
            dir.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    }
    /// Checks the dictionary, index lengths and checksummed block directories. Data blocks are checked on cache miss.
    /// Files must remain immutable after opening; previously cached blocks are not re-read automatically.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let mut manifest = BufReader::new(File::open(dir.join("MANIFEST"))?);
        let mut magic = [0; 8];
        manifest.read_exact(&mut magic)?;
        // RDFSNAP2: one legacy dictionary file; RDFSNAP3: dictionary.term + dictionary.id; RDFSNAP4: one unified file.
        let version = match &magic {
            b"RDFSNAP2" => 2,
            b"RDFSNAP3" => 3,
            b"RDFSNAP4" => 4,
            b"RDFSNAP5" => 5,
            b"RDFSNAP6" => 6,
            // RDFSNAP7 (task #101): RDFSNAP6's layout, for a base holding RDF 1.2 terms.
            b"RDFSNAP7" if cfg!(feature = "rdf-12") => 7,
            b"RDFSNAP7" => return Err(io::Error::new(io::ErrorKind::Unsupported, "this store holds RDF 1.2 terms; open it with a build that has the rdf-12 feature")),
            _ => return Err(bad("bad snapshot manifest or version")),
        };
        // RDFSNAP6 (task #73): a byte of kept orders (bit i = `Perm::ALL[i]`, at least `REQUIRED_ORDERS`), a byte that
        // is 1 when the base holds named graphs (else 0), then the RDFSNAP4/5 identities of only the kept orders.
        let (orders, named_graphs) = if version >= 6 {
            let mut flags = [0u8; 2];
            manifest.read_exact(&mut flags)?;
            if flags[0] & !ALL_ORDERS != 0 || flags[0] & REQUIRED_ORDERS != REQUIRED_ORDERS || flags[1] > 1 {
                return Err(bad("bad RDFSNAP6 order set"));
            }
            (flags[0], flags[1] == 1)
        } else {
            (ALL_ORDERS, version == 5)
        };
        let kept = |slot: usize| orders & (1 << slot) != 0;
        let order_count = orders.count_ones() as usize;
        // Cache shares: three for the dictionary and one per kept order file (9, or 15 with the six quad orders, for
        // a six-order base), so a base that keeps fewer orders gives the rest more cache instead of leaving it unused.
        let shares = 3 + order_count + if named_graphs { order_count } else { 0 };
        // RDFSNAP5: the RDFSNAP4 identities, then six quad orders and the catalog.
        let mut expected = vec![[0u8; 32]; if version == 3 { 2 } else { 1 } + order_count + if named_graphs { order_count + 1 } else { 0 } + usize::from(version == 7)];
        let last = expected.len() - 1;
        for (i, item) in expected.iter_mut().enumerate() {
            match manifest.read_exact(item) {
                // RDFSNAP7 bases written before the parts table existed (unreleased, `rdf-12` builds of e51312ec)
                // end one identity early.
                Err(e) if version == 7 && i == last && e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err(bad("RDFSNAP7 base without a triple-term table: rebuild it"));
                }
                r => r?,
            }
        }
        let mut extra = [0];
        if manifest.read(&mut extra)? != 0 {
            return Err(bad("manifest trailing bytes"));
        }
        let (dictionary, index_offset) = if version >= 4 {
            use super::lazy_dictionary::{DictionaryCache, LazyIndex, Order, UNIFIED_FILE};
            let unified = LazyIndex::open_shared(&dir.join(UNIFIED_FILE), Order::Unified, DictionaryCache::with_shares(shares))?;
            if unified.identity() != expected[0] {
                return Err(bad("dictionary directory checksum mismatch"));
            }
            (DictionaryBackend::Unified(unified), 1)
        } else if version == 3 {
            use super::lazy_dictionary::{DictionaryCache, LazyIndex, Order};
            let cache = DictionaryCache::new();
            let by_term = LazyIndex::open_shared(&dir.join("dictionary.term"), Order::Term, std::sync::Arc::clone(&cache))?;
            let by_id = LazyIndex::open_shared(&dir.join("dictionary.id"), Order::Id, cache)?;
            if by_term.identity() != expected[0] || by_id.identity() != expected[1] {
                return Err(bad("dictionary directory checksum mismatch"));
            }
            (DictionaryBackend::Lazy { by_term, by_id }, 2)
        } else {
            if digest(&dir.join(FILES[0]))? != expected[0] {
                return Err(bad("dictionary checksum mismatch"));
            }
            let (forward, reverse) = read_dictionary(&dir.join(FILES[0]))?;
            (DictionaryBackend::Legacy { forward, reverse }, 1)
        };
        let mut indexes = Vec::new();
        // With the six quad orders, every index takes a smaller share so the snapshot stays within its cache budget.
        let mut identities = expected[index_offset..].iter();
        for (i, name) in FILES[1..].iter().enumerate() {
            if !kept(i) { indexes.push(None); continue }
            let index = DiskIndex::<3>::open_with_share(dir.join(name), shares)?;
            if Some(&index.identity()) != identities.next() {
                return Err(bad("index directory checksum mismatch"));
            }
            indexes.push(Some(index));
        }
        let rows = indexes[0].as_ref().map_or(0, DiskIndex::len);
        if indexes.iter().flatten().any(|index| index.len() != rows) {
            return Err(bad("index counts differ"));
        }
        let named = if named_graphs {
            let mut quads = Vec::new();
            for (i, name) in QUAD_FILES.iter().enumerate() {
                if !kept(i) { quads.push(None); continue }
                let index = DiskIndex::<4>::open_with_share(dir.join(name), shares)?;
                if Some(&index.identity()) != identities.next() {
                    return Err(bad("quad index directory checksum mismatch"));
                }
                quads.push(Some(index));
            }
            let rows = quads[0].as_ref().map_or(0, DiskIndex::len);
            if quads.iter().flatten().any(|q| q.len() != rows) {
                return Err(bad("quad index counts differ"));
            }
            let catalog = read_catalog(&dir.join(CATALOG_FILE), *identities.next().expect("sized above"), rows)?;
            Some(NamedBase { quads, catalog })
        } else {
            None
        };
        // RDFSNAP7: the triple-term parts table, the last identity.
        let triple_terms = if version == 7 {
            let index = DiskIndex::<4>::open(dir.join(TRIPLE_TERMS_FILE))?;
            if Some(&index.identity()) != identities.next() {
                return Err(bad("triple-term table checksum mismatch"));
            }
            Some(index)
        } else {
            None
        };
        Ok(Self {
            dictionary,
            indexes,
            named,
            seek_memo: Default::default(),
            rdf12: version == 7,
            triple_terms,
        })
    }
    /// Exhaustive, exact verification. The phases run in this order because the later ones rely on the earlier:
    /// 1. Dictionary: every term-order entry `(id, term)` must equal `by_id[id]`, adjacent term-order terms must
    ///    differ, and both indexes must hold the same number of entries. The term index is strictly sorted, so its
    ///    N terms are distinct and map injectively into the ID index; with equal counts the two are one bijection,
    ///    and no reverse lookup is needed.
    /// 2. Indexes (in parallel): every block re-read and validated (strictly sorted, so no duplicate rows), and all
    ///    six indexes have the same row count.
    /// 3. Permutations (in parallel): every row of every order is in SPO. With phase 2 (no duplicates, equal counts)
    ///    this makes all six orders the same triple set.
    /// 4. References: every ID in SPO resolves in the dictionary. Because of phases 2 and 3 this covers every ID in
    ///    every order. Do not reorder these phases.
    pub fn verify(&self) -> io::Result<()> {
        use rayon::prelude::*;
        if let DictionaryBackend::Unified(unified) = &self.dictionary {
            // Every block decode already enforces: one kind per block, strictly increasing IDs and, inside a kind,
            // strictly increasing term text that agrees with the kind. That makes (ID <-> term) one bijection and
            // `find_term` exact, so a single pass over all blocks is the whole dictionary check.
            unified.clear_cache()?;
            let count = unified.check_text_order()?;
            if count == 0 && !self.index(Perm::Spo).is_empty() {
                return Err(bad("dictionary is empty but the indexes are not"));
            }
            // RDFSNAP7: the parts table has exactly one row per triple term (kind 4), in ID order, and each row's
            // parts are that term's subject, predicate and object.
            #[cfg(feature = "rdf-12")]
            if let Some(table) = &self.triple_terms {
                table.validate()?;
                let mut row = 0u64;
                unified.for_each_of_kind(crate::model::id_tag::TRIPLE_TERM, &mut |id, term| {
                    let Term::Triple(t) = term else { return Err(bad("kind 4 entry is not a triple term")) };
                    if row >= table.len() { return Err(bad("triple-term table is missing a row")) }
                    let [tt, s, p, o] = table.get(row)?;
                    let parts: [Term; 3] = [t.subject.clone().into(), t.predicate.clone().into(), t.object.clone()];
                    if tt != id || [s, p, o].iter().zip(&parts).any(|(&pid, part)| self.resolve(pid).ok().as_ref() != Some(part)) {
                        return Err(bad("triple-term table disagrees with the dictionary"));
                    }
                    row += 1;
                    Ok(())
                })?;
                if row != table.len() { return Err(bad("triple-term table has extra rows")) }
            }
        }
        if let DictionaryBackend::Lazy { by_term, by_id } = &self.dictionary {
            by_term.clear_cache()?;
            by_id.clear_cache()?;
            let mut term_count = 0u64;
            let mut previous: Option<Term> = None;
            by_term.for_each(&mut |id, term| {
                if previous.as_ref() == Some(term) {
                    return Err(bad("dictionary term index repeats a term"));
                }
                let mut found = None;
                by_id.with_id(id, &mut |v| found = Some(v.into_owned()))?;
                if found.as_ref() != Some(term) {
                    return Err(bad("dictionary index disagreement"));
                }
                previous = Some(term.clone());
                term_count += 1;
                Ok(())
            })?;
            let mut id_count = 0u64;
            by_id.for_each(&mut |_, _| {
                id_count += 1;
                Ok(())
            })?;
            if term_count != id_count {
                return Err(bad("dictionary index count mismatch"));
            }
        }
        // Phase 2.
        self.indexes.par_iter().flatten().try_for_each(|index| -> io::Result<()> {
            index.clear_cache()?;
            index.validate()
        })?;
        let rows = self.index(Perm::Spo).len();
        if self.indexes.iter().flatten().any(|index| index.len() != rows) {
            return Err(bad("permutation row counts differ"));
        }
        // Phase 3.
        self.verify_permutations()?;
        // Phase 4: SPO only, skipping an ID equal to the previous row's in the same column.
        let spo = self.index(Perm::Spo);
        let mut last = [None; 3];
        for i in 0..spo.len() {
            for (k, id) in spo.get(i)?.into_iter().enumerate() {
                if last[k] != Some(id) {
                    self.with_term(id, &mut |_| ())?;
                    last[k] = Some(id);
                }
            }
        }
        self.verify_named()
    }
    /// Phases 2-4 for the named graphs (`RDFSNAP5`): every quad block, every quad order's rows in G+SPO, every ID
    /// of G+SPO in the dictionary, every catalog ID an IRI with the triple count G+SPO holds for it.
    fn verify_named(&self) -> io::Result<()> {
        use rayon::prelude::*;
        let Some(named) = &self.named else { return Ok(()) };
        named.quads.par_iter().flatten().try_for_each(|index| -> io::Result<()> {
            index.clear_cache()?;
            index.validate()
        })?;
        let base = named.quads[0].as_ref().expect("a base keeps GSPO");
        if named.quads.iter().flatten().any(|q| q.len() != base.len()) {
            return Err(bad("quad index counts differ"));
        }
        Perm::ALL.par_iter().enumerate().try_for_each(|(slot, &perm)| -> io::Result<()> {
            let Some(index) = &named.quads[slot] else { return Ok(()) };
            let order = perm.order();
            for i in 0..index.len() {
                let row = index.get(i)?;
                let mut gspo = [row[0], TermId(0), TermId(0), TermId(0)];
                for k in 0..3 { gspo[1 + order[k]] = row[1 + k] }
                let matching = base.range(&gspo)?;
                if matching.end - matching.start != 1 {
                    return Err(bad("quad order contents disagree"));
                }
            }
            Ok(())
        })?;
        let mut last = [None; 4];
        for i in 0..base.len() {
            for (k, id) in base.get(i)?.into_iter().enumerate() {
                if last[k] != Some(id) {
                    self.with_term(id, &mut |_| ())?;
                    last[k] = Some(id);
                }
            }
        }
        for &(g, count) in &named.catalog {
            let mut iri = false;
            self.with_term(g, &mut |t| iri = matches!(t, triplelight_rdf::TermRef::NamedNode(_)))?;
            let range = base.range(&[g])?;
            if !iri || range.end - range.start != count {
                return Err(bad("graph catalog disagrees with the quad orders"));
            }
        }
        Ok(())
    }
    /// The named graphs of this base, ascending by ID, empty graphs included (none unless `RDFSNAP5`).
    pub fn graph_names(&self) -> Vec<TermId> {
        self.named.as_ref().map_or_else(Vec::new, |n| n.catalog.iter().map(|&(g, _)| g).collect())
    }
    /// A read view of named graph `g`, or `None` if the catalog does not list it.
    pub fn graph_view(self: &std::sync::Arc<Self>, g: TermId) -> Option<DiskGraph> {
        let named = self.named.as_ref()?;
        named.catalog.binary_search_by_key(&g, |&(id, _)| id).ok()?;
        Some(DiskGraph { snap: std::sync::Arc::clone(self), g, seek_memo: Default::default() })
    }
    /// Exhaustively check that every row of every permutation is in SPO (the six orders in parallel). Together with
    /// strictly sorted indexes of equal length (`verify` phase 2) this means all permutations hold the same set.
    /// This can be expensive; open() does not perform it.
    pub fn verify_permutations(&self) -> io::Result<()> {
        use rayon::prelude::*;
        let base = self.index(Perm::Spo);
        Perm::ALL.par_iter().try_for_each(|&perm| -> io::Result<()> {
            let Some(index) = self.kept(perm) else { return Ok(()) };
            let order = perm.order();
            for i in 0..index.len() {
                let row = index.get(i)?;
                let mut spo = [TermId(0); 3];
                for k in 0..3 {
                    spo[order[k]] = row[k];
                }
                let matching = base.range(&spo)?;
                if matching.end - matching.start != 1 {
                    return Err(bad("permutation contents disagree"));
                }
            }
            Ok(())
        })
    }
    fn kept(&self, perm: Perm) -> Option<&DiskIndex> {
        self.indexes[Perm::ALL.iter().position(|x| *x == perm).unwrap()].as_ref()
    }
    /// A kept order. SPO, PSO, POS and OSP are always kept.
    fn index(&self, perm: Perm) -> &DiskIndex {
        self.kept(perm).expect("a base keeps every order it is asked for directly")
    }
}
impl Dictionary for DiskSnapshot {
    /// From the `triple_terms` table: one block read. A base without the table (anything but `RDFSNAP7`) holds no
    /// triple term.
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> {
        let Some(table) = &self.triple_terms else { return Ok(None) };
        let range = table.range(&[id])?;
        if range.is_empty() { return Ok(None) }
        let [_, s, p, o] = table.get(range.start)?;
        Ok(Some([s, p, o]))
    }
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { forward, .. } => Ok(forward.get(term).copied()),
            DictionaryBackend::Lazy { by_term, .. } => {
                Ok(by_term.find(term.to_string().as_bytes())?.map(|v| v.0))
            }
            DictionaryBackend::Unified(unified) => Ok(unified.find_term(term.to_string().as_bytes())?.map(|v| v.0)),
        }
    }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        let mut value = None;
        self.with_term(id, &mut |v| value = Some(v.into_owned()))?;
        value.ok_or_else(|| bad("dictionary callback not invoked"))
    }
    fn with_term(
        &self,
        id: TermId,
        f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { reverse, .. } => {
                f(reverse
                    .get(&id)
                    .ok_or_else(|| bad("unknown term ID"))?
                    .as_ref());
                Ok(())
            }
            DictionaryBackend::Lazy { by_id, .. } => by_id.with_id(id, f),
            DictionaryBackend::Unified(unified) => unified.with_id(id, f),
        }
    }
    fn with_terms(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, triplelight_rdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        match &self.dictionary {
            DictionaryBackend::Legacy { reverse, .. } => {
                for (i, id) in ids.iter().enumerate() {
                    f(
                        i,
                        reverse
                            .get(id)
                            .ok_or_else(|| bad("unknown term ID"))?
                            .as_ref(),
                    );
                }
                Ok(())
            }
            DictionaryBackend::Lazy { by_id, .. } => by_id.with_ids(ids, f),
            DictionaryBackend::Unified(unified) => unified.with_ids(ids, f),
        }
    }
}

impl Snapshot for DiskSnapshot {
    fn has_rdf12_terms(&self) -> bool { self.rdf12 }
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        super::derived_order::scan(self, perm, prefix)
    }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        let index = self.index(Perm::Spo);
        let mut pos = 0;
        Box::new(std::iter::from_fn(move || {
            if pos >= index.len() { return None; }
            let end = pos.saturating_add(4096).min(index.len());
            match index.block_slice_uncached(pos, end) {
                Ok(rows) => {
                    pos += rows.len() as u64;
                    let mut batch = TripleBatch::default();
                    for row in rows { for (col, value) in batch.cols.iter_mut().zip(row) { col.push(value); } }
                    Some(Ok(batch))
                }
                Err(e) => { pos = index.len(); Some(Err(e)) }
            }
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { super::derived_order::count(self, perm, prefix) }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        super::derived_order::seek(self, perm, prefix, target)
    }
}
impl super::derived_order::KeptOrders for DiskSnapshot {
    fn has(&self, perm: Perm) -> bool { self.kept(perm).is_some() }
    fn seek_memo(&self) -> &super::derived_order::SeekMemo { &self.seek_memo }
    fn scan_kept<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        let index = self.index(perm);
        match index.range(prefix) {
            Err(e) => Box::new(std::iter::once(Err(e))),
            Ok(range) => {
                let mut pos = range.start;
                Box::new(std::iter::from_fn(move || {
                    if pos >= range.end {
                        return None;
                    }
                    let end = pos.saturating_add(4096).min(range.end);
                    let mut batch = TripleBatch::default();
                    while pos < end {
                        match index.block_slice(pos, end) {
                            Ok(rows) => {
                                pos += rows.len() as u64;
                                for row in rows {
                                    for (col, value) in batch.cols.iter_mut().zip(row) {
                                        col.push(value);
                                    }
                                }
                            }
                            Err(e) => {
                                pos = range.end;
                                return Some(Err(e));
                            }
                        }
                    }
                    Some(Ok(batch))
                }))
            }
        }
    }
    fn count_kept(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        let r = self.index(perm).range(prefix)?;
        Ok(r.end - r.start)
    }
    fn seek_kept(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        self.index(perm).seek_value(prefix, target)
    }
}

/// One named graph of an `RDFSNAP5` base: the graph-first quad orders with `g` bound, over the base's dictionary.
pub struct DiskGraph {
    snap: std::sync::Arc<DiskSnapshot>,
    g: TermId,
    seek_memo: super::derived_order::SeekMemo,
}
impl DiskGraph {
    fn kept(&self, perm: Perm) -> Option<&DiskIndex<4>> {
        self.snap.named.as_ref().expect("a graph view exists only for RDFSNAP5").quads[Perm::ALL.iter().position(|x| *x == perm).unwrap()].as_ref()
    }
    fn index(&self, perm: Perm) -> &DiskIndex<4> { self.kept(perm).expect("a base keeps every quad order it is asked for directly") }
    fn key(&self, prefix: &[TermId]) -> Vec<TermId> {
        let mut key = Vec::with_capacity(prefix.len() + 1);
        key.push(self.g);
        key.extend_from_slice(prefix);
        key
    }
}
impl Dictionary for DiskGraph {
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> { self.snap.triple_term_parts(id) }
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.snap.lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.snap.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.snap.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> {
        self.snap.with_terms(ids, f)
    }
}
impl Snapshot for DiskGraph {
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        super::derived_order::scan(self, perm, prefix)
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { super::derived_order::count(self, perm, prefix) }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        super::derived_order::seek(self, perm, prefix, target)
    }
}
impl super::derived_order::KeptOrders for DiskGraph {
    fn has(&self, perm: Perm) -> bool { self.kept(perm).is_some() }
    fn seek_memo(&self) -> &super::derived_order::SeekMemo { &self.seek_memo }
    fn scan_kept<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        let index = self.index(perm);
        match index.range(&self.key(prefix)) {
            Err(e) => Box::new(std::iter::once(Err(e))),
            Ok(range) => {
                let mut pos = range.start;
                Box::new(std::iter::from_fn(move || {
                    if pos >= range.end { return None }
                    let end = pos.saturating_add(4096).min(range.end);
                    let mut batch = TripleBatch::default();
                    while pos < end {
                        match index.block_slice(pos, end) {
                            Ok(rows) => {
                                pos += rows.len() as u64;
                                for row in rows {
                                    for (col, &value) in batch.cols.iter_mut().zip(&row[1..]) { col.push(value) }
                                }
                            }
                            Err(e) => { pos = range.end; return Some(Err(e)) }
                        }
                    }
                    Some(Ok(batch))
                }))
            }
        }
    }
    fn count_kept(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        let r = self.index(perm).range(&self.key(prefix))?;
        Ok(r.end - r.start)
    }
    fn seek_kept(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        if prefix.len() >= 3 { return Ok(None) }
        self.index(perm).seek_value(&self.key(prefix), target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use triplelight_rdf::{Literal, NamedNode, Triple};
    /// Task #73: compaction keeps the order set of the base it replaces, four or six, for default-graph and dataset
    /// bases alike, whatever this process's build setting.
    #[test]
    fn compaction_keeps_the_order_set() {
        use crate::model::{Change, DatasetChange, GraphTarget};
        use crate::store::{Overlay, Store};
        let iri = |s: &str| NamedNode::new(format!("http://ex/{s}")).unwrap();
        for (label, orders) in [("four", REQUIRED_ORDERS), ("six", ALL_ORDERS)] {
            let dir = std::env::temp_dir().join(format!("rdf-compact-orders-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).unwrap();
            let empty = Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([])));
            crate::external_build::from_dataset_keeping(&dir.join("base-0"), &empty, orders).unwrap();
            fs::write(dir.join("CURRENT"), "base-0\n").unwrap();
            let store = Store::open_with(&dir, None, Box::new(DiskBase { workers: 1 })).unwrap();
            let t = |s: &str| Triple::new(iri(s), iri("p"), iri("o"));
            store.commit(&[(Change::Insert, t("a"))]).unwrap();
            store.compact().unwrap();
            assert_eq!(kept_orders(&dir.join("base-1")).unwrap(), orders, "{label} default-graph compaction");
            store.commit_dataset(&[DatasetChange::Quad(GraphTarget::Named(iri("g")), Change::Insert, t("b"))]).unwrap();
            store.compact().unwrap();
            let manifest = fs::read(dir.join("base-2").join("MANIFEST")).unwrap();
            assert_eq!(kept_orders(&dir.join("base-2")).unwrap(), orders, "{label} dataset compaction");
            assert_eq!(&manifest[..8], if orders == ALL_ORDERS { b"RDFSNAP5" } else { b"RDFSNAP6" });
            let base = DiskSnapshot::open(dir.join("base-2")).unwrap();
            base.verify().unwrap();
            assert_eq!(base.count(Perm::Sop, &[]).unwrap(), 1);
            assert_eq!(base.graph_names().len(), 1);
            drop(store);
            fs::remove_dir_all(&dir).unwrap();
        }
    }
    /// Task #73: a base without SOP and OPS answers every scan, count and seek on them exactly as the same base with
    /// all six orders, for the default graph and every named graph. The fixture has a subject and an object with more
    /// than one batch (4096 rows) of triples, so regrouped groups span batches.
    #[test]
    fn derived_orders_answer_like_stored_ones() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use crate::model::{Change, DatasetChange, GraphTarget};
        let iri = |s: &str| NamedNode::new(format!("http://ex/{s}")).unwrap();
        let mut changes = Vec::new();
        let mut add = |t: Triple, g: Option<&str>| changes.push(DatasetChange::Quad(g.map_or(GraphTarget::Default, |g| GraphTarget::Named(iri(g))), Change::Insert, t));
        for i in 0..600u32 {
            let t = Triple::new(iri(&format!("s{}", i % 29)), iri(&format!("p{}", i % 5)), Literal::new_simple_literal(format!("{}", i % 37)));
            add(t.clone(), None);
            add(t, Some(["g1", "g2"][(i % 2) as usize]));
        }
        for i in 0..5000u32 {
            add(Triple::new(iri("hub"), iri(&format!("p{}", i % 3)), iri(&format!("o{i}"))), None);
            add(Triple::new(iri(&format!("in{i}")), iri(&format!("p{}", i % 3)), iri("sink")), Some("g1"));
        }
        let source = crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_dataset_changes(&changes).unwrap().0;
        let parent = std::env::temp_dir().join(format!("rdf-derived-orders-{}", std::process::id()));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir(&parent).unwrap();
        let (six, four) = (parent.join("six"), parent.join("four"));
        write_disk_dataset(&six, &source).unwrap();
        crate::external_build::from_dataset_keeping(&four, &source, REQUIRED_ORDERS).unwrap();
        // The four-order base is RDFSNAP6 and has no SOP/OPS files; everything it keeps is byte-identical.
        let manifest = fs::read(four.join("MANIFEST")).unwrap();
        assert_eq!(&manifest[..10], b"RDFSNAP6\x1d\x01");
        assert_eq!(manifest.len(), 10 + 32 * (1 + 4 + 4 + 1));
        for name in ["sop", "ops", "gsop", "gops"] { assert!(!four.join(name).exists(), "{name}") }
        for name in ["dictionary.unified", "spo", "pso", "pos", "osp", "gspo", "gpso", "gpos", "gosp", "graphs"] {
            assert_eq!(fs::read(four.join(name)).unwrap(), fs::read(six.join(name)).unwrap(), "{name}");
        }
        let full = std::sync::Arc::new(DiskSnapshot::open(&six).unwrap());
        let derived = std::sync::Arc::new(DiskSnapshot::open(&four).unwrap());
        derived.verify().unwrap();
        // Cache shares follow the kept files: 3 + 6 + 6 = 15 for the six-order dataset base, 3 + 4 + 4 = 11 for the
        // four-order one, so each kept index of the smaller base gets a larger share.
        let capacity = |shares: usize| (crate::disk_index::cache_budget_mb() * 1024 * 1024 / (shares * 16 * 256 * 24)).max(1);
        assert_eq!(full.index(Perm::Spo).shard_capacity(), capacity(15));
        assert_eq!(derived.index(Perm::Spo).shard_capacity(), capacity(11));
        let rows = |view: &dyn Snapshot, perm: Perm, prefix: &[TermId]| -> Vec<[TermId; 3]> {
            let mut v = Vec::new();
            for b in view.scan(perm, prefix) {
                let b = b.unwrap();
                assert!(!b.is_empty() && b.len() <= 4096, "{perm:?} {prefix:?} batch of {}", b.len());
                v.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]));
            }
            v
        };
        let check = |full: &dyn Snapshot, derived: &dyn Snapshot, label: &str| {
            for perm in [Perm::Sop, Perm::Ops] {
                let all = rows(full, perm, &[]);
                assert!(all.len() > 4096, "{label}: the fixture spans batches");
                assert_eq!(rows(derived, perm, &[]), all, "{label} {perm:?} full scan");
                let mut prefixes: Vec<Vec<TermId>> = vec![vec![], vec![TermId(1)], vec![TermId(u64::MAX), TermId(1)]];
                for r in all.iter().step_by(97) { for k in 1..=3 { prefixes.push(r[..k].to_vec()) } }
                prefixes.extend([all[0][..1].to_vec(), all[all.len() - 1][..1].to_vec()]);
                for prefix in &prefixes {
                    assert_eq!(rows(derived, perm, prefix), rows(full, perm, prefix), "{label} {perm:?} {prefix:?}");
                    assert_eq!(derived.count(perm, prefix).unwrap(), full.count(perm, prefix).unwrap(), "{label} {perm:?} {prefix:?}");
                    if prefix.len() < 3 {
                        let mut targets = vec![TermId(0), TermId(u64::MAX)];
                        targets.extend(all.iter().step_by(311).map(|r| r[prefix.len()]));
                        targets.extend(targets.clone().into_iter().map(|t| TermId(t.0.saturating_add(1))));
                        for t in targets {
                            assert_eq!(derived.seek(perm, prefix, t).unwrap(), full.seek(perm, prefix, t).unwrap(), "{label} {perm:?} {prefix:?} {t:?}");
                        }
                    }
                }
            }
        };
        check(&*full, &*derived, "default");
        // Bounded regroup: every group above 1, 7 or 300 rows goes through the external sorter in a private scratch
        // directory; the answers must not change, and no scratch may survive a finished or abandoned scan.
        let scratch = parent.join("spill");
        fs::create_dir(&scratch).unwrap();
        for limit in [1usize, 7, 300] {
            crate::derived_order::spill_on_this_thread(Some(limit), Some(scratch.clone()));
            check(&*full, &*derived, &format!("spill above {limit}"));
            assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0, "scratch left after spill above {limit}");
        }
        {
            // Dropped after one batch, mid-way through a spilled group (the 5,000-row `sink` group): its scratch goes
            // with it.
            crate::derived_order::spill_on_this_thread(Some(1), Some(scratch.clone()));
            let sink = derived.lookup(&iri("sink").into()).unwrap().unwrap();
            let g1 = derived.graph_view(derived.lookup(&iri("g1").into()).unwrap().unwrap()).unwrap();
            let mut it = g1.scan(Perm::Ops, &[sink]);
            assert!(!it.next().unwrap().unwrap().is_empty());
            assert!(fs::read_dir(&scratch).unwrap().count() > 0, "a group is spilled");
        }
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0, "scratch left after an abandoned scan");
        crate::derived_order::spill_on_this_thread(None, None);
        for g in full.graph_names() {
            let (f, d) = (full.graph_view(g).unwrap(), derived.graph_view(g).unwrap());
            if rows(&f, Perm::Spo, &[]).len() > 4096 { check(&f, &d, "g1") }
            else {
                for perm in [Perm::Sop, Perm::Ops] { assert_eq!(rows(&d, perm, &[]), rows(&f, perm, &[]), "{g:?} {perm:?}") }
            }
        }
        // A default-graph-only base keeps the same layout without the quad orders and catalog.
        let plain = parent.join("plain");
        crate::external_build::from_dataset_keeping(&plain, &crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([Triple::new(iri("a"), iri("b"), iri("c"))]))), REQUIRED_ORDERS).unwrap();
        let manifest = fs::read(plain.join("MANIFEST")).unwrap();
        assert_eq!((&manifest[..10], manifest.len()), (&b"RDFSNAP6\x1d\x00"[..], 10 + 32 * 5));
        let plain = DiskSnapshot::open(&plain).unwrap();
        plain.verify().unwrap();
        let [spo] = rows(&plain, Perm::Spo, &[])[..] else { panic!("one triple") };
        assert_eq!(rows(&plain, Perm::Sop, &[]), [[spo[0], spo[2], spo[1]]]);
        // Order sets that drop a required order, use reserved bits or a bad named-graph flag are refused.
        for (orders, named) in [(REQUIRED_ORDERS & !1, 0u8), (REQUIRED_ORDERS & !(1 << 4), 0), (REQUIRED_ORDERS | 0x40, 0), (REQUIRED_ORDERS, 2)] {
            let mut bad = manifest.clone();
            (bad[8], bad[9]) = (orders, named);
            let dir = parent.join(format!("bad-{orders}-{named}"));
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("MANIFEST"), &bad).unwrap();
            let err = DiskSnapshot::open(&dir).err().expect("refused");
            assert!(err.to_string().contains("order set"), "{orders:#b} {named}: {err}");
        }
        fs::remove_dir_all(&parent).unwrap();
    }
    /// A dataset base (`RDFSNAP5`): every named graph view answers like the overlay it was written from (by term)
    /// and like a row model of its quad rows (by ID) for every order, prefix and seek; the catalog keeps empty graphs.
    #[test]
    fn dataset_base_matches_its_source_and_a_row_model() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use crate::model::{Change, DatasetChange, GraphTarget};
        use triplelight_core::storage::DatasetView;
        let iri = |s: &str| NamedNode::new(format!("http://ex/{s}")).unwrap();
        let mut changes = Vec::new();
        for i in 0..400u32 {
            let t = Triple::new(iri(&format!("s{}", i % 23)), iri(&format!("p{}", i % 4)), Literal::new_simple_literal(format!("{}", i % 31)));
            let g = ["g1", "g2", "g3"][(i % 3) as usize];
            changes.push(DatasetChange::Quad(GraphTarget::Named(iri(g)), Change::Insert, t.clone()));
            if i % 5 == 0 { changes.push(DatasetChange::Quad(GraphTarget::Default, Change::Insert, t)) }
        }
        changes.push(DatasetChange::Create { graph: iri("empty"), silent: false });
        let source = crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_dataset_changes(&changes).unwrap().0;
        let parent = std::env::temp_dir().join(format!("rdf-dataset-base-{}", std::process::id()));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir(&parent).unwrap();
        let dir = parent.join("base");
        write_disk_dataset(&dir, &source).unwrap();
        assert_eq!(&fs::read(dir.join("MANIFEST")).unwrap()[..8], b"RDFSNAP5");
        let disk = std::sync::Arc::new(DiskSnapshot::open(&dir).unwrap());
        disk.verify().unwrap();
        let rows_as_text = |view: &dyn Snapshot, perm: Perm| -> Vec<String> {
            let mut v = Vec::new();
            for b in view.scan(perm, &[]) {
                let b = b.unwrap();
                let r = |id: TermId| view.resolve(id).unwrap().to_string();
                for ((&s, &p), &o) in b.cols[0].iter().zip(&b.cols[1]).zip(&b.cols[2]) { v.push(format!("{} {} {}", r(s), r(p), r(o))) }
            }
            v.sort();
            v
        };
        for p in Perm::ALL { assert_eq!(rows_as_text(&*disk, p), rows_as_text(&source, p), "default {p:?}") }
        let names = |ids: Vec<TermId>, d: &dyn Dictionary| -> Vec<String> { let mut v: Vec<String> = ids.into_iter().map(|g| d.resolve(g).unwrap().to_string()).collect(); v.sort(); v };
        assert_eq!(names(disk.graph_names(), &*disk), names(source.graph_names().unwrap(), &source));
        assert_eq!(disk.graph_names().len(), 4, "the empty graph is in the catalog");
        for g in disk.graph_names() {
            let view = disk.graph_view(g).unwrap();
            let src = source.graph(source.lookup(&disk.resolve(g).unwrap()).unwrap().unwrap()).unwrap().unwrap();
            let mut spo: Vec<[TermId; 3]> = Vec::new();
            for b in view.scan(Perm::Spo, &[]) { let b = b.unwrap(); spo.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])) }
            for p in Perm::ALL {
                assert_eq!(rows_as_text(&view, p), rows_as_text(src, p), "graph {g:?} {p:?}");
                let order = p.order();
                let mut rows: Vec<[TermId; 3]> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
                rows.sort();
                let mut prefixes: Vec<Vec<TermId>> = vec![vec![], vec![TermId(1)]];
                for r in rows.iter().step_by(7) { for k in 1..=3 { prefixes.push(r[..k].to_vec()) } }
                for prefix in &prefixes {
                    let want: Vec<[TermId; 3]> = rows.iter().filter(|r| r[..prefix.len()] == prefix[..]).copied().collect();
                    let mut got = Vec::new();
                    for b in view.scan(p, prefix) { let b = b.unwrap(); got.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])) }
                    assert_eq!(got, want, "{p:?} {prefix:?}");
                    assert_eq!(view.count(p, prefix).unwrap(), want.len() as u64);
                    if prefix.len() < 3 {
                        for target in want.iter().map(|r| r[prefix.len()]).chain([TermId(0), TermId(u64::MAX)]) {
                            let expect = want.iter().map(|r| r[prefix.len()]).filter(|&v| v >= target).min();
                            assert_eq!(view.seek(p, prefix, target).unwrap(), expect);
                        }
                    }
                }
            }
        }
        assert!(disk.graph_view(TermId(1)).is_none());
        // Without named graphs the dataset writer gives exactly the RDFSNAP4 base of the default graph.
        let plain = crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_changes(
            &changes.iter().filter_map(|c| match c { DatasetChange::Quad(GraphTarget::Default, ch, t) => Some((*ch, t.clone())), _ => None }).collect::<Vec<_>>()).unwrap().0;
        let (a, b) = (parent.join("a"), parent.join("b"));
        write_disk_dataset(&a, &plain).unwrap();
        write_disk_snapshot(&b, &plain).unwrap();
        for entry in fs::read_dir(&b).unwrap() {
            let name = entry.unwrap().file_name();
            assert_eq!(fs::read(a.join(&name)).unwrap(), fs::read(b.join(&name)).unwrap(), "{name:?}");
        }
        assert_eq!(&fs::read(a.join("MANIFEST")).unwrap()[..8], b"RDFSNAP4");
        // Corruption: a catalog byte fails open; a quad row swapped in one order fails verify.
        drop(disk);
        let catalog = fs::read(dir.join(CATALOG_FILE)).unwrap();
        let mut bad_catalog = catalog.clone();
        bad_catalog[20] ^= 1;
        fs::write(dir.join(CATALOG_FILE), &bad_catalog).unwrap();
        assert!(DiskSnapshot::open(&dir).is_err());
        fs::write(dir.join(CATALOG_FILE), &catalog).unwrap();
        let gpos = DiskIndex::<4>::open(dir.join("gpos")).unwrap();
        let mut rows: Vec<[TermId; 4]> = (0..gpos.len()).map(|i| gpos.get(i).unwrap()).collect();
        drop(gpos);
        let last = rows.len() - 1;
        rows[last][3] = TermId(rows[last][3].0 + 1); // a row that is in no other order
        rows.sort();
        rows.dedup();
        let mut out = Vec::new();
        crate::disk_index::write_index(&mut out, rows.into_iter()).unwrap();
        fs::write(dir.join("gpos"), &out).unwrap();
        // Keep the manifest's identity in step, so only verify can notice.
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        let slot = 7 + 3;
        manifest[8 + slot * 32..8 + slot * 32 + 32].copy_from_slice(&DiskIndex::<4>::open(dir.join("gpos")).unwrap().identity());
        fs::write(dir.join("MANIFEST"), &manifest).unwrap();
        let reopened = DiskSnapshot::open(&dir).unwrap();
        assert!(reopened.verify().is_err());
        fs::remove_dir_all(parent).unwrap();
    }

    /// A dataset whose named graphs are all empty (only `CREATE GRAPH`): the base builds with empty quad orders,
    /// lists every graph, verifies, and each view is empty.
    /// #45 step 4 store gate: every stored term reads back and is found again by its text, `lookup(resolve(id)) ==
    /// Some(id)`, in an in-memory build (`RDFSNAP3`), an external build (`RDFSNAP4`) and a dataset base (`RDFSNAP5`).
    /// The term dictionaries search by the text `Display` renders, so a rendering change would make stored terms
    /// unfindable while `verify` still passes. The stores are byte-identical to main's (PR #103), so this also covers
    /// stores built by main.
    #[test]
    fn every_stored_term_is_found_by_its_text() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use crate::model::{Change, DatasetChange, GraphTarget};
        use triplelight_core::storage::DatasetView;
        use triplelight_rdf::BlankNode;
        let iri = |s: &str| NamedNode::new(s).unwrap();
        let xsd = |t: &str| iri(&format!("http://www.w3.org/2001/XMLSchema#{t}"));
        let objects: Vec<Term> = vec![
            Literal::new_simple_literal("plain").into(),
            Literal::new_simple_literal("tab\t new\n line\r \"quote\" back\\slash").into(),
            Literal::new_simple_literal("control \u{0} \u{1} \u{1f} \u{7f}").into(),
            Literal::new_simple_literal("é ü 中文 \u{1F600}").into(),
            Literal::new_simple_literal("").into(),
            Literal::new_simple_literal("seven!!").into(), // a short string: an inline ID in memory stores
            Literal::new_language_tagged_literal("hello", "en-GB").unwrap().into(),
            Literal::new_language_tagged_literal("hallo", "DE-at").unwrap().into(),
            Literal::new_typed_literal("0042", xsd("integer")).into(),
            Literal::new_typed_literal("typed string", xsd("string")).into(),
            Literal::new_typed_literal("1.5e3", xsd("double")).into(),
            Literal::new_typed_literal("x", iri("http://ex/custom-type")).into(),
            iri("http://ex/é/%20?q=1#frag").into(),
            iri(&format!("http://ex/long/{}", "seg/".repeat(500))).into(),
            BlankNode::new("b1").unwrap().into(),
            BlankNode::new("a1b2c3d4e5f6a7b8").unwrap().into(), // hex: oxrdf's numeric-ID form
            BlankNode::new("0123").unwrap().into(),
            BlankNode::default().into(),
        ];
        let subjects = [iri("http://ex/s"), iri("http://ex/s2")];
        let mut triples = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            let s: triplelight_rdf::NamedOrBlankNode = if i % 3 == 0 { BlankNode::new(format!("s{i}")).unwrap().into() } else { subjects[i % 2].clone().into() };
            triples.push(Triple::new(s, iri(&format!("http://ex/p{}", i % 4)), o.clone()));
        }
        let check = |label: &str, dir: &Path, named: bool| {
            let disk = std::sync::Arc::new(DiskSnapshot::open(dir).unwrap());
            disk.verify().unwrap();
            let mut ids: Vec<TermId> = Vec::new();
            let mut views: Vec<&dyn Snapshot> = vec![&*disk];
            let graph_views: Vec<DiskGraph> = disk.graph_names().into_iter().filter_map(|g| { ids.push(g); disk.graph_view(g) }).collect();
            for v in &graph_views { views.push(v) }
            for view in views {
                for b in view.scan(Perm::Spo, &[]) { let b = b.unwrap(); for c in &b.cols { ids.extend(c.iter().copied()) } }
            }
            ids.sort_unstable();
            ids.dedup();
            assert!(ids.len() > objects.len(), "{label}: {} IDs", ids.len());
            for &id in &ids {
                let term = disk.resolve(id).unwrap();
                assert_eq!(disk.lookup(&term).unwrap(), Some(id), "{label}: {term} (id {id:?}) is not found by its text");
            }
            assert_eq!(disk.lookup(&Literal::new_simple_literal("never stored").into()).unwrap(), None, "{label}");
            if named { assert_eq!(disk.graph_names().len(), 2, "{label}") }
            assert_eq!(&fs::read(dir.join("MANIFEST")).unwrap()[..8], match (label, named) { ("in-memory", _) => b"RDFSNAP3", (_, false) => b"RDFSNAP4", (_, true) => b"RDFSNAP5" }, "{label}");
        };
        let parent = std::env::temp_dir().join(format!("rdf-lookup-round-trip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir(&parent).unwrap();
        // In-memory build: RDFSNAP3 (two dictionary files).
        let memory = MemorySnapshot::from_triples(triples.clone());
        DiskSnapshot::create(parent.join("memory"), &memory).unwrap();
        check("in-memory", &parent.join("memory"), false);
        // External build: RDFSNAP4 (the unified dictionary).
        write_disk_snapshot(parent.join("external"), &memory).unwrap();
        check("external", &parent.join("external"), false);
        // Dataset base: RDFSNAP5, with the same terms also in named graphs and as graph names.
        let mut changes: Vec<DatasetChange> = triples.iter().map(|t| DatasetChange::Quad(GraphTarget::Default, Change::Insert, t.clone())).collect();
        for (i, t) in triples.iter().enumerate() {
            changes.push(DatasetChange::Quad(GraphTarget::Named(iri(if i % 2 == 0 { "http://ex/g/é" } else { "http://ex/g2" })), Change::Insert, t.clone()));
        }
        let source = crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_dataset_changes(&changes).unwrap().0;
        assert_eq!(source.graph_names().unwrap().len(), 2);
        write_disk_dataset(parent.join("dataset"), &source).unwrap();
        check("dataset", &parent.join("dataset"), true);
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn dataset_base_with_only_empty_named_graphs() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use crate::model::{Change, DatasetChange, GraphTarget};
        let iri = |s: &str| NamedNode::new(format!("http://ex/{s}")).unwrap();
        let changes = vec![
            DatasetChange::Quad(GraphTarget::Default, Change::Insert, Triple::new(iri("s"), iri("p"), Literal::new_simple_literal("o"))),
            DatasetChange::Create { graph: iri("e1"), silent: false },
            DatasetChange::Create { graph: iri("e2"), silent: false },
        ];
        let source = crate::store::Overlay::from_base(std::sync::Arc::new(MemorySnapshot::from_triples([]))).with_dataset_changes(&changes).unwrap().0;
        let dir = std::env::temp_dir().join(format!("rdf-dataset-empty-graphs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_disk_dataset(&dir, &source).unwrap();
        assert_eq!(&fs::read(dir.join("MANIFEST")).unwrap()[..8], b"RDFSNAP5");
        let disk = std::sync::Arc::new(DiskSnapshot::open(&dir).unwrap());
        disk.verify().unwrap();
        let mut names: Vec<String> = disk.graph_names().into_iter().map(|g| disk.resolve(g).unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, ["<http://ex/e1>", "<http://ex/e2>"]);
        for g in disk.graph_names() {
            let view = disk.graph_view(g).unwrap();
            for p in Perm::ALL {
                assert_eq!(view.count(p, &[]).unwrap(), 0);
                assert!(view.scan(p, &[]).next().is_none());
                assert_eq!(view.seek(p, &[], TermId(0)).unwrap(), None);
            }
        }
        assert_eq!(disk.count(Perm::Spo, &[]).unwrap(), 1);
        assert!(!dir.join(".quads.raw").exists(), "no scratch is left behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn generic_snapshot_writer_reopens_and_refuses_overwrite() {
        let source = MemorySnapshot::from_triples([Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_language_tagged_literal("hello", "en").unwrap(),
        )]);
        let parent = std::env::temp_dir().join(format!("rdf-disk-generic-{}", std::process::id()));
        fs::create_dir(&parent).unwrap();
        let first = parent.join("first");
        let second = parent.join("second");
        write_disk_snapshot(&first, &source).unwrap();
        let disk = DiskSnapshot::open(&first).unwrap();
        write_disk_snapshot(&second, &disk).unwrap();
        assert!(write_disk_snapshot(&second, &disk).is_err());
        let copy = DiskSnapshot::open(&second).unwrap();
        copy.verify().unwrap();
        let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
        assert_eq!(
            format!("{:?}", crate::exec::query(&source, q).unwrap()),
            format!("{:?}", crate::exec::query(&copy, q).unwrap())
        );
        drop(copy);
        drop(disk);
        fs::remove_dir_all(parent).unwrap();
    }
    #[test]
    fn lazy_dictionary_block_corruption_is_reported_as_query_error() {
        let triple = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_simple_literal("data"),
        );
        let source = MemorySnapshot::from_triples([triple]);
        let dir =
            std::env::temp_dir().join(format!("rdf-lazy-query-corrupt-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("dictionary.term");
        let mut bytes = fs::read(&path).unwrap();
        bytes[8] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        let err = crate::exec::query(&disk, "SELECT ?o WHERE { <urn:s> <urn:p> ?o }").unwrap_err();
        assert_eq!(err.kind, crate::exec::QueryErrorKind::Storage);
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn lazy_dictionary_corruption_aborts_id_serialization() {
        let triple = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            NamedNode::new("urn:p").unwrap(),
            Literal::new_simple_literal("data"),
        );
        let source = MemorySnapshot::from_triples([triple]);
        let dir =
            std::env::temp_dir().join(format!("rdf-lazy-output-corrupt-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("dictionary.id");
        let mut bytes = fs::read(&path).unwrap();
        bytes[8] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        let opts = crate::exec::Options {
            raw_ids: true,
            ..Default::default()
        };
        let crate::exec::QueryResult::Ids(result) =
            crate::exec::query_with(&disk, "SELECT ?s ?p ?o WHERE { ?s ?p ?o }", &opts).unwrap()
        else {
            panic!("expected IDs")
        };
        assert!(crate::exec::results::serialize_ids(&result, &disk).is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn deep_verification_finds_consistent_checksum_with_wrong_permutation() {
        let p = NamedNode::new("urn:p").unwrap();
        let t = Triple::new(
            NamedNode::new("urn:s").unwrap(),
            p,
            Literal::new_simple_literal("x"),
        );
        let source = MemorySnapshot::from_triples([t]);
        let dir = std::env::temp_dir().join(format!("rdf-disk-perms-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let mut out = File::create(dir.join("sop")).unwrap();
        write_index(&mut out, source.index_rows(0)).unwrap();
        drop(out);
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        let offset = 8 + 3 * 32;
        manifest[offset..offset + 32]
            .copy_from_slice(&DiskIndex::<3>::open(dir.join("sop")).unwrap().identity());
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify_permutations().is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    /// Three triples whose IDs are all in the dictionary, written as a fresh snapshot in `dir`.
    fn three_triple_snapshot(dir: &Path) -> MemorySnapshot {
        let p = NamedNode::new("urn:p").unwrap();
        let source = MemorySnapshot::from_triples((0..3).map(|i| {
            Triple::new(NamedNode::new(format!("urn:s{i}")).unwrap(), p.clone(), Literal::new_simple_literal(format!("value number {i}")))
        }));
        DiskSnapshot::create(dir, &source).unwrap();
        source
    }
    /// Replace one manifest slot's file: slot 0 term dictionary, 1 ID dictionary, 2.. the six orders of `Perm::ALL`.
    fn replace_manifest_identity(dir: &Path, slot: usize, identity: [u8; 32]) {
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        manifest[8 + slot * 32..8 + (slot + 1) * 32].copy_from_slice(&identity);
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
    }
    fn rewrite_order(dir: &Path, perm: Perm, mut rows: Vec<[TermId; 3]>) {
        rows.sort_unstable();
        let name = FILES[1 + Perm::ALL.iter().position(|x| *x == perm).unwrap()];
        fs::remove_file(dir.join(name)).unwrap();
        let mut out = File::create(dir.join(name)).unwrap();
        write_index(&mut out, rows.into_iter()).unwrap();
        drop(out);
        let slot = 2 + Perm::ALL.iter().position(|x| *x == perm).unwrap();
        replace_manifest_identity(dir, slot, DiskIndex::<3>::open(dir.join(name)).unwrap().identity());
    }
    fn verify_error(dir: &Path) -> String {
        let disk = DiskSnapshot::open(dir).unwrap();
        let err = disk.verify().expect_err("verify must fail").to_string();
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
        err
    }
    #[test]
    fn verify_rejects_a_non_spo_order_with_one_triple_swapped_for_another_valid_triple() {
        // Same count, still sorted, every ID valid, but one row is a triple SPO does not contain (Kaiming 0b631830):
        // exactly the case the SPO-only reference check depends on the permutation phase to catch.
        let dir = std::env::temp_dir().join(format!("rdf-verify-swap-{}", std::process::id()));
        let source = three_triple_snapshot(&dir);
        let spo: Vec<[TermId; 3]> = source.index_rows(0).collect();
        let order = Perm::Osp.order();
        let mut osp: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let foreign = [spo[0][0], spo[0][1], spo[1][2]]; // s0 p o1: valid IDs, not a stored triple
        osp[2] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order(&dir, Perm::Osp, osp);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn verify_rejects_an_id_missing_from_the_dictionary_that_only_a_non_spo_order_contains() {
        let dir = std::env::temp_dir().join(format!("rdf-verify-unknown-{}", std::process::id()));
        let source = three_triple_snapshot(&dir);
        let spo: Vec<[TermId; 3]> = source.index_rows(0).collect();
        let order = Perm::Pos.order();
        let mut pos: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let unknown = TermId((1 << 60) | 0x0fff_ffff_ffff_fff0);
        let foreign = [unknown, spo[0][1], spo[0][2]];
        pos[1] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order(&dir, Perm::Pos, pos);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn verify_rejects_a_term_index_that_disagrees_with_the_id_index() {
        let dir = std::env::temp_dir().join(format!("rdf-verify-dict-swap-{}", std::process::id()));
        three_triple_snapshot(&dir);
        let by_term = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap();
        let mut rows = Vec::new();
        by_term.for_each(&mut |id, term| { rows.push((id, term.clone())); Ok(()) }).unwrap();
        drop(by_term);
        // Swap the IDs of the first two terms: still a valid, sorted term index, but it disagrees with the ID index.
        let (a, b) = (rows[0].0, rows[1].0);
        rows[0].0 = b;
        rows[1].0 = a;
        rows.sort_by(|x, y| x.1.to_string().cmp(&y.1.to_string()).then(x.0.cmp(&y.0)));
        fs::remove_file(dir.join("dictionary.term")).unwrap();
        super::super::lazy_dictionary::write_index(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term, rows.into_iter().map(Ok)).unwrap();
        let identity = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap().identity();
        replace_manifest_identity(&dir, 0, identity);
        assert!(verify_error(&dir).contains("dictionary index disagreement"));
    }
    #[test]
    fn a_term_index_cannot_repeat_a_term_under_two_ids() {
        // Term-order keys are the term text alone, and both the writer and every block decode require strictly
        // increasing keys, so a repeated term is unrepresentable; `verify` keeps an explicit guard as well.
        let dir = std::env::temp_dir().join(format!("rdf-verify-dict-dup-{}", std::process::id()));
        three_triple_snapshot(&dir);
        let by_term = super::super::lazy_dictionary::LazyIndex::open(&dir.join("dictionary.term"), super::super::lazy_dictionary::Order::Term).unwrap();
        let mut rows = Vec::new();
        by_term.for_each(&mut |id, term| { rows.push((id, term.clone())); Ok(()) }).unwrap();
        drop(by_term);
        rows[1].1 = rows[0].1.clone();
        rows.sort_by(|x, y| x.1.to_string().cmp(&y.1.to_string()).then(x.0.cmp(&y.0)));
        let path = dir.join("dictionary.term.dup");
        let err = super::super::lazy_dictionary::write_index(&path, super::super::lazy_dictionary::Order::Term, rows.into_iter().map(Ok)).unwrap_err();
        assert!(err.to_string().contains("not unique"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }
    // ---- RDFSNAP4 (external-build) stores: the verify corruption cases of the RDFSNAP3 tests above ----
    /// Builds a three-triple store through the external build (manifest RDFSNAP4) and returns its SPO rows.
    fn unified_snapshot(dir: &Path) -> Vec<[TermId; 3]> {
        let nt = dir.with_extension("nt");
        let text: String = (0..3).map(|i| format!("<urn:s{i}> <urn:p> \"value number {i}\" .\n")).collect();
        fs::write(&nt, text).unwrap();
        super::super::external_build::from_nt(dir, Some(&nt)).unwrap();
        fs::remove_file(&nt).unwrap();
        let disk = DiskSnapshot::open(dir).unwrap();
        assert_eq!(&fs::read(dir.join("MANIFEST")).unwrap()[..8], b"RDFSNAP4");
        let spo = disk.index(Perm::Spo);
        (0..spo.len()).map(|i| spo.get(i).unwrap()).collect()
    }
    /// RDFSNAP4 manifest slots: 0 the unified dictionary, 1.. the six orders of `Perm::ALL`.
    fn replace_identity4(dir: &Path, slot: usize, identity: [u8; 32]) {
        let mut manifest = fs::read(dir.join("MANIFEST")).unwrap();
        manifest[8 + slot * 32..8 + (slot + 1) * 32].copy_from_slice(&identity);
        fs::write(dir.join("MANIFEST"), manifest).unwrap();
    }
    fn rewrite_order4(dir: &Path, perm: Perm, mut rows: Vec<[TermId; 3]>) {
        rows.sort_unstable();
        let pos = Perm::ALL.iter().position(|x| *x == perm).unwrap();
        let name = FILES[1 + pos];
        fs::remove_file(dir.join(name)).unwrap();
        let mut out = File::create(dir.join(name)).unwrap();
        write_index(&mut out, rows.into_iter()).unwrap();
        drop(out);
        replace_identity4(dir, 1 + pos, DiskIndex::<3>::open(dir.join(name)).unwrap().identity());
    }
    #[test]
    fn unified_verify_rejects_a_non_spo_order_with_one_triple_swapped() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        let dir = std::env::temp_dir().join(format!("rdf-verify4-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let spo = unified_snapshot(&dir);
        let order = Perm::Osp.order();
        let mut osp: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let foreign = [spo[0][0], spo[0][1], spo[1][2]];
        osp[2] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order4(&dir, Perm::Osp, osp);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn unified_verify_rejects_an_id_missing_from_the_dictionary() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        let dir = std::env::temp_dir().join(format!("rdf-verify4-unknown-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let spo = unified_snapshot(&dir);
        let order = Perm::Pos.order();
        let mut pos: Vec<_> = spo.iter().map(|r| [r[order[0]], r[order[1]], r[order[2]]]).collect();
        let unknown = TermId((1 << 60) | 0x0fff_ffff_ffff_fff0);
        let foreign = [unknown, spo[0][1], spo[0][2]];
        pos[1] = [foreign[order[0]], foreign[order[1]], foreign[order[2]]];
        rewrite_order4(&dir, Perm::Pos, pos);
        assert!(verify_error(&dir).contains("permutation contents disagree"));
    }
    #[test]
    fn unified_dictionary_byte_flips_fail_open_or_verify() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        let dir = std::env::temp_dir().join(format!("rdf-verify4-flip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unified_snapshot(&dir);
        let path = dir.join("dictionary.unified");
        let good = fs::read(&path).unwrap();
        // A flipped byte inside the first block: the directory (and manifest identity) are untouched, so open works and
        // verify reads the block and fails its checksum.
        let mut bad_block = good.clone();
        bad_block[12] ^= 0xff;
        fs::write(&path, &bad_block).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify().is_err(), "a corrupt block must fail verify");
        drop(disk);
        // A flipped byte in the directory changes the directory identity: open fails against the manifest.
        let mut bad_dir = good.clone();
        let n = bad_dir.len();
        bad_dir[n - 50] ^= 0xff;
        fs::write(&path, &bad_dir).unwrap();
        assert!(DiskSnapshot::open(&dir).is_err(), "a corrupt directory must fail open");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unified_verify_rejects_broken_text_order_that_open_and_id_lookups_accept() {
        crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
        use super::super::lazy_dictionary::{LazyIndex, Order, UNIFIED_FILE, write_unified_without_text_check};
        let dir = std::env::temp_dir().join(format!("rdf-verify4-order-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        unified_snapshot(&dir);
        let path = dir.join(UNIFIED_FILE);
        let u = LazyIndex::open(&path, Order::Unified).unwrap();
        let mut rows = Vec::new();
        u.for_each(&mut |id, term| { rows.push((id, term.clone())); Ok(()) }).unwrap();
        drop(u);
        let first_id = rows[0].0;
        // Keep every ID and kind, but swap the terms of two neighbouring IRIs: the text order is broken.
        let iris: Vec<usize> = rows.iter().enumerate().filter(|(_, (_, t))| matches!(t, Term::NamedNode(_))).map(|(i, _)| i).collect();
        let (a, b) = (iris[1], iris[2]);
        let (ta, tb) = (rows[a].1.clone(), rows[b].1.clone());
        rows[a].1 = tb;
        rows[b].1 = ta;
        fs::remove_file(&path).unwrap();
        write_unified_without_text_check(&path, rows.into_iter().map(Ok)).unwrap();
        replace_identity4(&dir, 0, LazyIndex::open(&path, Order::Unified).unwrap().identity());
        // Open works and an ID lookup answers (the cheap load guard holds) ...
        let disk = DiskSnapshot::open(&dir).unwrap();
        disk.with_term(first_id, &mut |_| ()).unwrap();
        // ... and verify rejects the broken order.
        let err = disk.verify().expect_err("verify must fail").to_string();
        assert!(err.contains("not strictly sorted"), "{err}");
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn data_corruption_is_lazy_but_verify_and_query_fail() {
        let p = NamedNode::new("urn:p").unwrap();
        let source = MemorySnapshot::from_triples([Triple::new(
            NamedNode::new("urn:s").unwrap(),
            p,
            Literal::new_simple_literal("x"),
        )]);
        let dir = std::env::temp_dir().join(format!("rdf-disk-lazy-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let path = dir.join("spo");
        let mut bytes = fs::read(&path).unwrap();
        bytes[24] ^= 1;
        fs::write(path, bytes).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        assert!(disk.verify().is_err());
        assert!(disk.scan(Perm::Spo, &[]).next().unwrap().is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cache_miss_after_truncation_is_an_error() {
        let p = NamedNode::new("urn:p").unwrap();
        let rows = (0..34_000).map(|i| {
            Triple::new(
                NamedNode::new(format!("urn:s{i:06}")).unwrap(),
                p.clone(),
                Literal::new_simple_literal("x"),
            )
        });
        let source = MemorySnapshot::from_triples(rows);
        let dir = std::env::temp_dir().join(format!("rdf-disk-short-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        File::create(dir.join("spo")).unwrap();
        let first = disk.scan(Perm::Spo, &[]).next().unwrap();
        assert!(first.is_err());
        assert!(crate::exec::query(&disk, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1").is_err());
        drop(disk);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn roundtrip_prefix_seek_and_checksum() {
        let p = NamedNode::new("urn:p").unwrap();
        let triples: Vec<_> = (0..600)
            .map(|i| {
                Triple::new(
                    NamedNode::new(format!("urn:s{i}")).unwrap(),
                    p.clone(),
                    Literal::new_simple_literal(format!("v{}", i % 4)),
                )
            })
            .collect();
        let source = MemorySnapshot::from_triples(triples.clone());
        let dir = std::env::temp_dir().join(format!("rdf-disk-snapshot-{}", std::process::id()));
        DiskSnapshot::create(&dir, &source).unwrap();
        let disk = DiskSnapshot::open(&dir).unwrap();
        disk.verify_permutations().unwrap();
        for perm in Perm::ALL {
            let src: Vec<_> = source
                .scan(perm, &[])
                .map(|b| b.unwrap())
                .flat_map(|b| {
                    (0..b.len())
                        .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                        .collect::<Vec<_>>()
                })
                .collect();
            let dst: Vec<_> = disk
                .scan(perm, &[])
                .map(|b| b.unwrap())
                .flat_map(|b| {
                    (0..b.len())
                        .map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(src, dst);
            for row in src.iter().step_by(17) {
                for n in 0..=3 {
                    let prefix = &row[..n];
                    assert_eq!(
                        source.count(perm, prefix).unwrap(),
                        disk.count(perm, prefix).unwrap()
                    );
                    if n < 3 {
                        assert_eq!(
                            source.seek(perm, prefix, row[n]).unwrap(),
                            disk.seek(perm, prefix, row[n]).unwrap()
                        );
                    }
                }
            }
        }
        for query in [
            "SELECT ?s ?o WHERE { ?s <urn:p> ?o } ORDER BY ?s LIMIT 25",
            "ASK { <urn:s5> <urn:p> \"v1\" }",
            "SELECT ?s WHERE { ?s <urn:p> \"v2\" } ORDER BY ?s LIMIT 10",
        ] {
            let memory = crate::exec::query(&source, query).unwrap();
            let persisted = crate::exec::query(&disk, query).unwrap();
            assert_eq!(
                crate::exec::results::serialize(&memory),
                crate::exec::results::serialize(&persisted)
            );
        }
        let t: Term = triples[7].object.clone();
        assert_eq!(disk.lookup(&t).unwrap(), source.lookup(&t).unwrap());
        assert_eq!(disk.resolve(disk.lookup(&t).unwrap().unwrap()).unwrap(), t);
        drop(disk);
        let path = dir.join("spo");
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(DiskSnapshot::open(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
