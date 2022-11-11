use crate::{
    revm_wrap::{self, State, SubState},
    Config,
};
use reth_interfaces::{
    executor::{BlockExecutor, Error},
    provider::StateProvider,
};
use reth_primitives::BlockLocked;
use revm::{AnalysisKind, ExecutionResult, SpecId, EVM};

/// Main block executor
pub struct Executor {
    /// Configuration, Spec and optional flags.
    pub config: Config,
}

impl Executor {
    /// Create new Executor
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Verify block. Execute all transaction and compare results.
    pub fn verify<DB: StateProvider>(&self, block: &BlockLocked, db: DB) -> Result<(), Error> {
        let db = SubState::new(State::new(db));
        let mut evm = EVM::new();
        evm.database(db);

        evm.env.cfg.chain_id = 1.into();
        evm.env.cfg.spec_id = SpecId::LATEST;
        evm.env.cfg.perf_all_precompiles_have_balance = true;
        evm.env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

        revm_wrap::fill_block_env(&mut evm.env.block, block);
        let mut cumulative_gas_used = 0;

        for (transaction, receipt) in block.body.iter().zip(block.receipts.iter()) {
            revm_wrap::fill_tx_env(&mut evm.env.tx, transaction.as_ref());

            // execute transaction
            let ExecutionResult { exit_reason, gas_used, logs, .. } = evm.transact_commit();

            // fatal internal error
            if exit_reason == revm::Return::FatalExternalError {
                return Err(Error::ExecutionFatalError);
            }

            // Success flag was added in `EIP-658: Embedding transaction status code in receipts`
            let is_success = matches!(
                exit_reason,
                revm::Return::Continue
                    | revm::Return::Stop
                    | revm::Return::Return
                    | revm::Return::SelfDestruct
            );

            if receipt.success != is_success {
                return Err(Error::ExecutionSuccessDiff {
                    got: is_success,
                    expected: receipt.success,
                });
            }

            // add spend gas
            cumulative_gas_used += gas_used;

            // check if used gas is same as in receipt
            if cumulative_gas_used != receipt.cumulative_gas_used {
                return Err(Error::ReceiptCumulativeGasUsedDiff {
                    got: cumulative_gas_used,
                    expected: receipt.cumulative_gas_used,
                });
            }

            // check logs count
            if receipt.logs.len() != logs.len() {
                return Err(Error::ReceiptLogCountDiff {
                    got: logs.len(),
                    expected: receipt.logs.len(),
                });
            }

            // iterate over all receipts and try to find difference between them
            if logs
                .iter()
                .zip(receipt.logs.iter())
                .find(|(revm_log, reth_log)| !revm_wrap::is_log_equal(revm_log, reth_log))
                .is_some()
            {
                return Err(Error::ReceiptLogDiff);
            }

            // TODO
            // receipt.bloom;

            // TODO
            // Sum of the transaction’s gas limit and the gas utilized in this block prior
        }

        Err(Error::VerificationFailed)
    }
}

impl BlockExecutor for Executor {}
