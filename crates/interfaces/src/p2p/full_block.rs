use crate::p2p::{
    bodies::client::{BodiesClient, SingleBodyRequest},
    error::PeerRequestResult,
    headers::client::{HeadersClient, SingleHeaderRequest},
};
use reth_primitives::{BlockBody, Header, SealedBlock, SealedHeader, H256};
use std::{
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// A Client that can fetch full blocks from the network.
#[derive(Debug, Clone)]
pub struct FullBlockClient<Client> {
    client: Client,
}

impl<Client> FullBlockClient<Client> {
    /// Creates a new instance of `FullBlockClient`.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl<Client> FullBlockClient<Client>
where
    Client: BodiesClient + HeadersClient + Clone,
{
    /// Returns a future that fetches the [SealedBlock] for the given hash.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) response but guarantees that the
    /// [SealedHeader] matches the requested hash.
    pub fn get_full_block(&self, hash: H256) -> FetchFullBlockFuture<Client> {
        let client = self.client.clone();
        FetchFullBlockFuture {
            hash,
            request: FullBlockRequest {
                header: Some(client.get_header(hash.into())),
                body: Some(client.get_block_body(hash)),
            },
            client,
            header: None,
            body: None,
        }
    }
}

/// A future that downloads a full block from the network.
#[must_use = "futures do nothing unless polled"]
pub struct FetchFullBlockFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    client: Client,
    hash: H256,
    request: FullBlockRequest<Client>,
    header: Option<SealedHeader>,
    body: Option<BlockBody>,
}

impl<Client> FetchFullBlockFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    /// If the header request is already complete, this returns the block number
    pub fn block_number(&self) -> Option<u64> {
        self.header.as_ref().map(|h| h.number)
    }

    /// Returns the [SealedBlock] if the request is complete.
    fn take_block(&mut self) -> Option<SealedBlock> {
        if self.header.is_none() || self.body.is_none() {
            return None
        }
        let header = self.header.take().unwrap();
        let body = self.body.take().unwrap();

        Some(SealedBlock::new(header, body))
    }
}

impl<Client> Future for FetchFullBlockFuture<Client>
where
    Client: BodiesClient + HeadersClient + Unpin + 'static,
{
    type Output = SealedBlock;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.request.poll(cx)) {
                ResponseResult::Header(res) => {
                    match res {
                        Ok(maybe_header) => {
                            let (peer, maybe_header) =
                                maybe_header.map(|h| h.map(|h| h.seal_slow())).split();
                            if let Some(header) = maybe_header {
                                if header.hash() != this.hash {
                                    debug!(target: "downloaders", expected=?this.hash, received=?header.hash, "Received wrong header");
                                    // received bad header
                                    this.client.report_bad_message(peer)
                                } else {
                                    this.header = Some(header);
                                }
                            }
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.hash, "Header download failed");
                        }
                    }

                    if this.header.is_none() {
                        // received bad response
                        this.request.header = Some(this.client.get_header(this.hash.into()));
                    }
                }
                ResponseResult::Body(res) => {
                    match res {
                        Ok(maybe_body) => {
                            this.body = maybe_body.into_data();
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.hash, "Body download failed");
                        }
                    }
                    if this.body.is_none() {
                        // received bad response
                        this.request.body = Some(this.client.get_block_body(this.hash));
                    }
                }
            }

            if let Some(res) = this.take_block() {
                return Poll::Ready(res)
            }
        }
    }
}

impl<Client> Debug for FetchFullBlockFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchFullBlockFuture")
            .field("hash", &self.hash)
            .field("header", &self.header)
            .field("body", &self.body)
            .finish()
    }
}

struct FullBlockRequest<Client>
where
    Client: BodiesClient + HeadersClient,
{
    header: Option<SingleHeaderRequest<<Client as HeadersClient>::Output>>,
    body: Option<SingleBodyRequest<<Client as BodiesClient>::Output>>,
}

impl<Client> FullBlockRequest<Client>
where
    Client: BodiesClient + HeadersClient,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ResponseResult> {
        if let Some(fut) = Pin::new(&mut self.header).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                self.header = None;
                return Poll::Ready(ResponseResult::Header(res))
            }
        }

        if let Some(fut) = Pin::new(&mut self.body).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                self.body = None;
                return Poll::Ready(ResponseResult::Body(res))
            }
        }

        Poll::Pending
    }
}

enum ResponseResult {
    Header(PeerRequestResult<Option<Header>>),
    Body(PeerRequestResult<Option<BlockBody>>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::{
        download::DownloadClient, headers::client::HeadersRequest, priority::Priority,
    };
    use parking_lot::Mutex;
    use reth_primitives::{BlockHashOrNumber, PeerId, WithPeerId};
    use std::{collections::HashMap, sync::Arc};

    #[derive(Clone, Default, Debug)]
    struct TestSingleFullBlockClient {
        headers: Arc<Mutex<HashMap<H256, Header>>>,
        bodies: Arc<Mutex<HashMap<H256, BlockBody>>>,
    }

    impl TestSingleFullBlockClient {
        fn insert(&self, header: SealedHeader, body: BlockBody) {
            let hash = header.hash();
            let header = header.unseal();
            self.headers.lock().insert(hash, header);
            self.bodies.lock().insert(hash, body);
        }
    }

    impl DownloadClient for TestSingleFullBlockClient {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl HeadersClient for TestSingleFullBlockClient {
        type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            _priority: Priority,
        ) -> Self::Output {
            let headers = self.headers.lock();
            let resp = match request.start {
                BlockHashOrNumber::Hash(hash) => headers.get(&hash).cloned(),
                BlockHashOrNumber::Number(num) => {
                    headers.values().find(|h| h.number == num).cloned()
                }
            }
            .map(|h| vec![h])
            .unwrap_or_default();
            futures::future::ready(Ok(WithPeerId::new(PeerId::random(), resp)))
        }
    }

    impl BodiesClient for TestSingleFullBlockClient {
        type Output = futures::future::Ready<PeerRequestResult<Vec<BlockBody>>>;

        fn get_block_bodies_with_priority(
            &self,
            hashes: Vec<H256>,
            _priority: Priority,
        ) -> Self::Output {
            let bodies = self.bodies.lock();
            let mut all_bodies = Vec::new();
            for hash in hashes {
                if let Some(body) = bodies.get(&hash) {
                    all_bodies.push(body.clone());
                }
            }
            futures::future::ready(Ok(WithPeerId::new(PeerId::random(), all_bodies)))
        }
    }

    #[tokio::test]
    async fn download_single_full_block() {
        let client = TestSingleFullBlockClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::new(client);

        let received = client.get_full_block(header.hash()).await;
        assert_eq!(received, SealedBlock::new(header, body));
    }
}
