use std::{
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
    Address, Bytes, SealedBlockWithSenders, B256, U256,
};
use reth_provider::{bundle_state::StorageRevertsIter, OriginalValuesKnown, ProviderError};
use rusqlite::Connection;

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
            CREATE TABLE IF NOT EXISTS bytecode (
                id   INTEGER PRIMARY KEY,
                hash TEXT UNIQUE,
                data TEXT
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

        for PlainStorageChangeset { address, wipe_storage, storage } in changeset.storage {
            if wipe_storage {
                tx.execute("DELETE FROM storage WHERE address = ?", (address.to_string(),))?;
            }

            for (key, value) in storage {
                tx.execute(
                    "INSERT INTO storage (address, key, data) VALUES (?, ?, ?) ON CONFLICT(address, key) DO UPDATE SET data = excluded.data",
                    (address.to_string(), key.to_string(), value.to_string()),
                )?;
            }
        }

        for (hash, bytecode) in changeset.contracts {
            tx.execute(
                "INSERT INTO bytecode (hash, data) VALUES (?, ?) ON CONFLICT(hash) DO UPDATE SET data = excluded.data",
                (hash.to_string(), bytecode.bytes().to_string()),
            )?;
        }

        for (address, account) in
            reverts.accounts.first().ok_or(eyre::eyre!("no account reverts"))?
        {
            tx.execute(
                "INSERT INTO account_revert (block_number, address, data) VALUES (?, ?, ?) ON CONFLICT(block_number, address) DO UPDATE SET data = excluded.data",
                (block.header.number.to_string(), address.to_string(), serde_json::to_string(account)?),
            )?;
        }

        for storage_reverts in reverts.storage {
            for PlainStorageRevert { address, wiped, storage_revert } in storage_reverts {
                let storage = storage_revert
                    .into_iter()
                    .map(|(k, v)| (B256::new(k.to_be_bytes()), v))
                    .collect::<Vec<_>>();
                let wiped_storage = if wiped { get_storages(&tx, address)? } else { Vec::new() };
                for (key, value) in StorageRevertsIter::new(storage, wiped_storage) {
                    tx.execute(
                    "INSERT INTO storage_revert (block_number, address, key, data) VALUES (?, ?, ?, ?) ON CONFLICT(block_number, address, key) DO UPDATE SET data = excluded.data",
                    (block.header.number.to_string(), address.to_string(), key.to_string(), value.to_string()),
                )?;
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
        let mut connection = self.connection();
        let tx = connection.transaction()?;

        let account = get_account(&tx, address)?;
        let account = f(account)?;
        tx.execute(
            "INSERT INTO account (address, data) VALUES (?, ?) ON CONFLICT(address) DO UPDATE SET data = excluded.data",
            (address.to_string(), serde_json::to_string(&account)?),
        )?;
        tx.commit()?;

        Ok(())
    }

    /// Get account by address.
    pub fn get_account(&mut self, address: Address) -> eyre::Result<Option<AccountInfo>> {
        get_account(&self.connection(), address)
    }
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

impl reth::revm::Database for Database {
    type Error = ProviderError;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.get_account(address).map_err(|_| ProviderError::UnsupportedProvider)
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
            Err(_) => Err(ProviderError::UnsupportedProvider),
        }
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let storage = self.connection().query_row::<String, _, _>(
            "SELECT data FROM storage WHERE address = ? AND index = ?",
            (address.to_string(), index.to_string()),
            |row| row.get(0),
        );
        match storage {
            Ok(data) => Ok(U256::from_str(&data).unwrap()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(U256::ZERO),
            Err(_) => Err(ProviderError::UnsupportedProvider),
        }
    }

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        let block_hash = self.connection().query_row::<String, _, _>(
            "SELECT hash FROM block WHERE number = ?",
            (number.to_string(),),
            |row| row.get(0),
        );
        match block_hash {
            Ok(data) => Ok(B256::from_str(&data).unwrap()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(B256::default()),
            Err(_) => Err(ProviderError::UnsupportedProvider),
        }
    }
}
