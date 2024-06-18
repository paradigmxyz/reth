//! Handler that can download blocks on demand (e.g. from the network).

use crate::engine::DownloadRequest;
use reth_primitives::SealedBlockWithSenders;
use std::task::{Context, Poll};

/// A trait that can download blocks on demand.
pub trait BlockDownloader: Send + Sync {
    /// Handle an action.
    fn on_action(&mut self, event: DownloadAction);

    /// Advance in progress requests if any
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DownloadOutcome>;
}

/// Actions that can be performed by the block downloader.
#[derive(Debug)]
pub enum DownloadAction {
    /// Stop downloading blocks.
    Clear,
    /// Download given blocks
    Download(DownloadRequest),
}

/// Outcome of downloaded blocks.
#[derive(Debug)]
pub enum DownloadOutcome {
    /// Downloaded blocks.
    Blocks(Vec<SealedBlockWithSenders>),
}

/// A [BlockDownloader] that does nothing.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopBlockDownloader;

impl BlockDownloader for NoopBlockDownloader {
    fn on_action(&mut self, _event: DownloadAction) {}

    fn poll(&mut self, _cx: &mut Context<'_>) -> Poll<DownloadOutcome> {
        Poll::Pending
    }
}
