//! In-memory transport for testing the production [`crate::EthStream`].
//!
//! Raw byte links support the production ECIES authentication and framing, `RLPx` capability
//! negotiation, ping/pong handling, and ETH message streams through [`authenticated_pair`]. The
//! simpler length-prefixed transport isolates ETH decoding. Peer discovery remains out of scope.

use alloy_primitives::{
    bytes::{Bytes, BytesMut},
    Keccak256, B256,
};
use futures::{Sink, Stream};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_tasks::{TaskError, TaskHandle, TaskRuntime};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf},
    sync::Notify,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Owns the forwarding actors for a pair of simulated ETH transports.
///
/// Keep this value alive while using either endpoint. Dropping it aborts both forwarding actors
/// at their next poll. Use [`Self::shutdown`] when a restart must wait for those actors to release
/// their streams.
#[derive(Debug)]
pub struct SimulatedLink {
    control: Arc<LinkControl>,
    actors: [TaskHandle<()>; 2],
    stats: Arc<Mutex<TrafficStats>>,
    corrupt: Arc<[AtomicBool; 2]>,
}

impl SimulatedLink {
    /// Creates two connected, bounded byte streams and their forwarding actors.
    ///
    /// The streams implement the transport traits required by [`crate::UnauthedEthStream`]. Use
    /// its `handshake_without_timeout` method with a deadline driven by `runtime`, or construct an
    /// [`crate::EthStream`] directly when testing an already established ETH session.
    ///
    /// # Panics
    ///
    /// Panics for a zero capacity/chunk size or jitter exceeding `u64::MAX` nanoseconds.
    pub fn new(runtime: TaskRuntime, config: LinkConfig) -> (WireStream, WireStream, Self) {
        let (left, right, link) = Self::new_raw(runtime, config);
        let framed = |stream| {
            let codec = LengthDelimitedCodec::builder()
                .max_frame_length(crate::message::MAX_MESSAGE_SIZE)
                .new_codec();
            let mut stream = Framed::new(stream, codec);
            stream.set_backpressure_boundary(config.capacity);
            stream
        };
        (framed(left), framed(right), link)
    }

    /// Creates raw byte endpoints for the production ECIES codec and `RLPx` transport.
    ///
    /// # Panics
    ///
    /// Has the same configuration requirements as [`Self::new`].
    pub fn new_raw(runtime: TaskRuntime, config: LinkConfig) -> (DuplexStream, DuplexStream, Self) {
        assert!(config.capacity > 0, "link capacity must be nonzero");
        assert!(config.max_chunk > 0, "maximum transfer chunk must be nonzero");
        let jitter_nanos =
            u64::try_from(config.jitter.as_nanos()).expect("link jitter exceeds u64 nanoseconds");
        let (left, left_relay) = tokio::io::duplex(config.capacity);
        let (right, right_relay) = tokio::io::duplex(config.capacity);
        let (left_read, left_write) = tokio::io::split(left_relay);
        let (right_read, right_write) = tokio::io::split(right_relay);
        let control = Arc::new(LinkControl {
            state: Mutex::new(LinkState::Connected),
            changed: Notify::new(),
        });
        let stats = Arc::new(Mutex::new(TrafficStats::default()));
        let corrupt = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);

        let spawn = |direction, reader, writer, seed| {
            runtime
                .spawn(
                    "eth_wire_transport",
                    forward(
                        reader,
                        writer,
                        runtime.clone(),
                        config,
                        jitter_nanos,
                        seed,
                        direction,
                        Arc::clone(&control),
                        Arc::clone(&stats),
                        Arc::clone(&corrupt),
                    ),
                )
                .abort_on_drop()
        };
        let actors = [
            spawn(0, left_read, right_write, config.seed),
            spawn(1, right_read, left_write, config.seed ^ 0x9e37_79b9_7f4a_7c15),
        ];
        (left, right, Self { control, actors, stats, corrupt })
    }

    /// Pauses or resumes forwarding at chunk boundaries.
    ///
    /// Already buffered bytes and a write in progress may still be received. A disconnected link
    /// cannot be resumed; create a new pair to model reconnection.
    pub fn set_partitioned(&self, partitioned: bool) {
        {
            let mut state = self.control.state.lock().expect("link control lock poisoned");
            if *state == LinkState::Disconnected {
                return;
            }
            let next = if partitioned { LinkState::Partitioned } else { LinkState::Connected };
            if *state == next {
                return;
            }
            *state = next;
        }
        self.control.changed.notify_waiters();
    }

    /// Disconnects both directions, interrupting pending latency and I/O on their next poll.
    pub fn disconnect(&self) {
        *self.control.state.lock().expect("link control lock poisoned") = LinkState::Disconnected;
        self.control.changed.notify_waiters();
    }

    /// Flips one bit in the next chunk to inject invalid peer bytes after authentication.
    ///
    /// Direction zero is left-to-right and one is right-to-left.
    ///
    /// # Panics
    ///
    /// Panics if `direction` is greater than one.
    pub fn corrupt_next(&self, direction: usize) {
        self.corrupt[direction].store(true, Ordering::Release);
    }

    /// Disconnects and waits until the forwarding actors have released their endpoints.
    pub async fn shutdown(self) -> Result<(), TaskError> {
        self.disconnect();
        let [left, right] = self.actors;
        let (left, right) = futures::future::join(left, right).await;
        left.and(right)
    }

    /// Returns counts and content hashes of completely forwarded traffic in each direction.
    pub fn stats(&self) -> LinkStats {
        let stats = self.stats.lock().expect("link statistics lock poisoned");
        let mut result = stats.summary;
        result.content_hashes =
            std::array::from_fn(|direction| stats.hashes[direction].clone().finalize());
        result
    }
}

/// A framed byte transport accepted by the production ETH handshake and stream.
pub type WireStream = Framed<DuplexStream, LengthDelimitedCodec>;

/// A production `RLPx` stream over authenticated ECIES frames and a simulated byte link.
///
/// Reading also drives control writes, as the production active-session task does. This ensures
/// an idle fixture still exchanges pings and observes ping timeouts without an application send.
#[derive(Debug)]
pub struct AuthenticatedWireStream {
    inner: Box<crate::P2PStream<reth_ecies::stream::ECIESStream<DuplexStream>>>,
}

impl AuthenticatedWireStream {
    /// Returns the ECIES stream, including its authenticated remote identity.
    pub fn inner(&self) -> &reth_ecies::stream::ECIESStream<DuplexStream> {
        self.inner.inner()
    }
}

impl Stream for AuthenticatedWireStream {
    type Item = Result<BytesMut, crate::errors::P2PStreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let inner = self.get_mut().inner.as_mut();
        // A ping transition creates a fresh timeout timer. A second readiness poll registers
        // that timer before this driver sleeps. Still read when writes are backpressured.
        for _ in 0..2 {
            match Pin::new(&mut *inner).poll_ready(cx) {
                Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                Poll::Pending => break,
                Poll::Ready(Ok(())) => {}
            }
        }
        let result = Pin::new(&mut *inner).poll_next(cx);
        if result.is_pending() {
            // Reading a Pong resets the next ping timer; reading a Ping queues a Pong. Register
            // the fresh deadline and drive that reply even when there are no ETH messages.
            for _ in 0..2 {
                match Pin::new(&mut *inner).poll_ready(cx) {
                    Poll::Ready(Err(error)) => return Poll::Ready(Some(Err(error))),
                    Poll::Pending => break,
                    Poll::Ready(Ok(())) => {}
                }
            }
        }
        result
    }
}

impl Sink<Bytes> for AuthenticatedWireStream {
    type Error = crate::errors::P2PStreamError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(self.get_mut().inner.as_mut()).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        Pin::new(self.get_mut().inner.as_mut()).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(self.get_mut().inner.as_mut()).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(self.get_mut().inner.as_mut()).poll_close(cx)
    }
}

impl crate::CanDisconnect<Bytes> for AuthenticatedWireStream {
    fn disconnect(
        &mut self,
        reason: crate::DisconnectReason,
    ) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + '_>> {
        Box::pin(self.inner.disconnect(reason))
    }
}

/// Performs ECIES and `RLPx` Hello negotiation using fixed test identities and seeded entropy.
///
/// All deadlines, including subsequent ping/pong timers, use `runtime`. The resulting streams
/// still require the usual ETH Status handshake. Reproducible test identities and entropy must
/// not be used outside simulation.
pub async fn authenticated_pair(
    runtime: TaskRuntime,
    config: LinkConfig,
) -> Result<(AuthenticatedWireStream, AuthenticatedWireStream, SimulatedLink), LinkHandshakeError> {
    use reth_ecies::stream::ECIESStream;
    use reth_network_peers::pk2id;
    use secp256k1::{SecretKey, SECP256K1};

    let (left, right, link) = SimulatedLink::new_raw(runtime.clone(), config);
    let left_key = SecretKey::from_byte_array(&[1; 32]).expect("valid fixed simulation key");
    let right_key = SecretKey::from_byte_array(&[2; 32]).expect("valid fixed simulation key");
    let left_id = pk2id(&left_key.public_key(SECP256K1));
    let right_id = pk2id(&right_key.public_key(SECP256K1));
    let negotiate = async {
        let (left, right) = futures::future::try_join(
            ECIESStream::connect_seeded(left, left_key, right_id, config.seed),
            ECIESStream::incoming_seeded(right, right_key, config.seed ^ 0xd1b5_4a32_d192_ed03),
        )
        .await?;
        let ((left, right_hello), (right, left_hello)) = futures::future::try_join(
            Box::pin(negotiate_hello(left, left_id, runtime.clone())),
            Box::pin(negotiate_hello(right, right_id, runtime.clone())),
        )
        .await?;
        // The ECIES key, rather than a self-reported Hello identity, identifies the peer.
        if left.inner().remote_id() != right_hello.id || right.inner().remote_id() != left_hello.id
        {
            return Err(LinkHandshakeError::IdentityMismatch);
        }
        Ok::<_, LinkHandshakeError>((left, right))
    };
    let (left, right) = tokio::select! {
        biased;
        result = negotiate => result?,
        _ = runtime.sleep(crate::HANDSHAKE_TIMEOUT) => return Err(LinkHandshakeError::Timeout),
    };
    Ok((left, right, link))
}

async fn negotiate_hello(
    stream: reth_ecies::stream::ECIESStream<DuplexStream>,
    id: reth_network_peers::PeerId,
    runtime: TaskRuntime,
) -> Result<(AuthenticatedWireStream, crate::HelloMessage), crate::errors::P2PStreamError> {
    let (stream, hello) = crate::UnauthedP2PStream::new(stream)
        .handshake_with_runtime(
            crate::HelloMessage::builder(id).protocol(crate::EthVersion::Eth68).build(),
            runtime,
        )
        .await?;
    // Compression state makes an established stream large. Keep it on the heap while the two
    // peer handshakes are joined, so nested unoptimized test futures do not copy large frames.
    Ok((AuthenticatedWireStream { inner: Box::new(stream) }, hello))
}

/// Failure while establishing a simulated authenticated `RLPx` connection.
#[derive(Debug, thiserror::Error)]
pub enum LinkHandshakeError {
    /// ECIES authentication or frame validation failed.
    #[error(transparent)]
    Ecies(#[from] reth_ecies::ECIESError),
    /// `RLPx` capability negotiation failed.
    #[error(transparent)]
    P2p(#[from] crate::errors::P2PStreamError),
    /// The runtime deadline expired.
    #[error("simulated handshake timed out")]
    Timeout,
    /// Hello claimed an identity different from the key authenticated by ECIES.
    #[error("simulated Hello identity did not match authenticated ECIES key")]
    IdentityMismatch,
}

/// Parameters for one simulated link. Randomness is local to each direction and explicitly seeded.
#[derive(Clone, Copy, Debug)]
pub struct LinkConfig {
    /// Seed used to vary fragment lengths and latency without ambient randomness.
    pub seed: u64,
    /// Capacity in bytes of each underlying duplex buffer.
    pub capacity: usize,
    /// Maximum bytes read and forwarded in one chunk. Chunk sizes vary from one to this value.
    pub max_chunk: usize,
    /// Minimum simulated delay before forwarding a chunk.
    pub latency: Duration,
    /// Maximum additional simulated delay, sampled separately for each chunk.
    pub jitter: Duration,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            capacity: 4096,
            max_chunk: 1024,
            latency: Duration::from_millis(1),
            jitter: Duration::from_millis(1),
        }
    }
}

/// Traffic that completed forwarding, indexed as left-to-right and right-to-left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Number of successfully forwarded chunks.
    pub fragments: [usize; 2],
    /// Number of bytes in those chunks, including framing and authentication bytes.
    pub bytes: [usize; 2],
    /// Hash of completely forwarded bytes in each direction, independent of chunk boundaries.
    pub content_hashes: [B256; 2],
}

#[derive(Debug, Default)]
struct TrafficStats {
    summary: LinkStats,
    hashes: [Keccak256; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkState {
    Connected,
    Partitioned,
    Disconnected,
}

/// A single notification queue preserves waiter order. Tokio watch channels shard waiters with
/// ambient randomness, which would change the simulator's ready queue across repeated seeds.
#[derive(Debug)]
struct LinkControl {
    state: Mutex<LinkState>,
    changed: Notify,
}

impl LinkControl {
    async fn disconnected(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if *self.state.lock().expect("link control lock poisoned") == LinkState::Disconnected {
                return;
            }
            changed.await;
        }
    }

    async fn connected(&self) -> io::Result<()> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match *self.state.lock().expect("link control lock poisoned") {
                LinkState::Connected => return Ok(()),
                LinkState::Disconnected => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "simulated disconnect",
                    ));
                }
                LinkState::Partitioned => {}
            }
            changed.await;
        }
    }
}

#[expect(clippy::too_many_arguments)]
async fn forward(
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
    runtime: TaskRuntime,
    config: LinkConfig,
    jitter_nanos: u64,
    seed: u64,
    direction: usize,
    control: Arc<LinkControl>,
    stats: Arc<Mutex<TrafficStats>>,
    corrupt: Arc<[AtomicBool; 2]>,
) {
    let disconnected = control.disconnected();
    let transfer = transfer(
        reader,
        writer,
        runtime,
        config,
        jitter_nanos,
        seed,
        direction,
        Arc::clone(&control),
        stats,
        corrupt,
    );
    // Dropping an interrupted transfer releases both halves. Bytes from a partially written
    // frame may reach the receiver, which must observe EOF or a decoding error rather than a
    // manufactured complete message.
    tokio::select! {
        biased;
        _ = disconnected => {},
        _ = transfer => {},
    }
}

#[expect(clippy::too_many_arguments)]
async fn transfer(
    mut reader: ReadHalf<DuplexStream>,
    mut writer: WriteHalf<DuplexStream>,
    runtime: TaskRuntime,
    config: LinkConfig,
    jitter_nanos: u64,
    seed: u64,
    direction: usize,
    control: Arc<LinkControl>,
    stats: Arc<Mutex<TrafficStats>>,
    corrupt: Arc<[AtomicBool; 2]>,
) -> io::Result<()> {
    let mut random = StdRng::seed_from_u64(seed);
    let mut buffer = vec![0; config.max_chunk];
    loop {
        control.connected().await?;
        let length = random.random_range(1..=config.max_chunk);
        let read = reader.read(&mut buffer[..length]).await?;
        if read == 0 {
            writer.shutdown().await?;
            return Ok(());
        }
        let jitter = Duration::from_nanos(random.random_range(0..=jitter_nanos));
        runtime.sleep(config.latency + jitter).await;
        control.connected().await?;
        if corrupt[direction].swap(false, Ordering::AcqRel) {
            buffer[0] ^= 1;
        }
        writer.write_all(&buffer[..read]).await?;
        {
            let mut stats = stats.lock().expect("link statistics lock poisoned");
            stats.summary.fragments[direction] += 1;
            stats.summary.bytes[direction] += read;
            stats.hashes[direction].update(&buffer[..read]);
        }
        runtime.yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        message::RequestPair, BlockHeaders, EthMessage, EthNetworkPrimitives, EthStream,
        EthVersion, GetBlockHeaders, HeadersDirection, UnauthedEthStream, UnifiedStatus,
    };
    use alloy_consensus::Header;
    use alloy_primitives::{bytes::Bytes, B256};
    use commonware_runtime::{deterministic, Runner, Supervisor};
    use futures::{SinkExt, StreamExt};
    use reth_ethereum_forks::{ForkFilter, Head};

    async fn exercise(runtime: TaskRuntime, seed: u64) -> LinkStats {
        let config = LinkConfig {
            seed,
            capacity: 512,
            max_chunk: 17,
            latency: Duration::from_micros(10),
            jitter: Duration::from_micros(10),
        };
        let (left, right, link) = authenticated_pair(runtime.clone(), config).await.unwrap();
        let forks = ForkFilter::new(Head::default(), B256::ZERO, 0, Vec::new());
        let status = UnifiedStatus {
            genesis: B256::ZERO,
            blockhash: B256::ZERO,
            forkid: forks.current(),
            ..Default::default()
        };
        let ((mut client, peer_status), (mut server, client_status)) = futures::future::try_join(
            UnauthedEthStream::new(left)
                .handshake_without_timeout::<EthNetworkPrimitives>(status, forks.clone()),
            UnauthedEthStream::new(right)
                .handshake_without_timeout::<EthNetworkPrimitives>(status, forks),
        )
        .await
        .unwrap();
        assert_eq!(peer_status, client_status);

        let mut random = StdRng::seed_from_u64(seed);
        let headers: Vec<_> = (1..=4)
            .map(|number| {
                let mut header = Header { number, ..Default::default() };
                // Distinct bloom bytes keep the response larger than the transport capacity after
                // Snappy compression, so this exercises backpressure as well as fragmentation.
                random.fill(header.logs_bloom.as_mut_slice());
                header
            })
            .collect();
        let expected = headers.clone();
        let service = runtime.spawn("wire_headers", async move {
            let Some(Ok(EthMessage::GetBlockHeaders(request))) = server.next().await else {
                panic!("peer did not receive the encoded headers request");
            };
            assert_eq!(request.request_id, 17);
            assert_eq!(request.message.limit, 4);
            server
                .send(EthMessage::BlockHeaders(RequestPair {
                    request_id: request.request_id,
                    message: BlockHeaders(headers),
                }))
                .await
                .unwrap();
            server
        });
        let request = EthMessage::GetBlockHeaders(RequestPair {
            request_id: 17,
            message: GetBlockHeaders {
                start_block: 1u64.into(),
                limit: 4,
                skip: 0,
                direction: HeadersDirection::Rising,
            },
        });
        client.send(request.clone()).await.unwrap();
        let Some(Ok(EthMessage::BlockHeaders(response))) = client.next().await else {
            panic!("client did not decode the headers response");
        };
        assert_eq!(response.request_id, 17);
        assert_eq!(response.message.0, expected);
        let mut server = service.await.unwrap();

        link.set_partitioned(true);
        client.send(request.clone()).await.unwrap();
        runtime.sleep(Duration::from_millis(2)).await;
        assert!(futures::poll!(server.next()).is_pending());
        link.set_partitioned(false);
        assert_eq!(server.next().await.unwrap().unwrap(), request);

        link.set_partitioned(true);
        client.send(request).await.unwrap();
        let stats = link.stats();
        assert!(stats.bytes[1] > config.capacity);
        assert!(stats.fragments.iter().all(|count| *count > 1));
        link.shutdown().await.unwrap();
        assert!(!matches!(server.next().await, Some(Ok(_))));

        // Feed a malformed protocol message through the same byte framing. The production ETH
        // decoder, rather than a fixture-specific message dispatcher, must reject it.
        let (mut raw, right, link) = SimulatedLink::new(runtime.clone(), config);
        let mut server = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth68, right);
        // BlockHeaders (0x04), followed by a truncated long-list RLP prefix.
        raw.send(Bytes::from_static(&[0x04, 0xf8])).await.unwrap();
        assert!(matches!(
            server.next().await,
            Some(Err(crate::errors::EthStreamError::InvalidMessage(_)))
        ));
        link.shutdown().await.unwrap();

        // Corrupt encrypted bytes after authentication. The ECIES MAC must reject the frame
        // before malformed plaintext can reach the ETH decoder.
        let (left, right, link) = authenticated_pair(runtime, config).await.unwrap();
        let mut client = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth68, left);
        let mut server = EthStream::<_, EthNetworkPrimitives>::new(EthVersion::Eth68, right);
        link.corrupt_next(0);
        client
            .send(EthMessage::GetBlockHeaders(RequestPair {
                request_id: 18,
                message: GetBlockHeaders {
                    start_block: 1u64.into(),
                    limit: 1,
                    skip: 0,
                    direction: HeadersDirection::Rising,
                },
            }))
            .await
            .unwrap();
        let error = server.next().await.unwrap().unwrap_err();
        assert!(error.as_io().is_some(), "corrupted ciphertext reached ETH decoding: {error}");
        link.shutdown().await.unwrap();
        stats
    }

    #[test]
    fn deterministic_idle_ping_and_partition_timeout() {
        fn run(seed: u64) -> (String, LinkStats) {
            on_simulation_thread(move || {
                deterministic::Runner::new(
                    deterministic::Config::default()
                        .with_seed(seed)
                        .with_timeout(Some(Duration::from_secs(210))),
                )
                .start(|context| async move {
                    let runtime = TaskRuntime::deterministic(context.child("idle_wire"));
                    let config = LinkConfig { seed, ..Default::default() };
                    let (mut left, mut right, link) =
                        authenticated_pair(runtime.clone(), config).await.unwrap();
                    let before = link.stats();
                    let mut left = runtime
                        .spawn("idle_left", async move { left.next().await })
                        .abort_on_drop();
                    let mut right = runtime
                        .spawn("idle_right", async move { right.next().await })
                        .abort_on_drop();

                    // No application messages are sent. Two complete ping/pong rounds prove that
                    // the timer remains registered after receiving a Pong and resetting its
                    // deadline.
                    runtime.sleep(Duration::from_secs(121)).await;
                    assert!(futures::poll!(&mut left).is_pending());
                    assert!(futures::poll!(&mut right).is_pending());
                    let alive = link.stats();
                    assert!(alive.bytes[0] > before.bytes[0]);
                    assert!(alive.bytes[1] > before.bytes[1]);

                    link.set_partitioned(true);
                    let partitioned_at = runtime.now();
                    let (left, right) = futures::future::join(left, right).await;
                    assert!(left.unwrap().is_none());
                    assert!(right.unwrap().is_none());
                    // The next ping is due near second 180, then its normal 15-second deadline
                    // expires. A stalled or wall-clock timer would exceed the runner's deadline.
                    let elapsed = runtime.now().duration_since(partitioned_at).unwrap();
                    assert!(elapsed >= Duration::from_secs(70));
                    assert!(elapsed < Duration::from_secs(76));
                    let stats = link.stats();
                    link.shutdown().await.unwrap();
                    (context.auditor().state(), stats)
                })
            })
        }

        for seed in replay_seeds() {
            eprintln!("idle encrypted wire DST: seed={seed}");
            assert_eq!(run(seed), run(seed), "idle wire replay diverged for seed {seed}");
        }
    }

    #[tokio::test]
    async fn production_wire_transport() {
        exercise(TaskRuntime::from(reth_tasks::Runtime::test()), 0).await;
    }

    #[test]
    fn deterministic_wire_transport() {
        fn run(seed: u64) -> (String, LinkStats) {
            on_simulation_thread(move || {
                deterministic::Runner::new(
                    deterministic::Config::default()
                        .with_seed(seed)
                        .with_timeout(Some(Duration::from_secs(5))),
                )
                .start(|context| async move {
                    let runtime = TaskRuntime::deterministic(context.child("wire"));
                    let stats = exercise(runtime, seed).await;
                    (context.auditor().state(), stats)
                })
            })
        }

        let seeds = replay_seeds();
        let mut audits = std::collections::BTreeSet::new();
        for seed in seeds.iter().copied() {
            eprintln!("ETH wire DST: seed={seed}");
            let result = run(seed);
            assert_eq!(result, run(seed), "wire replay diverged for seed {seed}");
            audits.insert(result.0);
        }
        if seeds.len() > 1 {
            assert!(audits.len() > 1);
        }
    }

    // The unoptimized nested ECIES/Hello/ETH futures exceed libtest's default stack while
    // being constructed. This thread only hosts the single Commonware executor; actors still
    // run exclusively on its controlled scheduler.
    fn on_simulation_thread<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .name("wire-simulation".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(run)
            .unwrap()
            .join()
            .unwrap()
    }

    fn replay_seeds() -> Vec<u64> {
        match std::env::var("RETH_DST_SEED") {
            Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
            Err(std::env::VarError::NotPresent) => (0..16).collect(),
            Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
        }
    }
}
