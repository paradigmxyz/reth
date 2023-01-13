use super::DownloaderResult;
use reth_eth_wire::BlockBody;
use reth_primitives::{SealedHeader, H256};
use tokio::sync::mpsc::UnboundedSender;

/// The sender part of the communication channel between the downloader and message request sender.
pub type MessageSender<T> = UnboundedSender<DownloaderResult<T>>;

/// Downloader message request.
#[derive(Debug)]
pub enum DownloaderMessage {
    /// Header download request.
    Headers(MessageSender<SealedHeader>, HeadersMessage),
    /// Bodies download request.
    Bodies(MessageSender<BlockBody>, Vec<H256>),
}

/// Headers download message request.
#[derive(Debug)]
pub struct HeadersMessage {
    /// Header to download data to
    pub head: SealedHeader,
    /// The tip to download data from
    pub tip: H256,
}
