//! Backend contract for lowering verified IR.

use bifrost_core::HostIsa;
use bifrost_ir::VerifiedIrBlock;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct CodegenOptions {}

/// A reusable compiler. The backend owns its compilation state and defines artifact ownership.
/// Implementations may refuse unsupported IR and must preserve the guest semantics of accepted IR.
pub trait CodegenBackend {
    type Artifact;
    type Error: std::error::Error + Send + Sync + 'static;

    fn target(&self) -> HostIsa;
    fn compile(
        &mut self,
        block: &VerifiedIrBlock<'_>,
        options: &CodegenOptions,
    ) -> Result<Self::Artifact, Self::Error>;
}
