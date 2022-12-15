use crate::{
    consensus::Consensus,
    p2p::{
        downloader::{DownloadStream, Downloader},
        error::{DownloadError, DownloadResult},
    },
};

use reth_primitives::{SealedHeader, H256};

/// A downloader capable of fetching block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient] represents a client capable of fulfilling these requests.
// ANCHOR: trait-HeaderDownloader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeaderDownloader: Downloader {
    /// Stream the headers
    fn stream(&self, head: SealedHeader, tip: H256) -> DownloadStream<'_, SealedHeader>;

    /// Validate whether the header is valid in relation to it's parent
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
        validate_header_download(self.consensus(), header, parent)?;
        Ok(())
    }
}
// ANCHOR_END: trait-HeaderDownloader

/// Validate whether the header is valid in relation to it's parent
///
/// Returns Ok(false) if the
pub fn validate_header_download<C: Consensus>(
    consensus: &C,
    header: &SealedHeader,
    parent: &SealedHeader,
) -> DownloadResult<()> {
    ensure_parent(header, parent)?;
    consensus
        .validate_header(header, parent)
        .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
    Ok(())
}

/// Ensures that the given `parent` header is the actual parent of the `header`
pub fn ensure_parent(header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
    if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
        return Err(DownloadError::MismatchedHeaders {
            header_number: header.number.into(),
            parent_number: parent.number.into(),
            header_hash: header.hash(),
            parent_hash: parent.hash(),
        })
    }
    Ok(())
}
