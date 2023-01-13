use reth_primitives::{SealedHeader, H256};

/// Represents the current download state.
#[derive(Debug)]
pub struct DownloaderState {
    /// The current chain tip.
    tip: H256,
    /// The latest downloaded header.
    latest_downloaded: Option<SealedHeader>,
    /// Current download range.
    range: Option<(SealedHeader, SealedHeader)>,
}
