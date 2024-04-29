use std::{
    collections::{hash_map::Entry, HashMap},
    ops::Deref,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

use reth::revm::db::{
    states::{PlainStorageChangeset, PlainStorageRevert},
    BundleState,
};
use reth_primitives::{
    revm_primitives::{AccountInfo, Bytecode},
    Address, Bytes, SealedBlockWithSenders, StorageEntry, B256, U256,
};
use reth_provider::{bundle_state::StorageRevertsIter, OriginalValuesKnown, ProviderError};
use reth_tracing::tracing::error;
use rusqlite::Connection;

/// Type used to initialize revms bundle state.
type BundleStateInit =
    HashMap<Address, (Option<AccountInfo>, Option<AccountInfo>, HashMap<B256, (U256, U256)>)>;

/// Types used inside RevertsInit to initialize revms reverts.
pub type AccountRevertInit = (Option<Option<AccountInfo>>, Vec<StorageEntry>);

/// Type used to initialize revms reverts.
pub type RevertsInit = HashMap<Address, AccountRevertInit>;

pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    /// Create new database with the provided connection.
    pub fn new(connection: Connection) -> eyre::Result<Self> {
        let database = Self { connection: Arc::new(Mutex::new(connection)) };
        database.create_tables()?;
        Ok(database)
    }

    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().expect("failed to acquire database lock")
    }

    fn create_tables(&self) -> eyre::Result<()> {
        self.connection().execute_batch(
            "CREATE TABLE IF NOT EXISTS block (
                id     INTEGER PRIMARY KEY,
                number TEXT UNIQUE,
                data   TEXT
            );
            CREATE TABLE IF NOT EXISTS account (
                id      INTEGER PRIMARY KEY,
                address TEXT UNIQUE,
                data    TEXT
            );
            CREATE TABLE IF NOT EXISTS account_revert (
                id           INTEGER PRIMARY KEY,
                block_number TEXT,
                address      TEXT,
                data         TEXT,
                UNIQUE (block_number, address)
            );
            CREATE TABLE IF NOT EXISTS storage (
                id      INTEGER PRIMARY KEY,
                address TEXT,
                key     TEXT,
                data    TEXT,
                UNIQUE (address, key)
            );
            CREATE TABLE IF NOT EXISTS storage_revert (
                id           INTEGER PRIMARY KEY,
                block_number TEXT,
                address      TEXT,
                key          TEXT,
                data         TEXT,
                UNIQUE (block_number, address, key)
            );
            CREATE TABLE IF NOT EXISTS bytecode (
                id   INTEGER PRIMARY KEY,
                hash TEXT UNIQUE,
                data TEXT
            );",
        )?;
        Ok(())
    }

    /// Insert block with bundle into the database.
    /// Populates the block, account, storage, and bytecode tables.
    pub fn insert_block_with_bundle(
        &mut self,
        block: &SealedBlockWithSenders,
        bundle: BundleState,
    ) -> eyre::Result<()> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;

        tx.execute(
            "INSERT INTO block (number, data) VALUES (?, ?)",
            (block.header.number.to_string(), serde_json::to_string(block)?),
        )?;

        let (changeset, reverts) = bundle.into_plain_state_and_reverts(OriginalValuesKnown::Yes);

        for (address, account) in changeset.accounts {
            if let Some(account) = account {
                tx.execute(
                    "INSERT INTO account (address, data) VALUES (?, ?) ON CONFLICT(address) DO UPDATE SET data = excluded.data",
                    (address.to_string(), serde_json::to_string(&account)?),
                )?;
            } else {
                tx.execute("DELETE FROM account WHERE address = ?", (address.to_string(),))?;
            }
        }

        if reverts.accounts.len() > 1 {
            eyre::bail!("too many blocks in account reverts");
        }
        for (address, account) in
            reverts.accounts.first().ok_or(eyre::eyre!("no account reverts"))?
        {
            tx.execute(
                "INSERT INTO account_revert (block_number, address, data) VALUES (?, ?, ?) ON CONFLICT(block_number, address) DO UPDATE SET data = excluded.data",
                (block.header.number.to_string(), address.to_string(), serde_json::to_string(account)?),
            )?;
        }

        for PlainStorageChangeset { address, wipe_storage, storage } in changeset.storage {
            if wipe_storage {
                tx.execute("DELETE FROM storage WHERE address = ?", (address.to_string(),))?;
            }

            for (key, data) in storage {
                tx.execute(
                    "INSERT INTO storage (address, key, data) VALUES (?, ?, ?) ON CONFLICT(address, key) DO UPDATE SET data = excluded.data",
                    (address.to_string(), key.to_string(), data.to_string()),
                )?;
            }
        }

        if reverts.storage.len() > 1 {
            eyre::bail!("too many blocks in storage reverts");
        }
        for PlainStorageRevert { address, wiped, storage_revert } in
            reverts.storage.into_iter().next().ok_or(eyre::eyre!("no storage reverts"))?
        {
            let storage = storage_revert
                .into_iter()
                .map(|(k, v)| (B256::new(k.to_be_bytes()), v))
                .collect::<Vec<_>>();
            let wiped_storage = if wiped { get_storages(&tx, address)? } else { Vec::new() };
            for (key, data) in StorageRevertsIter::new(storage, wiped_storage) {
                tx.execute(
                    "INSERT INTO storage_revert (block_number, address, key, data) VALUES (?, ?, ?, ?) ON CONFLICT(block_number, address, key) DO UPDATE SET data = excluded.data",
                    (block.header.number.to_string(), address.to_string(), key.to_string(), data.to_string()),
                )?;
            }
        }

        for (hash, bytecode) in changeset.contracts {
            tx.execute(
                "INSERT INTO bytecode (hash, data) VALUES (?, ?) ON CONFLICT(hash) DO NOTHING",
                (hash.to_string(), bytecode.bytes().to_string()),
            )?;
        }

        tx.commit()?;

        Ok(())
    }

    #[allow(unreachable_code)]
    pub fn revert_block(&mut self, number: u64) -> eyre::Result<()> {
        let mut connection = self.connection();
        let tx = connection.transaction()?;

        tx.execute("DELETE FROM block WHERE number = ?", (number.to_string(),))?;

        let mut state = BundleStateInit::new();
        let mut reverts = RevertsInit::new();

        let account_reverts = tx
            .prepare("SELECT address, data FROM account_revert WHERE block_number = ?")?
            .query((number.to_string(),))?
            .mapped(|row| {
                Ok((
                    Address::from_str(row.get_ref(0)?.as_str()?),
                    serde_json::from_str::<Option<AccountInfo>>(row.get_ref(1)?.as_str()?),
                ))
            })
            .map(|result| {
                let (address, data) = result?;
                Ok((address?, data?))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        for (address, old_info) in account_reverts {
            // insert old info into reverts
            reverts.entry(address).or_default().0 = Some(old_info.clone());

            match state.entry(address) {
                Entry::Vacant(entry) => {
                    let new_info = get_account(&tx, address)?;
                    entry.insert((old_info, new_info, HashMap::new()));
                }
                Entry::Occupied(mut entry) => {
                    // overwrite old account state
                    entry.get_mut().0 = old_info;
                }
            }
        }

        let storage_reverts = tx
            .prepare("SELECT address, key, data FROM storage_revert WHERE block_number = ?")?
            .query((number.to_string(),))?
            .mapped(|row| {
                Ok((
                    Address::from_str(row.get_ref(0)?.as_str()?),
                    B256::from_str(row.get_ref(1)?.as_str()?),
                    U256::from_str(row.get_ref(2)?.as_str()?),
                ))
            })
            .map(|result| {
                let (address, key, data) = result?;
                Ok((address?, key?, data?))
            })
            .collect::<eyre::Result<Vec<_>>>()?;

        for (address, key, old_data) in storage_reverts.into_iter().rev() {
            let old_storage = StorageEntry { key, value: old_data };

            // insert old info into reverts
            reverts.entry(address).or_default().1.push(old_storage);

            // get account state or insert from plain state
            let account_state = match state.entry(address) {
                Entry::Vacant(entry) => {
                    let present_info = get_account(&tx, address)?;
                    entry.insert((present_info.clone(), present_info, HashMap::new()))
                }
                Entry::Occupied(entry) => entry.into_mut(),
            };

            // match storage
            match account_state.2.entry(old_storage.key) {
                Entry::Vacant(entry) => {
                    let new_value = get_storage(&tx, address, old_storage.key)?.unwrap_or_default();
                    entry.insert((old_storage.value, new_value));
                }
                Entry::Occupied(mut entry) => {
                    entry.get_mut().0 = old_storage.value;
                }
            };
        }

        // iterate over local plain state remove all account and all storages
        for (address, (old_account, new_account, storage)) in state {
            // revert account if needed
            if old_account != new_account {
                if let Some(account) = old_account {
                    upsert_account(&tx, address, |_| Ok(account))?;
                } else {
                    delete_account(&tx, address)?;
                }
            }

            // revert storages
            for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                // delete previous value
                delete_storage(&tx, address, storage_key)?;

                // insert value if needed
                if !old_storage_value.is_zero() {
                    upsert_storage(&tx, address, storage_key, old_storage_value)?;
                }
            }
        }

        tx.commit()?;

        Ok(())
    }

    /// Get block by number.
    pub fn get_block(&mut self, number: U256) -> eyre::Result<Option<SealedBlockWithSenders>> {
        let block = self.connection().query_row::<String, _, _>(
            "SELECT data FROM block WHERE number = ?",
            (number.to_string(),),
            |row| row.get(0),
        );
        match block {
            Ok(data) => Ok(Some(serde_json::from_str(&data)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Insert new account if it does not exist, update otherwise. The provided closure is called
    /// with the current account, if it exists.
    pub fn upsert_account(
        &mut self,
        address: Address,
        f: impl FnOnce(Option<AccountInfo>) -> eyre::Result<AccountInfo>,
    ) -> eyre::Result<()> {
        upsert_account(&self.connection(), address, f)
    }

    /// Get account by address.
    pub fn get_account(&mut self, address: Address) -> eyre::Result<Option<AccountInfo>> {
        get_account(&self.connection(), address)
    }
}

/// Insert new account if it does not exist, update otherwise. The provided closure is called
/// with the current account, if it exists. Connection can be either
/// [rusqlite::Transaction] or [rusqlite::Connection].
pub fn upsert_account<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
    f: impl FnOnce(Option<AccountInfo>) -> eyre::Result<AccountInfo>,
) -> eyre::Result<()> {
    let account = get_account(connection, address)?;
    let account = f(account)?;
    connection.execute(
        "INSERT INTO account (address, data) VALUES (?, ?) ON CONFLICT(address) DO UPDATE SET data = excluded.data",
        (address.to_string(), serde_json::to_string(&account)?),
    )?;

    Ok(())
}

/// Delete account by address. Connection can be either [rusqlite::Transaction] or
/// [rusqlite::Connection].
pub fn delete_account<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
) -> eyre::Result<()> {
    connection.execute("DELETE FROM account WHERE address = ?", (address.to_string(),))?;
    Ok(())
}

/// Get account by address using the database connection. Connection can be either
/// [rusqlite::Transaction] or [rusqlite::Connection].
fn get_account<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
) -> eyre::Result<Option<AccountInfo>> {
    match connection.deref().query_row::<String, _, _>(
        "SELECT data FROM account WHERE address = ?",
        (address.to_string(),),
        |row| row.get(0),
    ) {
        Ok(account_info) => Ok(Some(serde_json::from_str(&account_info)?)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Insert new storage if it does not exist, update otherwise. Connection can be either
/// [rusqlite::Transaction] or [rusqlite::Connection].
pub fn upsert_storage<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
    key: B256,
    data: U256,
) -> eyre::Result<()> {
    connection.execute(
        "INSERT INTO storage (address, key, data) VALUES (?, ?, ?) ON CONFLICT(address, key) DO UPDATE SET data = excluded.data",
        (address.to_string(), key.to_string(), data.to_string()),
    )?;
    Ok(())
}

/// Delete storage by address and key. Connection can be either [rusqlite::Transaction] or
/// [rusqlite::Connection].
pub fn delete_storage<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
    key: B256,
) -> eyre::Result<()> {
    connection.execute(
        "DELETE FROM storage WHERE address = ? AND key = ?",
        (address.to_string(), key.to_string()),
    )?;
    Ok(())
}

/// Get all storages for the provided address using the database connection. Connection can be
/// either [rusqlite::Transaction] or [rusqlite::Connection].
fn get_storages<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
) -> eyre::Result<Vec<(B256, U256)>> {
    connection
        .prepare("SELECT key, data FROM storage WHERE address = ?")?
        .query((address.to_string(),))?
        .mapped(|row| {
            Ok((
                B256::from_str(row.get_ref(0)?.as_str()?),
                U256::from_str(row.get_ref(1)?.as_str()?),
            ))
        })
        .map(|result| {
            let (key, data) = result?;
            Ok((key?, data?))
        })
        .collect()
}

/// Get storage for the provided address by key using the database connection. Connection can be
/// either [rusqlite::Transaction] or [rusqlite::Connection].
fn get_storage<C: Deref<Target = Connection>>(
    connection: &C,
    address: Address,
    key: B256,
) -> eyre::Result<Option<U256>> {
    match connection.deref().query_row::<String, _, _>(
        "SELECT data FROM storage WHERE address = ? AND key = ?",
        (address.to_string(), key.to_string()),
        |row| row.get(0),
    ) {
        Ok(data) => Ok(Some(U256::from_str(&data)?)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl reth::revm::Database for Database {
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.get_account(address).map_err(|err| {
            error!(%err, %address, "Failed to get account by address");
            ProviderError::UnsupportedProvider
        })
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let bytecode = self.connection().query_row::<String, _, _>(
            "SELECT data FROM bytecode WHERE hash = ?",
            (code_hash.to_string(),),
            |row| row.get(0),
        );
        match bytecode {
            Ok(data) => Ok(Bytecode::new_raw(Bytes::from_str(&data).unwrap())),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Bytecode::default()),
            Err(err) => {
                error!(%err, %code_hash, "Failed to get bytecode by hash");
                Err(ProviderError::UnsupportedProvider)
            }
        }
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        get_storage(&self.connection(), address, index.into())
            .map(|data| data.unwrap_or_default())
            .map_err(|err| {
                error!(%err, %address, "Failed to get account by address");
                ProviderError::UnsupportedProvider
            })
    }

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        let block_hash = self.connection().query_row::<String, _, _>(
            "SELECT hash FROM block WHERE number = ?",
            (number.to_string(),),
            |row| row.get(0),
        );
        match block_hash {
            Ok(data) => Ok(B256::from_str(&data).unwrap()),
            // No special handling for `QueryReturnedNoRows` is needed, because revm does block
            // number bound checks on its own.
            // See https://github.com/bluealloy/revm/blob/1ca3d39f6a9e9778f8eb0fcb74fe529345a531b4/crates/interpreter/src/instructions/host.rs#L106-L123.
            Err(err) => {
                error!(%err, %number, "Failed to get block hash by number");
                Err(ProviderError::UnsupportedProvider)
            }
        }
    }
}
