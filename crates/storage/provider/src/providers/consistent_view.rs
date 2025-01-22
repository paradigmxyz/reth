use crate::{BlockNumReader, DatabaseProviderFactory, HeaderProvider};
use alloy_primitives::B256;
use reth_errors::ProviderError;
use reth_storage_api::{DBProvider, StateCommitmentProvider};
use reth_storage_errors::provider::ProviderResult;

use reth_trie::HashedPostState;
use reth_trie_db::{DatabaseHashedPostState, StateCommitment};

pub use reth_storage_errors::provider::ConsistentViewError;
use tracing::debug;

/// A consistent view over state in the database.
///
/// View gets initialized with the latest or provided tip.
/// Upon every attempt to create a database provider, the view will
/// perform a consistency check of current tip against the initial one.
///
/// ## Usage
///
/// The view should only be used outside of staged-sync.
/// Otherwise, any attempt to create a provider will result in [`ConsistentViewError::Syncing`].
///
/// When using the view, the consumer should either
/// 1) have a failover for when the state changes and handle [`ConsistentViewError::Inconsistent`]
///    appropriately.
/// 2) be sure that the state does not change.
#[derive(Clone, Debug)]
pub struct ConsistentDbView<Factory> {
    factory: Factory,
    tip: Option<B256>,
}

impl<Factory> ConsistentDbView<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockNumReader + HeaderProvider>
        + StateCommitmentProvider,
{
    /// Creates new consistent database view.
    pub fn new(factory: Factory, tip: Option<B256>) -> Self {
        debug!(target: "providers::consistent_view", ?tip, "Initializing consistent view provider with latest tip num and hash");
        Self { factory, tip }
    }

    /// Creates new consistent database view with latest tip.
    pub fn new_with_latest_tip(provider: Factory) -> ProviderResult<Self> {
        let provider_ro = provider.database_provider_ro()?;
        // NOTE: there is a RACE CONDITION here! If we are currently persisting block N, during a
        // commit, we first commit to static files, then commit to the DB.
        //
        // This is to prevent other race conditions, eg something checking the stage finish
        // thresholds and then attempting to read a static file that is not committed yet.
        //
        // HOWEVER, because we commit in this order, and the `last_block_number` + header by number
        // methods check static files, these methods will return a high block number, one that may
        // not be in the DB's hash indeces yet.
        //
        // This means if we go:
        //
        // let last_num = provider_ro.last_block_number()?;
        // let tip = provider_ro.sealed_header(last_num)?.map(|h| h.hash());
        // // this returns `None`, because it only checks the DB, which is not committed yet
        // let tip = provider_ro.header(tip.hash())?;
        let last_num = provider_ro.last_block_number()?;
        let tip = provider_ro.sealed_header(last_num)?.map(|h| h.hash());
        debug!(target: "providers::consistent_view", ?tip, ?last_num, "Initializing consistent view provider after fetching tip num and hash");
        Ok(Self::new(provider, tip))
    }

    /// Retrieve revert hashed state down to the given block hash.
    pub fn revert_state(&self, block_hash: B256) -> ProviderResult<HashedPostState> {
        let provider = self.provider_ro()?;
        let block_number = provider
            .block_number(block_hash)?
            .ok_or(ProviderError::BlockHashNotFound(block_hash))?;
        if block_number == provider.best_block_number()? &&
            block_number == provider.last_block_number()?
        {
            Ok(HashedPostState::default())
        } else {
            Ok(HashedPostState::from_reverts::<
                <Factory::StateCommitment as StateCommitment>::KeyHasher,
            >(provider.tx_ref(), block_number + 1)?)
        }
    }

    /// Creates new read-only provider and performs consistency checks on the current tip.
    pub fn provider_ro(&self) -> ProviderResult<Factory::Provider> {
        // Create a new provider.
        let provider_ro = self.factory.database_provider_ro()?;

        // Check that the currently stored tip is included on-disk.
        // This means that the database has moved, but the view was not reorged.
        if let Some(tip) = self.tip {
            if provider_ro.header(&tip)?.is_none() {
            tracing::debug!(target: "providers::consistent_view", tip=?self.tip, "Could not initialize consistent vieiw RO provider");
                return Err(ConsistentViewError::Reorged { block: tip }.into())
            }
        }

        Ok(provider_ro)
    }
}
