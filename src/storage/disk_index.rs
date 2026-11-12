//! Fallible immutable on-disk ordered index with block checksums.
//! Checksums detect accidental corruption; the manifest is not authenticated.
use crate::model::TermId;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 8] = b"RDFIDX02";
const HEADER: u64 = 24;
const ROW: u64 = 24;
const BLOCK_ROWS: u64 = 256;
const META: u64 = 56;

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
pub fn write_index(
    mut out: impl Write,
    rows: impl ExactSizeIterator<Item = [TermId; 3]>,
) -> io::Result<()> {
    let count = rows.len() as u64;
    let mut header = [0u8; 24];
    header[..8].copy_from_slice(MAGIC);
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
#[derive(Clone, Copy)]
struct BlockMeta {
    last: [TermId; 3],
    checksum: [u8; 32],
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
    cache: Vec<Mutex<CacheShard>>,
    shard_capacity: usize,
}
impl DiskIndex {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut header = [0; 24];
        read_exact_at(&file, &mut header, 0)?;
        if &header[..8] != MAGIC {
            return Err(invalid("bad index magic or version"));
        }
        let rows = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if u64::from_le_bytes(header[16..24].try_into().unwrap()) != BLOCK_ROWS {
            return Err(invalid("bad block size"));
        }
        let blocks = rows.div_ceil(BLOCK_ROWS);
        let data = rows
            .checked_mul(ROW)
            .ok_or_else(|| invalid("index length overflow"))?;
        let meta = blocks
            .checked_mul(META)
            .ok_or_else(|| invalid("index length overflow"))?;
        let offset = HEADER
            .checked_add(data)
            .ok_or_else(|| invalid("index length overflow"))?;
        let expected = offset
            .checked_add(meta)
            .and_then(|v| v.checked_add(32))
            .ok_or_else(|| invalid("index length overflow"))?;
        if file.metadata()?.len() != expected {
            return Err(invalid("index length mismatch"));
        }
        let size = usize::try_from(meta).map_err(|_| invalid("block directory too large"))?;
        let mut raw = vec![0u8; size];
        read_exact_at(&file, &mut raw, offset)?;
        let mut footer = [0; 32];
        read_exact_at(&file, &mut footer, offset + meta)?;
        let mut check = Sha256::new();
        check.update(header);
        check.update(&raw);
        if check.finalize().as_slice() != footer {
            return Err(invalid("index header/directory checksum mismatch"));
        }
        let mut entries = Vec::new();
        for item in raw.chunks_exact(META as usize) {
            let last = decode_row(&item[..24]);
            let checksum = item[24..].try_into().unwrap();
            if last.iter().any(|x| x.0 == 0)
                || entries.last().is_some_and(|b: &BlockMeta| b.last >= last)
            {
                return Err(invalid("invalid block directory"));
            }
            entries.push(BlockMeta { last, checksum });
        }
        Ok(Self {
            file,
            rows,
            identity: footer,
            blocks: entries,
            cache: (0..16).map(|_| Mutex::new(CacheShard { entries: HashMap::new(), clock: VecDeque::new() })).collect(),
            shard_capacity: std::env::var("RDFDB_CACHE_MB").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(256).saturating_mul(1024 * 1024).checked_div(6 * 16 * BLOCK_ROWS as usize * ROW as usize).unwrap_or(1).max(1),
        })
    }
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }
    pub fn clear_cache(&self) -> io::Result<()> {
        for shard in &self.cache {
            let mut cache = shard.lock().map_err(|_| io::Error::other("index cache lock poisoned"))?;
            cache.entries.clear(); cache.clock.clear();
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
            .lock().map_err(|_| io::Error::other("index cache lock poisoned"))?;
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
        let mut bytes = vec![0; n * ROW as usize];
        read_exact_at(&self.file, &mut bytes, HEADER + start * ROW)?;
        let meta = self
            .blocks
            .get(id as usize)
            .ok_or_else(|| invalid("missing block directory entry"))?;
        if Sha256::digest(&bytes).as_slice() != meta.checksum {
            return Err(invalid("index block checksum mismatch"));
        }
        let rows: Vec<_> = bytes.chunks_exact(24).map(decode_row).collect();
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
            if entry.1 { entry.1 = false; cache.clock.push_back(old); }
            else { cache.entries.remove(&old); }
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
        if start >= end || end > self.rows { return Err(invalid("invalid block slice")); }
        let rows = self.block(start / BLOCK_ROWS)?;
        let from = (start % BLOCK_ROWS) as usize;
        let count = (end - start).min(BLOCK_ROWS - start % BLOCK_ROWS) as usize;
        Ok(rows[from..from + count].to_vec())
    }
    pub fn seek_value(&self, prefix: &[TermId], target: TermId) -> io::Result<Option<TermId>> {
        if prefix.len() >= 3 { return Ok(None); }
        let range = self.range(prefix)?;
        let mut key = [TermId(0); 3];
        key[..prefix.len()].copy_from_slice(prefix);
        key[prefix.len()] = target;
        let pos = self.bound(key, false)?;
        if pos >= range.end { Ok(None) } else { Ok(Some(self.get(pos)?[prefix.len()])) }
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
        std::fs::write(&path, &bytes[..24 + 699 * 24]).unwrap();
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
        write_index(&mut data, rows.clone().into_iter()).unwrap();
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
