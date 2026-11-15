//! Fallible immutable on-disk ordered index with block checksums.
//! Checksums detect accidental corruption; the manifest is not authenticated.
use crate::model::TermId;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const MAGIC_V2: &[u8; 8] = b"RDFIDX02";
const MAGIC_V3: &[u8; 8] = b"RDFIDX03";
const HEADER: u64 = 24;
const ROW: u64 = 24;
const BLOCK_ROWS: u64 = 256;
const META: u64 = 56;
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
        #[cfg(target_os = "macos")]
        if let Ok(out) = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if out.status.success() {
                if let Ok(bytes) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                    return usize::try_from(bytes / 4 / (1024 * 1024))
                        .unwrap_or(256)
                        .max(1);
                }
            }
        }
        256
    })
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, reason)
}
fn decode_row(row: &[u8]) -> [TermId; 3] {
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
pub fn write_index(
    out: impl Write,
    rows: impl ExactSizeIterator<Item = [TermId; 3]>,
) -> io::Result<()> {
    let count = rows.len() as u64;
    write_index_stream(out, count, rows.map(Ok))
}

/// Write a sorted stream whose exact row count is known; propagate source read errors.
pub fn write_index_stream(
    mut out: impl Write,
    count: u64,
    rows: impl Iterator<Item = io::Result<[TermId; 3]>>,
) -> io::Result<()> {
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(MAGIC_V3);
    header[8..16].copy_from_slice(&count.to_le_bytes());
    header[16..].copy_from_slice(&BLOCK_ROWS.to_le_bytes());
    out.write_all(&header)?;
    let mut meta = Vec::new();
    let mut block = Vec::with_capacity(BLOCK_ROWS as usize);
    let mut previous = None;
    let mut written = 0u64;
    let flush =
        |block: &mut Vec<[TermId; 3]>, out: &mut dyn Write, meta: &mut Vec<u8>| -> io::Result<()> {
            if block.is_empty() {
                return Ok(());
            }
            let bytes = super::block_codec::encode(block)?;
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
struct BlockMeta {
    last: [TermId; 3],
    checksum: [u8; 32],
    offset: u64,
    length: usize,
}
struct CacheShard {
    entries: HashMap<u64, (Arc<Vec<[TermId; 3]>>, bool)>,
    clock: VecDeque<u64>,
}
pub struct DiskIndex {
    file: File,
    rows: u64,
    identity: [u8; 32],
    blocks: Vec<BlockMeta>,
    compressed: bool,
    cache: Vec<Mutex<CacheShard>>,
    shard_capacity: usize,
    cache_epoch: AtomicU64,
}
impl DiskIndex {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut header = [0; 24];
        read_exact_at(&file, &mut header, 0)?;
        let compressed = match &header[..8] {
            m if m == MAGIC_V2 => false,
            m if m == MAGIC_V3 => true,
            _ => return Err(invalid("bad index magic or version")),
        };
        let rows = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if u64::from_le_bytes(header[16..24].try_into().unwrap()) != BLOCK_ROWS {
            return Err(invalid("bad block size"));
        }
        let blocks = rows.div_ceil(BLOCK_ROWS);
        let entry_size = if compressed { 64 } else { META };
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
                        rows.checked_mul(ROW)
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
            let last = decode_row(&item[..24]);
            let checksum = item[24..56].try_into().unwrap();
            if last.iter().any(|x| x.0 == 0)
                || entries.last().is_some_and(|b: &BlockMeta| b.last >= last)
            {
                return Err(invalid("invalid block directory"));
            }
            let len = if compressed {
                u64::from_le_bytes(item[56..64].try_into().unwrap())
            } else {
                (rows - (i as u64) * BLOCK_ROWS).min(BLOCK_ROWS) * ROW
            };
            if compressed && (len < 27 || len > 27 + BLOCK_ROWS * ROW) {
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
            file,
            rows,
            cache_epoch: AtomicU64::new(0),
            identity: footer,
            blocks: entries,
            compressed,
            cache: (0..16)
                .map(|_| {
                    Mutex::new(CacheShard {
                        entries: HashMap::new(),
                        clock: VecDeque::new(),
                    })
                })
                .collect(),
            shard_capacity: cache_budget_mb()
                .saturating_mul(1024 * 1024)
                .checked_div(8 * 16 * BLOCK_ROWS as usize * ROW as usize)
                .unwrap_or(1)
                .max(1),
        })
    }
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn clear_cache(&self) -> io::Result<()> {
        self.cache_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| io::Error::other("disk cache epoch exhausted"))?;
        for shard in &self.cache {
            let mut cache = shard
                .lock()
                .map_err(|_| io::Error::other("index cache lock poisoned"))?;
            cache.entries.clear();
            cache.clock.clear();
        }
        Ok(())
    }
    pub fn len(&self) -> u64 {
        self.rows
    }
    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }
    fn block(&self, id: u64) -> io::Result<Arc<Vec<[TermId; 3]>>> {
        let mut cache = self.cache[(id as usize) % self.cache.len()]
            .lock()
            .map_err(|_| io::Error::other("index cache lock poisoned"))?;
        if let Some((value, referenced)) = cache.entries.get_mut(&id) {
            *referenced = true;
            return Ok(Arc::clone(value));
        }
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
        read_exact_at(&self.file, &mut bytes, meta.offset)?;
        if Sha256::digest(&bytes).as_slice() != meta.checksum {
            return Err(invalid("index block checksum mismatch"));
        }
        let rows: Vec<_> = if self.compressed {
            super::block_codec::decode(&bytes, n)?
        } else {
            bytes.chunks_exact(24).map(decode_row).collect()
        };
        if rows.iter().any(|r| r.iter().any(|id| id.0 == 0))
            || rows.windows(2).any(|w| w[0] >= w[1])
            || rows.last() != Some(&meta.last)
            || (id > 0 && rows[0] <= self.blocks[id as usize - 1].last)
        {
            return Err(invalid("invalid index block ordering"));
        }
        let r = Arc::new(rows);
        while cache.entries.len() >= self.shard_capacity {
            let old = cache.clock.pop_front().expect("cache clock consistent");
            let entry = cache.entries.get_mut(&old).expect("cache entry consistent");
            if entry.1 {
                entry.1 = false;
                cache.clock.push_back(old);
            } else {
                cache.entries.remove(&old);
            }
        }
        cache.entries.insert(id, (Arc::clone(&r), true));
        cache.clock.push_back(id);
        Ok(r)
    }
    fn row(&self, i: u64) -> io::Result<[TermId; 3]> {
        if i >= self.rows {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "index row out of bounds",
            ));
        }
        Ok(self.block(i / BLOCK_ROWS)?[(i % BLOCK_ROWS) as usize])
    }
    fn bound(&self, key: [TermId; 3], upper: bool) -> io::Result<u64> {
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
        if prefix.len() > 3 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "prefix exceeds three terms",
            ));
        }
        let (mut low, mut high) = ([TermId(0); 3], [TermId(u64::MAX); 3]);
        low[..prefix.len()].copy_from_slice(prefix);
        high[..prefix.len()].copy_from_slice(prefix);
        Ok(self.bound(low, false)?..self.bound(high, true)?)
    }
    /// Copies at most one decoded block from the given position.
    pub fn block_slice(&self, start: u64, end: u64) -> io::Result<Vec<[TermId; 3]>> {
        if start >= end || end > self.rows {
            return Err(invalid("invalid block slice"));
        }
        let rows = self.block(start / BLOCK_ROWS)?;
        let from = (start % BLOCK_ROWS) as usize;
        let count = (end - start).min(BLOCK_ROWS - start % BLOCK_ROWS) as usize;
        Ok(rows[from..from + count].to_vec())
    }
    pub fn cursor(&self, prefix: &[TermId]) -> DiskValueCursor<'_> {
        let mut fixed = [TermId(0); 3];
        let valid = prefix.len() < 3;
        if valid {
            fixed[..prefix.len()].copy_from_slice(prefix);
        }
        DiskValueCursor {
            index: self,
            prefix: fixed,
            prefix_len: prefix.len(),
            valid,
            current: None,
        }
    }
    pub fn seek_value(&self, prefix: &[TermId], target: TermId) -> io::Result<Option<TermId>> {
        if prefix.len() >= 3 {
            return Ok(None);
        }
        let mut key = [TermId(0); 3];
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
    pub fn get(&self, i: u64) -> io::Result<[TermId; 3]> {
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
/// Cursor holds at most one decoded block; cache clear revokes the held view.
pub struct DiskValueCursor<'a> {
    index: &'a DiskIndex,
    prefix: [TermId; 3],
    prefix_len: usize,
    valid: bool,
    current: Option<(usize, u64, Arc<Vec<[TermId; 3]>>)>,
}
impl crate::storage::ValueCursor for DiskValueCursor<'_> {
    fn seek(&mut self, target: TermId) -> io::Result<Option<TermId>> {
        if !self.valid {
            return Ok(None);
        }
        let mut key = self.prefix;
        key[self.prefix_len] = target;
        let epoch = self.index.cache_epoch.load(Ordering::Acquire);
        let retained = self.current.as_ref().is_some_and(|(id, e, _)| {
            *e == epoch
                && self.index.blocks[*id].last >= key
                && (*id == 0 || self.index.blocks[*id - 1].last < key)
        });
        if !retained {
            let id = self.index.blocks.partition_point(|m| m.last < key);
            if id == self.index.blocks.len() {
                self.current = None;
                return Ok(None);
            }
            self.current = Some((id, epoch, self.index.block(id as u64)?));
        }
        let rows = &self.current.as_ref().expect("cursor block").2;
        let pos = rows.partition_point(|r| *r < key);
        let Some(row) = rows.get(pos) else {
            return Ok(None);
        };
        if row[..self.prefix_len] == self.prefix[..self.prefix_len] {
            Ok(Some(row[self.prefix_len]))
        } else {
            Ok(None)
        }
    }
}
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match file.read_at(buf, offset) {
            Ok(0) => return Err(io::Error::new(ErrorKind::UnexpectedEof, "short index read")),
            Ok(n) => {
                offset += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
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
    fn cursor_matches_stateless_seek_and_rechecks_after_cache_clear() {
        use crate::storage::ValueCursor;
        let path = std::env::temp_dir().join(format!("rdf-cursor-{}", std::process::id()));
        let rows: Vec<_> = (0..800u64)
            .map(|i| [TermId(i / 4 + 1), TermId(i % 4 + 1), TermId(7)])
            .collect();
        let mut bytes = Vec::new();
        write_index(&mut bytes, rows.into_iter()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::open(&path).unwrap();
        for prefix in [
            vec![],
            vec![TermId(1)],
            vec![TermId(64)],
            vec![TermId(200)],
            vec![TermId(201)],
        ] {
            let mut cursor = index.cursor(&prefix);
            for target in [0, 1, 2, 63, 64, 65, 199, 200, 201, u64::MAX, 1, 64] {
                assert_eq!(
                    cursor.seek(TermId(target)).unwrap(),
                    index.seek_value(&prefix, TermId(target)).unwrap()
                );
            }
        }
        let mut cursor = index.cursor(&[]);
        assert!(cursor.seek(TermId(1)).unwrap().is_some());
        bytes[24] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        index.clear_cache().unwrap();
        assert!(cursor.seek(TermId(1)).is_err());
        drop(cursor);
        drop(index);
        std::fs::remove_file(path).unwrap();
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
        let index = DiskIndex::open(&path).unwrap();
        for n in [1, 255, 256, 257, 512, 513, 700, 701] {
            let start = ((n - 1) as u64).min(700);
            assert_eq!(
                index.range(&[TermId(n)]).unwrap(),
                start..(start + u64::from(n <= 700))
            );
        }
        // Cached blocks remain usable by design; a miss after external truncation reports an error.
        drop(index);
        std::fs::write(&path, &bytes).unwrap();
        let index = DiskIndex::open(&path).unwrap();
        let cut = index.blocks[2].offset as usize + index.blocks[2].length / 2;
        std::fs::write(&path, &bytes[..cut]).unwrap();
        assert!(index.get(699).is_err());
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
        let index = DiskIndex::open(&path).unwrap();
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
        assert!(DiskIndex::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
        assert!(write_index(Vec::new(), [rows[0], rows[0]].into_iter()).is_err());
    }
}
