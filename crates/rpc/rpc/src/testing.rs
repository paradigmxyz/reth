//! Implementation of the `testing` namespace.
//!
//! This exposes `testing_buildBlockV1`, intended for non-production/debug use.

use alloy_consensus::{Header, Transaction};
use alloy_evm::Evm;
use alloy_primitives::{map::HashSet, U256};
use alloy_rpc_types_engine::ExecutionPayloadEnvelopeV5;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_errors::RethError;
use reth_ethereum_engine_primitives::EthBuiltPayload;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{execute::BlockBuilder, ConfigureEvm, NextBlockEnvAttributes};
use reth_primitives_traits::{AlloyBlockHeader as BlockTrait, Recovered, TxTy};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_api::{TestingApiServer, TestingBuildBlockRequestV1};
use reth_rpc_eth_api::{helpers::Call, FromEthApiError};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_storage_api::{BlockReader, HeaderProvider, StateProvider};
use revm::context::Block;
use revm_primitives::map::DefaultHashBuilder;
use std::sync::Arc;
use tracing::debug;

/// Testing API handler.
#[derive(Debug, Clone)]
pub struct TestingApi<Eth, Evm> {
    eth_api: Eth,
    evm_config: Evm,
}

impl<Eth, Evm> TestingApi<Eth, Evm> {
    /// Create a new testing API handler.
    pub const fn new(eth_api: Eth, evm_config: Evm) -> Self {
        Self { eth_api, evm_config }
    }
}

impl<Eth, Evm> TestingApi<Eth, Evm>
where
    Eth: Call<Provider: BlockReader<Header = Header>>,
    Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes, Primitives = EthPrimitives>
        + 'static,
{
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> Result<ExecutionPayloadEnvelopeV5, Eth::Error> {
        let evm_config = self.evm_config.clone();
        self.eth_api
            .spawn_with_state_at_block(request.parent_block_hash, move |eth_api, state| {
                let state = state.database.0;
                let mut db = State::builder()
                    .with_bundle_update()
                    .with_database(StateProviderDatabase::new(&state))
                    .build();
                let parent = eth_api
                    .provider()
                    .sealed_header_by_hash(request.parent_block_hash)?
                    .ok_or_else(|| {
                    EthApiError::HeaderNotFound(request.parent_block_hash.into())
                })?;

                let env_attrs = NextBlockEnvAttributes {
                    timestamp: request.payload_attributes.timestamp,
                    suggested_fee_recipient: request.payload_attributes.suggested_fee_recipient,
                    prev_randao: request.payload_attributes.prev_randao,
                    gas_limit: parent.gas_limit(),
                    parent_beacon_block_root: request.payload_attributes.parent_beacon_block_root,
                    withdrawals: request.payload_attributes.withdrawals.map(Into::into),
                    extra_data: request.extra_data.unwrap_or_default(),
                };

                let mut builder = evm_config
                    .builder_for_next_block(&mut db, &parent, env_attrs)
                    .map_err(RethError::other)
                    .map_err(Eth::Error::from_eth_err)?;
                builder.apply_pre_execution_changes().map_err(Eth::Error::from_eth_err)?;

                let mut total_fees = U256::ZERO;
                let base_fee = builder.evm_mut().block().basefee();

                debug!(
                    target: "rpc::testing",
                    parent_hash = ?request.parent_block_hash,
                    parent_number = parent.number(),
                    num_transactions = request.transactions.len(),
                    ?base_fee,
                    "Starting block build"
                );

                for (idx, tx) in request.transactions.iter().enumerate() {
                    let tx: Recovered<TxTy<Evm::Primitives>> = recover_raw_transaction(tx)?;
                    let sender = tx.signer();
                    let tx_nonce = tx.nonce();
                    let tx_hash = *tx.tx_hash();

                    let account_nonce = state
                        .account_nonce(&sender)
                        .map_err(RethError::other)
                        .map_err(Eth::Error::from_eth_err)?;

                    let tip = tx.effective_tip_per_gas(base_fee).unwrap_or_default();
                    let gas_used = builder.execute_transaction(tx).map_err(|e| {
                        debug!(
                            target: "rpc::testing",
                            tx_idx = idx,
                            ?tx_hash,
                            ?sender,
                            tx_nonce,
                            account_nonce = ?account_nonce,
                            error = ?e,
                            "Transaction execution failed"
                        );
                        Eth::Error::from_eth_err(e)
                    })?;

                    total_fees += U256::from(tip) * U256::from(gas_used);
                }
                let outcome = builder.finish(&state).map_err(Eth::Error::from_eth_err)?;

                let requests = outcome
                    .block
                    .requests_hash()
                    .is_some()
                    .then_some(outcome.execution_result.requests);

                EthBuiltPayload::new(
                    alloy_rpc_types_engine::PayloadId::default(),
                    Arc::new(outcome.block.into_sealed_block()),
                    total_fees,
                    requests,
                )
                .try_into_v5()
                .map_err(RethError::other)
                .map_err(Eth::Error::from_eth_err)
            })
            .await
    }

    async fn pack_block(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> Result<ExecutionPayloadEnvelopeV5, Eth::Error> {
        let evm_config = self.evm_config.clone();
        self.eth_api
            .spawn_with_state_at_block(request.parent_block_hash, move |eth_api, state| {
                let state = state.database.0;
                let mut db = State::builder()
                    .with_bundle_update()
                    .with_database(StateProviderDatabase::new(&state))
                    .build();
                let parent = eth_api
                    .provider()
                    .sealed_header_by_hash(request.parent_block_hash)?
                    .ok_or_else(|| {
                    EthApiError::HeaderNotFound(request.parent_block_hash.into())
                })?;

                let env_attrs = NextBlockEnvAttributes {
                    timestamp: request.payload_attributes.timestamp,
                    suggested_fee_recipient: request.payload_attributes.suggested_fee_recipient,
                    prev_randao: request.payload_attributes.prev_randao,
                    gas_limit: parent.gas_limit(),
                    parent_beacon_block_root: request.payload_attributes.parent_beacon_block_root,
                    withdrawals: request.payload_attributes.withdrawals.map(Into::into),
                    extra_data: request.extra_data.unwrap_or_default(),
                };

                let mut builder = evm_config
                    .builder_for_next_block(&mut db, &parent, env_attrs)
                    .map_err(RethError::other)
                    .map_err(Eth::Error::from_eth_err)?;
                builder.apply_pre_execution_changes().map_err(Eth::Error::from_eth_err)?;

                let mut total_fees = U256::ZERO;
                let base_fee = builder.evm_mut().block().basefee();

                // Keeps track of invalid senders - if one tx is invalid then all future txs from
                // that sender are invalid as well
                let mut invalid_senders_map = HashSet::<_, DefaultHashBuilder>::default();

                for (idx, tx) in request.transactions.iter().enumerate() {
                    let tx: Recovered<TxTy<Evm::Primitives>> = recover_raw_transaction(tx)?;
                    let sender = tx.signer();
                    let tx_nonce = tx.nonce();
                    let tx_hash = *tx.tx_hash();

                    if invalid_senders_map.contains(&sender) {
                        // skip any senders which we have ignored previously
                        continue;
                    }

                    let account_nonce = state
                        .account_nonce(&sender)
                        .map_err(RethError::other)
                        .map_err(Eth::Error::from_eth_err)?;

                    let tip = tx.effective_tip_per_gas(base_fee).unwrap_or_default();
                    let gas_used = match builder.execute_transaction(tx) {
                        Err(err) => {
                            invalid_senders_map.insert(sender);
                            debug!(
                                target: "rpc::testing",
                                tx_idx = idx,
                                ?tx_hash,
                                ?sender,
                                tx_nonce,
                                account_nonce = ?account_nonce,
                                error = ?err,
                                "Transaction execution failed"
                            );
                            continue
                        }
                        Ok(gas_used) => gas_used,
                    };

                    total_fees += U256::from(tip) * U256::from(gas_used);
                }
                let outcome = builder.finish(&state).map_err(Eth::Error::from_eth_err)?;

                let requests = outcome
                    .block
                    .requests_hash()
                    .is_some()
                    .then_some(outcome.execution_result.requests);

                let sealed_block = outcome.block.into_sealed_block();

                let envelope = EthBuiltPayload::new(
                    alloy_rpc_types_engine::PayloadId::default(),
                    Arc::new(sealed_block),
                    total_fees,
                    requests,
                )
                .try_into_v5()
                .map_err(RethError::other)
                .map_err(Eth::Error::from_eth_err)?;

                Ok(envelope)
            })
            .await
    }
}

#[async_trait]
impl<Eth, Evm> TestingApiServer for TestingApi<Eth, Evm>
where
    Eth: Call<Provider: BlockReader<Header = Header>>,
    Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes, Primitives = EthPrimitives>
        + 'static,
{
    /// Handles `testing_buildBlockV1` by gating concurrency via a semaphore and offloading heavy
    /// work to the blocking pool to avoid stalling the async runtime.
    async fn build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> RpcResult<ExecutionPayloadEnvelopeV5> {
        self.build_block_v1(request).await.map_err(Into::into)
    }

    /// Handles `testing_packBlock` by gating concurrency via a semaphore and offloading heavy
    /// work to the blocking pool to avoid stalling the async runtime.
    async fn pack_block(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> RpcResult<ExecutionPayloadEnvelopeV5> {
        self.pack_block(request).await.map_err(Into::into)
    }
}
