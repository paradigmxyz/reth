use crate::processor::{verify_receipt, EVMProcessor};
use reth_interfaces::executor::{
    BlockExecutionError, BlockValidationError, OptimismBlockExecutionError,
};
use reth_primitives::{
    revm::compat::into_reth_log, revm_primitives::ResultAndState, BlockWithSenders, Hardfork,
    Receipt, U256,
};
use reth_provider::{BlockExecutor, BlockExecutorStats, BundleStateWithReceipts};
use revm::DatabaseCommit;
use std::time::Instant;
use tracing::{debug, trace};

impl<'a> BlockExecutor for EVMProcessor<'a> {
    fn execute(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        let receipts = self.execute_inner(block, total_difficulty)?;
        self.save_receipts(receipts)
    }

    fn execute_and_verify_receipt(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError> {
        // execute block
        let receipts = self.execute_inner(block, total_difficulty)?;

        // TODO Before Byzantium, receipts contained state root that would mean that expensive
        // operation as hashing that is needed for state root got calculated in every
        // transaction This was replaced with is_success flag.
        // See more about EIP here: https://eips.ethereum.org/EIPS/eip-658
        if self.chain_spec.fork(Hardfork::Byzantium).active_at_block(block.header.number) {
            let time = Instant::now();
            if let Err(error) = verify_receipt(
                block.header.receipts_root,
                block.header.logs_bloom,
                receipts.iter(),
                self.chain_spec.as_ref(),
                block.timestamp,
            ) {
                debug!(target: "evm", ?error, ?receipts, "receipts verification failed");
                return Err(error)
            };
            self.stats.receipt_root_duration += time.elapsed();
        }

        self.save_receipts(receipts)
    }

    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        self.init_env(&block.header, total_difficulty);

        // perf: do not execute empty blocks
        if block.body.is_empty() {
            return Ok((Vec::new(), 0))
        }

        let is_regolith =
            self.chain_spec.fork(Hardfork::Regolith).active_at_timestamp(block.timestamp);

        // Ensure that the create2deployer is force-deployed at the canyon transition. Optimism
        // blocks will always have at least a single transaction in them (the L1 info transaction),
        // so we can safely assume that this will always be triggered upon the transition and that
        // the above check for empty blocks will never be hit on OP chains.
        super::ensure_create2_deployer(self.chain_spec().clone(), block.timestamp, self.db_mut())
            .map_err(|_| {
            BlockExecutionError::OptimismBlockExecution(
                OptimismBlockExecutionError::ForceCreate2DeployerFail,
            )
        })?;

        let mut cumulative_gas_used = 0;
        let mut receipts = Vec::with_capacity(block.body.len());
        for (sender, transaction) in block.transactions_with_sender() {
            let time = Instant::now();
            // The sum of the transaction’s gas limit, Tg, and the gas utilized in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.header.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas &&
                (is_regolith || !transaction.is_system_transaction())
            {
                return Err(BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                }
                .into())
            }

            // Cache the depositor account prior to the state transition for the deposit nonce.
            //
            // Note that this *only* needs to be done post-regolith hardfork, as deposit nonces
            // were not introduced in Bedrock. In addition, regular transactions don't have deposit
            // nonces, so we don't need to touch the DB for those.
            let depositor = (is_regolith && transaction.is_deposit())
                .then(|| {
                    self.db_mut()
                        .load_cache_account(*sender)
                        .map(|acc| acc.account_info().unwrap_or_default())
                })
                .transpose()
                .map_err(|_| BlockExecutionError::ProviderError)?;

            // Execute transaction.
            let ResultAndState { result, state } = self.transact(transaction, *sender)?;
            trace!(
                target: "evm",
                ?transaction, ?result, ?state,
                "Executed transaction"
            );
            self.stats.execution_duration += time.elapsed();
            let time = Instant::now();

            self.db_mut().commit(state);

            self.stats.apply_state_duration += time.elapsed();

            // append gas used
            cumulative_gas_used += result.gas_used();

            // Push transaction changeset and calculate header bloom filter for receipt.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                // Success flag was added in `EIP-658: Embedding transaction status code in
                // receipts`.
                success: result.is_success(),
                cumulative_gas_used,
                // convert to reth log
                logs: result.into_logs().into_iter().map(into_reth_log).collect(),
                #[cfg(feature = "optimism")]
                deposit_nonce: depositor.map(|account| account.nonce),
                // The deposit receipt version was introduced in Canyon to indicate an update to how
                // receipt hashes should be computed when set. The state transition process ensures
                // this is only set for post-Canyon deposit transactions.
                #[cfg(feature = "optimism")]
                deposit_receipt_version: self
                    .chain_spec()
                    .is_fork_active_at_timestamp(Hardfork::Canyon, block.timestamp)
                    .then_some(1),
            });
        }

        Ok((receipts, cumulative_gas_used))
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        let receipts = std::mem::take(&mut self.receipts);
        BundleStateWithReceipts::new(
            self.evm.db().unwrap().take_bundle(),
            receipts,
            self.first_block.unwrap_or_default(),
        )
    }

    fn stats(&self) -> BlockExecutorStats {
        self.stats.clone()
    }

    fn size_hint(&self) -> Option<usize> {
        self.evm.db.as_ref().map(|db| db.bundle_size_hint())
    }
}
