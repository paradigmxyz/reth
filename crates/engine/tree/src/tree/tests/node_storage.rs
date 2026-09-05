//! Datadir ownership independent of database handles, so a simulated node can reopen cold.

use reth_chainspec::ChainSpec;
use reth_db::{mdbx::DatabaseArguments, DatabaseEnv};
use reth_provider::{
    providers::{RocksDBBuilder, StaticFileProviderBuilder},
    test_utils::MockNodeTypesWithDB,
    ProviderFactory,
};
use reth_storage_overlay::OverlayManager;
use reth_tasks::Runtime;
use std::sync::{Arc, Weak};

/// Owns the datadir across launches while keeping no live native database handle.
pub(super) struct NodeStorage {
    directory: tempfile::TempDir,
    chain: Arc<ChainSpec>,
    last_database: Weak<DatabaseEnv>,
}

impl NodeStorage {
    pub(super) fn new(chain: Arc<ChainSpec>) -> Self {
        Self { directory: tempfile::tempdir().unwrap(), chain, last_database: Weak::new() }
    }

    /// Opening twice without releasing the previous engine/provider is a harness error.
    pub(super) fn open(&mut self, overlay: OverlayManager, runtime: Runtime) -> Factory {
        assert!(self.last_database.upgrade().is_none(), "node still holds its database open");
        let database = Arc::new(
            reth_db::init_db(self.directory.path().join("db"), DatabaseArguments::test()).unwrap(),
        );
        self.last_database = Arc::downgrade(&database);
        reth_fs_util::create_dir_all(self.directory.path().join("static_files")).unwrap();
        ProviderFactory::new(
            database,
            self.chain.clone(),
            StaticFileProviderBuilder::read_write(self.directory.path().join("static_files"))
                .with_genesis_block_number(self.chain.genesis.number.unwrap_or_default())
                .build()
                .unwrap(),
            RocksDBBuilder::new(self.directory.path().join("rocksdb"))
                .with_default_tables()
                .build()
                .unwrap(),
            runtime,
        )
        .unwrap()
        .with_overlay_manager(overlay)
    }
}

pub(super) type NodeTypes = MockNodeTypesWithDB<Arc<DatabaseEnv>>;
pub(super) type Factory = ProviderFactory<NodeTypes>;
