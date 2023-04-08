//! Trait abstractions used by the payload crate.

use crate::{error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes};
use futures_core::TryStream;

use std::sync::Arc;

/// A type that can build a payload.
///
/// This type is a Stream that yields better payloads.
pub trait PayloadJob:
    TryStream<Ok = Arc<BuiltPayload>, Error = PayloadBuilderError> + Send + Sync
{
}

/// A type that knows how to create new jobs for creating payloads.
pub trait PayloadJobGenerator: Send + Sync {
    /// The type that manages the lifecycle of a payload.
    ///
    /// This type is a Stream that yields better payloads payload.
    type Job: PayloadJob;

    /// Creates the initial payload and a new [PayloadJob] that yields better payloads.
    ///
    /// Note: this is expected to build a new (empty) payload without transactions, so it can be
    /// returned directly. when asked for
    fn new_payload_job(&self, attr: PayloadBuilderAttributes) -> (Arc<BuiltPayload>, Self::Job);
}
