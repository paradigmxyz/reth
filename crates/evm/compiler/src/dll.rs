use reth_primitives::B256;
use revm_jit::{debug_time, EvmCompilerFn};
use std::{
    collections::HashMap,
    ffi::{CStr, CString},
    io,
    path::Path,
    ptr::NonNull,
};

/// A handle to an open EVM compiler-generated DLL.
#[derive(Debug)]
pub struct EvmCompilerDll {
    handle: NonNull<libc::c_void>,
    cache: HashMap<B256, Option<EvmCompilerFn>>,
}

impl EvmCompilerDll {
    /// Open a DLL at the given path.
    pub fn open(path: &Path) -> io::Result<Self> {
        debug_time!("dlopen", || Self::open_inner(path))
    }

    fn open_inner(path: &Path) -> io::Result<Self> {
        let s = path.as_os_str().as_encoded_bytes();
        let cstring = CString::new(s).unwrap();
        let flags = libc::RTLD_LAZY | libc::RTLD_LOCAL;
        let handle = unsafe { libc::dlopen(cstring.as_ptr(), flags) };
        match NonNull::new(handle) {
            Some(handle) => Ok(Self { handle, cache: HashMap::new() }),
            None => {
                let s = unsafe { libc::dlerror() };
                Err(if s.is_null() {
                    io::Error::other("dlopen failed")
                } else {
                    io::Error::other(unsafe { CStr::from_ptr(s) }.to_string_lossy())
                })
            }
        }
    }

    /// Returns the function for the given bytecode hash.
    #[inline]
    pub fn get_function(&mut self, hash: B256) -> Option<EvmCompilerFn> {
        debug_time!("dlsym", || self.get_function_inner(hash))
    }

    #[inline]
    #[allow(clippy::missing_transmute_annotations)]
    fn get_function_inner(&mut self, hash: B256) -> Option<EvmCompilerFn> {
        *self.cache.entry(hash).or_insert_with(
            #[cold]
            || {
                let mut symbol = crate::SymbolBuffer::symbol(&hash);
                let sym = unsafe { libc::dlsym(self.handle.as_ptr(), symbol.make_cstr().as_ptr()) };
                (!sym.is_null()).then(|| EvmCompilerFn::new(unsafe { std::mem::transmute(sym) }))
            },
        )
    }
}

impl Drop for EvmCompilerDll {
    fn drop(&mut self) {
        if unsafe { libc::dlclose(self.handle.as_ptr()) } == -1 {
            error!("failed to close DLL: {}", io::Error::last_os_error());
        }
    }
}
