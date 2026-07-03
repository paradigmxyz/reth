//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use alloy_consensus::Transaction as _;
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
    database::StateProviderDatabase,
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutionError, BlockValidationError},
    ConfigureEvm, EvmEnv, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_cache::{CachedStateMetrics, CachedStateMetricsSource, CachedStateProvider};
use reth_payload_builder::{BlobSidecars, EthBuiltPayload};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadAttributes as _;
use reth_primitives_traits::transaction::error::InvalidTransactionError;
use reth_storage_api::StateProviderFactory;
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
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks> + Clone,
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
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments {
        mut cached_reads,
        execution_cache,
        trie_handle,
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
    let blob_params = chain_spec.blob_params_at_timestamp(attributes.timestamp());

    debug!(
        target: "payload_builder",
        id = %payload_id,
        parent_header = ?parent_header.hash(),
        parent_number = parent_header.number,
        "building payload"
    );

    let db = StateProviderDatabase::new(state_provider.as_ref());
    let cached_db = cached_reads.as_db_mut(db);
    let cached_db_handle = cached_db.clone();
    let evm_config = evm_config.with_jit_support();
    let stream_state_updates = trie_handle.is_some() && !skip_state_root;
    let next_block_env_attributes = NextBlockEnvAttributes {
        timestamp: attributes.timestamp(),
        suggested_fee_recipient: attributes.suggested_fee_recipient,
        prev_randao: attributes.prev_randao,
        gas_limit,
        parent_beacon_block_root: attributes.parent_beacon_block_root(),
        withdrawals: attributes.withdrawals.clone().map(Into::into),
        extra_data: builder_config.extra_data.clone(),
        slot_number: attributes.slot_number(),
    };
    let mut builder = evm_config
        .builder_for_next_block(cached_db, &parent_header, next_block_env_attributes)
        .map_err(PayloadBuilderError::other)?;
    let base_fee = builder.evm_env().block_base_fee();
    let blob_fee = blob_params.as_ref().map(|_| builder.evm_env().block_blob_base_fee());

    let use_sparse_trie =
        if let Some(handle) = trie_handle.as_ref().filter(|_| stream_state_updates) {
            let sender = handle.state_hook_sender();
            builder.set_state_hook(move |hashed_state| sender.send_hashed_state(hashed_state))
        } else {
            false
        };

    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

    let mut best_txs = best_txs(BestTransactionsAttributes::new(base_fee, blob_fee));
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

    let BlockBuilderOutcome { execution_result, block, block_access_list, .. } = if skip_state_root
    {
        debug!(
            target: "payload_builder",
            id = %payload_id,
            state_root = ?parent_header.state_root,
            "skipping payload state-root computation"
        );
        builder.finish(
            state_provider.as_ref(),
            Some((parent_header.state_root, TrieUpdates::default())),
        )?
    } else if use_sparse_trie {
        let mut handle = trie_handle.expect("sparse trie handle exists if hook was installed");
        builder.finish_with_state_root(state_provider.as_ref(), |_| match handle.state_root() {
            Ok(outcome) => {
                debug!(
                    target: "payload_builder",
                    id = %payload_id,
                    state_root = ?outcome.state_root,
                    "received state root from sparse trie"
                );
                Ok(Some((outcome.state_root, Arc::unwrap_or_clone(outcome.trie_updates))))
            }
            Err(err) => {
                warn!(target: "payload_builder", id = %payload_id, %err, "sparse trie failed, falling back to sync state root");
                Ok(None)
            }
        })?
    } else {
        builder.finish(state_provider.as_ref(), None)?
    };
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
