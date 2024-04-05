//! Run with
//!
//! ```not_rust
//! cargo run -p beacon-api-sidecar-fetcher -- node

use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures_util::{Stream, StreamExt};
use mev_share_sse::EventClient;
use reqwest::Error;
use reth::{providers::CanonStateNotification, transaction_pool::TransactionPoolExt};
use serde::{self, Deserialize, Serialize};
use tracing::debug;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let tasks = TaskManager::current();
    // create node config
    let node_config = NodeConfig::test()
        .dev()
        .with_rpc(RpcServerArgs::default().with_http())
        .with_chain(custom_chain());

    let NodeHandle { mut node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut notifications = node.provider.canonical_state_stream();

    Ok(())
}

#[derive(Debug)]
pub struct MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
{
    events: St,
    pool: P,
    client: reqwest::Client,
    pending_requests: Vec<Pin<Box<dyn Future<Output = BlobSidecar> + Send>>>,
    queued_actions: VecDeque<Result<BlobSidecar, reqwest::Error>>, // Buffer for ready items
}

impl<St, P> Stream for MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    type Item = BlobSidecar;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            // Attempt to drain the buffer first
            if let Some(item) = this.buffer.pop_front() {
                return Poll::Ready(Some(Ok(item)));
            }

            match this.events.as_mut().poll_next(cx) {
                Poll::Ready(Some(item)) if this.pool.exists(&item) => {
                    let processed_item = BlobSidecar::default(); // Placeholder for processing logic
                    this.buffer.push_back(processed_item);
                }
                Poll::Ready(Some(item)) => {
                    let future = async move { Ok(BlobSidecar::default()) }.boxed();
                    this.pending_requests.push_back(future);
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }

            // Try to make progress on any pending requests
            if let Some(future) = this.pending_requests.pop_front() {
                let future = future.as_mut();
                match future.poll(cx) {
                    Poll::Ready(Ok(blob)) => return Poll::Ready(Some(Ok(blob))),
                    Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e))),
                    Poll::Pending => {
                        this.pending_requests.push_front(future.boxed());
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

///TODO Add
impl<St, P> MinedSidecarStream<St, P>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    P: TransactionPoolExt + Unpin + 'static,
{
    // Ensure this method transforms a CanonStateNotification into a BlobSidecar
    fn data_exists(&mut self, item: &CanonStateNotification) -> BlobSidecar {
        // Transformation logic here
        // For demonstration, let's return a default BlobSidecar for now
        BlobSidecar { ..Default::default() }
    }
}
/// TODO: Import as feature
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlobSidecar {
    pub data: Vec<Data>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Data {
    pub index: String,
    pub blob: String,
    #[serde(rename = "kzg_commitment")]
    pub kzg_commitment: String,
    #[serde(rename = "kzg_proof")]
    pub kzg_proof: String,
    #[serde(rename = "signed_block_header")]
    pub signed_block_header: SignedBlockHeader,
    #[serde(rename = "kzg_commitment_inclusion_proof")]
    pub kzg_commitment_inclusion_proof: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedBlockHeader {
    pub message: Message,
    pub signature: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub slot: String,
    #[serde(rename = "proposer_index")]
    pub proposer_index: String,
    #[serde(rename = "parent_root")]
    pub parent_root: String,
    #[serde(rename = "state_root")]
    pub state_root: String,
    #[serde(rename = "body_root")]
    pub body_root: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BlobError {
    #[serde(rename = "code")]
    code: u16,
    #[serde(rename = "message")]
    message: String,
}
