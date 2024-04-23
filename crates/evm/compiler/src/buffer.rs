use reth_primitives::{hex, B256};
use std::{ffi::CStr, fmt, mem::MaybeUninit};

const MANGLE_PREFIX: &str = "__reth_evm_compiler_";

/// Simple stack-allocated buffer for building symbol names.
pub(crate) struct SymbolBuffer<const N: usize>(u8, [MaybeUninit<u8>; N]);

impl SymbolBuffer<{ MANGLE_PREFIX.len() + 32 * 2 + 1 }> {
    /// Returns the symbol name for the given bytecode hash.
    #[inline]
    pub(crate) fn symbol(hash: &B256) -> Self {
        let mut buffer = Self::new();
        buffer.push_str(MANGLE_PREFIX).unwrap();
        buffer.push_str(hex::Buffer::<32, false>::new().format(&hash.0)).unwrap();
        buffer
    }
}

impl<const N: usize> SymbolBuffer<N> {
    #[inline]
    pub(crate) fn new() -> Self {
        assert!(N > 0 && N < 256);
        Self(0, [MaybeUninit::uninit(); N])
    }

    #[inline]
    pub(crate) fn push_str(&mut self, s: &str) -> Option<()> {
        self.push_slice(s.as_bytes())
    }

    #[allow(dead_code)]
    #[inline]
    fn push(&mut self, byte: u8) -> Option<()> {
        if self.0 > self.1.len() as u8 {
            return None;
        }
        self.1[self.0 as usize].write(byte);
        self.0 += 1;
        Some(())
    }

    #[inline]
    fn push_slice(&mut self, b: &[u8]) -> Option<()> {
        if (self.0 as usize) + b.len() > self.1.len() {
            return None;
        }
        for byte in b {
            self.1[self.0 as usize].write(*byte);
            self.0 += 1;
        }
        Some(())
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.1.as_ptr().cast(), self.0 as usize) }
    }

    #[inline]
    fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.1.as_mut_ptr().cast(), self.0 as usize) }
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        if cfg!(debug_assertions) {
            std::str::from_utf8(self.as_slice()).unwrap()
        } else {
            unsafe { std::str::from_utf8_unchecked(self.as_slice()) }
        }
    }

    #[inline]
    pub(crate) fn as_str_mut(&mut self) -> &mut str {
        if cfg!(debug_assertions) {
            std::str::from_utf8_mut(self.as_slice_mut()).unwrap()
        } else {
            unsafe { std::str::from_utf8_unchecked_mut(self.as_slice_mut()) }
        }
    }

    #[inline]
    pub(crate) fn make_cstr(&mut self) -> &CStr {
        if !matches!(self.as_bytes().last(), Some(0)) {
            self.push(0).unwrap();
        }
        unsafe { self.as_cstr().unwrap_unchecked() }
    }

    #[inline]
    pub(crate) fn as_cstr(&self) -> Option<&CStr> {
        if !matches!(self.as_bytes().last(), Some(0)) {
            return None;
        }
        Some(unsafe { CStr::from_bytes_with_nul_unchecked(self.as_slice()) })
    }
}

impl<const N: usize> std::ops::Deref for SymbolBuffer<N> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<const N: usize> std::ops::DerefMut for SymbolBuffer<N> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_str_mut()
    }
}

impl<const N: usize> fmt::Display for SymbolBuffer<N> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}
