use std::sync::Arc;
use zstd::{
    bulk::{Compressor, Decompressor},
    dict::{DecoderDictionary, EncoderDictionary},
};

const COMPRESSION_LEVEL: i32 = 0;

pub fn get_receipt_compressor() -> Compressor<'static> {
    Compressor::new(COMPRESSION_LEVEL).expect("oops")
}

pub fn get_receipt_decompressor() -> Decompressor<'static> {
    Decompressor::new().expect("oops")
}

pub fn get_transaction_compressor() -> Compressor<'static> {
    Compressor::new(COMPRESSION_LEVEL).expect("oops")
}

pub fn get_transaction_decompressor() -> Decompressor<'static> {
    Decompressor::new().expect("oops")
}
