//! Raw guest call fixtures for the typed native bridge.

use bifrost_runtime::{
    abi::{AbiDescriptor, ArgumentLocation, Pair64, ResultLocation},
    bridge::{
        BridgeError, BridgeExecutor, BridgeRegistry, BufferMode, CallbackStack, NativeBuffer,
        NativeCallback, NativeFunction, NativeToken,
    },
    core::{
        AbiId, Arm64State, CpuState, GuestAddress as Addr, GuestIsa, GuestOs, GuestTarget,
        GuestThread, GuestThreadId, X86State,
    },
    memory::{AddressSpace, GuestMemory, GuestMemoryRead, Protection},
    raw::{ExecutionBudget, Executor, ExitReason, RawInterpreter},
};
use bifrost_testkit::native;

const CODE: Addr = Addr(0x1000);
const STACK: Addr = Addr(0x2000);
const DATA: Addr = Addr(0x3000);
const CALLER_SP: Addr = Addr(0x2f80);
const ADD: Addr = Addr(0x4000);
const MIXED: Addr = Addr(0x4010);
const PAIR: Addr = Addr(0x4020);
const BUFFER: Addr = Addr(0x4030);
const HANDLE: Addr = Addr(0x4040);
const CALLBACK: Addr = Addr(0x4050);
const STACK_WORDS: Addr = Addr(0x4060);
const SIGNED: Addr = Addr(0x4070);
const GUEST_CALLBACK: Addr = Addr(0x1200);
const FAULTING_CALLBACK: Addr = Addr(0x1240);
const RECURSIVE_CALLBACK: Addr = Addr(0x1280);
const CALLBACK_STACK: Addr = Addr(0x8000);
const CALLBACK_RETURN: Addr = Addr(0xb000);

pub const GUEST_ABIS: [AbiId; 4] = [
    AbiId::LinuxSysvAmd64,
    AbiId::WindowsX64,
    AbiId::LinuxAapcs64,
    AbiId::WindowsArm64,
];

#[derive(Clone, Copy, Debug)]
pub enum Backend {
    Interpreter,
    Jit,
}

#[derive(Debug, PartialEq)]
pub struct Report {
    pub scalar: u64,
    pub stack_scalar: u64,
    pub signed: i64,
    pub mixed: f64,
    pub aggregate: Pair64,
    pub buffer_sum: u64,
    pub buffer: [u8; 3],
    pub handle: u64,
    pub callback: u64,
}

pub fn demo(id: AbiId, backend: Backend) -> Result<Report, BridgeError> {
    let mut fixture = Fixture::new(id, backend)?;
    let scalar = fixture.call_word(ADD, [19, 23], 2)?;
    let stack_scalar = fixture.call_words(STACK_WORDS, &[1, 2, 3, 4, 5, 6, 7, 8, 9], 2)?;
    let signed = fixture.call_word(SIGNED, [(-20_i64) as u64, 22], 2)? as i64;
    let mixed = f64::from_bits(fixture.call_word(MIXED, [40, 2.5_f64.to_bits()], 2)?);
    let aggregate = fixture.pair(Pair64 {
        first: 12,
        second: 29,
    })?;
    fixture.memory.write(DATA, &[1, 2, 3])?;
    let buffer_sum = fixture.call_word(BUFFER, [DATA.0, 3], 2)?;
    let mut buffer = [0; 3];
    fixture.memory.read(DATA, &mut buffer)?;
    let handle = fixture.registry.insert_handle("fixture", NativeToken(39))?;
    let handle = fixture.call_word(HANDLE, [handle.bits(), 3], 2)?;
    let callback_steps = if fixture.abi.isa == GuestIsa::AArch64 {
        22
    } else {
        14
    };
    let callback = fixture.call_word(CALLBACK, [20, GUEST_CALLBACK.0], callback_steps)?;
    Ok(Report {
        scalar,
        stack_scalar,
        signed,
        mixed,
        aggregate,
        buffer_sum,
        buffer,
        handle,
        callback,
    })
}

struct Fixture {
    abi: &'static AbiDescriptor,
    memory: AddressSpace,
    registry: BridgeRegistry,
    backend: Backend,
}

impl Fixture {
    fn new(id: AbiId, backend: Backend) -> Result<Self, BridgeError> {
        let abi = AbiDescriptor::for_id(id)?;
        let mut memory = AddressSpace::new(4096, 5)?;
        for (base, len) in [
            (CODE, 4096),
            (STACK, 4096),
            (DATA, 4096),
            (CALLBACK_STACK, 8192),
        ] {
            memory.map(base, len, Protection::ReadWrite)?;
        }
        for destination in [
            ADD,
            MIXED,
            PAIR,
            BUFFER,
            HANDLE,
            CALLBACK,
            STACK_WORDS,
            SIGNED,
        ] {
            let site = call_site(destination);
            memory.write(site, &call_instruction(abi.isa, site, destination))?;
        }
        for (entry, destination, second) in [
            (GUEST_CALLBACK, ADD, 1),
            (RECURSIVE_CALLBACK, CALLBACK, RECURSIVE_CALLBACK.0),
        ] {
            memory.write(entry, &callback_code(abi, entry, destination, second))?;
        }
        let illegal: &[u8] = if abi.isa == GuestIsa::AArch64 {
            &[0; 4]
        } else {
            &[0x0f, 0x0b]
        };
        memory.write(FAULTING_CALLBACK, illegal)?;
        memory.protect(CODE, 4096, Protection::ReadExecute)?;
        Ok(Self {
            abi,
            memory,
            registry: bindings()?,
            backend,
        })
    }

    fn call_word(
        &mut self,
        destination: Addr,
        words: [u64; 2],
        steps: u64,
    ) -> Result<u64, BridgeError> {
        self.call_words(destination, &words, steps)
    }

    fn call_words(
        &mut self,
        destination: Addr,
        words: &[u64],
        steps: u64,
    ) -> Result<u64, BridgeError> {
        use bifrost_runtime::abi::{Argument, Scalar};
        let kind = if destination == MIXED {
            Scalar::F64
        } else {
            Scalar::U64
        };
        let mut arguments = vec![Scalar::U64; words.len()];
        if destination == MIXED {
            arguments[1] = Scalar::F64;
        }
        let locations = self.abi.argument_locations(&arguments)?;
        let mut cpu = self.frame(destination);
        for (location, &bits) in locations.into_iter().zip(words) {
            self.abi
                .write_word(&mut cpu, &mut self.memory, location, bits)?;
        }
        let kit = self.abi.layout(&[], Argument::Scalar(kind))?;
        let ResultLocation::Word(result) = kit.result else {
            unreachable!()
        };
        let finished = self.run(cpu, steps)?;
        self.abi
            .read_result_register(&finished, result)
            .map_err(Into::into)
    }

    fn pair(&mut self, value: Pair64) -> Result<Pair64, BridgeError> {
        let mut cpu = self.frame(PAIR);
        let signature = NativeFunction::Pair(native::swap_pair).signature();
        let layout = self.abi.layout(signature.0, signature.1)?;
        match layout.arguments[0] {
            ArgumentLocation::Pair([first, second]) => {
                self.abi
                    .write_word(&mut cpu, &mut self.memory, first, value.first)?;
                self.abi
                    .write_word(&mut cpu, &mut self.memory, second, value.second)?;
            }
            ArgumentLocation::IndirectPair(pointer) => {
                self.memory.write(DATA, &value.guest_bytes())?;
                self.abi
                    .write_word(&mut cpu, &mut self.memory, pointer, DATA.0)?;
            }
            _ => unreachable!(),
        }
        let result_buffer = Addr(DATA.0 + 32);
        if let ResultLocation::IndirectPair { pointer, .. } = layout.result {
            self.abi
                .write_word(&mut cpu, &mut self.memory, pointer, result_buffer.0)?;
        }
        let cpu = self.run(cpu, 2)?;
        match layout.result {
            ResultLocation::Pair([first, second]) => Ok(Pair64 {
                first: self.abi.read_result_register(&cpu, first)?,
                second: self.abi.read_result_register(&cpu, second)?,
            }),
            ResultLocation::IndirectPair { echo, .. } => {
                assert_eq!(self.abi.read_result_register(&cpu, echo)?, result_buffer.0);
                let mut bytes = [0; 16];
                self.memory.read(result_buffer, &mut bytes)?;
                Ok(Pair64::from_guest_bytes(bytes))
            }
            _ => unreachable!(),
        }
    }

    fn frame(&self, destination: Addr) -> CpuState {
        let pc = call_site(destination);
        let sp = Addr(CALLER_SP.0 - u64::from(self.abi.entry_stack_bias()));
        match self.abi.isa {
            GuestIsa::X86_64 => {
                let mut state = X86State {
                    rip: pc,
                    ..Default::default()
                };
                state.gpr[4] = sp.0;
                CpuState::X86_64(Box::new(state))
            }
            GuestIsa::AArch64 => CpuState::AArch64(Box::new(Arm64State {
                pc,
                sp,
                ..Default::default()
            })),
            _ => unreachable!(),
        }
    }

    fn run(&mut self, mut cpu: CpuState, steps: u64) -> Result<CpuState, BridgeError> {
        if let CpuState::X86_64(state) = &mut cpu {
            state.gpr[4] = CALLER_SP.0;
        }
        let expected = Addr(
            cpu.pc().0
                + if self.abi.isa == GuestIsa::X86_64 {
                    5
                } else {
                    4
                },
        );
        let mut thread = GuestThread {
            id: GuestThreadId(1),
            cpu,
            pending_fault: None,
        };
        let target = GuestTarget::new(self.abi.isa, GuestOs::None, AbiId::Bare, Default::default())
            .expect("bare profile");
        let stack = CallbackStack::new(CALLBACK_STACK, 2, CALLBACK_RETURN)?;
        let mut trace = ();
        let budget = ExecutionBudget {
            instructions: steps,
        };
        let result = match self.backend {
            Backend::Interpreter => {
                let engine = RawInterpreter::new(&target, &mut self.memory, &mut trace)
                    .map_err(|e| BridgeError::Backend(Box::new(e)))?;
                BridgeExecutor::new(self.abi, &self.registry, engine)
                    .with_callback_stack(stack)?
                    .run(&mut thread, budget)?
            }
            Backend::Jit => run_jit(
                &target,
                self.abi,
                &self.registry,
                &mut self.memory,
                &mut thread,
                stack,
                budget,
            )?,
        };
        assert_eq!(result, ExitReason::BudgetExhausted);
        assert_eq!(thread.cpu.pc(), expected);
        assert_eq!(thread.cpu.sp(), CALLER_SP);
        Ok(thread.cpu)
    }
}

#[cfg(all(
    feature = "jit-cranelift",
    any(
        all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ),
        all(target_arch = "aarch64", target_os = "linux")
    )
))]
pub const JIT_BUILT: bool = true;

#[cfg(not(all(
    feature = "jit-cranelift",
    any(
        all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ),
        all(target_arch = "aarch64", target_os = "linux")
    )
)))]
pub const JIT_BUILT: bool = false;

#[cfg(all(
    feature = "jit-cranelift",
    any(
        all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ),
        all(target_arch = "aarch64", target_os = "linux")
    )
))]
fn run_jit(
    target: &GuestTarget,
    abi: &AbiDescriptor,
    registry: &BridgeRegistry,
    memory: &mut AddressSpace,
    thread: &mut GuestThread,
    stack: CallbackStack,
    budget: ExecutionBudget,
) -> Result<ExitReason, BridgeError> {
    use bifrost_runtime::raw::{RawJit, SystemJitMemory};
    let mut trace = ();
    let engine = RawJit::new(target, memory, &mut trace, SystemJitMemory)
        .map_err(|e| BridgeError::Backend(Box::new(e)))?;
    BridgeExecutor::new(abi, registry, engine)
        .with_callback_stack(stack)?
        .run(thread, budget)
}

#[cfg(not(all(
    feature = "jit-cranelift",
    any(
        all(
            target_arch = "x86_64",
            any(target_os = "linux", target_os = "windows")
        ),
        all(target_arch = "aarch64", target_os = "linux")
    )
)))]
fn run_jit(
    _: &GuestTarget,
    _: &AbiDescriptor,
    _: &BridgeRegistry,
    _: &mut AddressSpace,
    _: &mut GuestThread,
    _: CallbackStack,
    _: ExecutionBudget,
) -> Result<ExitReason, BridgeError> {
    Err(BridgeError::UnsupportedHost)
}

#[allow(unsafe_code)]
fn bindings() -> Result<BridgeRegistry, BridgeError> {
    let mut result = BridgeRegistry::default();
    // SAFETY: Both statically linked reference functions obey the synchronous binding contracts.
    let (buffer, callback) = unsafe {
        (
            NativeBuffer::new(native::increment_buffer, BufferMode::InOut, 64),
            NativeCallback::new(native::call_twice),
        )
    };
    for (name, entry, function) in [
        ("add", ADD, NativeFunction::U64Binary(native::add_u64)),
        ("mixed", MIXED, NativeFunction::U64F64(native::add_mixed)),
        ("pair", PAIR, NativeFunction::Pair(native::swap_pair)),
        ("buffer", BUFFER, NativeFunction::Buffer(buffer)),
        ("handle", HANDLE, NativeFunction::HandleU64(native::add_u64)),
        ("callback", CALLBACK, NativeFunction::Callback(callback)),
        (
            "weighted",
            STACK_WORDS,
            NativeFunction::U64Nine(native::weighted),
        ),
        (
            "signed",
            SIGNED,
            NativeFunction::I64Binary(native::subtract_i64),
        ),
    ] {
        result.register("fixture", name, entry, function)?;
    }
    for entry in [GUEST_CALLBACK, FAULTING_CALLBACK, RECURSIVE_CALLBACK] {
        result.allow_callback("fixture", entry)?;
    }
    Ok(result)
}

const fn call_site(destination: Addr) -> Addr {
    Addr(CODE.0 + (destination.0 - ADD.0) * 2)
}

fn call_instruction(isa: GuestIsa, pc: Addr, destination: Addr) -> Vec<u8> {
    if isa == GuestIsa::AArch64 {
        let scaled = (destination.0 as i64 - pc.0 as i64) / 4;
        (0x9400_0000 | (scaled as u32 & 0x03ff_ffff))
            .to_le_bytes()
            .to_vec()
    } else {
        let offset = destination.0 as i64 - (pc.0 as i64 + 5);
        let mut bytes = vec![0xe8];
        bytes.extend(i32::try_from(offset).unwrap().to_le_bytes());
        bytes
    }
}

fn callback_code(abi: &AbiDescriptor, entry: Addr, destination: Addr, second: u64) -> Vec<u8> {
    if abi.isa == GuestIsa::AArch64 {
        let prelude = [
            0xd100_43ff_u32,                                     // sub sp,sp,#16
            0x9100_03e9,                                         // mov x9,sp
            0xf900_013e,                                         // str x30,[x9]
            0xd280_0001 | (u32::try_from(second).unwrap() << 5), // movz x1,#second
        ];
        let mut bytes: Vec<_> = prelude.into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend(call_instruction(abi.isa, Addr(entry.0 + 16), destination));
        let epilogue = [
            0x9100_03e9_u32, // mov x9,sp
            0xf940_013e,     // ldr x30,[x9]
            0x9100_43ff,     // add sp,sp,#16
            0xd65f_03c0,     // ret
        ];
        bytes.extend(epilogue.into_iter().flat_map(u32::to_le_bytes));
        bytes
    } else {
        let space = 8 + abi.shadow_bytes;
        let mov = if abi.integer_args[1] == 2 { 0xba } else { 0xbe };
        let mut bytes = vec![0x48, 0x83, 0xec, space, 0x48, mov];
        bytes.extend(second.to_le_bytes());
        bytes.extend(call_instruction(abi.isa, Addr(entry.0 + 14), destination));
        bytes.extend([0x48, 0x83, 0xc4, space, 0xc3]);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backends() -> impl Iterator<Item = Backend> {
        [Backend::Interpreter, Backend::Jit]
            .into_iter()
            .filter(|mode| !matches!(mode, Backend::Jit) || JIT_BUILT)
    }

    #[test]
    fn translated_calls_match_for_all_guest_profiles_and_enabled_backends() {
        if AbiDescriptor::native().is_none() {
            return;
        }
        for guest in GUEST_ABIS {
            for backend in backends() {
                assert_eq!(
                    demo(guest, backend).unwrap(),
                    Report {
                        scalar: 42,
                        stack_scalar: 285,
                        signed: -42,
                        mixed: 42.5,
                        aggregate: Pair64 {
                            first: 30,
                            second: 12
                        },
                        buffer_sum: 6,
                        buffer: [2, 3, 4],
                        handle: 42,
                        callback: 43
                    },
                    "{guest:?} with {backend:?}"
                );
            }
        }
    }

    #[test]
    fn checks_callback_policy_fault_depth_and_budget_in_both_guest_isas() {
        if AbiDescriptor::native().is_none() {
            return;
        }
        for guest in GUEST_ABIS {
            for backend in backends() {
                let mut fixture = Fixture::new(guest, backend).unwrap();
                let unregistered = Addr(GUEST_CALLBACK.0 + 4);
                assert!(
                    matches!(fixture.call_word(CALLBACK, [20, unregistered.0], 14),
                    Err(BridgeError::DisallowedCallback(address)) if address == unregistered)
                );
                assert!(
                    matches!(fixture.call_word(CALLBACK, [20, FAULTING_CALLBACK.0], 14),
                    Err(BridgeError::CallbackFault(fault)) if fault.pc == FAULTING_CALLBACK)
                );
                assert!(matches!(
                    fixture.call_word(CALLBACK, [20, GUEST_CALLBACK.0], 2),
                    Err(BridgeError::CallbackBudget)
                ));
                assert!(matches!(
                    fixture.call_word(CALLBACK, [20, RECURSIVE_CALLBACK.0], 100),
                    Err(BridgeError::CallbackDepth)
                ));
            }
        }
    }
}
