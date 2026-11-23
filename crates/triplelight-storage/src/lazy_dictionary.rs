//! Immutable on-disk dictionary indexes: the unified dictionary of current bases and the two-file layout of RDFSNAP3 (see docs/lazy-dictionary-design.md).
use super::external_build::MAX_TERM;
use super::term_block_codec;
use crate::model::{id_tag, TermId};
use triplelight_rdf::Term;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Write},
    os::unix::fs::FileExt,
    path::Path,
    sync::{Arc, Mutex},
};
const TERM_MAGIC: &[u8; 8] = b"RDFDTM01";
const ID_MAGIC: &[u8; 8] = b"RDFDID01";
/// One file in ID order whose directory also carries each block's last term text, so term lookups can search a
/// kind's run of blocks (inside a kind, ID order is term-text order).
const UNIFIED_MAGIC: &[u8; 8] = b"RDFDID02";
/// The unified layout with lean entry framing (`term_block_codec`, #105). Written by default; `RDFDID02` stays readable.
const UNIFIED_LEAN_MAGIC: &[u8; 8] = b"RDFDID03";
/// File name of the unified dictionary inside a snapshot directory.
pub(super) const UNIFIED_FILE: &str = "dictionary.unified";
/// Largest encoded block, and the byte budget a block is filled to: one maximum-length term plus its codec overhead.
const LIMIT: usize = MAX_TERM + 8192;
/// Longest sort record of each dictionary order: the term text plus its 8-byte ID, and for the term order also the
/// zero separator before the ID.
const ID_RECORD_MAX: usize = MAX_TERM + 8;
const TERM_RECORD_MAX: usize = MAX_TERM + 9;
// The values the literals had before they were derived from MAX_TERM; a change here changes sort-record validation.
const _: () = assert!(LIMIT == 16 * 1024 * 1024 + 8192 && ID_RECORD_MAX == 16 * 1024 * 1024 + 8 && TERM_RECORD_MAX == 16 * 1024 * 1024 + 9);
fn bad(s: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, s)
}
#[derive(Clone, Copy)]
pub(super) enum Order {
    Term,
    Id,
    Unified,
}
impl Order {
    fn magic(self) -> &'static [u8; 8] {
        match self {
            Self::Term => TERM_MAGIC,
            Self::Id => ID_MAGIC,
            Self::Unified => UNIFIED_LEAN_MAGIC,
        }
    }
    /// Whether this order's current format uses lean entry framing. Tests can make a thread (and the build's workers)
    /// write the previous unified format, as an older binary would (`disk_index::write_older_format_on_this_thread`).
    fn lean(self) -> bool { matches!(self, Self::Unified) && !super::disk_index::index_format_of_this_thread() }
    /// The magic this process writes for the order.
    fn write_magic(self) -> &'static [u8; 8] { if matches!(self, Self::Unified) && !self.lean() { UNIFIED_MAGIC } else { self.magic() } }
    fn key(self, id: TermId, term: &Term) -> Vec<u8> {
        match self {
            Self::Term => term.to_string().into_bytes(),
            Self::Id | Self::Unified => id.0.to_be_bytes().to_vec(),
        }
    }
}
/// Kind tag of a dictionary ID (1 IRI, 2 blank node, 3 literal).
fn id_kind(id: u64) -> u64 {
    id >> 60
}
/// Kind implied by an N-Triples term text, by its first byte (an RDF 1.2 triple term by its `<<(` opening: no IRI
/// starts with `<<`, since `<` cannot occur inside an IRI).
fn text_kind(text: &[u8]) -> Option<u64> {
    match text.first()? {
        #[cfg(feature = "rdf-12")]
        b'<' if text.starts_with(b"<<(") => Some(id_tag::TRIPLE_TERM),
        b'<' => Some(id_tag::IRI),
        b'_' => Some(id_tag::BLANK_NODE),
        b'"' => Some(id_tag::LITERAL),
        _ => None,
    }
}
struct Meta {
    last: Vec<u8>,
    /// Unified order only: the block's last term text.
    last_text: Vec<u8>,
    offset: u64,
    length: usize,
    hash: [u8; 32],
}
/// One cached decoded block: its rows, its charged size, its position in `Cache::keys`, and a reference bit set by
/// hits and cleared by eviction sampling.
struct Entry {
    rows: Arc<Vec<(TermId, Term)>>,
    size: usize,
    pos: usize,
    referenced: bool,
}
/// One cache shard. Eviction is scan-resistant: a miss into a full shard is admitted only with probability
/// 1/`ADMIT_ONE_IN`, and room is made by sampling `EVICTION_SAMPLE` residents and evicting an unreferenced one
/// (clearing the reference bits it passes). FIFO or LRU miss on every access of a repeated pass larger than the
/// shard; this keeps roughly capacity/working-set of such a pass while a hit-protected hot set survives. The
/// generator has a fixed per-shard seed, so a single reader replays the same decisions.
///
/// A rejected miss is remembered in a bounded ghost set (keys only, no rows). A second miss on a ghost key is admitted
/// for certain (#105): a block a query touches twice, such as a repeated query's output terms after earlier queries
/// filled the shard, gets in on its second touch instead of one time in four. A one-shot pass still stays at one in
/// four, so it cannot flush the hot set.
struct Cache {
    entries: HashMap<(usize, usize), Entry>,
    keys: Vec<(usize, usize)>,
    used: usize,
    seed: u64,
    rng: u64,
    ghost: std::collections::HashSet<(usize, usize)>,
    ghost_order: std::collections::VecDeque<(usize, usize)>,
}
/// The ghost set holds at most this many keys per resident block (and at least `GHOST_MIN`).
const GHOST_PER_RESIDENT: usize = 2;
const GHOST_MIN: usize = 256;
const ADMIT_ONE_IN: u64 = 4;
const EVICTION_SAMPLE: usize = 8;
impl Cache {
    fn new(shard: usize) -> Self {
        let seed = (0x9e37_79b9_7f4a_7c15 ^ (shard as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9)) | 1;
        Self { entries: HashMap::new(), keys: Vec::new(), used: 0, seed, rng: seed, ghost: Default::default(), ghost_order: Default::default() }
    }
    /// xorshift64*: cheap, deterministic, and good enough for sampling and admission coin flips.
    fn next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn get(&mut self, key: (usize, usize)) -> Option<Arc<Vec<(TermId, Term)>>> {
        let entry = self.entries.get_mut(&key)?;
        entry.referenced = true;
        Some(Arc::clone(&entry.rows))
    }
    // An inconsistent cache is reported as an I/O error, as before B2, rather than panicking under the shard lock
    // (which would poison it and hide the cause behind "lock poisoned").
    fn remove_at(&mut self, pos: usize) -> io::Result<usize> {
        let key = self.keys.swap_remove(pos);
        if let Some(&moved) = self.keys.get(pos) {
            self.entries.get_mut(&moved).ok_or_else(|| bad("dictionary cache invariant"))?.pos = pos;
        }
        Ok(self.entries.remove(&key).ok_or_else(|| bad("dictionary cache invariant"))?.size)
    }
    /// Evicts one block chosen by reference-bit sampling; returns its charged size.
    fn evict_one(&mut self) -> io::Result<usize> {
        let mut victim = None;
        let mut last = 0;
        // A block whose bit this sweep cleared is not eligible again in the same sweep: one sweep costs a referenced
        // block its bit, not its place.
        let mut cleared = [usize::MAX; EVICTION_SAMPLE];
        for n in 0..EVICTION_SAMPLE {
            let pos = (self.next() % self.keys.len() as u64) as usize;
            last = pos;
            let entry = self.entries.get_mut(&self.keys[pos]).ok_or_else(|| bad("dictionary cache invariant"))?;
            if entry.referenced {
                entry.referenced = false;
                cleared[n] = pos;
            } else if victim.is_none() && !cleared.contains(&pos) {
                victim = Some(pos);
            }
        }
        self.remove_at(victim.unwrap_or(last))
    }
    /// Caches `rows` if admitted; returns the number of evicted blocks. Never exceeds `budget`.
    fn insert(&mut self, key: (usize, usize), rows: &Arc<Vec<(TermId, Term)>>, size: usize, budget: usize) -> io::Result<u64> {
        if size > budget || self.entries.contains_key(&key) {
            return Ok(0);
        }
        if self.used + size > budget && !self.ghost.remove(&key) && !self.next().is_multiple_of(ADMIT_ONE_IN) {
            // Rejected: remember the key, so its next miss is admitted.
            if self.ghost.insert(key) { self.ghost_order.push_back(key) }
            // FIFO: the oldest keys go first. An admission removes its key from `ghost` but leaves it in
            // `ghost_order`, so the queue is bounded by its own length (stale fronts are simply dropped).
            let cap = (self.keys.len() * GHOST_PER_RESIDENT).max(GHOST_MIN);
            while self.ghost.len() > cap || self.ghost_order.len() > 2 * cap {
                match self.ghost_order.pop_front() { Some(old) => { self.ghost.remove(&old); } None => break }
            }
            return Ok(0);
        }
        let mut evicted = 0;
        while self.used + size > budget {
            if self.keys.is_empty() { return Err(bad("dictionary cache invariant")); }
            self.used -= self.evict_one()?;
            evicted += 1;
        }
        self.used += size;
        self.entries.insert(key, Entry { rows: Arc::clone(rows), size, pos: self.keys.len(), referenced: false });
        self.keys.push(key);
        Ok(evicted)
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.keys.clear();
        self.used = 0;
        self.rng = self.seed;
        self.ghost.clear();
        self.ghost_order.clear();
    }
}
pub(super) struct DictionaryCache {
    shards: Vec<Mutex<Cache>>,
    shard_budget: usize,
}
impl DictionaryCache {
    pub(super) fn new() -> Arc<Self> { Self::with_shares(super::disk_index::DEFAULT_SHARES) }
    /// The dictionary's three of `shares` budget shares (see `DiskIndex::open_with_share`).
    pub(super) fn with_shares(shares: usize) -> Arc<Self> {
        Arc::new(Self {
            shard_budget: super::disk_index::cache_budget_mb().saturating_mul(1024 * 1024).saturating_mul(3) / shares.max(3) / 16,
            shards: (0..16).map(|i| Mutex::new(Cache::new(i))).collect(),
        })
    }
}
pub(super) struct LazyIndex {
    file: File,
    order: Order,
    entries: Vec<Meta>,
    identity: [u8; 32],
    cache: Arc<DictionaryCache>,
    /// Blocks use lean entry framing (`RDFDID03`).
    lean: bool,
}
fn exact(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    exact_traced(file, bytes, offset, None)
}
fn exact_traced(
    file: &File,
    mut bytes: &mut [u8],
    mut offset: u64,
    slot: Option<usize>,
) -> io::Result<()> {
    while !bytes.is_empty() {
        if let Some(slot) = slot {
            super::disk_index::trace_dictionary(slot, 2, 1);
        }
        match file.read_at(bytes, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "short dictionary read",
                ));
            }
            Ok(n) => {
                if let Some(slot) = slot {
                    super::disk_index::trace_dictionary(slot, 3, n as u64);
                }
                offset += n as u64;
                bytes = &mut bytes[n..];
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
impl LazyIndex {
    pub(super) fn open(path: &Path, order: Order) -> io::Result<Self> {
        Self::open_shared(path, order, DictionaryCache::new())
    }
    pub(super) fn open_shared(path: &Path, order: Order, cache: Arc<DictionaryCache>) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < 52 {
            return Err(bad("truncated dictionary index"));
        }
        let mut header = [0; 8];
        exact(&file, &mut header, 0)?;
        let lean = if &header == order.magic() {
            matches!(order, Order::Unified)
        } else if matches!(order, Order::Unified) && &header == UNIFIED_MAGIC {
            false
        } else {
            return Err(bad("bad dictionary index magic"));
        };
        let mut footer = [0; 40];
        exact(&file, &mut footer, len - 40)?;
        let dlen = u64::from_le_bytes(footer[..8].try_into().unwrap());
        let start = len
            .checked_sub(40)
            .and_then(|v| v.checked_sub(dlen))
            .filter(|v| *v >= 8)
            .ok_or_else(|| bad("invalid dictionary directory length"))?;
        let mut raw =
            vec![0; usize::try_from(dlen).map_err(|_| bad("dictionary directory too large"))?];
        exact(&file, &mut raw, start)?;
        let mut hash = Sha256::new();
        hash.update(header);
        hash.update(&raw);
        let identity: [u8; 32] = hash.finalize().into();
        if identity != footer[8..] {
            return Err(bad("dictionary directory checksum mismatch"));
        }
        if raw.len() < 4 {
            return Err(bad("truncated dictionary directory"));
        }
        let count = u32::from_le_bytes(raw[..4].try_into().unwrap()) as usize;
        if count > (raw.len() - 4) / 48 {
            return Err(bad("invalid dictionary block count"));
        }
        let mut pos = 4usize;
        let mut next = 8u64;
        let mut entries: Vec<Meta> = Vec::with_capacity(count);
        for _ in 0..count {
            let end = pos
                .checked_add(16)
                .filter(|v| *v <= raw.len())
                .ok_or_else(|| bad("truncated dictionary directory entry"))?;
            let offset = u64::from_le_bytes(raw[pos..pos + 8].try_into().unwrap());
            let length = u32::from_le_bytes(raw[pos + 8..pos + 12].try_into().unwrap()) as usize;
            let keylen = u32::from_le_bytes(raw[pos + 12..end].try_into().unwrap()) as usize;
            let keyend = end
                .checked_add(keylen)
                .filter(|v| *v <= raw.len())
                .ok_or_else(|| bad("truncated dictionary key"))?;
            // Unified order: each entry carries the block's last term text between the key and the checksum.
            let (last_text, textend) = if matches!(order, Order::Unified) {
                let lenend = keyend
                    .checked_add(4)
                    .filter(|v| *v <= raw.len())
                    .ok_or_else(|| bad("truncated dictionary text length"))?;
                let textlen = u32::from_le_bytes(raw[keyend..lenend].try_into().unwrap()) as usize;
                let textend = lenend
                    .checked_add(textlen)
                    .filter(|v| *v <= raw.len())
                    .ok_or_else(|| bad("truncated dictionary text"))?;
                if !(1..=LIMIT).contains(&textlen) {
                    return Err(bad("invalid dictionary block metadata"));
                }
                (raw[lenend..textend].to_vec(), textend)
            } else {
                (Vec::new(), keyend)
            };
            let hashend = textend
                .checked_add(32)
                .filter(|v| *v <= raw.len())
                .ok_or_else(|| bad("truncated dictionary checksum"))?;
            if offset != next
                || !(2..=LIMIT).contains(&length)
                || !(1..=LIMIT).contains(&keylen)
                || (matches!(order, Order::Id | Order::Unified) && keylen != 8)
            {
                return Err(bad("invalid dictionary block metadata"));
            }
            let last = raw[end..keyend].to_vec();
            if entries.last().is_some_and(|e| e.last >= last) {
                return Err(bad("unsorted dictionary directory"));
            }
            if matches!(order, Order::Unified) {
                let kind = id_kind(u64::from_be_bytes(last.as_slice().try_into().unwrap()));
                if text_kind(&last_text) != Some(kind) {
                    return Err(bad("dictionary block kind disagrees with its last term"));
                }
                // Inside one kind, block order is term-text order.
                if let Some(prev) = entries.last() {
                    let prev_kind = id_kind(u64::from_be_bytes(prev.last.as_slice().try_into().unwrap()));
                    if prev_kind == kind && prev.last_text >= last_text {
                        return Err(bad("unsorted dictionary directory"));
                    }
                }
            }
            next = next
                .checked_add(length as u64)
                .filter(|v| *v <= start)
                .ok_or_else(|| bad("dictionary block bounds"))?;
            entries.push(Meta {
                last,
                last_text,
                offset,
                length,
                hash: raw[textend..hashend].try_into().unwrap(),
            });
            pos = hashend;
        }
        if pos != raw.len() || next != start {
            return Err(bad("dictionary directory trailing bytes or gaps"));
        }
        Ok(Self {
            file,
            order,
            entries,
            identity,
            lean,
            cache,
        })
    }
    pub(super) fn identity(&self) -> [u8; 32] {
        self.identity
    }
    fn block(&self, index: usize) -> io::Result<Arc<Vec<(TermId, Term)>>> {
        self.block_traced(index, match self.order {
            Order::Term => 6,
            Order::Id | Order::Unified => 7,
        })
    }
    /// `trace_slot` only labels the counters (6 = looked up by term, 7 = by ID). Both lookup paths of a unified
    /// dictionary read the same blocks, so they share one cache entry.
    fn block_traced(&self, index: usize, trace_slot: usize) -> io::Result<Arc<Vec<(TermId, Term)>>> {
        let mut cache = self.cache.shards[index % self.cache.shards.len()]
            .lock()
            .map_err(|_| io::Error::other("dictionary cache lock poisoned"))?;
        let cache_slot = match self.order {
            Order::Term => 6,
            Order::Id | Order::Unified => 7,
        };
        if let Some(value) = cache.get((cache_slot, index)) {
            super::disk_index::trace_dictionary(trace_slot, 0, 1);
            return Ok(value);
        }
        super::disk_index::trace_dictionary(trace_slot, 1, 1);
        let meta = self
            .entries
            .get(index)
            .ok_or_else(|| bad("dictionary block absent"))?;
        let mut bytes = vec![0; meta.length];
        exact_traced(&self.file, &mut bytes, meta.offset, Some(trace_slot))?;
        if Sha256::digest(&bytes).as_slice() != meta.hash {
            return Err(bad("dictionary block checksum mismatch"));
        }
        let rows = term_block_codec::decode(&bytes, self.lean)?;
        let keys: Vec<_> = rows.iter().map(|(id, t)| self.order.key(*id, t)).collect();
        if keys.windows(2).any(|w| w[0] >= w[1])
            || keys.last() != Some(&meta.last)
            || (index > 0 && keys[0] <= self.entries[index - 1].last)
        {
            return Err(bad("invalid dictionary block order"));
        }
        if matches!(self.order, Order::Unified) {
            self.check_unified_block(index, meta, &rows)?;
        }
        let size = bytes
            .len()
            .saturating_add(keys.iter().map(Vec::len).sum::<usize>())
            .saturating_add(rows.len() * 128);
        let rows = Arc::new(rows);
        let evicted = cache.insert((cache_slot, index), &rows, size, self.cache.shard_budget)?;
        if evicted > 0 {
            super::disk_index::trace_dictionary(trace_slot, 4, evicted);
        }
        Ok(rows)
    }
    /// Cheap checks on every unified block load: one kind (by the term variant, no text rendering), the last term
    /// equal to the directory's last text, and continuation of the previous block of the same kind. The strictly
    /// increasing text order inside a block is checked exhaustively by `check_text_order` (used by `verify`), not on
    /// every load, so the ID lookup path does not render each term back to text.
    fn check_unified_block(&self, index: usize, meta: &Meta, rows: &[(TermId, Term)]) -> io::Result<()> {
        let kind = id_kind(rows[0].0 .0);
        for (id, term) in rows {
            let variant = match term {
                Term::NamedNode(_) => id_tag::IRI,
                Term::BlankNode(_) => id_tag::BLANK_NODE,
                Term::Literal(_) => id_tag::LITERAL,
                #[cfg(feature = "rdf-12")]
                Term::Triple(_) => id_tag::TRIPLE_TERM,
            };
            if id_kind(id.0) != kind || variant != kind {
                return Err(bad("dictionary block mixes kinds or a term disagrees with its kind"));
            }
        }
        if rows.last().map(|(_, t)| t.to_string().into_bytes()).as_deref() != Some(meta.last_text.as_slice()) {
            return Err(bad("invalid dictionary block order"));
        }
        if index > 0 {
            let prev = &self.entries[index - 1];
            if id_kind(u64::from_be_bytes(prev.last.as_slice().try_into().unwrap())) == kind
                && rows[0].1.to_string().as_bytes() <= prev.last_text.as_slice()
            {
                return Err(bad("invalid dictionary block order"));
            }
        }
        Ok(())
    }
    /// Unified order only: every term's text must be strictly greater than the previous term's of the same kind and
    /// start with that kind's byte. Together with the per-load checks this makes `find_term` exact.
    pub(super) fn check_text_order(&self) -> io::Result<u64> {
        if !matches!(self.order, Order::Unified) {
            return Err(bad("wrong dictionary index"));
        }
        let mut previous: Option<(u64, String)> = None;
        let mut count = 0u64;
        self.for_each(&mut |id, term| {
            count += 1;
            let text = term.to_string();
            let kind = id_kind(id.0);
            if text_kind(text.as_bytes()) != Some(kind) {
                return Err(bad("dictionary term disagrees with its kind"));
            }
            if let Some((pk, ptext)) = &previous
                && *pk == kind
                && *ptext >= text
            {
                return Err(bad("dictionary terms not strictly sorted inside a kind"));
            }
            previous = Some((kind, text));
            Ok(())
        })?;
        Ok(count)
    }
    pub(super) fn clear_cache(&self) -> io::Result<()> {
        for shard in &self.cache.shards {
            let mut cache = shard
                .lock()
                .map_err(|_| io::Error::other("dictionary cache lock poisoned"))?;
            cache.clear();
        }
        Ok(())
    }
    pub(super) fn for_each(
        &self,
        f: &mut dyn FnMut(TermId, &Term) -> io::Result<()>,
    ) -> io::Result<()> {
        for index in 0..self.entries.len() {
            let rows = self.block(index)?;
            for (id, term) in rows.iter() {
                f(*id, term)?;
            }
        }
        Ok(())
    }
    /// Unified order only: every entry of `kind`, in ID order. A kind's blocks are one contiguous run, so only those
    /// blocks are read.
    #[cfg(feature = "rdf-12")]
    pub(super) fn for_each_of_kind(&self, kind: u64, f: &mut dyn FnMut(TermId, &Term) -> io::Result<()>) -> io::Result<()> {
        let kind_of = |e: &Meta| id_kind(u64::from_be_bytes(e.last.as_slice().try_into().unwrap()));
        let lo = self.entries.partition_point(|e| kind_of(e) < kind);
        let hi = self.entries.partition_point(|e| kind_of(e) <= kind);
        for index in lo..hi {
            for (id, term) in self.block(index)?.iter() {
                f(*id, term)?;
            }
        }
        Ok(())
    }
    pub(super) fn find(&self, key: &[u8]) -> io::Result<Option<(TermId, Term)>> {
        let i = self.entries.partition_point(|e| e.last.as_slice() < key);
        if i == self.entries.len() {
            return Ok(None);
        }
        let rows = self.block(i)?;
        let at = rows.partition_point(|(id, t)| self.order.key(*id, t).as_slice() < key);
        Ok(rows
            .get(at)
            .filter(|(id, t)| self.order.key(*id, t) == key)
            .cloned())
    }
    /// Unified order only: look a term up by its N-Triples text. The first byte gives the kind, whose blocks are a
    /// contiguous run; inside it ID order is text order, so the per-block last text locates the one block to read.
    pub(super) fn find_term(&self, text: &[u8]) -> io::Result<Option<(TermId, Term)>> {
        if !matches!(self.order, Order::Unified) {
            return Err(bad("wrong dictionary index"));
        }
        let Some(kind) = text_kind(text) else { return Ok(None) };
        let kind_of = |e: &Meta| id_kind(u64::from_be_bytes(e.last.as_slice().try_into().unwrap()));
        let lo = self.entries.partition_point(|e| kind_of(e) < kind);
        let hi = self.entries.partition_point(|e| kind_of(e) <= kind);
        let i = lo + self.entries[lo..hi].partition_point(|e| e.last_text.as_slice() < text);
        if i == hi {
            return Ok(None);
        }
        let rows = self.block_traced(i, 6)?;
        let at = rows.partition_point(|(_, t)| t.to_string().as_bytes() < text);
        Ok(rows.get(at).filter(|(_, t)| t.to_string().as_bytes() == text).cloned())
    }
    /// Unified order only: the smallest ID of `kind` whose term text is `>= text` (`> text` if `strict`), or `None`
    /// when every term of that kind is smaller. Inside a kind, ID order is text order, so this is the same block
    /// search as `find_term`, returning the position instead of requiring an exact match.
    pub(super) fn text_lower_bound(&self, kind: u64, text: &[u8], strict: bool) -> io::Result<Option<TermId>> {
        if !matches!(self.order, Order::Unified) {
            return Err(bad("wrong dictionary index"));
        }
        let kind_of = |e: &Meta| id_kind(u64::from_be_bytes(e.last.as_slice().try_into().unwrap()));
        let lo = self.entries.partition_point(|e| kind_of(e) < kind);
        let hi = self.entries.partition_point(|e| kind_of(e) <= kind);
        let below = |t: &[u8]| if strict { t <= text } else { t < text };
        // The first block of the kind whose last text qualifies holds the answer: every earlier block is below.
        let i = lo + self.entries[lo..hi].partition_point(|e| below(e.last_text.as_slice()));
        if i == hi {
            return Ok(None);
        }
        let rows = self.block_traced(i, 6)?;
        let at = rows.partition_point(|(_, t)| below(t.to_string().as_bytes()));
        rows.get(at).map(|(id, _)| Some(*id)).ok_or_else(|| bad("invalid dictionary block order"))
    }
    /// Group requests by on-disk block. Callback positions are original input indices.
    pub(super) fn with_ids(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, triplelight_rdf::TermRef<'_>),
    ) -> io::Result<()> {
        if !matches!(self.order, Order::Id | Order::Unified) {
            return Err(bad("wrong dictionary index"));
        }
        let mut requests = Vec::with_capacity(ids.len());
        for (position, &id) in ids.iter().enumerate() {
            let key = id.0.to_be_bytes();
            let block = self
                .entries
                .partition_point(|e| e.last.as_slice() < key.as_slice());
            if block == self.entries.len() {
                return Err(bad("unknown term ID"));
            }
            requests.push((block, position, id));
        }
        requests.sort_unstable_by_key(|r| r.0);
        let mut start = 0;
        while start < requests.len() {
            let block_id = requests[start].0;
            let end = start + requests[start..].partition_point(|r| r.0 == block_id);
            let block = self.block(block_id)?;
            for &(_, position, id) in &requests[start..end] {
                let at = block.partition_point(|(candidate, _)| candidate.0 < id.0);
                let term = &block
                    .get(at)
                    .filter(|(candidate, _)| *candidate == id)
                    .ok_or_else(|| bad("unknown term ID"))?
                    .1;
                f(position, term.as_ref());
            }
            start = end;
        }
        Ok(())
    }
    pub(super) fn with_id(
        &self,
        id: TermId,
        f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>),
    ) -> io::Result<()> {
        if !matches!(self.order, Order::Id | Order::Unified) {
            return Err(bad("wrong dictionary index"));
        }
        let key = id.0.to_be_bytes();
        let i = self
            .entries
            .partition_point(|e| e.last.as_slice() < key.as_slice());
        if i == self.entries.len() {
            return Err(bad("unknown term ID"));
        }
        let rows = self.block(i)?;
        let at = rows.partition_point(|(v, _)| v.0 < id.0);
        let term = &rows
            .get(at)
            .filter(|(v, _)| *v == id)
            .ok_or_else(|| bad("unknown term ID"))?
            .1;
        f(term.as_ref());
        Ok(())
    }
}
/// Write a new ordered index. Does not fsync or publish a manifest; caller owns durability.
pub(super) fn write_index(
    path: &Path,
    order: Order,
    rows: impl Iterator<Item = io::Result<(TermId, Term)>>,
) -> io::Result<()> {
    write_index_checked(path, order, rows, true)
}
/// Directory entry of one written block.
struct BlockEntry {
    /// Order key of the block's last term (its ID for the ID orders, its text for the term order).
    key: Vec<u8>,
    /// Text of the block's last term; recorded for the unified order only.
    last_text: Vec<u8>,
    offset: u64,
    len: usize,
    hash: [u8; 32],
}
/// Accumulates one block of rows and appends it to the file when it is full.
struct BlockWriter<W: Write> {
    out: W,
    order: Order,
    block: Vec<(TermId, Term)>,
    entries: Vec<BlockEntry>,
    offset: u64,
}
impl<W: Write> BlockWriter<W> {
    fn flush(&mut self) -> io::Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let bytes = term_block_codec::encode(&self.block, self.order.lean())?;
        let (id, t) = self.block.last().unwrap();
        let key = self.order.key(*id, t);
        self.out.write_all(&bytes)?;
        let last_text = if matches!(self.order, Order::Unified) { t.to_string().into_bytes() } else { Vec::new() };
        self.entries.push(BlockEntry { key, last_text, offset: self.offset, len: bytes.len(), hash: Sha256::digest(&bytes).into() });
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| bad("dictionary size overflow"))?;
        self.block.clear();
        Ok(())
    }
}
/// `check_text` is false only in tests that need a unified file whose text order is broken.
fn write_index_checked(
    path: &Path,
    order: Order,
    rows: impl Iterator<Item = io::Result<(TermId, Term)>>,
    check_text: bool,
) -> io::Result<()> {
    let mut out =
        std::io::BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    let header = order.write_magic();
    out.write_all(header)?;
    let mut writer = BlockWriter { out, order, block: Vec::new(), entries: Vec::new(), offset: 8 };
    let mut previous_key: Option<Vec<u8>> = None;
    // Unified order only: the previous row's ID and text, to start a block at a kind change and check the text order.
    let mut previous_unified: Option<(TermId, Vec<u8>)> = None;
    let mut bytes_estimate = 0usize;
    for row in rows {
        let (id, t) = row?;
        let key = order.key(id, &t);
        if id.0 == 0 || previous_key.as_ref().is_some_and(|v| *v >= key) {
            return Err(bad("dictionary keys not unique and sorted"));
        }
        let text_len = if matches!(order, Order::Unified) {
            // A block holds one kind, and inside a kind the text is strictly increasing.
            let text = t.to_string().into_bytes();
            if text_kind(&text) != Some(id_kind(id.0)) {
                return Err(bad("dictionary term disagrees with its kind"));
            }
            if let Some((pid, ptext)) = previous_unified.as_ref() {
                if id_kind(pid.0) != id_kind(id.0) {
                    writer.flush()?;
                    bytes_estimate = 0;
                } else if check_text && *ptext >= text {
                    return Err(bad("dictionary keys not unique and sorted"));
                }
            }
            let len = text.len();
            previous_unified = Some((id, text));
            len
        } else {
            t.to_string().len()
        };
        let estimate = text_len.saturating_add(16);
        if !writer.block.is_empty()
            && (writer.block.len() == 256 || bytes_estimate.saturating_add(estimate) > LIMIT)
        {
            writer.flush()?;
            bytes_estimate = 0;
        }
        bytes_estimate = bytes_estimate.saturating_add(estimate);
        previous_key = Some(key);
        writer.block.push((id, t));
    }
    writer.flush()?;
    let BlockWriter { mut out, entries, .. } = writer;
    let mut dir = Vec::new();
    let count = u32::try_from(entries.len()).map_err(|_| bad("too many dictionary blocks"))?;
    dir.extend_from_slice(&count.to_le_bytes());
    for e in entries {
        dir.extend_from_slice(&e.offset.to_le_bytes());
        dir.extend_from_slice(&(e.len as u32).to_le_bytes());
        dir.extend_from_slice(&(e.key.len() as u32).to_le_bytes());
        dir.extend_from_slice(&e.key);
        if matches!(order, Order::Unified) {
            dir.extend_from_slice(&(e.last_text.len() as u32).to_le_bytes());
            dir.extend_from_slice(&e.last_text);
        }
        dir.extend_from_slice(&e.hash);
    }
    out.write_all(&dir)?;
    out.write_all(&(dir.len() as u64).to_le_bytes())?;
    let mut hash = Sha256::new();
    hash.update(header);
    hash.update(&dir);
    out.write_all(&hash.finalize())?;
    out.flush()?;
    out.get_ref().sync_all()
}

/// Test-only: writes a unified dictionary without checking the text order inside a kind.
#[cfg(test)]
pub(super) fn write_unified_without_text_check(path: &Path, rows: impl Iterator<Item = io::Result<(TermId, Term)>>) -> io::Result<()> {
    write_index_checked(path, Order::Unified, rows, false)
}
/// Sorted-run reader that decodes `(id, term)` rows from an external sorter's output.
struct Rows {
    input: std::io::BufReader<File>,
    order: Order,
    done: bool,
}
impl Iterator for Rows {
    type Item = io::Result<(TermId, Term)>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let value = match super::external_sort::read_record(&mut self.input, TERM_RECORD_MAX) {
            Ok(Some(v)) => v,
            Ok(None) => {
                self.done = true;
                return None;
            }
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        let parsed = (|| {
            let (id, text) = match self.order {
                Order::Id | Order::Unified => {
                    if value.len() < 8 {
                        return Err(bad("short ID sort record"));
                    }
                    (
                        u64::from_be_bytes(value[..8].try_into().unwrap()),
                        &value[8..],
                    )
                }
                Order::Term => {
                    if value.len() < 9 || value[value.len() - 9] != 0 {
                        return Err(bad("short term sort record"));
                    }
                    (
                        u64::from_be_bytes(value[value.len() - 8..].try_into().unwrap()),
                        &value[..value.len() - 9],
                    )
                }
            };
            let term = std::str::from_utf8(text)
                .map_err(|_| bad("invalid term UTF-8"))?
                .parse()
                .map_err(|_| bad("invalid RDF term"))?;
            Ok((TermId(id), term))
        })();
        if parsed.is_err() {
            self.done = true;
        }
        Some(parsed)
    }
}


/// Opens a legacy `RDFDIC01` dictionary (the external build's term file) and returns it with its record count.
fn open_legacy(path: &Path) -> io::Result<(std::io::BufReader<File>, u64)> {
    use std::io::Read;
    let mut input = std::io::BufReader::new(File::open(path)?);
    let mut header = [0u8; 8];
    input.read_exact(&mut header)?;
    if &header != b"RDFDIC01" {
        return Err(bad("invalid legacy dictionary"));
    }
    input.read_exact(&mut header)?;
    let count = u64::from_le_bytes(header);
    if count > 100_000_000 {
        return Err(bad("dictionary count too large"));
    }
    Ok((input, count))
}
/// One decoded legacy record: its ID, canonical text and term.
fn read_legacy_record(input: &mut std::io::BufReader<File>) -> io::Result<(u64, Vec<u8>, Term)> {
    use std::io::Read;
    let mut header = [0u8; 8];
    input.read_exact(&mut header)?;
    let id = u64::from_le_bytes(header);
    if id == 0 {
        return Err(bad("zero dictionary ID"));
    }
    input.read_exact(&mut header)?;
    let len = usize::try_from(u64::from_le_bytes(header)).map_err(|_| bad("dictionary term length"))?;
    if len > MAX_TERM {
        return Err(bad("dictionary term too long"));
    }
    let mut text = vec![0; len];
    input.read_exact(&mut text)?;
    if text.contains(&0) {
        return Err(bad("invalid dictionary text"));
    }
    let term: Term = std::str::from_utf8(&text)
        .map_err(|_| bad("invalid dictionary UTF-8"))?
        .parse()
        .map_err(|_| bad("invalid dictionary term"))?;
    if term.to_string().as_bytes() != text {
        return Err(bad("noncanonical dictionary term"));
    }
    Ok((id, text, term))
}
/// Unified conversion for a legacy dictionary already in strictly increasing term-text order (the external build
/// writes it that way). Term text starts with `"` (literal), `<` (IRI) or `_` (blank node), so each kind is one
/// contiguous run of the file. The unified file wants ID order, which is kind 1, 2, 3 and, inside a kind, the same
/// text order: the runs are read back in that order. No sort is involved; `write_index` rejects any term that
/// breaks the order or its kind.
pub(super) fn convert_unified(dir: &Path) -> io::Result<[u8; 32]> {
    use std::io::{BufReader, Read, Seek, SeekFrom};
    let legacy = dir.join("dictionary");
    let (mut input, count) = open_legacy(&legacy)?;
    // Pass 1: where each kind's run starts and how many records it holds; every kind must be one run.
    let mut runs: Vec<(u64, u64, u64)> = Vec::new(); // (kind, byte offset, records)
    let mut offset = 16u64;
    for _ in 0..count {
        let at = offset;
        let (id, text, _) = read_legacy_record(&mut input)?;
        offset += 16 + text.len() as u64;
        let kind = id_kind(id);
        if text_kind(&text) != Some(kind) {
            return Err(bad("dictionary term disagrees with its kind"));
        }
        match runs.last_mut() {
            Some(run) if run.0 == kind => run.2 += 1,
            _ => {
                if runs.iter().any(|r| r.0 == kind) {
                    return Err(bad("dictionary kinds are not contiguous"));
                }
                runs.push((kind, at, 1));
            }
        }
    }
    let mut extra = [0u8; 1];
    if input.read(&mut extra)? != 0 {
        return Err(bad("dictionary trailing bytes"));
    }
    runs.sort_by_key(|r| r.0);
    // Pass 2: the runs in kind (ID) order.
    struct Unified {
        input: BufReader<File>,
        runs: std::vec::IntoIter<(u64, u64, u64)>,
        left: u64,
        failed: bool,
    }
    impl Iterator for Unified {
        type Item = io::Result<(TermId, Term)>;
        fn next(&mut self) -> Option<Self::Item> {
            if self.failed {
                return None;
            }
            while self.left == 0 {
                let (_, at, n) = self.runs.next()?;
                if let Err(e) = self.input.seek(SeekFrom::Start(at)) {
                    self.failed = true;
                    return Some(Err(e));
                }
                self.left = n;
            }
            self.left -= 1;
            match read_legacy_record(&mut self.input) {
                Ok((id, _, term)) => Some(Ok((TermId(id), term))),
                Err(e) => {
                    self.failed = true;
                    Some(Err(e))
                }
            }
        }
    }
    let path = dir.join(UNIFIED_FILE);
    write_index(&path, Order::Unified, Unified { input, runs: runs.into_iter(), left: 0, failed: false })?;
    let identity = LazyIndex::open(&path, Order::Unified)?.identity();
    std::fs::remove_file(&legacy)?;
    Ok(identity)
}

/// Convert a freshly written legacy dictionary into two immutable indexes.
/// Caller owns manifest publication and scratch directory durability.
pub(super) fn convert_legacy(dir: &Path) -> io::Result<([u8; 32], [u8; 32])> {
    use super::external_sort::ExternalSort;
    use std::io::Read;
    let (mut input, count) = open_legacy(&dir.join("dictionary"))?;
    let mut by_term = ExternalSort::new(dir, 128 * 1024 * 1024, TERM_RECORD_MAX)?;
    let mut by_id = ExternalSort::new(dir, 128 * 1024 * 1024, ID_RECORD_MAX)?;
    for _ in 0..count {
        let (id, text, _) = read_legacy_record(&mut input)?;
        let mut term_key = Vec::with_capacity(text.len() + 9);
        term_key.extend_from_slice(&text);
        term_key.push(0);
        term_key.extend_from_slice(&id.to_be_bytes());
        by_term.push(term_key)?;
        let mut id_key = Vec::with_capacity(text.len() + 8);
        id_key.extend_from_slice(&id.to_be_bytes());
        id_key.extend_from_slice(&text);
        by_id.push(id_key)?;
    }
    let mut extra = [0u8; 1];
    if input.read(&mut extra)? != 0 {
        return Err(bad("dictionary trailing bytes"));
    }
    drop(input);
    let term_run = by_term.finish()?;
    let id_run = by_id.finish()?;
    let term_path = dir.join("dictionary.term");
    let id_path = dir.join("dictionary.id");
    write_index(
        &term_path,
        Order::Term,
        Rows {
            input: std::io::BufReader::new(File::open(&term_run)?),
            order: Order::Term,
            done: false,
        },
    )?;
    std::fs::remove_file(&term_run)?;
    write_index(
        &id_path,
        Order::Id,
        Rows {
            input: std::io::BufReader::new(File::open(&id_run)?),
            order: Order::Id,
            done: false,
        },
    )?;
    std::fs::remove_file(&id_run)?;
    let identities = (
        LazyIndex::open(&term_path, Order::Term)?.identity(),
        LazyIndex::open(&id_path, Order::Id)?.identity(),
    );
    drop(by_term);
    drop(by_id);
    std::fs::remove_file(dir.join("dictionary"))?;
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use triplelight_rdf::{Literal, NamedNode};
    fn legacy_dictionary(dir: &std::path::Path, entries: &[(u64, &str)]) {
        let mut bytes = b"RDFDIC01".to_vec();
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for (id, text) in entries {
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
            bytes.extend_from_slice(text.as_bytes());
        }
        std::fs::write(dir.join("dictionary"), bytes).unwrap();
    }

    /// Terms in the text order the external build writes: literals (`"`), IRIs (`<`), blank nodes (`_`), with IDs
    /// `(kind << 60) | rank`, the rank rising in that global text order.
    fn sample_terms() -> Vec<(u64, String)> {
        let mut texts: Vec<String> = (0..700).map(|i| format!("<urn:t{i:04}>")).collect();
        texts.extend((0..300).map(|i| format!("\"lit{i:04}\"")));
        texts.extend((0..40).map(|i| format!("_:b{i:02}")));
        // The same lexical value in its plain, language-tagged and typed forms, and an adjacent lang/typed pair.
        texts.extend(["\"a\"", "\"a\"@en", "\"a\"@en-us", "\"a\"^^<http://example.org/dt>", "\"a\"^^<http://www.w3.org/2001/XMLSchema#integer>"].map(String::from));
        texts.sort();
        texts.dedup();
        texts
            .into_iter()
            .enumerate()
            .map(|(n, t)| {
                let kind = match t.as_bytes()[0] {
                    b'<' => 1u64,
                    b'_' => 2,
                    _ => 3,
                };
                ((kind << 60) | n as u64, t)
            })
            .collect()
    }

    #[test]
    fn unified_dictionary_answers_both_lookups_like_the_two_file_pair() {
        let root = std::env::temp_dir().join(format!("rdf-unified-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("unified"), root.join("pair"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let rows = sample_terms();
        let entries: Vec<(u64, &str)> = rows.iter().map(|(i, t)| (*i, t.as_str())).collect();
        legacy_dictionary(&a, &entries);
        legacy_dictionary(&b, &entries);
        convert_unified(&a).unwrap();
        convert_legacy(&b).unwrap();
        assert!(!a.join("dictionary").exists(), "legacy file removed");
        assert_eq!(std::fs::read_dir(&a).unwrap().count(), 1, "only the unified file remains");
        let unified = LazyIndex::open(&a.join(UNIFIED_FILE), Order::Unified).unwrap();
        let by_term = LazyIndex::open(&b.join("dictionary.term"), Order::Term).unwrap();
        let by_id = LazyIndex::open(&b.join("dictionary.id"), Order::Id).unwrap();
        for (id, text) in &rows {
            let from_unified = unified.find_term(text.as_bytes()).unwrap().expect("present");
            let from_pair = by_term.find(text.as_bytes()).unwrap().expect("present");
            assert_eq!(from_unified, from_pair, "{text}");
            assert_eq!(from_unified.0, TermId(*id));
            let (mut u, mut p) = (None, None);
            unified.with_id(TermId(*id), &mut |t| u = Some(t.into_owned())).unwrap();
            by_id.with_id(TermId(*id), &mut |t| p = Some(t.into_owned())).unwrap();
            assert_eq!(u, p, "{text}");
        }
        // Batched lookups in a scrambled order.
        let ids: Vec<TermId> = rows.iter().rev().step_by(7).map(|(i, _)| TermId(*i)).collect();
        let (mut got, mut want) = (vec![None; ids.len()], vec![None; ids.len()]);
        unified.with_ids(&ids, &mut |i, t| got[i] = Some(t.into_owned())).unwrap();
        by_id.with_ids(&ids, &mut |i, t| want[i] = Some(t.into_owned())).unwrap();
        assert_eq!(got, want);
        // Absent terms: before everything, between neighbours, after the last of a kind, wrong kind, empty and odd text.
        for absent in ["\"", "\"a\"@fr", "\"a\"^^<http://example.org/other>", "\"zzz\"", "<urn:t0000", "<urn:t0000>x", "<urn:t9999>", "<", "_:b00x", "_:zz", "_:", "", "x", "\"a"] {
            assert_eq!(unified.find_term(absent.as_bytes()).unwrap(), by_term.find(absent.as_bytes()).unwrap(), "{absent:?}");
            assert!(unified.find_term(absent.as_bytes()).unwrap().is_none(), "{absent:?}");
        }
        assert!(unified.with_id(TermId((1 << 60) | 99_999), &mut |_| ()).is_err());
        // IDs outside every dictionary run, such as the inline kind (15), are errors in both layouts: inline IDs never
        // reach the disk dictionary, they are resolved before it is consulted.
        for odd in [(15u64 << 60) | 5, 7, u64::MAX] {
            assert!(unified.with_id(TermId(odd), &mut |_| ()).is_err(), "{odd:#x}");
            assert!(by_id.with_id(TermId(odd), &mut |_| ()).is_err(), "{odd:#x}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unified_dictionary_with_a_missing_kind_still_finds_the_others() {
        let dir = std::env::temp_dir().join(format!("rdf-unified-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No blank nodes at all, and the literal run is a single block.
        legacy_dictionary(&dir, &[((3 << 60) | 1, "\"x\""), ((1 << 60) | 2, "<urn:a>"), ((1 << 60) | 3, "<urn:b>")]);
        convert_unified(&dir).unwrap();
        let u = LazyIndex::open(&dir.join(UNIFIED_FILE), Order::Unified).unwrap();
        assert_eq!(u.find_term(b"<urn:b>").unwrap().unwrap().0, TermId((1 << 60) | 3));
        assert_eq!(u.find_term(b"\"x\"").unwrap().unwrap().0, TermId((3 << 60) | 1));
        assert!(u.find_term(b"_:b1").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unified_conversion_rejects_disordered_or_mislabelled_input() {
        let root = std::env::temp_dir().join(format!("rdf-unified-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, entries) in [
            // Text out of order inside a kind.
            ("unsorted", vec![((1u64 << 60) | 1, "<urn:b>"), ((1 << 60) | 2, "<urn:a>")]),
            // IRI text carrying a literal kind tag.
            ("mislabelled", vec![((3u64 << 60) | 1, "<urn:a>")]),
            // A kind split into two runs.
            ("split", vec![((1u64 << 60) | 1, "<urn:a>"), ((2 << 60) | 2, "_:b"), ((1 << 60) | 3, "<urn:c>")]),
        ] {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            legacy_dictionary(&dir, &entries);
            assert!(convert_unified(&dir).is_err(), "{name} must be rejected");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broken_text_order_fails_verify_but_id_lookups_still_work() {
        let dir = std::env::temp_dir().join(format!("rdf-unified-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Valid IDs, kinds and last text, but the text order inside the block is broken.
        let rows: Vec<io::Result<(TermId, Term)>> = [(1u64, "urn:c"), (2, "urn:a"), (3, "urn:b")]
            .iter()
            .map(|(n, iri)| Ok((TermId((1 << 60) | n), triplelight_rdf::NamedNode::new(*iri).unwrap().into())))
            .collect();
        let path = dir.join(UNIFIED_FILE);
        write_index_checked(&path, Order::Unified, rows.into_iter(), false).unwrap();
        let u = LazyIndex::open(&path, Order::Unified).unwrap();
        // The cheap load-time guard (kind, IDs, last text) holds, so ID lookups still answer.
        let mut seen = None;
        u.with_id(TermId((1 << 60) | 2), &mut |t| seen = Some(t.to_string())).unwrap();
        assert_eq!(seen.as_deref(), Some("<urn:a>"));
        // The exhaustive check, used by `verify`, rejects it.
        assert!(u.check_text_order().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unified_dictionary_reports_a_corrupt_directory_or_block() {
        let dir = std::env::temp_dir().join(format!("rdf-unified-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rows = sample_terms();
        let entries: Vec<(u64, &str)> = rows.iter().map(|(i, t)| (*i, t.as_str())).collect();
        legacy_dictionary(&dir, &entries);
        convert_unified(&dir).unwrap();
        let path = dir.join(UNIFIED_FILE);
        let good = std::fs::read(&path).unwrap();
        // A flipped byte inside the first block: the directory still opens, the block read fails.
        let mut bad_block = good.clone();
        bad_block[20] ^= 0xff;
        std::fs::write(&path, &bad_block).unwrap();
        let u = LazyIndex::open(&path, Order::Unified).unwrap();
        assert!(u.find_term(rows.iter().find(|(_, t)| t.starts_with('<')).unwrap().1.as_bytes()).is_err() || u.for_each(&mut |_, _| Ok(())).is_err());
        // A flipped byte in the directory fails at open; truncation fails at open.
        let mut bad_dir = good.clone();
        let n = bad_dir.len();
        bad_dir[n - 60] ^= 0xff;
        std::fs::write(&path, &bad_dir).unwrap();
        assert!(LazyIndex::open(&path, Order::Unified).is_err());
        std::fs::write(&path, &good[..good.len() / 2]).unwrap();
        assert!(LazyIndex::open(&path, Order::Unified).is_err());
        // The old two-file readers refuse a unified file and vice versa.
        std::fs::write(&path, &good).unwrap();
        assert!(LazyIndex::open(&path, Order::Id).is_err());
        assert!(LazyIndex::open(&path, Order::Term).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Dictionary lookup A/B at the layer the Arrow decode uses (`with_ids` over result-sized batches, plus term
    /// lookups): a two-file store (`TL_BENCH_OLD`) against a unified one (`TL_BENCH_NEW`) built from the same data.
    /// `TL_BENCH_OLD=... TL_BENCH_NEW=... cargo test --release -p triplelight-storage dictionary_lookup_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dictionary_lookup_bench_two_file_vs_unified() {
        use std::time::Instant;
        let old_dir = std::path::PathBuf::from(std::env::var("TL_BENCH_OLD").expect("TL_BENCH_OLD"));
        let new_dir = std::path::PathBuf::from(std::env::var("TL_BENCH_NEW").expect("TL_BENCH_NEW"));
        let old_id = LazyIndex::open(&old_dir.join("dictionary.id"), Order::Id).unwrap();
        let old_term = LazyIndex::open(&old_dir.join("dictionary.term"), Order::Term).unwrap();
        let unified = LazyIndex::open(&new_dir.join(UNIFIED_FILE), Order::Unified).unwrap();
        // A deterministic sample of IDs and their texts, in scrambled order like a join result.
        let mut ids = Vec::new();
        let mut n = 0u64;
        old_id.for_each(&mut |id, _| { n += 1; if n.is_multiple_of(13) { ids.push(id); } Ok(()) }).unwrap();
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        for i in (1..ids.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            ids.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let mut texts = vec![String::new(); ids.len()];
        old_id.with_ids(&ids, &mut |i, t| texts[i] = t.to_string()).unwrap();
        // Same answers first.
        let mut a = vec![None; ids.len()];
        let mut b = vec![None; ids.len()];
        old_id.with_ids(&ids, &mut |i, t| a[i] = Some(t.into_owned())).unwrap();
        unified.with_ids(&ids, &mut |i, t| b[i] = Some(t.into_owned())).unwrap();
        assert_eq!(a, b);
        for text in texts.iter().step_by(97) {
            assert_eq!(old_term.find(text.as_bytes()).unwrap(), unified.find_term(text.as_bytes()).unwrap());
        }
        let by_id = |ix: &LazyIndex, cold: bool| -> f64 {
            if cold { ix.clear_cache().unwrap(); }
            let t = Instant::now();
            for chunk in ids.chunks(4096) { ix.with_ids(chunk, &mut |_, t| { std::hint::black_box(t); }).unwrap(); }
            t.elapsed().as_nanos() as f64 / ids.len() as f64
        };
        let by_text = |find: &dyn Fn(&[u8]), cold: Option<&LazyIndex>| -> f64 {
            if let Some(ix) = cold { ix.clear_cache().unwrap(); }
            let t = Instant::now();
            for text in &texts { find(text.as_bytes()); }
            t.elapsed().as_nanos() as f64 / texts.len() as f64
        };
        // Blocks read and bytes read per cold by-ID pass (needs TRIPLELIGHT_TRACE_BLOCK_CACHE=1); slot 7 serves both.
        for (name, ix) in [("two-file", &old_id), ("unified", &unified)] {
            ix.clear_cache().unwrap();
            let before = super::super::disk_index::cache_trace_snapshot()[7];
            for chunk in ids.chunks(4096) { ix.with_ids(chunk, &mut |_, t| { std::hint::black_box(t); }).unwrap(); }
            let after = super::super::disk_index::cache_trace_snapshot()[7];
            eprintln!("cold by-ID pass, {name}: block misses {}, bytes read {} (trace slot 7)", after[1] - before[1], after[3] - before[3]);
        }
        let mut rows: Vec<(&str, Vec<f64>, Vec<f64>)> = vec![("by ID cold", vec![], vec![]), ("by ID warm", vec![], vec![]), ("by term cold", vec![], vec![]), ("by term warm", vec![], vec![])];
        for round in 0..5 {
            for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let (c1, w1, c2, w2) = if variant == 0 {
                    (by_id(&old_id, true), by_id(&old_id, false),
                     by_text(&|k| { std::hint::black_box(old_term.find(k).unwrap()); }, Some(&old_term)),
                     by_text(&|k| { std::hint::black_box(old_term.find(k).unwrap()); }, None))
                } else {
                    (by_id(&unified, true), by_id(&unified, false),
                     by_text(&|k| { std::hint::black_box(unified.find_term(k).unwrap()); }, Some(&unified)),
                     by_text(&|k| { std::hint::black_box(unified.find_term(k).unwrap()); }, None))
                };
                for (r, v) in rows.iter_mut().zip([c1, w1, c2, w2]) { if variant == 0 { r.1.push(v) } else { r.2.push(v) } }
            }
        }
        for (name, mut o, mut n) in rows {
            o.sort_by(f64::total_cmp);
            n.sort_by(f64::total_cmp);
            eprintln!("{name}: ns per lookup, median of 5 interleaved: two-file {:.0}, unified {:.0}, ratio {:.3}", o[2], n[2], n[2] / o[2]);
        }
    }

    #[test]
    fn dual_indexes_roundtrip_cross_blocks_and_report_corruption() {
        let dir = std::env::temp_dir().join(format!("rdf-lazy-dict-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut entries: Vec<(TermId, Term)> = (0..600)
            .map(|i| {
                (
                    TermId(1 + i),
                    NamedNode::new(format!("urn:t{i:04}")).unwrap().into(),
                )
            })
            .collect();
        entries.push((
            TermId(u64::MAX),
            Literal::new_simple_literal("short").into(),
        ));
        let id_path = dir.join("id");
        let term_path = dir.join("term");
        entries.sort_unstable_by_key(|(id, _)| id.0);
        write_index(&id_path, Order::Id, entries.clone().into_iter().map(Ok)).unwrap();
        entries.sort_unstable_by_key(|(_, t)| t.to_string());
        write_index(&term_path, Order::Term, entries.clone().into_iter().map(Ok)).unwrap();
        let shared = DictionaryCache::new();
        let ids = LazyIndex::open_shared(&id_path, Order::Id, Arc::clone(&shared)).unwrap();
        let terms = LazyIndex::open_shared(&term_path, Order::Term, shared).unwrap();
        for (id, term) in &entries {
            assert_eq!(
                terms.find(term.to_string().as_bytes()).unwrap(),
                Some((*id, term.clone()))
            );
            ids.with_id(*id, &mut |t| assert_eq!(t.into_owned(), *term))
                .unwrap();
        }
        let batch = [TermId(u64::MAX), TermId(1), TermId(599), TermId(1)];
        let mut found = vec![None; batch.len()];
        ids.with_ids(&batch, &mut |i, t| found[i] = Some(t.into_owned()))
            .unwrap();
        for (position, id) in batch.iter().enumerate() {
            assert_eq!(
                found[position],
                entries
                    .iter()
                    .find(|(candidate, _)| candidate == id)
                    .map(|(_, t)| t.clone())
            );
        }
        let mut called = false;
        assert!(ids.with_id(TermId(0), &mut |_| called = true).is_err());
        assert!(!called);
        assert!(terms.find(b"<urn:missing>").unwrap().is_none());
        drop(ids);
        drop(terms);
        let mut bytes = std::fs::read(&term_path).unwrap();
        bytes[8] ^= 1;
        std::fs::write(&term_path, &bytes).unwrap();
        let terms = LazyIndex::open(&term_path, Order::Term).unwrap();
        assert!(terms.find(entries[0].1.to_string().as_bytes()).is_err());
        drop(terms);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod admission_cache_tests {
    use super::*;
    fn rows() -> Arc<Vec<(TermId, Term)>> { Arc::new(Vec::new()) }

    #[test]
    fn budget_is_never_exceeded_and_full_shards_admit_about_one_in_four() {
        let mut c = Cache::new(3);
        let r = rows();
        for i in 0..10 { c.insert((7, i), &r, 10, 100).unwrap(); }
        assert_eq!((c.used, c.keys.len()), (100, 10), "a shard with room admits every miss");
        let mut admitted = 0;
        for i in 10..4010 {
            let before = c.entries.contains_key(&(7, i));
            assert!(!before);
            c.insert((7, i), &r, 10, 100).unwrap();
            admitted += u32::from(c.entries.contains_key(&(7, i)));
            assert!(c.used <= 100 && c.keys.len() == c.entries.len());
            for (pos, key) in c.keys.iter().enumerate() { assert_eq!(c.entries[key].pos, pos) }
        }
        assert!((850..=1150).contains(&admitted), "admitted {admitted} of 4000, expected about 1000");
        assert_eq!(c.insert((7, 99_999), &r, 101, 100).unwrap(), 0, "a block larger than the shard is never cached");
    }

    #[test]
    fn a_second_miss_on_a_rejected_block_is_admitted_and_one_shot_passes_stay_at_one_in_four() {
        let mut c = Cache::new(5);
        let r = rows();
        for i in 0..10 { c.insert((7, i), &r, 10, 100).unwrap(); }
        // A repeated set of 40 blocks into a full shard: every block is resident after its second miss at the latest.
        for _ in 0..2 { for i in 100..140 { c.insert((7, i), &r, 10, 100).unwrap(); } }
        let resident = (100..140).filter(|i| c.entries.contains_key(&(7, *i))).count();
        assert!(resident >= 10 - 1, "the shard holds 10 blocks and they are now the repeated ones: {resident}");
        // Single misses on fresh keys are still admitted about one time in four.
        let mut admitted = 0;
        for i in 1000..5000 { c.insert((7, i), &r, 10, 100).unwrap(); admitted += u32::from(c.entries.contains_key(&(7, i))); }
        assert!((850..=1150).contains(&admitted), "admitted {admitted} of 4000");
        assert!(c.ghost.len() <= GHOST_MIN.max(c.keys.len() * GHOST_PER_RESIDENT));
        assert!(c.ghost_order.len() <= 4 * GHOST_MIN, "ghost queue stays bounded: {}", c.ghost_order.len());
        c.clear();
        assert!(c.ghost.is_empty() && c.ghost_order.is_empty());
    }

    #[test]
    fn a_referenced_block_survives_an_eviction_sample_sweep() {
        let mut c = Cache::new(0);
        let r = rows();
        for i in 0..8 { c.insert((7, i), &r, 10, 80).unwrap(); }
        assert!(c.get((7, 0)).is_some());
        // Every resident except block 0 is unreferenced, so one eviction never picks block 0 unless all eight
        // samples land on it, in which case its bit is cleared and it is the fallback victim.
        let mut survived = 0;
        for trial in 0..200 {
            let mut d = Cache::new(trial);
            for i in 0..8 { d.insert((7, i), &r, 10, 80).unwrap(); }
            d.get((7, 0));
            d.used -= d.evict_one().unwrap();
            survived += u32::from(d.entries.contains_key(&(7, 0)));
        }
        assert!(survived >= 199, "referenced block evicted in {} of 200 sweeps", 200 - survived);
    }

    #[test]
    fn clear_resets_contents_and_the_generator() {
        let mut c = Cache::new(5);
        let r = rows();
        for i in 0..50 { c.insert((6, i), &r, 10, 100).unwrap(); }
        let first: Vec<_> = (0..20).map(|_| c.next()).collect();
        c.clear();
        assert!(c.entries.is_empty() && c.keys.is_empty() && c.used == 0);
        let again: Vec<_> = (0..20).map(|_| c.next()).collect();
        assert_ne!(first, again, "the generator advanced before the first sample");
        c.clear();
        let mut fresh = Cache::new(5);
        assert_eq!((0..20).map(|_| c.next()).collect::<Vec<_>>(), (0..20).map(|_| fresh.next()).collect::<Vec<_>>());
    }
}

/// B2 hit-path microbenchmark (ignored; run with
/// `cargo test --release -p triplelight-storage b2_hit_path -- --ignored --nocapture`). Same process, same keys, same
/// resident set, interleaved old/new rounds: the pre-B2 FIFO shard (copied verbatim from main dda13fb5) against the
/// reference-bit shard, timing only `get` on blocks that are all resident (the all-hit case).
#[cfg(test)]
mod b2_hit_bench {
    use super::*;
    use std::collections::VecDeque;
    struct OldCache {
        entries: HashMap<(usize, usize), (Arc<Vec<(TermId, Term)>>, usize)>,
        order: VecDeque<(usize, usize)>,
        used: usize,
    }
    impl OldCache {
        fn get(&self, key: (usize, usize)) -> Option<Arc<Vec<(TermId, Term)>>> {
            self.entries.get(&key).map(|(v, _)| Arc::clone(v))
        }
    }
    #[test]
    #[ignore]
    fn b2_hit_path() {
        const BLOCKS: usize = 3_400; // ~54,231 Q9 blocks / 16 shards
        const LOOKUPS: usize = 2_000_000;
        let rows = Arc::new(Vec::new());
        let mut old = OldCache { entries: HashMap::new(), order: VecDeque::new(), used: 0 };
        let mut new = Cache::new(7);
        for i in 0..BLOCKS {
            old.entries.insert((7, i), (Arc::clone(&rows), 10));
            old.order.push_back((7, i));
            old.used += 10;
            new.insert((7, i), &rows, 10, usize::MAX).unwrap();
        }
        // Fixed pseudo-random key sequence shared by both.
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let keys: Vec<(usize, usize)> = (0..LOOKUPS).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (7, (x % BLOCKS as u64) as usize) }).collect();
        let mut times = (Vec::new(), Vec::new());
        let mut sink = 0usize;
        for round in 0..20 {
            for which in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let t = std::time::Instant::now();
                for &k in &keys {
                    let v = if which == 0 { old.get(k) } else { new.get(k) };
                    sink = sink.wrapping_add(v.map_or(0, |r| Arc::strong_count(&r)));
                }
                let ns = t.elapsed().as_nanos() as f64 / LOOKUPS as f64;
                if which == 0 { times.0.push(ns) } else { times.1.push(ns) }
            }
        }
        let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
        let (o, n) = (med(&mut times.0), med(&mut times.1));
        println!("b2 hit path: old FIFO get {o:.2} ns/lookup, new ref-bit get {n:.2} ns/lookup, delta {:+.2} ns ({:+.1}%), sink {sink}", n - o, 100.0 * (n - o) / o);
    }

    /// Gate D item 3 (Kaiming 33733518): the miss path into a full shard. Old: FIFO pop_front + remove + insert (the
    /// pre-B2 code path). New: `Cache::insert` with admission coin and 8-sample reference-bit sweep. Every lookup is a
    /// miss on a key not resident; both shards are full at the same capacity. Reports ns per miss (cache bookkeeping
    /// only; the decode a miss also pays is excluded and measured separately in a38).
    #[test]
    #[ignore]
    fn b2_miss_path() {
        const CAP: usize = 3_400;
        const MISSES: usize = 1_000_000;
        let rows = Arc::new(Vec::new());
        let budget = CAP * 10;
        let mut old = OldCache { entries: HashMap::new(), order: VecDeque::new(), used: 0 };
        let mut new = Cache::new(7);
        for i in 0..CAP {
            old.entries.insert((7, i), (Arc::clone(&rows), 10)); old.order.push_back((7, i)); old.used += 10;
            new.insert((7, i), &rows, 10, budget).unwrap();
        }
        // Half the residents referenced, so the sweep does real bit clearing.
        for i in (0..CAP).step_by(2) { new.get((7, i)); }
        let mut times = (Vec::new(), Vec::new());
        let mut next_key = CAP;
        let mut admitted_r0 = 0u64;
        for round in 0..10 {
            for which in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let t = std::time::Instant::now();
                for _ in 0..MISSES {
                    let key = (7, next_key); next_key += 1;
                    if which == 0 {
                        // Verbatim pre-B2 insertion: evict FIFO until it fits, then insert.
                        while old.used + 10 > budget {
                            let k = old.order.pop_front().unwrap();
                            if let Some((_, n)) = old.entries.remove(&k) { old.used -= n; }
                        }
                        old.used += 10; old.entries.insert(key, (Arc::clone(&rows), 10)); old.order.push_back(key);
                    } else {
                        new.insert(key, &rows, 10, budget).unwrap();
                        if which == 1 && round == 0 && new.entries.contains_key(&key) { admitted_r0 += 1; }
                    }
                }
                let ns = t.elapsed().as_nanos() as f64 / MISSES as f64;
                if which == 0 { times.0.push(ns) } else { times.1.push(ns) }
            }
        }
        let med = |v: &mut Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
        let (o, n) = (med(&mut times.0), med(&mut times.1));
        // Equal block sizes: each admission into a full shard evicts exactly one block; old samples nothing, new
        // samples EVICTION_SAMPLE residents per eviction. Old admits every miss; new about 1 in ADMIT_ONE_IN.
        let new_admit_frac = admitted_r0 as f64 / MISSES as f64;
        let per_admission_new = if new_admit_frac > 0.0 { n / new_admit_frac } else { f64::NAN };
        println!("b2 miss path (full shard, {MISSES} misses/round, 10 interleaved rounds): old FIFO {o:.1} ns/miss = {o:.1} ns/admission (admits 1.000, sweep steps 0); new {n:.1} ns/miss, admits {new_admit_frac:.3} -> {per_admission_new:.1} ns/admission, sweep steps {} per admission; added cost per miss {:+.1} ns = {:.4}% of ~180,000 ns decode per miss (a38 scope: conversion-window decode_ms / slot-7 misses, Q9 and Q14 at 1x); frozen ceiling 9,000 ns/miss", EVICTION_SAMPLE, n - o, 100.0 * (n - o).max(0.0) / 180_000.0);
    }
}
