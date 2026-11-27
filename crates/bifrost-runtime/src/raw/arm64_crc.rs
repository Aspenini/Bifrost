use bifrost_core::{Arm64State, GuestAddress};
use bifrost_decoder_arm64::Crc32;

const IEEE: [u32; 256] = table(0xedb8_8320);
const CASTAGNOLI: [u32; 256] = table(0x82f6_3b78);

const fn table(polynomial: u32) -> [u32; 256] {
    let mut entries = [0; 256];
    let mut index = 0;
    while index < entries.len() {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (polynomial & 0u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        entries[index] = crc;
        index += 1;
    }
    entries
}

pub(super) fn execute(cpu: &mut Arm64State, pc: GuestAddress, op: Crc32) {
    let read = |register| cpu.x.get(usize::from(register)).copied().unwrap_or(0);
    let mut crc = read(op.accumulator) as u32;
    let input = read(op.input).to_le_bytes();
    let table = if op.castagnoli { &CASTAGNOLI } else { &IEEE };
    for &byte in &input[..usize::from(op.bytes)] {
        crc = (crc >> 8) ^ table[usize::from(crc as u8 ^ byte)];
    }
    if let Some(destination) = cpu.x.get_mut(usize::from(op.dst)) {
        *destination = u64::from(crc);
    }
    cpu.pc = GuestAddress(pc.0.wrapping_add(4));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_polynomials_match_the_standard_check_string() {
        let pc = GuestAddress(0x1000);
        for (castagnoli, expected) in [(false, 0xcbf4_3926_u32), (true, 0xe306_9283)] {
            let mut cpu = Arm64State::default();
            cpu.x[0] = u64::MAX;
            cpu.x[1] = u64::from_le_bytes(*b"12345678");
            let op = Crc32 {
                dst: 0,
                accumulator: 0,
                input: 1,
                bytes: 8,
                castagnoli,
            };
            execute(&mut cpu, pc, op);
            cpu.x[1] = u64::from(b'9') | 0xabcd_0000;
            execute(&mut cpu, GuestAddress(pc.0 + 4), Crc32 { bytes: 1, ..op });
            assert_eq!(cpu.x[0], u64::from(!expected));
            assert_eq!(cpu.pc, GuestAddress(pc.0 + 8));
        }
    }
}
