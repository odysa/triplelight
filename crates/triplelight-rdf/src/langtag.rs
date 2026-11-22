//! RFC 5646 language-tag well-formedness (task #45), with exactly the acceptance of `oxilangtag` 0.1, which the
//! N-Triples parser and oxrdf used before: grandfathered tags, private use (`x-…`), and the langtag production. Validity
//! only; the parsers and `Literal::new_language_tagged_literal` lowercase the tag themselves.
//! Derived in part from oxilangtag (MIT); see THIRD_PARTY_NOTICES.md.

use std::fmt;

/// An ill-formed language tag (`oxilangtag::LanguageTagParseError` in oxrdf).
#[derive(Debug)]
pub struct LanguageTagParseError;

impl fmt::Display for LanguageTagParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("The language tag is not well-formed") }
}

impl std::error::Error for LanguageTagParseError {}

/// `tag` itself if it is well-formed.
pub fn parse(tag: String) -> Result<String, LanguageTagParseError> {
    if is_well_formed(&tag) { Ok(tag) } else { Err(LanguageTagParseError) }
}

const GRANDFATHERED: [&str; 26] = [
    "art-lojban", "cel-gaulish", "en-GB-oed", "i-ami", "i-bnn", "i-default", "i-enochian", "i-hak", "i-klingon",
    "i-lux", "i-mingo", "i-navajo", "i-pwn", "i-tao", "i-tay", "i-tsu", "no-bok", "no-nyn", "sgn-BE-FR", "sgn-BE-NL",
    "sgn-CH-DE", "zh-guoyu", "zh-hakka", "zh-min", "zh-min-nan", "zh-xiang",
];

fn alpha(s: &str) -> bool { s.bytes().all(|b| b.is_ascii_alphabetic()) }
fn digits(s: &str) -> bool { s.bytes().all(|b| b.is_ascii_digit()) }
fn alnum(s: &str) -> bool { s.bytes().all(|b| b.is_ascii_alphanumeric()) }

/// Whether `tag` is a well-formed BCP 47 language tag.
pub fn is_well_formed(tag: &str) -> bool {
    if tag.len() >= 5 && GRANDFATHERED.iter().any(|g| g.eq_ignore_ascii_case(tag)) { return true }
    if let Some(rest) = tag.strip_prefix("x-").or_else(|| tag.strip_prefix("X-")) {
        return !rest.is_empty() && rest.split('-').all(|s| !s.is_empty() && s.len() <= 8 && alnum(s));
    }
    #[derive(PartialEq)]
    enum State { Start, AfterLanguage, AfterExtLang, AfterScript, AfterRegion, InExtension { expected: bool }, InPrivateUse { expected: bool } }
    let mut state = State::Start;
    let mut extlangs = 0;
    for subtag in tag.split('-') {
        if subtag.is_empty() || subtag.len() > 8 { return false }
        state = match state {
            State::Start => {
                if subtag.len() < 2 || !alpha(subtag) { return false }
                if subtag.len() < 4 { State::AfterLanguage } else { State::AfterExtLang }
            }
            State::InPrivateUse { .. } => {
                if !alnum(subtag) { return false }
                State::InPrivateUse { expected: false }
            }
            _ if matches!(subtag, "x" | "X") => {
                if state == (State::InExtension { expected: true }) { return false }
                State::InPrivateUse { expected: true }
            }
            _ if subtag.len() == 1 && alnum(subtag) => {
                if state == (State::InExtension { expected: true }) { return false }
                State::InExtension { expected: true }
            }
            State::InExtension { .. } => {
                if !alnum(subtag) { return false }
                State::InExtension { expected: false }
            }
            State::AfterLanguage if subtag.len() == 3 && alpha(subtag) => {
                extlangs += 1;
                if extlangs > 3 { return false }
                State::AfterLanguage
            }
            State::AfterLanguage | State::AfterExtLang if subtag.len() == 4 && alpha(subtag) => State::AfterScript,
            State::AfterLanguage | State::AfterExtLang | State::AfterScript
                if subtag.len() == 2 && alpha(subtag) || subtag.len() == 3 && digits(subtag) => State::AfterRegion,
            State::AfterLanguage | State::AfterExtLang | State::AfterScript | State::AfterRegion
                if alnum(subtag)
                    && (subtag.len() >= 5 && subtag.as_bytes()[0].is_ascii_alphabetic()
                        || subtag.len() >= 4 && subtag.as_bytes()[0].is_ascii_digit()) => State::AfterRegion,
            _ => return false,
        };
    }
    !matches!(state, State::InExtension { expected: true } | State::InPrivateUse { expected: true })
}
