//! Self-authored, statically linked native C-ABI reference functions.

use bifrost_abi::Pair64;

pub extern "C" fn swap_pair(pair: Pair64) -> Pair64 {
    Pair64 {
        first: pair.second.wrapping_add(1),
        second: pair.first,
    }
}

/// Increments each supplied byte and returns the sum of the original values.
///
/// # Safety
///
/// The caller must provide an exclusive initialized buffer for the length of the call.
pub unsafe extern "C" fn increment_buffer(bytes: *mut u8, len: usize) -> u64 {
    // SAFETY: The caller guarantees the buffer lifetime and bounds, including zero length.
    let bytes = unsafe { std::slice::from_raw_parts_mut(bytes, len) };
    let mut sum = 0_u64;
    for byte in bytes {
        sum = sum.wrapping_add(u64::from(*byte));
        *byte = byte.wrapping_add(1);
    }
    sum
}

pub extern "C" fn add_u64(a: u64, b: u64) -> u64 {
    a.wrapping_add(b)
}
pub extern "C" fn subtract_i64(a: i64, b: i64) -> i64 {
    a.wrapping_sub(b)
}
pub extern "C" fn add_mixed(a: u64, b: f64) -> f64 {
    a as f64 + b
}
