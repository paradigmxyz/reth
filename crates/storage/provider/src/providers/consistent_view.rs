use crate::{BlockNumReader, DatabaseProviderFactory, DatabaseProviderRO, HeaderProvider};
use reth_db::database::Database;
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{GotExpected, B256};
use std::marker::PhantomData;

pub use reth_interfaces::provider::ConsistentViewError;

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
///
/// When using the view, the consumer should either
/// 1) have a failover for when the state changes and handle [ConsistentViewError::Inconsistent]
///    appropriately.
/// 2) be sure that the state does not change.
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
    pub fn new(provider: Provider, tip: Option<B256>) -> Self {
        Self { database: PhantomData, provider, tip }
    }

    /// Creates new consistent database view with latest tip.
    pub fn new_with_latest_tip(provider: Provider) -> ProviderResult<Self> {
        let provider_ro = provider.database_provider_ro()?;
        let last_num = provider_ro.last_block_number()?;
        let tip = provider_ro.sealed_header(last_num)?.map(|h| h.hash());
        Ok(Self::new(provider, tip))
    }

    /// Creates new read-only provider and performs consistency checks on the current tip.
    pub fn provider_ro(&self) -> ProviderResult<DatabaseProviderRO<DB>> {
        let provider_ro = self.provider.database_provider_ro()?;
        let last_num = provider_ro.last_block_number()?;
        let tip = provider_ro.sealed_header(last_num)?.map(|h| h.hash());
        if self.tip != tip {
            return Err(ConsistentViewError::Inconsistent {
                tip: GotExpected { got: tip, expected: self.tip },
            }
            .into())
        }

        let best_block_number = provider_ro.best_block_number()?;
        if last_num != best_block_number {
            return Err(ConsistentViewError::Syncing(best_block_number).into())
        }

        Ok(provider_ro)
    }
}
