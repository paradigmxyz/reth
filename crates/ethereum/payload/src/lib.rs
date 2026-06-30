//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use alloy_consensus::{Header, Transaction as _};
use alloy_primitives::U256;
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::PayloadAttributes as EthPayloadAttributes;
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder,
    PayloadConfig,
};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_consensus_common::validation::MAX_RLP_BLOCK_SIZE;
use reth_errors::ConsensusError;
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{
    execute::{
        BlockBuilder, BlockBuilderOutcome, BlockExecutionError, BlockExecutorFactory,
        BlockValidationError, HashedStateMode,
    },
    ConfigureEvm, ExecutionCtxFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_cache::{CachedStateMetrics, CachedStateMetricsSource, CachedStateProvider};
use reth_payload_builder::{BlobSidecars, EthBuiltPayload};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadAttributes as _;
use reth_primitives_traits::transaction::error::InvalidTransactionError;
use reth_storage_api::{BorrowedEvmStateProviderDatabase, StateProviderFactory};
use reth_transaction_pool::{
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError},
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
    ValidPoolTransaction,
};
use reth_trie_common::updates::TrieUpdates;
use std::sync::Arc;
use tracing::{debug, trace, warn};

mod config;
pub use config::*;

pub mod validator;
pub use validator::EthereumExecutionPayloadValidator;

type BestTransactionsIter<Pool> = Box<
    dyn BestTransactions<Item = Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>,
>;

/// Ethereum payload builder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthereumPayloadBuilder<Pool, Client, EvmConfig = EthEvmConfig> {
    /// Client providing access to node state.
    client: Client,
    /// Transaction pool.
    pool: Pool,
    /// The type responsible for creating the evm.
    evm_config: EvmConfig,
    /// Payload builder configuration.
    builder_config: EthereumBuilderConfig,
}

impl<Pool, Client, EvmConfig> EthereumPayloadBuilder<Pool, Client, EvmConfig> {
    /// `EthereumPayloadBuilder` constructor.
    pub const fn new(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        builder_config: EthereumBuilderConfig,
    ) -> Self {
        Self { client, pool, evm_config, builder_config }
    }
}

// Default implementation of [PayloadBuilder] for unit type
impl<Pool, Client, EvmConfig> PayloadBuilder for EthereumPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>
        + 'static,
    for<'a> EvmConfig::BlockExecutorFactory:
        'static + BlockExecutorFactory<ExecutionCtx<'a> = ExecutionCtxFor<'a, EvmConfig>>,
    Client: StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthereumHardforks + EthChainSpec<Header = Header>>
        + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = EthPayloadAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<EthPayloadAttributes, EthBuiltPayload>,
    ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError> {
        default_ethereum_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.builder_config.clone(),
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
        )
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        if self.builder_config.await_payload_on_missing {
            MissingPayloadBehaviour::AwaitInProgress
        } else {
            MissingPayloadBehaviour::RaceEmptyPayload
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<EthBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(
            Default::default(),
            Default::default(),
            None,
            config,
            Default::default(),
            None,
        );

        default_ethereum_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.builder_config.clone(),
            args,
            |_| -> BestTransactionsIter<Pool> { Box::new(std::iter::empty()) },
        )?
        .into_payload()
        .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}

/// Constructs an Ethereum transaction payload using the best transactions from the pool.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
#[inline]
pub fn default_ethereum_payload<EvmConfig, Client, Pool, F>(
    evm_config: EvmConfig,
    client: Client,
    pool: Pool,
    builder_config: EthereumBuilderConfig,
    args: BuildArguments<EthPayloadAttributes, EthBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>
        + 'static,
    for<'a> EvmConfig::BlockExecutorFactory:
        'static + BlockExecutorFactory<ExecutionCtx<'a> = ExecutionCtxFor<'a, EvmConfig>>,
    Client: StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthereumHardforks + EthChainSpec<Header = Header>>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments {
        mut cached_reads,
        execution_cache,
        trie_handle: _trie_handle,
        config,
        cancel,
        best_payload,
    } = args;
    let PayloadConfig { parent_header, attributes, payload_id, .. } = config;
    let skip_state_root = builder_config.skip_state_root;

    if client.chain_spec().is_amsterdam_active_at_timestamp(attributes.timestamp()) {
        return Err(PayloadBuilderError::other(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Amsterdam payload building is unsupported by the active pre-Amsterdam execution path",
        )))
    }

    let mut state_provider = client.state_by_block_hash(parent_header.hash())?;
    if let Some(execution_cache) = execution_cache {
        state_provider = Box::new(CachedStateProvider::new(
            state_provider,
            execution_cache.cache().clone(),
            Some(CachedStateMetrics::zeroed(CachedStateMetricsSource::Builder)),
        ));
    }
    let chain_spec = client.chain_spec();
    let gas_limit = builder_config
        .gas_limit_with_target(parent_header.gas_limit, attributes.target_gas_limit());
    let base_fee = chain_spec
        .next_block_base_fee(parent_header.as_ref(), attributes.timestamp())
        .unwrap_or_default();
    let blob_params = chain_spec.blob_params_at_timestamp(attributes.timestamp());
    let excess_blob_gas = blob_params.map(|params| {
        params.next_block_excess_blob_gas_osaka(
            parent_header.excess_blob_gas.unwrap_or(0),
            parent_header.blob_gas_used.unwrap_or(0),
            parent_header.base_fee_per_gas.unwrap_or(0),
        )
    });

    debug!(
        target: "payload_builder",
        id = %payload_id,
        parent_header = ?parent_header.hash(),
        parent_number = parent_header.number,
        "building payload"
    );

    // SAFETY: The borrowed EVM database is consumed by this synchronous payload build and cannot
    // outlive `state_provider`.
    let db = unsafe { BorrowedEvmStateProviderDatabase::new(state_provider.as_ref()) };
    let cached_db = cached_reads.as_db_mut(db);
    let cached_db_handle = cached_db.clone();
    let evm_config = evm_config.with_jit_support();
    let mut builder = evm_config
        .builder_for_next_block(
            cached_db,
            &parent_header,
            NextBlockEnvAttributes {
                timestamp: attributes.timestamp(),
                suggested_fee_recipient: attributes.suggested_fee_recipient,
                prev_randao: attributes.prev_randao,
                gas_limit,
                parent_beacon_block_root: attributes.parent_beacon_block_root(),
                withdrawals: attributes.withdrawals.clone().map(Into::into),
                extra_data: builder_config.extra_data.clone(),
                slot_number: attributes.slot_number(),
            },
            HashedStateMode::OutputOnly,
        )
        .map_err(PayloadBuilderError::other)?;

    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

    let mut best_txs = best_txs(BestTransactionsAttributes::new(
        base_fee,
        blob_params.and_then(|params| excess_blob_gas.map(|gas| params.calc_blob_fee(gas) as u64)),
    ));
    let mut total_fees = U256::ZERO;
    let mut cumulative_tx_gas_used = 0u64;
    let mut blob_sidecars = BlobSidecars::Empty;
    let mut block_blob_count = 0;
    let mut block_transactions_rlp_length = 0;
    let protocol_max_blob_count =
        blob_params.as_ref().map(|params| params.max_blob_count).unwrap_or_default();
    let max_blob_count = builder_config
        .max_blobs_per_block
        .map(|user_limit| std::cmp::min(user_limit, protocol_max_blob_count).max(1))
        .unwrap_or(protocol_max_blob_count);
    let is_osaka = chain_spec.is_osaka_active_at_timestamp(attributes.timestamp());
    let withdrawals_rlp_length =
        attributes.withdrawals.as_ref().map(|withdrawals| withdrawals.length()).unwrap_or(0);

    while let Some(pool_tx) = best_txs.next() {
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        let block_available_gas = gas_limit.saturating_sub(cumulative_tx_gas_used);
        if pool_tx.gas_limit() > block_available_gas {
            best_txs.mark_invalid(
                &pool_tx,
                InvalidPoolTransactionError::ExceedsGasLimit(
                    pool_tx.gas_limit(),
                    block_available_gas,
                ),
            );
            continue
        }

        let tx = pool_tx.to_consensus();
        let tx_rlp_len = tx.inner().length();
        let estimated_block_size_with_tx =
            block_transactions_rlp_length + tx_rlp_len + withdrawals_rlp_length + 1024;

        if is_osaka && estimated_block_size_with_tx > MAX_RLP_BLOCK_SIZE {
            best_txs.mark_invalid(
                &pool_tx,
                InvalidPoolTransactionError::OversizedData {
                    size: estimated_block_size_with_tx,
                    limit: MAX_RLP_BLOCK_SIZE,
                },
            );
            continue
        }

        let mut blob_tx_sidecar = None;
        let tx_blob_count = tx.blob_count();
        if let Some(tx_blob_count) = tx_blob_count {
            if block_blob_count + tx_blob_count > max_blob_count {
                trace!(
                    target: "payload_builder",
                    tx = ?tx.hash(),
                    ?block_blob_count,
                    "skipping blob transaction because it would exceed the max blob count per block"
                );
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::Eip4844(
                        Eip4844PoolTransactionError::TooManyEip4844Blobs {
                            have: block_blob_count + tx_blob_count,
                            permitted: max_blob_count,
                        },
                    ),
                );
                continue
            }

            let blob_sidecar_result = 'sidecar: {
                let Some(sidecar) =
                    pool.get_blob(*tx.hash()).map_err(PayloadBuilderError::other)?
                else {
                    break 'sidecar Err(Eip4844PoolTransactionError::MissingEip4844BlobSidecar)
                };

                if is_osaka {
                    if sidecar.is_eip7594() {
                        Ok(sidecar)
                    } else {
                        Err(Eip4844PoolTransactionError::UnexpectedEip4844SidecarAfterOsaka)
                    }
                } else if sidecar.is_eip4844() {
                    Ok(sidecar)
                } else {
                    Err(Eip4844PoolTransactionError::UnexpectedEip7594SidecarBeforeOsaka)
                }
            };

            blob_tx_sidecar = match blob_sidecar_result {
                Ok(sidecar) => Some(sidecar),
                Err(error) => {
                    best_txs.mark_invalid(&pool_tx, InvalidPoolTransactionError::Eip4844(error));
                    continue
                }
            };
        }

        let miner_fee = tx.effective_tip_per_gas(base_fee);
        let tx_hash = *tx.tx_hash();
        let gas_output = match builder.execute_transaction(tx) {
            Ok(gas_output) => gas_output,
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                error, ..
            })) => {
                if error.is_nonce_too_low() {
                    trace!(target: "payload_builder", %error, ?tx_hash, "skipping nonce too low transaction");
                } else {
                    trace!(target: "payload_builder", %error, ?tx_hash, "skipping invalid transaction and its descendants");
                    best_txs.mark_invalid(
                        &pool_tx,
                        InvalidPoolTransactionError::Consensus(
                            InvalidTransactionError::TxTypeNotSupported,
                        ),
                    );
                }
                continue
            }
            Err(BlockExecutionError::Validation(
                BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit,
                    block_available_gas,
                },
            )) => {
                trace!(target: "payload_builder", %transaction_gas_limit, %block_available_gas, ?tx_hash, "skipping transaction exceeding block gas limit");
                best_txs.mark_invalid(
                    &pool_tx,
                    InvalidPoolTransactionError::ExceedsGasLimit(
                        transaction_gas_limit,
                        block_available_gas,
                    ),
                );
                continue
            }
            Err(err) => return Err(PayloadBuilderError::evm(err)),
        };

        if let Some(blob_count) = tx_blob_count {
            block_blob_count += blob_count;
            if block_blob_count == max_blob_count {
                best_txs.skip_blobs();
            }
        }

        block_transactions_rlp_length += tx_rlp_len;

        let gas_used = gas_output.tx_gas_used();
        let miner_fee = miner_fee.expect("fee is always valid; execution succeeded");
        total_fees += U256::from(miner_fee) * U256::from(gas_used);
        cumulative_tx_gas_used += gas_used;

        if let Some(sidecar) = blob_tx_sidecar {
            blob_sidecars.push_sidecar_variant(sidecar.as_ref().clone());
        }
    }

    if !is_better_payload(best_payload.as_ref(), total_fees) {
        drop(builder);
        cached_db_handle.sync(&mut cached_reads);
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    let state_root_precomputed = skip_state_root.then(|| {
        debug!(
            target: "payload_builder",
            id = %payload_id,
            state_root = ?parent_header.state_root,
            "skipping payload state-root computation"
        );
        (parent_header.state_root, TrieUpdates::default())
    });
    let BlockBuilderOutcome { execution_result, block, block_access_list, .. } =
        builder.finish(state_provider.as_ref(), state_root_precomputed)?;
    cached_db_handle.sync(&mut cached_reads);

    let requests = chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp())
        .then_some(execution_result.requests);

    debug!(
        target: "payload_builder",
        id = %payload_id,
        sealed_block_header = ?block.sealed_header(),
        "sealed built block"
    );

    if is_osaka && block.rlp_length() > MAX_RLP_BLOCK_SIZE {
        return Err(PayloadBuilderError::other(ConsensusError::BlockTooLarge {
            rlp_length: block.rlp_length(),
            max_rlp_length: MAX_RLP_BLOCK_SIZE,
        }));
    }

    let payload = EthBuiltPayload::new(Arc::new(block), total_fees, requests, block_access_list)
        .with_sidecars(blob_sidecars);

    Ok(BuildOutcome::Better { payload, cached_reads })
}
