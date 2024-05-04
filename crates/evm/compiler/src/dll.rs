use libloading::Library;
use reth_primitives::B256;
use revm::primitives::SpecId;
use revm_jit::{debug_time, trace_time, EvmCompilerFn};
use std::{
    collections::{hash_map::Entry, HashMap},
    marker::PhantomData,
    path::Path,
};

/// A handle to an open EVM compiler-generated DLL.
#[derive(Debug)]
pub struct EvmCompilerDll {
    /// The underlying shared library.
    pub lib: Library,
    /// The cache of loaded functions.
    pub cache: HashMap<B256, Option<EvmCompilerFn>>,
}

impl EvmCompilerDll {
    /// Open a DLL in the given output directory.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the resulting path is a valid EVM compiler DLL.
    pub unsafe fn open_in(out_dir: &Path, spec_id: SpecId) -> Result<Self, libloading::Error> {
        let filename = out_dir.join(crate::dll_filename_spec_id(spec_id));
        Self::open(&filename)
    }

    /// Open a DLL at the given path.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the given file is a valid EVM compiler DLL.
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
    ) -> Result<Option<EvmCompilerSymbol<'_>>, libloading::Error> {
        trace_time!("dlsym", || self.get_function_inner(hash))
    }

    #[inline]
    fn get_function_inner(
        &mut self,
        hash: B256,
    ) -> Result<Option<EvmCompilerSymbol<'_>>, libloading::Error> {
        match self.cache.entry(hash) {
            Entry::Occupied(entry) => Ok(entry.get().map(EvmCompilerSymbol::new)),
            Entry::Vacant(entry) => Self::get_function_slow(&self.lib, entry),
        }
    }

    #[cold]
    fn get_function_slow<'a>(
        lib: &'a Library,
        entry: std::collections::hash_map::VacantEntry<'_, B256, Option<EvmCompilerFn>>,
    ) -> Result<Option<EvmCompilerSymbol<'a>>, libloading::Error> {
        let mut symbol_buffer = crate::SymbolBuffer::symbol(entry.key());
        let symbol_bytes = symbol_buffer.make_cstr().to_bytes_with_nul();
        // SAFETY: The symbol is known to have the `EvmCompilerFn` type.
        match unsafe { lib.get::<EvmCompilerFn>(symbol_bytes) } {
            Ok(symbol) => {
                #[allow(clippy::missing_transmute_annotations)]
                let ptr = unsafe { std::mem::transmute(symbol.into_raw().into_raw()) };
                entry.insert(ptr);
                Ok(ptr.map(EvmCompilerSymbol::new))
            }
            Err(e) => {
                entry.insert(None);
                // TODO: I'm assuming that Windows returns NotFound for undefined symbols.
                #[cfg(windows)]
                {
                    use std::error::Error;
                    if let Some(e) = e.source() {
                        if let Some(e) = e.downcast_ref::<std::io::Error>() {
                            if e.kind() == std::io::ErrorKind::NotFound {
                                return Ok(None);
                            }
                        }
                    }
                }
                #[cfg(unix)]
                if e.to_string().contains("undefined symbol") {
                    return Ok(None);
                }
                Err(e)
            }
        }
    }

    /// Unload the library.
    pub fn close(self) -> Result<(), libloading::Error> {
        self.lib.close()
    }
}

/// A symbol from an EVM compiler-generated DLL.
#[allow(missing_debug_implementations)]
pub struct EvmCompilerSymbol<'a> {
    f: EvmCompilerFn,
    _phantom_data: PhantomData<&'a EvmCompilerDll>,
}

impl EvmCompilerSymbol<'_> {
    #[inline]
    fn new(f: EvmCompilerFn) -> Self {
        Self { f, _phantom_data: PhantomData }
    }
}

impl std::ops::Deref for EvmCompilerSymbol<'_> {
    type Target = EvmCompilerFn;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.f
    }
}
