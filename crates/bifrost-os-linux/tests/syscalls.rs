use bifrost_core::{GuestAddress as Addr, GuestMemory, GuestMemoryRead, X86State};
use bifrost_memory::{AddressSpace, Protection};
use bifrost_os_linux::{Io, Linux, SyscallExit, Vfs};

#[test]
fn mounted_files_and_streams_use_guest_descriptors_and_checked_memory() {
    let mut vfs = Vfs::default();
    vfs.mount(b"/fixture", b"hello".to_vec()).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut linux = Linux::new(
        Io {
            input: &[],
            stdout: &mut out,
            stderr: &mut err,
        },
        &vfs,
    );
    let mut memory = AddressSpace::new(4096, 1).unwrap();
    memory
        .map(Addr(0x2000), 4096, Protection::ReadWrite)
        .unwrap();
    memory.write(Addr(0x2000), b"/fixture\0").unwrap();
    let mut cpu = X86State {
        rip: Addr(0x1000),
        ..Default::default()
    };
    let mut call = |n, a, b, c| {
        cpu.gpr[0] = n;
        cpu.gpr[7] = a;
        cpu.gpr[6] = b;
        cpu.gpr[2] = c;
        let status = linux.syscall(&mut cpu, &mut memory).unwrap();
        (status, cpu.gpr[0] as i64)
    };
    let (_, fd) = call(257, (-100_i64) as u64, 0x2000, 0);
    assert_eq!(fd, 3);
    assert_eq!(call(0, fd as u64, 0x1fff, 5).1, -14);
    assert_eq!(call(0, fd as u64, 0x2100, 5).1, 5);
    assert_eq!(call(1, 1, 0x2100, 5).1, 5);
    assert_eq!(call(3, fd as u64, 0, 0).1, 0);
    assert_eq!(call(0, fd as u64, 0x2100, 1).1, -9);
    assert_eq!(call(231, 42, 0, 0).0, SyscallExit::Exit(42));
    assert_eq!(out, b"hello");
    let mut bytes = [0; 5];
    memory.read(Addr(0x2100), &mut bytes).unwrap();
    assert_eq!(&bytes, b"hello");
}

#[test]
fn missing_calls_are_traced_to_the_original_pc_without_mutating_state() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let vfs = Vfs::default();
    let mut linux = Linux::new(
        Io {
            input: &[],
            stdout: &mut out,
            stderr: &mut err,
        },
        &vfs,
    );
    let mut cpu = X86State {
        rip: Addr(0x1000),
        ..Default::default()
    };
    cpu.gpr[0] = 999;
    let before = cpu.clone();
    let error = linux
        .syscall(&mut cpu, &mut AddressSpace::new(4096, 0).unwrap())
        .unwrap_err();
    assert_eq!(error.number, 999);
    assert_eq!(error.pc, Addr(0x1000));
    assert_eq!(cpu, before);
}
