//! Optimism-specific execution logic.

use crate::{
    executor::{verify_receipt, Executor},
    to_reth_acc,
};
use reth_interfaces::executor::{BlockExecutionError, BlockValidationError};
use reth_primitives::{bytes::BytesMut, Address, Block, Hardfork, Receipt, U256};
use reth_provider::{BlockExecutor, PostState, StateProvider};
use reth_revm_primitives::into_reth_log;
use revm::primitives::{ExecutionResult, ResultAndState};
use std::sync::Arc;

impl<DB> BlockExecutor<DB> for Executor<DB>
where
    DB: StateProvider,
{
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError> {
        let (post_state, cumulative_gas_used) =
            self.execute_transactions(block, total_difficulty, senders)?;

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            return Err(BlockValidationError::BlockGasUsed {
                got: cumulative_gas_used,
                expected: block.gas_used,
            }
            .into())
        }

        self.apply_post_block_changes(block, total_difficulty, post_state)
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<PostState, BlockExecutionError> {
        let post_state = self.execute(block, total_difficulty, senders)?;

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is needed for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
            verify_receipt(
                block.header.receipts_root,
                block.header.logs_bloom,
                post_state.receipts(block.number).iter(),
            )?;
        }

        Ok(post_state)
    }

    fn execute_transactions(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(PostState, u64), BlockExecutionError> {
        // perf: do not execute empty blocks
        if block.body.is_empty() {
            return Ok((PostState::default(), 0))
        }
        let senders = self.recover_senders(&block.body, senders)?;

        self.init_env(&block.header, total_difficulty);

        let l1_block_info =
            self.chain_spec.optimism.then(|| super::L1BlockInfo::try_from(block)).transpose()?;

        let mut cumulative_gas_used = 0;
        let mut post_state = PostState::with_tx_capacity(block.number, block.body.len());
        for (transaction, sender) in block.body.iter().zip(senders) {
            // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;

            let is_regolith =
                self.chain_spec.fork(Hardfork::Regolith).active_at_timestamp(block.timestamp);

            // Before regolith, system transactions did not care about the block gas limit as
            // they did not contribute any gas usage to the block.
            if transaction.gas_limit() > block_available_gas &&
                (is_regolith || !transaction.is_system_transaction())
            {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            let chain_spec = Arc::clone(&self.chain_spec);
            let db = self.db();
            let sender_account =
                db.load_account(sender).map_err(|_| BlockExecutionError::ProviderError)?.clone();

            // Before regolith, deposit transaction gas accounting was as follows:
            // - System tx: 0 gas used
            // - Regular Deposit tx: gas used = gas limit
            //
            // After regolith, system transactions are deprecated and deposit transactions
            // report the gas used during execution. all deposit transactions receive a gas
            // refund, but still skip coinbase payments.
            //
            // Deposit transactions only report this gas - their gas is prepaid on L1 and
            // the gas price is always 0. Deposit txs should not be subject to any regular
            // balance checks, base fee checks, or block gas limit checks.
            if transaction.is_deposit() {
                self.evm.env.cfg.disable_base_fee = true;
                self.evm.env.cfg.disable_block_gas_limit = true;
                self.evm.env.cfg.disable_balance_check = true;

                if !is_regolith {
                    self.evm.env.cfg.disable_gas_refund = true;
                }
            }

            if let Some(m) = transaction.mint() {
                // Add balance to the caler account equal to the minted amount.
                // Note: This is unconditional, and will not be reverted if the tx fails
                // (unless the block can't be built at all due to gas limit constraints)
                self.increment_account_balance(
                    block.number,
                    sender,
                    U256::from(m),
                    &mut post_state,
                )?;
            }

            let mut encoded = BytesMut::default();
            transaction.encode_enveloped(&mut encoded);
            let l1_cost = l1_block_info.as_ref().map(|l1_block_info| {
                l1_block_info.calculate_tx_l1_cost(
                    chain_spec,
                    block.timestamp,
                    &encoded.freeze().into(),
                    transaction.is_deposit(),
                )
            });

            if let Some(l1_cost) = l1_cost {
                // Check if the sender balance can cover the L1 cost.
                // Deposits pay for their gas directly on L1 so they are exempt from the L2 tx fee.
                if !transaction.is_deposit() {
                    if sender_account.info.balance.cmp(&l1_cost) == std::cmp::Ordering::Less {
                        return Err(BlockExecutionError::InsufficientFundsForL1Cost {
                            have: sender_account.info.balance.to::<u64>(),
                            want: l1_cost.to::<u64>(),
                        })
                    }

                    // Safely take l1_cost from sender (the rest will be deducted by the
                    // internal EVM execution and included in result.gas_used())
                    // TODO(clabby): need to handle calls with `disable_balance_check` flag set?
                    self.decrement_account_balance(block.number, sender, l1_cost, &mut post_state)?;
                }
            }

            // Execute transaction.
            let ResultAndState { result, state } = match self.transact(transaction, sender) {
                Ok(res) => res,
                Err(err) => {
                    if transaction.is_deposit() {
                        fail_deposit_tx!(
                            self.db(),
                            sender,
                            block.number,
                            transaction,
                            &mut post_state,
                            &mut cumulative_gas_used,
                            is_regolith,
                            BlockExecutionError::ProviderError
                        );
                        // Reset all revm configuration flags for the next iteration.
                        self.evm.env.cfg.disable_base_fee = false;
                        self.evm.env.cfg.disable_block_gas_limit = false;
                        self.evm.env.cfg.disable_balance_check = false;
                        self.evm.env.cfg.disable_gas_refund = false;
                        continue
                    }
                    return Err(err)
                }
            };

            // commit changes
            self.commit_changes(
                block.number,
                state,
                self.chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(block.number),
                &mut post_state,
            );

            if self.chain_spec.optimism {
                // Before Regolith, system transactions were a special type of deposit transaction
                // that contributed no gas usage to the block. Regular deposits reported their gas
                // usage as the gas limit of their transaction. After Regolith, system transactions
                // are deprecated and all deposit transactions report the gas used during execution
                // regardless of whether or not the transaction reverts.
                if is_regolith &&
                    transaction.is_deposit() &&
                    matches!(result, ExecutionResult::Halt { .. })
                {
                    // Manually bump the nonce
                    let sender_account = self
                        .db()
                        .load_account(sender)
                        .map_err(|_| BlockExecutionError::ProviderError)?
                        .clone();
                    let mut new_sender_account = sender_account.clone();
                    new_sender_account.info.nonce += 1;
                    post_state.change_account(
                        block.number,
                        sender,
                        to_reth_acc(&sender_account.info),
                        to_reth_acc(&new_sender_account.info),
                    );
                    self.db().insert_account_info(sender, sender_account.info);

                    cumulative_gas_used += transaction.gas_limit();
                } else if is_regolith || !transaction.is_deposit() {
                    cumulative_gas_used += result.gas_used();
                } else if transaction.is_deposit() &&
                    (!result.is_success() || !transaction.is_system_transaction())
                {
                    cumulative_gas_used += transaction.gas_limit();
                }

                // Pay out fees to Optimism vaults if the transaction is not a deposit. Deposits
                // are exempt from vault fees.
                if !transaction.is_deposit() {
                    // Route the l1 cost and base fee to the appropriate optimism vaults
                    if let Some(l1_cost) = l1_cost {
                        self.increment_account_balance(
                            block.number,
                            *super::L1_FEE_RECIPIENT,
                            l1_cost,
                            &mut post_state,
                        )?
                    }
                    self.increment_account_balance(
                        block.number,
                        *super::BASE_FEE_RECIPIENT,
                        U256::from(
                            block
                                .base_fee_per_gas
                                .unwrap_or_default()
                                .saturating_mul(result.gas_used()),
                        ),
                        &mut post_state,
                    )?;
                }
            } else {
                cumulative_gas_used += result.gas_used();
            }

            // cast revm logs to reth logs
            let logs = result.logs().into_iter().map(into_reth_log).collect();

            // Push transaction changeset and calculate header bloom filter for receipt.
            post_state.add_receipt(
                block.number,
                Receipt {
                    tx_type: transaction.tx_type(),
                    // Success flag was added in `EIP-658: Embedding transaction status code in
                    // receipts`.
                    success: result.is_success(),
                    cumulative_gas_used,
                    logs,
                    // Deposit nonce is only recorded after Regolith for deposit transactions.
                    deposit_nonce: (is_regolith && transaction.is_deposit())
                        .then_some(sender_account.info.nonce),
                },
            );

            // Reset all revm configuration flags for the next iteration.
            if transaction.is_deposit() {
                self.evm.env.cfg.disable_base_fee = false;
                self.evm.env.cfg.disable_block_gas_limit = false;
                self.evm.env.cfg.disable_balance_check = false;
                self.evm.env.cfg.disable_gas_refund = false;
            }
        }

        Ok((post_state, cumulative_gas_used))
    }
}

/// If the Deposited transaction failed, the deposit must still be included. In this case, we need
/// to increment the sender nonce and disregard the state changes. The transaction is also recorded
/// as using all gas unless it is a system transaction.
#[macro_export]
macro_rules! fail_deposit_tx {
    (
        $db:expr,
        $sender:ident,
        $block_number:expr,
        $transaction:ident,
        $post_state:expr,
        $cumulative_gas_used:expr,
        $is_regolith:ident,
        $error:expr
    ) => {
        let sender_account = $db.load_account($sender).map_err(|_| $error)?;
        let old_sender_info = to_reth_acc(&sender_account.info);
        sender_account.info.nonce += 1;
        let new_sender_info = to_reth_acc(&sender_account.info);

        $post_state.change_account($block_number, $sender, old_sender_info, new_sender_info);
        let sender_info = sender_account.info.clone();
        $db.insert_account_info($sender, sender_info);

        if $is_regolith || !$transaction.is_system_transaction() {
            *$cumulative_gas_used += $transaction.gas_limit();
        }

        $post_state.add_receipt(
            $block_number,
            Receipt {
                tx_type: $transaction.tx_type(),
                success: false,
                cumulative_gas_used: *$cumulative_gas_used,
                logs: vec![],
                // Deposit nonces are only recorded after Regolith
                deposit_nonce: $is_regolith.then_some(old_sender_info.nonce),
            },
        );
    };
}

pub use fail_deposit_tx;
