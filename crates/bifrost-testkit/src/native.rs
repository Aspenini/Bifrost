//! Self-authored, statically linked native C-ABI reference functions.

pub extern "C" fn add_u64(a: u64, b: u64) -> u64 {
    a.wrapping_add(b)
}
pub extern "C" fn subtract_i64(a: i64, b: i64) -> i64 {
    a.wrapping_sub(b)
}
pub extern "C" fn add_mixed(a: u64, b: f64) -> f64 {
    a as f64 + b
}
