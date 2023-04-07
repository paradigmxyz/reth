//! Trait abstractions used by the payload crate.

use std::future::Future;
use crate::BuiltPayload;
use crate::error::PayloadError;

pub trait PayloadBuilderStrategy {}

/// A type that knows how to create a new payload.
pub trait PayloadBuilder: Send + Sync {
    /// The Future type that represents a job of creating a new payload.
    type PayloadBuildJob: Future<Output = Result<BuiltPayload, PayloadError>> + Send + Sync;

}