mod receipt_dictionary;
mod transaction_dictionary;

pub use receipt_dictionary::RECEIPT_DICTIONARY;
pub use transaction_dictionary::TRANSACTION_DICTIONARY;

use zstd::bulk::{Compressor, Decompressor};

use std::{cell::RefCell, thread_local};

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
