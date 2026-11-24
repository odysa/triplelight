//! Read-only literal postings. The manifest and payload hashes fence the index from RDF data.
use crate::model::{TermId, id_tag};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    os::unix::fs::FileExt,
    path::Path,
};
use triplelight_core::text::ANALYZER_ID;

pub(super) const MAGIC: &[u8; 8] = b"TLTEXT01";
pub(super) const HEADER_BYTES: u64 = 112;
pub(super) const MAX_ANALYZED_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct TextIndex {
    file: File,
    postings_bytes: u64,
    directory_bytes: u64,
    tokens: u64,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid text index")
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("word in text index"))
}

impl TextIndex {
    pub(crate) fn open(base: &Path, manifest: &[u8; 32]) -> io::Result<Self> {
        let generation: u64 = base.file_name().and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("base-")).and_then(|s| s.parse().ok())
            .ok_or_else(invalid)?;
        let root = base.parent().ok_or_else(invalid)?;
        let mut file = File::open(root.join(format!("text-{generation}.v1")))?;
        let mut header = [0; HEADER_BYTES as usize];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC || u64_at(&header, 8) != generation || u64_at(&header, 16) != ANALYZER_ID {
            return Err(invalid());
        }
        if header[24..56] != manifest[..] {
            return Err(invalid());
        }
        let postings_bytes = u64_at(&header, 56);
        let directory_bytes = u64_at(&header, 64);
        let tokens = u64_at(&header, 72);
        let length = tokens.checked_mul(8)
            .and_then(|n| n.checked_add(directory_bytes))
            .and_then(|n| n.checked_add(postings_bytes))
            .and_then(|n| n.checked_add(HEADER_BYTES))
            .ok_or_else(invalid)?;
        if file.metadata()?.len() != length || tokens > directory_bytes / 17 {
            return Err(invalid());
        }
        let mut reader = BufReader::new(&mut file);
        let mut digest = Sha256::new();
        loop {
            let bytes = reader.fill_buf()?;
            if bytes.is_empty() { break; }
            digest.update(bytes);
            let count = bytes.len();
            reader.consume(count);
        }
        if header[80..112] != digest.finalize()[..] {
            return Err(invalid());
        }
        Ok(Self { file, postings_bytes, directory_bytes, tokens })
    }

    fn directory_offset(&self, i: u64) -> io::Result<u64> {
        if i == self.tokens { return Ok(self.directory_bytes); }
        let offset = HEADER_BYTES + self.postings_bytes + self.directory_bytes + i * 8;
        let mut value = [0; 8];
        self.file.read_exact_at(&mut value, offset)?;
        Ok(u64::from_le_bytes(value))
    }

    fn find(&self, token: &str) -> io::Result<Option<(u64, u64)>> {
        let mut lo = 0;
        let mut hi = self.tokens;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let start = self.directory_offset(mid)?;
            let end = self.directory_offset(mid + 1)?;
            let len = end.checked_sub(start).filter(|len| (17..=MAX_ANALYZED_BYTES as u64 + 16).contains(len))
                .ok_or_else(invalid)?;
            if end > self.directory_bytes { return Err(invalid()); }
            let offset = HEADER_BYTES + self.postings_bytes + start;
            let compare_len = (len as usize - 16).min(token.len());
            let mut buffer = [0; 1024];
            let mut compared = Ordering::Equal;
            for (i, piece) in token.as_bytes()[..compare_len].chunks(buffer.len()).enumerate() {
                self.file.read_exact_at(&mut buffer[..piece.len()], offset + 16 + i as u64 * 1024)?;
                compared = buffer[..piece.len()].cmp(piece);
                if compared != Ordering::Equal { break; }
            }
            match compared.then((len as usize - 16).cmp(&token.len())) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => {
                    let mut entry = [0; 16];
                    self.file.read_exact_at(&mut entry, offset)?;
                    let start = u64_at(&entry, 0);
                    let count = u64_at(&entry, 8);
                    if start >= self.postings_bytes || count == 0 || count > self.postings_bytes - start {
                        return Err(invalid());
                    }
                    return Ok(Some((start, count)));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn candidates(&self, tokens: &[&str], limit: usize) -> io::Result<Vec<TermId>> {
        let mut result: Option<Vec<TermId>> = None;
        for token in tokens {
            let Some((start, count)) = self.find(token)? else { return Ok(Vec::new()); };
            if count > limit as u64 { return Err(invalid()); }
            let ids = self.read_postings(start, count as usize)?;
            match &mut result {
                None => result = Some(ids),
                Some(result) => {
                    result.retain(|id| ids.binary_search(id).is_ok());
                    if result.is_empty() { return Ok(Vec::new()); }
                }
            }
        }
        Ok(result.unwrap_or_default())
    }

    fn read_postings(&self, start: u64, count: usize) -> io::Result<Vec<TermId>> {
        // Each read has its own logical offset. Cloned Unix File handles share their seek position.
        // Pread one bounded byte range for this list, then decode the first count values.
        let bytes = (self.postings_bytes - start).min((count as u64).saturating_mul(10));
        let mut buffer = vec![0; bytes as usize];
        self.file.read_exact_at(&mut buffer, HEADER_BYTES + start)?;
        let mut input = buffer.into_iter();
        let mut ids = Vec::with_capacity(count);
        let mut previous = 0u64;
        for _ in 0..count {
            let mut delta = 0u64;
            for shift in (0..=63).step_by(7) {
                let byte = input.next().ok_or_else(invalid)?;
                if shift == 63 && byte > 1 { return Err(invalid()); }
                delta |= u64::from(byte & 127) << shift;
                if byte < 128 { break; }
            }
            if delta == 0 { return Err(invalid()); }
            previous = previous.checked_add(delta).ok_or_else(invalid)?;
            if previous >> 60 != id_tag::LITERAL { return Err(invalid()); }
            ids.push(TermId(previous));
        }
        Ok(ids)
    }
}
