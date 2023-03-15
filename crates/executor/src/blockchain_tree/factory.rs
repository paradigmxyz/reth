//! Factory for instantiating the blockchain tree.

use super::{config::BlockchainTreeConfig, BlockchainTree};
use reth_db::database::Database;
use reth_interfaces::{consensus::Consensus, Error};
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, ShareableDatabase};
use std::sync::Arc;

/// Container for external abstractions.
pub struct TreeExternals<DB, C, EF> {
    /// Save sidechain, do reorgs and push new block to canonical chain that is inside db.
    pub db: DB,
    /// Consensus checks
    pub consensus: C,
    /// Create executor to execute blocks.
    pub executor_factory: EF,
    /// Chain spec
    pub chain_spec: Arc<ChainSpec>,
}

impl<DB, C, EF> TreeExternals<DB, C, EF> {
    /// Create new tree externals.
    pub fn new(db: DB, consensus: C, executor_factory: EF, chain_spec: Arc<ChainSpec>) -> Self {
        Self { db, consensus, executor_factory, chain_spec }
    }
}

impl<DB: Database, C, EF> TreeExternals<DB, C, EF> {
    /// Return shareable database helper structure.
    pub fn shareable_db(&self) -> ShareableDatabase<&DB> {
        ShareableDatabase::new(&self.db, self.chain_spec.clone())
    }
}

impl<DB: Clone, C: Clone, EF: Clone> Clone for TreeExternals<DB, C, EF> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            consensus: self.consensus.clone(),
            executor_factory: self.executor_factory.clone(),
            chain_spec: self.chain_spec.clone(),
        }
    }
}

/// The factory for instantiating the blockchain tree.
pub struct BlockchainTreeFactory<DB, C, EF> {
    /// Externals
    externals: TreeExternals<DB, C, EF>,
    /// Blockchain tree configuration.
    config: BlockchainTreeConfig,
}

impl<DB, C, EF> BlockchainTreeFactory<DB, C, EF> {
    /// Create blockchain tree factory.
    pub fn new(db: DB, consensus: C, executor_factory: EF, chain_spec: Arc<ChainSpec>) -> Self {
        Self::from_externals(TreeExternals::new(db, consensus, executor_factory, chain_spec))
    }

    /// Create blockchain tree factory from tree externals;
    pub fn from_externals(externals: TreeExternals<DB, C, EF>) -> Self {
        Self { externals, config: BlockchainTreeConfig::default() }
    }

    /// Override the default tree configuration.
    pub fn with_config(mut self, config: BlockchainTreeConfig) -> Self {
        self.config = config;
        self
    }
}

impl<DB, C, EF> BlockchainTreeFactory<DB, C, EF>
where
    DB: Database + Clone,
    C: Consensus + Clone,
    EF: ExecutorFactory + Clone,
{
    /// Create instance of the blockchain tree.
    pub fn create_tree(&self) -> Result<BlockchainTree<DB, C, EF>, Error> {
        BlockchainTree::new(self.externals.clone(), self.config.clone())
    }
}
