# Unicode 17.0.0 analyzer tables

Regenerate the tables using `run/generate_text_data.py <directory>`, with the
two Unicode 17.0.0 inputs below saved by filename. The generator verifies
their SHA-256 hashes and emits little-endian data.

- [DerivedNormalizationProps.txt](https://www.unicode.org/Public/17.0.0/ucd/DerivedNormalizationProps.txt)
- [DerivedGeneralCategory.txt](https://www.unicode.org/Public/17.0.0/ucd/extracted/DerivedGeneralCategory.txt)

`fold_ranges.bin` stores the start, end, UTF-8 offset, and UTF-8 length of
each specified NFKC_CF mapping. The UTF-8 bytes are in `fold_utf8.bin`.
`word_ranges.bin` stores the start and end of each contiguous range of
Unicode letters or numbers. See [third-party notices](../../../THIRD_PARTY_NOTICES.md).
