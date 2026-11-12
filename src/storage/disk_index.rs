//! A fallible, positional-read disk index primitive. The file is immutable after opening.
//! This is a building block, not yet a compressed, crash-safe RDF store.
use crate::model::TermId;
use std::fs::File;
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::FileExt;
use std::path::Path;

const MAGIC: &[u8; 8] = b"RDFIDX01";
const HEADER: u64 = 16;
const ROW: u64 = 24;
fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, reason)
}

/// Streams strictly increasing keys into a single index file.
/// The caller must atomically publish the file and arrange durability.
pub fn write_index(
    mut out: impl Write,
    rows: impl ExactSizeIterator<Item = [TermId; 3]>,
) -> io::Result<()> {
    let count = rows.len() as u64;
    out.write_all(MAGIC)?;
    out.write_all(&count.to_le_bytes())?;
    let mut previous = None;
    let mut written = 0u64;
    for row in rows {
        if row.iter().any(|id| id.0 == 0) || previous.is_some_and(|p| p >= row) {
            return Err(invalid("keys must be nonzero, sorted and unique"));
        }
        for id in row {
            out.write_all(&id.0.to_le_bytes())?;
        }
        previous = Some(row);
        written += 1;
    }
    if written != count {
        return Err(invalid("iterator length changed"));
    }
    Ok(())
}

pub struct DiskIndex {
    file: File,
    rows: u64,
}
impl DiskIndex {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut header = [0; 16];
        read_exact_at(&file, &mut header, 0)?;
        if &header[..8] != MAGIC {
            return Err(invalid("bad index magic"));
        }
        let rows = u64::from_le_bytes(header[8..].try_into().unwrap());
        let expected = rows
            .checked_mul(ROW)
            .and_then(|n| n.checked_add(HEADER))
            .ok_or_else(|| invalid("index length overflow"))?;
        if file.metadata()?.len() != expected {
            return Err(invalid("index length mismatch"));
        }
        Ok(Self { file, rows })
    }
    fn row(&self, index: u64) -> io::Result<[TermId; 3]> {
        if index >= self.rows {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "index row out of bounds",
            ));
        }
        let mut bytes = [0; 24];
        read_exact_at(&self.file, &mut bytes, HEADER + index * ROW)?;
        Ok(std::array::from_fn(|i| {
            TermId(u64::from_le_bytes(
                bytes[i * 8..i * 8 + 8].try_into().unwrap(),
            ))
        }))
    }
    fn bound(&self, key: [TermId; 3], upper: bool) -> io::Result<u64> {
        let (mut lo, mut hi) = (0, self.rows);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let row = self.row(mid)?;
            if row < key || (upper && row == key) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }
    /// Finds the half-open range of matching rows, or errors for invalid prefixes or read failures.
    pub fn range(&self, prefix: &[TermId]) -> io::Result<std::ops::Range<u64>> {
        if prefix.len() > 3 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "prefix exceeds three terms",
            ));
        }
        let mut low = [TermId(0); 3];
        let mut high = [TermId(u64::MAX); 3];
        low[..prefix.len()].copy_from_slice(prefix);
        high[..prefix.len()].copy_from_slice(prefix);
        Ok(self.bound(low, false)?..self.bound(high, true)?)
    }
    /// Reads one key in stored permutation order.
    pub fn get(&self, index: u64) -> io::Result<[TermId; 3]> {
        self.row(index)
    }
    /// Exhaustively validates sort order and term IDs; call before trusting external files.
    pub fn validate(&self) -> io::Result<()> {
        let mut previous = None;
        for i in 0..self.rows {
            let row = self.row(i)?;
            if row.iter().any(|id| id.0 == 0) || previous.is_some_and(|p| p >= row) {
                return Err(invalid("unsorted, duplicate, or unbound key"));
            }
            previous = Some(row);
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
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(index.get(2).is_err()); // A post-open short read remains an error.
        drop(index);
        assert!(DiskIndex::open(&path).is_err());
        std::fs::remove_file(path).unwrap();
        assert!(write_index(Vec::new(), [rows[0], rows[0]].into_iter()).is_err());
    }
}
