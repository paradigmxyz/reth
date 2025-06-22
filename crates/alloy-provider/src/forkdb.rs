use crate::{
    backend::SharedBackend,
    cache::BlockchainDb,
    database::{ForkDbStateSnapshot, RevertStateSnapshotAction, StateSnapshot},
    state_snapshots::StateSnapshots,
    DatabaseError,
};
use alloy_eips::BlockId;
use alloy_primitives::Address;
use revm::{
    database::CacheDB,
    state::{Account, AccountInfo, Bytecode},
    Database, DatabaseCommit, DatabaseRef,
};
use revm_primitives::{HashMap, B256, U256};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{trace, warn};

/// structed after
/// a [revm::Database] that's forked off another client
///
/// The `backend` is used to retrieve (missing) data, which is then fetched from the remote
/// endpoint. The inner in-memory database holds this storage and will be used for write operations.
/// This database uses the `backend` for read and the `db` for write operations. But note the
/// `backend` will also write (missing) data to the `db` in the background
#[derive(Clone, Debug)]
pub struct ForkDb {
    /// Responsible for fetching missing data.
    ///
    /// This is responsible for getting data.
    backend: SharedBackend,
    /// Cached Database layer, ensures that changes are not written to the database that
    /// exclusively stores the state of the remote client.
    ///
    /// This separates Read/Write operations
    ///   - reads from the `SharedBackend as DatabaseRef` writes to the internal cache storage.
    cache_db: CacheDB<SharedBackend>,
    /// Contains all the data already fetched.
    ///
    /// This exclusively stores the _unchanged_ remote client state.
    db: BlockchainDb,
    /// Holds the state snapshots of a blockchain.
    state_snapshots: Arc<Mutex<StateSnapshots<ForkDbStateSnapshot>>>,
}

impl ForkDb {
    /// Creates a new instance of this DB
    pub fn new(backend: SharedBackend, db: BlockchainDb) -> Self {
        Self {
            cache_db: CacheDB::new(backend.clone()),
            backend,
            db,
            state_snapshots: Arc::new(Mutex::new(Default::default())),
        }
    }

    pub fn database(&self) -> &CacheDB<SharedBackend> {
        &self.cache_db
    }

    pub fn database_mut(&mut self) -> &mut CacheDB<SharedBackend> {
        &mut self.cache_db
    }

    pub fn state_snapshots(&self) -> &Arc<Mutex<StateSnapshots<ForkDbStateSnapshot>>> {
        &self.state_snapshots
    }

    /// Reset the fork to a fresh forked state, and optionally update the fork config
    pub fn reset(
        &mut self,
        _url: Option<String>,
        block_number: impl Into<BlockId>,
    ) -> Result<(), String> {
        self.backend.set_pinned_block(block_number).map_err(|err| err.to_string())?;

        // TODO need to find a way to update generic provider via url

        // wipe the storage retrieved from remote
        self.inner().db().clear();
        // create a fresh `CacheDB`, effectively wiping modified state
        self.cache_db = CacheDB::new(self.backend.clone());
        trace!(target: "backend::forkdb", "Cleared database");
        Ok(())
    }

    /// Flushes the cache to disk if configured
    pub fn flush_cache(&self) {
        self.db.cache().flush()
    }

    /// Returns the database that holds the remote state
    pub fn inner(&self) -> &BlockchainDb {
        &self.db
    }

    pub fn create_state_snapshot(&self) -> ForkDbStateSnapshot {
        let db = self.db.db();
        let state_snapshot = StateSnapshot {
            accounts: db.accounts.read().clone(),
            storage: db.storage.read().clone(),
            block_hashes: db.block_hashes.read().clone(),
        };
        ForkDbStateSnapshot { local: self.cache_db.clone(), state_snapshot }
    }

    pub async fn insert_state_snapshot(&self) -> U256 {
        let state_snapshot = self.create_state_snapshot();
        let mut state_snapshots = self.state_snapshots().lock().await;
        let id = state_snapshots.insert(state_snapshot);
        trace!(target: "backend::forkdb", "Created new snapshot {}", id);
        id
    }

    /// Removes the snapshot from the tracked snapshot and sets it as the current state
    pub async fn revert_state_snapshot(
        &mut self,
        id: U256,
        action: RevertStateSnapshotAction,
    ) -> bool {
        let state_snapshot = { self.state_snapshots().lock().await.remove_at(id) };
        if let Some(state_snapshot) = state_snapshot {
            if action.is_keep() {
                self.state_snapshots().lock().await.insert_at(state_snapshot.clone(), id);
            }
            let ForkDbStateSnapshot {
                local,
                state_snapshot: StateSnapshot { accounts, storage, block_hashes },
            } = state_snapshot;
            let db = self.inner().db();
            {
                let mut accounts_lock = db.accounts.write();
                accounts_lock.clear();
                accounts_lock.extend(accounts);
            }
            {
                let mut storage_lock = db.storage.write();
                storage_lock.clear();
                storage_lock.extend(storage);
            }
            {
                let mut block_hashes_lock = db.block_hashes.write();
                block_hashes_lock.clear();
                block_hashes_lock.extend(block_hashes);
            }

            self.cache_db = local;

            trace!(target: "backend::forkdb", "Reverted snapshot {}", id);
            true
        } else {
            warn!(target: "backend::forkdb", "No snapshot to revert for {}", id);
            false
        }
    }
}

impl Database for ForkDb {
    type Error = DatabaseError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        // Note: this will always return Some, since the `SharedBackend` will always load the
        // account, this differs from `<CacheDB as Database>::basic`, See also
        // [MemDb::ensure_loaded](crate::backend::MemDb::ensure_loaded)
        Database::basic(&mut self.cache_db, address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Database::code_by_hash(&mut self.cache_db, code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        Database::storage(&mut self.cache_db, address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        Database::block_hash(&mut self.cache_db, number)
    }
}

impl DatabaseRef for ForkDb {
    type Error = DatabaseError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.cache_db.basic_ref(address)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.cache_db.code_by_hash_ref(code_hash)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        DatabaseRef::storage_ref(&self.cache_db, address, index)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.cache_db.block_hash_ref(number)
    }
}

impl DatabaseCommit for ForkDb {
    fn commit(&mut self, changes: HashMap<Address, Account>) {
        self.database_mut().commit(changes)
    }
}
