//! Implements the downloader algorithms.
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// The collection of algorithms for downloading block bodies.
pub mod bodies;

/// The collection of algorithms for downloading block headers.
pub mod headers;

/// Common downloader metrics.
pub mod metrics;

/// Module managing file-based data retrieval and buffering.
///
/// Contains [FileClient](file_client::FileClient) to read block data from files,
/// efficiently buffering headers and bodies for retrieval.
pub mod file_client;

/// Module managing file-based data retrieval and buffering of receipts.
///
/// Contains [ReceiptFileClient](receipt_file_client::ReceiptFileClient) to read receipt data from
/// files, efficiently buffering receipts for retrieval.
///
/// Currently configured to use codec [`HackReceipt`](file_codec_ovm_receipt::HackReceipt) based on
/// export of below Bedrock data using <https://github.com/testinprod-io/op-geth/pull/1>. Codec can
/// be replaced with regular encoding of receipts for export.
///
/// NOTE: receipts can be exported using regular op-geth encoding for `Receipt` type, to fit
/// reth's needs for importing. However, this would require patching the diff in <https://github.com/testinprod-io/op-geth/pull/1> to export the `Receipt` and not `HackReceipt` type (originally
/// made for op-erigon's import needs).
pub mod receipt_file_client;

/// Module with a codec for reading and encoding block bodies in files.
///
/// Enables decoding and encoding `Block` types within file contexts.
pub mod file_codec;

/// Module with a codec for reading and encoding receipts in files.
///
/// Enables decoding and encoding `HackReceipt` type. See <https://github.com/testinprod-io/op-geth/pull/1>.
pub mod file_codec_ovm_receipt;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
