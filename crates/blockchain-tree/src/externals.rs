//! Blockchain tree externals.

use reth_db::database::Database;
use reth_primitives::ChainSpec;
use reth_provider::ShareableDatabase;
use std::sync::Arc;

/// A container for external components.
///
/// This is a simple container for external components used throughout the blockchain tree
/// implementation:
///
/// - A handle to the database
/// - A handle to the consensus engine
/// - The executor factory to execute blocks with
/// - The chain spec
#[derive(Debug)]
pub struct TreeExternals<DB, C, EF> {
    /// The database, used to commit the canonical chain, or unwind it.
    pub(crate) db: DB,
    /// The consensus engine.
    pub(crate) consensus: C,
    /// The executor factory to execute blocks with.
    pub(crate) executor_factory: EF,
    /// The chain spec.
    pub(crate) chain_spec: Arc<ChainSpec>,
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
