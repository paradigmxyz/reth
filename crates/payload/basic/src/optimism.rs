//! Optimism's [PayloadBuilder] implementation.

use super::*;
use reth_primitives::Hardfork;
use reth_revm::{
    executor,
    optimism::{executor::fail_deposit_tx, L1BlockInfo},
    to_reth_acc,
};
use revm::primitives::ExecutionResult;

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
) -> Result<BuildOutcome, PayloadBuilderError>
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

    let extra_data = config.extra_data();
    let PayloadConfig {
        initialized_block_env,
        initialized_cfg,
        parent_block,
        attributes,
        chain_spec,
        ..
    } = config;

    debug!(parent_hash=?parent_block.hash, parent_number=parent_block.number, "building new payload");

    let state = State::new(client.state_by_block_hash(parent_block.hash)?);
    let mut db = CacheDB::new(cached_reads.as_db(&state));
    let mut post_state = PostState::default();

    let mut cumulative_gas_used = 0;
    let block_gas_limit: u64 = attributes
        .gas_limit
        .unwrap_or(initialized_block_env.gas_limit.try_into().unwrap_or(u64::MAX));
    let base_fee = initialized_block_env.basefee.to::<u64>();

    let mut executed_txs = Vec::new();
    let mut best_txs = pool.best_transactions_with_base_fee(base_fee);

    let mut total_fees = U256::ZERO;

    let block_number = initialized_block_env.number.to::<u64>();

    let is_regolith =
        chain_spec.is_fork_active_at_timestamp(Hardfork::Regolith, attributes.timestamp);

    // Parse the L1 block info from the first transaction in the payload attributes. This
    // transaction should always be the L1 info tx. We skip the first 4 bytes of the calldata
    // because the first 4 bytes are the function selector.
    let l1_block_info = (!attributes.transactions.is_empty())
        .then(|| optimism::L1BlockInfo::try_from(&attributes.transactions[0].input()[4..]))
        .transpose()
        .map_err(|_| PayloadBuilderError::L1BlockInfoParseFailed)?;

    // Transactions sent via the payload attributes are force included at the top of the block, in
    // the order that they were sent in.
    for sequencer_tx in attributes.transactions {
        // Check if the job was cancelled, if so we can exit early.
        if cancel.is_cancelled() {
            return Ok(BuildOutcome::Cancelled)
        }

        // Convert the transaction to a [TransactionSignedEcRecovered]. This is
        // purely for the purposes of utilizing the [tx_env_with_recovered] function.
        // Deposit transactions do not have signatures, so if the tx is a deposit, this
        // will just pull in its `from` address.
        let sequencer_tx = sequencer_tx
            .clone()
            .try_into_ecrecovered()
            .map_err(|_| PayloadBuilderError::TransactionEcRecoverFailed)?;

        // Compute the L1 cost of the transaction. This is the amount of ETH that it will cost to
        // post the entire encoded typed transaction to L1.
        let mut encoded = BytesMut::default();
        sequencer_tx.encode_enveloped(&mut encoded);
        let l1_cost = l1_block_info.as_ref().map(|l1_block_info| {
            l1_block_info.calculate_tx_l1_cost(
                Arc::clone(&chain_spec),
                attributes.timestamp,
                &encoded.freeze().into(),
                sequencer_tx.is_deposit(),
            )
        });

        let mut cfg = initialized_cfg.clone();
        let mut tx_env = tx_env_with_recovered(&sequencer_tx);

        let sender = db.load_account(sequencer_tx.signer())?.clone();
        let mut sender_new = sender.clone();

        // Before regolith, deposit transaction gas accounting was as follows:
        // - System tx: 0 gas used
        // - Regular Deposit tx: gas used = gas limit
        //
        // After regolith, system transactions are deprecated and deposit transactions report the
        // gas used during execution. All deposit transactions execute the gas refund for
        // accounting (it is a noop because of the gas price), but still skip coinbase payments.
        //
        // Deposit transactions only report this gas - their gas is prepaid on L1 and
        // the gas price is always 0. Deposit txs should not be subject to any regular
        // balance checks, base fee checks, or block gas limit checks.
        if sequencer_tx.is_deposit() {
            cfg.disable_base_fee = true;
            cfg.disable_balance_check = true;
            cfg.disable_block_gas_limit = true;

            if is_regolith {
                tx_env.nonce = Some(sender.info.nonce);
            } else {
                cfg.disable_gas_refund = true;
            }

            // Increase the sender's balance in the database if the deposit transaction mints eth.
            if let Some(m) = sequencer_tx.mint() {
                let m = U256::from(m);
                sender_new.info.balance += m;

                executor::increment_account_balance(
                    &mut db,
                    &mut post_state,
                    parent_block.number + 1,
                    sequencer_tx.signer(),
                    m,
                )?;
                db.insert_account_info(sequencer_tx.signer(), sender_new.info);
            }
        } else if let Some(l1_cost) = l1_cost {
            // Decrement the sender's balance by the L1 cost of the transaction prior to execution.
            sender_new.info.balance -= l1_cost;
            executor::decrement_account_balance(
                &mut db,
                &mut post_state,
                parent_block.number + 1,
                sequencer_tx.signer(),
                l1_cost,
            )?;
            db.insert_account_info(sequencer_tx.signer(), sender_new.info);
        }

        // Configure the environment for the block.
        let env = Env { cfg, block: initialized_block_env.clone(), tx: tx_env };

        let mut evm = revm::EVM::with_env(env);
        evm.database(&mut db);

        let ResultAndState { result, state } = match evm.transact() {
            Ok(res) => res,
            Err(err) => {
                if sequencer_tx.is_deposit() {
                    // Manually bump the nonce and include a receipt for the deposit transaction.
                    let sender = sequencer_tx.signer();
                    fail_deposit_tx!(
                        db,
                        sender,
                        block_number,
                        sequencer_tx,
                        &mut post_state,
                        &mut cumulative_gas_used,
                        is_regolith,
                        PayloadBuilderError::AccountLoadFailed(sender)
                    );
                    executed_txs.push(sequencer_tx.into_signed());
                    continue
                }

                match err {
                    EVMError::Transaction(err) => {
                        if matches!(err, InvalidTransaction::NonceTooLow { .. }) {
                            // if the nonce is too low, we can skip this transaction
                            trace!(?err, ?sequencer_tx, "skipping nonce too low transaction");
                        } else {
                            // if the transaction is invalid, we can skip it and all of its
                            // descendants
                            trace!(
                                ?err,
                                ?sequencer_tx,
                                "skipping invalid transaction and its descendants"
                            );
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
        commit_state_changes(&mut db, &mut post_state, block_number, state, true);

        if chain_spec.optimism {
            // Before Regolith, system transactions were a special type of deposit transaction
            // that contributed no gas usage to the block. Regular deposits reported their gas
            // usage as the gas limit of their transaction. After Regolith, system transactions
            // are deprecated and all deposit transactions report the gas used during execution
            // regardless of whether or not the transaction reverts.
            if is_regolith &&
                sequencer_tx.is_deposit() &&
                matches!(result, ExecutionResult::Halt { .. })
            {
                // Manually bump the nonce if the transaction was a contract creation.
                if sequencer_tx.to().is_none() {
                    let sender_account = db.load_account(sequencer_tx.signer())?.clone();
                    let mut new_sender_account = sender_account.clone();
                    new_sender_account.info.nonce += 1;
                    post_state.change_account(
                        parent_block.number + 1,
                        sequencer_tx.signer(),
                        to_reth_acc(&sender_account.info),
                        to_reth_acc(&new_sender_account.info),
                    );
                    db.insert_account_info(sequencer_tx.signer(), sender_account.info);
                }

                cumulative_gas_used += sequencer_tx.gas_limit();
            } else if is_regolith || !sequencer_tx.is_deposit() {
                cumulative_gas_used += result.gas_used();
            } else if sequencer_tx.is_deposit() &&
                (!result.is_success() || !sequencer_tx.is_system_transaction())
            {
                cumulative_gas_used += sequencer_tx.gas_limit();
            }

            // If the transaction is not a deposit, we route the l1 cost and base fee to the
            // appropriate optimism vaults.
            if !sequencer_tx.is_deposit() {
                // Route the l1 cost and base fee to the appropriate optimism vaults
                if let Some(l1_cost) = l1_cost {
                    executor::increment_account_balance(
                        &mut db,
                        &mut post_state,
                        parent_block.number + 1,
                        *executor::optimism::L1_FEE_RECIPIENT,
                        l1_cost,
                    )?
                }
                executor::increment_account_balance(
                    &mut db,
                    &mut post_state,
                    parent_block.number + 1,
                    *executor::optimism::BASE_FEE_RECIPIENT,
                    U256::from(base_fee.saturating_mul(result.gas_used())),
                )?;
            }
        } else {
            cumulative_gas_used += result.gas_used();
        }

        // Push transaction changeset and calculate header bloom filter for receipt.
        post_state.add_receipt(
            block_number,
            Receipt {
                tx_type: sequencer_tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.logs().into_iter().map(into_reth_log).collect(),
                deposit_nonce: (is_regolith && sequencer_tx.is_deposit())
                    .then_some(sender.info.nonce),
            },
        );

        // append transaction to the list of executed transactions
        executed_txs.push(sequencer_tx.into_signed());
    }

    if !attributes.no_tx_pool {
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
                                trace!(?err, ?tx, "skipping nonce too low transaction");
                            } else {
                                // if the transaction is invalid, we can skip it and all of its
                                // descendants
                                trace!(
                                    ?err,
                                    ?tx,
                                    "skipping invalid transaction and its descendants"
                                );
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

            let gas_used = result.gas_used();

            // commit changes
            commit_state_changes(&mut db, &mut post_state, block_number, state, true);

            // add gas used by the transaction to cumulative gas used, before creating the receipt
            cumulative_gas_used += gas_used;

            // Push transaction changeset and calculate header bloom filter for receipt.
            post_state.add_receipt(
                block_number,
                Receipt {
                    tx_type: tx.tx_type(),
                    success: result.is_success(),
                    cumulative_gas_used,
                    logs: result.logs().into_iter().map(into_reth_log).collect(),
                    #[cfg(feature = "optimism")]
                    deposit_nonce: None,
                },
            );

            // update add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
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

    let WithdrawalsOutcome { withdrawals_root, withdrawals } = commit_withdrawals(
        &mut db,
        &mut post_state,
        &chain_spec,
        block_number,
        attributes.timestamp,
        attributes.withdrawals,
    )?;

    let receipts_root = post_state.receipts_root(block_number);
    let logs_bloom = post_state.logs_bloom(block_number);

    // calculate the state root
    let state_root = state.state().state_root(post_state)?;

    // create the block header
    let transactions_root = proofs::calculate_transaction_root(&executed_txs);

    let header = Header {
        parent_hash: parent_block.hash,
        ommers_hash: EMPTY_OMMER_ROOT,
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
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
    };

    // seal the block
    let block = Block { header, body: executed_txs, ommers: vec![], withdrawals };

    let sealed_block = block.seal_slow();
    Ok(BuildOutcome::Better {
        payload: BuiltPayload::new(attributes.id, sealed_block, total_fees),
        cached_reads,
    })
}

/// Optimism's payload builder
#[derive(Clone)]
pub struct OptimismPayloadBuilder;

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
        optimism_payload_builder(args)
    }
}
