use crate::{
    revm_wrap::{self, State, SubState},
    Config,
};
use reth_interfaces::{
    executor::{BlockExecutor, Error},
    provider::StateProvider,
};
use reth_primitives::{bloom::logs_bloom, BlockLocked, Bloom, Log, Receipt};
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
        let mut receipts = Vec::with_capacity(block.body.len());

        for transaction in block.body.iter() {
            // The sum of the transaction’s gas limit, Tg, and the gas utilised in this block prior,
            // must be no greater than the block’s gasLimit.
            let block_available_gas = block.gas_limit - cumulative_gas_used;
            if transaction.gas_limit() > block_available_gas {
                return Err(Error::TransactionGasLimitMoreThenAvailableBlockGas {
                    transaction_gas_limit: transaction.gas_limit(),
                    block_available_gas,
                })
            }

            // Fill revm structure.
            revm_wrap::fill_tx_env(&mut evm.env.tx, transaction.as_ref());

            // Execute transaction.
            let ExecutionResult { exit_reason, gas_used, logs, .. } = evm.transact_commit();

            // Fatal internal error.
            if exit_reason == revm::Return::FatalExternalError {
                return Err(Error::ExecutionFatalError)
            }

            // Success flag was added in `EIP-658: Embedding transaction status code in receipts`.
            let is_success = matches!(
                exit_reason,
                revm::Return::Continue |
                    revm::Return::Stop |
                    revm::Return::Return |
                    revm::Return::SelfDestruct
            );

            // Add spend gas.
            cumulative_gas_used += gas_used;

            // Transform logs to reth format.
            let logs: Vec<Log> = logs
                .into_iter()
                .map(|l| Log { address: l.address, topics: l.topics, data: l.data })
                .collect();

            // Push receipts for calculating header bloom filter.
            receipts.push(Receipt {
                tx_type: transaction.tx_type(),
                success: is_success,
                cumulative_gas_used,
                bloom: logs_bloom(logs.iter()), // TODO
                logs,
            });
        }

        // TODO do state root.

        // Check if gas used matches the value set in header.
        if block.gas_used != cumulative_gas_used {
            return Err(Error::BlockGasUsed { got: cumulative_gas_used, expected: block.gas_used })
        }

        // Check receipts root.
        let receipts_root = reth_primitives::proofs::calculate_receipt_root(receipts.iter());
        if block.receipts_root != receipts_root {
            return Err(Error::ReceiptRootDiff { got: receipts_root, expected: block.receipts_root })
        }

        // Create header log bloom.
        let expected_logs_bloom = receipts.iter().fold(Bloom::zero(), |bloom, r| bloom | r.bloom);
        if expected_logs_bloom != block.logs_bloom {
            return Err(Error::BloomLogDiff {
                expected: Box::new(block.logs_bloom),
                got: Box::new(expected_logs_bloom),
            })
        }

        Ok(())
    }
}

impl BlockExecutor for Executor {}
