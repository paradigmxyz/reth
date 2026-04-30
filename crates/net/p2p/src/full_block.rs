use super::headers::client::HeadersRequest;
use crate::{
    block_access_lists::client::{BalRequirement, BlockAccessListsClient},
    bodies::client::{BodiesClient, SingleBodyRequest},
    download::DownloadClient,
    error::PeerRequestResult,
    headers::client::{HeadersClient, SingleHeaderRequest},
    priority::Priority,
    BlockClient,
};
use alloy_consensus::BlockHeader;
use alloy_primitives::{Sealable, B256};
use core::marker::PhantomData;
use futures::FutureExt;
use reth_consensus::Consensus;
use reth_eth_wire_types::{
    BlockAccessLists, EthNetworkPrimitives, HeadersDirection, NetworkPrimitives,
};
use reth_network_peers::{PeerId, WithPeerId};
use reth_primitives_traits::{SealedBlock, SealedHeader};
use std::{
    cmp::Reverse,
    collections::{HashMap, VecDeque},
    fmt::Debug,
    hash::Hash,
    ops::RangeInclusive,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// Response returned by [`FetchFullBlockRangeWithOptionalAccessListsFuture`].
pub type FullBlockRangeWithOptionalAccessListsResponse<B> =
    (Vec<SealedBlock<B>>, Option<BlockAccessLists>);

/// A Client that can fetch full blocks from the network.
#[derive(Debug, Clone)]
pub struct FullBlockClient<Client>
where
    Client: BlockClient,
{
    client: Client,
    consensus: Arc<dyn Consensus<Client::Block>>,
}

impl<Client> FullBlockClient<Client>
where
    Client: BlockClient,
{
    /// Creates a new instance of `FullBlockClient`.
    pub fn new(client: Client, consensus: Arc<dyn Consensus<Client::Block>>) -> Self {
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
        FetchFullBlockFuture::new(self.client.clone(), self.consensus.clone(), hash)
    }

    /// Returns a future that fetches [`SealedBlock`]s for the given hash and count.
    ///
    /// Note: this future is cancel safe.
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
                headers: Some(client.get_headers(HeadersRequest::falling(hash.into(), count))),
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

impl<Client> FullBlockClient<Client>
where
    Client: BlockClient + BlockAccessListsClient,
{
    /// Returns a future that fetches the [`SealedBlock`] and its [`BlockAccessLists`] for the
    /// given hash.
    ///
    /// Note: this future is cancel safe
    ///
    /// Caution: This does no validation of body (transactions) response but guarantees that the
    /// [`SealedHeader`] matches the requested hash.
    pub fn get_full_block_with_access_lists(
        &self,
        hash: B256,
    ) -> FetchFullBlockWithAccessListsFuture<Client> {
        let client = self.client.clone();
        FetchFullBlockWithAccessListsFuture {
            block: FetchFullBlockFuture::new(client.clone(), self.consensus.clone(), hash),
            block_result: None,
            bal_request_state: BalRequestState::Pending(client.get_block_access_lists(vec![hash])),
        }
    }

    /// Returns a future that fetches [`SealedBlock`]s and optionally their [`BlockAccessLists`] for
    /// the given hash and count.
    ///
    /// The block range is always the primary result. Access lists are requested after the block
    /// range is downloaded. `Some` may contain a partial prefix when the peer truncates the BAL
    /// response, while `None` means the optional BAL lookup failed or was unavailable.
    pub fn get_full_block_range_with_optional_access_lists(
        &self,
        hash: B256,
        count: u64,
    ) -> FetchFullBlockRangeWithOptionalAccessListsFuture<Client> {
        let client = self.client.clone();
        FetchFullBlockRangeWithOptionalAccessListsFuture {
            blocks: self.get_full_block_range(hash, count),
            client,
            block_result: None,
            access_lists: OptionalBlockAccessListsState::WaitingForBlocks,
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
    consensus: Arc<dyn Consensus<Client::Block>>,
    hash: B256,
    request: FullBlockRequest<Client>,
    header: Option<SealedHeader<Client::Header>>,
    body: Option<BodyResponse<Client::Body>>,
}

impl<Client> FetchFullBlockFuture<Client>
where
    Client: BlockClient,
{
    fn new(client: Client, consensus: Arc<dyn Consensus<Client::Block>>, hash: B256) -> Self {
        Self {
            hash,
            consensus,
            request: FullBlockRequest {
                header: Some(client.get_header(hash.into())),
                body: Some(client.get_block_body(hash)),
            },
            client,
            header: None,
            body: None,
        }
    }

    /// Returns the hash of the block being requested.
    pub const fn hash(&self) -> &B256 {
        &self.hash
    }

    /// If the header request is already complete, this returns the block number
    pub fn block_number(&self) -> Option<u64> {
        self.header.as_ref().map(|h| h.number())
    }

    /// Returns the [`SealedBlock`] if the request is complete and valid.
    fn take_block(&mut self) -> Option<SealedBlock<Client::Block>> {
        if self.header.is_none() || self.body.is_none() {
            return None
        }

        let header = self.header.take().unwrap();
        let resp = self.body.take().unwrap();
        match resp {
            BodyResponse::Validated(body) => Some(SealedBlock::from_sealed_parts(header, body)),
            BodyResponse::PendingValidation(resp) => {
                // ensure the block is valid, else retry
                if let Err(err) = self.consensus.validate_body_against_header(resp.data(), &header)
                {
                    debug!(target: "downloaders", %err, hash=?header.hash(), "Received wrong body");
                    self.client.report_bad_message(resp.peer_id());
                    self.header = Some(header);
                    self.request.body = Some(self.client.get_block_body(self.hash));
                    return None
                }
                Some(SealedBlock::from_sealed_parts(header, resp.into_data()))
            }
        }
    }

    fn on_block_response(&mut self, resp: WithPeerId<Client::Body>) {
        if let Some(ref header) = self.header {
            if let Err(err) = self.consensus.validate_body_against_header(resp.data(), header) {
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
    Client: BlockClient<Header: BlockHeader + Sealable> + 'static,
{
    type Output = SealedBlock<Client::Block>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // preemptive yield point
        let mut budget = 4;

        loop {
            match ready!(this.request.poll(cx)) {
                ResponseResult::Header(res) => {
                    match res {
                        Ok(maybe_header) => {
                            let (peer, maybe_header) =
                                maybe_header.map(|h| h.map(SealedHeader::seal_slow)).split();
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

            // ensure we still have enough budget for another iteration
            budget -= 1;
            if budget == 0 {
                // make sure we're woken up again
                cx.waker().wake_by_ref();
                return Poll::Pending
            }
        }
    }
}

/// A future that downloads a full block and its block access lists from the network.
///
/// This composes the existing full block downloader with a block access list request so the
/// header/body logic stays centralized.
#[must_use = "futures do nothing unless polled"]
pub struct FetchFullBlockWithAccessListsFuture<Client>
where
    Client: BlockClient + BlockAccessListsClient,
{
    block: FetchFullBlockFuture<Client>,
    block_result: Option<SealedBlock<Client::Block>>,
    bal_request_state: BalRequestState<<Client as BlockAccessListsClient>::Output>,
}

impl<Client> FetchFullBlockWithAccessListsFuture<Client>
where
    Client: BlockClient<Header: BlockHeader> + BlockAccessListsClient,
{
    /// Returns the hash of the block being requested.
    pub const fn hash(&self) -> &B256 {
        self.block.hash()
    }
}

impl<Client> FetchFullBlockWithAccessListsFuture<Client>
where
    Client: BlockClient<Header: BlockHeader + Sealable> + BlockAccessListsClient + 'static,
{
    /// If the header request is already complete, this returns the block number.
    pub fn block_number(&self) -> Option<u64> {
        self.block_result.as_ref().map(|block| block.number()).or_else(|| self.block.block_number())
    }

    fn send_bal_request(&mut self) {
        let hash = *self.block.hash();
        self.bal_request_state =
            BalRequestState::Pending(self.block.client.get_block_access_lists(vec![hash]));
    }

    // This retries BAL failures inline instead of surfacing them to the outer future, so
    // `FetchFullBlockWithAccessListsFuture` only makes progress once the BAL request either
    // becomes pending again or resolves with a single access-list entry.
    fn poll_bal_request(&mut self, cx: &mut Context<'_>) {
        loop {
            let poll = match &mut self.bal_request_state {
                BalRequestState::Pending(fut) => fut.poll_unpin(cx),
                BalRequestState::Ready(_) => return,
            };

            match poll {
                Poll::Pending => return,
                Poll::Ready(res) => match res {
                    Ok(bal) => {
                        let (peer, access_lists) = bal.split();
                        if access_lists.0.len() == 1 {
                            self.bal_request_state = BalRequestState::Ready(access_lists);
                            return;
                        }

                        debug!(
                            target: "downloaders",
                            hash = ?self.block.hash(),
                            expected = 1,
                            received = access_lists.0.len(),
                            "Received wrong access list response",
                        );
                        self.block.client.report_bad_message(peer);
                        self.send_bal_request();
                    }
                    Err(err) => {
                        debug!(
                            target: "downloaders",
                            %err,
                            hash = ?self.block.hash(),
                            "Access list download failed",
                        );
                        self.send_bal_request();
                    }
                },
            }
        }
    }

    fn take_block_and_access_lists(
        &mut self,
    ) -> Option<(SealedBlock<Client::Block>, BlockAccessLists)> {
        if self.block_result.is_some() && self.bal_request_state.is_ready() {
            let block = self.block_result.take().expect("block result should exist");
            let access_lists = match &mut self.bal_request_state {
                BalRequestState::Ready(access_lists) => std::mem::take(access_lists),
                BalRequestState::Pending(_) => unreachable!("access lists should be ready"),
            };
            return Some((block, access_lists))
        }

        None
    }
}

impl<Client> Future for FetchFullBlockWithAccessListsFuture<Client>
where
    Client: BlockClient<Header: BlockHeader + Sealable> + BlockAccessListsClient + 'static,
{
    type Output = (SealedBlock<Client::Block>, BlockAccessLists);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if this.block_result.is_none() &&
            let Poll::Ready(block) = this.block.poll_unpin(cx)
        {
            this.block_result = Some(block);
        }

        this.poll_bal_request(cx);

        if let Some(res) = this.take_block_and_access_lists() {
            return Poll::Ready(res)
        }

        Poll::Pending
    }
}

impl<Client> Debug for FetchFullBlockWithAccessListsFuture<Client>
where
    Client: BlockClient<Header: BlockHeader> + BlockAccessListsClient,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchFullBlockWithAccessListsFuture")
            .field("hash", &self.block.hash())
            .field("block_ready", &self.block_result.is_some())
            .field("bal_request_ready", &self.bal_request_state.is_ready())
            .finish()
    }
}

/// Tracks the BAL request and its completed result.
enum BalRequestState<Req> {
    Pending(Req),
    Ready(BlockAccessLists),
}

impl<Req> BalRequestState<Req> {
    const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

/// A future that downloads a range of full blocks and optionally their block access lists from the
/// network.
///
/// This composes the existing full block range downloader with a follow-up optional block
/// access-list request, so callers that only need blocks can keep using
/// [`FetchFullBlockRangeFuture`].
#[must_use = "futures do nothing unless polled"]
#[expect(missing_debug_implementations)]
pub struct FetchFullBlockRangeWithOptionalAccessListsFuture<Client>
where
    Client: BlockClient + BlockAccessListsClient,
{
    blocks: FetchFullBlockRangeFuture<Client>,
    client: Client,
    block_result: Option<Vec<SealedBlock<Client::Block>>>,
    access_lists: OptionalBlockAccessListsState<<Client as BlockAccessListsClient>::Output>,
}

impl<Client> FetchFullBlockRangeWithOptionalAccessListsFuture<Client>
where
    Client: BlockClient<Header: Debug + BlockHeader + Sealable + Clone + Hash + Eq>
        + BlockAccessListsClient,
{
    fn start_access_lists_request_if_possible(&mut self) {
        if !matches!(self.access_lists, OptionalBlockAccessListsState::WaitingForBlocks) {
            return
        }

        // BALs are requested by block hash, so wait until the block range is fully assembled.
        let Some(blocks) = self.block_result.as_ref() else { return };
        let hashes = blocks.iter().map(|block| block.hash()).collect::<Vec<_>>();
        self.access_lists = OptionalBlockAccessListsState::Pending(
            self.client.get_block_access_lists_with_requirement(hashes, BalRequirement::Optional),
        );
    }

    fn on_access_lists_response(&mut self, response: WithPeerId<BlockAccessLists>) {
        let (peer, access_lists) = response.split();
        let expected =
            self.block_result.as_ref().expect("blocks should be ready before BAL response").len();
        let received = access_lists.0.len();

        if received > expected {
            debug!(
                target: "downloaders",
                start_hash = ?self.blocks.start_hash(),
                expected,
                received,
                "Received wrong access list range response",
            );
            self.client.report_bad_message(peer);
            self.access_lists = OptionalBlockAccessListsState::Ready(None);
            return
        }

        // Short BAL responses are allowed by the wire protocol. Preserve the returned prefix and
        // let callers decide whether they need to fetch the remaining entries.
        self.access_lists = OptionalBlockAccessListsState::Ready(Some(access_lists));
    }

    /// Starts and polls the optional BAL request once, if it is ready to make progress.
    fn poll_access_lists(&mut self, cx: &mut Context<'_>) {
        self.start_access_lists_request_if_possible();

        let poll = match &mut self.access_lists {
            OptionalBlockAccessListsState::Pending(fut) => fut.poll_unpin(cx),
            OptionalBlockAccessListsState::WaitingForBlocks |
            OptionalBlockAccessListsState::Ready(_) => return,
        };

        match poll {
            Poll::Pending => {}
            Poll::Ready(Ok(access_lists)) => self.on_access_lists_response(access_lists),
            Poll::Ready(Err(err)) => {
                debug!(
                    target: "downloaders",
                    %err,
                    start_hash = ?self.blocks.start_hash(),
                    "Access list range download failed",
                );

                // Optional BAL lookup is best-effort: missing eth/71 support or request failures
                // should not block returning the downloaded block range.
                self.access_lists = OptionalBlockAccessListsState::Ready(None);
            }
        }
    }

    /// Returns the block range once blocks and the optional BAL lookup are both complete.
    fn take_response(
        &mut self,
    ) -> Option<FullBlockRangeWithOptionalAccessListsResponse<Client::Block>> {
        let OptionalBlockAccessListsState::Ready(access_lists) = &mut self.access_lists else {
            return None
        };

        let blocks = self.block_result.take()?;
        Some((blocks, access_lists.take()))
    }
}

impl<Client> Future for FetchFullBlockRangeWithOptionalAccessListsFuture<Client>
where
    Client: BlockClient<Header: Debug + BlockHeader + Sealable + Clone + Hash + Eq>
        + BlockAccessListsClient
        + 'static,
{
    type Output = FullBlockRangeWithOptionalAccessListsResponse<Client::Block>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Complete the normal block range first, then issue a separate BAL hash-list request.
        if this.block_result.is_none() &&
            let Poll::Ready(blocks) = this.blocks.poll_unpin(cx)
        {
            this.block_result = Some(blocks);
        }

        this.poll_access_lists(cx);

        if let Some(response) = this.take_response() {
            return Poll::Ready(response)
        }

        Poll::Pending
    }
}

/// Tracks an optional BAL range request and its completed result.
enum OptionalBlockAccessListsState<Req> {
    /// The block hashes needed for `GetBlockAccessLists` are not known yet.
    WaitingForBlocks,
    /// A `GetBlockAccessLists` request is in flight.
    Pending(Req),
    /// `None` means the block range is available but optional BAL data is not.
    Ready(Option<BlockAccessLists>),
}

impl<Client> Debug for FetchFullBlockFuture<Client>
where
    Client: BlockClient<Header: Debug, Body: Debug>,
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
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<ResponseResult<Client::Header, Client::Body>> {
        if let Some(fut) = Pin::new(&mut self.header).as_pin_mut() &&
            let Poll::Ready(res) = fut.poll(cx)
        {
            self.header = None;
            return Poll::Ready(ResponseResult::Header(res))
        }

        if let Some(fut) = Pin::new(&mut self.body).as_pin_mut() &&
            let Poll::Ready(res) = fut.poll(cx)
        {
            self.body = None;
            return Poll::Ready(ResponseResult::Body(res))
        }

        Poll::Pending
    }
}

/// The result of a request for a single header or body. This is yielded by the `FullBlockRequest`
/// future.
enum ResponseResult<H, B> {
    Header(PeerRequestResult<Option<H>>),
    Body(PeerRequestResult<Option<B>>),
}

/// The response of a body request.
#[derive(Debug)]
enum BodyResponse<B> {
    /// Already validated against transaction root of header
    Validated(B),
    /// Still needs to be validated against header
    PendingValidation(WithPeerId<B>),
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
#[expect(missing_debug_implementations)]
pub struct FetchFullBlockRangeFuture<Client>
where
    Client: BlockClient,
{
    /// The client used to fetch headers and bodies.
    client: Client,
    /// The consensus instance used to validate the blocks.
    consensus: Arc<dyn Consensus<Client::Block>>,
    /// The block hash to start fetching from (inclusive).
    start_hash: B256,
    /// How many blocks to fetch: `len([start_hash, ..]) == count`
    count: u64,
    /// Requests for headers and bodies that are in progress.
    request: FullBlockRangeRequest<Client>,
    /// Fetched headers.
    headers: Option<Vec<SealedHeader<Client::Header>>>,
    /// The next headers to request bodies for. This is drained as responses are received.
    pending_headers: VecDeque<SealedHeader<Client::Header>>,
    /// The bodies that have been received so far.
    bodies: HashMap<SealedHeader<Client::Header>, BodyResponse<Client::Body>>,
}

impl<Client> FetchFullBlockRangeFuture<Client>
where
    Client: BlockClient<Header: Debug + BlockHeader + Sealable + Clone + Hash + Eq>,
{
    /// Returns whether or not the bodies map is fully populated with requested headers and bodies.
    fn is_bodies_complete(&self) -> bool {
        self.bodies.len() == self.count as usize
    }

    /// Inserts a block body, matching it with the `next_header`.
    ///
    /// Note: this assumes the response matches the next header in the queue.
    fn insert_body(&mut self, body_response: BodyResponse<Client::Body>) {
        if let Some(header) = self.pending_headers.pop_front() {
            self.bodies.insert(header, body_response);
        }
    }

    /// Inserts multiple block bodies.
    fn insert_bodies(&mut self, bodies: impl IntoIterator<Item = BodyResponse<Client::Body>>) {
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
    fn take_blocks(&mut self) -> Option<Vec<SealedBlock<Client::Block>>> {
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
                        if let Err(err) =
                            self.consensus.validate_body_against_header(resp.data(), header)
                        {
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

                valid_responses
                    .push(SealedBlock::<Client::Block>::from_sealed_parts(header.clone(), body));
            }
        }

        if needs_retry {
            // put response hashes back into bodies map since we aren't returning them as a
            // response
            for block in valid_responses {
                let (header, body) = block.split_sealed_header_body();
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

    fn on_headers_response(&mut self, headers: WithPeerId<Vec<Client::Header>>) {
        let (peer, mut headers_falling) =
            headers.map(|h| h.into_iter().map(SealedHeader::seal_slow).collect::<Vec<_>>()).split();

        // fill in the response if it's the correct length
        if headers_falling.len() == self.count as usize {
            // sort headers from highest to lowest block number
            headers_falling.sort_unstable_by_key(|h| Reverse(h.number()));

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
    Client: BlockClient<Header: Debug + BlockHeader + Sealable + Clone + Hash + Eq> + 'static,
{
    type Output = Vec<SealedBlock<Client::Block>>;

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
                    if this.request.bodies.is_none() && !this.is_bodies_complete() {
                        // no pending bodies request (e.g., request error), retry remaining bodies
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
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<RangeResponseResult<Client::Header, Client::Body>> {
        if let Some(fut) = Pin::new(&mut self.headers).as_pin_mut() &&
            let Poll::Ready(res) = fut.poll(cx)
        {
            self.headers = None;
            return Poll::Ready(RangeResponseResult::Header(res))
        }

        if let Some(fut) = Pin::new(&mut self.bodies).as_pin_mut() &&
            let Poll::Ready(res) = fut.poll(cx)
        {
            self.bodies = None;
            return Poll::Ready(RangeResponseResult::Body(res))
        }

        Poll::Pending
    }
}

// The result of a request for headers or block bodies. This is yielded by the
// `FullBlockRangeRequest` future.
enum RangeResponseResult<H, B> {
    Header(PeerRequestResult<Vec<H>>),
    Body(PeerRequestResult<Vec<B>>),
}

/// A headers+bodies client implementation that does nothing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NoopFullBlockClient<Net = EthNetworkPrimitives>(PhantomData<Net>);

/// Implements the `DownloadClient` trait for the `NoopFullBlockClient` struct.
impl<Net> DownloadClient for NoopFullBlockClient<Net>
where
    Net: Debug + Send + Sync,
{
    /// Reports a bad message received from a peer.
    ///
    /// # Arguments
    ///
    /// * `_peer_id` - Identifier for the peer sending the bad message (unused in this
    ///   implementation).
    fn report_bad_message(&self, _peer_id: PeerId) {}

    /// Retrieves the number of connected peers.
    ///
    /// # Returns
    ///
    /// The number of connected peers, which is always zero in this implementation.
    fn num_connected_peers(&self) -> usize {
        0
    }
}

/// Implements the `BodiesClient` trait for the `NoopFullBlockClient` struct.
impl<Net> BodiesClient for NoopFullBlockClient<Net>
where
    Net: NetworkPrimitives,
{
    type Body = Net::BlockBody;
    /// Defines the output type of the function.
    type Output = futures::future::Ready<PeerRequestResult<Vec<Self::Body>>>;

    /// Retrieves block bodies based on provided hashes and priority.
    ///
    /// # Arguments
    ///
    /// * `_hashes` - A vector of block hashes (unused in this implementation).
    /// * `_priority` - Priority level for block body retrieval (unused in this implementation).
    ///
    /// # Returns
    ///
    /// A future containing an empty vector of block bodies and a randomly generated `PeerId`.
    fn get_block_bodies_with_priority_and_range_hint(
        &self,
        _hashes: Vec<B256>,
        _priority: Priority,
        _range_hint: Option<RangeInclusive<u64>>,
    ) -> Self::Output {
        // Create a future that immediately returns an empty vector of block bodies and a random
        // PeerId.
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), vec![])))
    }
}

impl<Net> HeadersClient for NoopFullBlockClient<Net>
where
    Net: NetworkPrimitives,
{
    type Header = Net::BlockHeader;
    /// The output type representing a future containing a peer request result with a vector of
    /// headers.
    type Output = futures::future::Ready<PeerRequestResult<Vec<Self::Header>>>;

    /// Retrieves headers with a specified priority level.
    ///
    /// This implementation does nothing and returns an empty vector of headers.
    ///
    /// # Arguments
    ///
    /// * `_request` - A request for headers (unused in this implementation).
    /// * `_priority` - The priority level for the headers request (unused in this implementation).
    ///
    /// # Returns
    ///
    /// Always returns a ready future with an empty vector of headers wrapped in a
    /// `PeerRequestResult`.
    fn get_headers_with_priority(
        &self,
        _request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        futures::future::ready(Ok(WithPeerId::new(PeerId::random(), vec![])))
    }
}

impl<Net> BlockClient for NoopFullBlockClient<Net>
where
    Net: NetworkPrimitives,
{
    type Block = Net::Block;
}

impl<Net> Default for NoopFullBlockClient<Net> {
    fn default() -> Self {
        Self(PhantomData::<Net>)
    }
}

#[cfg(test)]
mod tests {
    use reth_ethereum_primitives::BlockBody;

    use super::*;
    use crate::{error::RequestError, test_utils::TestFullBlockClient};
    use alloy_primitives::Bytes;
    use parking_lot::Mutex;
    use std::{
        collections::HashMap,
        ops::Range,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
    };
    use tokio::time::{timeout, Duration};

    // RLP encoding for an empty list.
    const EMPTY_LIST_CODE: u8 = 0xc0;

    #[tokio::test]
    async fn download_single_full_block() {
        let client = TestFullBlockClient::default();
        let header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block(header.hash()).await;
        assert_eq!(received, SealedBlock::from_sealed_parts(header, body));
    }

    #[tokio::test]
    async fn download_single_full_block_range() {
        let client = TestFullBlockClient::default();
        let header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        client.insert(header.clone(), body.clone());
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::from_sealed_parts(header, body));
    }

    #[tokio::test]
    async fn download_single_full_block_with_access_lists() {
        let client = FullBlockWithAccessListsClient::default();
        let header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        let access_list = Bytes::from_static(&[EMPTY_LIST_CODE]);
        client.insert(header.clone(), body.clone(), access_list.clone());

        let request_count = Arc::clone(&client.access_list_requests);
        let client = FullBlockClient::test_client(client);

        let (received_block, received_access_lists) =
            client.get_full_block_with_access_lists(header.hash()).await;

        assert_eq!(received_block, SealedBlock::from_sealed_parts(header, body));
        assert_eq!(received_access_lists, BlockAccessLists(vec![access_list]));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_single_full_block_with_access_lists_retries_after_invalid_response() {
        let client = FullBlockWithAccessListsClient::default();
        client.empty_first_response.store(true, Ordering::SeqCst);

        let header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        let access_list = Bytes::from_static(&[EMPTY_LIST_CODE]);
        client.insert(header.clone(), body.clone(), access_list.clone());

        let request_count = Arc::clone(&client.access_list_requests);
        let bad_messages = Arc::clone(&client.bad_messages);
        let client = FullBlockClient::test_client(client);

        let (received_block, received_access_lists) =
            timeout(Duration::from_secs(1), client.get_full_block_with_access_lists(header.hash()))
                .await
                .expect("access list request retry should complete");

        assert_eq!(received_block, SealedBlock::from_sealed_parts(header, body));
        assert_eq!(received_access_lists, BlockAccessLists(vec![access_list]));
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        assert_eq!(bad_messages.load(Ordering::SeqCst), 1);
    }

    /// Inserts headers and returns the last header and block body.
    fn insert_headers_into_client(
        client: &TestFullBlockClient,
        range: Range<usize>,
    ) -> (SealedHeader, BlockBody) {
        let mut sealed_header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        for _ in range {
            let (mut header, hash) = sealed_header.split();
            // update to the next header
            header.parent_hash = hash;
            header.number += 1;

            sealed_header = SealedHeader::seal_slow(header);

            client.insert(sealed_header.clone(), body.clone());
        }

        (sealed_header, body)
    }

    #[derive(Clone, Debug)]
    struct FullBlockWithAccessListsClient {
        inner: TestFullBlockClient,
        access_lists: Arc<Mutex<HashMap<B256, Bytes>>>,
        access_list_requests: Arc<AtomicUsize>,
        access_list_soft_limit: Arc<AtomicUsize>,
        extra_access_list_entries: Arc<AtomicUsize>,
        unsupported_access_lists: Arc<AtomicBool>,
        bad_messages: Arc<AtomicUsize>,
        empty_first_response: Arc<AtomicBool>,
    }

    impl Default for FullBlockWithAccessListsClient {
        fn default() -> Self {
            Self {
                inner: TestFullBlockClient::default(),
                access_lists: Arc::new(Mutex::new(HashMap::default())),
                access_list_requests: Arc::new(AtomicUsize::new(0)),
                access_list_soft_limit: Arc::new(AtomicUsize::new(usize::MAX)),
                extra_access_list_entries: Arc::new(AtomicUsize::new(0)),
                unsupported_access_lists: Arc::new(AtomicBool::new(false)),
                bad_messages: Arc::new(AtomicUsize::new(0)),
                empty_first_response: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl FullBlockWithAccessListsClient {
        fn insert(&self, header: SealedHeader, body: BlockBody, access_list: Bytes) {
            self.inner.insert(header.clone(), body);
            self.access_lists.lock().insert(header.hash(), access_list);
        }

        fn set_access_list_soft_limit(&self, limit: usize) {
            self.access_list_soft_limit.store(limit, Ordering::SeqCst);
        }

        fn set_extra_access_list_entries(&self, count: usize) {
            self.extra_access_list_entries.store(count, Ordering::SeqCst);
        }

        fn set_access_lists_unsupported(&self, unsupported: bool) {
            self.unsupported_access_lists.store(unsupported, Ordering::SeqCst);
        }
    }

    /// Inserts headers with block access lists and returns the last header and block body.
    fn insert_headers_with_access_lists_into_client(
        client: &FullBlockWithAccessListsClient,
        range: Range<usize>,
    ) -> (SealedHeader, BlockBody) {
        let mut sealed_header: SealedHeader = SealedHeader::default();
        let body = BlockBody::default();
        for block_idx in range {
            let (mut header, hash) = sealed_header.split();
            header.parent_hash = hash;
            header.number += 1;

            sealed_header = SealedHeader::seal_slow(header);
            let access_list = Bytes::from(vec![EMPTY_LIST_CODE, block_idx as u8]);

            client.insert(sealed_header.clone(), body.clone(), access_list);
        }

        (sealed_header, body)
    }

    impl DownloadClient for FullBlockWithAccessListsClient {
        fn report_bad_message(&self, peer_id: PeerId) {
            self.bad_messages.fetch_add(1, Ordering::SeqCst);
            self.inner.report_bad_message(peer_id);
        }

        fn num_connected_peers(&self) -> usize {
            self.inner.num_connected_peers()
        }
    }

    impl HeadersClient for FullBlockWithAccessListsClient {
        type Header = <TestFullBlockClient as HeadersClient>::Header;
        type Output = <TestFullBlockClient as HeadersClient>::Output;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            priority: Priority,
        ) -> Self::Output {
            self.inner.get_headers_with_priority(request, priority)
        }
    }

    impl BodiesClient for FullBlockWithAccessListsClient {
        type Body = <TestFullBlockClient as BodiesClient>::Body;
        type Output = <TestFullBlockClient as BodiesClient>::Output;

        fn get_block_bodies_with_priority_and_range_hint(
            &self,
            hashes: Vec<B256>,
            priority: Priority,
            range_hint: Option<RangeInclusive<u64>>,
        ) -> Self::Output {
            self.inner.get_block_bodies_with_priority_and_range_hint(hashes, priority, range_hint)
        }
    }

    impl BlockAccessListsClient for FullBlockWithAccessListsClient {
        type Output = futures::future::Ready<PeerRequestResult<BlockAccessLists>>;

        fn get_block_access_lists_with_priority_and_requirement(
            &self,
            hashes: Vec<B256>,
            _priority: Priority,
            _requirement: crate::block_access_lists::client::BalRequirement,
        ) -> Self::Output {
            self.access_list_requests.fetch_add(1, Ordering::SeqCst);

            if self.unsupported_access_lists.load(Ordering::SeqCst) {
                return futures::future::ready(Err(RequestError::UnsupportedCapability))
            }

            if self.empty_first_response.swap(false, Ordering::SeqCst) {
                return futures::future::ready(Ok(WithPeerId::new(
                    PeerId::random(),
                    BlockAccessLists(Vec::new()),
                )))
            }

            let mut access_lists: Vec<_> = hashes
                .into_iter()
                .take(self.access_list_soft_limit.load(Ordering::SeqCst))
                .map(|hash| {
                    self.access_lists
                        .lock()
                        .get(&hash)
                        .cloned()
                        .unwrap_or_else(|| Bytes::from_static(&[EMPTY_LIST_CODE]))
                })
                .collect();
            for _ in 0..self.extra_access_list_entries.load(Ordering::SeqCst) {
                access_lists.push(Bytes::from_static(&[EMPTY_LIST_CODE]));
            }

            futures::future::ready(Ok(WithPeerId::new(
                PeerId::random(),
                BlockAccessLists(access_lists),
            )))
        }
    }

    impl BlockClient for FullBlockWithAccessListsClient {
        type Block = reth_ethereum_primitives::Block;
    }

    #[derive(Clone, Debug)]
    struct FailingBodiesClient {
        inner: TestFullBlockClient,
        fail_on: usize,
        body_requests: Arc<AtomicUsize>,
    }

    impl FailingBodiesClient {
        fn new(inner: TestFullBlockClient, fail_on: usize) -> Self {
            Self { inner, fail_on, body_requests: Arc::new(AtomicUsize::new(0)) }
        }
    }

    impl DownloadClient for FailingBodiesClient {
        fn report_bad_message(&self, peer_id: PeerId) {
            self.inner.report_bad_message(peer_id);
        }

        fn num_connected_peers(&self) -> usize {
            self.inner.num_connected_peers()
        }
    }

    impl HeadersClient for FailingBodiesClient {
        type Header = <TestFullBlockClient as HeadersClient>::Header;
        type Output = <TestFullBlockClient as HeadersClient>::Output;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            priority: Priority,
        ) -> Self::Output {
            self.inner.get_headers_with_priority(request, priority)
        }
    }

    impl BodiesClient for FailingBodiesClient {
        type Body = <TestFullBlockClient as BodiesClient>::Body;
        type Output = <TestFullBlockClient as BodiesClient>::Output;

        fn get_block_bodies_with_priority_and_range_hint(
            &self,
            hashes: Vec<B256>,
            priority: Priority,
            range_hint: Option<RangeInclusive<u64>>,
        ) -> Self::Output {
            let attempt = self.body_requests.fetch_add(1, Ordering::SeqCst);
            if attempt == self.fail_on {
                return futures::future::ready(Err(RequestError::Timeout))
            }

            self.inner.get_block_bodies_with_priority_and_range_hint(hashes, priority, range_hint)
        }
    }

    impl BlockClient for FailingBodiesClient {
        type Block = reth_ethereum_primitives::Block;
    }

    #[tokio::test]
    async fn download_full_block_range() {
        let client = TestFullBlockClient::default();
        let (header, body) = insert_headers_into_client(&client, 0..50);
        let client = FullBlockClient::test_client(client);

        let received = client.get_full_block_range(header.hash(), 1).await;
        let received = received.first().expect("response should include a block");
        assert_eq!(*received, SealedBlock::from_sealed_parts(header.clone(), body));

        let received = client.get_full_block_range(header.hash(), 10).await;
        assert_eq!(received.len(), 10);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.number, expected_number);
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
        assert_eq!(*received, SealedBlock::from_sealed_parts(header.clone(), body));

        let received = client.get_full_block_range(header.hash(), 50).await;
        assert_eq!(received.len(), 50);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.number, expected_number);
        }
    }

    #[tokio::test]
    async fn download_full_block_range_retries_after_body_error() {
        let mut client = TestFullBlockClient::default();
        client.set_soft_limit(2);
        let (header, _) = insert_headers_into_client(&client, 0..3);

        let client = FailingBodiesClient::new(client, 1);
        let body_requests = Arc::clone(&client.body_requests);
        let client = FullBlockClient::test_client(client);

        let received =
            timeout(Duration::from_secs(1), client.get_full_block_range(header.hash(), 3))
                .await
                .expect("body request retry should complete");

        assert_eq!(received.len(), 3);
        assert_eq!(body_requests.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn download_full_block_range_with_access_lists() {
        let client = FullBlockWithAccessListsClient::default();
        let (header, _) = insert_headers_with_access_lists_into_client(&client, 0..3);

        let access_lists = Arc::clone(&client.access_lists);
        let request_count = Arc::clone(&client.access_list_requests);
        let client = FullBlockClient::test_client(client);

        let response = timeout(
            Duration::from_secs(1),
            client.get_full_block_range_with_optional_access_lists(header.hash(), 3),
        )
        .await
        .expect("range request should complete");

        let (blocks, received_access_lists) = response;
        assert_eq!(blocks.len(), 3);
        let expected = {
            let access_lists = access_lists.lock();
            blocks
                .iter()
                .map(|block| access_lists.get(&block.hash()).cloned().expect("access list exists"))
                .collect::<Vec<_>>()
        };
        assert_eq!(received_access_lists, Some(BlockAccessLists(expected)));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_full_block_range_with_access_lists_preserves_empty_response() {
        let client = FullBlockWithAccessListsClient::default();
        client.empty_first_response.store(true, Ordering::SeqCst);
        let (header, _) = insert_headers_with_access_lists_into_client(&client, 0..3);

        let request_count = Arc::clone(&client.access_list_requests);
        let client = FullBlockClient::test_client(client);

        let response = timeout(
            Duration::from_secs(1),
            client.get_full_block_range_with_optional_access_lists(header.hash(), 3),
        )
        .await
        .expect("range request should complete without access lists");

        let (blocks, received_access_lists) = response;
        assert_eq!(blocks.len(), 3);
        assert_eq!(received_access_lists, Some(BlockAccessLists(Vec::new())));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_full_block_range_with_access_lists_preserves_short_response() {
        let client = FullBlockWithAccessListsClient::default();
        client.set_access_list_soft_limit(2);
        let (header, _) = insert_headers_with_access_lists_into_client(&client, 0..5);

        let access_lists = Arc::clone(&client.access_lists);
        let request_count = Arc::clone(&client.access_list_requests);
        let client = FullBlockClient::test_client(client);

        let (blocks, received_access_lists) = timeout(
            Duration::from_secs(1),
            client.get_full_block_range_with_optional_access_lists(header.hash(), 5),
        )
        .await
        .expect("range request should complete without access lists");

        assert_eq!(blocks.len(), 5);
        let expected = {
            let access_lists = access_lists.lock();
            blocks
                .iter()
                .take(2)
                .map(|block| access_lists.get(&block.hash()).cloned().expect("access list exists"))
                .collect::<Vec<_>>()
        };
        assert_eq!(received_access_lists, Some(BlockAccessLists(expected)));
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_full_block_range_with_access_lists_returns_none_when_unavailable() {
        let client = FullBlockWithAccessListsClient::default();
        client.set_access_lists_unsupported(true);
        let (header, _) = insert_headers_with_access_lists_into_client(&client, 0..3);

        let request_count = Arc::clone(&client.access_list_requests);
        let client = FullBlockClient::test_client(client);

        let (blocks, received_access_lists) = timeout(
            Duration::from_secs(1),
            client.get_full_block_range_with_optional_access_lists(header.hash(), 3),
        )
        .await
        .expect("range request should complete without access lists");

        assert_eq!(blocks.len(), 3);
        assert!(received_access_lists.is_none());
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_full_block_range_with_access_lists_rejects_long_response() {
        let client = FullBlockWithAccessListsClient::default();
        client.set_extra_access_list_entries(1);
        let (header, _) = insert_headers_with_access_lists_into_client(&client, 0..3);

        let request_count = Arc::clone(&client.access_list_requests);
        let bad_messages = Arc::clone(&client.bad_messages);
        let client = FullBlockClient::test_client(client);

        let (blocks, received_access_lists) = timeout(
            Duration::from_secs(1),
            client.get_full_block_range_with_optional_access_lists(header.hash(), 3),
        )
        .await
        .expect("range request should complete without access lists");

        assert_eq!(blocks.len(), 3);
        assert!(received_access_lists.is_none());
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(bad_messages.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn download_full_block_range_with_invalid_header() {
        let client = TestFullBlockClient::default();
        let range_length: usize = 3;
        let (header, _) = insert_headers_into_client(&client, 0..range_length);

        let test_consensus = reth_consensus::test_utils::TestConsensus::default();
        test_consensus.set_fail_validation(true);
        test_consensus.set_fail_body_against_header(false);
        let client = FullBlockClient::new(client, Arc::new(test_consensus));

        let received = client.get_full_block_range(header.hash(), range_length as u64).await;

        assert_eq!(received.len(), range_length);
        for (i, block) in received.iter().enumerate() {
            let expected_number = header.number - i as u64;
            assert_eq!(block.number, expected_number);
        }
    }
}
