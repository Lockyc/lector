//! Vendored 64-bit FNV-1a. Derives a tab's stable webview label from its canonicalized dir.
//!
//! Deliberately *not* `std`'s `DefaultHasher`: std does not guarantee that algorithm is stable
//! across Rust releases, and a changed hash would remap every tab label — the active marks,
//! servers and pop-out bookkeeping are all keyed by them. FNV-1a is frozen here; the test vectors
//! lock it in place. (Window labels are the WindowSet's sanitized titles and use no hash.)
//!
//! **Not** the hash behind the geometry-store *filename*: that one hashes the resolved config path
//! and is a separate copy owned by shell-core (`geometry_filename`), a different domain (a path,
//! not a dir) — see the repo's CLAUDE.md footgun section.
//!
//! This is the **only** copy in this workspace. shell-core's dividing-line decision rules the hash
//! out of *that* crate — it says nothing about an app's own config crate, and a second copy in
//! `src-tauri` would be a shadow that drifts.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// 64-bit FNV-1a hash of `bytes`. Small, deterministic, and — crucially — stable across Rust
/// toolchains. Non-cryptographic; collision resistance is irrelevant here (the input is a
/// canonicalized repo dir).
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_fnv1a_vectors() {
        // Canonical FNV-1a/64 test vectors — pins the algorithm so tab labels can never drift with
        // the toolchain (the whole point of not using DefaultHasher).
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
