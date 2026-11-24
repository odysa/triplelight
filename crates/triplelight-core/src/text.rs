//! The versioned Unicode analyzer shared by literal matching and future postings.
use std::cmp::Ordering;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Identifies Unicode 17.0.0 NFKC_CF, NFC, UAX #29 default words, and the L/N segment filter.
pub const ANALYZER_ID: u64 = 1;

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

fn fold(text: &str, limit: usize) -> Option<String> {
    if text.is_ascii() {
        return (text.len() <= limit).then(|| text.to_ascii_lowercase());
    }
    let mut folded = String::with_capacity(text.len().min(limit));
    for ch in text.chars() {
        let mut encoded = [0; 4];
        let fragment = match FOLD_RANGES.binary_search_by(|range| in_range(range, ch.into())) {
            Ok(index) => {
                let range = &FOLD_RANGES[index];
                let start = word_at(range, 8) as usize;
                let end = start + usize::from(range[12]);
                std::str::from_utf8(&FOLD_UTF8[start..end]).expect("Unicode mapping is UTF-8")
            }
            Err(_) => ch.encode_utf8(&mut encoded),
        };
        if fragment.len() > limit.saturating_sub(folded.len()) { return None; }
        folded.push_str(fragment);
    }
    let mut normalized = String::with_capacity(folded.len());
    for ch in folded.nfc() {
        if ch.len_utf8() > limit.saturating_sub(normalized.len()) { return None; }
        normalized.push(ch);
    }
    Some(normalized)
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

/// Owned normalized text. Token strings borrow its normalized storage.
pub struct AnalyzedText(String);

impl AnalyzedText {
    /// Analyze a scalar argument.
    pub fn new(text: &str) -> Self {
        Self::bounded(text, usize::MAX).expect("unbounded analysis")
    }

    /// Bound each normalization buffer used when building an index.
    pub fn bounded(text: &str, bytes: usize) -> Option<Self> {
        fold(text, bytes).map(Self)
    }

    /// Word segments containing at least one letter or number.
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        tokens(&self.0)
    }
}
