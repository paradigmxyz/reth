use std::sync::Arc;
use zstd::{
    bulk::{Compressor, Decompressor},
    dict::{DecoderDictionary, EncoderDictionary},
};

mod receipt_dictionary;
mod transaction_dictionary;

const COMPRESSION_LEVEL: i32 = 0;

pub fn get_receipt_compressor() -> Compressor<'static> {
    Compressor::with_dictionary(COMPRESSION_LEVEL, &receipt_dictionary::RECEIPT_DICTIONARY)
        .expect("oops")
}

pub fn get_receipt_decompressor() -> Decompressor<'static> {
    Decompressor::with_dictionary(&receipt_dictionary::RECEIPT_DICTIONARY).expect("oops")
}

pub fn get_transaction_compressor() -> Compressor<'static> {
    Compressor::with_dictionary(COMPRESSION_LEVEL, &transaction_dictionary::TRANSACTION_DICTIONARY)
        .expect("oops")
}

pub fn get_transaction_decompressor() -> Decompressor<'static> {
    Decompressor::with_dictionary(&transaction_dictionary::TRANSACTION_DICTIONARY).expect("oops")
}
