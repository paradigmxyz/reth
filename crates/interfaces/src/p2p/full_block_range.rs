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
mod tests {}
