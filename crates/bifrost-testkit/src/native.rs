//! Self-authored, statically linked native C-ABI reference functions.

use bifrost_abi::Pair64;
use std::ffi::c_void;

/// Calls a synchronous callback twice and returns the sum.
///
/// # Safety
///
/// The function and context must remain valid on the calling thread for the entire call.
pub unsafe extern "C" fn call_twice(
    value: u64,
    callback: unsafe extern "C" fn(*mut c_void, u64) -> u64,
    context: *mut c_void,
) -> u64 {
    // SAFETY: The caller supplies the live pair. Each invocation finishes before the next.
    unsafe { callback(context, value).wrapping_add(callback(context, value.wrapping_add(1))) }
}

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

#[allow(clippy::too_many_arguments)]
pub extern "C" fn weighted(
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
    g: u64,
    h: u64,
    i: u64,
) -> u64 {
    [a, b, c, d, e, f, g, h, i]
        .into_iter()
        .zip(1..)
        .fold(0_u64, |sum, (word, weight)| {
            sum.wrapping_add(word.wrapping_mul(weight))
        })
}
