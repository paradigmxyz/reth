//! Bounded peer retrieval of separately stored proofs for the experimental EIP-8288 profile.

use crate::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};
use alloy_eips::eip8288::MAX_DEPENDENCY_PROOF_BYTES;
use alloy_primitives::{bytes::BytesMut, keccak256, map::B256Map, Bytes, B256};
use futures::{future::BoxFuture, stream::FuturesUnordered, FutureExt, Stream, StreamExt};
use reth_eth_wire::{
    capability::SharedCapabilities,
    multiplex::ProtocolConnection,
    protocol::{Protocol, ProtocolIngressLimits},
    Capability,
};
use reth_network_api::{Direction, PeerId};
use reth_storage_api::{DependencyProofFetcher, DependencyProofProvider, DependencyProofRequest};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    net::SocketAddr,
    ops::Bound::{Excluded, Unbounded},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use tracing::{debug, error, warn};

const CAPACITY: usize = 16;
const MAX_PEERS: usize = 128;
const MAX_SERVING: usize = 2;
const MAX_MESSAGE: usize = MAX_DEPENDENCY_PROOF_BYTES + 34;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PEER_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);

/// The node's bounded source of commitment-authenticated peer proofs.
#[derive(Debug)]
struct PeerDependencyProofFetcher(mpsc::Sender<Request>);

impl DependencyProofFetcher for PeerDependencyProofFetcher {
    fn fetch(
        &self,
        request: DependencyProofRequest,
    ) -> BoxFuture<'static, reth_storage_api::errors::provider::ProviderResult<Option<Bytes>>> {
        let sender = self.0.clone();
        async move {
            let (response, receiver) = oneshot::channel();
            if sender.try_send(Request { request, response }).is_err() {
                return Ok(None)
            }
            Ok(
                match tokio::time::timeout(REQUEST_TIMEOUT + Duration::from_secs(1), receiver).await
                {
                    Ok(Ok(proof)) => proof,
                    Ok(Err(_)) | Err(_) => None,
                },
            )
        }
        .boxed()
    }
}

/// Negotiates a separate proof capability, including when transaction gossip is disabled.
#[derive(Clone, Debug)]
pub struct DependencyProofProtocolHandler(mpsc::Sender<Event>);

impl DependencyProofProtocolHandler {
    /// Creates the protocol and downloader. Run the returned task for the node's lifetime.
    pub fn new<P: DependencyProofProvider + Clone + 'static>(
        provider: P,
    ) -> (Self, Arc<dyn DependencyProofFetcher>, BoxFuture<'static, ()>) {
        let (events, incoming) = mpsc::channel(CAPACITY);
        let (requests, downloads) = mpsc::channel(CAPACITY);
        (
            Self(events),
            Arc::new(PeerDependencyProofFetcher(requests)),
            run(provider, incoming, downloads).boxed(),
        )
    }
}

impl ProtocolHandler for DependencyProofProtocolHandler {
    type ConnectionHandler = Self;
    fn on_incoming(&self, _: SocketAddr) -> Option<Self> {
        Some(self.clone())
    }
    fn on_outgoing(&self, _: SocketAddr, _: PeerId) -> Option<Self> {
        Some(self.clone())
    }
}

impl ConnectionHandler for DependencyProofProtocolHandler {
    type Connection = DependencyProofConnection;
    fn protocol(&self) -> Protocol {
        Protocol::new(Capability::new_static("dep-proof", 1), 2)
    }
    fn inbound_limits(&self) -> ProtocolIngressLimits {
        ProtocolIngressLimits::new(MAX_MESSAGE)
            .with_max_buffered_bytes(MAX_MESSAGE * 2)
            .with_max_buffered_messages(2)
    }
    fn on_unsupported_by_peer(
        self,
        _: &SharedCapabilities,
        _: Direction,
        _: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }
    fn into_connection(
        self,
        _: Direction,
        peer: PeerId,
        connection: ProtocolConnection,
    ) -> Self::Connection {
        let (outgoing, receiver) = mpsc::channel(2);
        let admitted = self.0.try_send(Event::Opened(peer, outgoing)).is_ok();
        DependencyProofConnection { peer, connection, events: self.0, outgoing: receiver, admitted }
    }
}

/// A bounded connection for proof requests and responses.
pub struct DependencyProofConnection {
    peer: PeerId,
    connection: ProtocolConnection,
    events: mpsc::Sender<Event>,
    outgoing: mpsc::Receiver<BytesMut>,
    admitted: bool,
}

impl fmt::Debug for DependencyProofConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DependencyProofConnection")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl Stream for DependencyProofConnection {
    type Item = BytesMut;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if !this.admitted {
            return Poll::Ready(None)
        }
        if let Poll::Ready(message) = this.outgoing.poll_recv(cx) {
            return Poll::Ready(message)
        }
        for _ in 0..16 {
            match this.connection.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(message)) => {
                    if !valid_message(&message) {
                        return Poll::Ready(None)
                    }
                    match this.events.try_send(Event::Message(this.peer, message.freeze().into())) {
                        Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => return Poll::Ready(None),
                    }
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[derive(Debug)]
struct Request {
    request: DependencyProofRequest,
    response: oneshot::Sender<Option<Bytes>>,
}

#[derive(Debug)]
enum Event {
    Opened(PeerId, mpsc::Sender<BytesMut>),
    Message(PeerId, Bytes),
}

struct Peer {
    outgoing: mpsc::Sender<BytesMut>,
    next_serve: Instant,
    next_request: Instant,
}

struct Pending {
    request: DependencyProofRequest,
    responses: Vec<oneshot::Sender<Option<Bytes>>>,
    tried: BTreeSet<PeerId>,
    peer: Option<PeerId>,
    next_try: Instant,
    deadline: Instant,
}

const fn valid_message(message: &[u8]) -> bool {
    message.len() <= MAX_MESSAGE &&
        match message.first() {
            Some(0) => message.len() == 33,
            Some(1) => {
                message.len() >= 34 &&
                    (message[33] == 1 || (message[33] == 0 && message.len() == 34))
            }
            _ => false,
        }
}

fn authenticated(request: DependencyProofRequest, proof: &[u8]) -> bool {
    proof.len() <= MAX_DEPENDENCY_PROOF_BYTES && keccak256(proof) == request.commitment
}

fn encode_response(hash: B256, proof: Option<&[u8]>) -> BytesMut {
    let mut message = BytesMut::with_capacity(34 + proof.map_or(0, <[u8]>::len));
    message.extend_from_slice(&[1]);
    message.extend_from_slice(hash.as_slice());
    message.extend_from_slice(&[u8::from(proof.is_some())]);
    if let Some(proof) = proof {
        message.extend_from_slice(proof);
    }
    message
}

async fn run<P: DependencyProofProvider + Clone + 'static>(
    provider: P,
    mut events: mpsc::Receiver<Event>,
    mut requests: mpsc::Receiver<Request>,
) {
    let mut peers = BTreeMap::<PeerId, Peer>::new();
    let mut pending = B256Map::<Pending>::default();
    // Keep the cursor across fetch attempts so silent peers cannot monopolize every deadline.
    let mut last_peer = None;
    let mut serving = FuturesUnordered::<BoxFuture<'static, (PeerId, BytesMut)>>::new();
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(Request { request, response }) = request else { return };
                if let Some(existing) = pending.get_mut(&request.block_hash) {
                    if existing.request.commitment == request.commitment && existing.responses.len() < CAPACITY {
                        existing.responses.push(response);
                    }
                } else if pending.len() < CAPACITY {
                    pending.insert(request.block_hash, Pending {
                        request, responses: vec![response], tried: BTreeSet::new(), peer: None,
                        next_try: Instant::now(), deadline: Instant::now() + REQUEST_TIMEOUT,
                    });
                }
            },
            event = events.recv() => match event {
                None => return,
                Some(Event::Opened(id, outgoing)) => {
                    peers.retain(|_, peer| !peer.outgoing.is_closed());
                    if peers.len() < MAX_PEERS { peers.insert(id, Peer { outgoing, next_serve: Instant::now(), next_request: Instant::now() }); }
                },
                Some(Event::Message(id, message)) => {
                    if !peers.contains_key(&id) { continue }
                    let hash = B256::from_slice(&message[1..33]);
                    if message[0] == 0 {
                        if let Some(peer) = peers.get_mut(&id) && peer.next_serve <= Instant::now() && serving.len() < MAX_SERVING {
                            peer.next_serve = Instant::now() + REQUEST_INTERVAL;
                            let provider = provider.clone();
                            let span = tracing::Span::current();
                            serving.push(async move {
                                let proof = match tokio::task::spawn_blocking(move || {
                                    let _entered = span.enter();
                                    provider.dependency_proof(hash)
                                }).await {
                                    Ok(Ok(proof)) => proof.filter(|proof| proof.len() <= MAX_DEPENDENCY_PROOF_BYTES),
                                    Ok(Err(error)) => { error!(target: "reth::eip8288", %error, "Cannot serve dependency proof"); None },
                                    Err(error) => { error!(target: "reth::eip8288", %error, "Proof read worker failed"); None },
                                };
                                (id, encode_response(hash, proof.as_ref().map(|proof| proof.as_ref())))
                            }.boxed());
                        }
                    } else if let Some(download) = pending.get_mut(&hash) && download.peer == Some(id) {
                        if message[33] == 0 {
                            download.peer = None;
                            download.next_try = Instant::now();
                        } else if authenticated(download.request, &message[34..]) {
                            if let Some(download) = pending.remove(&hash) {
                                let proof = message.slice(34..);
                                for response in download.responses { if response.send(Some(proof.clone())).is_err() { debug!(target: "reth::eip8288", "Proof requester left"); } }
                            }
                        } else {
                            // Dropping the last sender closes the invalid peer's capability.
                            warn!(target: "reth::eip8288", peer = %id, %hash, "Dependency proof does not match its header");
                            peers.remove(&id);
                            download.peer = None;
                            download.next_try = Instant::now();
                        }
                    }
                }
            },
            Some((id, message)) = serving.next(), if !serving.is_empty() => {
                if let Some(peer) = peers.get(&id) && let Err(error) = peer.outgoing.try_send(message) {
                    debug!(target: "reth::eip8288", %error, "Proof response queue unavailable");
                }
            },
            _ = interval.tick() => {
                peers.retain(|_, peer| !peer.outgoing.is_closed());
                pending.retain(|_, download| {
                    download.responses.retain(|response| !response.is_closed());
                    !download.responses.is_empty() && download.deadline > Instant::now()
                });
                for download in pending.values_mut() {
                    if download.next_try > Instant::now() { continue }
                    download.peer = None;
                    // Availability and request failures are transient; revisit peers after a round.
                    if peers.keys().all(|id| download.tried.contains(id)) {
                        download.tried.clear();
                    }
                    let start = last_peer.map_or(Unbounded, Excluded);
                    if let Some(id) = peers.range((start, Unbounded)).chain(peers.iter())
                        .find(|(id, peer)| !download.tried.contains(*id) && peer.next_request <= Instant::now())
                        .map(|(id, _)| *id) && let Some(peer) = peers.get_mut(&id)
                    {
                        let mut message = BytesMut::with_capacity(33);
                        message.extend_from_slice(&[0]);
                        message.extend_from_slice(download.request.block_hash.as_slice());
                        match peer.outgoing.try_send(message) {
                            Ok(()) => {
                                last_peer = Some(id);
                                download.tried.insert(id);
                                download.peer = Some(id);
                                download.next_try = Instant::now() + PEER_TIMEOUT;
                                peer.next_request = Instant::now() + REQUEST_INTERVAL;
                            },
                            Err(error) => debug!(target: "reth::eip8288", %error, "Proof request queue unavailable"),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug)]
    struct EmptyProofs;

    impl DependencyProofProvider for EmptyProofs {
        fn dependency_proof(
            &self,
            _: B256,
        ) -> reth_storage_api::errors::provider::ProviderResult<Option<Bytes>> {
            Ok(None)
        }
        fn insert_dependency_proof(
            &self,
            _: B256,
            _: Bytes,
        ) -> reth_storage_api::errors::provider::ProviderResult<()> {
            Err(reth_storage_api::errors::provider::ProviderError::UnsupportedProvider)
        }
    }

    #[test]
    fn proof_messages_are_bounded_and_distinguish_empty_from_missing() {
        let mut message = vec![1; 34];
        assert!(valid_message(&message));
        message[33] = 0;
        assert!(valid_message(&message));
        message.push(0);
        assert!(!valid_message(&message));
        assert!(!valid_message(&vec![1; MAX_MESSAGE + 1]));
        assert!(!valid_message(&[0; 32]));
        assert!(valid_message(&[0; 33]));
    }

    #[test]
    fn downloaded_proofs_must_match_the_header_before_storage() {
        let proof = b"proof";
        let request =
            DependencyProofRequest { block_hash: B256::ZERO, commitment: keccak256(proof) };
        assert!(authenticated(request, proof));
        assert!(!authenticated(request, b"changed"));
        assert!(!authenticated(request, &vec![0; MAX_DEPENDENCY_PROOF_BYTES + 1]));
    }

    #[tokio::test]
    async fn single_peer_retries_unavailable_and_unresponsive_proofs() {
        for unavailable in [false, true] {
            let (handler, fetcher, driver) = DependencyProofProtocolHandler::new(EmptyProofs);
            let driver = tokio::spawn(driver);
            let peer = PeerId::repeat_byte(1);
            let (sender, mut messages) = mpsc::channel(2);
            handler.0.send(Event::Opened(peer, sender)).await.unwrap();
            let proof = Bytes::from_static(b"authenticated proof");
            let request = DependencyProofRequest {
                block_hash: B256::repeat_byte(3),
                commitment: keccak256(&proof),
            };
            let requester = Arc::clone(&fetcher);
            let started = Instant::now();
            let fetched = tokio::spawn(async move { requester.fetch(request).await });
            let message =
                tokio::time::timeout(PEER_TIMEOUT, messages.recv()).await.unwrap().unwrap();
            assert_eq!(&message[1..], request.block_hash.as_slice());
            if unavailable {
                handler
                    .0
                    .send(Event::Message(
                        peer,
                        encode_response(request.block_hash, None).freeze().into(),
                    ))
                    .await
                    .unwrap();
            }
            let message = tokio::time::timeout(PEER_TIMEOUT + REQUEST_INTERVAL, messages.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&message[1..], request.block_hash.as_slice());
            assert!(started.elapsed() >= if unavailable { REQUEST_INTERVAL } else { PEER_TIMEOUT });
            assert!(started.elapsed() < REQUEST_TIMEOUT);
            handler
                .0
                .send(Event::Message(
                    peer,
                    encode_response(request.block_hash, Some(&proof)).freeze().into(),
                ))
                .await
                .unwrap();
            assert_eq!(fetched.await.unwrap().unwrap(), Some(proof));
            driver.abort();
            assert!(driver.await.unwrap_err().is_cancelled());
        }
    }

    #[tokio::test]
    async fn consecutive_fetches_share_the_peer_cooldown() {
        let (handler, fetcher, driver) = DependencyProofProtocolHandler::new(EmptyProofs);
        let driver = tokio::spawn(driver);
        let peer = PeerId::repeat_byte(1);
        let (sender, mut messages) = mpsc::channel(2);
        handler.0.send(Event::Opened(peer, sender)).await.unwrap();
        let mut previous = None;
        for index in [1, 2] {
            let proof = Bytes::from_static(b"authenticated proof");
            let request = DependencyProofRequest {
                block_hash: B256::repeat_byte(index),
                commitment: keccak256(&proof),
            };
            let requester = Arc::clone(&fetcher);
            let started = Instant::now();
            let fetched = tokio::spawn(async move { requester.fetch(request).await });
            let message =
                tokio::time::timeout(PEER_TIMEOUT, messages.recv()).await.unwrap().unwrap();
            assert_eq!(&message[1..], request.block_hash.as_slice());
            if let Some(previous) = previous {
                assert!(Instant::now() - previous >= REQUEST_INTERVAL);
            }
            previous = Some(started);
            handler
                .0
                .send(Event::Message(
                    peer,
                    encode_response(request.block_hash, Some(&proof)).freeze().into(),
                ))
                .await
                .unwrap();
            assert_eq!(fetched.await.unwrap().unwrap(), Some(proof));
        }
        driver.abort();
        assert!(driver.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn unavailable_and_invalid_peers_are_retried_without_accepting_bad_bytes() {
        for invalid in [false, true] {
            let (handler, fetcher, driver) = DependencyProofProtocolHandler::new(EmptyProofs);
            let driver = tokio::spawn(driver);
            let first = PeerId::repeat_byte(1);
            let second = PeerId::repeat_byte(2);
            let (first_sender, mut first_messages) = mpsc::channel(2);
            let (second_sender, mut second_messages) = mpsc::channel(2);
            handler.0.send(Event::Opened(first, first_sender)).await.unwrap();
            handler.0.send(Event::Opened(second, second_sender)).await.unwrap();
            let proof = Bytes::from_static(b"authenticated proof");
            let request = DependencyProofRequest {
                block_hash: B256::repeat_byte(3),
                commitment: keccak256(&proof),
            };
            let fetched = tokio::spawn(fetcher.fetch(request));
            let message = tokio::time::timeout(Duration::from_secs(2), first_messages.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(message[0], 0);
            assert_eq!(&message[1..], request.block_hash.as_slice());
            handler
                .0
                .send(Event::Message(
                    first,
                    encode_response(request.block_hash, invalid.then_some(b"wrong".as_slice()))
                        .freeze()
                        .into(),
                ))
                .await
                .unwrap();
            let message = tokio::time::timeout(Duration::from_secs(2), second_messages.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&message[1..], request.block_hash.as_slice());
            handler
                .0
                .send(Event::Message(
                    second,
                    encode_response(request.block_hash, Some(&proof)).freeze().into(),
                ))
                .await
                .unwrap();
            assert_eq!(fetched.await.unwrap().unwrap(), Some(proof));
            if invalid {
                assert_eq!(first_messages.recv().await, None);
            } else {
                assert!(!first_messages.is_closed());
            }
            driver.abort();
            assert!(driver.await.unwrap_err().is_cancelled());
        }
    }

    #[tokio::test]
    async fn fetch_retries_reach_peers_beyond_the_first_deadline() {
        let (handler, fetcher, driver) = DependencyProofProtocolHandler::new(EmptyProofs);
        let driver = tokio::spawn(driver);
        let proof = Bytes::from_static(b"authenticated proof");
        let request = DependencyProofRequest {
            block_hash: B256::repeat_byte(7),
            commitment: keccak256(&proof),
        };
        let mut peers = Vec::new();
        for index in 1..=6 {
            let id = PeerId::repeat_byte(index);
            let (outgoing, mut messages) = mpsc::channel(2);
            handler.0.send(Event::Opened(id, outgoing)).await.unwrap();
            let handler = handler.clone();
            let proof = proof.clone();
            peers.push(tokio::spawn(async move {
                let mut requests = 0;
                while let Some(message) = messages.recv().await {
                    assert_eq!(message[0], 0);
                    assert_eq!(&message[1..], request.block_hash.as_slice());
                    requests += 1;
                    if index == 6 {
                        handler
                            .0
                            .send(Event::Message(
                                id,
                                encode_response(request.block_hash, Some(&proof)).freeze().into(),
                            ))
                            .await
                            .unwrap();
                    }
                }
                requests
            }));
        }

        // Five silent peers exhaust the first window; the next fetch must start after them.
        assert_eq!(fetcher.fetch(request).await.unwrap(), None);
        assert_eq!(fetcher.fetch(request).await.unwrap(), Some(proof));

        driver.abort();
        assert!(driver.await.unwrap_err().is_cancelled());
        for peer in peers {
            assert_eq!(peer.await.unwrap(), 1);
        }
    }
}
