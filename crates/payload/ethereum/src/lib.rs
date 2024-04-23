//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

use reth_basic_payload_builder::{
    commit_withdrawals, is_better_payload, pre_block_beacon_root_contract_call, BuildArguments,
    BuildOutcome, PayloadBuilder, PayloadConfig, WithdrawalsOutcome,
};
use reth_payload_builder::{
    error::PayloadBuilderError, EthBuiltPayload, EthPayloadBuilderAttributes,
};
use reth_primitives::{
    constants::{
        eip4844::MAX_DATA_GAS_PER_BLOCK, BEACON_NONCE, EMPTY_RECEIPTS, EMPTY_TRANSACTIONS,
    },
    eip4844::calculate_excess_blob_gas,
    proofs,
    revm::env::tx_env_with_recovered,
    Block, Header, IntoRecoveredTransaction, Receipt, Receipts, EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{BundleStateWithReceipts, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::{BestTransactionsAttributes, TransactionPool};
use revm::{
    db::states::bundle_state::BundleRetention,
    primitives::{EVMError, EnvWithHandlerCfg, InvalidTransaction, ResultAndState},
    DatabaseCommit, State,
};
use tracing::{debug, trace, warn};

/// Ethereum payload builder
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

// Default implementation of [PayloadBuilder] for unit type
impl<Pool, Client> PayloadBuilder<Pool, Client> for EthereumPayloadBuilder
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    type Attributes = EthPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, EthPayloadBuilderAttributes, EthBuiltPayload>,
    ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError> {
        default_ethereum_payload_builder(args)
    }

    fn build_empty_payload(
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<EthBuiltPayload, PayloadBuilderError> {
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
                commit_withdrawals(&mut db, &chain_spec, attributes.timestamp, attributes.withdrawals.clone()).map_err(|err| {
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

        if chain_spec.is_cancun_active_at_timestamp(attributes.timestamp) {
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
            timestamp: attributes.timestamp,
            mix_hash: attributes.prev_randao,
            nonce: BEACON_NONCE,
            base_fee_per_gas: Some(base_fee),
            number: parent_block.number + 1,
            gas_limit: block_gas_limit,
            difficulty: U256::ZERO,
            gas_used: 0,
            extra_data,
            blob_gas_used,
            excess_blob_gas,
            parent_beacon_block_root: attributes.parent_beacon_block_root,
        };

        let block = Block { header, body: vec![], ommers: vec![], withdrawals };
        let sealed_block = block.seal_slow();

        Ok(EthBuiltPayload::new(attributes.payload_id(), sealed_block, U256::ZERO))
    }
}

/// Constructs an Ethereum transaction payload using the best transactions from the pool.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
#[inline]
pub fn default_ethereum_payload_builder<Pool, Client>(
    args: BuildArguments<Pool, Client, EthPayloadBuilderAttributes, EthBuiltPayload>,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
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

    debug!(target: "payload_builder", id=%attributes.id, parent_hash = ?parent_block.hash(), parent_number = parent_block.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let mut sum_blob_gas_used = 0;
    let block_gas_limit: u64 = initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX);
    let base_fee = initialized_block_env.basefee.to::<u64>();

    let mut executed_txs = Vec::new();

    let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
        base_fee,
        initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
    ));

    let mut total_fees = U256::ZERO;

    let block_number = initialized_block_env.number.to::<u64>();

    // apply eip-4788 pre block contract call
    pre_block_beacon_root_contract_call(
        &mut db,
        &chain_spec,
        block_number,
        &initialized_cfg,
        &initialized_block_env,
        &attributes,
    )?;

    let mut receipts = Vec::new();
    while let Some(pool_tx) = best_txs.next() {
        // ensure we still have capacity for this transaction
        if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
            // we can't fit this transaction into the block, so we need to mark it as invalid
            // which also removes all dependent transaction from the iterator before we can
            // continue
            best_txs.mark_invalid(&pool_tx);
            continue
        }

        // check if the job was cancelled, if so we can exit early
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        // convert tx to a signed transaction
        let tx = pool_tx.to_recovered_transaction();

        // There's only limited amount of blob space available per block, so we need to check if
        // the EIP-4844 can still fit in the block
        if let Some(blob_tx) = tx.transaction.as_eip4844() {
            let tx_blob_gas = blob_tx.blob_gas();
            if sum_blob_gas_used + tx_blob_gas > MAX_DATA_GAS_PER_BLOCK {
                // we can't fit this _blob_ transaction into the block, so we mark it as
                // invalid, which removes its dependent transactions from
                // the iterator. This is similar to the gas limit condition
                // for regular transactions above.
                trace!(target: "payload_builder", tx=?tx.hash, ?sum_blob_gas_used, ?tx_blob_gas, "skipping blob transaction because it would exceed the max data gas per block");
                best_txs.mark_invalid(&pool_tx);
                continue
            }
        }

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

        // add to the total blob gas used if the transaction successfully executed
        if let Some(blob_tx) = tx.transaction.as_eip4844() {
            let tx_blob_gas = blob_tx.blob_gas();
            sum_blob_gas_used += tx_blob_gas;

            // if we've reached the max data gas per block, we can skip blob txs entirely
            if sum_blob_gas_used == MAX_DATA_GAS_PER_BLOCK {
                best_txs.skip_blobs();
            }
        }

        let gas_used = result.gas_used();

        // add gas used by the transaction to cumulative gas used, before creating the receipt
        cumulative_gas_used += gas_used;

        // Push transaction changeset and calculate header bloom filter for receipt.
        #[allow(clippy::needless_update)] // side-effect of optimism fields
        receipts.push(Some(Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            ..Default::default()
        }));

        // update add to total fees
        let miner_fee = tx
            .effective_tip_per_gas(Some(base_fee))
            .expect("fee is always valid; execution succeeded");
        total_fees += U256::from(miner_fee) * U256::from(gas_used);

        // append transaction to the list of executed transactions
        executed_txs.push(tx.into_signed());
    }

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // can skip building the block
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    let WithdrawalsOutcome { withdrawals_root, withdrawals } =
        commit_withdrawals(&mut db, &chain_spec, attributes.timestamp, attributes.withdrawals)?;

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    db.merge_transitions(BundleRetention::PlainState);

    let bundle = BundleStateWithReceipts::new(
        db.take_bundle(),
        Receipts::from_vec(vec![receipts]),
        block_number,
    );
    let receipts_root = bundle.receipts_root_slow(block_number).expect("Number is in range");
    let logs_bloom = bundle.block_logs_bloom(block_number).expect("Number is in range");

    // calculate the state root
    let state_root = state_provider.state_root(bundle.state())?;

    // create the block header
    let transactions_root = proofs::calculate_transaction_root(&executed_txs);

    // initialize empty blob sidecars at first. If cancun is active then this will
    let mut blob_sidecars = Vec::new();
    let mut excess_blob_gas = None;
    let mut blob_gas_used = None;

    // only determine cancun fields when active
    if chain_spec.is_cancun_active_at_timestamp(attributes.timestamp) {
        // grab the blob sidecars from the executed txs
        blob_sidecars = pool.get_all_blobs_exact(
            executed_txs.iter().filter(|tx| tx.is_eip4844()).map(|tx| tx.hash).collect(),
        )?;

        excess_blob_gas = if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) {
            let parent_excess_blob_gas = parent_block.excess_blob_gas.unwrap_or_default();
            let parent_blob_gas_used = parent_block.blob_gas_used.unwrap_or_default();
            Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        } else {
            // for the first post-fork block, both parent.blob_gas_used and
            // parent.excess_blob_gas are evaluated as 0
            Some(calculate_excess_blob_gas(0, 0))
        };

        blob_gas_used = Some(sum_blob_gas_used);
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
        timestamp: attributes.timestamp,
        mix_hash: attributes.prev_randao,
        nonce: BEACON_NONCE,
        base_fee_per_gas: Some(base_fee),
        number: parent_block.number + 1,
        gas_limit: block_gas_limit,
        difficulty: U256::ZERO,
        gas_used: cumulative_gas_used,
        extra_data,
        parent_beacon_block_root: attributes.parent_beacon_block_root,
        blob_gas_used,
        excess_blob_gas,
    };

    // seal the block
    let block = Block { header, body: executed_txs, ommers: vec![], withdrawals };

    let sealed_block = block.seal_slow();
    debug!(target: "payload_builder", ?sealed_block, "sealed built block");

    let mut payload = EthBuiltPayload::new(attributes.id, sealed_block, total_fees);

    // extend the payload with the blob sidecars from the executed txs
    payload.extend_sidecars(blob_sidecars);

    Ok(BuildOutcome::Better { payload, cached_reads })
}
