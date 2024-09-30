//! A basic Ethereum payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![allow(clippy::useless_let_if_seq)]

use alloy_primitives::U256;
use reth_basic_payload_builder::{
    commit_withdrawals, is_better_payload, BuildArguments, BuildOutcome, PayloadBuilder,
    PayloadConfig, WithdrawalsOutcome,
};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_errors::RethError;
use reth_evm::{system_calls::SystemCaller, ConfigureEvm, NextBlockEnvAttributes};
use reth_evm_ethereum::{eip6110::parse_deposits_from_receipts, EthEvmConfig};
use reth_execution_types::ExecutionOutcome;
use reth_payload_builder::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_payload_primitives::{PayloadBuilderAttributes, PayloadBuilderError};
use reth_primitives::{
    constants::{eip4844::MAX_DATA_GAS_PER_BLOCK, BEACON_NONCE},
    proofs::{self, calculate_requests_root},
    revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg},
    Block, BlockBody, EthereumHardforks, Header, Receipt, EMPTY_OMMER_ROOT_HASH,
};
use reth_provider::{ChainSpecProvider, StateProviderFactory};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::{
    noop::NoopTransactionPool, BestTransactionsAttributes, TransactionPool,
};
use reth_trie::HashedPostState;
use revm::{
    db::{states::bundle_state::BundleRetention, State},
    primitives::{EVMError, EnvWithHandlerCfg, InvalidTransaction, ResultAndState},
    DatabaseCommit,
};
use revm_primitives::calc_excess_blob_gas;
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Ethereum payload builder
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthereumPayloadBuilder<EvmConfig = EthEvmConfig> {
    /// The type responsible for creating the evm.
    evm_config: EvmConfig,
}

impl<EvmConfig> EthereumPayloadBuilder<EvmConfig> {
    /// `EthereumPayloadBuilder` constructor.
    pub const fn new(evm_config: EvmConfig) -> Self {
        Self { evm_config }
    }
}

impl<EvmConfig> EthereumPayloadBuilder<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
{
    /// Returns the configured [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the targeted payload
    /// (that has the `parent` as its parent).
    fn cfg_and_block_env(
        &self,
        config: &PayloadConfig<EthPayloadBuilderAttributes>,
        parent: &Header,
    ) -> (CfgEnvWithHandlerCfg, BlockEnv) {
        let next_attributes = NextBlockEnvAttributes {
            timestamp: config.attributes.timestamp(),
            suggested_fee_recipient: config.attributes.suggested_fee_recipient(),
            prev_randao: config.attributes.prev_randao(),
        };
        self.evm_config.next_cfg_and_block_env(parent, next_attributes)
    }
}

// Default implementation of [PayloadBuilder] for unit type
impl<EvmConfig, Pool, Client> PayloadBuilder<Pool, Client> for EthereumPayloadBuilder<EvmConfig>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool,
{
    type Attributes = EthPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Pool, Client, EthPayloadBuilderAttributes, EthBuiltPayload>,
    ) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError> {
        let (cfg_env, block_env) = self.cfg_and_block_env(&args.config, &args.config.parent_block);
        default_ethereum_payload(self.evm_config.clone(), args, cfg_env, block_env)
    }

    fn build_empty_payload(
        &self,
        client: &Client,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<EthBuiltPayload, PayloadBuilderError> {
        let args = BuildArguments {
            client,
            config,
            // we use defaults here because for the empty payload we don't need to execute anything
            pool: NoopTransactionPool::default(),
            cached_reads: Default::default(),
            cancel: Default::default(),
            best_payload: None,
        };
        let (cfg_env, block_env) = self.cfg_and_block_env(&args.config, &args.config.parent_block);
        default_ethereum_payload(self.evm_config.clone(), args, cfg_env, block_env)?
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
pub fn default_ethereum_payload<EvmConfig, Pool, Client>(
    evm_config: EvmConfig,
    args: BuildArguments<Pool, Client, EthPayloadBuilderAttributes, EthBuiltPayload>,
    initialized_cfg: CfgEnvWithHandlerCfg,
    initialized_block_env: BlockEnv,
) -> Result<BuildOutcome<EthBuiltPayload>, PayloadBuilderError>
where
    EvmConfig: ConfigureEvm<Header = Header>,
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = ChainSpec>,
    Pool: TransactionPool,
{
    let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

    let chain_spec = client.chain_spec();
    let state_provider = client.state_by_block_hash(config.parent_block.hash())?;
    let state = StateProviderDatabase::new(state_provider);
    let mut db =
        State::builder().with_database_ref(cached_reads.as_db(state)).with_bundle_update().build();
    let PayloadConfig { parent_block, extra_data, attributes } = config;

    debug!(target: "payload_builder", id=%attributes.id, parent_hash = ?parent_block.hash(), parent_number = parent_block.number, "building new payload");
    let mut cumulative_gas_used = 0;
    let mut sum_blob_gas_used = 0;
    let block_gas_limit: u64 = initialized_block_env.gas_limit.to::<u64>();
    let base_fee = initialized_block_env.basefee.to::<u64>();

    let mut executed_txs = Vec::new();
    let mut executed_senders = Vec::new();

    let mut best_txs = pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
        base_fee,
        initialized_block_env.get_blob_gasprice().map(|gasprice| gasprice as u64),
    ));

    let mut total_fees = U256::ZERO;

    let block_number = initialized_block_env.number.to::<u64>();

    let mut system_caller = SystemCaller::new(&evm_config, chain_spec.clone());

    // apply eip-4788 pre block contract call
    system_caller
        .pre_block_beacon_root_contract_call(
            &mut db,
            &initialized_cfg,
            &initialized_block_env,
            attributes.parent_beacon_block_root,
        )
        .map_err(|err| {
            warn!(target: "payload_builder",
                parent_hash=%parent_block.hash(),
                %err,
                "failed to apply beacon root contract call for payload"
            );
            PayloadBuilderError::Internal(err.into())
        })?;

    // apply eip-2935 blockhashes update
    system_caller.pre_block_blockhashes_contract_call(
        &mut db,
        &initialized_cfg,
        &initialized_block_env,
        parent_block.hash(),
    )
    .map_err(|err| {
        warn!(target: "payload_builder", parent_hash=%parent_block.hash(), %err, "failed to update blockhashes for payload");
        PayloadBuilderError::Internal(err.into())
    })?;

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

        let env = EnvWithHandlerCfg::new_with_cfg_env(
            initialized_cfg.clone(),
            initialized_block_env.clone(),
            evm_config.tx_env(&tx),
        );

        // Configure the environment for the block.
        let mut evm = evm_config.evm_with_env(&mut db, env);

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

        // append sender and transaction to the respective lists
        executed_senders.push(tx.signer());
        executed_txs.push(tx.into_signed());
    }

    // check if we have a better block
    if !is_better_payload(best_payload.as_ref(), total_fees) {
        // can skip building the block
        return Ok(BuildOutcome::Aborted { fees: total_fees, cached_reads })
    }

    // calculate the requests and the requests root
    let (requests, requests_root) = if chain_spec
        .is_prague_active_at_timestamp(attributes.timestamp)
    {
        let deposit_requests = parse_deposits_from_receipts(&chain_spec, receipts.iter().flatten())
            .map_err(|err| PayloadBuilderError::Internal(RethError::Execution(err.into())))?;
        let withdrawal_requests = system_caller
            .post_block_withdrawal_requests_contract_call(
                &mut db,
                &initialized_cfg,
                &initialized_block_env,
            )
            .map_err(|err| PayloadBuilderError::Internal(err.into()))?;
        let consolidation_requests = system_caller
            .post_block_consolidation_requests_contract_call(
                &mut db,
                &initialized_cfg,
                &initialized_block_env,
            )
            .map_err(|err| PayloadBuilderError::Internal(err.into()))?;

        let requests = [deposit_requests, withdrawal_requests, consolidation_requests].concat();
        let requests_root = calculate_requests_root(&requests);
        (Some(requests.into()), Some(requests_root))
    } else {
        (None, None)
    };

    let WithdrawalsOutcome { withdrawals_root, withdrawals } =
        commit_withdrawals(&mut db, &chain_spec, attributes.timestamp, attributes.withdrawals)?;

    // merge all transitions into bundle state, this would apply the withdrawal balance changes
    // and 4788 contract call
    db.merge_transitions(BundleRetention::Reverts);

    let execution_outcome = ExecutionOutcome::new(
        db.take_bundle(),
        vec![receipts.clone()].into(),
        block_number,
        vec![requests.clone().unwrap_or_default()],
    );
    let receipts_root =
        execution_outcome.receipts_root_slow(block_number).expect("Number is in range");
    let logs_bloom = execution_outcome.block_logs_bloom(block_number).expect("Number is in range");

    // calculate the state root
    let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.state().state);
    let (state_root, trie_output) = {
        let state_provider = db.database.0.inner.borrow_mut();
        state_provider.db.state_root_with_updates(hashed_state.clone()).inspect_err(|err| {
            warn!(target: "payload_builder",
                parent_hash=%parent_block.hash(),
                %err,
                "failed to calculate state root for payload"
            );
        })?
    };

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
            Some(calc_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        } else {
            // for the first post-fork block, both parent.blob_gas_used and
            // parent.excess_blob_gas are evaluated as 0
            Some(calc_excess_blob_gas(0, 0))
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
        nonce: BEACON_NONCE.into(),
        base_fee_per_gas: Some(base_fee),
        number: parent_block.number + 1,
        gas_limit: block_gas_limit,
        difficulty: U256::ZERO,
        gas_used: cumulative_gas_used,
        extra_data,
        parent_beacon_block_root: attributes.parent_beacon_block_root,
        blob_gas_used: blob_gas_used.map(Into::into),
        excess_blob_gas: excess_blob_gas.map(Into::into),
        requests_root,
    };

    // seal the block
    let block = Block {
        header,
        body: BlockBody { transactions: executed_txs, ommers: vec![], withdrawals, requests },
    };

    let sealed_block = block.seal_slow();
    debug!(target: "payload_builder", ?sealed_block, "sealed built block");

    // create the executed block data
    let executed = ExecutedBlock {
        block: Arc::new(sealed_block.clone()),
        senders: Arc::new(executed_senders),
        execution_output: Arc::new(execution_outcome),
        hashed_state: Arc::new(hashed_state),
        trie: Arc::new(trie_output),
    };

    let mut payload = EthBuiltPayload::new(attributes.id, sealed_block, total_fees, Some(executed));

    // extend the payload with the blob sidecars from the executed txs
    payload.extend_sidecars(blob_sidecars);

    Ok(BuildOutcome::Better { payload, cached_reads })
}
