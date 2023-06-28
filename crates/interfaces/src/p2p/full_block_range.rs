use crate::p2p::{
    bodies::client::BodiesClient, error::PeerRequestResult, headers::client::HeadersClient,
};
use reth_primitives::{BlockBody, Header, HeadersDirection, SealedBlock, SealedHeader, H256};
use std::{
    cmp::Reverse,
    fmt::Debug,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

use super::headers::client::HeadersRequest;

/// A Client that can fetch full blocks from the network.
#[derive(Debug, Clone)]
pub struct FullBlockRangeClient<Client> {
    client: Client,
}

impl<Client> FullBlockRangeClient<Client> {
    /// Creates a new instance of `FullBlockRangeClient`.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl<Client> FullBlockRangeClient<Client>
where
    Client: BodiesClient + HeadersClient + Clone,
{
    /// Returns a future that fetches [SealedBlock]s for the given hash and count.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) responses but guarantees that
    /// [SealedHeader]s match the requested hash.
    pub fn get_full_block_range(
        &self,
        hash: H256,
        count: u64,
    ) -> FetchFullBlockRangeFuture<Client> {
        let client = self.client.clone();

        FetchFullBlockRangeFuture {
            hash,
            count,
            request: FullBlockRangeRequest {
                headers: Some(client.get_headers(HeadersRequest {
                    start: hash.into(),
                    limit: count,
                    direction: HeadersDirection::Falling,
                })),
                bodies: None,
            },
            client,
            headers: None,
            bodies: None,
        }
    }
}

/// A future that downloads a range of full blocks from the network.
#[must_use = "futures do nothing unless polled"]
pub struct FetchFullBlockRangeFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    client: Client,
    hash: H256,
    count: u64,
    request: FullBlockRangeRequest<Client>,
    headers: Option<Vec<SealedHeader>>,
    bodies: Option<Vec<BlockBody>>,
}

impl<Client> FetchFullBlockRangeFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    /// Returns the block hashes for the given range, if they are available.
    pub fn range_block_hashes(&self) -> Option<Vec<H256>> {
        self.headers.as_ref().map(|h| h.iter().map(|h| h.hash()).collect::<Vec<_>>())
    }

    /// Returns the [SealedBlock]s if the request is complete.
    fn take_blocks(&mut self) -> Option<Vec<SealedBlock>> {
        if self.headers.is_none() || self.bodies.is_none() {
            return None
        }

        let headers = self.headers.take().unwrap();
        let bodies = self.bodies.take().unwrap();
        Some(
            headers
                .iter()
                .zip(bodies.iter())
                .map(|(h, b)| SealedBlock::new(h.clone(), b.clone()))
                .collect::<Vec<_>>(),
        )
    }
}

impl<Client> Future for FetchFullBlockRangeFuture<Client>
where
    Client: BodiesClient + HeadersClient + Unpin + 'static,
{
    type Output = Vec<SealedBlock>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.request.poll(cx)) {
                RangeResponseResult::Header(res) => {
                    match res {
                        Ok(headers) => {
                            let (peer, mut headers) = headers
                                .map(|h| {
                                    h.iter().map(|h| h.clone().seal_slow()).collect::<Vec<_>>()
                                })
                                .split();

                            // ensure the response is what we requested
                            if headers.is_empty() || (headers.len() as u64) != this.count {
                                // received bad response
                                this.client.report_bad_message(peer);
                            } else {
                                // sort headers from highest to lowest block number
                                headers.sort_unstable_by_key(|h| Reverse(h.number));

                                // check the starting hash
                                if headers[0].hash() != this.hash {
                                    // received bad response
                                    this.client.report_bad_message(peer);
                                } else {
                                    // get the bodies request so it can be polled later
                                    let hashes =
                                        headers.iter().map(|h| h.hash()).collect::<Vec<_>>();

                                    // set the actual request
                                    this.request.bodies =
                                        Some(this.client.get_block_bodies(hashes));

                                    // set the headers response
                                    this.headers = Some(headers);
                                }
                            }
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.hash, "Header range download failed");
                        }
                    }

                    if this.headers.is_none() {
                        // received bad response, retry
                        this.request.headers = Some(this.client.get_headers(HeadersRequest {
                            start: this.hash.into(),
                            limit: this.count,
                            direction: HeadersDirection::Falling,
                        }));
                    }
                }
                RangeResponseResult::Body(res) => {
                    match res {
                        Ok(bodies_resp) => {
                            let (peer, bodies) = bodies_resp.split();
                            if bodies.len() != this.count as usize {
                                // received bad response
                                this.client.report_bad_message(peer);
                            } else {
                                // TODO: do we need to order the bodies?
                                // or can we assume they are in the same order as the headers?
                                this.bodies = Some(bodies);
                            }
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.hash, "Body download failed");
                        }
                    }
                    if this.bodies.is_none() {
                        // received bad response, re-request headers
                        // TODO: convert this into two futures, one which is a headers range
                        // future, and one which is a bodies range future.
                        //
                        // The headers range future should yield the bodies range future.
                        // The bodies range future should not have an Option<Vec<H256>>, it should
                        // have a populated Vec<H256> from the successful headers range future.
                        //
                        // This is optimal because we can not send a bodies request without
                        // first completing the headers request. This way we can get rid of the
                        // following `if let Some`.
                        if let Some(hashes) = this.range_block_hashes() {
                            this.request.bodies = Some(this.client.get_block_bodies(hashes));
                        }
                    }
                }
            }

            if let Some(res) = this.take_blocks() {
                return Poll::Ready(res)
            }
        }
    }
}

struct FullBlockRangeRequest<Client>
where
    Client: BodiesClient + HeadersClient,
{
    headers: Option<<Client as HeadersClient>::Output>,
    bodies: Option<<Client as BodiesClient>::Output>,
}

impl<Client> FullBlockRangeRequest<Client>
where
    Client: BodiesClient + HeadersClient,
{
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<RangeResponseResult> {
        if let Some(fut) = Pin::new(&mut self.headers).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                self.headers = None;
                return Poll::Ready(RangeResponseResult::Header(res))
            }
        }

        if let Some(fut) = Pin::new(&mut self.bodies).as_pin_mut() {
            if let Poll::Ready(res) = fut.poll(cx) {
                self.bodies = None;
                return Poll::Ready(RangeResponseResult::Body(res))
            }
        }

        Poll::Pending
    }
}

enum RangeResponseResult {
    Header(PeerRequestResult<Vec<Header>>),
    Body(PeerRequestResult<Vec<BlockBody>>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::{
        download::DownloadClient, headers::client::HeadersRequest, priority::Priority,
    };
    use parking_lot::Mutex;
    use reth_primitives::{BlockHashOrNumber, BlockNumHash, PeerId, WithPeerId};
    use std::{collections::HashMap, sync::Arc};

    #[derive(Clone, Default, Debug)]
    struct TestFullBlockRangeClient {
        headers: Arc<Mutex<HashMap<H256, Header>>>,
        bodies: Arc<Mutex<HashMap<H256, BlockBody>>>,
    }

    impl TestFullBlockRangeClient {
        fn insert(&self, header: SealedHeader, body: BlockBody) {
            let hash = header.hash();
            let header = header.unseal();
            self.headers.lock().insert(hash, header);
            self.bodies.lock().insert(hash, body);
        }
    }

    impl DownloadClient for TestFullBlockRangeClient {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl HeadersClient for TestFullBlockRangeClient {
        type Output = futures::future::Ready<PeerRequestResult<Vec<Header>>>;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            _priority: Priority,
        ) -> Self::Output {
            let headers = self.headers.lock();
            let mut block: BlockHashOrNumber = match request.start {
                BlockHashOrNumber::Hash(hash) => headers.get(&hash).cloned(),
                BlockHashOrNumber::Number(num) => {
                    headers.values().find(|h| h.number == num).cloned()
                }
            }
            .map(|h| h.number.into())
            .unwrap();

            let mut resp = Vec::new();

            for _ in 0..request.limit {
                // fetch from storage
                if let Some((_, header)) = headers.iter().find(|(hash, header)| {
                    BlockNumHash::new(header.number, **hash).matches_block_or_num(&block)
                }) {
                    match request.direction {
                        HeadersDirection::Falling => block = header.parent_hash.into(),
                        HeadersDirection::Rising => {
                            let next = header.number + 1;
                            block = next.into()
                        }
                    }
                    resp.push(header.clone());
                } else {
                    break
                }
            }
            futures::future::ready(Ok(WithPeerId::new(PeerId::random(), resp)))
        }
    }

    impl BodiesClient for TestFullBlockRangeClient {
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
        let client = TestFullBlockRangeClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockRangeClient::new(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::new(header, body));
    }

    #[tokio::test]
    async fn download_full_block_range() {
        let client = TestFullBlockRangeClient::default();
        let mut header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        for _ in 0..10 {
            header.parent_hash = header.hash_slow();
            header.number += 1;
            header = header.header.seal_slow();
            client.insert(header.clone(), body.clone());
        }
        let client = FullBlockRangeClient::new(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::new(header.clone(), body));

        let received = client.get_full_block_range(header.hash(), 10).await;
        assert_eq!(received.len(), 10);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.header.number, expected_number);
        }
    }
}
