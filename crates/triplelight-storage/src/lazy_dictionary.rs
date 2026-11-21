//! Prototype immutable on-disk dictionary indexes. Not yet connected to snapshots.
use super::term_block_codec;
use crate::model::TermId;
use oxrdf::Term;
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
const LIMIT: usize = 16 * 1024 * 1024 + 8192;
fn bad(s: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, s)
}
#[derive(Clone, Copy)]
pub(super) enum Order {
    Term,
    Id,
}
impl Order {
    fn magic(self) -> &'static [u8; 8] {
        match self {
            Self::Term => TERM_MAGIC,
            Self::Id => ID_MAGIC,
        }
    }
    fn key(self, id: TermId, term: &Term) -> Vec<u8> {
        match self {
            Self::Term => term.to_string().into_bytes(),
            Self::Id => id.0.to_be_bytes().to_vec(),
        }
    }
}
struct Meta {
    last: Vec<u8>,
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
struct Cache {
    entries: HashMap<(usize, usize), Entry>,
    keys: Vec<(usize, usize)>,
    used: usize,
    seed: u64,
    rng: u64,
}
const ADMIT_ONE_IN: u64 = 4;
const EVICTION_SAMPLE: usize = 8;
impl Cache {
    fn new(shard: usize) -> Self {
        let seed = (0x9e37_79b9_7f4a_7c15 ^ (shard as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9)) | 1;
        Self { entries: HashMap::new(), keys: Vec::new(), used: 0, seed, rng: seed }
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
    fn remove_at(&mut self, pos: usize) -> usize {
        let key = self.keys.swap_remove(pos);
        if let Some(&moved) = self.keys.get(pos) {
            self.entries.get_mut(&moved).expect("cache key without entry").pos = pos;
        }
        self.entries.remove(&key).expect("cache key without entry").size
    }
    /// Evicts one block chosen by reference-bit sampling; returns its charged size.
    fn evict_one(&mut self) -> usize {
        let mut victim = None;
        let mut last = 0;
        // A block whose bit this sweep cleared is not eligible again in the same sweep: one sweep costs a referenced
        // block its bit, not its place.
        let mut cleared = [usize::MAX; EVICTION_SAMPLE];
        for n in 0..EVICTION_SAMPLE {
            let pos = (self.next() % self.keys.len() as u64) as usize;
            last = pos;
            let entry = self.entries.get_mut(&self.keys[pos]).expect("cache key without entry");
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
    fn insert(&mut self, key: (usize, usize), rows: &Arc<Vec<(TermId, Term)>>, size: usize, budget: usize) -> u64 {
        if size > budget || self.entries.contains_key(&key) {
            return 0;
        }
        if self.used + size > budget && !self.next().is_multiple_of(ADMIT_ONE_IN) {
            return 0;
        }
        let mut evicted = 0;
        while self.used + size > budget {
            self.used -= self.evict_one();
            evicted += 1;
        }
        self.used += size;
        self.entries.insert(key, Entry { rows: Arc::clone(rows), size, pos: self.keys.len(), referenced: false });
        self.keys.push(key);
        evicted
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.keys.clear();
        self.used = 0;
        self.rng = self.seed;
    }
}
pub(super) struct DictionaryCache {
    shards: Vec<Mutex<Cache>>,
    shard_budget: usize,
}
impl DictionaryCache {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            shard_budget: super::disk_index::cache_budget_mb().saturating_mul(1024 * 1024) / 3 / 16,
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
        if &header != order.magic() {
            return Err(bad("bad dictionary index magic"));
        }
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
            let hashend = keyend
                .checked_add(32)
                .filter(|v| *v <= raw.len())
                .ok_or_else(|| bad("truncated dictionary checksum"))?;
            if offset != next
                || !(2..=LIMIT).contains(&length)
                || !(1..=LIMIT).contains(&keylen)
                || (matches!(order, Order::Id) && keylen != 8)
            {
                return Err(bad("invalid dictionary block metadata"));
            }
            let last = raw[end..keyend].to_vec();
            if entries.last().is_some_and(|e| e.last >= last) {
                return Err(bad("unsorted dictionary directory"));
            }
            next = next
                .checked_add(length as u64)
                .filter(|v| *v <= start)
                .ok_or_else(|| bad("dictionary block bounds"))?;
            entries.push(Meta {
                last,
                offset,
                length,
                hash: raw[keyend..hashend].try_into().unwrap(),
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
            cache,
        })
    }
    pub(super) fn identity(&self) -> [u8; 32] {
        self.identity
    }
    fn block(&self, index: usize) -> io::Result<Arc<Vec<(TermId, Term)>>> {
        let mut cache = self.cache.shards[index % self.cache.shards.len()]
            .lock()
            .map_err(|_| io::Error::other("dictionary cache lock poisoned"))?;
        let slot = match self.order {
            Order::Term => 6,
            Order::Id => 7,
        };
        if let Some(value) = cache.get((slot, index)) {
            super::disk_index::trace_dictionary(slot, 0, 1);
            return Ok(value);
        }
        super::disk_index::trace_dictionary(slot, 1, 1);
        let meta = self
            .entries
            .get(index)
            .ok_or_else(|| bad("dictionary block absent"))?;
        let mut bytes = vec![0; meta.length];
        exact_traced(&self.file, &mut bytes, meta.offset, Some(slot))?;
        if Sha256::digest(&bytes).as_slice() != meta.hash {
            return Err(bad("dictionary block checksum mismatch"));
        }
        let rows = term_block_codec::decode(&bytes)?;
        let keys: Vec<_> = rows.iter().map(|(id, t)| self.order.key(*id, t)).collect();
        if keys.windows(2).any(|w| w[0] >= w[1])
            || keys.last() != Some(&meta.last)
            || (index > 0 && keys[0] <= self.entries[index - 1].last)
        {
            return Err(bad("invalid dictionary block order"));
        }
        let size = bytes
            .len()
            .saturating_add(keys.iter().map(Vec::len).sum::<usize>())
            .saturating_add(rows.len() * 128);
        let rows = Arc::new(rows);
        let evicted = cache.insert((slot, index), &rows, size, self.cache.shard_budget);
        if evicted > 0 {
            super::disk_index::trace_dictionary(slot, 4, evicted);
        }
        Ok(rows)
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
    /// Group requests by on-disk block. Callback positions are original input indices.
    pub(super) fn with_ids(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, oxrdf::TermRef<'_>),
    ) -> io::Result<()> {
        if !matches!(self.order, Order::Id) {
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
        f: &mut dyn FnMut(oxrdf::TermRef<'_>),
    ) -> io::Result<()> {
        if !matches!(self.order, Order::Id) {
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
    let mut out =
        std::io::BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    let header = order.magic();
    out.write_all(header)?;
    let mut meta = Vec::new();
    let mut block = Vec::new();
    let mut previous: Option<Vec<u8>> = None;
    let mut offset = 8u64;
    let mut bytes_estimate = 0usize;
    fn flush(
        out: &mut impl Write,
        block: &mut Vec<(TermId, Term)>,
        order: Order,
        meta: &mut Vec<(Vec<u8>, u64, usize, [u8; 32])>,
        offset: &mut u64,
    ) -> io::Result<()> {
        if block.is_empty() {
            return Ok(());
        }
        let bytes = term_block_codec::encode(block)?;
        let (id, t) = block.last().unwrap();
        let key = order.key(*id, t);
        out.write_all(&bytes)?;
        meta.push((key, *offset, bytes.len(), Sha256::digest(&bytes).into()));
        *offset = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| bad("dictionary size overflow"))?;
        block.clear();
        Ok(())
    }
    for row in rows {
        let (id, t) = row?;
        let key = order.key(id, &t);
        if id.0 == 0 || previous.as_ref().is_some_and(|v| *v >= key) {
            return Err(bad("dictionary keys not unique and sorted"));
        }
        let estimate = t.to_string().len().saturating_add(16);
        if !block.is_empty()
            && (block.len() == 256 || bytes_estimate.saturating_add(estimate) > LIMIT)
        {
            flush(&mut out, &mut block, order, &mut meta, &mut offset)?;
            bytes_estimate = 0;
        }
        bytes_estimate = bytes_estimate.saturating_add(estimate);
        previous = Some(key);
        block.push((id, t));
    }
    flush(&mut out, &mut block, order, &mut meta, &mut offset)?;
    let mut dir = Vec::new();
    let count = u32::try_from(meta.len()).map_err(|_| bad("too many dictionary blocks"))?;
    dir.extend_from_slice(&count.to_le_bytes());
    for (key, off, len, hash) in meta {
        dir.extend_from_slice(&off.to_le_bytes());
        dir.extend_from_slice(&(len as u32).to_le_bytes());
        dir.extend_from_slice(&(key.len() as u32).to_le_bytes());
        dir.extend_from_slice(&key);
        dir.extend_from_slice(&hash);
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
        let value = match super::external_sort::read_record(&mut self.input, 16 * 1024 * 1024 + 9) {
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
                Order::Id => {
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


/// Single-pass variant for a legacy dictionary already in strictly increasing `Order::Term` key order (the external
/// build writes it that way). The term index is written straight from the file; `write_index` still rejects any
/// out-of-order or duplicate key. Only the ID-ordered copy needs a sort.
fn convert_term_sorted(dir: &Path) -> io::Result<([u8; 32], [u8; 32])> {
    use super::external_sort::ExternalSort;
    use std::io::{BufReader, Read};
    let mut input = BufReader::new(File::open(dir.join("dictionary"))?);
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
    let mut by_id = ExternalSort::new(dir, 128 * 1024 * 1024, 16 * 1024 * 1024 + 8)?;
    struct Legacy<'a> {
        input: BufReader<File>,
        remaining: u64,
        by_id: &'a mut ExternalSort,
        done: bool,
    }
    impl Legacy<'_> {
        fn one(&mut self) -> io::Result<Option<(TermId, Term)>> {
            let mut header = [0u8; 8];
            if self.remaining == 0 {
                let mut extra = [0u8; 1];
                if self.input.read(&mut extra)? != 0 {
                    return Err(bad("dictionary trailing bytes"));
                }
                return Ok(None);
            }
            self.input.read_exact(&mut header)?;
            let id = u64::from_le_bytes(header);
            if id == 0 {
                return Err(bad("zero dictionary ID"));
            }
            self.input.read_exact(&mut header)?;
            let len = usize::try_from(u64::from_le_bytes(header)).map_err(|_| bad("dictionary term length"))?;
            if len > 16 * 1024 * 1024 {
                return Err(bad("dictionary term too long"));
            }
            let mut text = vec![0; len];
            self.input.read_exact(&mut text)?;
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
            let mut id_key = Vec::with_capacity(len + 8);
            id_key.extend_from_slice(&id.to_be_bytes());
            id_key.extend_from_slice(&text);
            self.by_id.push(id_key)?;
            self.remaining -= 1;
            Ok(Some((TermId(id), term)))
        }
    }
    impl Iterator for Legacy<'_> {
        type Item = io::Result<(TermId, Term)>;
        fn next(&mut self) -> Option<Self::Item> {
            if self.done {
                return None;
            }
            match self.one() {
                Ok(Some(row)) => Some(Ok(row)),
                Ok(None) => {
                    self.done = true;
                    None
                }
                Err(e) => {
                    self.done = true;
                    Some(Err(e))
                }
            }
        }
    }
    let term_path = dir.join("dictionary.term");
    let id_path = dir.join("dictionary.id");
    write_index(&term_path, Order::Term, Legacy { input, remaining: count, by_id: &mut by_id, done: false })?;
    let id_run = by_id.finish()?;
    write_index(
        &id_path,
        Order::Id,
        Rows { input: BufReader::new(File::open(&id_run)?), order: Order::Id, done: false },
    )?;
    std::fs::remove_file(&id_run)?;
    let identities = (
        LazyIndex::open(&term_path, Order::Term)?.identity(),
        LazyIndex::open(&id_path, Order::Id)?.identity(),
    );
    drop(by_id);
    std::fs::remove_file(dir.join("dictionary"))?;
    Ok(identities)
}

/// Convert a freshly written legacy dictionary into two immutable indexes.
/// Caller owns manifest publication and scratch directory durability.
/// `term_sorted` declares that the legacy dictionary is already in strictly increasing term-text order.
pub(super) fn convert_legacy(dir: &Path, term_sorted: bool) -> io::Result<([u8; 32], [u8; 32])> {
    if term_sorted {
        return convert_term_sorted(dir);
    }
    use super::external_sort::ExternalSort;
    use std::io::{BufReader, Read};
    let mut input = BufReader::new(File::open(dir.join("dictionary"))?);
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
    let mut by_term = ExternalSort::new(dir, 128 * 1024 * 1024, 16 * 1024 * 1024 + 9)?;
    let mut by_id = ExternalSort::new(dir, 128 * 1024 * 1024, 16 * 1024 * 1024 + 8)?;
    for _ in 0..count {
        input.read_exact(&mut header)?;
        let id = u64::from_le_bytes(header);
        if id == 0 {
            return Err(bad("zero dictionary ID"));
        }
        input.read_exact(&mut header)?;
        let len = usize::try_from(u64::from_le_bytes(header))
            .map_err(|_| bad("dictionary term length"))?;
        if len > 16 * 1024 * 1024 {
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
        let mut term_key = Vec::with_capacity(len + 9);
        term_key.extend_from_slice(&text);
        term_key.push(0);
        term_key.extend_from_slice(&id.to_be_bytes());
        by_term.push(term_key)?;
        let mut id_key = Vec::with_capacity(len + 8);
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
    use oxrdf::{Literal, NamedNode};
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

    #[test]
    fn single_pass_conversion_matches_the_sorting_conversion() {
        let root = std::env::temp_dir().join(format!("rdf-lazy-conv-{}", std::process::id()));
        let (a, b) = (root.join("sorting"), root.join("single"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // Term-text order: literals ('"') before IRIs ('<'); IDs rise within each kind and kinds differ in the top bits.
        let mut texts: Vec<String> = (0..700).map(|i| format!("<urn:t{i:04}>")).collect();
        texts.extend((0..300).map(|i| format!("\"lit{i:04}\"")));
        texts.sort();
        let entries: Vec<(u64, &str)> = texts
            .iter()
            .enumerate()
            .map(|(n, t)| (((if t.starts_with('<') { 1u64 } else { 3 }) << 60) | n as u64, t.as_str()))
            .collect();
        legacy_dictionary(&a, &entries);
        legacy_dictionary(&b, &entries);
        let left = convert_legacy(&a, false).unwrap();
        let right = convert_legacy(&b, true).unwrap();
        assert_eq!(left, right);
        for name in ["dictionary.term", "dictionary.id"] {
            assert_eq!(std::fs::read(a.join(name)).unwrap(), std::fs::read(b.join(name)).unwrap(), "{name}");
        }
        assert!(!b.join("dictionary").exists());
        assert_eq!(std::fs::read_dir(&b).unwrap().count(), 2, "no scratch left behind");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn single_pass_conversion_rejects_an_unsorted_legacy_dictionary() {
        let dir = std::env::temp_dir().join(format!("rdf-lazy-unsorted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        legacy_dictionary(&dir, &[((1 << 60) | 1, "<urn:b>"), ((1 << 60) | 2, "<urn:a>")]);
        assert!(convert_legacy(&dir, true).is_err());
        assert!(!dir.join("dictionary.term").exists() || !dir.join("dictionary.id").exists());
        std::fs::remove_dir_all(dir).unwrap();
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
        for i in 0..10 { c.insert((7, i), &r, 10, 100); }
        assert_eq!((c.used, c.keys.len()), (100, 10), "a shard with room admits every miss");
        let mut admitted = 0;
        for i in 10..4010 {
            let before = c.entries.contains_key(&(7, i));
            assert!(!before);
            c.insert((7, i), &r, 10, 100);
            admitted += u32::from(c.entries.contains_key(&(7, i)));
            assert!(c.used <= 100 && c.keys.len() == c.entries.len());
            for (pos, key) in c.keys.iter().enumerate() { assert_eq!(c.entries[key].pos, pos) }
        }
        assert!((850..=1150).contains(&admitted), "admitted {admitted} of 4000, expected about 1000");
        assert_eq!(c.insert((7, 99_999), &r, 101, 100), 0, "a block larger than the shard is never cached");
    }

    #[test]
    fn a_referenced_block_survives_an_eviction_sample_sweep() {
        let mut c = Cache::new(0);
        let r = rows();
        for i in 0..8 { c.insert((7, i), &r, 10, 80); }
        assert!(c.get((7, 0)).is_some());
        // Every resident except block 0 is unreferenced, so one eviction never picks block 0 unless all eight
        // samples land on it, in which case its bit is cleared and it is the fallback victim.
        let mut survived = 0;
        for trial in 0..200 {
            let mut d = Cache::new(trial);
            for i in 0..8 { d.insert((7, i), &r, 10, 80); }
            d.get((7, 0));
            d.used -= d.evict_one();
            survived += u32::from(d.entries.contains_key(&(7, 0)));
        }
        assert!(survived >= 199, "referenced block evicted in {} of 200 sweeps", 200 - survived);
    }

    #[test]
    fn clear_resets_contents_and_the_generator() {
        let mut c = Cache::new(5);
        let r = rows();
        for i in 0..50 { c.insert((6, i), &r, 10, 100); }
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
