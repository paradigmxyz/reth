use crate::{revm_wrap, Config};
use reth_interfaces::{
    consensus::Consensus,
    executor::{BlockExecutor, Error, ExecutorDb},
};
use reth_primitives::BlockLocked;
use revm::{db::EmptyDB, AnalysisKind, Env, SpecId};

/// Main block executor
pub struct Executor {
    /// Configuration, Spec and optional flags.
    pub config: Config,
    /// Database
    pub db: Box<dyn ExecutorDb>,
    /// Consensus
    pub consensus: Box<dyn Consensus>,
}

impl Executor {
    /// Create new Executor
    pub fn new(config: Config, db: Box<dyn ExecutorDb>, consensus: Box<dyn Consensus>) -> Self {
        Self { config, db, consensus }
    }

    /// Verify block. Execute all transaction and compare results.
    pub fn verify(&self, block: &BlockLocked) -> Result<(), Error> {
        let mut env = Env::default();
        env.cfg.chain_id = 1.into();
        env.cfg.spec_id = SpecId::LATEST;
        env.cfg.perf_all_precompiles_have_balance = true;
        env.cfg.perf_analyse_created_bytecodes = AnalysisKind::Raw;

        revm_wrap::fill_block_env(&mut env.block, block);

        let _database = revm_wrap::TempStateDb::new(EmptyDB::default());

        for transaction in block.body.iter() {
            revm_wrap::fill_tx_env(&mut env.tx, transaction.as_ref());
        }

        Err(Error::VerificationFailed)
    }
}

impl BlockExecutor for Executor {}
