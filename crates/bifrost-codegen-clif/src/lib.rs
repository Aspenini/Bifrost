//! Cranelift machine code for verified Bifröst scalar blocks.

#[allow(unsafe_code)]
mod execute;
mod lower;

pub use execute::{BlockExit, ExecutionError, PublishedBlock};

use bifrost_codegen::{CodegenBackend, CodegenOptions};
use bifrost_core::{GuestAddress, GuestIsa, HostIsa, HostTarget};
use bifrost_ir::{CodeSpan, VerifiedIrBlock};
use cranelift_codegen::{Context, control::ControlPlane, isa::OwnedTargetIsa, settings};
use std::fmt;

pub const COMPILER_VERSION: &str = "cranelift-0.134.4";
const MAX_OPERATIONS: usize = 256;

/// Lowered host code and the guest span it implements.
#[derive(Debug)]
pub struct MachineCode {
    bytes: Vec<u8>,
    guest: CodeSpan,
    guest_isa: GuestIsa,
}

impl MachineCode {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn guest_span(&self) -> CodeSpan {
        self.guest
    }

    /// Resolves offsets within the emitted single-instruction block to its guest PC.
    pub fn guest_pc_at(&self, host_offset: usize) -> Option<GuestAddress> {
        (host_offset < self.bytes.len()).then_some(self.guest.start)
    }
}

#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CompileError {
    Host(String),
    UnsupportedBlock,
    UnsupportedOperation {
        pc: GuestAddress,
        operation: &'static str,
    },
    Codegen(String),
    ExternalRelocation,
    MachineTrap,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(message) => write!(f, "Cranelift host unavailable: {message}"),
            Self::UnsupportedBlock => {
                f.write_str("expected a single bounded scalar instruction without block parameters")
            }
            Self::UnsupportedOperation { pc, operation } => {
                write!(f, "cannot lower {operation} at guest PC {:#x}", pc.0)
            }
            Self::Codegen(message) => write!(f, "Cranelift compilation failed: {message}"),
            Self::ExternalRelocation => f.write_str("machine code requires an external relocation"),
            Self::MachineTrap => f.write_str("machine code contains a host trap"),
        }
    }
}

impl std::error::Error for CompileError {}

pub struct Compiler {
    isa: OwnedTargetIsa,
}

impl Compiler {
    /// Compiles for the baseline ISA and native calling convention of the current host.
    pub fn new() -> Result<Self, CompileError> {
        let builder = cranelift_native::builder_with_options(false)
            .map_err(|error| CompileError::Host(error.into()))?;
        let isa = builder
            .finish(settings::Flags::new(settings::builder()))
            .map_err(|error| CompileError::Host(error.to_string()))?;
        Ok(Self { isa })
    }
}

impl CodegenBackend for Compiler {
    type Artifact = MachineCode;
    type Error = CompileError;

    fn target(&self) -> HostIsa {
        HostTarget::current().isa
    }

    fn compile(
        &mut self,
        block: &VerifiedIrBlock<'_>,
        _: &CodegenOptions,
    ) -> Result<MachineCode, CompileError> {
        let isa = block.target().isa();
        let block = block.as_ref();
        if !matches!(isa, GuestIsa::X86_64 | GuestIsa::AArch64)
            || !block.parameters.is_empty()
            || block.span.start != block.exit_pc
            || block.instructions.len() > MAX_OPERATIONS
            || block
                .instructions
                .iter()
                .any(|instruction| instruction.pc != block.span.start)
        {
            return Err(CompileError::UnsupportedBlock);
        }

        let mut context = Context::new();
        lower::into_function(&mut context.func, self.isa.as_ref(), block)?;
        let code = context
            .compile(self.isa.as_ref(), &mut ControlPlane::default())
            .map_err(|error| CompileError::Codegen(format!("{:?}", error.inner)))?;
        if !code.buffer.relocs().is_empty() {
            return Err(CompileError::ExternalRelocation);
        }
        if !code.buffer.traps().is_empty() {
            return Err(CompileError::MachineTrap);
        }
        Ok(MachineCode {
            bytes: code.code_buffer().to_vec(),
            guest: block.span,
            guest_isa: isa,
        })
    }
}
