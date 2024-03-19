use std::{cell::RefCell, thread_local};
use zstd::bulk::{Compressor, Decompressor};

/// Compression/Decompression dictionary for `Receipt`.
pub static RECEIPT_DICTIONARY: &[u8] = include_bytes!("./receipt_dictionary.bin");
/// Compression/Decompression dictionary for `Transaction`.
pub static TRANSACTION_DICTIONARY: &[u8] = include_bytes!("./transaction_dictionary.bin");

// Reason for using static compressors is that dictionaries can be quite big, and zstd-rs
// recommends to use one context/compressor per thread. Thus the usage of `thread_local`.
thread_local! {
    /// Thread Transaction compressor.
    pub static TRANSACTION_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
        Compressor::with_dictionary(0, TRANSACTION_DICTIONARY)
            .expect("Failed to initialize compressor."),
    );

    /// Thread Transaction decompressor.
    pub static TRANSACTION_DECOMPRESSOR: RefCell<ReusableDecompressor> = RefCell::new(
                ReusableDecompressor::new(Decompressor::with_dictionary(TRANSACTION_DICTIONARY).expect("Failed to initialize decompressor."))
   );

    /// Thread receipt compressor.
    pub static RECEIPT_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
        Compressor::with_dictionary(0, RECEIPT_DICTIONARY)
            .expect("Failed to initialize compressor."),
    );

    /// Thread receipt decompressor.
    pub static RECEIPT_DECOMPRESSOR: RefCell<ReusableDecompressor> = RefCell::new(
                        ReusableDecompressor::new(Decompressor::with_dictionary(RECEIPT_DICTIONARY).expect("Failed to initialize decompressor."))
);
}

/// Reusable decompressor that uses its own internal buffer.
#[allow(missing_debug_implementations)]
pub struct ReusableDecompressor {
    /// zstd decompressor
    decompressor: Decompressor<'static>,
    /// buffer to decompress to.
    buf: Vec<u8>,
}

impl ReusableDecompressor {
    fn new(decompressor: Decompressor<'static>) -> Self {
        Self { decompressor, buf: Vec::with_capacity(4096) }
    }

    /// Decompresses `src` reusing the decompressor and its internal buffer.
    pub fn decompress(&mut self, src: &[u8]) -> &[u8] {
        // `decompress_to_buffer` will return an error if the output buffer doesn't have
        // enough capacity. However we don't actually have information on the required
        // length. So we hope for the best, and keep trying again with a fairly bigger size
        // if it fails.
        while let Err(err) = self.decompressor.decompress_to_buffer(src, &mut self.buf) {
            let err = err.to_string();
            if !err.contains("Destination buffer is too small") {
                panic!("Failed to decompress: {err}");
            }
            self.buf.reserve(self.buf.capacity() + 24_000);
        }
        &self.buf
    }
}
