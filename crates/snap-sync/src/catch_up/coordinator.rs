//! Sequences pivot changes and the canonical BAL downloader before account coverage grows.

use crate::{
    BlockAccessListCatchUp, CatchUpStep, SnapAttemptStore, SnapDownloadProgress, SnapStateStore,
    SnapSyncError, SnapSyncProvider,
};
use reth_network_p2p::snap::client::SnapClient;
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::{BlockHashReader, HeaderProvider, MetadataProvider, StageCheckpointReader};
use reth_tasks::Runtime;

/// Drives the persisted catch-up cursor to a selected pivot.
#[derive(Debug)]
pub(crate) struct CatchUpCoordinator<'a, C, F> {
    download: BlockAccessListCatchUp<&'a C, F>,
    store: SnapStateStore<'a, F>,
}

impl<'a, C, F: Clone> CatchUpCoordinator<'a, C, F> {
    pub(crate) fn new(client: &'a C, factory: &'a F, runtime: Runtime) -> Self {
        Self {
            download: BlockAccessListCatchUp::new(client, factory.clone(), runtime),
            store: SnapStateStore::new(factory),
        }
    }
}

impl<C, F> CatchUpCoordinator<'_, C, F>
where
    C: SnapClient,
    F: DatabaseProviderFactory<
            Provider: MetadataProvider + BlockHashReader + HeaderProvider + StageCheckpointReader,
        > + Clone
        + 'static,
    F::ProviderRW: SnapSyncProvider,
{
    pub(crate) async fn advance_pivot(
        &mut self,
        generation: SnapDownloadProgress,
        target: u64,
    ) -> Result<BlockAccessListCatchUpOutcome, SnapSyncError> {
        self.catch_up(generation, target, false).await
    }

    pub(crate) async fn run(
        &mut self,
        generation: SnapDownloadProgress,
        target: u64,
    ) -> Result<BlockAccessListCatchUpOutcome, SnapSyncError> {
        self.catch_up(generation, target, true).await
    }

    async fn catch_up(
        &mut self,
        generation: SnapDownloadProgress,
        target: u64,
        complete: bool,
    ) -> Result<BlockAccessListCatchUpOutcome, SnapSyncError> {
        let mut generation = self.store.advance_generation(generation, target)?;
        loop {
            let write = self
                .store
                .factory
                .database_provider_ro()?
                .active_snap_write()?
                .ok_or(SnapSyncError::NoAttempt)?;
            match self.download.next(write, generation.target_block).await? {
                CatchUpStep::Complete => {
                    if complete {
                        generation = self.store.complete_block_access_lists(generation)?;
                    }
                    return Ok(BlockAccessListCatchUpOutcome::Complete { generation });
                }
                CatchUpStep::Unavailable { .. } => {
                    return Ok(BlockAccessListCatchUpOutcome::Unavailable { generation })
                }
                CatchUpStep::Applied { .. } => {
                    generation = self
                        .store
                        .interrupted_generation()?
                        .ok_or(SnapSyncError::StaleGeneration)?;
                }
            }
        }
    }
}

/// Whether the covered state reached the pivot or needs a serving peer.
#[derive(Debug)]
pub(crate) enum BlockAccessListCatchUpOutcome {
    Complete { generation: SnapDownloadProgress },
    Unavailable { generation: SnapDownloadProgress },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{hashed_factory, BalChain, ScriptedSnapClient},
        RangeBudget, SnapPhase, StateDownloader,
    };
    use reth_provider::test_utils::insert_headers;
    use reth_trie_common::EMPTY_ROOT_HASH;

    #[tokio::test]
    async fn interrupted_pivot_advance_catches_up_before_account_downloads_resume() {
        let chain = BalChain::new(2, [Vec::new(), Vec::new()]);
        let factory = hashed_factory();
        insert_headers(&factory, &chain.headers);
        let store = SnapStateStore::new(&factory);
        let pivot = chain.block(0);
        let generation = SnapDownloadProgress::new(pivot.number, pivot.hash, EMPTY_ROOT_HASH);
        store.begin_generation(generation).unwrap();
        let old_write =
            factory.database_provider_ro().unwrap().active_snap_write().unwrap().unwrap();

        let client = ScriptedSnapClient::new([chain.response(1, [None])]);
        let outcome = CatchUpCoordinator::new(&client, &factory, Runtime::test())
            .advance_pivot(generation, chain.tip().number)
            .await
            .unwrap();
        let BlockAccessListCatchUpOutcome::Unavailable { generation } = outcome else {
            panic!("missing BAL")
        };
        assert_eq!(generation.target_block, 4);
        assert_eq!(generation.next_block, 3);
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));
        let provider = factory.database_provider_rw().unwrap();
        assert!(matches!(
            provider.authorize_snap_write(old_write),
            Err(SnapSyncError::StaleWrite { .. })
        ));
        drop(provider);

        let no_requests = ScriptedSnapClient::new([]);
        assert!(matches!(
            StateDownloader::new(&no_requests, &factory, Runtime::test())
                .run(generation, RangeBudget::new(1))
                .await,
            Err(SnapSyncError::InvalidGeneration(_))
        ));
        assert!(no_requests.origins().is_empty());

        // Recreating the coordinator simulates a restart after the pivot and first BAL commit.
        let client = ScriptedSnapClient::new([
            chain.response(1, [Some(1), None]),
            chain.response(2, [None]),
        ]);
        let outcome = CatchUpCoordinator::new(&client, &factory, Runtime::test())
            .advance_pivot(generation, 4)
            .await
            .unwrap();
        let BlockAccessListCatchUpOutcome::Unavailable { generation } = outcome else {
            panic!("missing second BAL")
        };
        assert_eq!(generation.next_block, 4);
        assert_eq!(store.interrupted_generation().unwrap(), Some(generation));

        let client = ScriptedSnapClient::new([chain.response(1, [Some(2)])]);
        let outcome = CatchUpCoordinator::new(&client, &factory, Runtime::test())
            .advance_pivot(generation, 4)
            .await
            .unwrap();
        let BlockAccessListCatchUpOutcome::Complete { generation } = outcome else {
            panic!("caught up")
        };
        assert_eq!(generation.next_block, 5);
        assert_eq!(generation.phase, SnapPhase::Accounts);
        assert_eq!(*client.block_requests(), [vec![chain.block(2).hash]]);
    }
}
