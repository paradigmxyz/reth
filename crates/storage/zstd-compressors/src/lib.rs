//! Commonly used zstd [`Compressor`] and [`Decompressor`] for reth types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use crate::alloc::string::ToString;
use alloc::vec::Vec;
#[cfg(feature = "std")]
use core::sync::atomic::{AtomicBool, Ordering};
use zstd::bulk::{Compressor, Decompressor};
pub use zstd::zstd_safe::CParameter;

/// Global flag to omit dictionary ID from compressed frames.
/// When set to true, saves 4 bytes per compressed transaction/receipt.
/// This is safe because reth uses hardcoded dictionaries.
#[cfg(feature = "std")]
static OMIT_DICTIONARY_ID: AtomicBool = AtomicBool::new(false);

/// Sets the global flag to omit dictionary ID from compressed frames.
///
/// Should be called once at node startup based on `StorageSettings::zstd_omit_dictionary_id`.
/// Once set, all subsequently created compressors will omit the dictionary ID.
#[cfg(feature = "std")]
pub fn set_omit_dictionary_id(omit: bool) {
    OMIT_DICTIONARY_ID.store(omit, Ordering::SeqCst);
}

/// Returns whether the dictionary ID should be omitted from compressed frames.
#[cfg(feature = "std")]
pub fn should_omit_dictionary_id() -> bool {
    OMIT_DICTIONARY_ID.load(Ordering::SeqCst)
}

/// Compression/Decompression dictionary for `Receipt`.
pub static RECEIPT_DICTIONARY: &[u8] = include_bytes!("../receipt_dictionary.bin");
/// Compression/Decompression dictionary for `Transaction`.
pub static TRANSACTION_DICTIONARY: &[u8] = include_bytes!("../transaction_dictionary.bin");

#[cfg(feature = "std")]
pub use locals::*;
#[cfg(feature = "std")]
mod locals {
    use super::*;
    use core::cell::RefCell;

    // We use `thread_local` compressors and decompressors because dictionaries can be quite big,
    // and zstd-rs recommends to use one context/compressor per thread
    std::thread_local! {
        /// Thread Transaction compressor.
        pub static TRANSACTION_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
            create_tx_compressor(should_omit_dictionary_id()),
        );

        /// Thread Transaction decompressor.
        pub static TRANSACTION_DECOMPRESSOR: RefCell<ReusableDecompressor> =
            RefCell::new(ReusableDecompressor::new(
                Decompressor::with_dictionary(TRANSACTION_DICTIONARY)
                    .expect("failed to initialize transaction decompressor"),
            ));

        /// Thread receipt compressor.
        pub static RECEIPT_COMPRESSOR: RefCell<Compressor<'static>> = RefCell::new(
            create_receipt_compressor(should_omit_dictionary_id()),
        );

        /// Thread receipt decompressor.
        pub static RECEIPT_DECOMPRESSOR: RefCell<ReusableDecompressor> =
            RefCell::new(ReusableDecompressor::new(
                Decompressor::with_dictionary(RECEIPT_DICTIONARY)
                    .expect("failed to initialize receipt decompressor"),
            ));
    }
}

/// Creates a tx [`Compressor`].
///
/// If `omit_dictionary_id` is true, the dictionary ID will not be written to the
/// compressed frame header, saving 4 bytes per frame.
pub fn create_tx_compressor(omit_dictionary_id: bool) -> Compressor<'static> {
    let mut compressor = Compressor::with_dictionary(0, TRANSACTION_DICTIONARY)
        .expect("Failed to instantiate tx compressor");
    if omit_dictionary_id {
        compressor.set_parameter(CParameter::DictIdFlag(false)).expect("Failed to set DictIdFlag");
    }
    compressor
}

/// Fn creates tx [`Decompressor`]
pub fn create_tx_decompressor() -> ReusableDecompressor {
    ReusableDecompressor::new(
        Decompressor::with_dictionary(TRANSACTION_DICTIONARY)
            .expect("Failed to instantiate tx decompressor"),
    )
}

/// Creates a receipt [`Compressor`].
///
/// If `omit_dictionary_id` is true, the dictionary ID will not be written to the
/// compressed frame header, saving 4 bytes per frame.
pub fn create_receipt_compressor(omit_dictionary_id: bool) -> Compressor<'static> {
    let mut compressor = Compressor::with_dictionary(0, RECEIPT_DICTIONARY)
        .expect("Failed to instantiate receipt compressor");
    if omit_dictionary_id {
        compressor.set_parameter(CParameter::DictIdFlag(false)).expect("Failed to set DictIdFlag");
    }
    compressor
}

/// Fn creates receipt [`Decompressor`]
pub fn create_receipt_decompressor() -> ReusableDecompressor {
    ReusableDecompressor::new(
        Decompressor::with_dictionary(RECEIPT_DICTIONARY)
            .expect("Failed to instantiate receipt decompressor"),
    )
}

/// Reusable decompressor that uses its own internal buffer.
#[expect(missing_debug_implementations)]
pub struct ReusableDecompressor {
    /// The `zstd` decompressor.
    decompressor: Decompressor<'static>,
    /// The buffer to decompress to.
    buf: Vec<u8>,
}

impl ReusableDecompressor {
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
                    if let Some(upper_bound) = Decompressor::upper_bound(src) &&
                        let Some(additional) = upper_bound.checked_sub(self.buf.capacity())
                    {
                        break 'b additional
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tx_roundtrip_with_dict_id() {
        let data = b"some transaction data to compress and decompress for testing purposes";

        let mut compressor = create_tx_compressor(false);
        let compressed = compressor.compress(data).expect("compression failed");

        let mut decompressor = create_tx_decompressor();
        let decompressed = decompressor.decompress(&compressed);

        assert_eq!(data.as_slice(), decompressed);
    }

    #[test]
    fn test_tx_roundtrip_without_dict_id() {
        let data = b"some transaction data to compress and decompress for testing purposes";

        let mut compressor = create_tx_compressor(true);
        let compressed = compressor.compress(data).expect("compression failed");

        let mut decompressor = create_tx_decompressor();
        let decompressed = decompressor.decompress(&compressed);

        assert_eq!(data.as_slice(), decompressed);
    }

    #[test]
    fn test_omitting_dict_id_saves_bytes() {
        let data = b"some transaction data to compress and decompress for testing purposes";

        let mut compressor_with = create_tx_compressor(false);
        let compressed_with = compressor_with.compress(data).expect("compression failed");

        let mut compressor_without = create_tx_compressor(true);
        let compressed_without = compressor_without.compress(data).expect("compression failed");

        assert!(
            compressed_with.len() > compressed_without.len(),
            "omitting dict ID should save bytes: with={}, without={}",
            compressed_with.len(),
            compressed_without.len()
        );
    }

    #[test]
    fn test_receipt_roundtrip() {
        let data = b"some receipt data to compress and decompress for testing purposes";

        let mut compressor = create_receipt_compressor(false);
        let compressed = compressor.compress(data).expect("compression failed");

        let mut decompressor = create_receipt_decompressor();
        let decompressed = decompressor.decompress(&compressed);

        assert_eq!(data.as_slice(), decompressed);
    }
}
