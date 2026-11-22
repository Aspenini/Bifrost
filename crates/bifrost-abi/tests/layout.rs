use bifrost_abi::{AbiDescriptor, Location, Scalar};
use bifrost_core::AbiId;

#[test]
fn mixed_arguments_use_the_guest_register_banks_and_stack() {
    use Location::{Float, Integer, Stack};
    use Scalar::{F64, U64};
    let args = [U64, F64, U64, F64, U64];
    let sysv = AbiDescriptor::for_id(AbiId::LinuxSysvAmd64).unwrap();
    let windows = AbiDescriptor::for_id(AbiId::WindowsX64).unwrap();
    assert_eq!(
        sysv.argument_locations(&args).unwrap(),
        [Integer(7), Float(0), Integer(6), Float(1), Integer(2)]
    );
    assert_eq!(
        windows.argument_locations(&args).unwrap(),
        [Integer(1), Float(1), Integer(8), Float(3), Stack(40)]
    );
    for id in [AbiId::LinuxAapcs64, AbiId::WindowsArm64] {
        let descriptor = AbiDescriptor::for_id(id).unwrap();
        assert_eq!(
            descriptor.argument_locations(&args).unwrap(),
            [Integer(0), Float(0), Integer(1), Float(1), Integer(2)]
        );
        assert_eq!(
            descriptor.argument_locations(&[U64; 10]).unwrap()[8..],
            [Stack(0), Stack(8)]
        );
    }
    assert_eq!(
        sysv.argument_locations(&[U64; 8]).unwrap()[6..],
        [Stack(8), Stack(16)]
    );
    assert_ne!(
        AbiDescriptor::for_id(AbiId::LinuxAapcs64).unwrap().variadic,
        AbiDescriptor::for_id(AbiId::WindowsArm64).unwrap().variadic
    );
}

#[test]
fn aggregate_spills_leave_sysv_registers_available_and_exhaust_the_aapcs_bank() {
    use Location::{Integer, Stack};
    use bifrost_abi::{Argument, ArgumentLocation, ResultLocation};
    let word = Argument::Scalar(Scalar::U64);
    let sysv = AbiDescriptor::for_id(AbiId::LinuxSysvAmd64).unwrap();
    let mut args = vec![word; 5];
    args.extend([Argument::Pair64, word]);
    let plan = sysv.layout(&args, Argument::Pair64).unwrap();
    assert_eq!(
        plan.arguments[5..],
        [
            ArgumentLocation::Pair([Stack(8), Stack(16)]),
            ArgumentLocation::Word(Integer(9))
        ]
    );
    assert_eq!(plan.result, ResultLocation::Pair([Integer(0), Integer(2)]));

    let aapcs = AbiDescriptor::for_id(AbiId::LinuxAapcs64).unwrap();
    let mut args = vec![word; 7];
    args.extend([Argument::Pair64, word]);
    let plan = aapcs.layout(&args, Argument::Pair64).unwrap();
    assert_eq!(
        plan.arguments[7..],
        [
            ArgumentLocation::Pair([Stack(0), Stack(8)]),
            ArgumentLocation::Word(Stack(16))
        ]
    );

    let windows = AbiDescriptor::for_id(AbiId::WindowsX64).unwrap();
    let plan = windows
        .layout(&[Argument::Pair64, word], Argument::Pair64)
        .unwrap();
    assert_eq!(
        plan.arguments,
        [
            ArgumentLocation::IndirectPair(Integer(2)),
            ArgumentLocation::Word(Integer(8))
        ]
    );
    assert_eq!(
        plan.result,
        ResultLocation::IndirectPair {
            pointer: Integer(1),
            echo: Integer(0)
        }
    );
}
