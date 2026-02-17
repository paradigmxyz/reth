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

use alloy_consensus::{Header, Transaction};
use alloy_eips::eip2718::Decodable2718;
use alloy_evm::{Evm, RecoveredTx};
use alloy_primitives::{map::HashSet, Address, Bytes, B256, U256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, PayloadAttributes};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_consensus_common::validation::MAX_RLP_BLOCK_SIZE;
use reth_errors::RethError;
use reth_ethereum_engine_primitives::EthBuiltPayload;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{execute::BlockBuilder, ConfigureEvm, NextBlockEnvAttributes};
use reth_primitives_traits::{
    transaction::{recover::try_recover_signers, signed::RecoveryError},
    AlloyBlockHeader as BlockTrait, TxTy,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_api::TestingApiServer;
use reth_rpc_eth_api::{helpers::Call, FromEthApiError};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{BlockReader, HeaderProvider};
use revm::context::Block;
use revm_primitives::map::DefaultHashBuilder;
use std::sync::Arc;
use tracing::debug;

/// Testing API handler.
#[derive(Debug, Clone)]
pub struct TestingApi<Eth, Evm> {
    eth_api: Eth,
    evm_config: Evm,
    /// If true, skip invalid transactions instead of failing.
    skip_invalid_transactions: bool,
}

impl<Eth, Evm> TestingApi<Eth, Evm> {
    /// Create a new testing API handler.
    pub const fn new(eth_api: Eth, evm_config: Evm) -> Self {
        Self { eth_api, evm_config, skip_invalid_transactions: false }
    }

    /// Enable skipping invalid transactions instead of failing.
    /// When a transaction fails, all subsequent transactions from the same sender are also
    /// skipped.
    pub const fn with_skip_invalid_transactions(mut self) -> Self {
        self.skip_invalid_transactions = true;
        self
    }
}

impl<Eth, Evm> TestingApi<Eth, Evm>
where
    Eth: Call<
        Provider: BlockReader<Header = Header> + ChainSpecProvider<ChainSpec: EthereumHardforks>,
    >,
    Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes, Primitives = EthPrimitives>
        + 'static,
{
    async fn build_block_v1(
        &self,
        parent_block_hash: B256,
        payload_attributes: PayloadAttributes,
        transactions: Option<Vec<Bytes>>,
        extra_data: Option<Bytes>,
    ) -> Result<ExecutionPayloadEnvelopeV5, Eth::Error> {
        let evm_config = self.evm_config.clone();
        let skip_invalid_transactions = self.skip_invalid_transactions;
        self.eth_api
            .spawn_with_state_at_block(parent_block_hash, move |eth_api, state| {
                let state = state.database.0;
                let mut db = State::builder()
                    .with_bundle_update()
                    .with_database(StateProviderDatabase::new(&state))
                    .build();
                let parent = eth_api
                    .provider()
                    .sealed_header_by_hash(parent_block_hash)?
                    .ok_or_else(|| {
                    EthApiError::HeaderNotFound(parent_block_hash.into())
                })?;

                let chain_spec = eth_api.provider().chain_spec();
                let is_osaka =
                    chain_spec.is_osaka_active_at_timestamp(payload_attributes.timestamp);

                let withdrawals = payload_attributes.withdrawals.clone();
                let withdrawals_rlp_length = withdrawals.as_ref().map(|w| w.length()).unwrap_or(0);

                let env_attrs = NextBlockEnvAttributes {
                    timestamp: payload_attributes.timestamp,
                    suggested_fee_recipient: payload_attributes.suggested_fee_recipient,
                    prev_randao: payload_attributes.prev_randao,
                    gas_limit: parent.gas_limit(),
                    parent_beacon_block_root: payload_attributes.parent_beacon_block_root,
                    withdrawals: withdrawals.map(Into::into),
                    extra_data: extra_data.unwrap_or_default(),
                };

                let mut builder = evm_config
                    .builder_for_next_block(&mut db, &parent, env_attrs)
                    .map_err(RethError::other)
                    .map_err(Eth::Error::from_eth_err)?;
                builder.apply_pre_execution_changes().map_err(Eth::Error::from_eth_err)?;

                let mut total_fees = U256::ZERO;
                let base_fee = builder.evm_mut().block().basefee();

                let mut invalid_senders: HashSet<Address, DefaultHashBuilder> = HashSet::default();
                let mut block_transactions_rlp_length = 0usize;

                // When transactions is None (JSON null), the spec says to build from mempool.
                // Mempool-based block building is not yet supported via this endpoint.
                let transactions = transactions.ok_or_else(|| {
                    Eth::Error::from_eth_err(EthApiError::InvalidParams(
                        "transactions=null (mempool build) is not supported yet".to_string(),
                    ))
                })?;

                // Decode and recover all transactions in parallel
                let recovered_txs = try_recover_signers(&transactions, |tx| {
                    TxTy::<Evm::Primitives>::decode_2718_exact(tx.as_ref())
                        .map_err(RecoveryError::from_source)
                })
                .or(Err(EthApiError::InvalidTransactionSignature))?;

                for (idx, tx) in recovered_txs.into_iter().enumerate() {
                    let signer = tx.signer();
                    if skip_invalid_transactions && invalid_senders.contains(&signer) {
                        continue;
                    }

                    // EIP-7934: Check estimated block size before adding transaction
                    let tx_rlp_len = tx.tx().length();
                    if is_osaka {
                        // 1KB overhead for block header
                        let estimated_block_size = block_transactions_rlp_length +
                            tx_rlp_len +
                            withdrawals_rlp_length +
                            1024;
                        if estimated_block_size > MAX_RLP_BLOCK_SIZE {
                            if skip_invalid_transactions {
                                debug!(
                                    target: "rpc::testing",
                                    tx_idx = idx,
                                    ?signer,
                                    estimated_block_size,
                                    max_size = MAX_RLP_BLOCK_SIZE,
                                    "Skipping transaction: would exceed block size limit"
                                );
                                invalid_senders.insert(signer);
                                continue;
                            }
                            return Err(Eth::Error::from_eth_err(EthApiError::InvalidParams(
                                format!(
                                    "transaction at index {} would exceed max block size: {} > {}",
                                    idx, estimated_block_size, MAX_RLP_BLOCK_SIZE
                                ),
                            )));
                        }
                    }

                    let tip = tx.effective_tip_per_gas(base_fee).unwrap_or_default();
                    let gas_used = match builder.execute_transaction(tx) {
                        Ok(gas_used) => gas_used,
                        Err(err) => {
                            if skip_invalid_transactions {
                                debug!(
                                    target: "rpc::testing",
                                    tx_idx = idx,
                                    ?signer,
                                    error = ?err,
                                    "Skipping invalid transaction"
                                );
                                invalid_senders.insert(signer);
                                continue;
                            }
                            debug!(
                                target: "rpc::testing",
                                tx_idx = idx,
                                ?signer,
                                error = ?err,
                                "Transaction execution failed"
                            );
                            return Err(Eth::Error::from_eth_err(err));
                        }
                    };

                    block_transactions_rlp_length += tx_rlp_len;
                    total_fees += U256::from(tip) * U256::from(gas_used);
                }
                let outcome = builder.finish(&state).map_err(Eth::Error::from_eth_err)?;

                let has_requests = outcome.block.requests_hash().is_some();
                let sealed_block = Arc::new(outcome.block.into_sealed_block());

                let requests = has_requests.then_some(outcome.execution_result.requests);

                EthBuiltPayload::new(
                    alloy_rpc_types_engine::PayloadId::default(),
                    sealed_block,
                    total_fees,
                    requests,
                )
                .try_into_v5()
                .map_err(RethError::other)
                .map_err(Eth::Error::from_eth_err)
            })
            .await
    }
}

#[async_trait]
impl<Eth, Evm> TestingApiServer for TestingApi<Eth, Evm>
where
    Eth: Call<
        Provider: BlockReader<Header = Header> + ChainSpecProvider<ChainSpec: EthereumHardforks>,
    >,
    Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes, Primitives = EthPrimitives>
        + 'static,
{
    /// Handles `testing_buildBlockV1` by gating concurrency via a semaphore and offloading heavy
    /// work to the blocking pool to avoid stalling the async runtime.
    async fn build_block_v1(
        &self,
        parent_block_hash: B256,
        payload_attributes: PayloadAttributes,
        transactions: Option<Vec<Bytes>>,
        extra_data: Option<Bytes>,
    ) -> RpcResult<ExecutionPayloadEnvelopeV5> {
        self.build_block_v1(parent_block_hash, payload_attributes, transactions, extra_data)
            .await
            .map_err(Into::into)
    }
}
