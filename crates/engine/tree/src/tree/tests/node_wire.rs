//! A download client whose requests and responses traverse the production ETH wire codec.

use alloy_consensus::Header;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{B256, B512};
use futures::{future::BoxFuture, stream::FuturesUnordered, FutureExt, SinkExt, StreamExt};
use reth_chainspec::{ForkFilter, Head};
use reth_eth_wire::{
    message::RequestPair,
    simulation::{authenticated_pair, LinkConfig, LinkStats, SimulatedLink},
    BlockBodies, BlockHeaders, EthMessage, EthNetworkPrimitives, GetBlockBodies, GetBlockHeaders,
    UnauthedEthStream, UnifiedStatus,
};
use reth_ethereum_primitives::{Block, BlockBody};
use reth_network_p2p::{
    bodies::client::BodiesFut,
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    headers::client::{HeadersFut, HeadersRequest},
    priority::Priority,
    BlockClient, BodiesClient, HeadersClient,
};
use reth_primitives_traits::SealedBlock;
use reth_tasks::{TaskHandle, TaskRuntime};
use std::{
    collections::BTreeMap,
    future::Future,
    ops::RangeInclusive,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Notify};

/// A downloader handle for one established ETH peer.
#[derive(Clone, Debug)]
pub(super) struct WireBlockClient {
    runtime: TaskRuntime,
    commands: mpsc::UnboundedSender<EthMessage>,
    state: Arc<PeerState>,
}

impl reth_network_p2p::BlockAccessListsClient for WireBlockClient {
    type Output = futures::future::Ready<PeerRequestResult<reth_eth_wire::BlockAccessLists>>;

    fn get_block_access_lists_with_priority_and_requirement(
        &self,
        _hashes: Vec<B256>,
        _priority: Priority,
        requirement: reth_network_p2p::block_access_lists::client::BalRequirement,
    ) -> Self::Output {
        // This peer exercises pre-Amsterdam blocks without BAL support.
        assert_eq!(
            requirement,
            reth_network_p2p::block_access_lists::client::BalRequirement::Optional
        );
        futures::future::ready(Err(RequestError::UnsupportedCapability))
    }
}

impl WireBlockClient {
    pub(super) async fn new(
        runtime: TaskRuntime,
        blocks: Arc<Mutex<BTreeMap<u64, SealedBlock<Block>>>>,
        config: LinkConfig,
    ) -> (Self, WirePeer) {
        let (left, right, link) = authenticated_pair(runtime.clone(), config)
            .await
            .expect("simulated authenticated RLPx handshake failed");
        let peer_id = left.inner().remote_id();
        let genesis = blocks.lock().unwrap().get(&0).map_or(B256::ZERO, SealedBlock::hash);
        let forks = ForkFilter::new(Head::default(), genesis, 0, Vec::new());
        let status = UnifiedStatus {
            genesis,
            blockhash: genesis,
            forkid: forks.current(),
            ..Default::default()
        };
        let ((client, _), (server, _)) = futures::future::try_join(
            UnauthedEthStream::new(left)
                .handshake_without_timeout::<EthNetworkPrimitives>(status, forks.clone()),
            UnauthedEthStream::new(right)
                .handshake_without_timeout::<EthNetworkPrimitives>(status, forks),
        )
        .await
        .expect("simulated ETH status handshake failed");
        let (mut client_sink, mut client_stream) = client.split();
        let (mut server_sink, mut server_stream) = server.split();
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let (responses, mut response_rx) = mpsc::channel(16);
        let state = Arc::new(PeerState { peer_id, ..Default::default() });
        let mut actors = Vec::new();

        let writer_state = Arc::clone(&state);
        actors.push(
            runtime
                .spawn("wire_client_send", async move {
                    loop {
                        let message = tokio::select! {
                            biased;
                            _ = writer_state.wait_stopped() => break,
                            message = command_rx.recv() => message,
                        };
                        let Some(message) = message else { break };
                        let sent = tokio::select! {
                            biased;
                            _ = writer_state.wait_stopped() => break,
                            sent = client_sink.send(message) => sent,
                        };
                        if sent.is_err() {
                            break;
                        }
                    }
                    writer_state.close();
                })
                .abort_on_drop(),
        );

        let reader_state = Arc::clone(&state);
        actors.push(
            runtime
                .spawn("wire_client_receive", async move {
                    loop {
                        let message = tokio::select! {
                            biased;
                            _ = reader_state.wait_stopped() => break,
                            message = client_stream.next() => message,
                        };
                        let Some(Ok(message)) = message else { break };
                        match message {
                            EthMessage::BlockHeaders(pair) => {
                                reader_state.record(WireEvent::Response {
                                    request_id: pair.request_id,
                                    headers: true,
                                });
                                if let Some(pending) =
                                    reader_state.pending.lock().unwrap().remove(&pair.request_id)
                                {
                                    match pending {
                                        PendingRequest::Headers(sender) => {
                                            let _ = sender.send(Ok(pair.message.0));
                                        }
                                        other => other.fail(RequestError::BadResponse),
                                    }
                                }
                            }
                            EthMessage::BlockBodies(pair) => {
                                reader_state.record(WireEvent::Response {
                                    request_id: pair.request_id,
                                    headers: false,
                                });
                                if let Some(pending) =
                                    reader_state.pending.lock().unwrap().remove(&pair.request_id)
                                {
                                    match pending {
                                        PendingRequest::Bodies(sender) => {
                                            let _ = sender.send(Ok(pair.message.0));
                                        }
                                        other => other.fail(RequestError::BadResponse),
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    reader_state.close();
                })
                .abort_on_drop(),
        );

        let server_state = Arc::clone(&state);
        let server_runtime = runtime.clone();
        actors.push(runtime.spawn("wire_peer_requests", async move {
            let mut pending = FuturesUnordered::<BoxFuture<'static, EthMessage>>::new();
            loop {
                tokio::select! {
                    biased;
                    _ = server_state.wait_stopped() => break,
                    response = pending.next(), if !pending.is_empty() => {
                        if responses.send(response.unwrap()).await.is_err() { break; }
                    }
                    request = server_stream.next(), if pending.len() < 16 => {
                        let Some(Ok(request)) = request else { break };
                        let (request_id, headers, response) = match request {
                            EthMessage::GetBlockHeaders(pair) => (
                                pair.request_id,
                                true,
                                EthMessage::BlockHeaders(RequestPair {
                                    request_id: pair.request_id,
                                    message: BlockHeaders(serve_headers(&blocks.lock().unwrap(), pair.message)),
                                }),
                            ),
                            EthMessage::GetBlockBodies(pair) => {
                                let blocks = blocks.lock().unwrap();
                                let bodies = pair.message.0.iter().filter_map(|hash| {
                                    blocks.values().find(|block| block.hash() == *hash).map(|block| block.body().clone())
                                }).collect();
                                (pair.request_id, false, EthMessage::BlockBodies(RequestPair {
                                    request_id: pair.request_id,
                                    message: BlockBodies(bodies),
                                }))
                            }
                            _ => break,
                        };
                        server_state.record(WireEvent::Request { request_id, headers });
                        let delay = Duration::from_millis((2 - request_id.wrapping_add(config.seed) % 3) * 10);
                        let runtime = server_runtime.clone();
                        pending.push(async move { runtime.sleep(delay).await; response }.boxed());
                    }
                }
            }
            server_state.close();
        }).abort_on_drop());

        let writer_state = Arc::clone(&state);
        actors.push(
            runtime
                .spawn("wire_peer_responses", async move {
                    loop {
                        let response = tokio::select! {
                            biased;
                            _ = writer_state.wait_stopped() => break,
                            response = response_rx.recv() => response,
                        };
                        let Some(response) = response else { break };
                        let sent = tokio::select! {
                            biased;
                            _ = writer_state.wait_stopped() => break,
                            sent = server_sink.send(response) => sent,
                        };
                        if sent.is_err() {
                            break;
                        }
                    }
                    writer_state.close();
                })
                .abort_on_drop(),
        );

        (
            Self { runtime, commands, state: Arc::clone(&state) },
            WirePeer { link: Some(link), actors, state },
        )
    }

    fn request<T: Send + 'static>(
        &self,
        message: impl FnOnce(u64) -> EthMessage,
        pending: impl FnOnce(oneshot::Sender<Result<T, RequestError>>) -> PendingRequest,
    ) -> Pin<Box<dyn Future<Output = PeerRequestResult<T>> + Send + Sync>> {
        let request_id = self.state.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.state.pending.lock().unwrap().insert(request_id, pending(sender));
        if (self.state.closed.load(Ordering::Acquire) ||
            self.commands.send(message(request_id)).is_err()) &&
            let Some(pending) = self.state.pending.lock().unwrap().remove(&request_id)
        {
            pending.fail(RequestError::ConnectionDropped);
        }
        let guard = RequestGuard { state: Arc::clone(&self.state), request_id };
        let runtime = self.runtime.clone();
        let peer_id = self.state.peer_id;
        Box::pin(async move {
            let _guard = guard;
            let response = tokio::select! {
                biased;
                response = receiver => response.map_err(|_| RequestError::ConnectionDropped)?,
                _ = runtime.sleep(Duration::from_secs(1)) => Err(RequestError::Timeout),
            }?;
            Ok((peer_id, response).into())
        })
    }
}

impl DownloadClient for WireBlockClient {
    fn report_bad_message(&self, _peer_id: B512) {
        self.state.bad_messages.fetch_add(1, Ordering::Relaxed);
    }

    fn num_connected_peers(&self) -> usize {
        usize::from(!self.state.closed.load(Ordering::Acquire))
    }
}

impl HeadersClient for WireBlockClient {
    type Header = Header;
    type Output = HeadersFut;

    fn get_headers_with_priority(
        &self,
        request: HeadersRequest,
        _priority: Priority,
    ) -> Self::Output {
        self.request(
            |request_id| {
                EthMessage::GetBlockHeaders(RequestPair {
                    request_id,
                    message: GetBlockHeaders {
                        start_block: request.start,
                        limit: request.limit,
                        skip: 0,
                        direction: request.direction,
                    },
                })
            },
            PendingRequest::Headers,
        )
    }
}

impl BodiesClient for WireBlockClient {
    type Body = BlockBody;
    type Output = BodiesFut;

    fn get_block_bodies_with_priority_and_range_hint(
        &self,
        hashes: Vec<B256>,
        _priority: Priority,
        _range_hint: Option<RangeInclusive<u64>>,
    ) -> Self::Output {
        self.request(
            |request_id| {
                EthMessage::GetBlockBodies(RequestPair {
                    request_id,
                    message: GetBlockBodies(hashes),
                })
            },
            PendingRequest::Bodies,
        )
    }
}

impl BlockClient for WireBlockClient {
    type Block = Block;
}

/// Owns every transport and protocol actor associated with the virtual peer.
#[derive(Debug)]
pub(super) struct WirePeer {
    link: Option<SimulatedLink>,
    actors: Vec<TaskHandle<()>>,
    state: Arc<PeerState>,
}

impl WirePeer {
    pub(super) fn set_partitioned(&self, partitioned: bool) {
        self.link.as_ref().unwrap().set_partitioned(partitioned);
    }

    pub(super) fn disconnect(&self) {
        self.link.as_ref().unwrap().disconnect();
    }

    pub(super) fn trace(&self) -> Vec<WireEvent> {
        self.state.trace.lock().unwrap().clone()
    }

    pub(super) fn stats(&self) -> LinkStats {
        self.link.as_ref().unwrap().stats()
    }

    pub(super) fn bad_messages(&self) -> usize {
        self.state.bad_messages.load(Ordering::Relaxed)
    }

    pub(super) async fn shutdown(mut self) {
        self.state.close();
        self.link.take().unwrap().shutdown().await.unwrap();
        for actor in self.actors.drain(..) {
            actor.await.unwrap();
        }
    }
}

impl Drop for WirePeer {
    fn drop(&mut self) {
        self.state.close();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WireEvent {
    Request { request_id: u64, headers: bool },
    Response { request_id: u64, headers: bool },
}

#[derive(Debug)]
struct PeerState {
    peer_id: B512,
    next_id: AtomicU64,
    pending: Mutex<BTreeMap<u64, PendingRequest>>,
    closed: AtomicBool,
    stopped: Notify,
    bad_messages: AtomicUsize,
    trace: Mutex<Vec<WireEvent>>,
}

impl Default for PeerState {
    fn default() -> Self {
        Self {
            peer_id: B512::ZERO,
            next_id: AtomicU64::new(0),
            pending: Mutex::new(BTreeMap::new()),
            closed: AtomicBool::new(false),
            stopped: Notify::new(),
            bad_messages: AtomicUsize::new(0),
            trace: Mutex::new(Vec::new()),
        }
    }
}

impl PeerState {
    async fn wait_stopped(&self) {
        let stopped = self.stopped.notified();
        tokio::pin!(stopped);
        stopped.as_mut().enable();
        if !self.closed.load(Ordering::Acquire) {
            stopped.await;
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.stopped.notify_waiters();
        let pending = std::mem::take(&mut *self.pending.lock().unwrap());
        for (_, pending) in pending {
            pending.fail(RequestError::ConnectionDropped);
        }
    }

    fn record(&self, event: WireEvent) {
        self.trace.lock().unwrap().push(event);
    }
}

#[derive(Debug)]
enum PendingRequest {
    Headers(oneshot::Sender<Result<Vec<Header>, RequestError>>),
    Bodies(oneshot::Sender<Result<Vec<BlockBody>, RequestError>>),
}

impl PendingRequest {
    fn fail(self, error: RequestError) {
        match self {
            Self::Headers(sender) => {
                let _ = sender.send(Err(error));
            }
            Self::Bodies(sender) => {
                let _ = sender.send(Err(error));
            }
        }
    }
}

struct RequestGuard {
    state: Arc<PeerState>,
    request_id: u64,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.state.pending.lock().unwrap().remove(&self.request_id);
    }
}

fn serve_headers(
    blocks: &BTreeMap<u64, SealedBlock<Block>>,
    request: GetBlockHeaders,
) -> Vec<Header> {
    let first = match request.start_block {
        BlockHashOrNumber::Number(number) => Some(number),
        BlockHashOrNumber::Hash(hash) => {
            blocks.iter().find_map(|(number, block)| (block.hash() == hash).then_some(*number))
        }
    };
    let Some(mut number) = first else { return Vec::new() };
    let mut headers = Vec::new();
    let step = u64::from(request.skip) + 1;
    for _ in 0..request.limit.min(blocks.len() as u64) {
        let Some(block) = blocks.get(&number) else { break };
        headers.push(block.header().clone());
        let next = if request.direction.is_rising() {
            number.checked_add(step)
        } else {
            number.checked_sub(step)
        };
        let Some(next) = next else { break };
        number = next;
    }
    headers
}

#[test]
fn deterministic_wire_request_lifecycle() {
    use commonware_runtime::{deterministic, Runner, Supervisor};
    use reth_primitives_traits::Block as _;

    fn run(seed: u64) -> (String, Vec<WireEvent>, LinkStats) {
        deterministic::Runner::new(
            deterministic::Config::default()
                .with_seed(seed)
                .with_timeout(Some(Duration::from_secs(5))),
        )
        .start(|context| async move {
            let runtime = TaskRuntime::deterministic(context.child("wire_peer"));
            let mut blocks = BTreeMap::new();
            let mut parent_hash = B256::ZERO;
            for number in 0..=2 {
                let block = Block {
                    header: Header { number, parent_hash, ..Default::default() },
                    body: Default::default(),
                }
                .seal_slow();
                parent_hash = block.hash();
                blocks.insert(number, block);
            }
            let blocks = Arc::new(Mutex::new(blocks));
            let (client, peer) = WireBlockClient::new(
                runtime.clone(),
                Arc::clone(&blocks),
                LinkConfig {
                    seed,
                    max_chunk: 127,
                    latency: Duration::from_micros(10),
                    jitter: Duration::from_micros(10),
                    ..Default::default()
                },
            )
            .await;
            let first = client.get_headers(HeadersRequest::one(1u64.into()));
            let second = client.get_headers(HeadersRequest::one(2u64.into()));
            let (first, second) = futures::future::join(first, second).await;
            assert_eq!(first.unwrap().into_data()[0].number, 1);
            assert_eq!(second.unwrap().into_data()[0].number, 2);
            let responses: Vec<_> = peer
                .trace()
                .iter()
                .filter_map(|event| match event {
                    WireEvent::Response { request_id, .. } => Some(*request_id),
                    _ => None,
                })
                .collect();
            if seed % 3 != 2 {
                assert_eq!(
                    responses,
                    [1, 0],
                    "responses should overtake without crossing request IDs"
                );
            }

            let block = Block {
                header: Header { number: 3, parent_hash, ..Default::default() },
                body: Default::default(),
            }
            .seal_slow();
            let hash = block.hash();
            blocks.lock().unwrap().insert(3, block);
            let headers = client
                .get_headers(HeadersRequest::falling(hash.into(), 3))
                .await
                .unwrap()
                .into_data();
            assert_eq!(headers.iter().map(|header| header.number).collect::<Vec<_>>(), [3, 2, 1]);
            let bodies = client.get_block_bodies(vec![hash]).await.unwrap().into_data();
            assert_eq!(bodies, [blocks.lock().unwrap().get(&3).unwrap().body().clone()]);

            // Cancel a request before polling its future. Its response may arrive later, but the
            // registry must release it and a subsequent request must still complete normally.
            drop(client.get_headers(HeadersRequest::one(1u64.into())));
            assert!(client.state.pending.lock().unwrap().is_empty());
            runtime.sleep(Duration::from_millis(100)).await;
            assert_eq!(
                client.get_headers(HeadersRequest::one(2u64.into())).await.unwrap().into_data()[0]
                    .number,
                2
            );

            peer.set_partitioned(true);
            let timed_out = client.get_headers(HeadersRequest::one(1u64.into())).await;
            assert!(matches!(timed_out, Err(RequestError::Timeout)));
            assert!(client.state.pending.lock().unwrap().is_empty());
            peer.set_partitioned(false);
            assert_eq!(
                client.get_headers(HeadersRequest::one(3u64.into())).await.unwrap().into_data()[0]
                    .number,
                3
            );

            peer.set_partitioned(true);
            let interrupted = client.get_block_bodies(vec![hash]);
            runtime.yield_now().await;
            peer.disconnect();
            assert!(matches!(interrupted.await, Err(RequestError::ConnectionDropped)));
            assert_eq!(client.num_connected_peers(), 0);
            assert_eq!(peer.bad_messages(), 0);
            let trace = peer.trace();
            let stats = peer.stats();
            peer.shutdown().await;
            (context.auditor().state(), trace, stats)
        })
    }

    let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
        Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
        Err(std::env::VarError::NotPresent) => (0..16).collect(),
        Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
    };
    for seed in seeds {
        eprintln!("wire peer DST: seed={seed}");
        assert_eq!(run(seed), run(seed), "wire request replay diverged for seed {seed}");
    }
}
