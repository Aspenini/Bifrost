use bifrost_core::{AccessKind, GuestAddress as Addr};
use bifrost_memory::{
    AddressSpace, GuestMemory, GuestMemoryRead, MemoryError, MemoryRegion, Protection,
};

#[test]
fn region_queries_follow_current_protection_and_unmapped_gaps() {
    let mut memory = AddressSpace::new(4096, 4).unwrap();
    memory
        .map(Addr(0x1000), 8192, Protection::ReadWrite)
        .unwrap();
    memory.map(Addr(0x4000), 4096, Protection::Guard).unwrap();
    let last_page = Addr(u64::MAX - 4095);
    memory.map(last_page, 4096, Protection::Read).unwrap();
    assert_eq!(
        memory.region_at(Addr(0x2010)),
        MemoryRegion {
            start: Addr(0x1000),
            end: Some(Addr(0x3000)),
            protection: Some(Protection::ReadWrite)
        }
    );
    memory
        .protect(Addr(0x1000), 4096, Protection::Read)
        .unwrap();
    assert_eq!(memory.region_at(Addr(0x2010)).start, Addr(0x2000));
    assert_eq!(
        memory.region_at(Addr(0x3010)),
        MemoryRegion {
            start: Addr(0x3000),
            end: Some(Addr(0x4000)),
            protection: None
        }
    );
    assert_eq!(
        memory.region_at(Addr(0x4000)).protection,
        Some(Protection::Guard)
    );
    assert_eq!(
        memory.region_at(Addr(u64::MAX)),
        MemoryRegion {
            start: last_page,
            end: None,
            protection: Some(Protection::Read)
        }
    );
}

#[test]
fn page_loans_preserve_contents_and_allow_partial_returns_at_the_page_limit() {
    let source = Addr(0x1000);
    let destination = Addr(0x4000);
    let mut memory = AddressSpace::new(4096, 3).unwrap();
    memory.map(source, 8192, Protection::ReadWrite).unwrap();
    memory
        .map(Addr(0x5000), 4096, Protection::ReadWrite)
        .unwrap();
    memory.write(Addr(0x2000), &[12]).unwrap();
    let previous_generation = memory.generation_at(source);
    assert_eq!(
        memory.lend_pages(source, destination, 8192),
        Err(MemoryError::AlreadyMapped(Addr(0x5000)))
    );
    assert_eq!(memory.generation_at(source), previous_generation);
    assert_eq!(memory.generation_at(destination), None);
    memory.unmap(Addr(0x5000), 4096).unwrap();
    memory
        .map(Addr(0x6000), 4096, Protection::ReadWrite)
        .unwrap();
    memory.lend_pages(source, destination, 8192).unwrap();
    assert_eq!(memory.mapped_pages(), 3);
    assert!(memory.generation_at(source) > previous_generation);
    assert_eq!(memory.region_at(Addr(0x5000)).end, Some(Addr(0x6000)));
    assert!(memory.read(source, &mut [0]).is_err());
    assert!(memory.unmap(source, 4096).is_err());
    assert!(memory.protect(destination, 4096, Protection::Read).is_err());
    assert!(memory.return_pages(source, Addr(0x5000), 4096).is_err());

    let mut byte = [0];
    memory.read(Addr(0x5000), &mut byte).unwrap();
    assert_eq!(byte, [12]);
    memory.write(Addr(0x5000), &[34]).unwrap();
    memory
        .return_pages(Addr(0x2000), Addr(0x5000), 4096)
        .unwrap();
    assert_eq!(memory.generation_at(Addr(0x5000)), None);
    assert!(memory.read(source, &mut [0]).is_err());
    memory.read(Addr(0x2000), &mut byte).unwrap();
    assert_eq!(byte, [34]);
    memory.return_pages(source, destination, 4096).unwrap();
    assert_eq!(memory.mapped_pages(), 3);
}

#[test]
fn guarded_stack_mapping_is_atomic() {
    let mut memory = AddressSpace::new(4096, 3).unwrap();
    memory
        .map(Addr(0x2000), 4096, Protection::ReadWrite)
        .unwrap();
    assert_eq!(
        memory.map_guarded_stack(Addr(0x1000), 4096),
        Err(MemoryError::AlreadyMapped(Addr(0x2000)))
    );
    assert_eq!(memory.generation_at(Addr(0x1000)), None);

    let top = memory.map_guarded_stack(Addr(0x3000), 4096).unwrap();
    assert_eq!(top, Addr(0x5000));
    assert!(matches!(
        memory.write(Addr(0x3fff), &[0]),
        Err(MemoryError::Denied {
            protection: Protection::Guard,
            ..
        })
    ));
    memory.write(Addr(0x4fff), &[12]).unwrap();
}

#[test]
fn validates_the_entire_access_before_copying() {
    let mut memory = AddressSpace::new(4096, 3).unwrap();
    memory
        .map(Addr(0x1000), 4096, Protection::ReadWrite)
        .unwrap();
    memory.map(Addr(0x2000), 4096, Protection::Guard).unwrap();
    let generation = memory.generation_at(Addr(0x1000));
    let denied = |access| MemoryError::Denied {
        address: Addr(0x2000),
        access,
        protection: Protection::Guard,
    };

    assert_eq!(
        memory.write(Addr(0x1fff), &[1, 2]),
        Err(denied(AccessKind::Write))
    );
    assert_eq!(memory.generation_at(Addr(0x1000)), generation);
    let mut bytes = [9, 9];
    assert_eq!(
        memory.read(Addr(0x1fff), &mut bytes),
        Err(denied(AccessKind::Read))
    );
    assert_eq!(bytes, [9, 9]);
    memory.read(Addr(0x1fff), &mut bytes[..1]).unwrap();
    assert_eq!(bytes, [0, 9]);

    memory
        .protect(Addr(0x2000), 4096, Protection::ReadWrite)
        .unwrap();
    memory.write(Addr(0x1fff), &[1, 2]).unwrap();
    memory.read(Addr(0x1fff), &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2]);
    assert_eq!(
        memory.generation_at(Addr(0x1000)),
        memory.generation_at(Addr(0x2000))
    );
}

#[test]
fn distinguishes_fetch_and_access_and_advances_generations() {
    let mut memory = AddressSpace::new(4096, 1).unwrap();
    memory.map(Addr(0), 4096, Protection::ReadWrite).unwrap();
    let loaded = memory.generation_at(Addr(0));
    memory.write(Addr(7), &[0xcc]).unwrap();
    let written = memory.generation_at(Addr(0));
    let mut byte = [0];
    assert!(memory.fetch(Addr(7), &mut byte).is_err());
    memory.protect(Addr(0), 4096, Protection::Execute).unwrap();
    let executable = memory.generation_at(Addr(0));
    memory.fetch(Addr(7), &mut byte).unwrap();
    assert_eq!(byte, [0xcc]);
    assert!(memory.read(Addr(7), &mut byte).is_err());
    assert!(memory.write(Addr(7), &[0]).is_err());

    memory.unmap(Addr(0), 4096).unwrap();
    assert_eq!(memory.generation_at(Addr(0)), None);
    memory.map(Addr(0), 4096, Protection::ReadExecute).unwrap();
    memory.fetch(Addr(7), &mut byte).unwrap();
    assert_eq!(byte, [0]);
    assert!(loaded < written && written < executable && executable < memory.generation_at(Addr(0)));
}

#[test]
fn handles_the_top_address_and_rejects_invalid_regions_atomically() {
    let mut memory = AddressSpace::new(16, 3).unwrap();
    let last = Addr(u64::MAX);
    let base = Addr(u64::MAX - 15);
    memory.map(base, 16, Protection::ReadWrite).unwrap();
    memory.write(last, &[7]).unwrap();
    assert_eq!(
        memory.write(last, &[8, 9]),
        Err(MemoryError::AddressOverflow(last))
    );
    assert_eq!(
        memory.map(base, 32, Protection::ReadWrite),
        Err(MemoryError::AddressOverflow(base))
    );
    assert_eq!(
        memory.map(Addr(base.0 - 16), 32, Protection::ReadWrite),
        Err(MemoryError::AlreadyMapped(base))
    );
    assert_eq!(memory.generation_at(Addr(base.0 - 16)), None);
    assert_eq!(
        memory.protect(Addr(base.0 - 16), 32, Protection::None),
        Err(MemoryError::Unmapped(Addr(base.0 - 16)))
    );
    assert_eq!(
        memory.unmap(Addr(base.0 - 16), 32),
        Err(MemoryError::Unmapped(Addr(base.0 - 16)))
    );
    let mut byte = [0];
    memory.read(last, &mut byte).unwrap();
    assert_eq!(byte, [7]);
    assert_eq!(
        memory.map(Addr(0), 48, Protection::ReadWrite),
        Err(MemoryError::PageLimit)
    );
    assert!(matches!(
        memory.map(Addr(1), 16, Protection::ReadWrite),
        Err(MemoryError::InvalidRegion { .. })
    ));
}
