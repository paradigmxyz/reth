use auto_impl::auto_impl;
use reth_db::database::Database;
use reth_interfaces::Result;

use crate::DatabaseProviderRO;

/// A type that provides read access to any part of the database.
#[auto_impl(&, Arc, Box)]
pub trait DatabaseReader<DB>: Send + Sync
where
    DB: Database,
{
    /// Returns a readable database.
    fn database(&self) -> Result<DatabaseProviderRO<'_, &DB>>;
}
