//! Blockchain tree externals.

use reth_db::database::Database;
use reth_primitives::ChainSpec;
use reth_provider::ShareableDatabase;
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
