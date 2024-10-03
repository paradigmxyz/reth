use super::headers::client::HeadersRequest;
use crate::{
    bodies::client::{BodiesClient, SingleBodyRequest},
    error::PeerRequestResult,
    headers::client::{HeadersClient, SingleHeaderRequest},
    BlockClient,
};
use alloy_primitives::{Sealable, B256};
use reth_consensus::{Consensus, ConsensusError};
use reth_eth_wire_types::HeadersDirection;
use reth_network_peers::WithPeerId;
use reth_primitives::{BlockBody, GotExpected, Header, SealedBlock, SealedHeader};
use std::{
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// A Client that can fetch full blocks from the network.
#[derive(Debug, Clone)]
pub struct FullBlockClient<Client> {
    client: Client,
    consensus: Arc<dyn Consensus>,
}

impl<Client> FullBlockClient<Client> {
    /// Creates a new instance of `FullBlockClient`.
    pub fn new(client: Client, consensus: Arc<dyn Consensus>) -> Self {
        Self { client, consensus }
    }

    /// Returns a client with Test consensus
    #[cfg(any(test, feature = "test-utils"))]
    pub fn test_client(client: Client) -> Self {
        Self::new(client, Arc::new(reth_consensus::test_utils::TestConsensus::default()))
    }
}

impl<Client> FullBlockClient<Client>
where
    Client: BlockClient,
{
    /// Returns a future that fetches the [`SealedBlock`] for the given hash.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) response but guarantees that the
    /// [`SealedHeader`] matches the requested hash.
    pub fn get_full_block(&self, hash: B256) -> FetchFullBlockFuture<Client> {
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

    /// Returns a future that fetches [`SealedBlock`]s for the given hash and count.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) responses but guarantees that
    /// the starting [`SealedHeader`] matches the requested hash, and that the number of headers and
    /// bodies received matches the requested limit.
    ///
    /// The returned future yields bodies in falling order, i.e. with descending block numbers.
    pub fn get_full_block_range(
        &self,
        hash: B256,
        count: u64,
    ) -> FetchFullBlockRangeFuture<Client> {
        let client = self.client.clone();
        FetchFullBlockRangeFuture {
            start_hash: hash,
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
            pending_headers: VecDeque::new(),
            bodies: HashMap::default(),
            consensus: Arc::clone(&self.consensus),
        }
    }
}

/// A future that downloads a full block from the network.
///
/// This will attempt to fetch both the header and body for the given block hash at the same time.
/// When both requests succeed, the future will yield the full block.
#[must_use = "futures do nothing unless polled"]
pub struct FetchFullBlockFuture<Client>
where
    Client: BlockClient,
{
    client: Client,
    hash: B256,
    request: FullBlockRequest<Client>,
    header: Option<SealedHeader>,
    body: Option<BodyResponse>,
}

impl<Client> FetchFullBlockFuture<Client>
where
    Client: BlockClient,
{
    /// Returns the hash of the block being requested.
    pub const fn hash(&self) -> &B256 {
        &self.hash
    }

    /// If the header request is already complete, this returns the block number
    pub fn block_number(&self) -> Option<u64> {
        self.header.as_ref().map(|h| h.number)
    }

    /// Returns the [`SealedBlock`] if the request is complete and valid.
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
                    debug!(target: "downloaders", %err, hash=?header.hash(), "Received wrong body");
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
                debug!(target: "downloaders", %err, hash=?header.hash(), "Received wrong body");
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
    Client: BlockClient + 'static,
{
    type Output = SealedBlock;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.request.poll(cx)) {
                ResponseResult::Header(res) => {
                    match res {
                        Ok(maybe_header) => {
                            let (peer, maybe_header) = maybe_header
                                .map(|h| {
                                    h.map(|h| {
                                        let sealed = h.seal_slow();
                                        let (header, seal) = sealed.into_parts();
                                        SealedHeader::new(header, seal)
                                    })
                                })
                                .split();
                            if let Some(header) = maybe_header {
                                if header.hash() == this.hash {
                                    this.header = Some(header);
                                } else {
                                    debug!(target: "downloaders", expected=?this.hash, received=?header.hash(), "Received wrong header");
                                    // received a different header than requested
                                    this.client.report_bad_message(peer)
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
    Client: BlockClient,
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
    Client: BlockClient,
{
    header: Option<SingleHeaderRequest<<Client as HeadersClient>::Output>>,
    body: Option<SingleBodyRequest<<Client as BodiesClient>::Output>>,
}

impl<Client> FullBlockRequest<Client>
where
    Client: BlockClient,
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
    let ommers_hash = block.calculate_ommers_root();
    if header.ommers_hash != ommers_hash {
        return Err(ConsensusError::BodyOmmersHashDiff(
            GotExpected { got: ommers_hash, expected: header.ommers_hash }.into(),
        ))
    }

    let tx_root = block.calculate_tx_root();
    if header.transactions_root != tx_root {
        return Err(ConsensusError::BodyTransactionRootDiff(
            GotExpected { got: tx_root, expected: header.transactions_root }.into(),
        ))
    }

    match (header.withdrawals_root, &block.withdrawals) {
        (Some(header_withdrawals_root), Some(withdrawals)) => {
            let withdrawals = withdrawals.as_slice();
            let withdrawals_root = reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
            if withdrawals_root != header_withdrawals_root {
                return Err(ConsensusError::BodyWithdrawalsRootDiff(
                    GotExpected { got: withdrawals_root, expected: header_withdrawals_root }.into(),
                ))
            }
        }
        (None, None) => {
            // this is ok because we assume the fork is not active in this case
        }
        _ => return Err(ConsensusError::WithdrawalsRootUnexpected),
    }

    match (header.requests_root, &block.requests) {
        (Some(header_requests_root), Some(requests)) => {
            let requests = requests.0.as_slice();
            let requests_root = reth_primitives::proofs::calculate_requests_root(requests);
            if requests_root != header_requests_root {
                return Err(ConsensusError::BodyRequestsRootDiff(
                    GotExpected { got: requests_root, expected: header_requests_root }.into(),
                ))
            }
        }
        (None, None) => {
            // this is ok because we assume the fork is not active in this case
        }
        _ => return Err(ConsensusError::RequestsRootUnexpected),
    }

    Ok(())
}

/// A future that downloads a range of full blocks from the network.
///
/// This first fetches the headers for the given range using the inner `Client`. Once the request
/// is complete, it will fetch the bodies for the headers it received.
///
/// Once the bodies request completes, the [`SealedBlock`]s will be assembled and the future will
/// yield the full block range.
///
/// The full block range will be returned with falling block numbers, i.e. in descending order.
///
/// NOTE: this assumes that bodies responses are returned by the client in the same order as the
/// hash array used to request them.
#[must_use = "futures do nothing unless polled"]
#[allow(missing_debug_implementations)]
pub struct FetchFullBlockRangeFuture<Client>
where
    Client: BlockClient,
{
    /// The client used to fetch headers and bodies.
    client: Client,
    /// The consensus instance used to validate the blocks.
    consensus: Arc<dyn Consensus>,
    /// The block hash to start fetching from (inclusive).
    start_hash: B256,
    /// How many blocks to fetch: `len([start_hash, ..]) == count`
    count: u64,
    /// Requests for headers and bodies that are in progress.
    request: FullBlockRangeRequest<Client>,
    /// Fetched headers.
    headers: Option<Vec<SealedHeader>>,
    /// The next headers to request bodies for. This is drained as responses are received.
    pending_headers: VecDeque<SealedHeader>,
    /// The bodies that have been received so far.
    bodies: HashMap<SealedHeader, BodyResponse>,
}

impl<Client> FetchFullBlockRangeFuture<Client>
where
    Client: BlockClient,
{
    /// Returns the block hashes for the given range, if they are available.
    pub fn range_block_hashes(&self) -> Option<Vec<B256>> {
        self.headers.as_ref().map(|h| h.iter().map(|h| h.hash()).collect::<Vec<_>>())
    }

    /// Returns whether or not the bodies map is fully populated with requested headers and bodies.
    fn is_bodies_complete(&self) -> bool {
        self.bodies.len() == self.count as usize
    }

    /// Inserts a block body, matching it with the `next_header`.
    ///
    /// Note: this assumes the response matches the next header in the queue.
    fn insert_body(&mut self, body_response: BodyResponse) {
        if let Some(header) = self.pending_headers.pop_front() {
            self.bodies.insert(header, body_response);
        }
    }

    /// Inserts multiple block bodies.
    fn insert_bodies(&mut self, bodies: impl IntoIterator<Item = BodyResponse>) {
        for body in bodies {
            self.insert_body(body);
        }
    }

    /// Returns the remaining hashes for the bodies request, based on the headers that still exist
    /// in the `root_map`.
    fn remaining_bodies_hashes(&self) -> Vec<B256> {
        self.pending_headers.iter().map(|h| h.hash()).collect()
    }

    /// Returns the [`SealedBlock`]s if the request is complete and valid.
    ///
    /// The request is complete if the number of blocks requested is equal to the number of blocks
    /// received. The request is valid if the returned bodies match the roots in the headers.
    ///
    /// These are returned in falling order starting with the requested `hash`, i.e. with
    /// descending block numbers.
    fn take_blocks(&mut self) -> Option<Vec<SealedBlock>> {
        if !self.is_bodies_complete() {
            // not done with bodies yet
            return None
        }

        let headers = self.headers.take()?;
        let mut needs_retry = false;
        let mut valid_responses = Vec::new();

        for header in &headers {
            if let Some(body_resp) = self.bodies.remove(header) {
                // validate body w.r.t. the hashes in the header, only inserting into the response
                let body = match body_resp {
                    BodyResponse::Validated(body) => body,
                    BodyResponse::PendingValidation(resp) => {
                        // ensure the block is valid, else retry
                        if let Err(err) = ensure_valid_body_response(header, resp.data()) {
                            debug!(target: "downloaders", %err, hash=?header.hash(), "Received wrong body in range response");
                            self.client.report_bad_message(resp.peer_id());

                            // get body that doesn't match, put back into vecdeque, and retry it
                            self.pending_headers.push_back(header.clone());
                            needs_retry = true;
                            continue
                        }

                        resp.into_data()
                    }
                };

                valid_responses.push(SealedBlock::new(header.clone(), body));
            }
        }

        if needs_retry {
            // put response hashes back into bodies map since we aren't returning them as a
            // response
            for block in valid_responses {
                let (header, body) = block.split_header_body();
                self.bodies.insert(header, BodyResponse::Validated(body));
            }

            // put headers back since they were `take`n before
            self.headers = Some(headers);

            // create response for failing bodies
            let hashes = self.remaining_bodies_hashes();
            self.request.bodies = Some(self.client.get_block_bodies(hashes));
            return None
        }

        Some(valid_responses)
    }

    fn on_headers_response(&mut self, headers: WithPeerId<Vec<Header>>) {
        let (peer, mut headers_falling) = headers
            .map(|h| {
                h.into_iter()
                    .map(|h| {
                        let sealed = h.seal_slow();
                        let (header, seal) = sealed.into_parts();
                        SealedHeader::new(header, seal)
                    })
                    .collect::<Vec<_>>()
            })
            .split();

        // fill in the response if it's the correct length
        if headers_falling.len() == self.count as usize {
            // sort headers from highest to lowest block number
            headers_falling.sort_unstable_by_key(|h| Reverse(h.number));

            // check the starting hash
            if headers_falling[0].hash() == self.start_hash {
                let headers_rising = headers_falling.iter().rev().cloned().collect::<Vec<_>>();
                // check if the downloaded headers are valid
                if let Err(err) = self.consensus.validate_header_range(&headers_rising) {
                    debug!(target: "downloaders", %err, ?self.start_hash, "Received bad header response");
                    self.client.report_bad_message(peer);
                }

                // get the bodies request so it can be polled later
                let hashes = headers_falling.iter().map(|h| h.hash()).collect::<Vec<_>>();

                // populate the pending headers
                self.pending_headers = headers_falling.clone().into();

                // set the actual request if it hasn't been started yet
                if !self.has_bodies_request_started() {
                    // request the bodies for the downloaded headers
                    self.request.bodies = Some(self.client.get_block_bodies(hashes));
                }

                // set the headers response
                self.headers = Some(headers_falling);
            } else {
                // received a different header than requested
                self.client.report_bad_message(peer);
            }
        }
    }

    /// Returns whether or not a bodies request has been started, returning false if there is no
    /// pending request.
    const fn has_bodies_request_started(&self) -> bool {
        self.request.bodies.is_some()
    }

    /// Returns the start hash for the request
    pub const fn start_hash(&self) -> B256 {
        self.start_hash
    }

    /// Returns the block count for the request
    pub const fn count(&self) -> u64 {
        self.count
    }
}

impl<Client> Future for FetchFullBlockRangeFuture<Client>
where
    Client: BlockClient + 'static,
{
    type Output = Vec<SealedBlock>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match ready!(this.request.poll(cx)) {
                // This branch handles headers responses from peers - it first ensures that the
                // starting hash and number of headers matches what we requested.
                //
                // If these don't match, we penalize the peer and retry the request.
                // If they do match, we sort the headers by block number and start the request for
                // the corresponding block bodies.
                //
                // The next result that should be yielded by `poll` is the bodies response.
                RangeResponseResult::Header(res) => {
                    match res {
                        Ok(headers) => {
                            this.on_headers_response(headers);
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.start_hash, "Header range download failed");
                        }
                    }

                    if this.headers.is_none() {
                        // did not receive a correct response yet, retry
                        this.request.headers = Some(this.client.get_headers(HeadersRequest {
                            start: this.start_hash.into(),
                            limit: this.count,
                            direction: HeadersDirection::Falling,
                        }));
                    }
                }
                // This branch handles block body responses from peers - it first inserts the
                // bodies into the `bodies` map, and then checks if the request is complete.
                //
                // If the request is not complete, and we need to request more bodies, we send
                // a bodies request for the headers we don't yet have bodies for.
                RangeResponseResult::Body(res) => {
                    match res {
                        Ok(bodies_resp) => {
                            let (peer, new_bodies) = bodies_resp.split();

                            // first insert the received bodies
                            this.insert_bodies(
                                new_bodies
                                    .into_iter()
                                    .map(|resp| WithPeerId::new(peer, resp))
                                    .map(BodyResponse::PendingValidation),
                            );

                            if !this.is_bodies_complete() {
                                // get remaining hashes so we can send the next request
                                let req_hashes = this.remaining_bodies_hashes();

                                // set a new request
                                this.request.bodies = Some(this.client.get_block_bodies(req_hashes))
                            }
                        }
                        Err(err) => {
                            debug!(target: "downloaders", %err, ?this.start_hash, "Body range download failed");
                        }
                    }
                    if this.bodies.is_empty() {
                        // received bad response, re-request headers
                        // TODO: convert this into two futures, one which is a headers range
                        // future, and one which is a bodies range future.
                        //
                        // The headers range future should yield the bodies range future.
                        // The bodies range future should not have an Option<Vec<B256>>, it should
                        // have a populated Vec<B256> from the successful headers range future.
                        //
                        // This is optimal because we can not send a bodies request without
                        // first completing the headers request. This way we can get rid of the
                        // following `if let Some`. A bodies request should never be sent before
                        // the headers request completes, so this should always be `Some` anyways.
                        let hashes = this.remaining_bodies_hashes();
                        if !hashes.is_empty() {
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

/// A request for a range of full blocks. Polling this will poll the inner headers and bodies
/// futures until they return responses. It will return either the header or body result, depending
/// on which future successfully returned.
struct FullBlockRangeRequest<Client>
where
    Client: BlockClient,
{
    headers: Option<<Client as HeadersClient>::Output>,
    bodies: Option<<Client as BodiesClient>::Output>,
}

impl<Client> FullBlockRangeRequest<Client>
where
    Client: BlockClient,
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

// The result of a request for headers or block bodies. This is yielded by the
// `FullBlockRangeRequest` future.
enum RangeResponseResult {
    Header(PeerRequestResult<Vec<Header>>),
    Body(PeerRequestResult<Vec<BlockBody>>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestFullBlockClient;
    use std::ops::Range;

    #[tokio::test]
    async fn download_single_full_block() {
        let client = TestFullBlockClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block(header.hash()).await;
        assert_eq!(received, SealedBlock::new(header, body));
    }

    #[tokio::test]
    async fn download_single_full_block_range() {
        let client = TestFullBlockClient::default();
        let header = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::new(header, body));
    }

    /// Inserts headers and returns the last header and block body.
    fn insert_headers_into_client(
        client: &TestFullBlockClient,
        range: Range<usize>,
    ) -> (SealedHeader, BlockBody) {
        let mut sealed_header = SealedHeader::default();
        let body = BlockBody::default();
        for _ in range {
            let (mut header, hash) = sealed_header.split();
            // update to the next header
            header.parent_hash = hash;
            header.number += 1;

            let sealed = header.seal_slow();
            let (header, seal) = sealed.into_parts();
            sealed_header = SealedHeader::new(header, seal);

            client.insert(sealed_header.clone(), body.clone());
        }

        (sealed_header, body)
    }

    #[tokio::test]
    async fn download_full_block_range() {
        let client = TestFullBlockClient::default();
        let (header, body) = insert_headers_into_client(&client, 0..50);
        let client = FullBlockClient::test_client(client);

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

    #[tokio::test]
    async fn download_full_block_range_over_soft_limit() {
        // default soft limit is 20, so we will request 50 blocks
        let client = TestFullBlockClient::default();
        let (header, body) = insert_headers_into_client(&client, 0..50);
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::new(header.clone(), body));

        let received = client.get_full_block_range(header.hash(), 50).await;
        assert_eq!(received.len(), 50);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.header.number, expected_number);
        }
    }

    #[tokio::test]
    async fn download_full_block_range_with_invalid_header() {
        let client = TestFullBlockClient::default();
        let range_length: usize = 3;
        let (header, _) = insert_headers_into_client(&client, 0..range_length);

        let test_consensus = reth_consensus::test_utils::TestConsensus::default();
        test_consensus.set_fail_validation(true);
        let client = FullBlockClient::new(client, Arc::new(test_consensus));

        let received = client.get_full_block_range(header.hash(), range_length as u64).await;

        assert_eq!(received.len(), range_length);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.header.number, expected_number);
        }
    }
}
