//! Trait abstractions used by the payload crate.

use crate::{error::PayloadBuilderError, BuiltPayload, PayloadBuilderAttributes};

use std::future::Future;

use std::sync::Arc;

/// A type that can build a payload.
///
/// This type is a Future that resolves when the job is done (e.g. timed out) or it failed. It's not
/// supposed to return the best payload built when it resolves instead [PayloadJob::best_payload]
/// should be used for that.
///
/// A PayloadJob must always be prepared to return the best payload built so far to make there's a
/// valid payload to deliver to the CL, so it does not miss a slot, even if the payload is empty.
///
/// Note: A PayloadJob need to be cancel safe because it might be dropped after the CL has requested the payload via `engine_getPayloadV1`, See also <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_getpayloadv1>
pub trait PayloadJob: Future<Output = Result<(), PayloadBuilderError>> + Send + Sync {
    /// Returns the best payload that has been built so far.
    ///
    /// Note: this is expected to be an empty block without transaction if nothing has been built
    /// yet.
    fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError>;
}

/// A type that knows how to create new jobs for creating payloads.
pub trait PayloadJobGenerator: Send + Sync {
    /// The type that manages the lifecycle of a payload.
    ///
    /// This type is a Stream that yields better payloads payload.
    type Job: PayloadJob;

    /// Creates the initial payload and a new [PayloadJob] that yields better payloads.
    ///
    /// This is called when the CL requests a new payload job via a fork choice update.
    ///
    /// Note: this is expected to build a new (empty) payload without transactions, so it can be
    /// returned directly. when asked for
    fn new_payload_job(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError>;
}
