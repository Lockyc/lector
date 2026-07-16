//! Vendored 64-bit FNV-1a. Used to derive the window id, tab labels, and the window-state filename.
//!
//! Deliberately *not* `std`'s `DefaultHasher`: std does not guarantee that algorithm is stable
//! across Rust releases, and these hashes drive a **persistent on-disk filename** (the window-state
//! plugin's). lector is rebuilt from source with whatever toolchain the tree pins, so a `rustc` bump
//! that reshuffled hashing would silently change the filename, the plugin would find no state file,
//! and every window would open at default bounds — reading as "lector forgot my layout", never as a
//! toolchain problem. curator shipped exactly that bug (fixed 2026-07-16). FNV-1a is frozen here;
//! the test vectors lock it in place.
//!
//! This is the **only** copy in this workspace. shell-core's dividing-line decision rules the hash
//! out of *that* crate — it says nothing about an app's own config crate, and duplicating it into
//! `src-tauri` (as curator did) is a shadow that drifts.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// 64-bit FNV-1a hash of `bytes`. Small, deterministic, and — crucially — stable across Rust
/// toolchains. Non-cryptographic; collision resistance is irrelevant here (the inputs are a trusted
/// config path and a canonicalized repo dir).
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
        // Canonical FNV-1a/64 test vectors — pins the algorithm so the window-state filename can
        // never drift with the toolchain (the whole point of not using DefaultHasher).
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
