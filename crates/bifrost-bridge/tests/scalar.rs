use bifrost_abi::{AbiDescriptor, Location, Scalar};
use bifrost_bridge::NativeFunction;
use bifrost_core::{AbiId, Arm64State, CpuState, GuestAddress as Addr, GuestIsa, X86State};
use bifrost_memory::{AddressSpace, Protection};
use bifrost_testkit::native;

#[test]
fn all_four_guest_descriptors_call_the_native_test_library() {
    if AbiDescriptor::native().is_none() {
        return;
    }
    for id in [
        AbiId::LinuxSysvAmd64,
        AbiId::WindowsX64,
        AbiId::LinuxAapcs64,
        AbiId::WindowsArm64,
    ] {
        let abi = AbiDescriptor::for_id(id).unwrap();
        let mut memory = AddressSpace::new(4096, 1).unwrap();
        memory
            .map(Addr(0x2000), 4096, Protection::ReadWrite)
            .unwrap();
        let sp = Addr(0x2100 + u64::from(abi.entry_stack_bias()));
        let mut cpu = if abi.isa == GuestIsa::X86_64 {
            let mut state = X86State::default();
            state.gpr[4] = sp.0;
            CpuState::X86_64(Box::new(state))
        } else {
            CpuState::AArch64(Box::new(Arm64State {
                sp,
                ..Default::default()
            }))
        };
        let return_location = if abi.isa == GuestIsa::X86_64 {
            Location::Stack(0)
        } else {
            Location::Integer(30)
        };
        abi.write_word(&mut cpu, &mut memory, return_location, 0x1000)
            .unwrap();
        let args = abi.argument_locations(&[Scalar::U64, Scalar::F64]).unwrap();
        abi.write_word(&mut cpu, &mut memory, args[0], 40).unwrap();
        abi.write_word(&mut cpu, &mut memory, args[1], 2.5_f64.to_bits())
            .unwrap();
        NativeFunction::U64F64(native::add_mixed)
            .invoke(abi, &mut cpu, &mut memory)
            .unwrap();
        assert_eq!(cpu.pc(), Addr(0x1000));
        assert_eq!(cpu.sp(), Addr(sp.0 + u64::from(abi.entry_stack_bias())));
        let result = match cpu {
            CpuState::X86_64(state) => state.xmm[0],
            CpuState::AArch64(state) => state.v[0],
            _ => unreachable!(),
        };
        assert_eq!(f64::from_bits(result as u64), 42.5);
    }
}
