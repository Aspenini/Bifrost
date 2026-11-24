use bifrost_core::{CpuFeature, FaultClass, GuestAddress as Addr};
use bifrost_decoder_arm64::decode;
use bifrost_ir::{Exit, Op};
use bifrost_lifter_arm64::{Arm64Lifter, LiftError};

#[test]
fn emits_verified_ir_with_arm64_pc_at_every_effect() {
    for word in [
        0xd280_0520_u32, // movz x0,#41
        0xf2a2_4680,     // movk x0,#0x1234,lsl #16
        0xab01_0000,     // adds x0,x0,x1
        0xeb01_001f,     // cmp x0,x1
        0xf900_0420,     // str x0,[x1,#8]
        0xf940_0420,     // ldr x0,[x1,#8]
        0x5400_004d,     // b.le +8
        0xb500_0040,     // cbnz x0,+8
        0x9400_0002,     // bl +8
        0xd65f_03c0,     // ret
        0xd503_3bbf,     // dmb ish
        0x0000_0000,     // udf #0
    ] {
        let pc = Addr(0x1000);
        let decoded = decode(word.to_le_bytes(), pc).unwrap();
        let verified = Arm64Lifter
            .lift(decoded, &Default::default())
            .unwrap()
            .verify()
            .unwrap();
        let block = verified.blocks().next().unwrap();
        assert_eq!(block.as_ref().span.start, pc);
        assert_eq!(block.as_ref().span.len, 4);
        assert!(block.as_ref().instructions.iter().all(|inst| inst.pc == pc));
        if word == 0xd503_3bbf {
            assert!(matches!(block.as_ref().instructions[0].op, Op::Fence));
        }
        if word == 0 {
            assert_eq!(
                block.as_ref().exit,
                Exit::Fault(FaultClass::IllegalInstruction)
            );
        }
    }
}

#[test]
fn unsupported_words_and_wrong_isa_features_are_typed() {
    let pc = Addr(0x1000);
    let hvc = decode(0xd400_0002_u32.to_le_bytes(), pc).unwrap();
    assert!(matches!(
        Arm64Lifter.lift(hvc, &Default::default()),
        Err(LiftError::UnsupportedInstruction {
            pc: Addr(0x1000),
            word: 0xd400_0002,
            ..
        })
    ));
    let wrong = [CpuFeature::Sse2].into_iter().collect();
    assert!(matches!(
        Arm64Lifter.lift(hvc, &wrong),
        Err(LiftError::Target(_))
    ));
}
