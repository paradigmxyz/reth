use crate::DatabaseProviderRO;
use reth_db::database::Database;
use reth_interfaces::provider::ProviderResult;

/// Database provider factory.
pub trait DatabaseProviderFactory<DB: Database> {
    /// Create new read-only database provider.
    fn database_provider_ro(&self) -> ProviderResult<DatabaseProviderRO<DB>>;
}
