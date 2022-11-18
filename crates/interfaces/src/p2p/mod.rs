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

use futures::Stream;
use std::pin::Pin;

/// The stream of responses from the connected peers, generic over the response type.
pub type MessageStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
