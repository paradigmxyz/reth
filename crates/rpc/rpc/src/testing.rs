//! Implementation of the `testing` namespace.
//!
//! This exposes `testing_buildBlockV1`, intended for non-production/debug use.
//!
//! # Enabling the testing namespace
//!
//! The `testing_` namespace is disabled by default for security reasons.
//! To enable it, add `testing` to the `--http.api` flag when starting the node:
//!
//! ```sh
//! reth node --http --http.api eth,testing
//! ```
//!
//! **Warning:** This namespace allows building arbitrary blocks. Never expose it
//! on public-facing RPC endpoints without proper authentication.

use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, PayloadAttributes};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_engine_primitives::ConsensusEngineHandle;
use reth_payload_primitives::PayloadTypes;
use reth_rpc_api::{TestingApiServer, TestingBuildBlockRequestV1};
use reth_rpc_eth_types::EthApiError;

/// Testing API handler.
#[derive(Debug, Clone)]
pub struct TestingApi<Eth, Evm, Payload: PayloadTypes> {
    #[expect(dead_code)]
    eth_api: Eth,
    #[expect(dead_code)]
    evm_config: Evm,
    /// Desired gas limit to move toward while respecting the consensus gas limit bounds.
    #[expect(dead_code)]
    desired_gas_limit: u64,
    #[expect(dead_code)]
    engine_handle: ConsensusEngineHandle<Payload>,
    /// If true, skip invalid transactions instead of failing.
    skip_invalid_transactions: bool,
    /// If set, override the block gas limit in `testing_buildBlockV1`.
    gas_limit_override: Option<u64>,
}

impl<Eth, Evm, Payload: PayloadTypes> TestingApi<Eth, Evm, Payload> {
    /// Create a new testing API handler.
    pub const fn new(
        eth_api: Eth,
        evm_config: Evm,
        desired_gas_limit: u64,
        engine_handle: ConsensusEngineHandle<Payload>,
    ) -> Self {
        Self {
            eth_api,
            evm_config,
            desired_gas_limit,
            engine_handle,
            skip_invalid_transactions: false,
            gas_limit_override: None,
        }
    }

    /// Enable skipping invalid transactions instead of failing.
    /// When a transaction fails, all subsequent transactions from the same sender are also
    /// skipped.
    pub const fn with_skip_invalid_transactions(mut self) -> Self {
        self.skip_invalid_transactions = true;
        self
    }

    /// Override the gas limit used by `testing_buildBlockV1`.
    pub const fn with_gas_limit_override(mut self, gas_limit: u64) -> Self {
        self.gas_limit_override = Some(gas_limit);
        self
    }
}

fn unsupported_testing<T>() -> RpcResult<T> {
    Err(EthApiError::Unsupported("testing RPC is unsupported by the active EVM execution path")
        .into())
}

#[async_trait]
impl<Eth, Evm, Payload> TestingApiServer for TestingApi<Eth, Evm, Payload>
where
    Eth: Send + Sync + 'static,
    Evm: Send + Sync + 'static,
    Payload: PayloadTypes + Send + Sync + 'static,
{
    /// Handles `testing_buildBlockV1`.
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> RpcResult<ExecutionPayloadEnvelopeV5> {
        let _ = request;
        unsupported_testing()
    }

    /// Handles `testing_commitBlockV1`.
    async fn commit_block_v1(
        &self,
        payload_attributes: PayloadAttributes,
        transactions: Option<Vec<Bytes>>,
        extra_data: Option<Bytes>,
    ) -> RpcResult<B256> {
        let _ = (payload_attributes, transactions, extra_data);
        unsupported_testing()
    }
}
