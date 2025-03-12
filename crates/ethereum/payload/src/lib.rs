//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

pub mod validator;
pub use validator::EthereumExecutionPayloadValidator;

use alloy_consensus::{Transaction, Typed2718};
use alloy_primitives::U256;
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{ChainSpec, ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_errors::{BlockExecutionError, BlockValidationError};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm, Evm, NextBlockEnvAttributes,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_primitives_traits::SignedTransaction;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactions, BestTransactionsAttributes,
    PoolTransaction, TransactionPool, ValidPoolTransaction,
};
use revm::context_interface::Block as _;
use std::sync::Arc;
use tracing::{debug, trace, warn};

mod config;
pub use config::*;
use reth_primitives_traits::transaction::error::InvalidTransactionError;
use reth_transaction_pool::error::Eip4844PoolTransactionError;

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
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
{
    type Attributes = EthPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<EthPayloadBuilderAttributes, EthBuiltPayload>,
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

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<EthBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(Default::default(), config, Default::default(), None);

        default_ethereum_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.builder_config.clone(),
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
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
    args: BuildArguments<EthPayloadBuilderAttributes, EthBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TransactionSigned>>,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;
    let PayloadConfig { parent_header, attributes } = config;

    let state_provider = client.state_by_block_hash(parent_header.hash())?;
    let state = StateProviderDatabase::new(&state_provider);
    let mut db =
        State::builder().with_database(cached_reads.as_db_mut(state)).with_bundle_update().build();

    let mut builder = evm_config
        .builder_for_next_block(
            &mut db,
            &parent_header,
            NextBlockEnvAttributes {
                timestamp: attributes.timestamp(),
                suggested_fee_recipient: attributes.suggested_fee_recipient(),
                prev_randao: attributes.prev_randao(),
                gas_limit: builder_config.gas_limit(parent_header.gas_limit),
                parent_beacon_block_root: attributes.parent_beacon_block_root(),
                withdrawals: Some(attributes.withdrawals().clone()),
            },
        )
        .map_err(PayloadBuilderError::other)?;

    let chain_spec = client.chain_spec();

    debug!(target: "payload_builder", id=%attributes.id, parent_header = ?parent_header.hash(), parent_number = parent_header.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let block_gas_limit: u64 = builder.evm_mut().block().gas_limit;
    let base_fee = builder.evm_mut().block().basefee;

    let mut best_txs = best_txs(BestTransactionsAttributes::new(
        base_fee,
        builder.evm_mut().block().blob_gasprice().map(|gasprice| gasprice as u64),
    ));
    let mut total_fees = U256::ZERO;

    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

    let mut block_blob_count = 0;
    let blob_params = chain_spec.blob_params_at_timestamp(attributes.timestamp);
    let max_blob_count =
        blob_params.as_ref().map(|params| params.max_blob_count).unwrap_or_default();

    while let Some(pool_tx) = best_txs.next() {
        // ensure we still have capacity for this transaction
        if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
            // we can't fit this transaction into the block, so we need to mark it as invalid
            // which also removes all dependent transaction from the iterator before we can
            // continue
            best_txs.mark_invalid(
                &pool_tx,
                InvalidPoolTransactionError::ExceedsGasLimit(pool_tx.gas_limit(), block_gas_limit),
            );
            continue
        }

        // check if the job was cancelled, if so we can exit early
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        // convert tx to a signed transaction
        let tx = pool_tx.to_consensus();

        // There's only limited amount of blob space available per block, so we need to check if
        // the EIP-4844 can still fit in the block
        if let Some(blob_tx) = tx.as_eip4844() {
            let tx_blob_count = blob_tx.blob_versioned_hashes.len() as u64;

            if block_blob_count + tx_blob_count > max_blob_count {
                // we can't fit this _blob_ transaction into the block, so we mark it as
                // invalid, which removes its dependent transactions from
                // the iterator. This is similar to the gas limit condition
                // for regular transactions above.
                trace!(target: "payload_builder", tx=?tx.hash(), ?block_blob_count, "skipping blob transaction because it would exceed the max blob count per block");
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
        }

        let gas_used = match builder.execute_transaction(tx.clone()) {
            Ok(gas_used) => gas_used,
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                error, ..
            })) => {
                if error.is_nonce_too_low() {
                    // if the nonce is too low, we can skip this transaction
                    trace!(target: "payload_builder", %error, ?tx, "skipping nonce too low transaction");
                } else {
                    // if the transaction is invalid, we can skip it and all of its
                    // descendants
                    trace!(target: "payload_builder", %error, ?tx, "skipping invalid transaction and its descendants");
                    best_txs.mark_invalid(
                        &pool_tx,
                        InvalidPoolTransactionError::Consensus(
                            InvalidTransactionError::TxTypeNotSupported,
                        ),
                    );
                }
                continue
            }
            // this is an error that we should treat as fatal for this attempt
            Err(err) => return Err(PayloadBuilderError::evm(err)),
        };

        // add to the total blob gas used if the transaction successfully executed
        if let Some(blob_tx) = tx.as_eip4844() {
            block_blob_count += blob_tx.blob_versioned_hashes.len() as u64;

            // if we've reached the max blob count, we can skip blob txs entirely
            if block_blob_count == max_blob_count {
                best_txs.skip_blobs();
            }
        }

        // update add to total fees
        let miner_fee =
            tx.effective_tip_per_gas(base_fee).expect("fee is always valid; execution succeeded");
        total_fees += U256::from(miner_fee) * U256::from(gas_used);
        cumulative_gas_used += gas_used;
    }

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // Release db
        drop(builder);
        // can skip building the block
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    let BlockBuilderOutcome { execution_result, block, .. } = builder.finish(&state_provider)?;

    let requests = chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp)
        .then_some(execution_result.requests);

    // initialize empty blob sidecars at first. If cancun is active then this will
    let mut blob_sidecars = Vec::new();

    // only determine cancun fields when active
    if chain_spec.is_cancun_active_at_timestamp(attributes.timestamp) {
        // grab the blob sidecars from the executed txs
        blob_sidecars = pool
            .get_all_blobs_exact(
                block
                    .body()
                    .transactions()
                    .filter(|tx| tx.is_eip4844())
                    .map(|tx| *tx.tx_hash())
                    .collect(),
            )
            .map_err(PayloadBuilderError::other)?;
    }

    let sealed_block = Arc::new(block.sealed_block().clone());
    debug!(target: "payload_builder", id=%attributes.id, sealed_block_header = ?sealed_block.sealed_header(), "sealed built block");

    let mut payload = EthBuiltPayload::new(attributes.id, sealed_block, total_fees, requests);

    // extend the payload with the blob sidecars from the executed txs
    payload.extend_sidecars(blob_sidecars.into_iter().map(Arc::unwrap_or_clone));

    Ok(BuildOutcome::Better { payload, cached_reads })
}
