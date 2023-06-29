use crate::{
    consensus::ConsensusError,
    p2p::{
        bodies::client::{BodiesClient, SingleBodyRequest},
        error::PeerRequestResult,
        headers::client::{HeadersClient, SingleHeaderRequest},
    },
};
use reth_primitives::{BlockBody, Header, SealedBlock, SealedHeader, H256, HeadersDirection, WithPeerId};
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

    /// Returns a future that fetches [SealedBlock]s for the given hash and count.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) responses but guarantees that
    /// the starting [SealedHeader] matches the requested hash, and that the number of headers and
    /// bodies received matches the requested limit.
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
    body: Option<BodyResponse>,
}

impl<Client> FetchFullBlockFuture<Client>
where
    Client: BodiesClient + HeadersClient,
{
    /// Returns the hash of the block being requested.
    pub fn hash(&self) -> &H256 {
        &self.hash
    }

    /// If the header request is already complete, this returns the block number
    pub fn block_number(&self) -> Option<u64> {
        self.header.as_ref().map(|h| h.number)
    }

    /// Returns the [SealedBlock] if the request is complete and valid.
    fn take_block(&mut self) -> Option<SealedBlock> {
        if self.header.is_none() || self.body.is_none() {
            return None
        }

        let header = self.header.take().unwrap();
        let resp = self.body.take().unwrap();
        match resp {
            BodyResponse::Validated(body) => Some(SealedBlock::new(header, body)),
            BodyResponse::PendingValidation(resp) => {
                // ensure the block is valid, else retry
                if let Err(err) = ensure_valid_body_response(&header, resp.data()) {
                    debug!(target: "downloaders", ?err,  hash=?header.hash, "Received wrong body");
                    self.client.report_bad_message(resp.peer_id());
                    self.header = Some(header);
                    self.request.body = Some(self.client.get_block_body(self.hash));
                    return None
                }
                Some(SealedBlock::new(header, resp.into_data()))
            }
        }
    }

    fn on_block_response(&mut self, resp: WithPeerId<BlockBody>) {
        if let Some(ref header) = self.header {
            if let Err(err) = ensure_valid_body_response(header, resp.data()) {
                debug!(target: "downloaders", ?err,  hash=?header.hash, "Received wrong body");
                self.client.report_bad_message(resp.peer_id());
                return
            }
            self.body = Some(BodyResponse::Validated(resp.into_data()));
            return
        }
        self.body = Some(BodyResponse::PendingValidation(resp));
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
                            if let Some(body) = maybe_body.transpose() {
                                this.on_block_response(body);
                            }
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

/// The result of a request for a single header or body. This is yielded by the `FullBlockRequest`
/// future.
enum ResponseResult {
    Header(PeerRequestResult<Option<Header>>),
    Body(PeerRequestResult<Option<BlockBody>>),
}

/// The response of a body request.
#[derive(Debug)]
enum BodyResponse {
    /// Already validated against transaction root of header
    Validated(BlockBody),
    /// Still needs to be validated against header
    PendingValidation(WithPeerId<BlockBody>),
}

/// Ensures the block response data matches the header.
///
/// This ensures the body response items match the header's hashes:
///   - ommer hash
///   - transaction root
///   - withdrawals root
fn ensure_valid_body_response(
    header: &SealedHeader,
    block: &BlockBody,
) -> Result<(), ConsensusError> {
    let ommers_hash = reth_primitives::proofs::calculate_ommers_root(&block.ommers);
    if header.ommers_hash != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff {
            got: ommers_hash,
            expected: header.ommers_hash,
        })
    }

    let transaction_root = reth_primitives::proofs::calculate_transaction_root(&block.transactions);
    if header.transactions_root != transaction_root {
        return Err(ConsensusError::BodyTransactionRootDiff {
            got: transaction_root,
            expected: header.transactions_root,
        })
    }

    let withdrawals = block.withdrawals.as_deref().unwrap_or(&[]);
    if let Some(header_withdrawals_root) = header.withdrawals_root {
        let withdrawals_root = reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
        if withdrawals_root != header_withdrawals_root {
            return Err(ConsensusError::BodyWithdrawalsRootDiff {
                got: withdrawals_root,
                expected: header_withdrawals_root,
            })
        }
        return Ok(())
    }

    if !withdrawals.is_empty() {
        return Err(ConsensusError::WithdrawalsRootUnexpected)
    }

    Ok(())
}

/// A future that downloads a range of full blocks from the network.
///
/// This first fetches the headers for the given range using the inner `Client`. Once the request
/// is complete, it will fetch the bodies for the headers it received.
///
/// Once the bodies request completes, this assembles the [SealedBlock]s and returns them.
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
    struct TestFullBlockClient {
        headers: Arc<Mutex<HashMap<H256, Header>>>,
        bodies: Arc<Mutex<HashMap<H256, BlockBody>>>,
    }

    impl TestFullBlockClient {
        fn insert(&self, header: SealedHeader, body: BlockBody) {
            let hash = header.hash();
            let header = header.unseal();
            self.headers.lock().insert(hash, header);
            self.bodies.lock().insert(hash, body);
        }
    }

    impl DownloadClient for TestFullBlockClient {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl HeadersClient for TestFullBlockClient {
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

    impl BodiesClient for TestFullBlockClient {
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
        let client = TestFullBlockClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::new(client);

        let received = client.get_full_block(header.hash()).await;
        assert_eq!(received, SealedBlock::new(header, body));
    }

    #[tokio::test]
    async fn download_single_full_block_range() {
        let client = TestFullBlockClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::new(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::new(header, body));
    }

    #[tokio::test]
    async fn download_full_block_range() {
        let client = TestFullBlockClient::default();
        let mut header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        for _ in 0..10 {
            header.parent_hash = header.hash_slow();
            header.number += 1;
            header = header.header.seal_slow();
            client.insert(header.clone(), body.clone());
        }
        let client = FullBlockClient::new(client);

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
