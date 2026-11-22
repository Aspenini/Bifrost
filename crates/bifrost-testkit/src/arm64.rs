//! Self-authored A64 raw instructions and their expected guest results.

/// Stores 41, orders the subsequent load with DMB, and adds one. A matching comparison
/// reaches a BL/RET pair that adds one more before branching over the UDF path.
pub const MOVE_ADD_AND_CALL: [u32; 13] = [
    0xd280_0520,
    0xf900_0420,
    0xd503_3bbf,
    0xf940_0422,
    0xb100_0440,
    0xf100_a81f,
    0x5400_0061,
    0x9400_0003,
    0x1400_0004,
    0x0000_0000,
    0x9100_0400,
    0xd65f_03c0,
    0xd503_201f,
];
pub const MOVE_ADD_AND_CALL_STEPS: u64 = 12;

pub fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}
