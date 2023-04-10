#![warn(missing_docs)]
#![deny(
    unused_must_use,
    rust_2018_idioms,
    rustdoc::broken_intra_doc_links,
    unused_crate_dependencies
)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! This trait implements the [PayloadBuilderService] responsible for managing payload jobs.
//!
//! It Defines the abstractions to create and update payloads:
//!   - [PayloadJobGenerator]: a type that knows how to create new jobs for creating payloads based
//!     on [PayloadAttributes].
//!   - [PayloadJob]: a type that can yields (better) payloads over time.

pub mod error;
mod payload;
mod service;
mod traits;
pub use payload::{BuiltPayload, PayloadBuilderAttributes};
pub use reth_rpc_types::engine::PayloadId;
pub use service::{PayloadBuilderHandle, PayloadBuilderService, PayloadStore as PayloadStore2};
pub use traits::{PayloadJob, PayloadJobGenerator};

use crate::error::PayloadBuilderError;
use parking_lot::Mutex;
use reth_primitives::{H256, U256};
use reth_rpc_types::engine::{ExecutionPayloadEnvelope, PayloadAttributes};
use std::{collections::HashMap, sync::Arc};

/// A type that has access to all locally built payloads and can create new ones.
/// This type is intended to by used by the engine API.
pub trait PayloadStore: Send + Sync {
    /// Returns true if the payload store contains the given payload.
    fn contains(&self, payload_id: PayloadId) -> bool;

    /// Returns the current [ExecutionPayloadEnvelope] associated with the [PayloadId].
    ///
    /// Returns `None` if the payload is not yet built, See [PayloadStore::new_payload].
    fn get_execution_payload(&self, payload_id: PayloadId) -> Option<ExecutionPayloadEnvelope>;

    /// Builds and stores a new payload using the given attributes.
    ///
    /// Returns an error if the payload could not be built.
    // TODO: does this require async?
    fn new_payload(
        &self,
        parent: H256,
        attributes: PayloadAttributes,
    ) -> Result<PayloadId, PayloadBuilderError>;
}

/// A simple in-memory payload store.
#[derive(Debug, Default)]
pub struct TestPayloadStore {
    payloads: Arc<Mutex<HashMap<PayloadId, BuiltPayload>>>,
}

impl PayloadStore for TestPayloadStore {
    fn contains(&self, payload_id: PayloadId) -> bool {
        self.payloads.lock().contains_key(&payload_id)
    }

    fn get_execution_payload(&self, _payload_id: PayloadId) -> Option<ExecutionPayloadEnvelope> {
        // TODO requires conversion
        None
    }

    fn new_payload(
        &self,
        parent: H256,
        attributes: PayloadAttributes,
    ) -> Result<PayloadId, PayloadBuilderError> {
        let attr = PayloadBuilderAttributes::new(parent, attributes);
        let payload_id = attr.payload_id();
        self.payloads
            .lock()
            .insert(payload_id, BuiltPayload::new(payload_id, Default::default(), U256::ZERO));
        Ok(payload_id)
    }
}
