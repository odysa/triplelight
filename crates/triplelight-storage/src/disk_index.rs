//! Fallible immutable on-disk ordered index with block checksums.
//! Checksums detect accidental corruption; the manifest is not authenticated.
use crate::model::TermId;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

const MAGIC_V2: &[u8; 8] = b"RDFIDX02";
const MAGIC_V3: &[u8; 8] = b"RDFIDX03";
/// `RDFIDX03` plus tag-rotated columns in blocks (`block_codec::ROTATED`, task #105). Written by default; `RDFIDX03`
/// files stay readable.
const MAGIC_V4: &[u8; 8] = b"RDFIDX04";
/// Graph-first quad indexes (task #36): four IDs per row, otherwise the `RDFIDX03` layout.
const MAGIC_QUAD_V1: &[u8; 8] = b"RDFQIX01";
/// `RDFQIX01` plus tag-rotated columns, as `RDFIDX04` is to `RDFIDX03`.
const MAGIC_QUAD_V2: &[u8; 8] = b"RDFQIX02";
const HEADER: u64 = 24;
/// Cache shares of a triple-only snapshot (see `DiskIndex::open_with_share`).
pub const DEFAULT_SHARES: usize = 9;
/// Cache shares when a base also has the six quad orders.
pub const DATASET_SHARES: usize = 15;
const ROW: u64 = 24;
const BLOCK_ROWS: u64 = 256;
const META: u64 = 56;
// Instrumentation branch only: counters are process-wide and include concurrent
// work; a single-query run after background statistics has finished isolates it.
static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static HITS: [AtomicU64; TRACE_SLOTS] = [const { AtomicU64::new(0) }; TRACE_SLOTS];
static MISSES: [AtomicU64; TRACE_SLOTS] = [const { AtomicU64::new(0) }; TRACE_SLOTS];
static READS: [AtomicU64; TRACE_SLOTS] = [const { AtomicU64::new(0) }; TRACE_SLOTS];
static BYTES: [AtomicU64; TRACE_SLOTS] = [const { AtomicU64::new(0) }; TRACE_SLOTS];
static EVICTIONS: [AtomicU64; TRACE_SLOTS] = [const { AtomicU64::new(0) }; TRACE_SLOTS];
pub fn cache_trace_enabled() -> bool {
    *TRACE_ENABLED.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_BLOCK_CACHE").is_some())
}
pub(super) fn trace_dictionary(slot: usize, metric: usize, value: u64) {
    if !cache_trace_enabled() {
        return;
    }
    let counters = match metric {
        0 => &HITS,
        1 => &MISSES,
        2 => &READS,
        3 => &BYTES,
        4 => &EVICTIONS,
        _ => unreachable!(),
    };
    counters[slot].fetch_add(value, Ordering::Relaxed);
}
/// Trace slots: 0-5 the triple orders (`Perm::ALL`), 6 and 7 the dictionary lookups (by term, by ID), 8 the six
/// graph-first quad orders of a named-graph base together.
pub const TRACE_SLOTS: usize = 9;
/// The trace slot shared by the quad orders.
pub const QUAD_TRACE_SLOT: usize = 8;
pub fn cache_trace_snapshot() -> [[u64; 5]; TRACE_SLOTS] {
    std::array::from_fn(|i| {
        [
            HITS[i].load(Ordering::Relaxed),
            MISSES[i].load(Ordering::Relaxed),
            READS[i].load(Ordering::Relaxed),
            BYTES[i].load(Ordering::Relaxed),
            EVICTIONS[i].load(Ordering::Relaxed),
        ]
    })
}

static CACHE_MB_OVERRIDE: AtomicUsize = AtomicUsize::new(0);
static DEFAULT_CACHE_MB: OnceLock<usize> = OnceLock::new();
/// Sets the per-snapshot aggregate block cache target before opening a snapshot.
pub fn set_cache_budget_mb(value: usize) -> io::Result<()> {
    if value == 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "cache budget must be nonzero",
        ));
    }
    CACHE_MB_OVERRIDE.store(value, Ordering::Relaxed);
    Ok(())
}
pub(super) fn cache_budget_mb() -> usize {
    let value = CACHE_MB_OVERRIDE.load(Ordering::Relaxed);
    if value != 0 {
        return value;
    }
    *DEFAULT_CACHE_MB.get_or_init(|| {
        physical_memory_bytes().map_or(256, |bytes| {
            usize::try_from(bytes / 4 / (1024 * 1024))
                .unwrap_or(256)
                .max(1)
        })
    })
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, reason)
}
/// A row width the index supports: 3 (triples, `RDFIDX02`/`RDFIDX03`) or 4 (graph-first quads, `RDFQIX01`). Each
/// width has its own per-thread block memo, so the triple index's hot path is unchanged.
pub struct Width<const N: usize>;
pub trait SupportedWidth<const N: usize> {
    /// The compressed format written, which allows tag-rotated columns.
    #[doc(hidden)]
    const MAGIC: &'static [u8; 8];
    /// The previous compressed format (no rotated columns), still read.
    #[doc(hidden)]
    const OLDER: &'static [u8; 8];
    #[doc(hidden)]
    fn memo() -> &'static std::thread::LocalKey<std::cell::RefCell<[MemoEntry<N>; MEMO_SLOTS]>>;
}
impl SupportedWidth<3> for Width<3> {
    const MAGIC: &'static [u8; 8] = MAGIC_V4;
    const OLDER: &'static [u8; 8] = MAGIC_V3;
    fn memo() -> &'static std::thread::LocalKey<std::cell::RefCell<[MemoEntry<3>; MEMO_SLOTS]>> { &BLOCK_MEMO }
}
impl SupportedWidth<4> for Width<4> {
    const MAGIC: &'static [u8; 8] = MAGIC_QUAD_V2;
    const OLDER: &'static [u8; 8] = MAGIC_QUAD_V1;
    fn memo() -> &'static std::thread::LocalKey<std::cell::RefCell<[MemoEntry<4>; MEMO_SLOTS]>> { &BLOCK_MEMO_QUAD }
}
/// Bytes of one uncompressed row of width `N`.
const fn row_bytes(n: usize) -> u64 { n as u64 * 8 }

fn decode_row<const N: usize>(row: &[u8]) -> [TermId; N] {
    std::array::from_fn(|i| {
        TermId(u64::from_le_bytes(
            row[i * 8..i * 8 + 8].try_into().unwrap(),
        ))
    })
}
/// Writes strictly increasing keys, block hashes and a checksummed footer.
/// The caller owns atomic file publication and durability.
#[cfg(test)]
fn write_index_v2(
    mut out: impl Write,
    rows: impl ExactSizeIterator<Item = [TermId; 3]>,
) -> io::Result<()> {
    let count = rows.len() as u64;
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(MAGIC_V2);
    header[8..16].copy_from_slice(&count.to_le_bytes());
    header[16..].copy_from_slice(&BLOCK_ROWS.to_le_bytes());
    out.write_all(&header)?;
    let (mut written, mut in_block) = (0u64, 0u64);
    let mut previous = None;
    let mut hash = Sha256::new();
    let mut meta = Vec::new();
    for row in rows {
        if row.iter().any(|id| id.0 == 0) || previous.is_some_and(|p| p >= row) {
            return Err(invalid("keys must be nonzero, sorted and unique"));
        }
        let mut bytes = [0u8; 24];
        for i in 0..3 {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&row[i].0.to_le_bytes());
        }
        out.write_all(&bytes)?;
        hash.update(bytes);
        written += 1;
        in_block += 1;
        previous = Some(row);
        if in_block == BLOCK_ROWS || written == count {
            meta.extend_from_slice(&bytes);
            meta.extend_from_slice(&hash.finalize_reset());
            in_block = 0;
        }
    }
    if written != count {
        return Err(invalid("iterator length changed"));
    }
    out.write_all(&meta)?;
    let mut footer = Sha256::new();
    footer.update(header);
    footer.update(&meta);
    out.write_all(&footer.finalize())?;
    Ok(())
}
/// Writes compressed, independently checksummed blocks and a checksummed directory.
pub fn write_index<const N: usize>(
    out: impl Write,
    rows: impl ExactSizeIterator<Item = [TermId; N]>,
) -> io::Result<()>
where
    Width<N>: SupportedWidth<N>,
{
    let count = rows.len() as u64;
    write_index_stream(out, count, rows.map(Ok))
}

/// Write a sorted stream whose exact row count is known; propagate source read errors.
pub fn write_index_stream<const N: usize>(
    out: impl Write,
    count: u64,
    rows: impl Iterator<Item = io::Result<[TermId; N]>>,
) -> io::Result<()>
where
    Width<N>: SupportedWidth<N>,
{
    write_index_format(out, count, rows, true)
}
/// `rotate`: the current format (`SupportedWidth::MAGIC`, tag-rotated columns allowed); otherwise the previous one
/// (`OLDER`), which tests use to check that older files stay readable.
fn write_index_format<const N: usize>(
    mut out: impl Write,
    count: u64,
    rows: impl Iterator<Item = io::Result<[TermId; N]>>,
    rotate: bool,
) -> io::Result<()>
where
    Width<N>: SupportedWidth<N>,
{
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(if rotate { <Width<N> as SupportedWidth<N>>::MAGIC } else { <Width<N> as SupportedWidth<N>>::OLDER });
    header[8..16].copy_from_slice(&count.to_le_bytes());
    header[16..].copy_from_slice(&BLOCK_ROWS.to_le_bytes());
    out.write_all(&header)?;
    let mut meta = Vec::new();
    let mut block = Vec::with_capacity(BLOCK_ROWS as usize);
    let mut previous = None;
    let mut written = 0u64;
    let flush =
        |block: &mut Vec<[TermId; N]>, out: &mut dyn Write, meta: &mut Vec<u8>| -> io::Result<()> {
            if block.is_empty() {
                return Ok(());
            }
            let bytes = super::block_codec::encode(block, rotate)?;
            for id in block.last().unwrap() {
                meta.extend_from_slice(&id.0.to_le_bytes());
            }
            meta.extend_from_slice(&Sha256::digest(&bytes));
            meta.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.write_all(&bytes)?;
            block.clear();
            Ok(())
        };
    for row in rows {
        let row = row?;
        if row.iter().any(|id| id.0 == 0) || previous.is_some_and(|p| p >= row) {
            return Err(invalid("keys must be nonzero, sorted and unique"));
        }
        previous = Some(row);
        block.push(row);
        written += 1;
        if block.len() == BLOCK_ROWS as usize {
            flush(&mut block, &mut out, &mut meta)?;
        }
    }
    if written != count {
        return Err(invalid("iterator length changed"));
    }
    flush(&mut block, &mut out, &mut meta)?;
    out.write_all(&meta)?;
    let mut digest = Sha256::new();
    digest.update(header);
    digest.update(&meta);
    out.write_all(&digest.finalize())?;
    Ok(())
}
#[derive(Clone, Copy)]
struct BlockMeta<const N: usize> {
    last: [TermId; N],
    checksum: [u8; 32],
    offset: u64,
    length: usize,
}
struct CacheShard<const N: usize> {
    /// Block id -> (rows, CLOCK referenced bit). The bit is atomic so hits need only the shard's read lock.
    entries: HashMap<u64, (Arc<Vec<[TermId; N]>>, AtomicBool)>,
    clock: VecDeque<u64>,
    /// Bumped by `clear_cache`; a block read from disk is inserted only if no clear happened since its lookup began.
    generation: u64,
}
#[doc(hidden)]
pub const MEMO_SLOTS: usize = 8;
/// (index instance, cache epoch, block id, rows). Up to `MEMO_SLOTS` decoded blocks per long-lived thread stay
/// pinned outside the cache budget (about 6 KB each), also after the index is dropped, until the slot is reused.
/// Memo hits are not counted by the shared-cache trace counters.
#[doc(hidden)]
pub type MemoEntry<const N: usize> = Option<(u64, u64, u64, Arc<Vec<[TermId; N]>>)>;
thread_local! {
    static BLOCK_MEMO: std::cell::RefCell<[MemoEntry<3>; MEMO_SLOTS]> = const { std::cell::RefCell::new([const { None }; MEMO_SLOTS]) };
    static BLOCK_MEMO_QUAD: std::cell::RefCell<[MemoEntry<4>; MEMO_SLOTS]> = const { std::cell::RefCell::new([const { None }; MEMO_SLOTS]) };
}
#[cfg(test)]
thread_local! {
    /// Test-only: runs in `block_shared` after a miss's disk read, before it takes the write lock to insert.
    static MISS_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&dyn std::any::Any)>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
thread_local! {
    /// Test-only: runs inside `clear_cache` between emptying the shards and bumping the epoch.
    static CLEAR_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce(&dyn std::any::Any)>>> = const { std::cell::RefCell::new(None) };
}
/// Unique per opened index, so a thread's block memo can never confuse two indexes (no address reuse).
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
/// An immutable sorted index of `N`-ID rows (3 for triples, 4 for graph-first quads).
pub struct DiskIndex<const N: usize = 3> {
    instance: u64,
    /// Bumped by `clear_cache`; memo entries from an older epoch are ignored, so verification re-reads from disk.
    epoch: AtomicU64,
    file: File,
    rows: u64,
    identity: [u8; 32],
    blocks: Vec<BlockMeta<N>>,
    compressed: bool,
    /// The format allows tag-rotated block columns (`RDFIDX04`/`RDFQIX02`).
    rotated: bool,
    cache: Vec<RwLock<CacheShard<N>>>,
    shard_capacity: usize,
    trace_slot: Option<usize>,
}
impl<const N: usize> DiskIndex<N>
where
    Width<N>: SupportedWidth<N>,
{
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_with_share(path, DEFAULT_SHARES)
    }
    /// Blocks one cache shard holds (tests check the share a snapshot gave this index).
    #[cfg(test)]
    pub(super) fn shard_capacity(&self) -> usize { self.shard_capacity }
    /// Opens with a block cache of `1 / shares` of the snapshot budget (the dictionary takes three shares; a
    /// triple-only snapshot's six orders one each, so 9; with the six quad orders of a named-graph base, 15).
    pub fn open_with_share(path: impl AsRef<Path>, shares: usize) -> io::Result<Self> {
        let trace_slot = match path.as_ref().file_name().and_then(|v| v.to_str()) {
            Some("spo") => Some(0),
            Some("sop") => Some(1),
            Some("pso") => Some(2),
            Some("pos") => Some(3),
            Some("osp") => Some(4),
            Some("ops") => Some(5),
            // All six graph-first quad orders share one slot (task #36).
            Some("gspo" | "gsop" | "gpso" | "gpos" | "gosp" | "gops") => Some(QUAD_TRACE_SLOT),
            _ => None,
        };
        let file = File::open(path)?;
        let mut header = [0; 24];
        read_exact_at(&file, &mut header, 0)?;
        let (compressed, rotated) = match &header[..8] {
            m if N == 3 && m == MAGIC_V2 => (false, false),
            m if m == <Width<N> as SupportedWidth<N>>::OLDER => (true, false),
            m if m == <Width<N> as SupportedWidth<N>>::MAGIC => (true, true),
            _ => return Err(invalid("bad index magic or version")),
        };
        let rows = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if u64::from_le_bytes(header[16..24].try_into().unwrap()) != BLOCK_ROWS {
            return Err(invalid("bad block size"));
        }
        let blocks = rows.div_ceil(BLOCK_ROWS);
        let entry_size = if compressed { row_bytes(N) + 40 } else { META };
        let meta_len = blocks
            .checked_mul(entry_size)
            .ok_or_else(|| invalid("index length overflow"))?;
        let file_len = file.metadata()?.len();
        let offset = file_len
            .checked_sub(32)
            .and_then(|n| n.checked_sub(meta_len))
            .filter(|n| *n >= HEADER)
            .ok_or_else(|| invalid("index length mismatch"))?;
        if !compressed
            && offset
                != HEADER
                    .checked_add(
                        rows.checked_mul(row_bytes(N))
                            .ok_or_else(|| invalid("index length overflow"))?,
                    )
                    .ok_or_else(|| invalid("index length overflow"))?
        {
            return Err(invalid("index length mismatch"));
        }
        let size = usize::try_from(meta_len).map_err(|_| invalid("block directory too large"))?;
        let mut raw = vec![0u8; size];
        read_exact_at(&file, &mut raw, offset)?;
        let mut footer = [0u8; 32];
        read_exact_at(&file, &mut footer, offset + meta_len)?;
        let mut check = Sha256::new();
        check.update(header);
        check.update(&raw);
        if check.finalize().as_slice() != footer {
            return Err(invalid("index header/directory checksum mismatch"));
        }
        let mut entries = Vec::new();
        let mut data_pos = HEADER;
        for (i, item) in raw.chunks_exact(entry_size as usize).enumerate() {
            let w = row_bytes(N) as usize;
            let last = decode_row::<N>(&item[..w]);
            let checksum = item[w..w + 32].try_into().unwrap();
            if last.iter().any(|x| x.0 == 0)
                || entries.last().is_some_and(|b: &BlockMeta<N>| b.last >= last)
            {
                return Err(invalid("invalid block directory"));
            }
            let len = if compressed {
                u64::from_le_bytes(item[w + 32..w + 40].try_into().unwrap())
            } else {
                (rows - (i as u64) * BLOCK_ROWS).min(BLOCK_ROWS) * ROW
            };
            let codec_header = super::block_codec::header_len(N) as u64;
            if compressed && !(codec_header..=codec_header + BLOCK_ROWS * row_bytes(N)).contains(&len) {
                return Err(invalid("invalid compressed block length"));
            }
            let next = data_pos
                .checked_add(len)
                .filter(|n| *n <= offset)
                .ok_or_else(|| invalid("block offset out of range"))?;
            entries.push(BlockMeta {
                last,
                checksum,
                offset: data_pos,
                length: usize::try_from(len).map_err(|_| invalid("block too large"))?,
            });
            data_pos = next;
        }
        if data_pos != offset {
            return Err(invalid("index data length mismatch"));
        }
        Ok(Self {
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            epoch: AtomicU64::new(0),
            file,
            rows,
            trace_slot,
            identity: footer,
            blocks: entries,
            compressed,
            rotated,
            cache: (0..16)
                .map(|_| {
                    RwLock::new(CacheShard {
                        entries: HashMap::new(),
                        clock: VecDeque::new(),
                        generation: 0,
                    })
                })
                .collect(),
            shard_capacity: cache_budget_mb()
                .saturating_mul(1024 * 1024)
                .checked_div(shares.max(1) * 16 * BLOCK_ROWS as usize * row_bytes(N) as usize)
                .unwrap_or(1)
                .max(1),
        })
    }
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn clear_cache(&self) -> io::Result<()> {
        for shard in &self.cache {
            let mut cache = shard
                .write()
                .map_err(|_| io::Error::other("index cache lock poisoned"))?;
            cache.entries.clear();
            cache.clock.clear();
            cache.generation += 1;
        }
        #[cfg(test)]
        if let Some(hook) = CLEAR_HOOK.with(|h| h.borrow_mut().take()) { hook(self as &dyn std::any::Any) }
        // Then invalidate every thread's memo entries (they are keyed by epoch). Order matters under concurrent
        // reads: a reader memoizes under the epoch it loaded *before* its lookup. A reader that saw the old epoch
        // can only memoize under the old epoch (invalid after this bump), whatever it fetched. A reader that sees
        // the new epoch started after every shard was emptied, so its block comes from disk (checksummed): shards
        // are only ever filled from disk reads, never from memo entries.
        self.epoch.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.rows
    }
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }
    fn block(&self, id: u64) -> io::Result<Arc<Vec<[TermId; N]>>> {
        // Per-thread memo of recently used blocks: leapfrog seeks and index nested-loop probes hit the same few
        // blocks back to back from every rayon worker, and taking the shard mutex each time serialized them
        // (mutex waits dominated a disk LUBM-1000 Q2 profile). Blocks are immutable for an index instance, so a
        // memo entry keyed by (instance, block) never goes stale.
        let epoch = self.epoch.load(Ordering::Acquire);
        let memo = <Width<N> as SupportedWidth<N>>::memo();
        if let Some(rows) = memo.with(|m| m.borrow().iter().flatten().find(|e| e.0 == self.instance && e.1 == epoch && e.2 == id).map(|e| Arc::clone(&e.3))) {
            return Ok(rows);
        }
        let rows = self.block_shared(id)?;
        memo.with(|m| {
            let mut m = m.borrow_mut();
            let slot = m.iter().position(Option::is_none).unwrap_or_else(|| { m.rotate_left(1); MEMO_SLOTS - 1 });
            m[slot] = Some((self.instance, epoch, id, Arc::clone(&rows)));
        });
        Ok(rows)
    }
    fn block_shared(&self, id: u64) -> io::Result<Arc<Vec<[TermId; N]>>> {
        let shard = &self.cache[(id as usize) % self.cache.len()];
        let poisoned = || io::Error::other("index cache lock poisoned");
        // Hit: shared read lock only, so concurrent readers of hot blocks don't serialize.
        let generation = {
            let cache = shard.read().map_err(|_| poisoned())?;
            if let Some((value, referenced)) = cache.entries.get(&id) {
                if cache_trace_enabled() && let Some(i) = self.trace_slot {
                    HITS[i].fetch_add(1, Ordering::Relaxed);
                }
                referenced.store(true, Ordering::Relaxed);
                return Ok(Arc::clone(value));
            }
            cache.generation
        };
        if cache_trace_enabled() && let Some(i) = self.trace_slot {
            MISSES[i].fetch_add(1, Ordering::Relaxed);
        }
        // Miss: read, verify and decode without holding the lock. Concurrent misses on one block may each read and
        // decode it (first insert wins), so decoded blocks can transiently exceed the nominal cache budget.
        let r = Arc::new(self.read_block(id)?);
        #[cfg(test)]
        if let Some(hook) = MISS_HOOK.with(|h| h.borrow_mut().take()) { hook(self as &dyn std::any::Any) }
        let mut cache = shard.write().map_err(|_| poisoned())?;
        if let Some((value, referenced)) = cache.entries.get(&id) {
            referenced.store(true, Ordering::Relaxed);
            return Ok(Arc::clone(value));
        }
        if cache.generation != generation {
            // A clear_cache ran while we read: don't repopulate the cleared cache with a pre-clear read.
            return Ok(r);
        }
        while cache.entries.len() >= self.shard_capacity {
            let old = cache.clock.pop_front().expect("cache clock consistent");
            let entry = cache.entries.get(&old).expect("cache entry consistent");
            if entry.1.swap(false, Ordering::Relaxed) {
                cache.clock.push_back(old);
            } else {
                cache.entries.remove(&old);
                if cache_trace_enabled() && let Some(i) = self.trace_slot {
                    EVICTIONS[i].fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        cache.entries.insert(id, (Arc::clone(&r), AtomicBool::new(true)));
        cache.clock.push_back(id);
        Ok(r)
    }
    /// Reads, checksums, decodes and validates one block from the file (no caching).
    fn read_block(&self, id: u64) -> io::Result<Vec<[TermId; N]>> {
        let start = id
            .checked_mul(BLOCK_ROWS)
            .ok_or_else(|| invalid("block overflow"))?;
        if start >= self.rows {
            return Err(invalid("block out of range"));
        }
        let n = (self.rows - start).min(BLOCK_ROWS) as usize;
        let meta = self
            .blocks
            .get(id as usize)
            .ok_or_else(|| invalid("missing block directory entry"))?;
        let mut bytes = vec![0; meta.length];
        read_exact_at_traced(&self.file, &mut bytes, meta.offset, self.trace_slot)?;
        if Sha256::digest(&bytes).as_slice() != meta.checksum {
            return Err(invalid("index block checksum mismatch"));
        }
        let rows: Vec<_> = if self.compressed {
            super::block_codec::decode::<N>(&bytes, n, self.rotated)?
        } else {
            bytes.chunks_exact(row_bytes(N) as usize).map(decode_row::<N>).collect()
        };
        if rows.iter().any(|r| r.iter().any(|id| id.0 == 0))
            || rows.windows(2).any(|w| w[0] >= w[1])
            || rows.last() != Some(&meta.last)
            || (id > 0 && rows[0] <= self.blocks[id as usize - 1].last)
        {
            return Err(invalid("invalid index block ordering"));
        }
        Ok(rows)
    }
    fn row(&self, i: u64) -> io::Result<[TermId; N]> {
        if i >= self.rows {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "index row out of bounds",
            ));
        }
        Ok(self.block(i / BLOCK_ROWS)?[(i % BLOCK_ROWS) as usize])
    }
    fn bound(&self, key: [TermId; N], upper: bool) -> io::Result<u64> {
        let block = self
            .blocks
            .partition_point(|m| m.last < key || (upper && m.last == key));
        if block == self.blocks.len() {
            return Ok(self.rows);
        }
        let rows = self.block(block as u64)?;
        let offset = rows.partition_point(|r| *r < key || (upper && *r == key));
        Ok(block as u64 * BLOCK_ROWS + offset as u64)
    }
    pub fn range(&self, prefix: &[TermId]) -> io::Result<std::ops::Range<u64>> {
        if prefix.len() > N {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "prefix longer than the index rows",
            ));
        }
        let (mut low, mut high) = ([TermId(0); N], [TermId(u64::MAX); N]);
        low[..prefix.len()].copy_from_slice(prefix);
        high[..prefix.len()].copy_from_slice(prefix);
        Ok(self.bound(low, false)?..self.bound(high, true)?)
    }
    /// Copies at most one decoded block from the given position.
    pub fn block_slice(&self, start: u64, end: u64) -> io::Result<Vec<[TermId; N]>> {
        if start >= end || end > self.rows {
            return Err(invalid("invalid block slice"));
        }
        let rows = self.block(start / BLOCK_ROWS)?;
        let from = (start % BLOCK_ROWS) as usize;
        let count = (end - start).min(BLOCK_ROWS - start % BLOCK_ROWS) as usize;
        Ok(rows[from..from + count].to_vec())
    }
    /// Read a full-scan block without populating the shared query cache.
    pub fn block_slice_uncached(&self, start: u64, end: u64) -> io::Result<Vec<[TermId; N]>> {
        if start >= end || end > self.rows { return Err(invalid("invalid block slice")); }
        let rows = self.read_block(start / BLOCK_ROWS)?;
        let from = (start % BLOCK_ROWS) as usize;
        let count = (end - start).min(BLOCK_ROWS - start % BLOCK_ROWS) as usize;
        Ok(rows[from..from + count].to_vec())
    }

    pub fn seek_value(&self, prefix: &[TermId], target: TermId) -> io::Result<Option<TermId>> {
        if prefix.len() >= N {
            return Ok(None);
        }
        let mut key = [TermId(0); N];
        key[..prefix.len()].copy_from_slice(prefix);
        key[prefix.len()] = target;
        let id = self.blocks.partition_point(|m| m.last < key);
        if id == self.blocks.len() {
            return Ok(None);
        }
        let rows = self.block(id as u64)?;
        let offset = rows.partition_point(|r| *r < key);
        let Some(row) = rows.get(offset) else {
            return Ok(None);
        };
        if &row[..prefix.len()] == prefix {
            Ok(Some(row[prefix.len()]))
        } else {
            Ok(None)
        }
    }
    pub fn get(&self, i: u64) -> io::Result<[TermId; N]> {
        self.row(i)
    }
    /// Read and verify every data block; can be expensive for large indexes.
    pub fn validate(&self) -> io::Result<()> {
        for i in 0..self.blocks.len() {
            self.block(i as u64)?;
        }
        Ok(())
    }
}
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    read_exact_at_traced(file, buf, offset, None)
}
fn read_exact_at_traced(
    file: &File,
    mut buf: &mut [u8],
    mut offset: u64,
    slot: Option<usize>,
) -> io::Result<()> {
    while !buf.is_empty() {
        if cache_trace_enabled() && let Some(i) = slot {
            READS[i].fetch_add(1, Ordering::Relaxed);
        }
        match file.read_at(buf, offset) {
            Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "short index read")),
            Ok(n) => {
                if cache_trace_enabled() && let Some(i) = slot {
                    BYTES[i].fetch_add(n as u64, Ordering::Relaxed);
                }
                offset += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
/// Memory available to this process in bytes, if it can be determined: macOS `sysctl hw.memsize`; Linux
/// `/proc/meminfo` `MemTotal`, capped by the process's cgroup memory limit (a container or a `systemd-run` scope), so
/// the default cache and build budgets fit inside the limit.
/// Computed once per process. On macOS this spawns `sysctl`, and a child process may briefly share the parent's
/// open file descriptions until it execs; spawning on every disk open made it likely that a concurrently closed
/// database's directory lock (flock) outlived its close for a few milliseconds. Caching removes this repeated
/// source; it does not make immediate reopen race-free against the first call or any other child spawned by the
/// process.
pub fn physical_memory_bytes() -> Option<u64> {
    static MEMORY: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEMORY.get_or_init(read_physical_memory_bytes)
}

/// The tightest memory limit on the process's cgroups, from `/proc/self/cgroup` (`groups`) and the files `read` returns:
/// cgroup v2 `memory.max` of the process's group and of each ancestor, or cgroup v1 `memory.limit_in_bytes`. `None` if
/// there is no numeric limit (`max`, or nothing readable). A v1 "unlimited" value is a huge number, which the caller's
/// `min` with `MemTotal` makes harmless.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cgroup_memory_limit(groups: &str, read: &dyn Fn(&str) -> Option<String>) -> Option<u64> {
    let number = |file: String| read(&file).and_then(|t| t.trim().parse::<u64>().ok());
    let mut tightest: Option<u64> = None;
    for line in groups.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path = path.trim_end_matches('/');
        let limits: Vec<u64> = if controllers.is_empty() {
            // v2: "0::/a/b". The group and every ancestor up to the root can carry a limit.
            let mut dir = path.to_string();
            let mut found = Vec::new();
            loop {
                found.extend(number(format!("/sys/fs/cgroup{dir}/memory.max")));
                match dir.rfind('/') { Some(i) => dir.truncate(i), None => break }
            }
            found
        } else if controllers.split(',').any(|c| c == "memory") {
            // The group's own path, or, inside a v1 container whose memory mount is its own group, the mount root.
            let own = number(format!("/sys/fs/cgroup/memory{path}/memory.limit_in_bytes"));
            own.or_else(|| number("/sys/fs/cgroup/memory/memory.limit_in_bytes".to_string())).into_iter().collect()
        } else {
            Vec::new()
        };
        for limit in limits { tightest = Some(tightest.map_or(limit, |t| t.min(limit))) }
    }
    tightest
}

fn read_physical_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        return String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok();
    }
    #[cfg(target_os = "linux")]
    {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = info
            .lines()
            .find(|l| l.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        let total = kb * 1024;
        let groups = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
        let limit = cgroup_memory_limit(&groups, &|path| std::fs::read_to_string(path).ok());
        return Some(limit.map_or(total, |limit| limit.min(total)));
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    /// The cgroup limit is the tightest `memory.max` on the group or an ancestor (v2), or the v1 memory controller's
    /// limit; `max` or a missing file means no limit.
    #[test]
    fn cgroup_memory_limit_takes_the_tightest_limit() {
        let files = |entries: &'static [(&'static str, &'static str)]| move |path: &str| {
            entries.iter().find(|(p, _)| *p == path).map(|(_, v)| v.to_string())
        };
        let v2 = files(&[
            ("/sys/fs/cgroup/system.slice/bench.scope/memory.max", "8589934592\n"),
            ("/sys/fs/cgroup/system.slice/memory.max", "max\n"),
            ("/sys/fs/cgroup/memory.max", "max\n"),
        ]);
        assert_eq!(cgroup_memory_limit("0::/system.slice/bench.scope\n", &v2), Some(8 << 30));
        let parent = files(&[("/sys/fs/cgroup/a/memory.max", "4294967296"), ("/sys/fs/cgroup/a/b/memory.max", "max")]);
        assert_eq!(cgroup_memory_limit("0::/a/b", &parent), Some(4 << 30), "an ancestor's limit applies");
        let nested = files(&[("/sys/fs/cgroup/a/memory.max", "4294967296"), ("/sys/fs/cgroup/a/b/memory.max", "8589934592")]);
        assert_eq!(cgroup_memory_limit("0::/a/b", &nested), Some(4 << 30), "the tightest of several limits wins");
        let docker = files(&[("/sys/fs/cgroup/memory.max", "17179869184")]);
        assert_eq!(cgroup_memory_limit("0::/\n", &docker), Some(16 << 30), "a container sees its group as the root");
        assert_eq!(cgroup_memory_limit("0::/x", &files(&[("/sys/fs/cgroup/x/memory.max", "max")])), None);
        let v1 = files(&[("/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes", "2147483648")]);
        assert_eq!(cgroup_memory_limit("12:cpu,cpuacct:/docker/abc\n11:memory:/docker/abc\n", &v1), Some(2 << 30));
        let v1_container = files(&[("/sys/fs/cgroup/memory/memory.limit_in_bytes", "1073741824")]);
        assert_eq!(cgroup_memory_limit("11:memory:/docker/abc\n", &v1_container), Some(1 << 30), "a v1 container sees its group at the mount root");
        assert_eq!(cgroup_memory_limit("", &files(&[])), None);
        assert_eq!(cgroup_memory_limit("garbage\n", &files(&[])), None);
    }
    use super::*;
    #[test]
    fn fallible_stream_propagates_errors_and_rejects_wrong_count() {
        let row = [TermId(1), TermId(2), TermId(3)];
        let input = vec![Ok(row), Err(io::Error::other("source failed"))];
        let err = write_index_stream(Vec::new(), 2, input.into_iter()).unwrap_err();
        assert!(err.to_string().contains("source failed"));
        assert!(write_index_stream(Vec::new(), 2, [Ok(row)].into_iter()).is_err());
    }
    #[test]
    fn block_boundaries_cache_and_late_short_read() {
        let path = std::env::temp_dir().join(format!("rdf-cache-{}", std::process::id()));
        let rows: Vec<_> = (1..=700)
            .map(|i| [TermId(i), TermId(1), TermId(2)])
            .collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.clone().into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        for n in [1, 255, 256, 257, 512, 513, 700, 701] {
            let start = (n - 1).min(700);
            assert_eq!(
                index.range(&[TermId(n)]).unwrap(),
                start..(start + u64::from(n <= 700))
            );
        }
        // Cached blocks remain usable by design; a miss after external truncation reports an error.
        drop(index);
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        let cut = index.blocks[2].offset as usize + index.blocks[2].length / 2;
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(index.get(699).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn uncached_stats_blocks_do_not_pollute_cache_and_check_corruption() {
        let path = std::env::temp_dir().join(format!("rdf-stats-uncached-{}", std::process::id()));
        let rows: Vec<_> = (1..=700).map(|i| [TermId(i), TermId(1), TermId(2)]).collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.clone().into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        assert_eq!(index.block_slice_uncached(256, 512).unwrap(), rows[256..512]);
        assert!(index.cache.iter().all(|shard| shard.read().unwrap().entries.is_empty()));
        let mut bad = bytes;
        bad[index.blocks[1].offset as usize + 3] ^= 0xff;
        std::fs::write(&path, bad).unwrap();
        assert!(index.block_slice_uncached(256, 512).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn clear_cache_invalidates_thread_memo() {
        // Warm a block on this thread, corrupt it on disk, clear caches: the next read must hit disk and fail.
        let path = std::env::temp_dir().join(format!("rdf-memo-{}", std::process::id()));
        let rows: Vec<_> = (1..=700).map(|i| [TermId(i), TermId(1), TermId(2)]).collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        assert!(index.get(300).is_ok());
        let mut bad = bytes.clone();
        let pos = index.blocks[1].offset as usize + 3;
        bad[pos] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        assert!(index.get(300).is_ok(), "memoized/cached read before invalidation");
        index.clear_cache().unwrap();
        assert!(index.get(300).is_err(), "corruption must be detected after clear_cache");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn read_between_shard_clear_and_epoch_bump_leaves_no_usable_memo() {
        // The race window: a reader runs after the shards are emptied but before the epoch bump, while the file is
        // already corrupt. Whatever it gets, nothing it memoizes may survive the bump.
        let path = std::env::temp_dir().join(format!("rdf-memo-race-{}", std::process::id()));
        let rows: Vec<_> = (1..=700).map(|i| [TermId(i), TermId(1), TermId(2)]).collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        assert!(index.get(300).is_ok());
        let mut bad = bytes.clone();
        bad[index.blocks[1].offset as usize + 3] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        CLEAR_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(|ix: &dyn std::any::Any| {
            let ix = ix.downcast_ref::<DiskIndex>().unwrap();
            let _ = ix.get(300); // old-epoch reader inside the window (may be served from its old memo)
            let _ = ix.get(301);
        })));
        index.clear_cache().unwrap();
        assert!(index.get(300).is_err(), "a block memoized inside the clear window must not survive the epoch bump");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn clear_during_inflight_miss_does_not_repopulate_shard() {
        // A miss reads a block, then (before inserting) a clear_cache runs: the cleared shard must stay empty.
        let path = std::env::temp_dir().join(format!("rdf-miss-race-{}", std::process::id()));
        let rows: Vec<_> = (1..=700).map(|i| [TermId(i), TermId(1), TermId(2)]).collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        MISS_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(|ix: &dyn std::any::Any| ix.downcast_ref::<DiskIndex>().unwrap().clear_cache().unwrap())));
        assert!(index.block_shared(1).is_ok());
        let shard = index.cache[1 % index.cache.len()].read().unwrap();
        assert!(!shard.entries.contains_key(&1), "pre-clear read must not repopulate the cleared shard");
        drop(shard);
        // Without a concurrent clear, a miss is cached as usual.
        assert!(index.block_shared(2).is_ok());
        assert!(index.cache[2 % index.cache.len()].read().unwrap().entries.contains_key(&2));
        std::fs::remove_file(path).unwrap();
    }
    /// `RDFIDX03` files (no rotated columns) stay readable, and `RDFIDX04` stores mixed-tag columns in less space.
    #[test]
    fn older_and_current_formats_read_the_same_rows_and_mixed_tags_shrink() {
        // Objects mix IRIs (tag 1), literals (tag 3) and inline numbers (tag 0xf), as in an SPO order.
        let tag = |t: u64, rank: u64| TermId((t << 60) | rank);
        let rows: Vec<[TermId; 3]> = (0..2000u64)
            .map(|i| [tag(1, 10 + i / 6), tag(1, 3 + i % 6), tag([1, 3, 0xf][(i % 3) as usize], 1 + i * 37)])
            .collect();
        let (mut current, mut older) = (Vec::new(), Vec::new());
        write_index_format(&mut current, rows.len() as u64, rows.iter().copied().map(Ok), true).unwrap();
        write_index_format(&mut older, rows.len() as u64, rows.iter().copied().map(Ok), false).unwrap();
        assert_eq!(&current[..8], MAGIC_V4);
        assert_eq!(&older[..8], MAGIC_V3);
        assert!(current.len() * 3 < older.len() * 2, "rotated object columns: {} against {} bytes", current.len(), older.len());
        for (name, bytes) in [("current", &current), ("older", &older)] {
            let path = std::env::temp_dir().join(format!("rdf-index-format-{name}-{}", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            let index = DiskIndex::<3>::open(&path).unwrap();
            index.validate().unwrap();
            let got: Vec<[TermId; 3]> = (0..index.len()).map(|i| index.get(i).unwrap()).collect();
            assert_eq!(got, rows, "{name}");
            assert_eq!(index.range(&[rows[700][0]]).unwrap().len(), 6, "{name}");
            std::fs::remove_file(&path).unwrap();
        }
    }
    /// A graph-first quad index answers `range`, `seek_value`, `get` and block reads like a sorted row model, across
    /// block boundaries, and each width refuses the other's files.
    #[test]
    fn quad_index_matches_a_row_model_and_widths_do_not_mix() {
        let path = std::env::temp_dir().join(format!("rdf-quad-index-{}", std::process::id()));
        let mut rows: Vec<[TermId; 4]> = Vec::new();
        for g in 1..=4u64 {
            for s in 1..=12u64 {
                for p in 1..=4u64 {
                    for o in 1..=(1 + (g * s + p) % 5) { rows.push([TermId(g * 10), TermId(s), TermId(p), TermId(o * 7)]) }
                }
            }
        }
        assert!(rows.len() > 2 * BLOCK_ROWS as usize, "spans several blocks: {}", rows.len());
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.clone().into_iter()).unwrap();
        assert_eq!(&bytes[..8], MAGIC_QUAD_V2);
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::<4>::open(&path).unwrap();
        index.validate().unwrap();
        assert_eq!(index.len(), rows.len() as u64);
        let mut prefixes: Vec<Vec<TermId>> = vec![vec![], vec![TermId(5)], vec![TermId(20), TermId(100)], vec![TermId(u64::MAX)]];
        for r in rows.iter().step_by(13) { for k in 1..=4 { prefixes.push(r[..k].to_vec()) } }
        for prefix in &prefixes {
            let want: Vec<usize> = (0..rows.len()).filter(|&i| rows[i][..prefix.len()] == prefix[..]).collect();
            let range = index.range(prefix).unwrap();
            match (want.first(), want.last()) {
                (Some(&a), Some(&b)) => assert_eq!(range, a as u64..b as u64 + 1, "{prefix:?}"),
                _ => assert!(range.is_empty(), "{prefix:?}"),
            }
            if prefix.len() < 4 {
                for target in [TermId(0), TermId(3), TermId(7), TermId(30), TermId(u64::MAX)] {
                    let expect = want.iter().map(|&i| rows[i][prefix.len()]).filter(|&v| v >= target).min();
                    assert_eq!(index.seek_value(prefix, target).unwrap(), expect, "{prefix:?} {target:?}");
                }
            }
        }
        for i in [0, 255, 256, rows.len() - 1] { assert_eq!(index.get(i as u64).unwrap(), rows[i]) }
        assert_eq!(index.block_slice(250, 260).unwrap(), rows[250..256]);
        assert!(index.range(&[TermId(1); 5]).is_err());
        // Widths do not mix: a quad file is not a triple index and vice versa.
        assert!(DiskIndex::<3>::open(&path).is_err());
        let triples: Vec<[TermId; 3]> = (1..=10).map(|i| [TermId(i), TermId(1), TermId(2)]).collect();
        let mut tbytes = Vec::new();
        write_index(&mut tbytes, triples.into_iter()).unwrap();
        std::fs::write(&path, &tbytes).unwrap();
        assert!(DiskIndex::<4>::open(&path).is_err());
        // Block corruption is detected on read.
        let mut bad = bytes.clone();
        bad[index.blocks[1].offset as usize + 5] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        let corrupt = DiskIndex::<4>::open(&path).unwrap();
        assert!(corrupt.validate().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn range_and_corruption() {
        let rows = vec![
            [TermId(1), TermId(2), TermId(3)],
            [TermId(1), TermId(3), TermId(4)],
            [TermId(2), TermId(1), TermId(5)],
        ];
        let path = std::env::temp_dir().join(format!(
            "rdf-index-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut data = Vec::new();
        write_index_v2(&mut data, rows.clone().into_iter()).unwrap();
        std::fs::write(&path, &data).unwrap();
        let index = DiskIndex::<3>::open(&path).unwrap();
        index.validate().unwrap();
        assert_eq!(index.range(&[]).unwrap(), 0..3);
        assert_eq!(index.range(&[TermId(1)]).unwrap(), 0..2);
        assert_eq!(index.range(&[TermId(1), TermId(3)]).unwrap(), 1..2);
        assert_eq!(
            index.range(&[TermId(1), TermId(3), TermId(4)]).unwrap(),
            1..2
        );
        assert_eq!(index.range(&[TermId(3)]).unwrap(), 3..3);
        assert_eq!(index.get(1).unwrap(), rows[1]);
        assert!(index.range(&[TermId(1); 4]).is_err());
        drop(index);
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(DiskIndex::<3>::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
        assert!(write_index(Vec::new(), [rows[0], rows[0]].into_iter()).is_err());
    }
}

