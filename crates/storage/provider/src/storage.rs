//! Types for writing to storage.

use reth_db::database::Database;
use reth_storage_api::StorageWriter;
use crate::DatabaseProviderRW;

impl<DB: Database> StorageWriter for DatabaseProviderRW<DB> {}