use crate::{BridgeError, NativeFunction, PreparedCall};
use bifrost_abi::AbiDescriptor;
use bifrost_core::{CpuState, GuestAddress, GuestMemory};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleId(u64);

impl HandleId {
    pub const fn bits(self) -> u64 {
        self.0
    }
}

/// Value extracted from a module-owned handle for typed native functions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeToken(pub u64);

#[derive(Default)]
struct Module {
    symbols: BTreeMap<String, GuestAddress>,
    callbacks: BTreeSet<GuestAddress>,
}

struct Entry {
    module: String,
    function: NativeFunction,
}
struct Handle {
    module: String,
    value: Arc<dyn Any + Send + Sync>,
}

/// Owns guest symbol bindings and opaque resources. Each guest address is unique in a registry.
#[derive(Default)]
pub struct BridgeRegistry {
    modules: BTreeMap<String, Module>,
    entries: BTreeMap<GuestAddress, Entry>,
    handles: BTreeMap<u64, Handle>,
}

impl BridgeRegistry {
    pub fn register(
        &mut self,
        module: &str,
        symbol: &str,
        address: GuestAddress,
        function: NativeFunction,
    ) -> Result<(), BridgeError> {
        if module.is_empty() || symbol.is_empty() {
            return Err(BridgeError::EmptyBindingName);
        }
        if self.entries.contains_key(&address)
            || self
                .modules
                .values()
                .any(|module| module.callbacks.contains(&address))
        {
            return Err(BridgeError::DuplicateEntry(address));
        }
        let namespace = self.modules.entry(module.into()).or_default();
        if namespace.symbols.contains_key(symbol) {
            return Err(BridgeError::DuplicateSymbol {
                module: module.into(),
                symbol: symbol.into(),
            });
        }
        namespace.symbols.insert(symbol.into(), address);
        self.entries.insert(
            address,
            Entry {
                module: module.into(),
                function,
            },
        );
        Ok(())
    }

    pub fn address(&self, module: &str, symbol: &str) -> Option<GuestAddress> {
        self.modules.get(module)?.symbols.get(symbol).copied()
    }

    pub fn contains(&self, address: GuestAddress) -> bool {
        self.entries.contains_key(&address)
    }

    pub fn allow_callback(
        &mut self,
        module: &str,
        address: GuestAddress,
    ) -> Result<(), BridgeError> {
        if self.entries.contains_key(&address) {
            return Err(BridgeError::DuplicateEntry(address));
        }
        self.modules
            .get_mut(module)
            .ok_or_else(|| BridgeError::UnknownModule(module.into()))?
            .callbacks
            .insert(address);
        Ok(())
    }

    pub fn insert_handle<T: Any + Send + Sync>(
        &mut self,
        module: &str,
        value: T,
    ) -> Result<HandleId, BridgeError> {
        self.require_module(module)?;
        let raw = NEXT_HANDLE
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| BridgeError::HandleIdsExhausted)?;
        self.handles.insert(
            raw,
            Handle {
                module: module.into(),
                value: Arc::new(value),
            },
        );
        Ok(HandleId(raw))
    }

    pub fn resolve_handle<T: Any + Send + Sync>(
        &self,
        module: &str,
        handle: HandleId,
    ) -> Result<Arc<T>, BridgeError> {
        let record = self
            .handles
            .get(&handle.0)
            .filter(|record| record.module == module)
            .ok_or(BridgeError::InvalidHandle(handle.0))?;
        record
            .value
            .clone()
            .downcast::<T>()
            .map_err(|_| BridgeError::WrongHandleType(handle.0))
    }

    pub fn close_handle(&mut self, module: &str, handle: HandleId) -> Result<(), BridgeError> {
        self.handles
            .get(&handle.0)
            .filter(|record| record.module == module)
            .ok_or(BridgeError::InvalidHandle(handle.0))?;
        self.handles.remove(&handle.0);
        Ok(())
    }

    pub fn invoke(
        &self,
        abi: &AbiDescriptor,
        cpu: &mut CpuState,
        memory: &mut dyn GuestMemory,
    ) -> Result<(), BridgeError> {
        let mut call = self.prepare(abi, cpu, memory)?;
        let result = call.call_native(None)?;
        call.finish(cpu, memory, result)
    }

    pub(crate) fn prepare<'a>(
        &self,
        abi: &'a AbiDescriptor,
        cpu: &CpuState,
        memory: &dyn GuestMemory,
    ) -> Result<PreparedCall<'a>, BridgeError> {
        let address = cpu.pc();
        abi.validate_code_address(address)?;
        let entry = self
            .entries
            .get(&address)
            .ok_or(BridgeError::UnknownEntry(address))?;
        let mut call = PreparedCall::prepare(entry.function, abi, cpu, memory)?;
        if let Some(raw) = call.handle() {
            let token = self.resolve_handle::<NativeToken>(&entry.module, HandleId(raw))?;
            call.resolve_handle(token.0);
        }
        if let Some(callback) = call.callback_address() {
            if !self.modules[&entry.module].callbacks.contains(&callback) {
                return Err(BridgeError::DisallowedCallback(callback));
            }
            abi.validate_code_address(callback)?;
            let mut bytes = [0; 4];
            let width = if abi.isa == bifrost_core::GuestIsa::AArch64 {
                4
            } else {
                1
            };
            memory.fetch(callback, &mut bytes[..width])?;
        }
        Ok(call)
    }

    pub(crate) fn require_module(&self, module: &str) -> Result<(), BridgeError> {
        if self.modules.contains_key(module) {
            Ok(())
        } else {
            Err(BridgeError::UnknownModule(module.into()))
        }
    }
}
