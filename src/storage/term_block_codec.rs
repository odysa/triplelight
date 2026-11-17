//! Front-coded RDF term blocks. Block payloads are checksummed by the caller.
use crate::model::TermId;
use oxrdf::Term;
use std::io::{self, ErrorKind};
const MAX_TERM: usize = 16 * 1024 * 1024;
const MAX_BLOCK: usize = MAX_TERM + 8192;
fn bad(s: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, s)
}
/// Encode at most 256 ID/term pairs. The caller determines and validates order.
pub(super) fn encode(entries: &[(TermId, Term)]) -> io::Result<Vec<u8>> {
    if entries.is_empty() || entries.len() > 256 {
        return Err(bad("invalid term block count"));
    }
    let mut result = Vec::new();
    result.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut previous = Vec::new();
    for (id, term) in entries {
        if id.0 == 0 {
            return Err(bad("zero term ID"));
        }
        let text = term.to_string();
        let text = text.as_bytes();
        if text.len() > MAX_TERM {
            return Err(bad("term too large"));
        }
        let prefix = text
            .iter()
            .zip(&previous)
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = &text[prefix..];
        result.extend_from_slice(&id.0.to_le_bytes());
        result.extend_from_slice(&(prefix as u32).to_le_bytes());
        result.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
        result.extend_from_slice(suffix);
        if result.len() > MAX_BLOCK {
            return Err(bad("term block too large"));
        }
        previous.clear();
        previous.extend_from_slice(text);
    }
    Ok(result)
}
/// Version-2 payload: tagged raw or LZ4-compressed version-1 bytes.
pub(super) fn encode_v2(entries: &[(TermId, Term)]) -> io::Result<Vec<u8>> {
    let plain = encode(entries)?;
    let compressed = lz4_flex::block::compress_prepend_size(&plain);
    let mut result = Vec::new();
    if compressed.len() < plain.len() {
        result.push(1);
        result.extend_from_slice(&compressed);
    } else {
        result.push(0);
        result.extend_from_slice(&plain);
    }
    Ok(result)
}
pub(super) fn decode_v2(data: &[u8]) -> io::Result<Vec<(TermId, Term)>> {
    match data.split_first() {
        Some((0, plain)) => decode(plain),
        Some((1, compressed)) if compressed.len() >= 4 => {
            let expected = u32::from_le_bytes(compressed[..4].try_into().unwrap()) as usize;
            if expected > MAX_BLOCK || expected < 2 { return Err(bad("invalid decompressed term block size")); }
            let plain = lz4_flex::block::decompress_size_prepended(compressed).map_err(|_| bad("invalid compressed term block"))?;
            if plain.len() != expected { return Err(bad("decompressed term block size mismatch")); }
            decode(&plain)
        }
        _ => Err(bad("invalid compressed term block tag")),
    }
}
/// Strictly decode a complete bounded block; no unchecked parsing of disk bytes.
pub(super) fn decode(data: &[u8]) -> io::Result<Vec<(TermId, Term)>> {
    if data.len() > MAX_BLOCK {
        return Err(bad("term block too large"));
    }
    if data.len() < 2 {
        return Err(bad("truncated term block"));
    }
    let count = u16::from_le_bytes(data[..2].try_into().unwrap()) as usize;
    if count == 0 || count > 256 {
        return Err(bad("invalid term block count"));
    }
    let mut pos = 2usize;
    let mut previous = Vec::new();
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        let header = pos
            .checked_add(16)
            .filter(|v| *v <= data.len())
            .ok_or_else(|| bad("truncated term header"))?;
        let id = TermId(u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()));
        let prefix = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
        let suffix = u32::from_le_bytes(data[pos + 12..header].try_into().unwrap()) as usize;
        let end = header
            .checked_add(suffix)
            .filter(|v| *v <= data.len())
            .ok_or_else(|| bad("truncated term suffix"))?;
        let length = prefix
            .checked_add(suffix)
            .filter(|v| *v <= MAX_TERM)
            .ok_or_else(|| bad("term too large"))?;
        if id.0 == 0 || prefix > previous.len() {
            return Err(bad("invalid term block prefix or ID"));
        }
        let mut text = Vec::with_capacity(length);
        text.extend_from_slice(&previous[..prefix]);
        text.extend_from_slice(&data[header..end]);
        let strval = std::str::from_utf8(&text).map_err(|_| bad("invalid term UTF-8"))?;
        let term: Term = strval.parse().map_err(|_| bad("invalid RDF term"))?;
        if term.to_string() != strval {
            return Err(bad("noncanonical RDF term"));
        }
        result.push((id, term));
        previous = text;
        pos = end;
    }
    if pos != data.len() {
        return Err(bad("trailing term block bytes"));
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::{BlankNode, Literal, NamedNode};
    #[test]
    fn compressed_and_raw_payloads_reject_corruption() {
        let repeated: Vec<_> = (1..=200).map(|i| {
            (TermId(i), NamedNode::new(format!("urn:prefix:{}:{i:04}", "repeated".repeat(20))).unwrap().into())
        }).collect();
        let packed = encode_v2(&repeated).unwrap();
        assert_eq!(packed[0], 1);
        assert_eq!(decode_v2(&packed).unwrap(), repeated);
        assert!(decode_v2(&packed[..packed.len()-1]).is_err());
        assert!(decode_v2(&[1, 255, 255, 255, 255]).is_err());
        assert!(decode_v2(&[0, 1]).is_err());
        assert!(decode_v2(&[255]).is_err());
        let tiny = vec![(TermId(1), NamedNode::new("urn:x").unwrap().into())];
        let raw = encode_v2(&tiny).unwrap();
        assert_eq!(decode_v2(&raw).unwrap(), tiny);
    }
    #[test]
    fn terms_roundtrip_and_malformed_blocks_fail() {
        let terms = vec![
            (TermId(1), NamedNode::new("urn:é").unwrap().into()),
            (TermId(2), BlankNode::new("node1").unwrap().into()),
            (
                TermId(3),
                Literal::new_language_tagged_literal("hello", "en")
                    .unwrap()
                    .into(),
            ),
            (
                TermId(u64::MAX),
                Literal::new_simple_literal("hello world").into(),
            ),
        ];
        let packed = encode_v2(&terms).unwrap();
        assert_eq!(decode_v2(&packed).unwrap(), terms);
        assert!(decode_v2(&[1, 255, 255, 255, 255]).is_err());
        assert!(decode_v2(&[2]).is_err());
        let bytes = encode(&terms).unwrap();
        assert_eq!(decode(&bytes).unwrap(), terms);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n]).is_err());
        }
        let mut badbytes = bytes.clone();
        badbytes[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&badbytes).is_err());
        let mut extra = bytes;
        extra.push(0);
        assert!(decode(&extra).is_err());
    }
}
