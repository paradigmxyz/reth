//! Implementation of the `testing` namespace.
//!
//! This exposes `testing_buildBlockV1`, intended for non-production/debug use.

use alloy_consensus::Transaction as ConsensusTransaction;
use alloy_evm::Evm;
use alloy_primitives::{B256, U256};
use alloy_rlp::Decodable;
use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV4;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::ChainSpec;
use reth_ethereum_engine_primitives::{BuiltPayloadConversionError, EthBuiltPayload};
use reth_ethereum_primitives::{Block as EthBlock, EthPrimitives, Receipt, TransactionSigned};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{Block as BlockTrait, Recovered, SealedBlock, SignerRecoverable};
use reth_rpc_api::{TestingApiServer, TestingBuildBlockRequestV1};
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_tasks::TaskExecutor;
use std::{marker::PhantomData, sync::Arc};
use tokio::sync::{oneshot, Semaphore};

// EVM / payload building
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_evm::{execute::BlockBuilder, ConfigureEvm, NextBlockEnvAttributes};
// BlockExecutionError is unused after inlining error handling; keep import minimal.
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};

/// Trait for types that can build a block for `testing_buildBlockV1`.
#[async_trait]
pub trait TestingBlockBuilder: Send + Sync + std::fmt::Debug + 'static {
    /// Request type accepted by this builder.
    type Request: Send + Sync + 'static;
    /// Response type returned by this builder.
    type Response: Send + Sync + 'static;

    /// Build a block according to the testing API request.
    async fn build_block(&self, request: Self::Request) -> RpcResult<Self::Response>;
}

#[async_trait]
impl<T: TestingBlockBuilder + ?Sized> TestingBlockBuilder for Arc<T> {
    type Request = T::Request;
    type Response = T::Response;

    async fn build_block(&self, request: Self::Request) -> RpcResult<Self::Response> {
        (**self).build_block(request).await
    }
}

/// Testing API handler.
#[derive(Debug, Clone)]
pub struct TestingApi<B> {
    builder: B,
    semaphore: Arc<Semaphore>,
    executor: TaskExecutor,
}

impl<B> TestingApi<B> {
    /// Create a new testing API handler.
    pub fn new(builder: B, max_concurrent: usize, executor: TaskExecutor) -> Self {
        let permits = max_concurrent.max(1);
        Self { builder, semaphore: Arc::new(Semaphore::new(permits)), executor }
    }
}

#[async_trait]
impl<B> TestingApiServer for TestingApi<B>
where
    B: TestingBlockBuilder<
            Request = TestingBuildBlockRequestV1,
            Response = ExecutionPayloadEnvelopeV4,
        > + Clone
        + Send
        + Sync,
    B::Request: Send + 'static,
    B::Response: Send + 'static,
{
    /// Handles `testing_buildBlockV1` by gating concurrency via a semaphore and offloading heavy
    /// work to the blocking pool to avoid stalling the async runtime.
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> RpcResult<ExecutionPayloadEnvelopeV4> {
        let permit = self
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| internal_rpc_err("testing_buildBlockV1 concurrency limiter closed"))?;

        let builder = self.builder.clone();
        let executor = self.executor.clone();
        let (tx, rx) = oneshot::channel();

        let join_handle = executor.spawn_blocking(Box::pin(async move {
            let res = builder.build_block(request).await;
            let _ = tx.send(res);
        }));

        let res = rx.await.map_err(|_| internal_rpc_err("testing_buildBlockV1 worker canceled"))?;
        join_handle
            .await
            .map_err(|err| internal_rpc_err(format!("testing_buildBlockV1 failed: {err}")))?;

        drop(permit);
        res
    }
}

/// Ethereum-specific testing block builder that performs basic validation/decoding.
///
/// NOTE: This currently rejects blob transactions because sidecars are not provided via the
/// testing API and txpool access is disabled per spec.
#[derive(Clone, Debug)]
pub struct EthTestingBlockBuilder<Provider> {
    /// Underlying provider.
    provider: Provider,
    /// EVM configuration.
    evm_config: EthEvmConfig<ChainSpec>,
    /// Builder configuration.
    builder_config: EthereumBuilderConfig,
    /// Marker to keep type parameters.
    _marker: PhantomData<()>,
}

impl<Provider> EthTestingBlockBuilder<Provider> {
    /// Create a new builder instance.
    pub const fn new(
        provider: Provider,
        evm_config: EthEvmConfig<ChainSpec>,
        builder_config: EthereumBuilderConfig,
    ) -> Self {
        Self { provider, evm_config, builder_config, _marker: PhantomData }
    }

    /// Validate `extra_data` length per spec.
    fn validate_extra_data(extra: &Option<alloy_primitives::Bytes>) -> RpcResult<()> {
        if let Some(data) = extra.as_ref() &&
            data.len() > 32
        {
            return Err(invalid_params_rpc_err("extraData must be at most 32 bytes"));
        }
        Ok(())
    }

    /// Decode raw signed transactions, reject blob transactions (no sidecars in testing API),
    /// and recover signer.
    fn decode_transactions(
        transactions: Vec<alloy_primitives::Bytes>,
    ) -> RpcResult<Vec<Recovered<TransactionSigned>>> {
        let mut recovered_txs = Vec::with_capacity(transactions.len());
        for raw in transactions {
            let mut bytes: &[u8] = raw.as_ref();
            let tx = TransactionSigned::decode(&mut bytes).map_err(|_| {
                invalid_params_rpc_err("failed to decode transaction (invalid RLP or signature)")
            })?;

            if tx.blob_versioned_hashes().is_some_and(|hashes: &[B256]| !hashes.is_empty()) {
                // Without sidecars we cannot include blob transactions safely.
                return Err(invalid_params_rpc_err(
                    "blob transactions are not supported in testing_buildBlockV1 without sidecars",
                ));
            }

            let signer = tx
                .recover_signer()
                .map_err(|_| invalid_params_rpc_err("failed to recover transaction signer"))?;

            recovered_txs.push(Recovered::new_unchecked(tx, signer));
        }
        Ok(recovered_txs)
    }
}

#[async_trait]
impl<Provider> TestingBlockBuilder for EthTestingBlockBuilder<Provider>
where
    Provider: BlockReaderIdExt<
            Block = EthBlock,
            Header = <EthBlock as BlockTrait>::Header,
            Transaction = TransactionSigned,
            Receipt = Receipt,
        > + StateProviderFactory
        + ChainSpecProvider<ChainSpec = ChainSpec>
        + Send
        + Sync
        + 'static,
    EthEvmConfig<ChainSpec>:
        ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
{
    type Request = TestingBuildBlockRequestV1;
    type Response = ExecutionPayloadEnvelopeV4;

    /// Core build logic used by the testing RPC; wrapped by the RPC layer in a semaphore +
    /// blocking task to avoid exhausting the async runtime.
    async fn build_block(&self, request: Self::Request) -> RpcResult<Self::Response> {
        Self::validate_extra_data(&request.extra_data)?;
        let recovered_txs = Self::decode_transactions(request.transactions)?;

        // Build the block using the provided parent and attributes.
        let provider = &self.provider;
        let parent = provider
            .sealed_header_by_hash(request.parent_block_hash)
            .map_err(|err| internal_rpc_err(err.to_string()))?
            .ok_or_else(|| invalid_params_rpc_err("parentBlockHash not found"))?;

        // Prepare state.
        let state_provider = provider
            .state_by_block_hash(request.parent_block_hash)
            .map_err(|err| internal_rpc_err(err.to_string()))?;
        let state_db = StateProviderDatabase::new(&state_provider);
        let mut db = State::builder().with_database(state_db).with_bundle_update().build();

        // Prepare env attributes.
        let attrs = request.payload_attributes;
        let timestamp: u64 = attrs.timestamp;
        let gas_limit = self.builder_config.gas_limit(parent.gas_limit);
        let env_attrs = NextBlockEnvAttributes {
            timestamp,
            suggested_fee_recipient: attrs.suggested_fee_recipient,
            prev_randao: attrs.prev_randao,
            gas_limit,
            parent_beacon_block_root: attrs.parent_beacon_block_root,
            withdrawals: attrs.withdrawals.map(Into::into),
        };

        // Per spec: if extraData is provided, override header extra_data with this value.
        let mut evm_config = self.evm_config.clone();
        if let Some(extra) = request.extra_data {
            evm_config = evm_config.with_extra_data(extra);
        }

        // Build block.
        let mut builder = evm_config
            .builder_for_next_block(&mut db, &parent, env_attrs)
            .map_err(|err: _| internal_rpc_err(err.to_string()))?;

        builder
            .apply_pre_execution_changes()
            .map_err(|err: _| internal_rpc_err(err.to_string()))?;

        let mut total_fees = U256::ZERO;
        let base_fee = builder.evm_mut().block().basefee;

        // Per spec: include all provided transactions, in the given order; do not source from
        // txpool.
        for tx in recovered_txs {
            let gas_used: u64 = builder
                .execute_transaction(tx.clone())
                .map_err(|err: _| internal_rpc_err(err.to_string()))?;
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .ok_or_else(|| invalid_params_rpc_err("invalid tip for transaction"))?;
            total_fees += U256::from(miner_fee) * U256::from(gas_used);
        }

        let outcome =
            builder.finish(&state_provider).map_err(|err: _| internal_rpc_err(err.to_string()))?;

        let requests = provider
            .chain_spec()
            .is_prague_active_at_timestamp(timestamp)
            .then_some(outcome.execution_result.requests);

        let sealed_block: Arc<SealedBlock<EthBlock>> =
            Arc::new(outcome.block.sealed_block().clone());

        let built = EthBuiltPayload::new(
            alloy_rpc_types_engine::PayloadId::default(),
            sealed_block,
            total_fees,
            requests,
        );

        let envelope: ExecutionPayloadEnvelopeV4 =
            <EthBuiltPayload as TryInto<ExecutionPayloadEnvelopeV4>>::try_into(built)
                .map_err(|err: BuiltPayloadConversionError| internal_rpc_err(err.to_string()))?;
        Ok(envelope)
    }
}
