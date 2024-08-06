#[cfg(feature = "std")]
use std::{cell::RefCell, thread_local};
#[cfg(not(feature = "std"))]
use zstd::bulk::Decompressor;
#[cfg(feature = "std")]
use zstd::bulk::{Compressor, Decompressor};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(not(feature = "std"))]
use alloc::string::ToString;

#[cfg(feature = "std")]
/// Compression/Decompression dictionary for `Receipt`.
pub static RECEIPT_DICTIONARY: &[u8] = include_bytes!("./receipt_dictionary.bin");
#[cfg(feature = "std")]
/// Compression/Decompression dictionary for `Transaction`.
pub static TRANSACTION_DICTIONARY: &[u8] = include_bytes!("./transaction_dictionary.bin");

// We use `thread_local` compressors and decompressors because dictionaries can be quite big, and
// zstd-rs recommends to use one context/compressor per thread
#[cfg(feature = "std")]
thread_local! {
    /// Thread Transaction compressor.
    pub static TRANSACTION_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
        Compressor::with_dictionary(0, TRANSACTION_DICTIONARY)
            .expect("failed to initialize transaction compressor"),
    );

    /// Thread Transaction decompressor.
    pub static TRANSACTION_DECOMPRESSOR: RefCell<ReusableDecompressor> =
        RefCell::new(ReusableDecompressor::new(
            Decompressor::with_dictionary(TRANSACTION_DICTIONARY)
                .expect("failed to initialize transaction decompressor"),
        ));

    /// Thread receipt compressor.
    pub static RECEIPT_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
        Compressor::with_dictionary(0, RECEIPT_DICTIONARY)
            .expect("failed to initialize receipt compressor"),
    );

    /// Thread receipt decompressor.
    pub static RECEIPT_DECOMPRESSOR: RefCell<ReusableDecompressor> =
        RefCell::new(ReusableDecompressor::new(
            Decompressor::with_dictionary(RECEIPT_DICTIONARY)
                .expect("failed to initialize receipt decompressor"),
        ));
}

/// Reusable decompressor that uses its own internal buffer.
#[allow(missing_debug_implementations)]
pub struct ReusableDecompressor {
    /// The `zstd` decompressor.
    decompressor: Decompressor<'static>,
    /// The buffer to decompress to.
    buf: Vec<u8>,
}

impl ReusableDecompressor {
    #[cfg(feature = "std")]
    fn new(decompressor: Decompressor<'static>) -> Self {
        Self { decompressor, buf: Vec::with_capacity(4096) }
    }

    /// Decompresses `src` reusing the decompressor and its internal buffer.
    pub fn decompress(&mut self, src: &[u8]) -> &[u8] {
        // If the decompression fails because the buffer is too small, we try to reserve more space
        // by getting the upper bound and retry the decompression.
        let mut reserved_upper_bound = false;
        while let Err(err) = self.decompressor.decompress_to_buffer(src, &mut self.buf) {
            let err = err.to_string();
            assert!(
                err.contains("Destination buffer is too small"),
                "Failed to decompress {} bytes: {err}",
                src.len()
            );

            let additional = 'b: {
                // Try to get the upper bound of the decompression for the given source.
                // Do this only once as it might be expensive and will be the same for the same
                // source.
                if !reserved_upper_bound {
                    reserved_upper_bound = true;
                    if let Some(upper_bound) = Decompressor::upper_bound(src) {
                        if let Some(additional) = upper_bound.checked_sub(self.buf.capacity()) {
                            break 'b additional;
                        }
                    }
                }

                // Otherwise, double the capacity of the buffer.
                // This should normally not be reached as the upper bound should be enough.
                self.buf.capacity() + 24_000
            };
            self.reserve(additional, src.len());
        }

        // `decompress_to_buffer` sets the length of the vector to the number of bytes written, so
        // we can safely return it as a slice.
        &self.buf
    }

    #[track_caller]
    fn reserve(&mut self, additional: usize, src_len: usize) {
        if let Err(e) = self.buf.try_reserve(additional) {
            panic!(
                "failed to allocate to {existing} + {additional} bytes \
                 for the decompression of {src_len} bytes: {e}",
                existing = self.buf.capacity(),
            );
        }
    }
}
