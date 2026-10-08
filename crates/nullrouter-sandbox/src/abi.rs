//! The module ABI as the host sees it: export names, sizes and the packed result.
//!
//! Only ABI 1 exists, so there is no previous-ABI shim yet. When `KIT_ABI` becomes 2, the shim
//! that lets an ABI 1 module run goes here, and `module::supported` already admits it.

/// Most bytes the host writes into a module as input, and reads back from it as output.
pub const MAX_IO: usize = 16 << 20;

pub const EXPORT_MEMORY: &str = "memory";
pub const EXPORT_ALLOC: &str = "zr_alloc";

/// A module's result: `(ptr << 32) | len`, where `0` means no edits. Returns `(ptr, len)`.
pub fn unpack(packed: i64) -> Option<(usize, usize)> {
    if packed == 0 {
        return None;
    }
    let bits = packed as u64;
    Some(((bits >> 32) as usize, (bits & 0xffff_ffff) as usize))
}

#[cfg(test)]
mod tests {
    use super::unpack;

    #[test]
    fn zero_is_no_edits_and_anything_else_splits_into_pointer_and_length() {
        assert_eq!(unpack(0), None);
        assert_eq!(unpack((3000_i64 << 32) | 7), Some((3000, 7)));
        assert_eq!(unpack(-1), Some((0xffff_ffff, 0xffff_ffff)));
    }
}
