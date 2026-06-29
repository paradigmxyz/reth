//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use alloy_consensus::{
    proofs::{calculate_ommers_root, calculate_transaction_root, calculate_withdrawals_root},
    Block, BlockBody, Header, TxReceipt,
};
use alloy_primitives::{Bloom, U256};
use alloy_rpc_types_engine::PayloadAttributes as EthPayloadAttributes;
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder,
    PayloadConfig,
};
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthExecutorSpec, EthereumHardforks};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::hashed_post_state_sorted_from_execution_state;
use reth_payload_builder::{BlobSidecars, EthBuiltPayload};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadAttributes as _;
use reth_primitives_traits::RecoveredBlock;
use reth_storage_api::{BorrowedEvmStateProviderDatabase, StateProviderFactory};
use reth_transaction_pool::{
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
    ValidPoolTransaction,
};
use reth_trie_common::{HashedPostState, KeccakKeyHasher};
use std::sync::Arc;
use tracing::{debug, trace};

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
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives>,
    Client: StateProviderFactory
        + ChainSpecProvider<
            ChainSpec: EthereumHardforks + EthExecutorSpec + EthChainSpec<Header = Header>,
        > + Clone,
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
    _pool: Pool,
    builder_config: EthereumBuilderConfig,
    args: BuildArguments<EthPayloadAttributes, EthBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Primitives = EthPrimitives>,
    Client: StateProviderFactory
        + ChainSpecProvider<
            ChainSpec: EthereumHardforks + EthExecutorSpec + EthChainSpec<Header = Header>,
        >,
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
    let _ = (execution_cache, trie_handle);

    if client.chain_spec().is_amsterdam_active_at_timestamp(attributes.timestamp()) {
        return Err(PayloadBuilderError::other(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Amsterdam payload building is unsupported by the active pre-Amsterdam execution path",
        )))
    }

    let state_provider = client.state_by_block_hash(parent_header.hash())?;
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
    let block_env_header = Header {
        parent_hash: parent_header.hash(),
        beneficiary: attributes.suggested_fee_recipient,
        timestamp: attributes.timestamp(),
        number: parent_header.number + 1,
        gas_limit,
        base_fee_per_gas: Some(base_fee),
        mix_hash: attributes.prev_randao,
        parent_beacon_block_root: attributes.parent_beacon_block_root(),
        excess_blob_gas,
        withdrawals_root: attributes
            .withdrawals
            .as_ref()
            .map(|withdrawals| calculate_withdrawals_root(withdrawals.as_slice())),
        slot_number: attributes.slot_number(),
        ..Default::default()
    };

    debug!(
        target: "payload_builder",
        id = %payload_id,
        parent_header = ?parent_header.hash(),
        parent_number = parent_header.number,
        "building payload"
    );

    let best_txs = best_txs(BestTransactionsAttributes::new(
        base_fee,
        blob_params.and_then(|params| excess_blob_gas.map(|gas| params.calc_blob_fee(gas) as u64)),
    ));
    let mut transactions = Vec::new();
    let mut senders = Vec::new();
    let mut cumulative_gas_limit = 0u64;

    for pool_tx in best_txs {
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        let tx_gas_limit = pool_tx.gas_limit();
        if cumulative_gas_limit.saturating_add(tx_gas_limit) > gas_limit {
            trace!(
                target: "payload_builder",
                ?payload_id,
                tx_hash = ?pool_tx.hash(),
                tx_gas_limit,
                gas_limit,
                cumulative_gas_limit,
                "skipping transaction because it would exceed the block gas limit"
            );
            continue
        }

        let recovered = pool_tx.to_consensus();
        let (tx, sender) = recovered.into_parts();
        transactions.push(tx);
        senders.push(sender);
        cumulative_gas_limit += tx_gas_limit;
    }

    let body = BlockBody {
        transactions,
        ommers: Vec::new(),
        withdrawals: attributes.withdrawals.clone().map(Into::into),
    };
    let execution_header = Header {
        transactions_root: calculate_transaction_root(&body.transactions),
        ommers_hash: calculate_ommers_root(&body.ommers),
        withdrawals_root: body
            .withdrawals
            .as_ref()
            .map(|withdrawals| calculate_withdrawals_root(withdrawals.as_slice())),
        ..block_env_header
    };
    let execution_block = RecoveredBlock::new_unhashed(
        Block { header: execution_header, body: body.clone() },
        senders.clone(),
    );
    // SAFETY: The borrowed EVM database is consumed by this synchronous block execution call and
    // cannot outlive `state_provider`.
    let db = unsafe { BorrowedEvmStateProviderDatabase::new(state_provider.as_ref()) };
    let cached_db = cached_reads.as_db_mut(db);
    let cached_db_handle = cached_db.clone();
    let output = evm_config
        .executor(cached_db)
        .execute(&execution_block)
        .map_err(PayloadBuilderError::evm)?;
    cached_db_handle.sync(&mut cached_reads);

    let total_fees = U256::ZERO;
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    let hashed_state = output.hashed_state.clone().map_or_else(
        || hashed_post_state_sorted_from_execution_state::<KeccakKeyHasher>(&output.state),
        HashedPostState::into_sorted,
    );
    let state_root = state_provider.state_root_sorted(hashed_state)?;
    let receipts_root =
        reth_ethereum_primitives::calculate_receipt_root_no_memo(&output.result.receipts);
    let logs_bloom =
        output.result.receipts.iter().fold(Bloom::ZERO, |bloom, receipt| bloom | receipt.bloom());

    let header = Header {
        state_root,
        receipts_root,
        logs_bloom,
        gas_used: output.result.gas_used,
        blob_gas_used: blob_params.map(|_| output.result.blob_gas_used),
        ..execution_block.header().clone()
    };
    let block = RecoveredBlock::new_unhashed(Block { header, body }, senders);
    let requests = chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp())
        .then_some(output.result.requests);
    let payload = EthBuiltPayload::new(Arc::new(block), total_fees, requests, None)
        .with_sidecars(BlobSidecars::Empty);

    Ok(BuildOutcome::Better { payload, cached_reads })
}
