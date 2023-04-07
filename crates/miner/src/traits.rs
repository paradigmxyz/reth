//! Trait abstractions used by the payload crate.

use crate::{error::PayloadError, BuiltPayload, PayloadBuilderAttributes};
use reth_rpc_types::engine::PayloadId;
use tokio_stream::Stream;

/// Trait used to associate a type with a payload.
pub trait HasPayloadId {
    /// Returns the identifier of the payload
    fn payload_id(&self) -> PayloadId;
}

/// A type that knows how to create new payloads and the updater type.
pub trait PayloadGenerator: Send + Sync {
    /// The type that manages the lifecycle of a payload.
    ///
    /// This type is expected to be a stream that emits better payloads.
    type PayloadUpdates: Stream<Item = Result<BuiltPayload, PayloadError>>
        + HasPayloadId
        + Send
        + Sync;

    /// Creates the initial payload.
    ///
    /// Note: this is expected to build a new (empty) payload without transactions.
    fn new_payload(&self, attr: PayloadBuilderAttributes) -> (BuiltPayload, Self::PayloadUpdates);
}
