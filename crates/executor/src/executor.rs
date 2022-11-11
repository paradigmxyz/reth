use crate::{
    revm_wrap::{self, State, SubState},
    Config,
};
use reth_interfaces::{
    executor::{BlockExecutor, Error},
    provider::StateProvider,
};
use reth_primitives::BlockLocked;
use revm::{AnalysisKind, SpecId, EVM};

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

        for transaction in block.body.iter() {
            // TODO Check if Transaction is new
            revm_wrap::fill_tx_env(&mut evm.env.tx, transaction.as_ref());

            let res = evm.transact_commit();

            if res.exit_reason == revm::Return::FatalExternalError {
                // stop executing. Fatal error thrown from database
            }

            // calculate commulative gas used

            // create receipt
            // bloom filter from logs

            // Sum of the transaction’s gas limit and the gas utilized in this block prior

            // Receipt outcome EIP-658: Embedding transaction status code in receipts
            // EIP-658 supperseeded EIP-98 in Byzantium fork
        }

        Err(Error::VerificationFailed)
    }
}

impl BlockExecutor for Executor {}
