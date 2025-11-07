//! XLayer-specific payload builder extensions
//!
//! This module extends the standard OP payload builder with XLayer-specific
//! transaction interception logic, similar to op-geth's worker_okx.go

use crate::{
    builder::{ExecutionInfo, OpPayloadBuilderCtx},
    intercept_bridge_transaction_if_need, OpPayloadPrimitives,
};
use alloy_consensus::{Transaction, Typed2718};
use alloy_primitives::U256;
use reth_chainspec::EthChainSpec;
use reth_evm::{
    execute::{BlockBuilder, BlockExecutionError, BlockExecutor, BlockValidationError},
    ConfigureEvm, Evm,
};
use reth_optimism_forks::OpHardforks;
use reth_optimism_primitives::transaction::OpTransaction;
use reth_optimism_txpool::{
    estimated_da_size::DataAvailabilitySized,
    interop::{is_valid_interop, MaybeInteropTransaction},
    OpPooledTx,
};
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_payload_util::PayloadTransactions;
use reth_primitives_traits::TxTy;
use reth_transaction_pool::PoolTransaction;
use revm::context::result::ExecutionResult;
use tracing::trace;

impl<Evm, ChainSpec, Attrs> OpPayloadBuilderCtx<Evm, ChainSpec, Attrs>
where
    Evm: ConfigureEvm<Primitives: OpPayloadPrimitives>,
    ChainSpec: EthChainSpec + OpHardforks,
    Attrs: PayloadBuilderAttributes,
{
    /// Execute best transactions from the transaction pool with bridge interception
    ///
    /// This method is similar to `execute_best_transactions` but adds bridge transaction
    /// interception logic. It executes transactions one by one, checking each transaction's
    /// logs for bridge events. If a bridge event matching the interception criteria is
    /// detected, the transaction is marked as invalid and skipped.
    ///
    /// # Arguments
    ///
    /// * `info` - Execution info tracking gas usage and fees
    /// * `builder` - Block builder for executing transactions
    /// * `best_txs` - Iterator of best transactions from the pool
    ///
    /// # Returns
    ///
    /// * `Ok(Some(()))` - If the job was cancelled
    /// * `Ok(None)` - If all transactions were processed successfully
    /// * `Err(...)` - If a fatal error occurred during execution
    pub fn execute_best_transactions_xlayer(
        &self,
        info: &mut ExecutionInfo,
        builder: &mut impl BlockBuilder<Primitives = Evm::Primitives>,
        mut best_txs: impl PayloadTransactions<
            Transaction: PoolTransaction<Consensus = TxTy<Evm::Primitives>> + OpPooledTx,
        >,
    ) -> Result<Option<()>, PayloadBuilderError> {
        let block_gas_limit = builder.evm_mut().block().gas_limit;
        let block_da_limit = self.da_config.max_da_block_size();
        let tx_da_limit = self.da_config.max_da_tx_size();
        let base_fee = builder.evm_mut().block().basefee;

        while let Some(tx) = best_txs.next(()) {
            let interop = tx.interop_deadline();
            let tx_da_size = tx.estimated_da_size();
            let tx = tx.into_consensus();
            if info.is_tx_over_limits(
                tx_da_size,
                block_gas_limit,
                tx_da_limit,
                block_da_limit,
                tx.gas_limit(),
            ) {
                // we can't fit this transaction into the block, so we need to mark it as
                // invalid which also removes all dependent transaction from
                // the iterator before we can continue
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue
            }

            // A sequencer's block should never contain blob or deposit transactions from the pool.
            if tx.is_eip4844() || tx.is_deposit() {
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue
            }

            // We skip invalid cross chain txs, they would be removed on the next block update in
            // the maintenance job
            if let Some(interop) = interop &&
                !is_valid_interop(interop, self.config.attributes.timestamp())
            {
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue
            }
            // check if the job was cancelled, if so we can exit early
            if self.cancel.is_cancelled() {
                return Ok(Some(()))
            }

            let mut should_skip = false;
            let gas_used = match builder.executor_mut().execute_transaction_with_result_closure(
                tx.clone(),
                |result| {
                    if let ExecutionResult::Success { logs, .. } = result {
                        if intercept_bridge_transaction_if_need(
                            logs,
                            tx.signer(),
                            &self.bridge_intercept,
                        )
                        .is_err()
                        {
                            should_skip = true;
                        }
                    }
                },
            ) {
                Ok(gas_used) => gas_used,
                Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                    error,
                    ..
                })) => {
                    if error.is_nonce_too_low() {
                        // if the nonce is too low, we can skip this transaction
                        trace!(target: "payload_builder", %error, ?tx, "skipping nonce too low transaction");
                    } else {
                        // if the transaction is invalid, we can skip it and all of its
                        // descendants
                        trace!(target: "payload_builder", %error, ?tx, "skipping invalid transaction and its descendants");
                        best_txs.mark_invalid(tx.signer(), tx.nonce());
                    }
                    continue
                }
                Err(err) => {
                    // this is an error that we should treat as fatal for this attempt
                    return Err(PayloadBuilderError::EvmExecutionError(Box::new(err)))
                }
            };

            if should_skip {
                best_txs.mark_invalid(tx.signer(), tx.nonce());
                continue;
            }

            // add gas used by the transaction to cumulative gas used, before creating the
            // receipt
            info.cumulative_gas_used += gas_used;
            info.cumulative_da_bytes_used += tx_da_size;

            // update and add to total fees
            let miner_fee = tx
                .effective_tip_per_gas(base_fee)
                .expect("fee is always valid; execution succeeded");
            info.total_fees += U256::from(miner_fee) * U256::from(gas_used);
        }

        Ok(None)
    }
}
