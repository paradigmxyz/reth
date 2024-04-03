//! Optimism payload builder implementation.

use crate::{
    error::OptimismPayloadBuilderError,
    payload::{OptimismBuiltPayload, OptimismPayloadBuilderAttributes},
};
use reth_basic_payload_builder::*;
use reth_payload_builder::error::PayloadBuilderError;
use reth_primitives::{
    constants::{BEACON_NONCE, EMPTY_RECEIPTS, EMPTY_TRANSACTIONS},
    eip4844::calculate_excess_blob_gas,
    proofs,
    revm::env::tx_env_with_recovered,
    Block, ChainSpec, Hardfork, Header, IntoRecoveredTransaction, Receipt, Receipts, TxType,
    EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{BundleStateWithReceipts, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::{BestTransactionsAttributes, TransactionPool};
use revm::{
    db::states::bundle_state::BundleRetention,
    primitives::{EVMError, EnvWithHandlerCfg, InvalidTransaction, ResultAndState},
    DatabaseCommit, State,
};
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Optimism's payload builder
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismPayloadBuilder {
    /// The rollup's compute pending block configuration option.
    // TODO(clabby): Implement this feature.
    compute_pending_block: bool,
    /// The rollup's chain spec.
    chain_spec: Arc<ChainSpec>,
}

impl OptimismPayloadBuilder {
    /// OptimismPayloadBuilder constructor.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { compute_pending_block: true, chain_spec }
    }

    /// Sets the rollup's compute pending block configuration option.
    pub fn set_compute_pending_block(mut self, compute_pending_block: bool) -> Self {
        self.compute_pending_block = compute_pending_block;
        self
    }

    /// Enables the rollup's compute pending block configuration option.
    pub fn compute_pending_block(self) -> Self {
        self.set_compute_pending_block(true)
    }

    /// Returns the rollup's compute pending block configuration option.
    pub fn is_compute_pending_block(&self) -> bool {
        self.compute_pending_block
    }

    /// Sets the rollup's chainspec.
    pub fn set_chain_spec(mut self, chain_spec: Arc<ChainSpec>) -> Self {
        self.chain_spec = chain_spec;
        self
    }
}

/// Implementation of the [PayloadBuilder] trait for [OptimismPayloadBuilder].
impl<Pool, Client> PayloadBuilder<Pool, Client> for OptimismPayloadBuilder
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    type Attributes = OptimismPayloadBuilderAttributes;
    type BuiltPayload = OptimismBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
    ) -> Result<BuildOutcome<OptimismBuiltPayload>, PayloadBuilderError> {
        optimism_payload_builder(args, self.compute_pending_block)
    }

    fn on_missing_payload(
        &self,
        args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
    ) -> Option<OptimismBuiltPayload> {
        // In Optimism, the PayloadAttributes can specify a `no_tx_pool` option that implies we
        // should not pull transactions from the tx pool. In this case, we build the payload
        // upfront with the list of transactions sent in the attributes without caring about
        // the results of the polling job, if a best payload has not already been built.
        if args.config.attributes.no_tx_pool {
            if let Ok(BuildOutcome::Better { payload, .. }) = self.try_build(args) {
                trace!(target: "payload_builder", "[OPTIMISM] Forced best payload");
                return Some(payload)
            }
        }

        None
    }

    fn build_empty_payload(
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<OptimismBuiltPayload, PayloadBuilderError> {
        let extra_data = config.extra_data();
        let PayloadConfig {
            initialized_block_env,
            parent_block,
            attributes,
            chain_spec,
            initialized_cfg,
            ..
        } = config;

        debug!(target: "payload_builder", parent_hash = ?parent_block.hash(), parent_number = parent_block.number, "building empty payload");

        let state = client.state_by_block_hash(parent_block.hash()).map_err(|err| {
                warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to get state for empty payload");
                err
            })?;
        let mut db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(&state)))
            .with_bundle_update()
            .build();

        let base_fee = initialized_block_env.basefee.to::<u64>();
        let block_number = initialized_block_env.number.to::<u64>();
        let block_gas_limit: u64 = initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX);

        // apply eip-4788 pre block contract call
        pre_block_beacon_root_contract_call(
                &mut db,
                &chain_spec,
                block_number,
                &initialized_cfg,
                &initialized_block_env,
                &attributes,
            ).map_err(|err| {
                warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to apply beacon root contract call for empty payload");
                err
            })?;

        let WithdrawalsOutcome { withdrawals_root, withdrawals } =
                commit_withdrawals(&mut db, &chain_spec, attributes.payload_attributes.timestamp, attributes.payload_attributes.withdrawals.clone()).map_err(|err| {
                    warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to commit withdrawals for empty payload");
                    err
                })?;

        // merge all transitions into bundle state, this would apply the withdrawal balance
        // changes and 4788 contract call
        db.merge_transitions(BundleRetention::PlainState);

        // calculate the state root
        let bundle_state = db.take_bundle();
        let state_root = state.state_root(&bundle_state).map_err(|err| {
                warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to calculate state root for empty payload");
                err
            })?;

        let mut excess_blob_gas = None;
        let mut blob_gas_used = None;

        if chain_spec.is_cancun_active_at_timestamp(attributes.payload_attributes.timestamp) {
            excess_blob_gas = if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) {
                let parent_excess_blob_gas = parent_block.excess_blob_gas.unwrap_or_default();
                let parent_blob_gas_used = parent_block.blob_gas_used.unwrap_or_default();
                Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
            } else {
                // for the first post-fork block, both parent.blob_gas_used and
                // parent.excess_blob_gas are evaluated as 0
                Some(calculate_excess_blob_gas(0, 0))
            };

            blob_gas_used = Some(0);
        }

        let header = Header {
            parent_hash: parent_block.hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: initialized_block_env.coinbase,
            state_root,
            transactions_root: EMPTY_TRANSACTIONS,
            withdrawals_root,
            receipts_root: EMPTY_RECEIPTS,
            logs_bloom: Default::default(),
            timestamp: attributes.payload_attributes.timestamp,
            mix_hash: attributes.payload_attributes.prev_randao,
            nonce: BEACON_NONCE,
            base_fee_per_gas: Some(base_fee),
            number: parent_block.number + 1,
            gas_limit: block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: 0,
            extra_data,
            blob_gas_used,
            excess_blob_gas,
            parent_beacon_block_root: attributes.payload_attributes.parent_beacon_block_root,
        };

        let block = Block { header, body: vec![], ommers: vec![], withdrawals };
        let sealed_block = block.seal_slow();

        Ok(OptimismBuiltPayload::new(
            attributes.payload_attributes.payload_id(),
            sealed_block,
            U256::ZERO,
            chain_spec,
            attributes,
        ))
    }
}

/// Constructs an Ethereum transaction payload from the transactions sent through the
/// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
/// the payload attributes, the transaction pool will be ignored and the only transactions
/// included in the payload will be those sent through the attributes.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
#[inline]
pub(crate) fn optimism_payload_builder<Pool, Client>(
    args: BuildArguments<Pool, Client, OptimismPayloadBuilderAttributes, OptimismBuiltPayload>,
    _compute_pending_block: bool,
) -> Result<BuildOutcome<OptimismBuiltPayload>, PayloadBuilderError>
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

    let state_provider = client.state_by_block_hash(config.parent_block.hash())?;
    let state = StateProviderDatabase::new(&state_provider);
    let mut db =
        State::builder().with_database_ref(cached_reads.as_db(&state)).with_bundle_update().build();
    let extra_data = config.extra_data();
    let PayloadConfig {
        initialized_block_env,
        initialized_cfg,
        parent_block,
        attributes,
        chain_spec,
        ..
    } = config;

    debug!(target: "payload_builder", id=%attributes.payload_attributes.payload_id(), parent_hash = ?parent_block.hash(), parent_number = parent_block.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let block_gas_limit: u64 = attributes
        .gas_limit
        .unwrap_or_else(|| initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX));
    let base_fee = initialized_block_env.basefee.to::<u64>();

    let mut executed_txs = Vec::new();
    let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
        base_fee,
        initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
    ));

    let mut total_fees = U256::ZERO;

    let block_number = initialized_block_env.number.to::<u64>();

    let is_regolith = chain_spec
        .is_fork_active_at_timestamp(Hardfork::Regolith, attributes.payload_attributes.timestamp);

    // apply eip-4788 pre block contract call
    pre_block_beacon_root_contract_call(
        &mut db,
        &chain_spec,
        block_number,
        &initialized_cfg,
        &initialized_block_env,
        &attributes,
    )?;

    // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
    // blocks will always have at least a single transaction in them (the L1 info transaction),
    // so we can safely assume that this will always be triggered upon the transition and that
    // the above check for empty blocks will never be hit on OP chains.
    reth_revm::optimism::ensure_create2_deployer(
        chain_spec.clone(),
        attributes.payload_attributes.timestamp,
        &mut db,
    )
    .map_err(|_| {
        PayloadBuilderError::other(OptimismPayloadBuilderError::ForceCreate2DeployerFail)
    })?;

    let mut receipts = Vec::new();
    for sequencer_tx in &attributes.transactions {
        // Check if the job was cancelled, if so we can exit early.
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        // A sequencer's block should never contain blob transactions.
        if matches!(sequencer_tx.tx_type(), TxType::Eip4844) {
            return Err(PayloadBuilderError::other(
                OptimismPayloadBuilderError::BlobTransactionRejected,
            ))
        }

        // Convert the transaction to a [TransactionSignedEcRecovered]. This is
        // purely for the purposes of utilizing the [tx_env_with_recovered] function.
        // Deposit transactions do not have signatures, so if the tx is a deposit, this
        // will just pull in its `from` address.
        let sequencer_tx = sequencer_tx.clone().try_into_ecrecovered().map_err(|_| {
            PayloadBuilderError::other(OptimismPayloadBuilderError::TransactionEcRecoverFailed)
        })?;

        // Cache the depositor account prior to the state transition for the deposit nonce.
        //
        // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
        // were not introduced in Bedrock. In addition, regular transactions don't have deposit
        // nonces, so we don't need to touch the DB for those.
        let depositor = (is_regolith && sequencer_tx.is_deposit())
            .then(|| {
                db.load_cache_account(sequencer_tx.signer())
                    .map(|acc| acc.account_info().unwrap_or_default())
            })
            .transpose()
            .map_err(|_| {
                PayloadBuilderError::other(OptimismPayloadBuilderError::AccountLoadFailed(
                    sequencer_tx.signer(),
                ))
            })?;

        let mut evm = revm::Evm::builder()
            .with_db(&mut db)
            .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
                initialized_cfg.clone(),
                initialized_block_env.clone(),
                tx_env_with_recovered(&sequencer_tx),
            ))
            .build();

        let ResultAndState { result, state } = match evm.transact() {
            Ok(res) => res,
            Err(err) => {
                match err {
                    EVMError::Transaction(err) => {
                        trace!(target: "payload_builder", %err, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                        continue
                    }
                    err => {
                        // this is an error that we should treat as fatal for this attempt
                        return Err(PayloadBuilderError::EvmExecutionError(err))
                    }
                }
            }
        };

        // to release the db reference drop evm.
        drop(evm);
        // commit changes
        db.commit(state);

        let gas_used = result.gas_used();

        // add gas used by the transaction to cumulative gas used, before creating the receipt
        cumulative_gas_used += gas_used;

        // Push transaction changeset and calculate header bloom filter for receipt.
        receipts.push(Some(Receipt {
            tx_type: sequencer_tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            deposit_nonce: depositor.map(|account| account.nonce),
            // The deposit receipt version was introduced in Canyon to indicate an update to how
            // receipt hashes should be computed when set. The state transition process
            // ensures this is only set for post-Canyon deposit transactions.
            deposit_receipt_version: chain_spec
                .is_fork_active_at_timestamp(
                    Hardfork::Canyon,
                    attributes.payload_attributes.timestamp,
                )
                .then_some(1),
        }));

        // append transaction to the list of executed transactions
        executed_txs.push(sequencer_tx.into_signed());
    }

    if !attributes.no_tx_pool {
        while let Some(pool_tx) = best_txs.next() {
            // ensure we still have capacity for this transaction
            if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(&pool_tx);
                continue
            }

            // A sequencer's block should never contain blob transactions.
            if pool_tx.tx_type() == TxType::Eip4844 as u8 {
                return Err(PayloadBuilderError::other(
                    OptimismPayloadBuilderError::BlobTransactionRejected,
                ))
            }

            // check if the job was cancelled, if so we can exit early
            if cancel.is_cancelled() {
                return Ok(BuildOutcome::Cancelled)
            }

            // convert tx to a signed transaction
            let tx = pool_tx.to_recovered_transaction();

            // Configure the environment for the block.
            let mut evm = revm::Evm::builder()
                .with_db(&mut db)
                .with_env_with_handler_cfg(EnvWithHandlerCfg::new_with_cfg_env(
                    initialized_cfg.clone(),
                    initialized_block_env.clone(),
                    tx_env_with_recovered(&tx),
                ))
                .build();

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
                                // if the nonce is too low, we can skip this transaction
                                trace!(target: "payload_builder", %err, ?tx, "skipping nonce too low transaction");
                            } else {
                                // if the transaction is invalid, we can skip it and all of its
                                // descendants
                                trace!(target: "payload_builder", %err, ?tx, "skipping invalid transaction and its descendants");
                                best_txs.mark_invalid(&pool_tx);
                            }

                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err))
                        }
                    }
                }
            };
            // drop evm so db is released.
            drop(evm);
            // commit changes
            db.commit(state);

            let gas_used = result.gas_used();

            // add gas used by the transaction to cumulative gas used, before creating the
            // receipt
            cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Some(Receipt {
                tx_type: tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.into_logs().into_iter().map(Into::into).collect(),
                deposit_nonce: None,
                deposit_receipt_version: None,
            }));

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(Some(base_fee))
                .expect("fee is always valid; execution succeeded");
            total_fees += U256::from(miner_fee) * U256::from(gas_used);

            // append transaction to the list of executed transactions
            executed_txs.push(tx.into_signed());
        }
    }

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // can skip building the block
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    let WithdrawalsOutcome { withdrawals_root, withdrawals } = commit_withdrawals(
        &mut db,
        &chain_spec,
        attributes.payload_attributes.timestamp,
        attributes.clone().payload_attributes.withdrawals,
    )?;

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    db.merge_transitions(BundleRetention::PlainState);

    let bundle = BundleStateWithReceipts::new(
        db.take_bundle(),
        Receipts::from_vec(vec![receipts]),
        block_number,
    );
    let receipts_root = bundle
        .optimism_receipts_root_slow(
            block_number,
            chain_spec.as_ref(),
            attributes.payload_attributes.timestamp,
        )
        .expect("Number is in range");
    let logs_bloom = bundle.block_logs_bloom(block_number).expect("Number is in range");

    // calculate the state root
    let state_root = state_provider.state_root(bundle.state())?;

    // create the block header
    let transactions_root = proofs::calculate_transaction_root(&executed_txs);

    // initialize empty blob sidecars. There are no blob transactions on L2.
    let blob_sidecars = Vec::new();
    let mut excess_blob_gas = None;
    let mut blob_gas_used = None;

    // only determine cancun fields when active
    if chain_spec.is_cancun_active_at_timestamp(attributes.payload_attributes.timestamp) {
        excess_blob_gas = if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) {
            let parent_excess_blob_gas = parent_block.excess_blob_gas.unwrap_or_default();
            let parent_blob_gas_used = parent_block.blob_gas_used.unwrap_or_default();
            Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        } else {
            // for the first post-fork block, both parent.blob_gas_used and
            // parent.excess_blob_gas are evaluated as 0
            Some(calculate_excess_blob_gas(0, 0))
        };

        blob_gas_used = Some(0);
    }

    let header = Header {
        parent_hash: parent_block.hash(),
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        beneficiary: initialized_block_env.coinbase,
        state_root,
        transactions_root,
        receipts_root,
        withdrawals_root,
        logs_bloom,
        timestamp: attributes.payload_attributes.timestamp,
        mix_hash: attributes.payload_attributes.prev_randao,
        nonce: BEACON_NONCE,
        base_fee_per_gas: Some(base_fee),
        number: parent_block.number + 1,
        gas_limit: block_gas_limit,
        difficulty: U256::ZERO,
        gas_used: cumulative_gas_used,
        extra_data,
        parent_beacon_block_root: attributes.payload_attributes.parent_beacon_block_root,
        blob_gas_used,
        excess_blob_gas,
    };

    // seal the block
    let block = Block { header, body: executed_txs, ommers: vec![], withdrawals };

    let sealed_block = block.seal_slow();
    debug!(target: "payload_builder", ?sealed_block, "sealed built block");

    let mut payload = OptimismBuiltPayload::new(
        attributes.payload_attributes.id,
        sealed_block,
        total_fees,
        chain_spec,
        attributes,
    );

    // extend the payload with the blob sidecars from the executed txs
    payload.extend_sidecars(blob_sidecars);

    Ok(BuildOutcome::Better { payload, cached_reads })
}
