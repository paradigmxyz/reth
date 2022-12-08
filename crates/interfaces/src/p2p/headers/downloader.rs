use super::client::HeadersClient;
use crate::{
    consensus::Consensus,
    p2p::{headers::error::DownloadError, traits::BatchDownload},
};

use futures::Stream;
use reth_primitives::SealedHeader;
use reth_rpc_types::engine::ForkchoiceState;
use std::pin::Pin;

/// A Future for downloading a batch of headers.
pub type HeaderBatchDownload<'a> = Pin<
    Box<
        dyn BatchDownload<
                Ok = SealedHeader,
                Error = DownloadError,
                Output = Result<Vec<SealedHeader>, DownloadError>,
            > + Send
            + 'a,
    >,
>;

/// A stream for downloading headers.
pub type HeaderDownloadStream =
    Pin<Box<dyn Stream<Item = Result<SealedHeader, DownloadError>> + Send>>;

/// A downloader capable of fetching block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient] represents a client capable of fulfilling these requests.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait HeaderDownloader: Sync + Send + Unpin {
    /// The Consensus used to verify block validity when
    /// downloading
    type Consensus: Consensus;

    /// The Client used to download the headers
    type Client: HeadersClient;

    /// The consensus engine
    fn consensus(&self) -> &Self::Consensus;

    /// The headers client
    fn client(&self) -> &Self::Client;

    /// Download the headers
    fn download(&self, head: SealedHeader, forkchoice: ForkchoiceState) -> HeaderBatchDownload<'_>;

    /// Stream the headers
    fn stream(&self, head: SealedHeader, forkchoice: ForkchoiceState) -> HeaderDownloadStream;

    /// Validate whether the header is valid in relation to it's parent
    ///
    /// Returns Ok(false) if the
    fn validate(&self, header: &SealedHeader, parent: &SealedHeader) -> Result<(), DownloadError> {
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
) -> Result<(), DownloadError> {
    ensure_parent(header, parent)?;
    consensus
        .validate_header(header, parent)
        .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
    Ok(())
}

/// Ensures that the given `parent` header is the actual parent of the `header`
pub fn ensure_parent(header: &SealedHeader, parent: &SealedHeader) -> Result<(), DownloadError> {
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
