use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use alloy_primitives::{Address, BlockNumber};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_storage_api::AccountStoragePruner;
use std::{fmt::Debug, ops::RangeInclusive};
use tracing::{instrument, trace};

/// One account storage prune target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountStoragePruneTarget {
    /// Block number at which this target becomes prunable.
    pub block_number: BlockNumber,
    /// Account whose storage should be pruned.
    pub address: Address,
}

/// Batch of account storage prune targets.
#[derive(Debug)]
pub struct AccountStoragePruneBatch {
    /// Targets to prune.
    pub targets: Vec<AccountStoragePruneTarget>,
    /// Whether the source finished scanning the requested range.
    pub done: bool,
}

/// Source of expired account storage prune targets.
pub trait AccountStoragePruneSource<Provider>: Debug + Send + Sync {
    /// Returns expired account storage targets in `range`.
    ///
    /// Account storage pruning is atomic per account. The delete limit controls how many account
    /// targets are fetched and when the segment yields between accounts.
    fn account_storage_prune_targets(
        &self,
        provider: &Provider,
        range: RangeInclusive<BlockNumber>,
        limit: usize,
    ) -> Result<AccountStoragePruneBatch, PrunerError>;
}

/// Prunes expired account storage bodies while retaining storage root commitments.
#[derive(Debug)]
pub struct AccountStorage<Source> {
    mode: PruneMode,
    source: Source,
}

impl<Source> AccountStorage<Source> {
    /// Create a new account storage pruning segment.
    pub const fn new(mode: PruneMode, source: Source) -> Self {
        Self { mode, source }
    }
}

impl<Provider, Source> Segment<Provider> for AccountStorage<Source>
where
    Provider: AccountStoragePruner,
    Source: AccountStoragePruneSource<Provider>,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountStorage
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(
        name = "AccountStorage::prune",
        target = "pruner",
        skip(self, provider),
        ret(level = "trace")
    )]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account storage to prune");
                return Ok(SegmentOutput::done())
            }
        };

        if input.limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                input.limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let limit = input.limiter.deleted_entries_limit_left().unwrap_or(usize::MAX);
        let mut batch =
            self.source.account_storage_prune_targets(provider, range.clone(), limit)?;
        batch.targets.sort_unstable_by_key(|target| (target.block_number, target.address));

        if batch.targets.is_empty() {
            return Ok(SegmentOutput {
                progress: input.limiter.progress(batch.done),
                pruned: 0,
                checkpoint: batch.done.then_some(SegmentOutputCheckpoint {
                    block_number: Some(*range.end()),
                    tx_number: None,
                }),
            })
        }

        let mut limiter = input.limiter;
        let mut pruned = 0;
        let mut highest_pruned_block = None;
        let mut interrupted_before_block = None;
        let mut done = batch.done;

        for target in batch.targets {
            if limiter.is_limit_reached() {
                done = false;
                interrupted_before_block = Some(target.block_number);
                break;
            }

            let output = provider.prune_account_storage(target.address)?;
            pruned += output.deleted_storage_entries + output.deleted_trie_nodes;
            limiter.increment_deleted_entries_count_by(
                output.deleted_storage_entries + output.deleted_trie_nodes,
            );
            highest_pruned_block = Some(target.block_number);
        }

        let checkpoint_block = if done {
            Some(*range.end())
        } else if let Some(block) = interrupted_before_block {
            block.checked_sub(1)
        } else {
            highest_pruned_block.and_then(|block| block.checked_sub(1))
        };

        Ok(SegmentOutput {
            progress: limiter.progress(done),
            pruned,
            checkpoint: checkpoint_block.map(|block| SegmentOutputCheckpoint {
                block_number: Some(block),
                tx_number: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segments::{PruneInput, PruneLimiter, Segment};
    use alloy_primitives::{keccak256, B256};
    use reth_prune_types::{PruneCheckpoint, PruneInterruptReason, PruneProgress};
    use reth_storage_api::{errors::provider::ProviderResult, PrunedAccountStorage};
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct TestProvider {
        pruned: Mutex<Vec<Address>>,
    }

    impl AccountStoragePruner for TestProvider {
        fn prune_account_storage(&self, address: Address) -> ProviderResult<PrunedAccountStorage> {
            self.pruned.lock().unwrap().push(address);
            Ok(PrunedAccountStorage {
                address,
                hashed_address: keccak256(address),
                storage_root: B256::ZERO,
                deleted_storage_entries: 2,
                deleted_trie_nodes: 1,
            })
        }
    }

    #[derive(Debug)]
    struct TestSource {
        targets: Vec<AccountStoragePruneTarget>,
        done: bool,
        calls: Mutex<Vec<(RangeInclusive<BlockNumber>, usize)>>,
    }

    impl<Provider> AccountStoragePruneSource<Provider> for TestSource {
        fn account_storage_prune_targets(
            &self,
            _provider: &Provider,
            range: RangeInclusive<BlockNumber>,
            limit: usize,
        ) -> Result<AccountStoragePruneBatch, PrunerError> {
            self.calls.lock().unwrap().push((range, limit));
            Ok(AccountStoragePruneBatch { targets: self.targets.clone(), done: self.done })
        }
    }

    #[test]
    fn prunes_targets_and_finishes_when_source_is_done() {
        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let source = TestSource {
            targets: vec![
                AccountStoragePruneTarget { block_number: 7, address: address2 },
                AccountStoragePruneTarget { block_number: 3, address: address1 },
            ],
            done: true,
            calls: Mutex::default(),
        };
        let segment = AccountStorage::new(PruneMode::Before(10), source);
        let provider = TestProvider::default();
        let input = PruneInput {
            previous_checkpoint: Some(PruneCheckpoint {
                block_number: Some(1),
                tx_number: None,
                prune_mode: PruneMode::Before(10),
            }),
            to_block: 10,
            limiter: PruneLimiter::default().set_deleted_entries_limit(10),
        };

        let output = segment.prune(&provider, input).unwrap();

        assert_eq!(output.progress, PruneProgress::Finished);
        assert_eq!(output.pruned, 6);
        assert_eq!(output.checkpoint.unwrap().block_number, Some(10));
        assert_eq!(*provider.pruned.lock().unwrap(), vec![address1, address2]);

        let calls = segment.source.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, 2..=10);
        assert_eq!(calls[0].1, 10);
    }

    #[test]
    fn stops_on_deleted_entries_limit() {
        let address1 = Address::with_last_byte(1);
        let address2 = Address::with_last_byte(2);
        let source = TestSource {
            targets: vec![
                AccountStoragePruneTarget { block_number: 3, address: address2 },
                AccountStoragePruneTarget { block_number: 3, address: address1 },
            ],
            done: true,
            calls: Mutex::default(),
        };
        let segment = AccountStorage::new(PruneMode::Before(10), source);
        let provider = TestProvider::default();
        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 10,
            limiter: PruneLimiter::default().set_deleted_entries_limit(3),
        };

        let output = segment.prune(&provider, input).unwrap();

        assert_eq!(
            output.progress,
            PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached)
        );
        assert_eq!(output.pruned, 3);
        assert_eq!(output.checkpoint.unwrap().block_number, Some(2));
        assert_eq!(*provider.pruned.lock().unwrap(), vec![address1]);
    }
}
