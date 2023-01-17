use crate::{
    consensus::Consensus,
    p2p::{
        downloader::Downloader,
        error::{DownloadError, DownloadResult},
    },
};
use futures::Stream;
use reth_primitives::SealedHeader;

/// A downloader capable of fetching and yielding block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient] represents a client capable of fulfilling these requests.
///
/// A [HeaderDownloader] is a [Stream] that returns batches for headers.
pub trait HeaderDownloader: Downloader + Stream<Item = Vec<SealedHeader>> {
    /// Sets the headers batch size that the Stream should return.
    fn set_batch_size(&self, _limit: usize) {}

    /// Validate whether the header is valid in relation to it's parent
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
        validate_header_download(self.consensus(), header, parent)?;
        Ok(())
    }
}

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
