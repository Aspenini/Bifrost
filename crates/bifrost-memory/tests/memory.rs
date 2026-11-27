use bifrost_core::{AccessKind, GuestAddress as Addr};
use bifrost_memory::{
    AddressSpace, GuestMemory, GuestMemoryRead, MemoryError, MemoryRegion, Protection,
};

#[test]
fn an_in_place_reservation_preserves_data_and_requires_a_complete_release() {
    let mut memory = AddressSpace::new(4096, 4).unwrap();
    let start = Addr(0x1000);
    memory.map(start, 8192, Protection::ReadWrite).unwrap();
    memory.write(start, b"keep").unwrap();
    assert!(
        memory
            .reserve_pages(start, 12288, Protection::Read)
            .is_err()
    );
    assert_eq!(
        memory.region_at(start).protection,
        Some(Protection::ReadWrite)
    );

    memory.reserve_pages(start, 8192, Protection::Read).unwrap();
    let mut bytes = [0; 4];
    memory.read(start, &mut bytes).unwrap();
    assert_eq!(&bytes, b"keep");
    assert!(memory.write(start, b"busy").is_err());
    assert!(memory.unmap(start, 8192).is_err());
    assert!(memory.protect(start, 8192, Protection::ReadWrite).is_err());
    assert!(memory.set_permission_lock(start, 8192, false).is_err());
    assert!(memory.release_reserved_pages(start, 4096).is_err());
    assert_eq!(memory.mapped_pages(), 2);

    assert!(
        memory
            .write_reserved(start, 4096, 0, b"wrong range")
            .is_err()
    );
    assert!(memory.write_reserved(start, 8192, 8191, b"out").is_err());
    memory.write_reserved(start, 8192, 4094, b"shared").unwrap();
    let mut shared = [0; 6];
    memory.read(Addr(start.0 + 4094), &mut shared).unwrap();
    assert_eq!(&shared, b"shared");
    assert!(memory.write(start, b"busy").is_err());

    memory.release_reserved_pages(start, 8192).unwrap();
    assert!(memory.write_reserved(start, 8192, 0, b"closed").is_err());
    memory.read(start, &mut bytes).unwrap();
    assert_eq!(&bytes, b"keep");
    memory.write(start, b"done").unwrap();
    memory.unmap(start, 8192).unwrap();
}

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
fn permission_locks_split_regions_and_prevent_partial_reprotection() {
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    let start = Addr(0x1000);
    let locked = Addr(0x2000);
    memory.map(start, 8192, Protection::ReadWrite).unwrap();
    memory.set_permission_lock(locked, 4096, true).unwrap();
    assert_eq!(memory.region_at(start).end, Some(locked));
    assert!(memory.permission_locked_at(locked));
    assert_eq!(
        memory.protect(start, 8192, Protection::Read),
        Err(MemoryError::PermissionLocked(locked))
    );
    memory.write(start, &[42]).unwrap();
    memory.set_permission_lock(locked, 4096, false).unwrap();
    memory.protect(start, 8192, Protection::Read).unwrap();
    assert!(!memory.permission_locked_at(locked));
    assert!(memory.write(start, &[0]).is_err());
}

#[test]
fn demand_zero_pages_preserve_untouched_bytes_and_generations() {
    let mut memory = AddressSpace::new(4096, 3).unwrap();
    let start = Addr(0x1000);
    memory.map(start, 8192, Protection::ReadWrite).unwrap();
    let edge = Addr(0x1fff);
    let mut data = [0xff; 4];
    memory.read(Addr(edge.0 - 1), &mut data).unwrap();
    assert_eq!(data, [0; 4]);
    let before = memory.generation_at(start);
    memory.write(edge, &[0, 0]).unwrap();
    assert!(memory.generation_at(start) > before);
    memory.write(edge, &[12, 34]).unwrap();
    memory.read(Addr(edge.0 - 1), &mut data).unwrap();
    assert_eq!(data, [0, 12, 34, 0]);
    memory.unmap(start, 8192).unwrap();
    memory.map(start, 8192, Protection::ReadWrite).unwrap();
    memory.read(Addr(edge.0 - 1), &mut data).unwrap();
    assert_eq!(data, [0; 4]);
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
