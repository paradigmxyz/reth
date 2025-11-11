use crate::{
    prune::error::{OpProofStoragePrunerResult, PrunerOutput},
    BlockStateDiff, OpProofsStorageResult, OpProofsStore,
};
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash};
use alloy_primitives::B256;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{error, info, trace};

#[derive(Debug)]
pub struct OpProofStoragePruner<Provider> {
    // Database provider for the prune
    provider: Provider,
    /// Keep at least these many recent blocks (i.e., earliest advances to latest -
    /// min_block_interval).
    min_block_interval: u64,
    /// Maximum time for one pruner run. If `None`, no timeout.
    timeout: Option<Duration>,
    // TODO: metrics
}

impl<Provider> OpProofStoragePruner<Provider> {
    pub fn new(
        provider: Provider,
        min_block_interval: u64,
        timeout: Option<Duration>,
    ) -> OpProofStoragePruner<Provider> {
        Self { provider, min_block_interval, timeout }
    }
}

impl<Provider> OpProofStoragePruner<Provider>
where
    Provider: OpProofsStore,
{
    async fn run_inner(&mut self) -> OpProofStoragePrunerResult {
        let t = Instant::now();
        // TODO: handle timeout

        let latest_block_opt = self.provider.get_latest_block_number().await?;
        if latest_block_opt.is_none() {
            trace!(target: "pruner", "No latest blocks in the proof storage");
            return Ok(PrunerOutput::default())
        }

        let earliest_block_opt = self.provider.get_earliest_block_number().await?;
        if earliest_block_opt.is_none() {
            trace!(target: "pruner", "No earliest blocks in the proof storage");
            return Ok(PrunerOutput::default())
        }

        let latest_block = latest_block_opt.unwrap().0;
        let earliest_block = earliest_block_opt.unwrap().0;

        let interval = latest_block - earliest_block;
        if interval < self.min_block_interval {
            trace!(target: "pruner", "Nothing to prune");
            return Ok(PrunerOutput::default())
        }

        let new_earliest_block = latest_block - self.min_block_interval;

        info!(
            target: "pruner",
            from_block = earliest_block,
            to_block = new_earliest_block - 1,
           "Starting pruning proof storage",
        );

        let mut final_diff = BlockStateDiff::default();
        for i in earliest_block..new_earliest_block {
            let diff = self.provider.fetch_trie_updates(i).await?;
            final_diff.extend(diff);
        }

        // TODO: how we can get parent here? Or, change the function signature.
        let block_with_parent = BlockWithParent {
            parent: B256::default(),
            block: BlockNumHash { number: new_earliest_block, hash: B256::default() },
        };

        self.provider.prune_earliest_state(block_with_parent, final_diff).await?;

        Ok(PrunerOutput {
            duration: t.elapsed(),
            start_block: earliest_block,
            end_block: new_earliest_block - 1,
            total_entries_pruned: 0, // TODO: get it from the prune_earliest_state
        })
    }

    pub async fn run(&mut self) {
        let res = self.run_inner().await;
        if let Err(e) = res {
            error!(target: "pruner", "Pruner failed: {:?}", e);
            return;
        }
        info!(target: "pruner", result = %res.unwrap(), "Finished pruning proof storage");
    }
}
