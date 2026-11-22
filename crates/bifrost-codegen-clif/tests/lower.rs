#![cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]

use bifrost_codegen::{CodegenBackend, CodegenOptions};
use bifrost_codegen_clif::{CompileError, Compiler};
use bifrost_core::{GuestAddress, HostTarget};
use bifrost_decoder_x86::decode;
use bifrost_lifter_x86::X86Lifter;
use bifrost_testkit::{scalar_ir, x86::ARITHMETIC};

#[test]
fn emits_machine_code_and_guest_pc_maps_for_scalar_fixtures() {
    let mut compiler = Compiler::new().unwrap();
    assert_eq!(compiler.target(), HostTarget::current().isa);
    let pc = GuestAddress(0x1000);
    for bytes in ARITHMETIC.iter().map(|case| case.bytes).chain([
        &[0x74, 2][..],
        &[0xeb, 0xfc],
        &[0x0f, 0x0b],
        &[0x90],
    ]) {
        let decoded = decode(bytes, pc).unwrap();
        let function = X86Lifter
            .lift(&decoded, &Default::default())
            .unwrap()
            .verify()
            .unwrap();
        let block = function.blocks().next().unwrap();
        let code = compiler
            .compile(&block, &CodegenOptions::default())
            .unwrap();
        assert_eq!(code.guest_span(), block.as_ref().span);
        assert!(!code.bytes().is_empty());
        assert_eq!(code.guest_pc_at(0), Some(pc));
        assert_eq!(code.guest_pc_at(code.bytes().len() - 1), Some(pc));
        assert_eq!(code.guest_pc_at(code.bytes().len()), None);
    }
}

#[test]
fn refuses_wide_integers_outside_the_scalar_subset() {
    let mut function = scalar_ir();
    function.blocks[0].instructions.insert(
        0,
        bifrost_ir::Instruction {
            pc: GuestAddress(0x1000),
            op: bifrost_ir::Op::Const {
                out: bifrost_ir::ValueId(1000),
                ty: bifrost_ir::Type::I128,
                bits: u128::MAX,
            },
        },
    );
    let function = function.verify().unwrap();
    let block = function.blocks().next().unwrap();
    assert!(matches!(
        Compiler::new()
            .unwrap()
            .compile(&block, &CodegenOptions::default()),
        Err(CompileError::UnsupportedOperation {
            pc: GuestAddress(0x1000),
            ..
        })
    ));
}
