//! This crate defines abstractions to create and update payloads (blocks):
//! - [`PayloadJobGenerator`]: a type that knows how to create new jobs for creating payloads based
//!   on [`PayloadAttributes`](alloy_rpc_types::engine::PayloadAttributes).
//! - [`PayloadJob`]: a type that yields (better) payloads over time.
//!
//! This crate comes with the generic [`PayloadBuilderService`] responsible for managing payload
//! jobs.
//!
//! ## Node integration
//!
//! In a standard node the [`PayloadBuilderService`] sits downstream of the engine API, or rather
//! the component that handles requests from the consensus layer like `engine_forkchoiceUpdatedV1`.
//!
//! Payload building is enabled if the forkchoice update request contains payload attributes.
//!
//! See also [the engine API docs](https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#engine_forkchoiceupdatedv2)
//! If the forkchoice update request is `VALID` and contains payload attributes the
//! [`PayloadBuilderService`] will create a new [`PayloadJob`] via the given [`PayloadJobGenerator`]
//! and start polling it until the payload is requested by the CL and the payload job is resolved
//! (see [`PayloadJob::resolve`]).
//!
//! ## Example
//!
//! A simple example of a [`PayloadJobGenerator`] that creates empty blocks:
//!
//! ```
//! use std::future::Future;
//! use std::pin::Pin;
//! use std::sync::Arc;
//! use std::task::{Context, Poll};
//! use alloy_primitives::U256;
//! use reth_payload_builder::{EthBuiltPayload, PayloadBuilderError, KeepPayloadJobAlive, EthPayloadBuilderAttributes, PayloadJob, PayloadJobGenerator, PayloadKind};
//! use reth_primitives::{Block, Header};
//!
//! /// The generator type that creates new jobs that builds empty blocks.
//! pub struct EmptyBlockPayloadJobGenerator;
//!
//! impl PayloadJobGenerator for EmptyBlockPayloadJobGenerator {
//!     type Job = EmptyBlockPayloadJob;
//!
//! /// This is invoked when the node receives payload attributes from the beacon node via `engine_forkchoiceUpdatedV1`
//! fn new_payload_job(&self, attr: EthPayloadBuilderAttributes) -> Result<Self::Job, PayloadBuilderError> {
//!         Ok(EmptyBlockPayloadJob{ attributes: attr,})
//!     }
//!
//! }
//!
//! /// A [PayloadJob] that builds empty blocks.
//! pub struct EmptyBlockPayloadJob {
//!   attributes: EthPayloadBuilderAttributes,
//! }
//!
//! impl PayloadJob for EmptyBlockPayloadJob {
//!    type PayloadAttributes = EthPayloadBuilderAttributes;
//!    type ResolvePayloadFuture = futures_util::future::Ready<Result<EthBuiltPayload, PayloadBuilderError>>;
//!    type BuiltPayload = EthBuiltPayload;
//!
//! fn best_payload(&self) -> Result<EthBuiltPayload, PayloadBuilderError> {
//!     // NOTE: some fields are omitted here for brevity
//!     let block = Block {
//!         header: Header {
//!             parent_hash: self.attributes.parent,
//!             timestamp: self.attributes.timestamp,
//!             beneficiary: self.attributes.suggested_fee_recipient,
//!             ..Default::default()
//!         },
//!         ..Default::default()
//!     };
//!     let payload = EthBuiltPayload::new(self.attributes.id, Arc::new(block.seal_slow()), U256::ZERO, None, None);
//!     Ok(payload)
//! }
//!
//! fn payload_attributes(&self) -> Result<EthPayloadBuilderAttributes, PayloadBuilderError> {
//!     Ok(self.attributes.clone())
//! }
//!
//! fn resolve_kind(&mut self, _kind: PayloadKind) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
//!        let payload = self.best_payload();
//!        (futures_util::future::ready(payload), KeepPayloadJobAlive::No)
//!     }
//! }
//!
//! /// A [PayloadJob] is a future that's being polled by the `PayloadBuilderService`
//! impl Future for EmptyBlockPayloadJob {
//!  type Output = Result<(), PayloadBuilderError>;
//!
//! fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
//!         Poll::Pending
//!     }
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `test-utils`: Export utilities for testing

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod metrics;
mod service;
mod traits;

pub mod noop;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

pub use alloy_rpc_types::engine::PayloadId;
pub use reth_payload_primitives::{PayloadBuilderError, PayloadKind};
pub use service::{
    PayloadBuilderHandle, PayloadBuilderService, PayloadServiceCommand, PayloadStore,
};
pub use traits::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};

// re-export the Ethereum engine primitives for convenience
#[doc(inline)]
pub use reth_ethereum_engine_primitives::{EthBuiltPayload, EthPayloadBuilderAttributes};
