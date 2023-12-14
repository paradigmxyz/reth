use std::{cell::RefCell, thread_local};
use zstd::bulk::{Compressor, Decompressor};

/// Compression/Decompression dictionary for `Receipt`.
pub static RECEIPT_DICTIONARY: [u8; 100000] = include!("./receipt_dictionary.in");
/// Compression/Decompression dictionary for `Transaction`.
pub static TRANSACTION_DICTIONARY: [u8; 100000] = include!("./transaction_dictionary.in");

// Reason for using static compressors is that dictionaries can be quite big, and zstd-rs
// recommends to use one context/compressor per thread. Thus the usage of `thread_local`.
thread_local! {
    /// Thread Transaction compressor.
    pub static TRANSACTION_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(Compressor::with_dictionary(0, &TRANSACTION_DICTIONARY)
            .expect("Failed to initialize compressor."));

    /// Thread Transaction decompressor.
    pub static TRANSACTION_DECOMPRESSOR: RefCell<Decompressor<'static>> = RefCell::new(Decompressor::with_dictionary(&TRANSACTION_DICTIONARY)
            .expect("Failed to initialize decompressor."));

    /// Thread receipt compressor.
    pub static RECEIPT_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(Compressor::with_dictionary(0, &RECEIPT_DICTIONARY)
            .expect("Failed to initialize compressor."));

    /// Thread receipt decompressor.
    pub static RECEIPT_DECOMPRESSOR: RefCell<Decompressor<'static>> = RefCell::new(Decompressor::with_dictionary(&RECEIPT_DICTIONARY)
            .expect("Failed to initialize decompressor."));
}