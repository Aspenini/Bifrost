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
