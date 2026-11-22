use bifrost_core::{AbiId, CpuFeature, GuestAddress as Addr, GuestIsa, GuestOs, GuestTarget};
use bifrost_ir::*;

fn block(id: u32, pc: u64, parameters: Vec<Parameter>, ops: Vec<Op>, exit: Exit) -> IrBlock {
    IrBlock {
        id: BlockId(id),
        span: CodeSpan {
            start: Addr(pc),
            len: 2,
        },
        parameters,
        instructions: ops
            .into_iter()
            .map(|op| Instruction { pc: Addr(pc), op })
            .collect(),
        exit_pc: Addr(pc),
        exit,
    }
}

fn function(blocks: Vec<IrBlock>) -> IrFunction {
    IrFunction {
        target: GuestTarget::new(
            GuestIsa::X86_64,
            GuestOs::None,
            AbiId::Bare,
            Default::default(),
        )
        .unwrap(),
        entry: BlockId(0),
        blocks,
    }
}

fn constant(out: u32, ty: Type, bits: u128) -> Op {
    Op::Const {
        out: ValueId(out),
        ty,
        bits,
    }
}

fn trap(ops: Vec<Op>) -> IrFunction {
    function(vec![block(0, 0x1000, vec![], ops, Exit::Trap { code: 1 })])
}

fn error(input: IrFunction) -> VerifyErrorKind {
    input.verify().unwrap_err().kind
}

#[test]
fn checks_explicit_feature_requirements_against_the_guest_profile() {
    let mut input = trap(vec![Op::RequireFeature(CpuFeature::Sse2)]);
    assert_eq!(
        error(input.clone()),
        VerifyErrorKind::UndeclaredFeature(CpuFeature::Sse2)
    );
    input.target = GuestTarget::new(
        GuestIsa::X86_64,
        GuestOs::None,
        AbiId::Bare,
        [CpuFeature::Sse2].into_iter().collect(),
    )
    .unwrap();
    assert!(input.verify().is_ok());
}

#[test]
fn joins_typed_edges_and_rejects_malformed_control_flow() {
    let edge = Edge {
        target: BlockId(1),
        args: vec![ValueId(1)],
    };
    let valid = function(vec![
        block(
            0,
            0x1000,
            vec![],
            vec![
                constant(0, Type::I1, 0),
                constant(1, Type::GuestAddress, 0x2000),
            ],
            Exit::Branch {
                condition: ValueId(0),
                yes: edge.clone(),
                no: edge,
            },
        ),
        block(
            1,
            0x2000,
            vec![Parameter {
                value: ValueId(10),
                ty: Type::GuestAddress,
            }],
            vec![],
            Exit::NextPc(ValueId(10)),
        ),
    ]);
    assert_eq!(valid.clone().verify().unwrap().blocks().len(), 2);

    let mut invalid = valid.clone();
    invalid.blocks[0].exit = Exit::Jump(Edge {
        target: BlockId(2),
        args: vec![],
    });
    assert_eq!(error(invalid), VerifyErrorKind::MissingBlock);

    let mut invalid = valid.clone();
    invalid.blocks[0].exit = Exit::Jump(Edge {
        target: BlockId(1),
        args: vec![],
    });
    assert_eq!(
        error(invalid),
        VerifyErrorKind::BranchArity {
            target: BlockId(1),
            expected: 1,
            found: 0
        }
    );

    let mut invalid = valid.clone();
    invalid.blocks[0].exit = Exit::Jump(Edge {
        target: BlockId(1),
        args: vec![ValueId(0)],
    });
    assert!(matches!(
        error(invalid),
        VerifyErrorKind::TypeMismatch { .. }
    ));

    let mut invalid = valid.clone();
    invalid.blocks[0].exit = Exit::Trap { code: 0 };
    assert_eq!(error(invalid), VerifyErrorKind::UnreachableBlock);

    let mut invalid = valid;
    invalid.blocks[1].id = BlockId(0);
    assert_eq!(error(invalid), VerifyErrorKind::DuplicateBlock);
}

#[test]
fn checks_types_definitions_and_guest_pcs() {
    assert_eq!(
        error(trap(vec![constant(0, Type::I8, 256)])),
        VerifyErrorKind::ConstantOutOfRange(Type::I8)
    );
    assert!(
        trap(vec![constant(0, Type::I128, u128::MAX)])
            .verify()
            .is_ok()
    );
    assert_eq!(
        error(trap(vec![
            constant(0, Type::I1, 1),
            constant(0, Type::I1, 0)
        ])),
        VerifyErrorKind::DuplicateValue(ValueId(0))
    );

    let add = Op::Int {
        out: ValueId(2),
        op: IntOp::Add,
        lhs: ValueId(0),
        rhs: ValueId(1),
    };
    assert_eq!(
        error(trap(vec![constant(0, Type::I64, 0), add])),
        VerifyErrorKind::UndefinedValue(ValueId(1))
    );
    assert_eq!(
        error(trap(vec![
            constant(0, Type::F64, 0),
            constant(1, Type::F64, 0),
            add
        ])),
        VerifyErrorKind::ExpectedInteger(ValueId(0))
    );

    let register = Register::Arm64(Arm64Reg::Sp);
    assert_eq!(
        error(trap(vec![Op::ReadRegister {
            out: ValueId(0),
            register
        }])),
        VerifyErrorKind::InvalidRegister(register)
    );
    let register = Register::X86(X86Reg::Gpr(16));
    assert_eq!(
        error(trap(vec![Op::ReadRegister {
            out: ValueId(0),
            register
        }])),
        VerifyErrorKind::InvalidRegister(register)
    );

    let mut invalid = trap(vec![constant(0, Type::I64, 0)]);
    invalid.blocks[0].instructions[0].pc = Addr(0xfff);
    assert_eq!(error(invalid), VerifyErrorKind::PcOutOfOrderOrSpan);
    let mut invalid = trap(vec![]);
    invalid.blocks[0].span.start = Addr(u64::MAX);
    assert_eq!(error(invalid), VerifyErrorKind::InvalidSpan);
}

#[test]
fn requires_single_use_memory_guards_with_exact_permissions_and_layout() {
    let layout = MemoryLayout {
        ty: Type::I64,
        alignment: 8,
        endian: Endian::Little,
    };
    let addr = constant(0, Type::GuestAddress, 0x2000);
    let guard = Op::Guard {
        out: ValueId(1),
        address: ValueId(0),
        access: MemoryAccess::Read,
        layout,
    };
    let load = Op::Load {
        out: ValueId(2),
        guard: ValueId(1),
        layout,
    };
    assert!(trap(vec![addr, guard, load]).verify().is_ok());
    assert_eq!(
        error(trap(vec![addr, load])),
        VerifyErrorKind::UndefinedValue(ValueId(1))
    );
    assert_eq!(
        error(trap(vec![addr, guard])),
        VerifyErrorKind::UnusedGuard(ValueId(1))
    );
    assert_eq!(
        error(trap(vec![addr, guard, load, load])),
        VerifyErrorKind::IncompatibleGuard(ValueId(1))
    );

    let store = Op::Store {
        guard: ValueId(1),
        value: ValueId(3),
        layout,
    };
    let scalar = constant(3, Type::I64, 0);
    assert_eq!(
        error(trap(vec![addr, scalar, guard, store])),
        VerifyErrorKind::IncompatibleGuard(ValueId(1))
    );
    let write_guard = Op::Guard {
        out: ValueId(1),
        address: ValueId(0),
        access: MemoryAccess::Write,
        layout,
    };
    assert!(
        trap(vec![addr, scalar, write_guard, store])
            .verify()
            .is_ok()
    );

    let bad_layout = MemoryLayout {
        alignment: 3,
        ..layout
    };
    let bad_guard = Op::Guard {
        out: ValueId(1),
        address: ValueId(0),
        access: MemoryAccess::Read,
        layout: bad_layout,
    };
    assert_eq!(
        error(trap(vec![addr, bad_guard])),
        VerifyErrorKind::InvalidMemoryLayout(bad_layout)
    );

    let mut invalid = trap(vec![addr, guard, load]);
    invalid.blocks[0].instructions[2].pc = Addr(0x1001);
    invalid.blocks[0].exit_pc = Addr(0x1001);
    assert_eq!(
        error(invalid),
        VerifyErrorKind::IncompatibleGuard(ValueId(1))
    );
}
