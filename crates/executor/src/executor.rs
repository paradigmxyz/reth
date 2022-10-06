#![allow(missing_debug_implementations)]

use crate::Config;
use reth_interfaces::{
    consensus::Consensus,
    executor::{BlockExecutor, Error, ExecutorDb},
};
use reth_primitives::Block;

/// Main block executor
pub struct Executor {
    /// Configuration, Spec and optional flags.
    config: Config,
    /// Database
    db: Box<dyn ExecutorDb>,
    /// Consensus
    consensus: Box<dyn Consensus>,
}

impl Executor {
    /// Create new Executor
    pub fn new(config: Config, db: Box<dyn ExecutorDb>, consensus: Box<dyn Consensus>) -> Self {
        Self { config, db, consensus }
    }

    /// Verify block (TODO)
    pub fn verify(&self, block: &Block) -> Result<(), Error> {
        // TODO example
        let _ = self.consensus.validate_header(&block.header);
        if self.config.example {
            let _block_exist = self.db.get_block(block.header.number);
        }
        Err(Error::VerificationFailed)
    }
}

impl BlockExecutor for Executor {}
