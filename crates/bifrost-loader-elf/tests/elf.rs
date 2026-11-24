use bifrost_core::GuestAddress;
use bifrost_loader::LoadError;
use bifrost_loader_elf::parse;
use bifrost_memory::GuestMemoryRead;

const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/linux/hello.elf");
const PIE: &[u8] = include_bytes!("../../../tests/fixtures/linux/pie.elf");

#[test]
fn rebases_pie_pointer_before_protecting_relro() {
    use bifrost_core::GuestMemory;
    let parsed = parse(PIE).unwrap();
    let base = parsed.image.metadata.entry.0 - 0x1000;
    assert_ne!(base, 0);
    let (mut memory, state) = parsed.start(&[], &[]).unwrap();
    assert_eq!(state.rip.0, base + 0x1000);
    let pointer = GuestAddress(base + 0x2000);
    let mut value = [0; 8];
    memory.read(pointer, &mut value).unwrap();
    let address = GuestAddress(u64::from_le_bytes(value));
    let mut message = [0; 24];
    memory.read(address, &mut message).unwrap();
    assert_eq!(&message, b"Hello from Bifrost PIE!\n");
    assert!(memory.write(pointer, &[1]).is_err());
}

#[test]
fn rejects_unsupported_dynamic_tags_and_relocation_targets() {
    let mut tagged = PIE.to_vec();
    // Replace the first dynamic tag with DT_NEEDED, which requires a guest linker.
    tagged[0x2008..0x2010].copy_from_slice(&1_u64.to_le_bytes());
    assert!(matches!(
        parse(&tagged),
        Err(LoadError::Unsupported("ELF dynamic dependency or tag"))
    ));

    let mut executable_target = PIE.to_vec();
    // Redirect R_X86_64_RELATIVE to the executable segment.
    executable_target[0x210..0x218].copy_from_slice(&0x1000_u64.to_le_bytes());
    assert!(matches!(
        parse(&executable_target),
        Err(LoadError::Invalid("ELF relocation destination"))
    ));

    let mut symbol = PIE.to_vec();
    symbol[0x218..0x220].copy_from_slice(&0x1_0000_0008_u64.to_le_bytes());
    assert!(matches!(parse(&symbol), Err(LoadError::Unsupported(_))));
}

#[test]
fn validates_headers_and_builds_a_linux_process_stack() {
    let elf = parse(FIXTURE).unwrap();
    let (memory, cpu) = elf.start(&[b"hello", b"world"], &[b"NAME=value"]).unwrap();
    let sp = cpu.gpr[4];
    assert_eq!(sp % 16, 0);
    let get = |address| {
        let mut word = [0; 8];
        memory.read(GuestAddress(address), &mut word).unwrap();
        u64::from_le_bytes(word)
    };
    assert_eq!(get(sp), 2);
    let mut text = [0; 6];
    memory.read(GuestAddress(get(sp + 8)), &mut text).unwrap();
    assert_eq!(&text, b"hello\0");
    assert_eq!(get(sp + 24), 0);
    assert_eq!(get(sp + 40), 0);
    assert_eq!(get(sp + 48), 3);
    assert!(get(sp + 56) != 0);
    assert!(memory.fetch(cpu.rip, &mut [0]).is_ok());
    for segment in &elf.image.segments {
        if segment.size > segment.data.len() as u64 {
            let mut byte = [9];
            memory
                .read(
                    segment
                        .address
                        .checked_add(segment.data.len() as u64)
                        .unwrap(),
                    &mut byte,
                )
                .unwrap();
            assert_eq!(byte, [0]);
        }
    }
}

#[test]
fn rejects_truncation_and_malformed_program_header_offsets() {
    for n in 0..64 {
        assert!(parse(&FIXTURE[..n]).is_err(), "{n}");
    }
    let mut bad = FIXTURE.to_vec();
    bad[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(parse(&bad), Err(LoadError::Truncated)));
    let mut bad = FIXTURE.to_vec();
    bad[24..32].copy_from_slice(&0_u64.to_le_bytes());
    assert!(matches!(
        parse(&bad).unwrap().image.map(),
        Err(LoadError::Invalid(_))
    ));
}
