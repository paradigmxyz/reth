use async_trait::async_trait;
use futures::Stream;
use reth_primitives::{rpc::BlockId, Header, H256, H512};
use std::{collections::HashSet, fmt::Debug, pin::Pin};

/// The stream of messages
pub type MessageStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

/// The header request struct
#[derive(Debug)]
pub struct HeaderRequest {
    /// The starting block
    pub start: BlockId,
    /// The response max size
    pub limit: u64,
    /// Flag indicating whether the blocks should
    /// arrive in reverse
    pub reverse: bool,
}

/// The block headers downloader client
#[async_trait]
pub trait HeadersClient: Send + Sync + Debug {
    /// Update the current node status
    async fn update_status(&self, height: u64, hash: H256, td: H256);

    /// Send the header request
    async fn send_header_request(&self, id: u64, request: HeaderRequest) -> HashSet<H512>;

    /// Stream the header response messages
    async fn stream_headers(&self) -> MessageStream<(u64, Vec<Header>)>;
}
