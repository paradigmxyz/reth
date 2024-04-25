use libloading::{Library, Symbol};
use reth_primitives::B256;
use revm_jit::{debug_time, trace_time, EvmCompilerFn};
use std::{
    collections::{hash_map::Entry, HashMap},
    path::Path,
};

#[cfg(unix)]
use libloading::os::unix as os;
#[cfg(windows)]
use libloading::os::windows as os;

/// A handle to an open EVM compiler-generated DLL.
#[derive(Debug)]
pub struct EvmCompilerDll {
    lib: Library,
    cache: HashMap<B256, os::Symbol<Option<EvmCompilerFn>>>,
}

impl EvmCompilerDll {
    /// Open a DLL at the given path.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the DLL is a valid EVM compiler DLL.
    pub unsafe fn open(path: &Path) -> Result<Self, libloading::Error> {
        debug_time!("dlopen", || Self::open_inner(path))
    }

    unsafe fn open_inner(filename: &Path) -> Result<Self, libloading::Error> {
        Library::new(filename).map(|lib| Self { lib, cache: HashMap::new() })
    }

    /// Returns the function for the given bytecode hash.
    #[inline]
    pub fn get_function(
        &mut self,
        hash: B256,
    ) -> Result<Option<Symbol<'_, EvmCompilerFn>>, libloading::Error> {
        trace_time!("dlsym", || self.get_function_inner(hash))
    }

    #[inline]
    fn get_function_inner(
        &mut self,
        hash: B256,
    ) -> Result<Option<Symbol<'_, EvmCompilerFn>>, libloading::Error> {
        match self.cache.entry(hash) {
            Entry::Occupied(entry) => {
                // SAFETY: We have ownership over the library, so the lifetime is valid.
                Ok(unsafe { Symbol::from_raw(entry.get().clone(), &self.lib) }.lift_option())
            }
            Entry::Vacant(entry) => {
                let mut symbol_buffer = crate::SymbolBuffer::symbol(&hash);
                let symbol_bytes = symbol_buffer.make_cstr().to_bytes_with_nul();
                // SAFETY: The symbol is known to have the `EvmCompilerFn` type.
                let symbol = unsafe { self.lib.get::<Option<EvmCompilerFn>>(symbol_bytes) }?;
                entry.insert(unsafe { symbol.clone().into_raw() });
                Ok(symbol.lift_option())
            }
        }
    }

    /// Unload the library.
    pub fn close(self) -> Result<(), libloading::Error> {
        self.lib.close()
    }
}
