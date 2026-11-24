//! Explicit, disposable literal postings bound to an immutable base generation.
use super::{Store, text_postings::{HEADER_BYTES, MAGIC, MAX_ANALYZED_BYTES}};
use crate::{DiskSnapshot, external_sort::ExternalSort};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Seek, SeekFrom, Write},
    path::Path,
};
use triplelight_core::text::{ANALYZER_ID, AnalyzedText};
use triplelight_rdf::vocab::{rdf, xsd};

const MIB: usize = 1024 * 1024;

impl Store {
    /// Builds a disposable text index for the currently published disk base.
    /// Returns the number of indexed tokens. A concurrent compaction discards the candidate.
    /// The budget controls sorter buffers and the maximum size of each normalization buffer.
    pub fn build_text_index(&self, memory_mb: usize) -> io::Result<u64> {
        let budget = memory_mb.checked_mul(MIB).filter(|bytes| *bytes != 0)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid text index build budget"))?;
        let literal_limit = (budget / 16).min(MAX_ANALYZED_BYTES);
        let sort_budget = budget - budget / 4;
        let (generation, _) = self.generation_snapshot();
        let (root, manifest, snapshot) = self
            .with_generation(generation, |root| -> io::Result<_> {
                let base = root.join(format!("base-{generation}"));
                let snapshot = DiskSnapshot::open(base)?;
                Ok((root.to_path_buf(), snapshot.manifest_identity(), snapshot))
            })
            .ok_or_else(generation_changed)??;
        let mut sort = ExternalSort::new(&root, sort_budget, literal_limit + 9)?;
        snapshot.visit_unified_literals(&mut |id, literal| {
            let kind = literal.datatype();
            if kind != xsd::STRING && kind != rdf::LANG_STRING {
                return Ok(());
            }
            let words = AnalyzedText::bounded(literal.value(), literal_limit)
                .ok_or_else(|| io::Error::other("literal exceeds text index analysis buffer"))?;
            for token in words.tokens() {
                if token.as_bytes().contains(&0) {
                    return Err(io::Error::other("invalid NUL in analyzed token"));
                }
                let mut key = Vec::with_capacity(token.len() + 9);
                key.extend_from_slice(token.as_bytes());
                key.push(0);
                key.extend_from_slice(&id.0.to_be_bytes());
                sort.push(key)?;
            }
            Ok(())
        })?;

        let file = sort.scratch_dir().join("text-index");
        let n = write_index(&mut sort, &file, generation, manifest)?;
        self.with_generation(generation, |root| {
            fs::rename(&file, root.join(format!("text-{generation}.v1")))?;
            File::open(root)?.sync_all()
        })
        .ok_or_else(generation_changed)??;
        Ok(n)
    }
}

fn generation_changed() -> io::Error {
    io::Error::other("the base generation changed during the text index build")
}

struct Token {
    key: Vec<u8>,
    offset: u64,
    last_id: u64,
    count: u64,
}

impl Token {
    fn append(&mut self, id: u64, postings: &mut BufWriter<File>) -> io::Result<()> {
        if id == self.last_id { return Ok(()); }
        let mut delta = id - self.last_id;
        while delta >= 128 {
            postings.write_all(&[((delta & 127) as u8) | 128])?;
            delta >>= 7;
        }
        postings.write_all(&[delta as u8])?;
        self.last_id = id;
        self.count += 1;
        Ok(())
    }

    fn finish(self, directory: &mut BufWriter<File>, offsets: &mut BufWriter<File>) -> io::Result<()> {
        offsets.write_all(&directory.stream_position()?.to_le_bytes())?;
        directory.write_all(&self.offset.to_le_bytes())?;
        directory.write_all(&self.count.to_le_bytes())?;
        directory.write_all(&self.key)
    }
}

fn write_index(sort: &mut ExternalSort, path: &Path, generation: u64, manifest: [u8; 32]) -> io::Result<u64> {
    let mut payload = BufWriter::new(OpenOptions::new().read(true).write(true).create_new(true).open(path)?);
    payload.write_all(&[0; HEADER_BYTES as usize])?;
    let directory_path = sort.scratch_dir().join("text-tokens");
    let offsets_path = sort.scratch_dir().join("text-offsets");
    let mut directory = BufWriter::new(File::create(&directory_path)?);
    let mut offsets = BufWriter::new(File::create(&offsets_path)?);

    let mut token: Option<Token> = None;
    let mut tokens = 0u64;
    let mut records = sort.finish_stream()?;
    while let Some(key) = records.next_record()? {
        let key_end = key.len() - 9;
        let id = u64::from_be_bytes(key[key_end + 1..].try_into().unwrap());
        let changed = token.as_ref().is_none_or(|current| current.key != key[..key_end]);
        if changed {
            if let Some(previous) = token.take() {
                previous.finish(&mut directory, &mut offsets)?;
            }
            let offset = payload.stream_position()? - HEADER_BYTES;
            token = Some(Token { key: key[..key_end].to_vec(), offset, last_id: 0, count: 0 });
            tokens += 1;
        }
        token.as_mut().expect("current token exists").append(id, &mut payload)?;
    }
    if let Some(last) = token {
        last.finish(&mut directory, &mut offsets)?;
    }

    let postings_bytes = payload.stream_position()? - HEADER_BYTES;
    let directory_bytes = directory.stream_position()?;
    drop(directory);
    offsets.flush()?;
    drop(offsets);
    io::copy(&mut File::open(directory_path)?, &mut payload)?;
    io::copy(&mut File::open(offsets_path)?, &mut payload)?;
    payload.flush()?;
    let mut file = payload.into_inner().map_err(|e| e.into_error())?;
    file.seek(SeekFrom::Start(HEADER_BYTES))?;
    let mut reader = BufReader::new(&mut file);
    let mut digest = Sha256::new();
    loop {
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() { break; }
        digest.update(bytes);
        let count = bytes.len();
        reader.consume(count);
    }

    file.seek(SeekFrom::Start(0))?;
    file.write_all(MAGIC)?;
    file.write_all(&generation.to_le_bytes())?;
    file.write_all(&ANALYZER_ID.to_le_bytes())?;
    file.write_all(&manifest)?;
    file.write_all(&postings_bytes.to_le_bytes())?;
    file.write_all(&directory_bytes.to_le_bytes())?;
    file.write_all(&tokens.to_le_bytes())?;
    file.write_all(&digest.finalize())?;
    file.sync_all()?;
    Ok(tokens)
}
