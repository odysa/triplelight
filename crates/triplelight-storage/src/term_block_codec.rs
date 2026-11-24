//! Front-coded RDF term blocks. Block payloads are checksummed by the caller.
use crate::model::TermId;
use triplelight_rdf::Term;
use std::io::{self, ErrorKind};
use super::base_format::MAX_TERM;
const MAX_BLOCK: usize = MAX_TERM + 8192;
fn bad(s: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, s)
}
fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 { out.push(v as u8 | 0x80); v >>= 7 }
    out.push(v as u8);
}
fn get_varint(data: &[u8], pos: &mut usize) -> io::Result<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos).ok_or_else(|| bad("truncated term header"))?;
        *pos += 1;
        if shift == 63 && b > 1 { return Err(bad("varint overflow")) }
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            // One canonical encoding per value: no trailing zero groups.
            if b == 0 && shift > 0 { return Err(bad("noncanonical varint")) }
            return Ok(v);
        }
    }
    Err(bad("varint overflow"))
}

/// Encode at most 256 ID/term pairs. The caller determines and validates order.
///
/// `lean` (`RDFDID03`, #105): each entry is `varint(ID - previous ID)` (the block's first entry: `varint(ID)`), then
/// `varint(prefix)`, `varint(suffix length)` and the suffix bytes. IDs ascend within a block, and 99.6% of entries are
/// the previous ID + 1, so the framing is usually 3 bytes instead of 16. Otherwise (`RDFDID01`/`RDFDID02`): an 8-byte
/// ID and two 4-byte lengths.
pub(super) fn encode(entries: &[(TermId, Term)], lean: bool) -> io::Result<Vec<u8>> {
    if entries.is_empty() || entries.len() > 256 {
        return Err(bad("invalid term block count"));
    }
    let mut result = Vec::new();
    result.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut previous = Vec::new();
    let mut previous_id: Option<u64> = None;
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
        if lean {
            let delta = match previous_id {
                None => id.0,
                Some(p) => id.0.checked_sub(p).filter(|d| *d > 0).ok_or_else(|| bad("term IDs not ascending in a block"))?,
            };
            put_varint(&mut result, delta);
            put_varint(&mut result, prefix as u64);
            put_varint(&mut result, suffix.len() as u64);
        } else {
            result.extend_from_slice(&id.0.to_le_bytes());
            result.extend_from_slice(&(prefix as u32).to_le_bytes());
            result.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
        }
        previous_id = Some(id.0);
        result.extend_from_slice(suffix);
        if result.len() > MAX_BLOCK {
            return Err(bad("term block too large"));
        }
        previous.clear();
        previous.extend_from_slice(text);
    }
    Ok(result)
}
/// Strictly decode a complete bounded block; no unchecked parsing of disk bytes. `lean` as for `encode`.
///
/// `canonical`: also require every term to re-serialize to exactly its stored text. That re-serializes all 256 terms
/// of a block, so the read path (block-cache misses) skips it: the block's SHA-256 and the parse already reject
/// corruption, and the text was canonical when the build wrote it. `verify` (`check_text_order`) runs it on every
/// block. If a store's text were ever noncanonical (an old or buggy writer), the read path would still decode it, but
/// lookups of a term's canonical form could miss its ID; that is why `verify` refuses such a store.
pub(super) fn decode(data: &[u8], lean: bool, canonical: bool) -> io::Result<Vec<(TermId, Term)>> {
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
    let mut previous_id: Option<u64> = None;
    for _ in 0..count {
        let (id, prefix, suffix, header) = if lean {
            let mut p = pos;
            let delta = get_varint(data, &mut p)?;
            let id = match previous_id {
                None => delta,
                Some(prev) => prev.checked_add(delta).filter(|_| delta > 0).ok_or_else(|| bad("invalid term block prefix or ID"))?,
            };
            let prefix = usize::try_from(get_varint(data, &mut p)?).map_err(|_| bad("term too large"))?;
            let suffix = usize::try_from(get_varint(data, &mut p)?).map_err(|_| bad("term too large"))?;
            (TermId(id), prefix, suffix, p)
        } else {
            let header = pos
                .checked_add(16)
                .filter(|v| *v <= data.len())
                .ok_or_else(|| bad("truncated term header"))?;
            let id = TermId(u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap()));
            let prefix = u32::from_le_bytes(data[pos + 8..pos + 12].try_into().unwrap()) as usize;
            let suffix = u32::from_le_bytes(data[pos + 12..header].try_into().unwrap()) as usize;
            (id, prefix, suffix, header)
        };
        previous_id = Some(id.0);
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
        if canonical && term.to_string() != strval {
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
        let bytes = encode(&terms, false).unwrap();
        assert_eq!(decode(&bytes, false, true).unwrap(), terms);
        for n in 0..bytes.len() {
            assert!(decode(&bytes[..n], false, true).is_err());
        }
        let mut badbytes = bytes.clone();
        badbytes[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&badbytes, false, true).is_err());
        let mut extra = bytes;
        extra.push(0);
        assert!(decode(&extra, false, true).is_err());
    }
    #[test]
    fn lean_blocks_roundtrip_shrink_and_reject_malformed_input() {
        let iri = |i: u64| -> Term { NamedNode::new(format!("http://example.org/resource/item{i:06}")).unwrap().into() };
        // Consecutive IDs, as in most blocks, plus a gap and a large final ID.
        let mut terms: Vec<(TermId, Term)> = (0..200u64).map(|i| (TermId((1 << 60) | (i + 1)), iri(i))).collect();
        terms.push((TermId((1 << 60) | 10_000), iri(10_000)));
        terms.push((TermId(u64::MAX), Literal::new_simple_literal("é and a long tail".repeat(20)).into()));
        let lean = encode(&terms, true).unwrap();
        let old = encode(&terms, false).unwrap();
        assert_eq!(decode(&lean, true, true).unwrap(), terms);
        assert_eq!(decode(&old, false, true).unwrap(), terms);
        assert!(old.len() - lean.len() >= 12 * 200, "framing shrinks by about 13 bytes per entry: {} against {}", lean.len(), old.len());
        for n in 0..lean.len() { assert!(decode(&lean[..n], true, true).is_err(), "truncated at {n}") }
        let mut extra = lean.clone();
        extra.push(0);
        assert!(decode(&extra, true, true).is_err());
        // A zero ID delta (a repeated ID) and a noncanonical varint are refused.
        let mut dup = vec![2, 0];
        for (id, _) in [(5u64, ()), (0, ())] { put_varint(&mut dup, id); put_varint(&mut dup, 0); put_varint(&mut dup, 3); dup.extend_from_slice(b"<a>") }
        assert!(decode(&dup, true, true).is_err());
        let mut pos = 0;
        assert!(get_varint(&[0x81, 0x00], &mut pos).is_err());
        assert!(encode(&[(TermId(5), iri(1)), (TermId(5), iri(2))], true).is_err(), "IDs must ascend in a lean block");
    }
    #[test]
    fn noncanonical_text_is_refused_only_when_canonical_is_checked() {
        // `"a"^^xsd:string` parses, but serializes as `"a"`: text an older or a buggy writer could have stored.
        let text = br#""a"^^<http://www.w3.org/2001/XMLSchema#string>"#;
        let mut block = vec![1, 0];
        put_varint(&mut block, (3 << 60) | 1);
        put_varint(&mut block, 0);
        put_varint(&mut block, text.len() as u64);
        block.extend_from_slice(text);
        assert!(decode(&block, true, true).is_err(), "verify refuses noncanonical text");
        let rows = decode(&block, true, false).unwrap();
        assert_eq!(rows[0].1.to_string(), "\"a\"", "the read path takes the parsed term");
        // Text that does not parse is refused either way.
        let mut bad = vec![1, 0];
        put_varint(&mut bad, 1);
        put_varint(&mut bad, 0);
        put_varint(&mut bad, 3);
        bad.extend_from_slice(b"<a ");
        assert!(decode(&bad, true, false).is_err());
    }
}

/// `RDFDID04` (#105): a one-byte codec tag, then the lean block (`encode(_, true)`): stored as is (`RAW`), or as one
/// zstd frame (`ZSTD`). The dictionary writer compresses literal blocks, whose text front coding barely shrinks
/// (`docs/literal-compression-v1/`), and only when that is smaller; IRI blocks stay raw.
pub(super) const RAW: u8 = 0;
pub(super) const ZSTD: u8 = 1;
const ZSTD_LEVEL: i32 = 3;

pub(super) fn encode_tagged(entries: &[(TermId, Term)], compress: bool) -> io::Result<Vec<u8>> {
    let lean = encode(entries, true)?;
    if compress {
        let packed = zstd::bulk::compress(&lean, ZSTD_LEVEL)?;
        if packed.len() < lean.len() {
            let mut out = Vec::with_capacity(packed.len() + 1);
            out.push(ZSTD);
            out.extend_from_slice(&packed);
            return Ok(out);
        }
    }
    let mut out = Vec::with_capacity(lean.len() + 1);
    out.push(RAW);
    out.extend_from_slice(&lean);
    Ok(out)
}

/// Decodes an `RDFDID04` block; `canonical` as for `decode`. A zstd frame may not expand past the largest lean block.
pub(super) fn decode_tagged(data: &[u8], canonical: bool) -> io::Result<Vec<(TermId, Term)>> {
    match data.split_first() {
        Some((&RAW, rest)) => decode(rest, true, canonical),
        Some((&ZSTD, rest)) => {
            let lean = zstd::bulk::decompress(rest, MAX_BLOCK).map_err(|_| bad("invalid compressed term block"))?;
            decode(&lean, true, canonical)
        }
        _ => Err(bad("invalid term block codec")),
    }
}

#[cfg(test)]
mod tagged_tests {
    use super::*;
    use triplelight_rdf::{Literal, NamedNode};
    #[test]
    fn literal_blocks_compress_round_trip_and_reject_bad_frames() {
        let words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
        let lits: Vec<(TermId, Term)> = (0..200u64)
            .map(|i| (TermId((3 << 60) | (i + 1)), Literal::new_language_tagged_literal(
                (0..40).map(|k| words[((i * 7 + k * 3) % 6) as usize]).collect::<Vec<_>>().join(" "), "en").unwrap().into()))
            .collect();
        let packed = encode_tagged(&lits, true).unwrap();
        let raw = encode_tagged(&lits, false).unwrap();
        assert_eq!(packed[0], ZSTD);
        assert_eq!(raw[0], RAW);
        assert!(packed.len() * 2 < raw.len(), "generated text compresses: {} against {}", packed.len(), raw.len());
        assert_eq!(decode_tagged(&packed, true).unwrap(), lits);
        assert_eq!(decode_tagged(&raw, true).unwrap(), lits);
        // An incompressible block is kept raw even when compression is asked for.
        let iri: Vec<(TermId, Term)> = vec![(TermId((1 << 60) | 1), NamedNode::new("urn:x").unwrap().into())];
        assert_eq!(encode_tagged(&iri, true).unwrap()[0], RAW);
        for bad in [vec![], vec![7, 1, 2], vec![ZSTD, 1, 2, 3], packed[..packed.len() - 1].to_vec()] {
            assert!(decode_tagged(&bad, true).is_err());
        }
    }
}
