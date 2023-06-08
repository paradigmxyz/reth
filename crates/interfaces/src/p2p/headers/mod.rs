/// Trait definition for [`HeadersClient`]
///
/// [`HeadersClient`]: client::HeadersClient
pub mod client;

/// A downloader that receives and verifies block headers, is generic
/// over the Consensus and the HeadersClient being used.
///
/// [`Consensus`]: crate::consensus::Consensus
/// [`HeadersClient`]: client::HeadersClient
pub mod downloader;

/// Header downloader error.
pub mod error;
