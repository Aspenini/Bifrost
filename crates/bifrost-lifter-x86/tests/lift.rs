use bifrost_core::{FaultClass, GuestAddress as Addr};
use bifrost_decoder_x86::decode;
use bifrost_ir::{Exit, Op, Register, X86Reg};
use bifrost_lifter_x86::{LiftError, X86Lifter};
use bifrost_testkit::x86::{ARITHMETIC, CALL_AND_RETURN, MOVE_AND_ADD};

#[test]
fn verifies_every_instruction_in_the_declared_reference_sequences() {
    for sequence in ARITHMETIC
        .iter()
        .map(|case| case.bytes)
        .chain([MOVE_AND_ADD, CALL_AND_RETURN])
    {
        let mut offset = 0;
        while offset < sequence.len() {
            let pc = Addr(0x1000 + offset as u64);
            let instruction = decode(&sequence[offset..], pc).unwrap();
            let ir = X86Lifter
                .lift(&instruction, &Default::default())
                .unwrap()
                .verify()
                .unwrap();
            let block = ir.blocks().next().unwrap();
            assert_eq!(block.as_ref().span.start, pc);
            assert_eq!(block.as_ref().span.len as usize, instruction.bytes().len());
            offset += instruction.bytes().len();
        }
    }
}

#[test]
fn checks_control_targets_and_stack_access_before_updating_rsp() {
    for (bytes, call) in [(&[0xe8, 0, 0, 0, 0][..], true), (&[0xc3][..], false)] {
        let instruction = decode(bytes, Addr(0x1000)).unwrap();
        let function = X86Lifter.lift(&instruction, &Default::default()).unwrap();
        let ops: Vec<_> = function.blocks[0]
            .instructions
            .iter()
            .map(|inst| inst.op)
            .collect();
        let target = ops
            .iter()
            .position(|op| matches!(op, Op::CheckTarget { .. }))
            .unwrap();
        let stack = ops
            .iter()
            .position(|op| {
                matches!(
                    op,
                    Op::WriteRegister {
                        register: Register::X86(X86Reg::Gpr(4)),
                        ..
                    }
                )
            })
            .unwrap();
        let memory = ops
            .iter()
            .position(|op| matches!(op, Op::Load { .. } | Op::Store { .. }))
            .unwrap();
        assert!(target < stack && memory < stack);
        assert_eq!(target < memory, call);
    }
}

#[test]
fn distinguishes_illegal_execution_from_an_unsupported_opcode() {
    let pc = Addr(0x1000);
    let ud2 = decode(&[0x0f, 0x0b], pc).unwrap();
    let ir = X86Lifter.lift(&ud2, &Default::default()).unwrap();
    assert_eq!(
        ir.blocks[0].exit,
        Exit::Fault(FaultClass::IllegalInstruction)
    );
    let syscall = decode(&[0x0f, 0x05], pc).unwrap();
    assert!(matches!(
        X86Lifter.lift(&syscall, &Default::default()),
        Err(LiftError::UnsupportedInstruction {
            pc: Addr(0x1000),
            ..
        })
    ));
}
