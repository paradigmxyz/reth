use crate::{BlockNumReader, DatabaseProviderFactory, DatabaseProviderRO, ProviderError};
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::provider::{ConsistentViewError, ProviderResult};
use reth_primitives::{GotExpected, B256};
use std::marker::PhantomData;

/// A consistent view over state in the database.
///
/// View gets initialized with the latest or provided tip.
/// Upon every attempt to create a database provider, the view will
/// perform a consistency check of current tip against the initial one.
///
/// ## Usage
///
/// The view should only be used outside of staged-sync.
/// Otherwise, any attempt to create a provider will result in [ConsistentViewError::Syncing].
#[derive(Clone, Debug)]
pub struct ConsistentDbView<DB, Provider> {
    database: PhantomData<DB>,
    provider: Provider,
    tip: Option<B256>,
}

impl<DB, Provider> ConsistentDbView<DB, Provider>
where
    DB: Database,
    Provider: DatabaseProviderFactory<DB>,
{
    /// Creates new consistent database view.
    pub fn new(provider: Provider) -> Self {
        Self { database: PhantomData, provider, tip: None }
    }

    /// Initializes the view with provided tip.
    pub fn with_tip(mut self, tip: B256) -> Self {
        self.tip = Some(tip);
        self
    }

    /// Initializes the view with latest tip.
    pub fn with_latest_tip(mut self) -> ProviderResult<Self> {
        let provider = self.provider.database_provider_ro()?;
        let tip = provider.tx_ref().cursor_read::<tables::CanonicalHeaders>()?.last()?;
        self.tip = tip.map(|(_, hash)| hash);
        Ok(self)
    }

    /// Creates new read-only provider and performs consistency checks on the current tip.
    pub fn provider_ro(&self) -> Result<DatabaseProviderRO<DB>, ConsistentViewError> {
        let provider_ro = self.provider.database_provider_ro()?;
        let last_entry = provider_ro
            .tx_ref()
            .cursor_read::<tables::CanonicalHeaders>()
            .and_then(|mut cursor| cursor.last())
            .map_err(ProviderError::Database)?;

        let tip = last_entry.map(|(_, hash)| hash);
        if self.tip != tip {
            return Err(ConsistentViewError::InconsistentView {
                tip: GotExpected { got: tip, expected: self.tip },
            })
        }

        let best_block_number = provider_ro.best_block_number()?;
        if last_entry.map(|(number, _)| number).unwrap_or_default() != best_block_number {
            return Err(ConsistentViewError::Syncing(best_block_number))
        }

        Ok(provider_ro)
    }
}
