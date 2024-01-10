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

/// Module with a codec for reading and encoding block bodies in files.
///
/// Enables decoding and encoding `Block` types within file contexts.
pub mod file_codec;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
