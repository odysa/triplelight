//! CRC-32C (Castagnoli), the per-block checksum of `RDFIDX05`/`RDFQIX03` index files: about 0.3 ns per byte-row in
//! hardware against SHA-256's 7-20 ns per row, which cost more than decoding the block (#143). It detects accidental
//! corruption, as the SHA-256 it replaces did there; the files are not authenticated either way.

/// CRC-32C of `data` (initial value and final XOR `0xffff_ffff`, reflected, as iSCSI and ext4).
pub(crate) fn crc32c(data: &[u8]) -> u32 {
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("crc") {
        // SAFETY: the `crc` feature was detected at run time.
        return unsafe { hw_aarch64(data) };
    }
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("sse4.2") {
        // SAFETY: `sse4.2` was detected at run time.
        return unsafe { hw_x86(data) };
    }
    software(data)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn hw_aarch64(data: &[u8]) -> u32 {
    use std::arch::aarch64::{__crc32cb, __crc32cd};
    let mut crc = !0u32;
    let mut words = data.chunks_exact(8);
    for w in &mut words { crc = __crc32cd(crc, u64::from_le_bytes(w.try_into().unwrap())) }
    for &b in words.remainder() { crc = __crc32cb(crc, b) }
    !crc
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn hw_x86(data: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};
    let mut crc = !0u32;
    let mut words = data.chunks_exact(8);
    for w in &mut words { crc = _mm_crc32_u64(crc as u64, u64::from_le_bytes(w.try_into().unwrap())) as u32 }
    for &b in words.remainder() { crc = _mm_crc32_u8(crc, b) }
    !crc
}

/// Bitwise fallback for CPUs without the instruction (only the wasm/other targets, which have no disk stores).
fn software(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 { crc = (crc >> 1) ^ (0x82f6_3b78 & (crc & 1).wrapping_neg()) }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_vectors_and_the_hardware_path_agrees_with_software() {
        // RFC 3720 (iSCSI) B.4 test vectors.
        assert_eq!(crc32c(b"123456789"), 0xe306_9283);
        assert_eq!(crc32c(&[0u8; 32]), 0x8a91_36aa);
        assert_eq!(crc32c(&[0xffu8; 32]), 0x62a8_ab43);
        assert_eq!(crc32c(&(0..32u8).collect::<Vec<_>>()), 0x46dd_794e);
        assert_eq!(crc32c(b""), 0);
        let data: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for len in [0, 1, 7, 8, 9, 63, 64, 65, 999, 1000] { assert_eq!(crc32c(&data[..len]), software(&data[..len]), "len {len}") }
    }
}
