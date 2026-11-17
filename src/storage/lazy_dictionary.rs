//! Prototype immutable on-disk dictionary indexes. Not yet connected to snapshots.
use super::term_block_codec;
use crate::model::TermId;
use oxrdf::Term;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, ErrorKind, Write},
    os::unix::fs::FileExt,
    path::Path,
    sync::{Arc, Mutex},
};
const TERM_MAGIC: &[u8; 8] = b"RDFDTM01";
const ID_MAGIC: &[u8; 8] = b"RDFDID01";
const LIMIT: usize = 16 * 1024 * 1024 + 8193;
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
    fn magic_v2(self) -> &'static [u8; 8] {
        match self { Self::Term => b"RDFDTM02", Self::Id => b"RDFDID02" }
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
struct Cache {
    entries: HashMap<(usize, usize), (Arc<Vec<(TermId, Term)>>, usize)>,
    order: VecDeque<(usize, usize)>,
    used: usize,
}
pub(super) struct DictionaryCache {
    shards: Vec<Mutex<Cache>>,
    shard_budget: usize,
}
impl DictionaryCache {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            shard_budget: super::disk_index::cache_budget_mb().saturating_mul(1024 * 1024) / 3 / 16,
            shards: (0..16).map(|_| Mutex::new(Cache { entries: HashMap::new(), order: VecDeque::new(), used: 0 })).collect(),
        })
    }
}
pub(super) struct LazyIndex {
    file: File,
    order: Order,
    entries: Vec<Meta>,
    identity: [u8; 32],
    compressed: bool,
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
        let compressed = if &header == order.magic() { false } else if &header == order.magic_v2() { true } else { return Err(bad("bad dictionary index magic")); };
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
                || length < 2
                || length > LIMIT
                || keylen == 0
                || keylen > LIMIT
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
            compressed,
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
        if let Some((value, _)) = cache.entries.get(&(slot, index)) {
            super::disk_index::trace_dictionary(slot, 0, 1);
            return Ok(Arc::clone(value));
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
        let rows = if self.compressed { term_block_codec::decode_v2(&bytes)? } else { term_block_codec::decode(&bytes)? };
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
        if size <= self.cache.shard_budget {
            while cache.used.saturating_add(size) > self.cache.shard_budget {
                let old = cache
                    .order
                    .pop_front()
                    .ok_or_else(|| bad("dictionary cache invariant"))?;
                if let Some((_, n)) = cache.entries.remove(&old) {
                    cache.used -= n;
                    super::disk_index::trace_dictionary(slot, 4, 1);
                }
            }
            cache.used += size;
            cache.entries.insert((slot, index), (Arc::clone(&rows), size));
            cache.order.push_back((slot, index));
        }
        Ok(rows)
    }
    pub(super) fn clear_cache(&self) -> io::Result<()> {
        for shard in &self.cache.shards {
            let mut cache = shard
                .lock()
                .map_err(|_| io::Error::other("dictionary cache lock poisoned"))?;
            cache.entries.clear();
            cache.order.clear();
            cache.used = 0;
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
    write_index_format(path, order, rows, true)
}
fn write_index_format(
    path: &Path,
    order: Order,
    rows: impl Iterator<Item = io::Result<(TermId, Term)>>,
    compressed: bool,
) -> io::Result<()> {
    let mut out =
        std::io::BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    let header = if compressed { order.magic_v2() } else { order.magic() };
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
        compressed: bool,
    ) -> io::Result<()> {
        if block.is_empty() {
            return Ok(());
        }
        let bytes = if compressed { term_block_codec::encode_v2(block)? } else { term_block_codec::encode(block)? };
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
            flush(&mut out, &mut block, order, &mut meta, &mut offset, compressed)?;
            bytes_estimate = 0;
        }
        bytes_estimate = bytes_estimate.saturating_add(estimate);
        previous = Some(key);
        block.push((id, t));
    }
    flush(&mut out, &mut block, order, &mut meta, &mut offset, compressed)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{Literal, NamedNode};
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
        let legacy_path = dir.join("id-legacy");
        let mut legacy_entries = entries.clone();
        legacy_entries.sort_unstable_by_key(|(id, _)| id.0);
        write_index_format(&legacy_path, Order::Id, legacy_entries.into_iter().map(Ok), false).unwrap();
        let legacy = LazyIndex::open(&legacy_path, Order::Id).unwrap();
        legacy.with_id(TermId(1), &mut |term| assert_eq!(term.into_owned(), Term::from(NamedNode::new("urn:t0000").unwrap()))).unwrap();
        drop(legacy);
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

/// Convert a freshly written legacy dictionary into two immutable indexes.
/// Caller owns manifest publication and scratch directory durability.
pub(super) fn convert_legacy(dir: &Path) -> io::Result<([u8; 32], [u8; 32])> {
    use super::external_sort::{ExternalSort, read_record};
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
            let value = match read_record(&mut self.input, 16 * 1024 * 1024 + 9) {
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
    let term_run = by_term.finish()?;
    let id_run = by_id.finish()?;
    let term_path = dir.join("dictionary.term");
    let id_path = dir.join("dictionary.id");
    write_index(
        &term_path,
        Order::Term,
        Rows {
            input: std::io::BufReader::new(File::open(term_run)?),
            order: Order::Term,
            done: false,
        },
    )?;
    write_index(
        &id_path,
        Order::Id,
        Rows {
            input: std::io::BufReader::new(File::open(id_run)?),
            order: Order::Id,
            done: false,
        },
    )?;
    let identities = (
        LazyIndex::open(&term_path, Order::Term)?.identity(),
        LazyIndex::open(&id_path, Order::Id)?.identity(),
    );
    drop(by_term);
    drop(by_id);
    std::fs::remove_file(dir.join("dictionary"))?;
    Ok(identities)
}
