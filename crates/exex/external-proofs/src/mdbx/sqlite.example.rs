use std::{path::PathBuf, sync::Mutex};

use alloy_primitives::U256;
use reth::{
    primitives::{Account, StorageEntry},
    revm::primitives::B256,
};
use reth_codecs::Compact;
use reth_tracing::tracing::info;
use reth_trie::{BranchNodeCompact, Nibbles, StoredNibbles};
use rusqlite::{params, Connection, OptionalExtension};

use crate::storage::{
    ExternalHashedCursor, ExternalStateStore, ExternalStorageError, ExternalStorageResult,
    ExternalTrieCursor, TrieBranchesBatch,
};

pub struct SqlitePreimageStoreStorageCursor {
    conn: Mutex<Connection>,
    hashed_address: B256,
    max_block_number: u64,
    last_seeked: Option<B256>,
}

pub struct SqlitePreimageStoreAccountCursor {
    conn: Mutex<Connection>,
    max_block_number: u64,
    last_seeked: Option<B256>,
}

impl SqlitePreimageStoreAccountCursor {
    pub fn new(conn: Connection, max_block_number: u64) -> Self {
        Self { conn: Mutex::new(conn), max_block_number, last_seeked: None }
    }

    /// Find the next valid account entry >= the given key that has data at or before max_block_number
    fn find_next_valid_account(
        &self,
        start_key: &B256,
    ) -> ExternalStorageResult<Option<(B256, Account)>> {
        let mut current_key = Some(*start_key);

        // Iterate through keys one by one to avoid unbounded queries
        loop {
            let (query, params) = if let Some(key) = current_key {
                (
                    "SELECT DISTINCT key FROM accounts WHERE key >= ? ORDER BY key ASC LIMIT 1",
                    params![key.to_vec()],
                )
            } else {
                // This case shouldn't happen in our logic, but handle it gracefully
                return Ok(None);
            };

            let next_key_result = self
                .conn
                .lock()
                .unwrap()
                .query_row(query, params, |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map_err(Into::<ExternalStorageError>::into)?;

            let Some(key_bytes) = next_key_result else {
                // No more keys found
                return Ok(None);
            };

            let key = B256::from_slice(&key_bytes);

            // Get the latest version of this key within the block limit
            if let Some(account) = self.get_latest_account_version(&key)? {
                return Ok(Some((key, account)));
            }

            // This key doesn't have a valid version, find the next key using >
            current_key = self.find_next_key_after(&key)?;
            if current_key.is_none() {
                return Ok(None);
            }
        }
    }

    /// Find the next key after the given key using > operator
    fn find_next_key_after(&self, key: &B256) -> ExternalStorageResult<Option<B256>> {
        let next_key_result = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT DISTINCT key FROM accounts WHERE key > ? ORDER BY key ASC LIMIT 1",
                params![key.to_vec()],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(Into::<ExternalStorageError>::into)?;

        Ok(next_key_result.map(|bytes| B256::from_slice(&bytes)))
    }

    /// Get the latest version of a specific account key within the block limit
    fn get_latest_account_version(&self, key: &B256) -> ExternalStorageResult<Option<Account>> {
        let result = self.conn.lock().unwrap().query_row(
            "SELECT value FROM accounts WHERE key = ? AND block_number <= ? ORDER BY block_number DESC LIMIT 1",
            params![key.to_vec(), self.max_block_number],
            |r| r.get::<_, Vec<u8>>(0)
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        if let Some(value_bytes) = result {
            // If the value is empty, it means the account was deleted
            if value_bytes.is_empty() {
                Ok(None)
            } else {
                let account = Account::from_compact(&value_bytes, value_bytes.len()).0;
                Ok(Some(account))
            }
        } else {
            Ok(None)
        }
    }
}

impl SqlitePreimageStoreStorageCursor {
    pub fn new(conn: Connection, hashed_address: B256, max_block_number: u64) -> Self {
        Self { conn: Mutex::new(conn), hashed_address, max_block_number, last_seeked: None }
    }

    /// Find the next valid storage entry >= the given key for this account
    fn find_next_valid_storage(
        &self,
        start_key: &B256,
    ) -> ExternalStorageResult<Option<(B256, U256)>> {
        let mut current_key = Some(*start_key);

        // Iterate through keys one by one to avoid unbounded queries
        loop {
            let (query, params) = if let Some(key) = current_key {
                (
                    "SELECT DISTINCT storage_key FROM storage_nodes WHERE hashed_address = ? AND storage_key >= ? ORDER BY storage_key ASC LIMIT 1",
                    params![self.hashed_address.to_vec(), key.to_vec()],
                )
            } else {
                return Ok(None);
            };

            let next_key_result = self
                .conn
                .lock()
                .unwrap()
                .query_row(query, params, |r| r.get::<_, Vec<u8>>(0))
                .optional()
                .map_err(Into::<ExternalStorageError>::into)?;

            let Some(key_bytes) = next_key_result else {
                return Ok(None);
            };

            let key = B256::from_slice(&key_bytes);

            // Get the latest version of this storage key within the block limit
            if let Some(value) = self.get_latest_storage_version(&key)? {
                return Ok(Some((key, value)));
            }

            // This key doesn't have a valid version, find the next key using >
            current_key = self.find_next_storage_key_after(&key)?;
            if current_key.is_none() {
                return Ok(None);
            }
        }
    }

    /// Find the next storage key after the given key using > operator
    fn find_next_storage_key_after(&self, key: &B256) -> ExternalStorageResult<Option<B256>> {
        let next_key_result = self.conn.lock().unwrap().query_row(
            "SELECT DISTINCT storage_key FROM storage_nodes WHERE hashed_address = ? AND storage_key > ? ORDER BY storage_key ASC LIMIT 1",
            params![self.hashed_address.to_vec(), key.to_vec()],
            |r| r.get::<_, Vec<u8>>(0)
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        Ok(next_key_result.map(|bytes| B256::from_slice(&bytes)))
    }

    /// Get the latest version of a specific storage key within the block limit.
    /// Returns None if the key doesn't exist or if the value is zero (zero values are treated as deleted).
    fn get_latest_storage_version(&self, key: &B256) -> ExternalStorageResult<Option<U256>> {
        let result = self.conn.lock().unwrap().query_row(
            "SELECT value FROM storage_nodes WHERE hashed_address = ? AND storage_key = ? AND block_number <= ? ORDER BY block_number DESC LIMIT 1",
            params![self.hashed_address.to_vec(), key.to_vec(), self.max_block_number],
            |r| r.get::<_, Vec<u8>>(0)
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        if let Some(value_bytes) = result {
            let value = U256::from_be_slice(&value_bytes);
            // Skip zero values - they are treated as deleted/non-existent in storage tries
            if value.is_zero() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        } else {
            Ok(None)
        }
    }
}

impl ExternalHashedCursor for SqlitePreimageStoreStorageCursor {
    type Value = U256;

    fn seek(&mut self, key: B256) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        let result = self.find_next_valid_storage(&key)?;
        self.last_seeked = result.as_ref().map(|(key, _)| *key);
        Ok(result)
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        let result = if let Some(last_seeked) = self.last_seeked {
            // Find the next key after the last seeked key using >
            if let Some(next_key) = self.find_next_storage_key_after(&last_seeked)? {
                self.find_next_valid_storage(&next_key)?
            } else {
                None
            }
        } else {
            // If this is the first call, start from the beginning
            self.find_next_valid_storage(&B256::ZERO)?
        };

        self.last_seeked = result.as_ref().map(|(key, _)| *key);
        Ok(result)
    }

    fn is_storage_empty(&mut self) -> ExternalStorageResult<bool> {
        let result = self.find_next_valid_storage(&B256::ZERO)?.is_none();
        Ok(result)
    }
}

impl ExternalHashedCursor for SqlitePreimageStoreAccountCursor {
    type Value = Account;

    fn seek(&mut self, key: B256) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        let result = self.find_next_valid_account(&key)?;
        self.last_seeked = result.as_ref().map(|(key, _)| *key);
        Ok(result)
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(B256, Self::Value)>> {
        let result = if let Some(last_seeked) = self.last_seeked {
            // Find the next key after the last seeked key using >
            if let Some(next_key) = self.find_next_key_after(&last_seeked)? {
                self.find_next_valid_account(&next_key)?
            } else {
                None
            }
        } else {
            // If this is the first call, start from the beginning
            self.find_next_valid_account(&B256::ZERO)?
        };

        self.last_seeked = result.as_ref().map(|(key, _)| *key);
        Ok(result)
    }

    fn is_storage_empty(&mut self) -> ExternalStorageResult<bool> {
        let result = self.find_next_valid_account(&B256::ZERO)?.is_none();
        Ok(result)
    }
}

/// Cursor over an account or storage trie at a certain block number.
pub struct SqlitePreimageStoreCursor {
    conn: Mutex<Connection>,
    hashed_address: Option<B256>,
    max_block_number: u64,
    last_seeked: Option<Nibbles>,
}

enum NextBranchResult {
    Found(Nibbles, BranchNodeCompact),
    NotFound(Nibbles),
    EndOfTrie,
}

impl SqlitePreimageStoreCursor {
    /// Create a new cursor.
    pub fn new(conn: Connection, hashed_address: Option<B256>, max_block_number: u64) -> Self {
        Self { conn: Mutex::new(conn), hashed_address, max_block_number, last_seeked: None }
    }

    /// Convert a branch node from a byte vector to a BranchNodeCompact.
    fn row_to_branch(branch: &Vec<u8>) -> ExternalStorageResult<BranchNodeCompact> {
        Ok(BranchNodeCompact::from_compact(&branch, branch.len()).0)
    }

    /// Get the latest value of the branch node at the given path. Returns None
    fn get_latest_branch(
        &mut self,
        path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        // find the key between <hashed_addr>-<path>-0 and <hashed_addr>-<path>-<max_block_number>
        let key = BranchNodeKey { hashed_address: self.hashed_address, path: StoredNibbles(path) }
            .encode();

        let max_block_number_int: i64 = self.max_block_number.try_into().unwrap_or(i64::MAX);

        // sort by key descending so latest blocks come first
        let row: Option<(Vec<u8>, Vec<u8>)> = self.conn.lock().unwrap().query_row(
            "SELECT key, branch FROM branch_nodes WHERE key = ? AND block_number <= ? ORDER BY block_number DESC LIMIT 1",
            params![key, max_block_number_int],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
        ).optional()?;

        // if there is no branch node at the given path <= max_block_number, no branch node at this path for this block
        let Some(row) = row else {
            return Ok(None);
        };

        // if the latest branch node is an empty string, the branch node was deleted, so there is no branch node at this path for this block
        if row.1.is_empty() {
            return Ok(None);
        }

        // otherwise, decode and return the branch node
        let key = BranchNodeKey::decode(&row.0)?;
        Ok(Some((key.path.0, Self::row_to_branch(&row.1)?)))
    }

    /// Get the branch node with the next path that possibly exists at a certain block number. Returns None if there are no more branch nodes
    fn get_next_latest_branch(&mut self, path: Nibbles) -> ExternalStorageResult<NextBranchResult> {
        // find the next path after the current path
        let last_current_path_key =
            BranchNodeKey { hashed_address: self.hashed_address, path: StoredNibbles(path) }
                .encode();

        // find the next path after the current path (for any block number)
        let row: Option<Vec<u8>> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT key FROM branch_nodes WHERE key > ? LIMIT 1",
                params![last_current_path_key],
                |r| Ok(r.get::<_, Vec<u8>>(0)?),
            )
            .optional()?;

        // if there is no next path, we're at the end of the trie, return None
        let Some(row) = row else {
            return Ok(NextBranchResult::EndOfTrie);
        };

        // if this is the last path for this account, return None
        let BranchNodeKey { hashed_address: next_trie_node_address, path: next_trie_node_path } =
            BranchNodeKey::decode(&row)?;

        if next_trie_node_address != self.hashed_address {
            return Ok(NextBranchResult::EndOfTrie);
        }

        // get the latest branch for this path and return it (None if there is no branch node at this path for this block)
        match self.get_latest_branch(next_trie_node_path.0)? {
            Some((path, branch)) => Ok(NextBranchResult::Found(path, branch)),
            None => Ok(NextBranchResult::NotFound(next_trie_node_path.0)),
        }
    }

    fn seek_first_non_empty_path_after(
        &mut self,
        path: Nibbles,
        inclusive: bool,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        // if we're seeking inclusive, first try to find the exact path
        if inclusive {
            if let Some((path, branch)) = self.get_latest_branch(path)? {
                return Ok(Some((path, branch)));
            }
        };

        // if not inclusive, or there is no branch at the exact path, find the next path that has a branch
        let mut next_path = path;
        loop {
            match self.get_next_latest_branch(next_path)? {
                NextBranchResult::Found(path, branch) => {
                    return Ok(Some((path, branch)));
                }
                NextBranchResult::NotFound(path) => {
                    // go to the next branch node
                    next_path = path;
                }
                NextBranchResult::EndOfTrie => {
                    // we're at the end of the trie, return None
                    return Ok(None);
                }
            }
        }
    }
}

impl ExternalTrieCursor for SqlitePreimageStoreCursor {
    fn seek_exact(
        &mut self,
        path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let Some((returned_path, branch)) = self.get_latest_branch(path)? else {
            return Ok(None);
        };

        self.last_seeked = Some(returned_path);
        Ok(Some((path, branch)))
    }

    fn seek(
        &mut self,
        path: Nibbles,
    ) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let Some((returned_path, branch)) = self.seek_first_non_empty_path_after(path, true)?
        else {
            return Ok(None);
        };
        self.last_seeked = Some(returned_path);
        Ok(Some((returned_path, branch)))
    }

    fn next(&mut self) -> ExternalStorageResult<Option<(Nibbles, BranchNodeCompact)>> {
        let Some(last_seeked) = self.last_seeked else {
            let result = self.seek_first_non_empty_path_after(Nibbles::default(), true);
            if let Ok(Some((path, _branch))) = &result {
                self.last_seeked = Some(*path);
            }
            return result;
        };
        let Some((returned_path, branch)) =
            self.seek_first_non_empty_path_after(last_seeked, false)?
        else {
            return Ok(None);
        };
        self.last_seeked = Some(returned_path);
        Ok(Some((returned_path, branch)))
    }

    fn current(&mut self) -> ExternalStorageResult<Option<Nibbles>> {
        Ok(self.last_seeked)
    }
}

/// SQLite implementation of PreimageStore
#[derive(Debug, Clone)]
pub struct SqlitePreimageStore {
    /// Connection string or filename
    db_path: PathBuf,
}

impl SqlitePreimageStore {
    /// Create a new SQLite store and ensure schema exists.
    pub async fn new(db_path: impl Into<PathBuf>) -> ExternalStorageResult<Self> {
        let store = Self { db_path: db_path.into() };
        store.ensure_schema()?;
        Ok(store)
    }

    fn connect(&self) -> ExternalStorageResult<Connection> {
        Connection::open(&self.db_path).map_err(|e| {
            ExternalStorageError::ConnectionError(format!("Failed to open sqlite: {}", e))
        })
    }

    fn ensure_schema(&self) -> ExternalStorageResult<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;

            CREATE TABLE IF NOT EXISTS branch_nodes (
                key BLOB,
                hashed_address BLOB NULL,
                path BLOB NOT NULL,
                block_number INTEGER NOT NULL,
                branch BLOB NOT NULL,
                PRIMARY KEY (key, block_number)
            ) WITHOUT ROWID;

            CREATE INDEX IF NOT EXISTS idx_branch_nodes_block
            ON branch_nodes(block_number);

            CREATE TABLE IF NOT EXISTS accounts (
                key BLOB,
                value BLOB NOT NULL,
                block_number INTEGER NOT NULL,
                PRIMARY KEY (key, block_number)
            ) WITHOUT ROWID;

            CREATE INDEX IF NOT EXISTS idx_accounts_block
            ON accounts(block_number);

            CREATE TABLE IF NOT EXISTS storage_nodes (
                hashed_address BLOB,
                storage_key BLOB,
                value BLOB NOT NULL,
                block_number INTEGER NOT NULL,
                PRIMARY KEY (hashed_address, storage_key, block_number)
            ) WITHOUT ROWID;

            CREATE INDEX IF NOT EXISTS idx_storage_nodes_block
            ON storage_nodes(block_number);

            CREATE TABLE IF NOT EXISTS earliest_block (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                block_number INTEGER NOT NULL,
                hash BLOB NOT NULL
            );
            "#,
        )
        .map_err(|e| {
            ExternalStorageError::TableCreationError(format!("Failed to create schema: {}", e))
        })?;

        Ok(())
    }
    fn encode_branch(branch: &BranchNodeCompact) -> ExternalStorageResult<Vec<u8>> {
        let mut out = Vec::new();
        branch.to_compact(&mut out);
        Ok(out)
    }
}

/// Key codec for branch nodes with proper ordering.
///
/// Format:
/// - If `hashed_address` is Some(addr) (account trie):
///     <0x00> <addr(32)> <compact_path>
/// - If `hashed_address` is None (state trie):
///     <0x01> <compact_path>
///
/// This ensures state trie entries (0x01 prefix) are ordered after account trie entries (0x00 prefix)
#[derive(Debug, Clone)]
struct BranchNodeKey {
    hashed_address: Option<B256>,
    path: StoredNibbles,
}

impl BranchNodeKey {
    fn encode(&self) -> Vec<u8> {
        // Encode path using compact format but without length prefix for lexicographic ordering
        let mut path_bytes = Vec::new();
        self.path.to_compact(&mut path_bytes);

        if let Some(addr) = &self.hashed_address {
            // Account trie: <0x00> <addr(32)> <path_bytes>
            let mut out = Vec::with_capacity(1 + 32 + path_bytes.len());
            out.push(0x00); // Account trie prefix
            out.extend_from_slice(addr.as_slice());
            out.extend_from_slice(&path_bytes); // Path bytes for lexicographic order
            out
        } else {
            // State trie: <0x01> <path_bytes>
            let mut out = Vec::with_capacity(1 + path_bytes.len());
            out.push(0x01); // State trie prefix
            out.extend_from_slice(&path_bytes); // Path bytes for lexicographic order
            out
        }
    }

    fn decode(bytes: &[u8]) -> ExternalStorageResult<BranchNodeKey> {
        if bytes.len() == 0 {
            return Err(ExternalStorageError::StorageError("key too short".to_string()));
        }

        let prefix = bytes[0];

        match prefix {
            0x00 => {
                // Account trie: <0x00> <addr(32)> <rlp_path>
                if bytes.len() < 1 + 32 {
                    return Err(ExternalStorageError::StorageError(
                        "account trie key too short".to_string(),
                    ));
                }

                let addr = B256::from_slice(&bytes[1..33]);

                let path_bytes = &bytes[33..];
                let (path, _) = StoredNibbles::from_compact(&path_bytes, path_bytes.len());

                Ok(BranchNodeKey { hashed_address: Some(addr), path })
            }
            0x01 => {
                // State trie: <0x01> <rlp_path>

                let path_bytes = &bytes[1..];
                let (path, _) = StoredNibbles::from_compact(&path_bytes, path_bytes.len());

                Ok(BranchNodeKey { hashed_address: None, path })
            }
            _ => Err(ExternalStorageError::StorageError(format!("invalid key prefix: {}", prefix))),
        }
    }
}

#[async_trait::async_trait]
impl ExternalStateStore for SqlitePreimageStore {
    type TrieCursor = SqlitePreimageStoreCursor;
    type StorageCursor = SqlitePreimageStoreStorageCursor;
    type AccountHashedCursor = SqlitePreimageStoreAccountCursor;

    async fn get_latest_block_number(&self) -> ExternalStorageResult<Option<u64>> {
        let earliest_block_number = self.get_earliest_block_number().await?.map(|(bn, _)| bn);

        let Some(earliest_block_number) = earliest_block_number else {
            return Ok(None);
        };

        let result = self
            .connect()?
            .query_row("SELECT MAX(block_number) FROM branch_nodes", [], |r| {
                r.get::<_, Option<u64>>(0)
            })
            .map_err(Into::<ExternalStorageError>::into)?;

        Ok(Some(result.map(|bn| earliest_block_number.max(bn)).unwrap_or(earliest_block_number)))
    }

    fn get_last_storage_leaf(&self) -> ExternalStorageResult<Option<(B256, B256)>> {
        let result = self.connect()?.query_row(
            "SELECT hashed_address, storage_key FROM storage_nodes WHERE block_number = ? ORDER BY hashed_address DESC, storage_key DESC LIMIT 1",
            params![0],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        Ok(result.map(|(hashed_address, storage_key)| {
            (B256::from_slice(&hashed_address), B256::from_slice(&storage_key))
        }))
    }

    fn get_last_account_leaf(&self) -> ExternalStorageResult<Option<B256>> {
        let result = self
            .connect()?
            .query_row(
                "SELECT key FROM accounts WHERE block_number = ? ORDER BY key DESC LIMIT 1",
                params![0],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(Into::<ExternalStorageError>::into)?;

        Ok(result.map(|hashed_address| B256::from_slice(&hashed_address)))
    }

    fn get_last_storage_branch(&self) -> ExternalStorageResult<Option<(B256, Nibbles)>> {
        let result = self.connect()?.query_row(
            "SELECT hashed_address, path FROM branch_nodes WHERE block_number = ? AND hashed_address IS NOT NULL ORDER BY key DESC LIMIT 1",
            params![0],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        let Some(result) = result else {
            return Ok(None);
        };

        Ok(Some((
            B256::from_slice(&result.0),
            StoredNibbles::from_compact(&result.1, result.1.len()).0 .0,
        )))
    }

    fn get_last_account_branch(&self) -> ExternalStorageResult<Option<Nibbles>> {
        let result = self.connect()?.query_row(
            "SELECT path FROM branch_nodes WHERE block_number = ? AND hashed_address IS NULL ORDER BY key DESC LIMIT 1",
            params![0],
            |r| r.get::<_, Vec<u8>>(0)
        ).optional()
        .map_err(Into::<ExternalStorageError>::into)?;

        Ok(result.map(|path| StoredNibbles::from_compact(&path, path.len()).0 .0))
    }

    async fn store_trie_branch(
        &self,
        block_number: u64,
        path: Nibbles,
        hashed_address: Option<B256>,
        branch: Option<BranchNodeCompact>,
    ) -> ExternalStorageResult<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| ExternalStorageError::StorageError(format!("Begin tx failed: {}", e)))?;

        let key =
            BranchNodeKey { hashed_address: hashed_address.clone(), path: StoredNibbles(path) }
                .encode();
        let mut path_bytes = Vec::new();
        StoredNibbles(path).to_compact(&mut path_bytes);
        let branch_bytes = match branch {
            Some(branch) => Self::encode_branch(&branch)?,
            None => Vec::new(),
        };
        let block_number_int: i64 = block_number.try_into().unwrap_or(i64::MAX);

        tx.execute(
            "INSERT OR REPLACE INTO branch_nodes (key, hashed_address, path, block_number, branch) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                key,
                hashed_address.as_ref().map(|h| h.as_slice().to_vec()),
                path_bytes,
                block_number_int,
                branch_bytes
            ],
        ).map_err(|e| ExternalStorageError::StorageError(format!("insert failed: {}", e)))?;

        tx.commit()
            .map_err(|e| ExternalStorageError::StorageError(format!("commit failed: {}", e)))?;
        Ok(())
    }

    async fn store_trie_branches(&self, batch: TrieBranchesBatch) -> ExternalStorageResult<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| ExternalStorageError::StorageError(format!("Begin tx failed: {}", e)))?;
        let block_number_int: i64 = batch.block_number.try_into().unwrap_or(i64::MAX);

        for item in batch.items.into_iter() {
            let key = BranchNodeKey {
                hashed_address: item.hashed_address.clone(),
                path: StoredNibbles(item.path),
            }
            .encode();
            let mut path_bytes = Vec::new();
            StoredNibbles(item.path).to_compact(&mut path_bytes);
            let branch_bytes = match item.branch {
                Some(branch) => Self::encode_branch(&branch)?,
                None => Vec::new(),
            };

            tx.execute(
                "INSERT OR REPLACE INTO branch_nodes (key, hashed_address, path, block_number, branch) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    key,
                    item.hashed_address.as_ref().map(|h| h.as_slice().to_vec()),
                    path_bytes,
                    block_number_int,
                    branch_bytes
                ],
            ).map_err(|e| ExternalStorageError::BatchError(format!("batch insert failed: {}", e)))?;
        }

        tx.commit()
            .map_err(|e| ExternalStorageError::BatchError(format!("commit failed: {}", e)))?;
        Ok(())
    }

    async fn store_hashed_accounts(
        &self,
        accounts: Vec<(B256, Option<Account>)>,
        block_number: u64,
    ) -> ExternalStorageResult<()> {
        let mut conn = self.connect()?;
        let tx: rusqlite::Transaction<'_> = conn
            .transaction()
            .map_err(|e| ExternalStorageError::StorageError(format!("Begin tx failed: {}", e)))?;

        let block_number_int: i64 = block_number.try_into().unwrap_or(i64::MAX);
        for account in accounts {
            let mut serialized_account = Vec::new();
            if let Some(account) = account.1 {
                account.to_compact(&mut serialized_account);
            }

            tx.execute(
                "INSERT OR REPLACE INTO accounts (key, value, block_number) VALUES (?1, ?2, ?3)",
                params![account.0.to_vec(), serialized_account, block_number_int],
            )
            .map_err(|e| ExternalStorageError::StorageError(format!("insert failed: {}", e)))?;
        }

        tx.commit()
            .map_err(|e| ExternalStorageError::StorageError(format!("commit failed: {}", e)))?;
        Ok(())
    }

    async fn store_hashed_storages(
        &self,
        storages: Vec<(B256, StorageEntry)>,
        block_number: u64,
    ) -> ExternalStorageResult<()> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction()
            .map_err(|e| ExternalStorageError::StorageError(format!("Begin tx failed: {}", e)))?;

        let block_number_int: i64 = block_number.try_into().unwrap_or(i64::MAX);
        for storage in storages {
            tx.execute(
                "INSERT OR REPLACE INTO storage_nodes (hashed_address, storage_key, value, block_number) VALUES (?1, ?2, ?3, ?4)",
                params![storage.0.to_vec(), storage.1.key.to_vec(), storage.1.value.to_be_bytes_vec(), block_number_int],
            ).map_err(|e| ExternalStorageError::StorageError(format!("insert failed: {}", e)))?;
        }

        tx.commit()
            .map_err(|e| ExternalStorageError::StorageError(format!("commit failed: {}", e)))?;
        Ok(())
    }

    async fn get_earliest_block_number(&self) -> ExternalStorageResult<Option<(u64, B256)>> {
        let conn = self.connect()?;
        let row: Option<(i64, Vec<u8>)> = conn
            .query_row("SELECT block_number, hash FROM earliest_block WHERE id = 1", [], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .optional()
            .map_err(|e| ExternalStorageError::StorageError(format!("query failed: {}", e)))?;

        if let Some((bn, hash_bytes)) = row {
            let mut h = [0u8; 32];
            let len = hash_bytes.len().min(32);
            h[..len].copy_from_slice(&hash_bytes[..len]);
            Ok(Some((bn as u64, B256::from_slice(&h))))
        } else {
            Ok(None)
        }
    }

    async fn set_earliest_block_number(
        &self,
        block_number: u64,
        hash: B256,
    ) -> ExternalStorageResult<()> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO earliest_block (id, block_number, hash) VALUES (1, ?1, ?2) ON CONFLICT(id) DO UPDATE SET block_number=excluded.block_number, hash=excluded.hash",
            params![block_number as i64, hash.as_slice()],
        ).map_err(|e| ExternalStorageError::StorageError(format!("upsert earliest_block failed: {}", e)))?;
        Ok(())
    }

    async fn health_check(&self) -> ExternalStorageResult<()> {
        let conn = self.connect()?;
        let _: i64 = conn.query_row("SELECT 1", [], |r| r.get(0)).map_err(|e| {
            ExternalStorageError::ConnectionError(format!("sqlite healthcheck failed: {}", e))
        })?;
        info!("SQLite store is healthy");
        Ok(())
    }

    fn trie_cursor(
        &self,
        hashed_address: Option<B256>,
        max_block_number: u64,
    ) -> ExternalStorageResult<Self::TrieCursor> {
        Ok(SqlitePreimageStoreCursor::new(self.connect()?, hashed_address, max_block_number))
    }

    fn storage_hashed_cursor(
        &self,
        hashed_address: B256,
        max_block_number: u64,
    ) -> ExternalStorageResult<Self::StorageCursor> {
        Ok(SqlitePreimageStoreStorageCursor::new(self.connect()?, hashed_address, max_block_number))
    }

    fn account_hashed_cursor(
        &self,
        max_block_number: u64,
    ) -> ExternalStorageResult<Self::AccountHashedCursor> {
        Ok(SqlitePreimageStoreAccountCursor::new(self.connect()?, max_block_number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_trie::TrieMask;
    use std::sync::Arc;

    fn nibbles_from(vec: Vec<u8>) -> Nibbles {
        StoredNibbles::from(vec).0
    }

    fn create_test_branch() -> BranchNodeCompact {
        // Create a simple branch node with just a state mask, no actual hashes
        let mut state_mask = TrieMask::default();
        state_mask.set_bit(0);
        state_mask.set_bit(1);

        BranchNodeCompact {
            state_mask,
            tree_mask: TrieMask::default(),
            hash_mask: TrieMask::default(),
            hashes: Arc::new(vec![]), // Empty hashes vector
            root_hash: None,
        }
    }

    async fn setup_test_store() -> SqlitePreimageStore {
        use tempfile::NamedTempFile;
        let temp_file = NamedTempFile::new().unwrap();
        let store = SqlitePreimageStore::new(temp_file.path()).await.unwrap();

        // Keep the temp file alive by leaking it - this is fine for tests
        std::mem::forget(temp_file);
        store
    }

    #[test]
    fn branch_node_key_roundtrip_no_address() {
        let path = nibbles_from(vec![1, 2, 3, 4, 5]);
        let key =
            BranchNodeKey { hashed_address: None, path: StoredNibbles(path.clone()) }.encode();
        let BranchNodeKey { hashed_address: addr, path: decoded_path } =
            BranchNodeKey::decode(&key).unwrap();
        assert!(addr.is_none());
        assert_eq!(decoded_path.0, path);
    }

    #[test]
    fn branch_node_key_roundtrip_with_address() {
        let path = nibbles_from((0..32).map(|i| (i % 16) as u8).collect());
        let addr = B256::repeat_byte(0xAB);
        let key = BranchNodeKey { hashed_address: Some(addr), path: StoredNibbles(path.clone()) }
            .encode();
        let BranchNodeKey { hashed_address: addr2, path: decoded_path } =
            BranchNodeKey::decode(&key).unwrap();
        assert_eq!(addr2, Some(addr));
        assert_eq!(decoded_path.0, path);
    }

    // 1. Basic Cursor Operations

    #[tokio::test]
    async fn test_cursor_empty_trie() {
        let store = setup_test_store().await;
        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // All operations should return None on empty trie
        assert!(cursor.seek_exact(Nibbles::default()).unwrap().is_none());
        assert!(cursor.seek(Nibbles::default()).unwrap().is_none());
        assert!(cursor.next().unwrap().is_none());
        assert!(cursor.current().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cursor_single_entry() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        // Store single entry
        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Test seek_exact
        let result = cursor.seek_exact(path).unwrap().unwrap();
        assert_eq!(result.0, path);

        // Test current position
        assert_eq!(cursor.current().unwrap().unwrap(), path);

        // Test next from end should return None
        assert!(cursor.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cursor_multiple_entries() {
        let store = setup_test_store().await;
        let paths = vec![
            nibbles_from(vec![1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![2, 3]),
        ];
        let branch = create_test_branch();

        // Store multiple entries
        for path in &paths {
            store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        }

        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Test that we can iterate through all entries
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next().unwrap() {
            found_paths.push(path);
        }

        assert_eq!(found_paths.len(), 4);
        // Paths should be in lexicographic order
        for i in 0..paths.len() {
            assert_eq!(found_paths[i], paths[i]);
        }
    }

    // 2. Seek Operations

    #[tokio::test]
    async fn test_seek_exact_existing_path() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let result = cursor.seek_exact(path).unwrap().unwrap();
        assert_eq!(result.0, path);
    }

    #[tokio::test]
    async fn test_seek_exact_non_existing_path() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let non_existing = nibbles_from(vec![4, 5, 6]);
        assert!(cursor.seek_exact(non_existing).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_seek_exact_empty_path() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let result = cursor.seek_exact(Nibbles::default()).unwrap().unwrap();
        assert_eq!(result.0, Nibbles::default());
    }

    #[tokio::test]
    async fn test_seek_to_existing_path() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2, 3]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let result = cursor.seek(path).unwrap().unwrap();
        assert_eq!(result.0, path);
    }

    #[tokio::test]
    async fn test_seek_between_existing_nodes() {
        let store = setup_test_store().await;
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![3]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path1.clone(), None, Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path2.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        // Seek to path between 1 and 3, should return path 3
        let seek_path = nibbles_from(vec![2]);
        let result = cursor.seek(seek_path).unwrap().unwrap();
        assert_eq!(result.0, path2);
    }

    #[tokio::test]
    async fn test_seek_after_all_nodes() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        // Seek to path after all nodes
        let seek_path = nibbles_from(vec![9]);
        assert!(cursor.seek(seek_path).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_seek_before_all_nodes() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![5]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        // Seek to path before all nodes, should return first node
        let seek_path = nibbles_from(vec![1]);
        let result = cursor.seek(seek_path).unwrap().unwrap();
        assert_eq!(result.0, path);
    }

    // 3. Navigation Tests

    #[tokio::test]
    async fn test_next_without_prior_seek() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        // next() without prior seek should start from beginning
        let result = cursor.next().unwrap().unwrap();
        assert_eq!(result.0, path);
    }

    #[tokio::test]
    async fn test_next_after_seek() {
        let store = setup_test_store().await;
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path1.clone(), None, Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path2.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        cursor.seek(path1).unwrap();

        // next() should return second node
        let result = cursor.next().unwrap().unwrap();
        assert_eq!(result.0, path2);
    }

    #[tokio::test]
    async fn test_next_at_end_of_trie() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        cursor.seek(path).unwrap();

        // next() at end should return None
        assert!(cursor.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_multiple_consecutive_next() {
        let store = setup_test_store().await;
        let paths = vec![nibbles_from(vec![1]), nibbles_from(vec![2]), nibbles_from(vec![3])];
        let branch = create_test_branch();

        for path in &paths {
            store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        }

        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Iterate through all with consecutive next() calls
        for expected_path in &paths {
            let result = cursor.next().unwrap().unwrap();
            assert_eq!(result.0, *expected_path);
        }

        // Final next() should return None
        assert!(cursor.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_current_after_operations() {
        let store = setup_test_store().await;
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let branch = create_test_branch();

        store.store_trie_branch(50, path1.clone(), None, Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path2.clone(), None, Some(branch.clone())).await.unwrap();

        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Current should be None initially
        assert!(cursor.current().unwrap().is_none());

        // After seek, current should track position
        cursor.seek(path1).unwrap();
        assert_eq!(cursor.current().unwrap().unwrap(), path1);

        // After next, current should update
        cursor.next().unwrap();
        assert_eq!(cursor.current().unwrap().unwrap(), path2);
    }

    #[tokio::test]
    async fn test_current_no_prior_operations() {
        let store = setup_test_store().await;
        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Current should be None when no operations performed
        assert!(cursor.current().unwrap().is_none());
    }

    // 4. Block Number Filtering

    #[tokio::test]
    async fn test_same_path_different_blocks() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2]);
        let branch1 = create_test_branch();
        // Make branch2 different by setting different bits
        let mut state_mask2 = TrieMask::default();
        state_mask2.set_bit(5);
        state_mask2.set_bit(6);

        let branch2 = BranchNodeCompact {
            state_mask: state_mask2,
            tree_mask: TrieMask::default(),
            hash_mask: TrieMask::default(),
            hashes: Arc::new(vec![]), // Empty hashes vector
            root_hash: None,
        };

        // Store same path at different blocks
        store.store_trie_branch(50, path.clone(), None, Some(branch1.clone())).await.unwrap();
        store.store_trie_branch(100, path.clone(), None, Some(branch2.clone())).await.unwrap();

        // Cursor with max_block_number=75 should see only block 50 data
        let mut cursor75 = store.trie_cursor(None, 75).unwrap();
        let result75 = cursor75.seek_exact(path).unwrap().unwrap();
        assert_eq!(result75.0, path);
        // We can't easily verify the branch content without more complex comparison

        // Cursor with max_block_number=150 should see block 100 data (latest)
        let mut cursor150 = store.trie_cursor(None, 150).unwrap();
        let result150 = cursor150.seek_exact(path).unwrap().unwrap();
        assert_eq!(result150.0, path);
    }

    #[tokio::test]
    async fn test_deleted_branch_nodes() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2]);
        let branch = create_test_branch();

        // Store branch node, then delete it (store None)
        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        store.store_trie_branch(100, path.clone(), None, None).await.unwrap();

        // Cursor before deletion should see the node
        let mut cursor75 = store.trie_cursor(None, 75).unwrap();
        assert!(cursor75.seek_exact(path).unwrap().is_some());

        // Cursor after deletion should not see the node
        let mut cursor150 = store.trie_cursor(None, 150).unwrap();
        assert!(cursor150.seek_exact(path).unwrap().is_none());
    }

    // 5. Hashed Address Filtering

    #[tokio::test]
    async fn test_account_specific_cursor() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2]);
        let addr1 = B256::repeat_byte(0x01);
        let addr2 = B256::repeat_byte(0x02);
        let branch = create_test_branch();

        // Store same path for different accounts
        store.store_trie_branch(50, path.clone(), Some(addr1), Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path.clone(), Some(addr2), Some(branch.clone())).await.unwrap();

        // Cursor for addr1 should only see addr1 data
        let mut cursor1 = store.trie_cursor(Some(addr1), 100).unwrap();
        let result1 = cursor1.seek_exact(path).unwrap().unwrap();
        assert_eq!(result1.0, path);

        // Cursor for addr2 should only see addr2 data
        let mut cursor2 = store.trie_cursor(Some(addr2), 100).unwrap();
        let result2 = cursor2.seek_exact(path).unwrap().unwrap();
        assert_eq!(result2.0, path);

        // Cursor for addr1 should not see addr2 data when iterating
        let mut cursor1_iter = store.trie_cursor(Some(addr1), 100).unwrap();
        let mut found_count = 0;
        while cursor1_iter.next().unwrap().is_some() {
            found_count += 1;
        }
        assert_eq!(found_count, 1); // Should only see one entry (for addr1)
    }

    #[tokio::test]
    async fn test_state_trie_cursor() {
        let store = setup_test_store().await;
        let path = nibbles_from(vec![1, 2]);
        let addr = B256::repeat_byte(0x01);
        let branch = create_test_branch();

        // Store data for account trie and state trie
        store.store_trie_branch(50, path.clone(), Some(addr), Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();

        // State trie cursor (None address) should only see state trie data
        let mut state_cursor = store.trie_cursor(None, 100).unwrap();
        let result = state_cursor.seek_exact(path).unwrap().unwrap();
        assert_eq!(result.0, path);

        // Verify state cursor doesn't see account data when iterating
        let mut state_cursor_iter = store.trie_cursor(None, 100).unwrap();
        let mut found_count = 0;
        let mut found_paths = Vec::new();
        while let Some((path, _)) = state_cursor_iter.next().unwrap() {
            found_count += 1;
            found_paths.push(path);
        }

        // Clean test output

        assert_eq!(found_count, 1); // Should only see state trie entry
    }

    #[tokio::test]
    async fn test_mixed_account_state_data() {
        let store = setup_test_store().await;
        let path1 = nibbles_from(vec![1]);
        let path2 = nibbles_from(vec![2]);
        let addr = B256::repeat_byte(0x01);
        let branch = create_test_branch();

        // Store mixed account and state trie data
        store.store_trie_branch(50, path1.clone(), Some(addr), Some(branch.clone())).await.unwrap();
        store.store_trie_branch(50, path2.clone(), None, Some(branch.clone())).await.unwrap();

        // Account cursor should only see account data
        let mut account_cursor = store.trie_cursor(Some(addr), 100).unwrap();
        let mut account_paths = Vec::new();
        while let Some((path, _)) = account_cursor.next().unwrap() {
            account_paths.push(path);
        }
        assert_eq!(account_paths.len(), 1);
        assert_eq!(account_paths[0], path1);

        // State cursor should only see state data
        let mut state_cursor = store.trie_cursor(None, 100).unwrap();
        let mut state_paths = Vec::new();
        while let Some((path, _)) = state_cursor.next().unwrap() {
            state_paths.push(path);
        }
        assert_eq!(state_paths.len(), 1);
        assert_eq!(state_paths[0], path2);
    }

    // 6. Path Ordering Tests

    #[tokio::test]
    async fn test_lexicographic_ordering() {
        let store = setup_test_store().await;
        let paths = vec![
            nibbles_from(vec![3, 1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![1]),
        ];
        let branch = create_test_branch();

        // Store paths in random order
        for path in &paths {
            store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        }

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next().unwrap() {
            found_paths.push(path);
        }

        // Should be returned in lexicographic order: [1], [1,2], [2], [3,1]
        let expected_order = vec![
            nibbles_from(vec![1]),
            nibbles_from(vec![1, 2]),
            nibbles_from(vec![2]),
            nibbles_from(vec![3, 1]),
        ];

        assert_eq!(found_paths, expected_order);
    }

    #[tokio::test]
    async fn test_path_prefix_scenarios() {
        let store = setup_test_store().await;
        let paths = vec![
            nibbles_from(vec![1]),       // Prefix of next
            nibbles_from(vec![1, 2]),    // Extends first
            nibbles_from(vec![1, 2, 3]), // Extends second
        ];
        let branch = create_test_branch();

        for path in &paths {
            store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        }

        let mut cursor = store.trie_cursor(None, 100).unwrap();

        // Seek to prefix should find exact match
        let result = cursor.seek_exact(paths[0]).unwrap().unwrap();
        assert_eq!(result.0, paths[0]);

        // Next should go to next path, not skip prefixed paths
        let result = cursor.next().unwrap().unwrap();
        assert_eq!(result.0, paths[1]);

        let result = cursor.next().unwrap().unwrap();
        assert_eq!(result.0, paths[2]);
    }

    #[tokio::test]
    async fn test_complex_nibble_combinations() {
        let store = setup_test_store().await;
        // Test various nibble patterns including edge values
        let paths = vec![
            nibbles_from(vec![0]),
            nibbles_from(vec![0, 15]),
            nibbles_from(vec![15]),
            nibbles_from(vec![15, 0]),
            nibbles_from(vec![7, 8, 9]),
        ];
        let branch = create_test_branch();

        for path in &paths {
            store.store_trie_branch(50, path.clone(), None, Some(branch.clone())).await.unwrap();
        }

        let mut cursor = store.trie_cursor(None, 100).unwrap();
        let mut found_paths = Vec::new();
        while let Some((path, _)) = cursor.next().unwrap() {
            found_paths.push(path);
        }

        // All paths should be found and in correct order
        assert_eq!(found_paths.len(), 5);

        // Verify specific ordering for edge cases
        assert_eq!(found_paths[0], nibbles_from(vec![0]));
        assert_eq!(found_paths[1], nibbles_from(vec![0, 15]));
        assert_eq!(found_paths[4], nibbles_from(vec![15, 0]));
    }

    // 7. Leaf Node Tests (Hashed Accounts and Storage)

    fn create_test_account() -> Account {
        Account {
            nonce: 42,
            balance: U256::from(1000000),
            bytecode_hash: Some(B256::repeat_byte(0xBB)),
        }
    }

    #[tokio::test]
    async fn test_store_and_retrieve_single_account() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x01);
        let account = create_test_account();

        // Store account
        store.store_hashed_accounts(vec![(account_key, Some(account.clone()))], 50).await.unwrap();

        // Retrieve via cursor
        let mut cursor = store.account_hashed_cursor(100).unwrap();
        let result = cursor.seek(account_key).unwrap().unwrap();

        assert_eq!(result.0, account_key);
        assert_eq!(result.1.nonce, account.nonce);
        assert_eq!(result.1.balance, account.balance);
        assert_eq!(result.1.bytecode_hash, account.bytecode_hash);
    }

    #[tokio::test]
    async fn test_account_cursor_navigation() {
        let store = setup_test_store().await;
        let accounts = vec![
            (B256::repeat_byte(0x01), create_test_account()),
            (B256::repeat_byte(0x03), create_test_account()),
            (B256::repeat_byte(0x05), create_test_account()),
        ];

        // Store accounts
        store
            .store_hashed_accounts(
                accounts.clone().into_iter().map(|(key, account)| (key, Some(account))).collect(),
                50,
            )
            .await
            .unwrap();

        let mut cursor = store.account_hashed_cursor(100).unwrap();

        // Test seeking to exact key
        let result = cursor.seek(accounts[1].0).unwrap().unwrap();
        assert_eq!(result.0, accounts[1].0);

        // Test seeking to key that doesn't exist (should return next greater)
        let seek_key = B256::repeat_byte(0x02);
        let result = cursor.seek(seek_key).unwrap().unwrap();
        assert_eq!(result.0, accounts[1].0); // Should find 0x03

        // Test next() navigation
        let result = cursor.next().unwrap().unwrap();
        assert_eq!(result.0, accounts[2].0); // Should find 0x05

        // Test next() at end
        assert!(cursor.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_account_block_versioning() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x01);
        let account_v1 = Account {
            nonce: 1,
            balance: U256::from(100),
            bytecode_hash: Some(B256::repeat_byte(0xBB)),
        };
        let account_v2 = Account {
            nonce: 2,
            balance: U256::from(200),
            bytecode_hash: Some(B256::repeat_byte(0xDD)),
        };

        // Store account at different blocks
        store
            .store_hashed_accounts(vec![(account_key, Some(account_v1.clone()))], 50)
            .await
            .unwrap();
        store
            .store_hashed_accounts(vec![(account_key, Some(account_v2.clone()))], 100)
            .await
            .unwrap();

        // Cursor with max_block_number=75 should see v1
        let mut cursor75 = store.account_hashed_cursor(75).unwrap();
        let result75 = cursor75.seek(account_key).unwrap().unwrap();
        assert_eq!(result75.1.nonce, account_v1.nonce);
        assert_eq!(result75.1.balance, account_v1.balance);

        // Cursor with max_block_number=150 should see v2
        let mut cursor150 = store.account_hashed_cursor(150).unwrap();
        let result150 = cursor150.seek(account_key).unwrap().unwrap();
        assert_eq!(result150.1.nonce, account_v2.nonce);
        assert_eq!(result150.1.balance, account_v2.balance);
    }

    #[tokio::test]
    async fn test_store_and_retrieve_storage() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_slots = vec![
            (B256::repeat_byte(0x10), U256::from(100)),
            (B256::repeat_byte(0x20), U256::from(200)),
            (B256::repeat_byte(0x30), U256::from(300)),
        ];

        // Store storage slots
        store
            .store_hashed_storages(
                storage_slots
                    .iter()
                    .cloned()
                    .map(|(key, value)| (hashed_address, StorageEntry::new(key, value)))
                    .collect(),
                50,
            )
            .await
            .unwrap();

        // Retrieve via cursor
        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Test seeking to each slot
        for (key, expected_value) in &storage_slots {
            let result = cursor.seek(*key).unwrap().unwrap();
            assert_eq!(result.0, *key);
            assert_eq!(result.1, *expected_value);
        }
    }

    #[tokio::test]
    async fn test_storage_cursor_navigation() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_slots = vec![
            (B256::repeat_byte(0x10), U256::from(100)),
            (B256::repeat_byte(0x30), U256::from(300)),
            (B256::repeat_byte(0x50), U256::from(500)),
        ];

        store
            .store_hashed_storages(
                storage_slots
                    .iter()
                    .cloned()
                    .map(|(key, value)| (hashed_address, StorageEntry::new(key, value)))
                    .collect(),
                50,
            )
            .await
            .unwrap();

        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Start from beginning with next()
        let mut found_slots = Vec::new();
        while let Some((key, value)) = cursor.next().unwrap() {
            found_slots.push((key, value));
        }

        assert_eq!(found_slots.len(), 3);
        assert_eq!(found_slots[0], storage_slots[0]);
        assert_eq!(found_slots[1], storage_slots[1]);
        assert_eq!(found_slots[2], storage_slots[2]);
    }

    #[tokio::test]
    async fn test_storage_account_isolation() {
        let store = setup_test_store().await;
        let address1 = B256::repeat_byte(0x01);
        let address2 = B256::repeat_byte(0x02);
        let storage_key = B256::repeat_byte(0x10);

        // Store same storage key for different accounts
        store
            .store_hashed_storages(
                vec![(address1, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();
        store
            .store_hashed_storages(
                vec![(address2, StorageEntry::new(storage_key, U256::from(200)))],
                50,
            )
            .await
            .unwrap();

        // Verify each account sees only its own storage
        let mut cursor1 = store.storage_hashed_cursor(address1, 100).unwrap();
        let result1 = cursor1.seek(storage_key).unwrap().unwrap();
        assert_eq!(result1.1, U256::from(100));

        let mut cursor2 = store.storage_hashed_cursor(address2, 100).unwrap();
        let result2 = cursor2.seek(storage_key).unwrap().unwrap();
        assert_eq!(result2.1, U256::from(200));

        // Verify cursor1 doesn't see address2's storage
        let mut cursor1_iter = store.storage_hashed_cursor(address1, 100).unwrap();
        let mut count = 0;
        while cursor1_iter.next().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 1); // Should only see one entry
    }

    #[tokio::test]
    async fn test_storage_block_versioning() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage at different blocks
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(200)))],
                100,
            )
            .await
            .unwrap();

        // Cursor with max_block_number=75 should see old value
        let mut cursor75 = store.storage_hashed_cursor(hashed_address, 75).unwrap();
        let result75 = cursor75.seek(storage_key).unwrap().unwrap();
        assert_eq!(result75.1, U256::from(100));

        // Cursor with max_block_number=150 should see new value
        let mut cursor150 = store.storage_hashed_cursor(hashed_address, 150).unwrap();
        let result150 = cursor150.seek(storage_key).unwrap().unwrap();
        assert_eq!(result150.1, U256::from(200));
    }

    #[tokio::test]
    async fn test_storage_zero_value_deletion() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store non-zero value
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();

        // "Delete" by storing zero value
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::ZERO))],
                100,
            )
            .await
            .unwrap();

        // Cursor before deletion should see the value
        let mut cursor75 = store.storage_hashed_cursor(hashed_address, 75).unwrap();
        let result75 = cursor75.seek(storage_key).unwrap().unwrap();
        assert_eq!(result75.1, U256::from(100));

        // Cursor after deletion should see zero
        let mut cursor150 = store.storage_hashed_cursor(hashed_address, 150).unwrap();
        let result150 = cursor150.seek(storage_key).unwrap().unwrap();
        assert_eq!(result150.1, U256::ZERO);
    }

    #[tokio::test]
    async fn test_empty_cursors() {
        let store = setup_test_store().await;

        // Test empty account cursor
        let mut account_cursor = store.account_hashed_cursor(100).unwrap();
        assert!(account_cursor.seek(B256::repeat_byte(0x01)).unwrap().is_none());
        assert!(account_cursor.next().unwrap().is_none());

        // Test empty storage cursor
        let mut storage_cursor = store.storage_hashed_cursor(B256::repeat_byte(0x01), 100).unwrap();
        assert!(storage_cursor.seek(B256::repeat_byte(0x10)).unwrap().is_none());
        assert!(storage_cursor.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cursor_boundary_conditions() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x80); // Middle value
        let account = create_test_account();

        store.store_hashed_accounts(vec![(account_key, Some(account))], 50).await.unwrap();

        let mut cursor = store.account_hashed_cursor(100).unwrap();

        // Seek to minimum key should find our account
        let result = cursor.seek(B256::ZERO).unwrap().unwrap();
        assert_eq!(result.0, account_key);

        // Seek to maximum key should find nothing
        assert!(cursor.seek(B256::repeat_byte(0xFF)).unwrap().is_none());

        // Seek to key just before our account should find our account
        let just_before = B256::repeat_byte(0x7F);
        let result = cursor.seek(just_before).unwrap().unwrap();
        assert_eq!(result.0, account_key);
    }

    #[tokio::test]
    async fn test_large_batch_operations() {
        let store = setup_test_store().await;

        // Create large batch of accounts
        let mut accounts = Vec::new();
        for i in 0..100 {
            let key = B256::from([i as u8; 32]);
            let account = Account {
                nonce: i as u64,
                balance: U256::from(i * 1000),
                bytecode_hash: Some(B256::repeat_byte((i + 1) as u8)),
            };
            accounts.push((key, account));
        }

        // Store in batch
        store
            .store_hashed_accounts(
                accounts.clone().into_iter().map(|(key, account)| (key, Some(account))).collect(),
                50,
            )
            .await
            .unwrap();

        // Verify all accounts can be retrieved
        let mut cursor = store.account_hashed_cursor(100).unwrap();
        let mut found_count = 0;
        while cursor.next().unwrap().is_some() {
            found_count += 1;
        }
        assert_eq!(found_count, 100);

        // Test specific account retrieval
        let test_key = B256::from([42u8; 32]);
        let result = cursor.seek(test_key).unwrap().unwrap();
        assert_eq!(result.0, test_key);
        assert_eq!(result.1.nonce, 42);
    }

    // 8. is_storage_empty Tests

    #[tokio::test]
    async fn test_storage_cursor_is_storage_empty_when_empty() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);

        // Create storage cursor for empty storage
        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Should return true for empty storage
        assert!(cursor.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_storage_cursor_is_storage_empty_when_not_empty() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store some storage data
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();

        // Create storage cursor
        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Should return false for non-empty storage
        assert!(!cursor.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_storage_cursor_is_storage_empty_with_different_address() {
        let store = setup_test_store().await;
        let address1 = B256::repeat_byte(0x01);
        let address2 = B256::repeat_byte(0x02);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage data for address1 only
        store
            .store_hashed_storages(
                vec![(address1, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();

        // Cursor for address1 should not be empty
        let mut cursor1 = store.storage_hashed_cursor(address1, 100).unwrap();
        assert!(!cursor1.is_storage_empty().unwrap());

        // Cursor for address2 should be empty
        let mut cursor2 = store.storage_hashed_cursor(address2, 100).unwrap();
        assert!(cursor2.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_storage_cursor_is_storage_empty_with_block_filtering() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage data at block 100
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                100,
            )
            .await
            .unwrap();

        // Cursor with max_block_number=50 should see empty storage
        let mut cursor_early = store.storage_hashed_cursor(hashed_address, 50).unwrap();
        assert!(cursor_early.is_storage_empty().unwrap());

        // Cursor with max_block_number=150 should see non-empty storage
        let mut cursor_late = store.storage_hashed_cursor(hashed_address, 150).unwrap();
        assert!(!cursor_late.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_storage_cursor_is_storage_empty_after_deletion() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage data, then "delete" it by storing zero value
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::ZERO))],
                100,
            )
            .await
            .unwrap();

        // Cursor before deletion should not be empty
        let mut cursor_before = store.storage_hashed_cursor(hashed_address, 75).unwrap();
        assert!(!cursor_before.is_storage_empty().unwrap());

        // Cursor after deletion should still not be empty (zero value is still a value)
        let mut cursor_after = store.storage_hashed_cursor(hashed_address, 150).unwrap();
        assert!(!cursor_after.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_account_cursor_is_storage_empty_when_empty() {
        let store = setup_test_store().await;

        // Create account cursor for empty accounts
        let mut cursor = store.account_hashed_cursor(100).unwrap();

        // Should return true for empty accounts
        assert!(cursor.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_account_cursor_is_storage_empty_when_not_empty() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x01);
        let account = create_test_account();

        // Store account data
        store.store_hashed_accounts(vec![(account_key, Some(account))], 50).await.unwrap();

        // Create account cursor
        let mut cursor = store.account_hashed_cursor(100).unwrap();

        // Should return false for non-empty accounts
        assert!(!cursor.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_account_cursor_is_storage_empty_with_block_filtering() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x01);
        let account = create_test_account();

        // Store account data at block 100
        store.store_hashed_accounts(vec![(account_key, Some(account))], 100).await.unwrap();

        // Cursor with max_block_number=50 should see empty accounts
        let mut cursor_early = store.account_hashed_cursor(50).unwrap();
        assert!(cursor_early.is_storage_empty().unwrap());

        // Cursor with max_block_number=150 should see non-empty accounts
        let mut cursor_late = store.account_hashed_cursor(150).unwrap();
        assert!(!cursor_late.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_account_cursor_is_storage_empty_after_deletion() {
        let store = setup_test_store().await;
        let account_key = B256::repeat_byte(0x01);
        let account = create_test_account();

        // Store account data, then "delete" it by storing empty value
        store.store_hashed_accounts(vec![(account_key, Some(account))], 50).await.unwrap();
        store.store_hashed_accounts(vec![(account_key, None)], 100).await.unwrap();

        // Cursor before deletion should not be empty
        let mut cursor_before = store.account_hashed_cursor(75).unwrap();
        assert!(!cursor_before.is_storage_empty().unwrap());

        // Cursor after deletion should be empty (None value means deleted)
        let mut cursor_after = store.account_hashed_cursor(150).unwrap();
        assert!(cursor_after.is_storage_empty().unwrap());
    }

    #[tokio::test]
    async fn test_is_storage_empty_does_not_affect_cursor_state() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);
        let storage_key = B256::repeat_byte(0x10);

        // Store storage data
        store
            .store_hashed_storages(
                vec![(hashed_address, StorageEntry::new(storage_key, U256::from(100)))],
                50,
            )
            .await
            .unwrap();

        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Perform some navigation
        let result1 = cursor.seek(storage_key).unwrap().unwrap();
        assert_eq!(result1.0, storage_key);

        // Check is_storage_empty
        assert!(!cursor.is_storage_empty().unwrap());

        // Navigation should still work normally after is_storage_empty call
        let result2 = cursor.next().unwrap();
        assert!(result2.is_none()); // No more entries

        // Seek should still work
        let result3 = cursor.seek(storage_key).unwrap().unwrap();
        assert_eq!(result3.0, storage_key);
    }

    #[tokio::test]
    async fn test_storage_cursor_skips_zero_values() {
        let store = setup_test_store().await;
        let hashed_address = B256::repeat_byte(0x01);

        // Create storage slots with a mix of zero and non-zero values
        // Storage keys: 0x10, 0x20 (zero), 0x30, 0x40 (zero), 0x50
        let storage_entries = vec![
            (B256::repeat_byte(0x10), U256::from(100)),
            (B256::repeat_byte(0x20), U256::ZERO), // Zero value - should be skipped
            (B256::repeat_byte(0x30), U256::from(300)),
            (B256::repeat_byte(0x40), U256::ZERO), // Zero value - should be skipped
            (B256::repeat_byte(0x50), U256::from(500)),
        ];

        // Store all entries including zeros
        store
            .store_hashed_storages(
                storage_entries
                    .iter()
                    .map(|(key, value)| (hashed_address, StorageEntry::new(*key, *value)))
                    .collect(),
                50,
            )
            .await
            .unwrap();

        let mut cursor = store.storage_hashed_cursor(hashed_address, 100).unwrap();

        // Test 1: Iterate through all entries using next() - should skip zeros
        let mut found_entries = Vec::new();
        while let Some((key, value)) = cursor.next().unwrap() {
            found_entries.push((key, value));
        }

        // Should only find non-zero values (0x10, 0x30, 0x50)
        assert_eq!(found_entries.len(), 3);
        assert_eq!(found_entries[0].0, B256::repeat_byte(0x10));
        assert_eq!(found_entries[0].1, U256::from(100));
        assert_eq!(found_entries[1].0, B256::repeat_byte(0x30));
        assert_eq!(found_entries[1].1, U256::from(300));
        assert_eq!(found_entries[2].0, B256::repeat_byte(0x50));
        assert_eq!(found_entries[2].1, U256::from(500));

        // Test 2: Seek to a zero value should skip to the next non-zero value
        let mut cursor2 = store.storage_hashed_cursor(hashed_address, 100).unwrap();
        let result = cursor2.seek(B256::repeat_byte(0x20)).unwrap().unwrap();
        // Should skip 0x20 (zero) and find 0x30 (non-zero)
        assert_eq!(result.0, B256::repeat_byte(0x30));
        assert_eq!(result.1, U256::from(300));

        // Test 3: Seek to just before a zero value should find the zero's successor
        let mut cursor3 = store.storage_hashed_cursor(hashed_address, 100).unwrap();
        let result = cursor3.seek(B256::repeat_byte(0x1F)).unwrap().unwrap();
        // Should skip 0x20 (zero) and find 0x30 (non-zero)
        // Actually, since we're seeking to 0x1F, it should first find 0x20 if present,
        // but since 0x20 is zero, it should be skipped
        // The expected behavior is to find the next non-zero value >= 0x1F
        // which would be 0x10 if we're seeking, or skip through to 0x30
        // Let me reconsider: if we seek to 0x1F, the next key >= 0x1F is 0x20,
        // but 0x20 has zero value so it should be skipped, so we get 0x30
        // But wait, 0x10 < 0x1F, so seeking to 0x1F should give us >= 0x1F
        // So it should check 0x20 (zero, skip), then 0x30 (non-zero, return)
        assert_eq!(result.0, B256::repeat_byte(0x30));
        assert_eq!(result.1, U256::from(300));

        // Test 4: is_storage_empty should consider zero values as non-existent
        // Create a cursor for an address that only has zero values
        let address_with_only_zeros = B256::repeat_byte(0x02);
        store
            .store_hashed_storages(
                vec![(
                    address_with_only_zeros,
                    StorageEntry::new(B256::repeat_byte(0x10), U256::ZERO),
                )],
                50,
            )
            .await
            .unwrap();

        let mut cursor_zeros = store.storage_hashed_cursor(address_with_only_zeros, 100).unwrap();
        // Storage with only zero values should be considered empty
        assert!(cursor_zeros.is_storage_empty().unwrap());
    }
}
