use bifrost_abi::{AbiDescriptor, Location, Pair64, Scalar};
use bifrost_bridge::{
    BridgeError, BridgeRegistry, BufferMode, NativeBuffer, NativeFunction, NativeToken,
};
use bifrost_core::{AbiId, Arm64State, CpuState, GuestAddress as Addr, GuestIsa, X86State};
use bifrost_memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection};
use bifrost_testkit::native;

const ENTRY: Addr = Addr(0x3000);

fn fixture(id: AbiId) -> (&'static AbiDescriptor, AddressSpace, CpuState) {
    let abi = AbiDescriptor::for_id(id).unwrap();
    let mut memory = AddressSpace::new(4096, 2).unwrap();
    memory
        .map(Addr(0x1000), 8192, Protection::ReadWrite)
        .unwrap();
    let sp = Addr(0x1100 + u64::from(abi.entry_stack_bias()));
    let (mut cpu, ret) = if abi.isa == GuestIsa::X86_64 {
        let mut state = X86State {
            rip: ENTRY,
            ..Default::default()
        };
        state.gpr[4] = sp.0;
        (CpuState::X86_64(Box::new(state)), Location::Stack(0))
    } else {
        (
            CpuState::AArch64(Box::new(Arm64State {
                sp,
                pc: ENTRY,
                ..Default::default()
            })),
            Location::Integer(30),
        )
    };
    abi.write_word(&mut cpu, &mut memory, ret, 0x4000).unwrap();
    (abi, memory, cpu)
}

#[test]
fn aggregate_calls_use_direct_pairs_and_windows_hidden_buffers() {
    if AbiDescriptor::native().is_none() {
        return;
    }
    let mut registry = BridgeRegistry::default();
    registry
        .register(
            "native",
            "swap",
            ENTRY,
            NativeFunction::Pair(native::swap_pair),
        )
        .unwrap();
    let original = Pair64 {
        first: 12,
        second: 29,
    };
    for id in [
        AbiId::LinuxSysvAmd64,
        AbiId::WindowsX64,
        AbiId::LinuxAapcs64,
        AbiId::WindowsArm64,
    ] {
        let (abi, mut memory, mut cpu) = fixture(id);
        if id == AbiId::WindowsX64 {
            memory.write(Addr(0x1200), &original.guest_bytes()).unwrap();
            abi.write_word(&mut cpu, &mut memory, Location::Integer(1), 0x1228)
                .unwrap();
            abi.write_word(&mut cpu, &mut memory, Location::Integer(2), 0x1200)
                .unwrap();
        } else {
            for (&reg, value) in abi
                .integer_args
                .iter()
                .zip([original.first, original.second])
            {
                abi.write_word(&mut cpu, &mut memory, Location::Integer(reg), value)
                    .unwrap();
            }
        }
        registry.invoke(abi, &mut cpu, &mut memory).unwrap();
        let returned = if id == AbiId::WindowsX64 {
            let mut bytes = [0; 16];
            assert_eq!(
                abi.read_word(&cpu, &memory, Location::Integer(0)),
                Err(bifrost_abi::AbiError::MisalignedStack(cpu.sp()))
            );
            let CpuState::X86_64(state) = &cpu else {
                unreachable!()
            };
            assert_eq!(state.gpr[0], 0x1228);
            memory.read(Addr(0x1228), &mut bytes).unwrap();
            Pair64::from_guest_bytes(bytes)
        } else {
            let words = match &cpu {
                CpuState::X86_64(state) => [state.gpr[0], state.gpr[2]],
                CpuState::AArch64(state) => [state.x[0], state.x[1]],
                _ => unreachable!(),
            };
            Pair64 {
                first: words[0],
                second: words[1],
            }
        };
        assert_eq!(
            returned,
            Pair64 {
                first: 30,
                second: 12
            },
            "{id:?}"
        );
        assert_eq!(cpu.pc(), Addr(0x4000));
    }
}

#[test]
#[allow(unsafe_code)]
fn buffer_copyback_requires_the_entire_guest_range_to_be_writable() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn count_increment(bytes: *mut u8, len: usize) -> u64 {
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: The counted wrapper forwards the same buffer contract.
        unsafe { native::increment_buffer(bytes, len) }
    }
    if AbiDescriptor::native().is_none() {
        return;
    }
    let (abi, mut memory, mut cpu) = fixture(AbiId::LinuxAapcs64);
    let address = Addr(0x1ffe);
    memory.write(address, &[1, 2, 3, 4]).unwrap();
    // SAFETY: The reference library mutates exactly the staging slice during the call.
    let function = NativeFunction::Buffer(unsafe {
        NativeBuffer::new(count_increment, BufferMode::InOut, 16)
    });
    let mut registry = BridgeRegistry::default();
    registry
        .register("native", "increment", ENTRY, function)
        .unwrap();
    abi.write_word(&mut cpu, &mut memory, Location::Integer(0), address.0)
        .unwrap();
    abi.write_word(&mut cpu, &mut memory, Location::Integer(1), 4)
        .unwrap();
    memory
        .protect(Addr(0x2000), 4096, Protection::Read)
        .unwrap();
    let original = cpu.clone();
    let generation = memory.generation_at(address);
    assert!(matches!(
        registry.invoke(abi, &mut cpu, &mut memory),
        Err(BridgeError::Abi(_))
    ));
    assert_eq!(cpu, original);
    assert_eq!(memory.generation_at(address), generation);
    assert_eq!(CALLS.load(Ordering::Relaxed), 0);
    let mut bytes = [0; 4];
    memory.read(address, &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);

    memory
        .protect(Addr(0x2000), 4096, Protection::ReadWrite)
        .unwrap();
    abi.write_word(&mut cpu, &mut memory, Location::Integer(1), 17)
        .unwrap();
    assert!(matches!(
        registry.invoke(abi, &mut cpu, &mut memory),
        Err(BridgeError::BufferLimit { .. })
    ));
    assert_eq!(CALLS.load(Ordering::Relaxed), 0);
    abi.write_word(&mut cpu, &mut memory, Location::Integer(1), 4)
        .unwrap();
    registry.invoke(abi, &mut cpu, &mut memory).unwrap();
    assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    memory.read(address, &mut bytes).unwrap();
    assert_eq!(bytes, [2, 3, 4, 5]);
    assert_eq!(
        abi.read_word(&cpu, &memory, Location::Integer(0)).unwrap(),
        10
    );
}

#[test]
fn module_bindings_and_handles_reject_confusion_and_reuse() {
    if AbiDescriptor::native().is_none() {
        return;
    }
    let (abi, mut memory, mut cpu) = fixture(AbiId::LinuxAapcs64);
    let mut registry = BridgeRegistry::default();
    let function = NativeFunction::HandleU64(native::add_u64);
    registry.register("one", "add", ENTRY, function).unwrap();
    registry
        .register("two", "add", Addr(ENTRY.0 + 4), function)
        .unwrap();
    assert_eq!(registry.address("one", "add"), Some(ENTRY));
    assert!(matches!(
        registry.register("one", "add", Addr(ENTRY.0 + 8), function),
        Err(BridgeError::DuplicateSymbol { .. })
    ));
    assert!(matches!(
        registry.register("two", "other", ENTRY, function),
        Err(BridgeError::DuplicateEntry(_))
    ));
    let handle = registry.insert_handle("one", NativeToken(9)).unwrap();
    assert!(matches!(
        registry.resolve_handle::<NativeToken>("two", handle),
        Err(BridgeError::InvalidHandle(_))
    ));
    assert!(matches!(
        registry.resolve_handle::<u64>("one", handle),
        Err(BridgeError::WrongHandleType(_))
    ));
    let args = abi.argument_locations(&[Scalar::U64; 2]).unwrap();
    abi.write_word(&mut cpu, &mut memory, args[0], handle.bits())
        .unwrap();
    abi.write_word(&mut cpu, &mut memory, args[1], 7).unwrap();
    let entry_state = cpu.clone();
    registry.invoke(abi, &mut cpu, &mut memory).unwrap();
    assert_eq!(
        abi.read_word(&cpu, &memory, Location::Integer(0)).unwrap(),
        16
    );
    registry.close_handle("one", handle).unwrap();
    assert_ne!(
        registry.insert_handle("one", NativeToken(42)).unwrap(),
        handle
    );
    assert!(matches!(
        registry.invoke(abi, &mut entry_state.clone(), &mut memory),
        Err(BridgeError::InvalidHandle(_))
    ));
}
