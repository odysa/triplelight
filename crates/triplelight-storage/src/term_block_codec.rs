//! Front-coded RDF term blocks. Block payloads are checksummed by the caller.
use crate::model::TermId;
use triplelight_rdf::Term;
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
    use triplelight_rdf::{BlankNode, Literal, NamedNode};
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
