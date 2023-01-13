use super::{message::MessageSender, DownloaderError, DownloaderResult};
use reth_eth_wire::BlockBody;
use reth_primitives::SealedHeader;
use std::cmp::Ordering;
use tokio::sync::mpsc::error::SendError;

/// Header donwload future.
pub(crate) mod headers;
use headers::HeadersDownload;

/// Body download future.
pub(crate) mod bodies;
use bodies::BodiesDownload;

/// A single download abstraction.
#[derive(Debug)]
pub(crate) enum Download<N, C> {
    /// The header download. Contains request origin and [HeadersDownload].
    Headers(DownloadOrigin<SealedHeader>, HeadersDownload<N, C>),
    /// The bodies download. Contains request origin and [BodiesDownload].
    Bodies(DownloadOrigin<BlockBody>, BodiesDownload<N, C>),
}

impl<N, C> PartialEq for Download<N, C> {
    fn eq(&self, _other: &Self) -> bool {
        // TODO:
        false
    }
}

impl<N, C> Eq for Download<N, C> {}

impl<N, C> PartialOrd for Download<N, C> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<N, C> Ord for Download<N, C> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority().cmp(&other.priority())
    }
}

impl<N, C> Download<N, C> {
    /// Return download sorting priotity.
    pub(crate) fn priority(&self) -> usize {
        match self {
            Download::Headers(origin, _) => 0 + origin.priority(),
            Download::Bodies(origin, _) => 1 + origin.priority(),
        }
    }

    /// Return flag indicating whether this download is in progress.
    pub(crate) fn in_progress(&self) -> bool {
        match self {
            Download::Headers(_, inner) => inner.fut.is_some(),
            Download::Bodies(_, _) => false, // TODO:
        }
    }
}

/// The origin of the download initiation.
/// The remote origin contains the sender part of the channel
/// to forward requests to.
#[derive(Clone, Debug)]
pub(crate) enum DownloadOrigin<T> {
    /// The remote download origin.
    Remote(MessageSender<T>),
    /// The download was initiated by downloader itself.
    Downloader,
}

impl<T> DownloadOrigin<T> {
    /// Return origin sorting priority.
    /// Remote origin is prioritized before downloader-initiated requests.
    pub(crate) fn priority(&self) -> usize {
        match self {
            Self::Remote(_) => 0,
            Self::Downloader => 1,
        }
    }

    /// Send response.
    pub(crate) fn try_send_response(
        &self,
        response: T,
    ) -> Result<(), SendError<DownloaderResult<T>>> {
        match self {
            Self::Remote(tx) => tx.send(Ok(response)),
            _ => Ok(()),
        }
    }

    /// Send error.
    pub(crate) fn try_send_error(
        &self,
        error: DownloaderError,
    ) -> Result<(), SendError<DownloaderResult<T>>> {
        match self {
            Self::Remote(tx) => tx.send(Err(error)),
            _ => Ok(()),
        }
    }
}
