//! Optimism's [PayloadBuilder] implementation.

use super::*;
use reth_primitives::Hardfork;

/// Constructs an Ethereum transaction payload from the transactions sent through the
/// Payload attributes by the sequencer. If the `no_tx_pool` argument is passed in
/// the payload attributes, the transaction pool will be ignored and the only transactions
/// included in the payload will be those sent through the attributes.
///
/// Given build arguments including an Ethereum client, transaction pool,
/// and configuration, this function creates a transaction payload. Returns
/// a result indicating success with the payload or an error in case of failure.
#[inline]
fn optimism_payload_builder<Pool, Client>(
    args: BuildArguments<Pool, Client>,
) -> Result<BuildOutcome, PayloadBuilderError>
where
    Client: StateProviderFactory,
    Pool: TransactionPool,
{
    let BuildArguments { client, pool, mut cached_reads, config, cancel, best_payload } = args;

    dbg!("[MADE IT TO PAYLOAD BUILDER]");

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

    dbg!(
        "[OP BUILDER] LOOPING SEQ TXS: ",
        &attributes.transactions.len(),
        &attributes.transactions
    );

    // Transactions sent via the payload attributes are force included at the top of the block, in
    // the order that they were sent in.
    for sequencer_tx in attributes.transactions {
        // Check if the job was cancelled, if so we can exit early.
        if cancel.is_cancelled() {
            dbg!("[OP BUILDER] CANCELLED");
            return Ok(BuildOutcome::Cancelled)
        }

        // Convert the transaction to a [TransactionSignedEcRecovered]. This is
        // purely for the purposes of utilizing the [tx_env_with_recovered] function.
        // Deposit transactions do not have signatures, so if the tx is a deposit, this
        // will just pull in its `from` address.
        let sequencer_tx = sequencer_tx.clone().try_into_ecrecovered().map_err(|err| {
            dbg!("[OP BUILDER] err converting to ecrecovered", err);
            PayloadBuilderError::TransactionEcRecoverFailed
        })?;

        dbg!("GOT SEQUENCER TX", &sequencer_tx);

        let mut cfg = initialized_cfg.clone();

        if sequencer_tx.is_deposit() {
            cfg.disable_base_fee = true;
        }

        // Configure the environment for the block.
        let env = Env {
            cfg,
            block: initialized_block_env.clone(),
            tx: tx_env_with_recovered(&sequencer_tx),
        };

        let mut evm = revm::EVM::with_env(env);
        evm.database(&mut db);

        let ResultAndState { result, state } = match evm.transact() {
            Ok(res) => res,
            Err(err) => {
                // TODO(clabby): This could be an issue - deposit transactions should always be
                // included with a receipt regardless of if there was an error or not. The
                // sequencer performs some basic validation on the transactions it sends prior
                // to sending a fork choice update, so this shouldn't be an issue, but we may
                // want to revisit this.
                match err {
                    EVMError::Transaction(err) => {
                        dbg!("Transaction error", err);
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
                        dbg!("EVM Error", &err);
                        // this is an error that we should treat as fatal for this attempt
                        return Err(PayloadBuilderError::EvmExecutionError(err))
                    }
                }
            }
        };

        dbg!("EXECUTED ", sequencer_tx.hash());

        // commit changes
        commit_state_changes(&mut db, &mut post_state, block_number, state, true);

        // Push transaction changeset and calculate header bloom filter for receipt.
        post_state.add_receipt(
            block_number,
            Receipt {
                tx_type: sequencer_tx.tx_type(),
                success: result.is_success(),
                cumulative_gas_used,
                logs: result.logs().into_iter().map(into_reth_log).collect(),
                deposit_nonce: if chain_spec
                    .is_fork_active_at_timestamp(Hardfork::Regolith, attributes.timestamp) &&
                    sequencer_tx.is_deposit()
                {
                    // Recovering the signer from the deposit transaction is only fetching
                    // the `from` address. Deposit transactions have no signature.
                    let from = sequencer_tx.signer();
                    let account = db.load_account(from)?;
                    // The deposit nonce is the account's nonce - 1. The account's nonce
                    // was incremented during the execution of the deposit transaction
                    // above.
                    Some(account.info.nonce.saturating_sub(1))
                } else {
                    None
                },
            },
        );

        dbg!("COMMITTED STATE CHANGES");

        // append transaction to the list of executed transactions
        executed_txs.push(sequencer_tx.into_signed());
    }

    dbg!("[OP BUILDER] LOOPING POOL TXS");

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
                            trace!(?err, ?tx, "skipping invalid transaction and its descendants");
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
        let miner_fee =
            tx.effective_tip_per_gas(base_fee).expect("fee is always valid; execution succeeded");
        total_fees += U256::from(miner_fee) * U256::from(gas_used);

        // append transaction to the list of executed transactions
        executed_txs.push(tx.into_signed());
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
    };

    dbg!("[OP BUILDER] BUILD PAYLOAD HEADER", &header);

    // seal the block
    let block = Block { header, body: executed_txs, ommers: vec![], withdrawals };

    let sealed_block = block.seal_slow();
    dbg!("[OP BUILDER] RETURNING BUILD OUTCOME");
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
