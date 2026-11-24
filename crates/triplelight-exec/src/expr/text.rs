//! The versioned Unicode analyzer shared by literal matching and future postings.
use std::cmp::Ordering;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

pub(crate) const MATCHES_IRI: &str = "urn:triplelight:text:matches";

const FOLD_RANGES: &[[u8; 13]] = include_bytes!("text_data/fold_ranges.bin").as_chunks::<13>().0;
const FOLD_UTF8: &[u8] = include_bytes!("text_data/fold_utf8.bin");
const WORD_RANGES: &[[u8; 8]] = include_bytes!("text_data/word_ranges.bin").as_chunks::<8>().0;

fn word_at(bytes: &[u8], start: usize) -> u32 {
    u32::from_le_bytes(bytes[start..start + 4].try_into().expect("complete Unicode table word"))
}

fn in_range(record: &[u8], point: u32) -> Ordering {
    if word_at(record, 4) < point {
        Ordering::Less
    } else if word_at(record, 0) > point {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut folded = String::with_capacity(text.len());
    for ch in text.chars() {
        match FOLD_RANGES.binary_search_by(|range| in_range(range, ch.into())) {
            Ok(index) => {
                let range = &FOLD_RANGES[index];
                let start = word_at(range, 8) as usize;
                let end = start + usize::from(range[12]);
                folded.push_str(std::str::from_utf8(&FOLD_UTF8[start..end]).expect("Unicode mapping is UTF-8"));
            }
            Err(_) => folded.push(ch),
        }
    }
    folded.nfc().collect()
}

fn tokens(folded: &str) -> impl Iterator<Item = &str> {
    folded.split_word_bounds().filter(|segment| {
        segment.chars().any(|ch| {
            if ch.is_ascii() {
                ch.is_ascii_alphanumeric()
            } else {
                WORD_RANGES.binary_search_by(|range| in_range(range, ch.into())).is_ok()
            }
        })
    })
}

pub(super) fn matches(text: &str, query: &str) -> bool {
    let normalized_query = fold(query);
    let mut needed: HashSet<_> = tokens(&normalized_query).collect();
    if needed.is_empty() { return false; }

    let normalized_text = fold(text);
    tokens(&normalized_text).any(|token| {
        needed.remove(token);
        needed.is_empty()
    })
}
