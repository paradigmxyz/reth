/// The generic downloader trait for implementing data downloaders over P2P.
pub mod downloader;

/// Traits for implementing P2P block body clients.
pub mod bodies;

/// Traits for implementing P2P Header Clients. Also includes implementations
/// of a Linear and a Parallel downloader generic over the [`Consensus`] and
/// [`HeadersClient`].
///
/// [`Consensus`]: crate::consensus::Consensus
/// [`HeadersClient`]: crate::p2p::headers::HeadersClient
pub mod headers;

/// Error types broadly used by p2p interfaces for any operation which may produce an error when
/// interacting with the network implementation
pub mod error;
