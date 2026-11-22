use bifrost_core::GuestAddress;
use bifrost_loader::LoadError;
use bifrost_loader_elf::parse;
use bifrost_memory::GuestMemoryRead;

const FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/linux/hello.elf");

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
