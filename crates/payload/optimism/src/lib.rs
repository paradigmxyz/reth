//! Optimism's payload builder implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[cfg(feature = "optimism")]
pub use builder::*;

pub mod error;

#[cfg(feature = "optimism")]
mod builder {
    use crate::error::OptimismPayloadBuilderError;
    use reth_basic_payload_builder::*;
    use reth_payload_builder::{error::PayloadBuilderError, BuiltPayload};
    use reth_primitives::{
        constants::BEACON_NONCE,
        proofs,
        revm::{compat::into_reth_log, env::tx_env_with_recovered},
        Block, Hardfork, Header, IntoRecoveredTransaction, Receipt, Receipts,
        EMPTY_OMMER_ROOT_HASH, U256,
    };
    use reth_provider::{BundleStateWithReceipts, StateProviderFactory};
    use reth_revm::database::StateProviderDatabase;
    use reth_transaction_pool::TransactionPool;
    use revm::{
        db::states::bundle_state::BundleRetention,
        primitives::{EVMError, Env, InvalidTransaction, ResultAndState},
        DatabaseCommit, State,
    };
    use std::sync::Arc;
    use tracing::{debug, trace};

    /// Optimism's payload builder
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[non_exhaustive]
    pub struct OptimismPayloadBuilder {
        /// The rollup's compute pending block configuration option.
        // TODO(clabby): Implement this feature.
        compute_pending_block: bool,
    }

    impl OptimismPayloadBuilder {
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
    }

    /// Implementation of the [PayloadBuilder] trait for [OptimismPayloadBuilder].
    impl<Pool, Client> PayloadBuilder<Pool, Client> for OptimismPayloadBuilder
    where
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        fn try_build(
            &self,
            args: BuildArguments<Pool, Client>,
        ) -> Result<BuildOutcome, PayloadBuilderError> {
            optimism_payload_builder(args, self.compute_pending_block)
        }

        fn on_missing_payload(
            &self,
            args: BuildArguments<Pool, Client>,
        ) -> Option<Arc<BuiltPayload>> {
            // In Optimism, the PayloadAttributes can specify a `no_tx_pool` option that implies we
            // should not pull transactions from the tx pool. In this case, we build the payload
            // upfront with the list of transactions sent in the attributes without caring about
            // the results of the polling job, if a best payload has not already been built.
            if args.config.attributes.optimism_payload_attributes.no_tx_pool {
                if let Ok(BuildOutcome::Better { payload, .. }) = self.try_build(args) {
                    trace!(target: "payload_builder", "[OPTIMISM] Forced best payload");
                    let payload = Arc::new(payload);
                    return Some(payload)
                }
            }

            None
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
        args: BuildArguments<Pool, Client>,
        _compute_pending_block: bool,
    ) -> Result<BuildOutcome, PayloadBuilderError>
    where
        Client: StateProviderFactory,
        Pool: TransactionPool,
    {
        let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

        let state_provider = client.state_by_block_hash(config.parent_block.hash)?;
        let state = StateProviderDatabase::new(&state_provider);
        let mut db = State::builder()
            .with_database_ref(cached_reads.as_db(&state))
            .with_bundle_update()
            .build();
        let extra_data = config.extra_data();
        let PayloadConfig {
            initialized_block_env,
            initialized_cfg,
            parent_block,
            attributes,
            chain_spec,
            ..
        } = config;

        debug!(target: "payload_builder", id=%attributes.id, parent_hash = ?parent_block.hash, parent_number = parent_block.number, "building new payload");
        let mut cumulative_gas_used = 0;
        let block_gas_limit: u64 = attributes
            .optimism_payload_attributes
            .gas_limit
            .unwrap_or(initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX));
        let base_fee = initialized_block_env.basefee.to::<u64>();

        let mut executed_txs = Vec::new();
        let mut best_txs = pool.best_transactions_with_base_fee(base_fee);

        let mut total_fees = U256::ZERO;

        let block_number = initialized_block_env.number.to::<u64>();

        let is_regolith =
            chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, attributes.timestamp);

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        reth_revm::optimism::ensure_create2_deployer(
            chain_spec.clone(),
            attributes.timestamp,
            &mut db,
        )
        .map_err(|_| {
            PayloadBuilderError::other(OptimismPayloadBuilderError::ForceCreate2DeployerFail)
        })?;

        let mut receipts = Vec::new();
        for sequencer_tx in attributes.optimism_payload_attributes.transactions {
            // Check if the job was cancelled, if so we can exit early.
            if cancel.is_cancelled() {
                return Ok(BuildOutcome::Cancelled)
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

            // Configure the environment for the block.
            let env = Env {
                cfg: initialized_cfg.clone(),
                block: initialized_block_env.clone(),
                tx: tx_env_with_recovered(&sequencer_tx),
            };

            let mut evm = revm::EVM::with_env(env);
            evm.database(&mut db);

            let ResultAndState { result, state } = match evm.transact() {
                Ok(res) => res,
                Err(err) => {
                    match err {
                        EVMError::Transaction(err) => {
                            trace!(target: "optimism_payload_builder", ?err, ?sequencer_tx, "Error in sequencer transaction, skipping.");
                            continue
                        }
                        err => {
                            // this is an error that we should treat as fatal for this attempt
                            return Err(PayloadBuilderError::EvmExecutionError(err))
                        }
                    }
                }
            };

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
                logs: result.logs().into_iter().map(into_reth_log).collect(),
                deposit_nonce: depositor.map(|account| account.nonce),
                // The deposit receipt version was introduced in Canyon to indicate an update to how
                // receipt hashes should be computed when set. The state transition process
                // ensures this is only set for post-Canyon deposit transactions.
                deposit_receipt_version: chain_spec
                    .is_fork_active_at_timestamp(Hardfork::Canyon, attributes.timestamp)
                    .then_some(1),
            }));

            // append transaction to the list of executed transactions
            executed_txs.push(sequencer_tx.into_signed());
        }

        if !attributes.optimism_payload_attributes.no_tx_pool {
            while let Some(pool_tx) = best_txs.next() {
                // ensure we still have capacity for this transaction
                if cumulative_gas_used + pool_tx.gas_limit() > block_gas_limit {
                    // we can't fit this transaction into the block, so we need to mark it as
                    // invalid which also removes all dependent transaction from
                    // the iterator before we can continue
                    best_txs.mark_invalid(&pool_tx);
                    continue
                }

                // check if the job was cancelled, if so we can exit early
                if cancel.is_cancelled() {
                    return Ok(BuildOutcome::Cancelled)
                }

                // convert tx to a signed transaction
                let tx = pool_tx.to_recovered_transaction();

                // Configure the environment for the block.
                let env = Env {
                    cfg: initialized_cfg.clone(),
                    block: initialized_block_env.clone(),
                    tx: tx_env_with_recovered(&tx),
                };

                let mut evm = revm::EVM::with_env(env);
                evm.database(&mut db);

                let ResultAndState { result, state } = match evm.transact() {
                    Ok(res) => res,
                    Err(err) => {
                        match err {
                            EVMError::Transaction(err) => {
                                if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
                                    // if the nonce is too low, we can skip this transaction
                                    trace!(target: "payload_builder", ?err, ?tx, "skipping nonce too low transaction");
                                } else {
                                    // if the transaction is invalid, we can skip it and all of its
                                    // descendants
                                    trace!(target: "payload_builder", ?err, ?tx, "skipping invalid transaction and its descendants");
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
                    logs: result.logs().into_iter().map(into_reth_log).collect(),
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
        if !is_better_payload(best_payload.as_deref(), total_fees) {
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
        let receipts_root = bundle
            .receipts_root_slow(block_number, chain_spec.as_ref(), attributes.timestamp)
            .expect("Number is in range");
        let logs_bloom = bundle.block_logs_bloom(block_number).expect("Number is in range");

        // calculate the state root
        let state_root = state_provider.state_root(&bundle)?;

        // create the block header
        let transactions_root = proofs::calculate_transaction_root(&executed_txs);

        // Cancun is not yet active on Optimism chains.
        let blob_sidecars = Vec::new();
        let excess_blob_gas = None;
        let blob_gas_used = None;

        let header = Header {
            parent_hash: parent_block.hash,
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

        let mut payload = BuiltPayload::new(attributes.id, sealed_block, total_fees);

        // extend the payload with the blob sidecars from the executed txs
        payload.extend_sidecars(blob_sidecars);

        Ok(BuildOutcome::Better { payload, cached_reads })
    }
}
