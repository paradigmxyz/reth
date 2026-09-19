//! Best-effort BAL backfill driven by canonical chain heads.

use alloy_consensus::BlockHeader;
use alloy_eips::NumHash;
use futures::{stream, FutureExt, Stream, StreamExt};
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use reth_network_p2p::block_access_lists::client::BlockAccessListsClient;
use reth_storage_api::{
    errors::provider::ProviderResult, BalStoreHandle, BlockHashReader, BlockNumReader,
    HeaderProvider, RawBal,
};
use std::{num::NonZeroUsize, ops::RangeInclusive, time::Duration};
use tracing::{debug, warn};

/// Fills missing BALs within the store's retention window.
///
/// Run on a blocking executor: header/store access and BAL validation are synchronous. Network
/// requests are best effort and never block chain progress. Failed or unavailable downloads are
/// retried from the first remaining gap. Progress is kept only for the current canonical chain.
#[derive(Debug)]
pub struct BalDownloader<P, C> {
    provider: P,
    store: BalStoreHandle,
    client: C,
    max_concurrent_requests: NonZeroUsize,
    retention: u64,
    /// Earlier blocks are already filled or precede BAL activation.
    next_block: u64,
    /// The canonical head that the scan progress belongs to.
    last_head: Option<NumHash>,
    metrics: BalDownloaderMetrics,
}

impl<P, C> BalDownloader<P, C>
where
    P: HeaderProvider + BlockNumReader + BlockHashReader + Sync,
    C: BlockAccessListsClient,
{
    /// Creates a downloader with the given concurrency bound and store retention distance.
    pub fn new(
        provider: P,
        store: BalStoreHandle,
        client: C,
        max_concurrent_requests: NonZeroUsize,
        retention: u64,
    ) -> Self {
        Self {
            provider,
            store,
            client,
            max_concurrent_requests,
            retention,
            next_block: 0,
            last_head: None,
            metrics: Default::default(),
        }
    }

    /// Backfills on startup and new heads. A timer also retries misses when the head is unchanged.
    pub async fn run(mut self, mut heads: impl Stream<Item = ()> + Unpin) {
        loop {
            if let Err(err) = self.backfill().await {
                warn!(target: "reth::bal", %err, "Failed to scan BAL gaps");
            }
            tokio::select! {
                head = heads.next() => {
                    if head.is_none() { return }
                    // Coalesce heads received while a pass was in progress.
                    while matches!(heads.next().now_or_never(), Some(Some(()))) {}
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            }
        }
    }

    async fn backfill(&mut self) -> ProviderResult<()> {
        let info = self.provider.chain_info()?;
        let tip = info.best_number;
        let current = NumHash::new(tip, info.best_hash);
        if self.last_head == Some(current) && self.next_block > tip {
            return Ok(())
        }
        let Some(head) = self.provider.sealed_header_by_hash(current.hash)? else { return Ok(()) };
        let is_child = self.last_head.is_some_and(|previous| {
            previous.number.checked_add(1) == Some(tip) && head.parent_hash() == previous.hash
        });
        if let Some(previous) = self.last_head &&
            !is_child &&
            (previous.number > tip ||
                self.provider.block_hash(previous.number)? != Some(previous.hash))
        {
            self.next_block = 0;
        }
        self.last_head = Some(current);
        if head.block_access_list_hash().is_none() {
            self.next_block = tip.saturating_add(1);
            return Ok(())
        }
        // Normal engine progress needs only a cache hit; failed inserts must still be backfilled.
        if is_child && self.next_block == tip && self.store.get_by_hash(current.hash)?.is_some() {
            self.metrics.skipped.increment(1);
            self.next_block = tip.saturating_add(1);
            return Ok(())
        }
        let start = tip.saturating_sub(self.retention).max(self.next_block);
        let mut next_block = tip.saturating_add(1);
        // Keep both scanned headers and response memory bounded, regardless of chain height.
        let ranges =
            (start..=tip).step_by(16).map(|start| start..=start.saturating_add(15).min(tip));
        let downloader = &*self;
        let mut downloads = stream::iter(ranges)
            .map(|range| async move {
                let start = *range.start();
                (start, downloader.download_range(range).await)
            })
            .buffer_unordered(self.max_concurrent_requests.get());
        while let Some((start, result)) = downloads.next().await {
            match result {
                Ok(Some(missing)) => next_block = next_block.min(missing),
                Ok(None) => {}
                Err(err) => {
                    next_block = next_block.min(start);
                    warn!(target: "reth::bal", %err, "Failed to fill BAL gap");
                }
            }
        }
        drop(downloads);
        self.next_block = next_block;
        Ok(())
    }

    /// Returns the first gap to retry. Headers before BAL activation are complete without a BAL.
    async fn download_range(&self, range: RangeInclusive<u64>) -> ProviderResult<Option<u64>> {
        let tip = self.provider.best_block_number()?;
        let range = *range.start()..=(*range.end()).min(tip);
        if range.is_empty() {
            return Ok(None)
        }
        let headers = self.provider.sealed_headers_range(range)?;
        let mut candidates = headers
            .into_iter()
            .filter_map(|header| {
                header.block_access_list_hash().map(|commitment| (header.num_hash(), commitment))
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Ok(None)
        }
        let hashes = candidates.iter().map(|(block, _)| block.hash).collect::<Vec<_>>();
        // Also retries buffered writes from a previous failed flush, without downloading again.
        self.store.flush(&candidates.iter().map(|(block, _)| *block).collect::<Vec<_>>())?;
        let stored = self.store.get_by_hashes(&hashes)?;
        let mut stored = stored.into_iter();
        candidates.retain(|_| stored.next().flatten().is_none());
        self.metrics.skipped.increment((hashes.len() - candidates.len()) as u64);

        let mut first_missing = None;
        while !candidates.is_empty() {
            let tip = self.provider.best_block_number()?;
            candidates.retain(|(block, _)| {
                block.number <= tip && !self.store.should_prune(block.number, tip)
            });
            if candidates.is_empty() {
                break
            }
            self.metrics.requested.increment(candidates.len() as u64);
            self.metrics.in_flight_requests.increment(1.);
            let response = self
                .client
                .get_block_access_lists(candidates.iter().map(|(block, _)| block.hash).collect())
                .await;
            self.metrics.in_flight_requests.decrement(1.);
            let (peer, response) = match response {
                Ok(response) => response.split(),
                Err(err) => {
                    self.metrics.unavailable.increment(candidates.len() as u64);
                    first_missing.get_or_insert(candidates[0].0.number);
                    debug!(target: "reth::bal", %err, "BAL request unavailable");
                    break
                }
            };
            if response.0.len() > candidates.len() {
                self.client.report_bad_message(peer);
                self.metrics.invalid.increment(1);
                first_missing.get_or_insert(candidates[0].0.number);
                break
            }
            if response.0.is_empty() {
                self.metrics.unavailable.increment(candidates.len() as u64);
                first_missing.get_or_insert(candidates[0].0.number);
                break
            }
            let count = response.0.len();
            let mut blocks = Vec::with_capacity(count);
            for ((block, commitment), bytes) in candidates.drain(..count).zip(response.0) {
                let Some(bytes) = bytes else {
                    self.metrics.unavailable.increment(1);
                    first_missing.get_or_insert(block.number);
                    continue
                };
                let raw = RawBal::new(bytes);
                if raw.ensure_hash(commitment).is_err() {
                    self.metrics.invalid.increment(1);
                    self.client.report_bad_message(peer);
                    first_missing.get_or_insert(block.number);
                    continue
                }
                // A head update or reorg may have made this result obsolete while it was in flight.
                let tip = self.provider.best_block_number()?;
                if block.number > tip ||
                    block.number < tip.saturating_sub(self.retention) ||
                    self.store.should_prune(block.number, tip) ||
                    self.provider.block_hash(block.number)? != Some(block.hash)
                {
                    continue
                }
                self.store.insert(block, raw)?;
                self.metrics.downloaded.increment(1);
                blocks.push(block);
            }
            if !blocks.is_empty() {
                self.store.flush(&blocks)?;
                debug!(target: "reth::bal", count = blocks.len(), first = blocks[0].number, "Filled BAL gaps");
            }
            // eth/71 may truncate at its response byte limit. Continue with the unreturned suffix.
        }
        Ok(first_missing)
    }
}

#[derive(Metrics)]
#[metrics(scope = "downloaders.bal")]
struct BalDownloaderMetrics {
    /// Network requests currently awaiting a response.
    in_flight_requests: Gauge,
    /// Requested BALs, including retries.
    requested: Counter,
    /// Validated BALs inserted into the store, including buffered writes awaiting a flush.
    downloaded: Counter,
    /// BALs already present in the store.
    skipped: Counter,
    /// BALs unavailable from the selected peer.
    unavailable: Counter,
    /// Responses with invalid commitments or excess entries.
    invalid: Counter,
}

#[cfg(test)]
mod tests;
