use bifrost_core::GuestAddress as Addr;
use bifrost_decoder_arm64::{DecodeError, Operand, Operation, Reg, decode};

fn op(word: u32) -> Operation {
    let decoded = decode(word.to_le_bytes(), Addr(0x1000)).unwrap();
    assert_eq!(decoded.bytes(), word.to_le_bytes());
    assert_eq!(decoded.next_pc(), Addr(0x1004));
    decoded.operation()
}

#[test]
fn distinguishes_sp_and_zero_and_sign_extends_branch_targets() {
    assert_eq!(
        op(0x9140_07ff),
        Operation::Arithmetic {
            dst: Reg::Sp,
            lhs: Reg::Sp,
            rhs: Operand::Immediate(4096),
            sub: false,
            flags: false
        }
    );
    assert_eq!(
        op(0xeb01_001f),
        Operation::Arithmetic {
            dst: Reg::Zero,
            lhs: Reg::X(0),
            rhs: Operand::Register(Reg::X(1)),
            sub: true,
            flags: true
        }
    );
    assert_eq!(
        op(0x17ff_ffff),
        Operation::Branch {
            target: Addr(0xffc),
            link: false
        }
    );
    assert_eq!(
        op(0x54ff_ffe1),
        Operation::BranchFlags {
            target: Addr(0xffc),
            condition: 1
        }
    );
    assert_eq!(
        op(0xf940_0420),
        Operation::Memory {
            reg: Reg::X(0),
            base: Reg::X(1),
            offset: 8,
            load: true
        }
    );
}

#[test]
fn rejects_other_forms_and_unaligned_fetches() {
    assert_eq!(op(0x8bc1_0400), Operation::Undefined); // reserved ROR form of add
    assert_eq!(
        op(0xf940_03e0),
        Operation::Memory {
            reg: Reg::X(0),
            base: Reg::Sp,
            offset: 0,
            load: true
        }
    ); // ldr x0,[sp]
    assert_eq!(op(0xd400_0002), Operation::Unsupported); // hvc #0
    assert_eq!(op(0x0000_1234), Operation::Undefined);
    assert_eq!(op(0xd503_3bbf), Operation::Fence);
    assert_eq!(op(0xd503_3b9f), Operation::Fence);
    assert_eq!(op(0xd503_3f9f), Operation::Fence);
    assert_eq!(op(0xd503_3fdf), Operation::InstructionSyncBarrier);
    assert_eq!(
        decode([0; 4], Addr(1)),
        Err(DecodeError::Alignment(Addr(1)))
    );
}

#[test]
fn identifies_pointer_strip_registers_and_address_classes() {
    use bifrost_core::{
        Arm64PointerAction, Arm64PointerAuth, Arm64PointerKey, Arm64PointerModifier,
    };
    for (word, key, pointer_register) in [
        (0xd503_20ff, Arm64PointerKey::InstructionA, 30),
        (0xdac1_43e0, Arm64PointerKey::InstructionA, 0),
        (0xdac1_47e1, Arm64PointerKey::DataA, 1),
    ] {
        assert_eq!(
            op(word),
            Operation::PointerAuth(Arm64PointerAuth {
                action: Arm64PointerAction::Strip,
                key,
                pointer_register,
                modifier: Arm64PointerModifier::Zero,
            })
        );
    }
}

#[test]
fn decodes_the_full_supervisor_call_immediate() {
    assert_eq!(
        op(0xd41f_ffe1),
        Operation::SupervisorCall { immediate: 0xffff }
    );
}

#[test]
fn forms_pc_relative_addresses_for_adr_and_adrp() {
    for (word, expected) in [
        (0x1000_0040u32, 0x245c),
        (0x10ff_ffe0, 0x2450),
        (0xb000_0000, 0x3000),
        (0xf0ff_ffe0, 0x1000),
    ] {
        let decoded = decode(word.to_le_bytes(), Addr(0x2454)).unwrap();
        assert_eq!(
            decoded.operation(),
            Operation::Move {
                dst: Reg::X(0),
                src: Operand::Immediate(expected),
            }
        );
    }
}

#[test]
fn rejects_reserved_logical_immediate_masks() {
    assert_eq!(op(0x3240_0000), Operation::Undefined); // 32-bit form with N=1
    assert_eq!(op(0xb240_fc00), Operation::Undefined); // all-one element
    assert_eq!(op(0xb200_fc00), Operation::Undefined); // no element size
}
