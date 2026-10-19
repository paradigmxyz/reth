#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

use alloy_consensus::BlockHeader;
use alloy_eips::{eip7685::Requests, BlockNumHash};
use alloy_primitives::{Bytes, U256};
use reth_payload_builder::EthBuiltPayload;
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::SealedBlock;
use reth_pureth_block::{
    ExecutionBlock, ExecutionForkConfig, ExecutionPrimitives, ExecutionRequests, ExecutionSource,
    PayloadSourceError, PayloadSourceInput, PayloadSourceKind,
};
use reth_pureth_receipt::CanonicalityStatus;
use std::sync::Arc;

mod builder;
mod engine;
mod execution;
mod storage;

pub use builder::ExecutionPayloadBuilder;
pub use engine::{ExecutionEngineTypes, ExecutionEngineValidator, ExecutionPayloadTypes};
pub use execution::{received_source, ExecutionConsensus, ExecutionEvmConfig};
pub use storage::ExecutionStorage;

#[derive(Clone, Debug)]
pub struct ExecutionBuiltPayload {
    inner: EthBuiltPayload<ExecutionPrimitives>,
    source: Option<Arc<ExecutionSource>>,
}

impl ExecutionBuiltPayload {
    pub fn try_new(
        inner: EthBuiltPayload<ExecutionPrimitives>,
        source: Option<Arc<ExecutionSource>>,
        fork: &ExecutionForkConfig,
    ) -> Result<Self, PayloadSourceError> {
        let block = inner.recovered_block();
        let active = fork.is_active_at_timestamp(block.timestamp());
        if active != block.header().is_ssz() {
            return Err(PayloadSourceError::SourceMismatch);
        }
        if active && source.is_none() {
            return Err(PayloadSourceError::MissingInput(PayloadSourceInput::FullPayload));
        }
        if let Some(source) = &source {
            let metadata = source.metadata();
            if !Arc::ptr_eq(inner.block_arc(), source.block_arc()) ||
                metadata.native_block != BlockNumHash::new(block.number(), block.hash()) ||
                metadata.source_kind != PayloadSourceKind::Built ||
                metadata.canonicality != CanonicalityStatus::Unknown
            {
                return Err(PayloadSourceError::SourceMismatch);
            }
            let payload = source.payload()?;
            let requests = inner
                .execution_requests()
                .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))?;
            let bal = inner
                .block_access_list()
                .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))?;
            if ExecutionRequests::from_native_groups(requests)
                .map_err(PayloadSourceError::Requests)? !=
                *payload.components().requests.requests() ||
                bal.as_ref() != payload.components().block_access_list.serialized()
            {
                return Err(PayloadSourceError::SourceMismatch);
            }
        }
        Ok(Self { inner, source })
    }

    pub const fn inner(&self) -> &EthBuiltPayload<ExecutionPrimitives> {
        &self.inner
    }

    pub const fn source(&self) -> Option<&Arc<ExecutionSource>> {
        self.source.as_ref()
    }
}

impl BuiltPayload for ExecutionBuiltPayload {
    type Primitives = ExecutionPrimitives;

    fn block(&self) -> &SealedBlock<ExecutionBlock> {
        self.inner.block()
    }

    fn fees(&self) -> U256 {
        self.inner.fees()
    }

    fn block_access_list(&self) -> Option<&Bytes> {
        self.inner.block_access_list()
    }

    fn requests(&self) -> Option<Requests> {
        self.inner.requests()
    }
}
