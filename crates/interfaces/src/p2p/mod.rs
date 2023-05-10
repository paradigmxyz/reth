/// Shared abstractions for downloader implementations.
pub mod download;

/// Traits for implementing P2P block body clients.
pub mod bodies;

/// An implementation that uses headers and bodies traits to download full blocks
pub mod full_block;

/// Traits for implementing P2P Header Clients. Also includes implementations
/// of a Linear and a Parallel downloader generic over the [`Consensus`] and
/// [`HeadersClient`].
///
/// [`Consensus`]: crate::consensus::Consensus
/// [`HeadersClient`]: crate::p2p::headers::client::HeadersClient
pub mod headers;

/// Error types broadly used by p2p interfaces for any operation which may produce an error when
/// interacting with the network implementation
pub mod error;

/// Priority enum for BlockHeader and BlockBody requests
pub mod priority;
