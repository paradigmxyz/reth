//! Post-quantum payload builder.
//!
//! Provides [`PqPayloadBuilder`] — a [`PayloadBuilder`] that creates blocks
//! containing PQ (ML-DSA-65) signed transactions, mirroring the Ethereum
//! payload builder but without blob handling.

use alloy_consensus::Transaction;
use alloy_primitives::{Bytes, U256};
use reth_basic_payload_builder::{
    is_better_payload, BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder,
    PayloadConfig,
};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_errors::BlockExecutionError;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome},
    ConfigureEvm, Evm, NextBlockEnvAttributes,
};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_pq_node_primitives::PqPrimitives;
use reth_pq_primitives::PqSignedTransaction;
use reth_primitives_traits::SealedBlock;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    error::InvalidPoolTransactionError, BestTransactions, BestTransactionsAttributes,
    PoolTransaction, TransactionPool, ValidPoolTransaction,
};
use revm::context_interface::Block as _;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::{PqBuiltPayload, PqEvmConfig};
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthPayloadBuilderAttributes};

type BestTransactionsIter<Pool> = Box<
    dyn BestTransactions<Item = Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>,
>;

/// Post-quantum payload builder.
///
/// Mirrors [`EthereumPayloadBuilder`] but:
/// - Uses `PqPrimitives` instead of `EthPrimitives`
/// - Uses `PqSignedTransaction` instead of `TransactionSigned`
/// - Produces `PqBuiltPayload` instead of `EthBuiltPayload`
/// - No blob handling (PQ transactions never carry blobs)
///
/// [`EthereumPayloadBuilder`]: reth_ethereum_payload_builder::EthereumPayloadBuilder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PqPayloadBuilder<Pool, Client, EvmConfig = PqEvmConfig> {
    /// Client providing access to node state.
    client: Client,
    /// Transaction pool.
    pool: Pool,
    /// EVM configuration.
    evm_config: EvmConfig,
    /// Desired gas limit for blocks.
    gas_limit: Option<u64>,
}

impl<Pool, Client, EvmConfig> PqPayloadBuilder<Pool, Client, EvmConfig> {
    /// Creates a new PQ payload builder.
    pub const fn new(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        gas_limit: Option<u64>,
    ) -> Self {
        Self { client, pool, evm_config, gas_limit }
    }
}

impl<Pool, Client, EvmConfig> PayloadBuilder for PqPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig: ConfigureEvm<Primitives = PqPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks> + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = PqSignedTransaction>>,
{
    type Attributes = EthPayloadBuilderAttributes;
    type BuiltPayload = PqBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        build_pq_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.gas_limit,
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
        )
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        MissingPayloadBehaviour::RaceEmptyPayload
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(Default::default(), config, Default::default(), None);

        build_pq_payload(
            self.evm_config.clone(),
            self.client.clone(),
            self.pool.clone(),
            self.gas_limit,
            args,
            |attributes| self.pool.best_transactions_with_attributes(attributes),
        )?
        .into_payload()
        .ok_or_else(|| PayloadBuilderError::MissingPayload)
    }
}

/// Constructs a PQ block payload using the best transactions from the pool.
///
/// Simplified version of the Ethereum payload builder:
/// - No blob handling (PQ transactions never carry blobs)
/// - No EIP-4844/EIP-7594 sidecar logic
/// - Otherwise follows the same pattern: state → builder → execute txs → finish
#[inline]
fn build_pq_payload<EvmConfig, Client, Pool, F>(
    evm_config: EvmConfig,
    client: Client,
    _pool: Pool,
    gas_limit: Option<u64>,
    args: BuildArguments<EthPayloadBuilderAttributes, PqBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<PqBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Primitives = PqPrimitives, NextBlockEnvCtx = NextBlockEnvAttributes>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = PqSignedTransaction>>,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments { mut cached_reads, config, cancel, best_payload } = args;
    let PayloadConfig { parent_header, attributes } = config;

    let state_provider = client.state_by_block_hash(parent_header.hash())?;
    let state = StateProviderDatabase::new(state_provider.as_ref());
    let mut db =
        State::builder().with_database(cached_reads.as_db_mut(state)).with_bundle_update().build();

    let effective_gas_limit = gas_limit.unwrap_or(parent_header.gas_limit);

    // Build with empty extra_data; the ML-DSA-65 seal will be injected
    // AFTER the block is finalized (state root, receipts root, etc. are known).
    let block_number = parent_header.number + 1;

    let mut builder = evm_config
        .builder_for_next_block(
            &mut db,
            &parent_header,
            NextBlockEnvAttributes {
                timestamp: attributes.timestamp(),
                suggested_fee_recipient: attributes.suggested_fee_recipient(),
                prev_randao: attributes.prev_randao(),
                gas_limit: effective_gas_limit,
                parent_beacon_block_root: attributes.parent_beacon_block_root(),
                withdrawals: Some(attributes.withdrawals().clone()),
                extra_data: Bytes::default(),
            },
        )
        .map_err(PayloadBuilderError::other)?;

    debug!(
        target: "payload_builder",
        id=%attributes.id,
        parent_header = ?parent_header.hash(),
        parent_number = parent_header.number,
        "building new PQ payload"
    );

    let mut cumulative_gas_used = 0;
    let block_gas_limit: u64 = builder.evm_mut().block().gas_limit();
    let base_fee = builder.evm_mut().block().basefee();

    let mut best_txs = best_txs(BestTransactionsAttributes::new(base_fee, None));
    let mut total_fees = U256::ZERO;

    debug!(
        target: "payload_builder",
        id=%attributes.id,
        %base_fee,
        %block_gas_limit,
        "PQ payload builder starting tx selection"
    );

    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

    while let Some(pool_tx) = best_txs.next() {
        debug!(
            target: "payload_builder",
            tx_hash = ?pool_tx.hash(),
            gas_limit = pool_tx.gas_limit(),
            "PQ payload builder got tx from pool"
        );
        // Gas limit check
        if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
            best_txs.mark_invalid(
                &pool_tx,
                &InvalidPoolTransactionError::ExceedsGasLimit(pool_tx.gas_limit(), block_gas_limit),
            );
            continue;
        }

        // Cancellation check
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled);
        }

        // Convert to consensus transaction
        let tx = pool_tx.to_consensus();

        // Execute the transaction
        let gas_used = match builder.execute_transaction(tx.clone()) {
            Ok(gas_used) => {
                debug!(target: "payload_builder", %gas_used, "PQ tx executed successfully");
                gas_used
            }
            Err(BlockExecutionError::Validation(
                reth_errors::BlockValidationError::InvalidTx { error, .. },
            )) => {
                if error.is_nonce_too_low() {
                    debug!(target: "payload_builder", %error, "skipping nonce too low PQ transaction");
                } else {
                    debug!(target: "payload_builder", %error, "skipping invalid PQ transaction and its descendants");
                    best_txs.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::Consensus(
                            reth_primitives_traits::transaction::error::InvalidTransactionError::TxTypeNotSupported,
                        ),
                    );
                }
                continue;
            }
            // Fatal error
            Err(err) => {
                debug!(target: "payload_builder", %err, "PQ tx execution fatal error");
                return Err(PayloadBuilderError::evm(err));
            }
        };

        // Update fees
        let miner_fee =
            tx.effective_tip_per_gas(base_fee).expect("fee is always valid; execution succeeded");
        total_fees += U256::from(miner_fee) * U256::from(gas_used);
        cumulative_gas_used += gas_used;
    }

    // Check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        drop(builder);
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads });
    }

    let BlockBuilderOutcome { execution_result, block, .. } =
        builder.finish(state_provider.as_ref())?;

    // PoA block sealing: inject ML-DSA-65 signature into extra_data.
    //
    // The seal covers SHAKE-256(RLP(header_with_empty_extra_data)), binding
    // it to every header field (parent hash, state root, transactions root,
    // receipts root, gas used, etc.) while excluding the seal itself.
    let sealed_block = if let Some(sk) = reth_pq_poa::get_signing_key() {
        let sealed = block.sealed_block().clone();
        let mut raw_block = sealed.unseal();

        // The header has empty extra_data (we set it to default above).
        // Compute the seal hash from the full RLP-encoded header.
        let seal_hash = reth_pq_poa::header_seal_hash(&raw_block.header);
        let seal = reth_pq_poa::seal_header(sk, &seal_hash);
        debug!(
            target: "payload_builder",
            block_number,
            seal_len = seal.len(),
            "PoA: sealed block with ML-DSA-65 (full header RLP)"
        );

        // Inject the seal and re-seal the block (recomputes block hash)
        raw_block.header.extra_data = Bytes::from(seal);
        Arc::new(SealedBlock::seal_slow(raw_block))
    } else {
        Arc::new(block.sealed_block().clone())
    };

    let chain_spec = client.chain_spec();
    let requests = chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp)
        .then_some(execution_result.requests);

    debug!(
        target: "payload_builder",
        id=%attributes.id,
        sealed_block_header = ?sealed_block.sealed_header(),
        "sealed built PQ block"
    );

    let payload = PqBuiltPayload::new(EthBuiltPayload::new(
        attributes.id,
        sealed_block,
        total_fees,
        requests,
    ));

    Ok(BuildOutcome::Better { payload, cached_reads })
}
