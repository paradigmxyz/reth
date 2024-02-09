# Network

The `network` crate is responsible for managing the node's connection to the Ethereum peer-to-peer (P2P) network, enabling communication with other nodes via the [various P2P subprotocols](https://github.com/ethereum/devp2p).

Reth's P2P networking consists primarily of 4 ongoing tasks:
- **Discovery**: Discovers new peers in the network
- **Transactions**: Accepts, requests, and broadcasts mempool transactions
- **ETH Requests**: Responds to incoming requests for headers and bodies
- **Network Management**: Handles incoming & outgoing connections with peers, and routes requests between peers and the other tasks

We'll leave most of the discussion of the discovery task for the [discv4](./discv4.md) chapter, and will focus on the other three here.

Let's take a look at how the main Reth CLI (i.e., a default-configured full node) makes use of the P2P layer to explore the primary interfaces and entrypoints into the `network` crate.

---

## The Network Management Task

The network management task is the one primarily used in the pipeline to interact with the P2P network. Apart from managing connectivity to the node's peers, it provides a couple of interfaces for sending _outbound_ requests.

Let's take a look at what the provided interfaces are, how they're used in the pipeline, and take a brief glance under the hood to highlight some important structs and traits in the network management task.

### Use of the Network in the Node

The `"node"` CLI command, used to run the node itself, does the following at a high level:
1. Initializes the DB
2. Initializes the consensus API
3. Writes the genesis block to the DB
4. Initializes the network
5. Instantiates a client for fetching data from the network
6. Configures the pipeline by adding stages to it
7. Runs the pipeline

Steps 5-6 are of interest to us as they consume items from the `network` crate:

[File: bin/reth/src/node/mod.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/bin/reth/src/node/mod.rs)
```rust,ignore
let network = start_network(network_config(db.clone(), chain_id, genesis_hash)).await?;

let fetch_client = Arc::new(network.fetch_client().await?);
let mut pipeline = reth_stages::Pipeline::new()
    .push(HeaderStage {
        downloader: headers::reverse_headers::ReverseHeadersDownloaderBuilder::default()
            .batch_size(config.stages.headers.downloader_batch_size)
            .retries(config.stages.headers.downloader_retries)
            .build(consensus.clone(), fetch_client.clone()),
        consensus: consensus.clone(),
        client: fetch_client.clone(),
        network_handle: network.clone(),
        commit_threshold: config.stages.headers.commit_threshold,
        metrics: HeaderMetrics::default(),
    })
    .push(BodyStage {
        downloader: Arc::new(
            bodies::bodies::BodiesDownloader::new(
                fetch_client.clone(),
                consensus.clone(),
            )
            .with_batch_size(config.stages.bodies.downloader_batch_size)
            .with_retries(config.stages.bodies.downloader_retries)
            .with_concurrency(config.stages.bodies.downloader_concurrency),
        ),
        consensus: consensus.clone(),
        commit_threshold: config.stages.bodies.commit_threshold,
    })
    .push(SenderRecoveryStage {
        commit_threshold: config.stages.sender_recovery.commit_threshold,
    })
    .push(ExecutionStage { config: ExecutorConfig::new_ethereum() });

if let Some(tip) = self.tip {
    debug!("Tip manually set: {}", tip);
    consensus.notify_fork_choice_state(ForkchoiceState {
        head_block_hash: tip,
        safe_block_hash: tip,
        finalized_block_hash: tip,
    })?;
}

// Run pipeline
info!("Starting pipeline");
pipeline.run(db.clone()).await?;
```

Let's begin by taking a look at the line where the network is started, with the call, unsurprisingly, to `start_network`. Sounds important, doesn't it?

[File: bin/reth/src/node/mod.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/bin/reth/src/node/mod.rs)
```rust,ignore
async fn start_network<C>(config: NetworkConfig<C>) -> Result<NetworkHandle, NetworkError>
where
    C: BlockReader + HeaderProvider + 'static,
{
    let client = config.client.clone();
    let (handle, network, _txpool, eth) =
        NetworkManager::builder(config).await?.request_handler(client).split_with_handle();

    tokio::task::spawn(network);
    // TODO: tokio::task::spawn(txpool);
    tokio::task::spawn(eth);
    Ok(handle)
}
```

At a high level, this function is responsible for starting the tasks listed at the start of this chapter.

It gets the handles for the network management, transactions, and ETH requests tasks downstream of the `NetworkManager::builder` method call, and spawns them.

The `NetworkManager::builder` constructor requires a `NetworkConfig` struct to be passed in as a parameter, which can be used as the main entrypoint for setting up the entire network layer:

[File: crates/net/network/src/config.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/config.rs)
```rust,ignore
pub struct NetworkConfig<C> {
    /// The client type that can interact with the chain.
    pub client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    pub secret_key: SecretKey,
    /// All boot nodes to start network discovery with.
    pub boot_nodes: Vec<NodeRecord>,
    /// How to set up discovery.
    pub discovery_v4_config: Discv4Config,
    /// Address to use for discovery
    pub discovery_addr: SocketAddr,
    /// Address to listen for incoming connections
    pub listener_addr: SocketAddr,
    /// How to instantiate peer manager.
    pub peers_config: PeersConfig,
    /// How to configure the [SessionManager](crate::session::SessionManager).
    pub sessions_config: SessionsConfig,
    /// The id of the network
    pub chain: Chain,
    /// Genesis hash of the network
    pub genesis_hash: B256,
    /// The [`ForkFilter`] to use at launch for authenticating sessions.
    ///
    /// See also <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md#stale-software-examples>
    ///
    /// For sync from block `0`, this should be the default chain [`ForkFilter`] beginning at the
    /// first hardfork, `Frontier` for mainnet.
    pub fork_filter: ForkFilter,
    /// The block importer type.
    pub block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    pub network_mode: NetworkMode,
    /// The executor to use for spawning tasks.
    pub executor: Option<TaskExecutor>,
    /// The `Status` message to send to peers at the beginning.
    pub status: Status,
    /// Sets the hello message for the p2p handshake in RLPx
    pub hello_message: HelloMessage,
}
```

The discovery task progresses as the network management task is polled, handling events regarding peer management through the `Swarm` struct which is stored as a field on the `NetworkManager`:

[File: crates/net/network/src/swarm.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/swarm.rs)
```rust,ignore
pub(crate) struct Swarm<C> {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// All sessions.
    sessions: SessionManager,
    /// Tracks the entire state of the network and handles events received from the sessions.
    state: NetworkState<C>,
}
```

The `Swarm` struct glues together incoming connections from peers, managing sessions with peers, and recording the network's state (e.g. number of active peers, genesis hash of the network, etc.). It emits these as `SwarmEvent`s to the `NetworkManager`, and routes commands and events between the `SessionManager` and `NetworkState` structs that it holds.

We'll touch more on the `NetworkManager` shortly! It's perhaps the most important struct in this crate.

More information about the discovery task can be found in the [discv4](./discv4.md) chapter.

The ETH requests and transactions task will be explained in their own sections, following this one.

The variable `network` returned from `start_network` and the variable `fetch_client` returned from `network.fetch_client` are of types `NetworkHandle` and `FetchClient`, respectively. These are the two main interfaces for interacting with the P2P network, and are currently used in the `HeaderStage` and `BodyStage`.

Let's walk through how each is implemented, and then apply that knowledge to understand how they are used in the pipeline. In doing so, we'll dig deeper under the hood inside the network management task to get a sense of what's going on.

### Interacting with the Network Management Task Using `NetworkHandle`

The `NetworkHandle` struct is a client for the network management task that can be shared across threads. It wraps an `Arc` around the `NetworkInner` struct, defined as follows:

[File: crates/net/network/src/network.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/network.rs)
```rust,ignore
struct NetworkInner {
    /// Number of active peer sessions the node's currently handling.
    num_active_peers: Arc<AtomicUsize>,
    /// Sender half of the message channel to the [`NetworkManager`].
    to_manager_tx: UnboundedSender<NetworkHandleMessage>,
    /// The local address that accepts incoming connections.
    listener_address: Arc<Mutex<SocketAddr>>,
    /// The identifier used by this node.
    local_peer_id: PeerId,
    /// Access to all the nodes
    peers: PeersHandle,
    /// The mode of the network
    network_mode: NetworkMode,
}
```

The field of note here is `to_manager_tx`, which is a handle that can be used to send messages in a channel to an instance of the `NetworkManager` struct.

[File: crates/net/network/src/manager.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/manager.rs)
```rust,ignore
pub struct NetworkManager<C> {
    /// The type that manages the actual network part, which includes connections.
    swarm: Swarm<C>,
    /// Underlying network handle that can be shared.
    handle: NetworkHandle,
    /// Receiver half of the command channel set up between this type and the [`NetworkHandle`]
    from_handle_rx: UnboundedReceiverStream<NetworkHandleMessage>,
    /// Handles block imports according to the `eth` protocol.
    block_import: Box<dyn BlockImport>,
    /// All listeners for high level network events.
    event_listeners: NetworkEventListeners,
    /// Sender half to send events to the
    /// [`TransactionsManager`](crate::transactions::TransactionsManager) task, if configured.
    to_transactions_manager: Option<mpsc::UnboundedSender<NetworkTransactionEvent>>,
    /// Sender half to send events to the
    /// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler) task, if configured.
    to_eth_request_handler: Option<mpsc::UnboundedSender<IncomingEthRequest>>,
    /// Tracks the number of active session (connected peers).
    ///
    /// This is updated via internal events and shared via `Arc` with the [`NetworkHandle`]
    /// Updated by the `NetworkWorker` and loaded by the `NetworkService`.
    num_active_peers: Arc<AtomicUsize>,
}
```

Now we're getting to the meat of the `network` crate! The `NetworkManager` struct represents the "Network Management" task described above. It is implemented as an endless [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) that can be thought of as a "hub" process which listens for messages from the `NetworkHandle` or the node's peers and dispatches messages to the other tasks, while keeping track of the state of the network.

While the `NetworkManager` is meant to be spawned as a standalone [`tokio::task`](https://docs.rs/tokio/0.2.4/tokio/task/index.html), the `NetworkHandle` can be passed around and shared, enabling access to the `NetworkManager` from anywhere by sending requests & commands through the appropriate channels.

#### Usage of `NetworkHandle` in the Pipeline

In the pipeline, the `NetworkHandle` is used to instantiate the `FetchClient` - which we'll get into next - and is used in the `HeaderStage` to update the node's ["status"](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#status-0x00) (record the total difficulty, hash, and height of the last processed block).

[File: crates/stages/src/stages/headers.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/stages/src/stages/headers.rs)
```rust,ignore
async fn update_head<DB: Database>(
    &self,
    tx: &Transaction<'_, DB>,
    height: BlockNumber,
) -> Result<(), StageError> {
    // --snip--
    self.network_handle.update_status(height, block_key.hash(), td);
    // --snip--

}
```

Now that we have some understanding about the internals of the network management task, let's look at a higher-level abstraction that can be used to retrieve data from other peers: the `FetchClient`.

### Using `FetchClient` to Get Data in the Pipeline Stages

The `FetchClient` struct, similar to `NetworkHandle`, can be shared across threads, and is a client for fetching data from the network. It's a fairly lightweight struct:

[File: crates/net/network/src/fetch/client.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/fetch/client.rs)
```rust,ignore
pub struct FetchClient {
    /// Sender half of the request channel.
    pub(crate) request_tx: UnboundedSender<DownloadRequest>,
    /// The handle to the peers
    pub(crate) peers_handle: PeersHandle,
}
```

The `request_tx` field is a handle to a channel that can be used to send requests for downloading data, and the `peers_handle` field is a wrapper struct around a handle to a channel that can be used to send messages for applying manual changes to the peer set.

#### Instantiating the `FetchClient`

The fields `request_tx` and `peers_handle` are cloned off of the `StateFetcher` struct when instantiating the `FetchClient`, which is the lower-level struct responsible for managing data fetching operations over the network:

[File: crates/net/network/src/fetch/mod.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/fetch/mod.rs)
```rust,ignore
pub struct StateFetcher {
    /// Currently active [`GetBlockHeaders`] requests
    inflight_headers_requests:
        HashMap<PeerId, Request<HeadersRequest, PeerRequestResult<Vec<Header>>>>,
    /// Currently active [`GetBlockBodies`] requests
    inflight_bodies_requests:
        HashMap<PeerId, Request<Vec<B256>, PeerRequestResult<Vec<BlockBody>>>>,
    /// The list of _available_ peers for requests.
    peers: HashMap<PeerId, Peer>,
    /// The handle to the peers manager
    peers_handle: PeersHandle,
    /// Requests queued for processing
    queued_requests: VecDeque<DownloadRequest>,
    /// Receiver for new incoming download requests
    download_requests_rx: UnboundedReceiverStream<DownloadRequest>,
    /// Sender for download requests, used to detach a [`FetchClient`]
    download_requests_tx: UnboundedSender<DownloadRequest>,
}
```

This struct itself is nested deeply within the `NetworkManager`: its `Swarm` struct (shown earlier in the chapter) contains a `NetworkState` struct that has the `StateFetcher` as a field:

[File: crates/net/network/src/state.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/state.rs)
```rust,ignore
pub struct NetworkState<C> {
    /// All active peers and their state.
    active_peers: HashMap<PeerId, ActivePeer>,
    /// Manages connections to peers.
    peers_manager: PeersManager,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateAction>,
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Network discovery.
    discovery: Discovery,
    /// The genesis hash of the network we're on
    genesis_hash: B256,
    /// The type that handles requests.
    ///
    /// The fetcher streams RLPx related requests on a per-peer basis to this type. This type will
    /// then queue in the request and notify the fetcher once the result has been received.
    state_fetcher: StateFetcher,
}
```

#### Usage of `FetchClient` in the Pipeline

The `FetchClient` implements the `HeadersClient` and `BodiesClient` traits, defining the functionality to get headers and block bodies from available peers.

[File: crates/net/network/src/fetch/client.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/fetch/client.rs)
```rust,ignore
impl HeadersClient for FetchClient {
    /// Sends a `GetBlockHeaders` request to an available peer.
    async fn get_headers(&self, request: HeadersRequest) -> PeerRequestResult<BlockHeaders> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response })?;
        rx.await?.map(WithPeerId::transform)
    }
}

impl BodiesClient for FetchClient {
    async fn get_block_bodies(&self, request: Vec<B256>) -> PeerRequestResult<Vec<BlockBody>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockBodies { request, response })?;
        rx.await?
    }
}
```

This functionality is used in the `HeaderStage` and `BodyStage`, respectively.

In the pipeline used by the main Reth binary, the `HeaderStage` uses a `ReverseHeadersDownloader` to stream headers from the network:

[File: crates/net/downloaders/src/headers/reverse_headers.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/downloaders/src/headers/reverse_headers.rs)
```rust,ignore
pub struct ReverseHeadersDownloader<C, H> {
    /// The consensus client
    consensus: Arc<C>,
    /// The headers client
    client: Arc<H>,
    /// The batch size per one request
    pub batch_size: u64,
    /// The number of retries for downloading
    pub request_retries: usize,
}
```

A `FetchClient` is passed in to the `client` field, and the `get_headers` method it implements gets used when polling the stream created by the `ReverseHeadersDownloader` in the `execute` method of the `HeaderStage`.

[File: crates/net/downloaders/src/headers/reverse_headers.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/downloaders/src/headers/reverse_headers.rs)
```rust,ignore
fn get_or_init_fut(&mut self) -> HeadersRequestFuture {
    match self.request.take() {
        None => {
            // queue in the first request
            let client = Arc::clone(&self.client);
            let req = self.headers_request();
            tracing::trace!(
                target: "downloaders::headers",
                "requesting headers {req:?}"
            );
            HeadersRequestFuture {
                request: req.clone(),
                fut: Box::pin(async move { client.get_headers(req).await }),
                retries: 0,
                max_retries: self.request_retries,
            }
        }
        Some(fut) => fut,
    }
}
```

In the `BodyStage` configured by the main binary, a `BodiesDownloader` is used:

[File: crates/net/downloaders/src/bodies/bodies.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/downloaders/src/bodies/bodies.rs)
```rust,ignore
pub struct BodiesDownloader<Client, Consensus> {
    /// The bodies client
    client: Arc<Client>,
    /// The consensus client
    consensus: Arc<Consensus>,
    /// The number of retries for each request.
    retries: usize,
    /// The batch size per one request
    batch_size: usize,
    /// The maximum number of requests to send concurrently.
    concurrency: usize,
}
```

Here, similarly, a `FetchClient` is passed in to the `client` field, and the `get_block_bodies` method it implements is used when constructing the stream created by the `BodiesDownloader` in the `execute` method of the `BodyStage`.

[File: crates/net/downloaders/src/bodies/bodies.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/downloaders/src/bodies/bodies.rs)
```rust,ignore
async fn fetch_bodies(
    &self,
    headers: Vec<&SealedHeader>,
) -> DownloadResult<Vec<BlockResponse>> {
    // --snip--
    let (peer_id, bodies) =
        self.client.get_block_bodies(headers_with_txs_and_ommers).await?.split();
    // --snip--
}
```

It's worth noting that once the node starts downloading either headers or bodies from a given peer, it does _not_ necessarily stick with that peer, as this would preclude the ability to effectively support concurrent requests.

When `FetchClient.get_headers` or `FetchClient.get_block_bodies` is called, those `DownloadRequest`s are sent into the `StateFetcher.download_requests_tx` channel, and are processed as the `StateFetcher` gets polled.

Every time the `StateFetcher` is polled, it finds the next idle peer available to service the current request (for either a block header, or a block body). In this context, "idle" means any peer that is not currently handling a request from the node:

[File: crates/net/network/src/fetch/mod.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/fetch/mod.rs)
```rust,ignore
/// Returns the next action to return
fn poll_action(&mut self) -> PollAction {
    // we only check and not pop here since we don't know yet whether a peer is available.
    if self.queued_requests.is_empty() {
        return PollAction::NoRequests
    }

    let peer_id = if let Some(peer_id) = self.next_peer() {
        peer_id
    } else {
        return PollAction::NoPeersAvailable
    };

    let request = self.queued_requests.pop_front().expect("not empty; qed");
    let request = self.prepare_block_request(peer_id, request);

    PollAction::Ready(FetchAction::BlockRequest { peer_id, request })
}
```

---

## ETH Requests Task

The ETH requests task serves _incoming_ requests related to blocks in the [`eth` P2P subprotocol](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages) from other peers.

Similar to the network management task, it's implemented as an endless future, but it is meant to run as a background task (on a standalone `tokio::task`) and not to be interacted with directly from the pipeline. It's represented by the following `EthRequestHandler` struct:

[File: crates/net/network/src/eth_requests.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/eth_requests.rs)
```rust,ignore
pub struct EthRequestHandler<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Used for reporting peers.
    #[allow(dead_code)]
    peers: PeersHandle,
    /// Incoming request from the [NetworkManager](crate::NetworkManager).
    incoming_requests: UnboundedReceiverStream<IncomingEthRequest>,
}
```

The `client` field here is a client that's used to fetch data from the database, not to be confused with the `client` field on a downloader like the `ReverseHeadersDownloader` discussed above, which is a `FetchClient`.

### Input Streams to the ETH Requests Task

The `incoming_requests` field is the receiver end of a channel that accepts, as you might have guessed, incoming ETH requests from peers. The sender end of this channel is stored on the `NetworkManager` struct as the `to_eth_request_handler` field.

As the `NetworkManager` is polled and listens for events from peers passed through the `Swarm` struct it holds, it sends any received ETH requests into the channel.

### The Operation of the ETH Requests Task

Being an endless future, the core of the ETH requests task's functionality is in its `poll` method implementation. As the `EthRequestHandler` is polled, it listens for any ETH requests coming through the channel, and handles them accordingly. At the time of writing, the ETH requests task can handle the [`GetBlockHeaders`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03) and [`GetBlockBodies`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockbodies-0x05) requests.

[File: crates/net/network/src/eth_requests.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/eth_requests.rs)
```rust,ignore
fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();

    loop {
        match this.incoming_requests.poll_next_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(None) => return Poll::Ready(()),
            Poll::Ready(Some(incoming)) => match incoming {
                IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                    this.on_headers_request(peer_id, request, response)
                }
                IncomingEthRequest::GetBlockBodies { peer_id, request, response } => {
                    this.on_bodies_request(peer_id, request, response)
                }
                IncomingEthRequest::GetNodeData { .. } => {}
                IncomingEthRequest::GetReceipts { .. } => {}
            },
        }
    }
}
```

The handling of these requests is fairly straightforward. The `GetBlockHeaders` payload is the following:

[File: crates/net/eth-wire/src/types/blocks.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/types/blocks.rs)
```rust,ignore
pub struct GetBlockHeaders {
    /// The block number or hash that the peer should start returning headers from.
    pub start_block: BlockHashOrNumber,

    /// The maximum number of headers to return.
    pub limit: u64,

    /// The number of blocks that the node should skip while traversing and returning headers.
    /// A skip value of zero denotes that the peer should return contiguous headers, starting from
    /// [`start_block`](#structfield.start_block) and returning at most
    /// [`limit`](#structfield.limit) headers.
    pub skip: u32,

    /// The direction in which the headers should be returned in.
    pub direction: HeadersDirection,
}
```

In handling this request, the ETH requests task attempts, starting with `start_block`, to fetch the associated header from the database, increment/decrement the block number to fetch by `skip` depending on the `direction` while checking for overflow/underflow, and checks that bounds specifying the maximum numbers of headers or bytes to send have not been breached.

[File: crates/net/network/src/eth_requests.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/eth_requests.rs)
```rust,ignore
fn get_headers_response(&self, request: GetBlockHeaders) -> Vec<Header> {
    let GetBlockHeaders { start_block, limit, skip, direction } = request;

    let mut headers = Vec::new();

    let mut block: BlockHashOrNumber = match start_block {
        BlockHashOrNumber::Hash(start) => start.into(),
        BlockHashOrNumber::Number(num) => {
            if let Some(hash) = self.client.block_hash(num.into()).unwrap_or_default() {
                hash.into()
            } else {
                return headers
            }
        }
    };

    let skip = skip as u64;
    let mut total_bytes = APPROX_HEADER_SIZE;

    for _ in 0..limit {
        if let Some(header) = self.client.header_by_hash_or_number(block).unwrap_or_default() {
            match direction {
                HeadersDirection::Rising => {
                    if let Some(next) = (header.number + 1).checked_add(skip) {
                        block = next.into()
                    } else {
                        break
                    }
                }
                HeadersDirection::Falling => {
                    if skip > 0 {
                        // prevent under flows for block.number == 0 and `block.number - skip <
                        // 0`
                        if let Some(next) =
                            header.number.checked_sub(1).and_then(|num| num.checked_sub(skip))
                        {
                            block = next.into()
                        } else {
                            break
                        }
                    } else {
                        block = header.parent_hash.into()
                    }
                }
            }

            headers.push(header);

            if headers.len() >= MAX_HEADERS_SERVE {
                break
            }

            total_bytes += APPROX_HEADER_SIZE;

            if total_bytes > SOFT_RESPONSE_LIMIT {
                break
            }
        } else {
            break
        }
    }

    headers
}
```

The `GetBlockBodies` payload is simpler, it just contains a vector of requested block hashes:

[File: crates/net/eth-wire/src/types/blocks.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/eth-wire/src/types/blocks.rs)
```rust,ignore
pub struct GetBlockBodies(
    /// The block hashes to request bodies for.
    pub Vec<B256>,
);
```

In handling this request, similarly, the ETH requests task attempts, for each hash in the requested order, to fetch the block body (transactions & ommers), while checking that bounds specifying the maximum numbers of bodies or bytes to send have not been breached.

[File: crates/net/network/src/eth_requests.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/eth_requests.rs)
```rust,ignore
fn on_bodies_request(
    &mut self,
    _peer_id: PeerId,
    request: GetBlockBodies,
    response: oneshot::Sender<RequestResult<BlockBodies>>,
) {
    let mut bodies = Vec::new();

    let mut total_bytes = APPROX_BODY_SIZE;

    for hash in request.0 {
        if let Some(block) = self.client.block(hash.into()).unwrap_or_default() {
            let body = BlockBody { transactions: block.body, ommers: block.ommers };

            bodies.push(body);

            total_bytes += APPROX_BODY_SIZE;

            if total_bytes > SOFT_RESPONSE_LIMIT {
                break
            }

            if bodies.len() >= MAX_BODIES_SERVE {
                break
            }
        } else {
            break
        }
    }

    let _ = response.send(Ok(BlockBodies(bodies)));
}
```

---

## Transactions Task

The transactions task listens for, requests, and propagates transactions both from the node's peers, and those that are added locally (e.g., submitted via RPC). Note that this task focuses solely on the network communication involved with Ethereum transactions, we will talk more about the structure of the transaction pool itself 
in the [transaction-pool](../../../ethereum/transaction-pool/README.md) chapter.

Again, like the network management and ETH requests tasks, the transactions task is implemented as an endless future that runs as a background task on a standalone `tokio::task`. It's represented by the `TransactionsManager` struct:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
pub struct TransactionsManager<Pool> {
    /// Access to the transaction pool.
    pool: Pool,
    /// Network access.
    network: NetworkHandle,
    /// Subscriptions to all network related events.
    ///
    /// From which we get all new incoming transaction related messages.
    network_events: UnboundedReceiverStream<NetworkEvent>,
    /// All currently active requests for pooled transactions.
    inflight_requests: Vec<GetPooledTxRequest>,
    /// All currently pending transactions grouped by peers.
    ///
    /// This way we can track incoming transactions and prevent multiple pool imports for the same
    /// transaction
    transactions_by_peers: HashMap<TxHash, Vec<PeerId>>,
    /// Transactions that are currently imported into the `Pool`
    pool_imports: FuturesUnordered<PoolImportFuture>,
    /// All the connected peers.
    peers: HashMap<PeerId, Peer>,
    /// Send half for the command channel.
    command_tx: mpsc::UnboundedSender<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
    command_rx: UnboundedReceiverStream<TransactionsCommand>,
    /// Incoming commands from [`TransactionsHandle`].
    pending_transactions: ReceiverStream<TxHash>,
    /// Incoming events from the [`NetworkManager`](crate::NetworkManager).
    transaction_events: UnboundedReceiverStream<NetworkTransactionEvent>,
}
```

Unlike the ETH requests task, but like the network management task's `NetworkHandle`, the transactions task can also be accessed via a shareable "handle" struct, the `TransactionsHandle`:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
pub struct TransactionsHandle {
    /// Command channel to the [`TransactionsManager`]
    manager_tx: mpsc::UnboundedSender<TransactionsCommand>,
}
```

### Input Streams to the Transactions Task

We'll touch on most of the fields in the `TransactionsManager` as the chapter continues, but some worth noting now are the 4 streams from which inputs to the task are fed:
- `transaction_events`: A listener for `NetworkTransactionEvent`s sent from the `NetworkManager`, which consist solely of events related to transactions emitted by the network.
- `network_events`: A listener for `NetworkEvent`s sent from the `NetworkManager`, which consist of other "meta" events such as sessions with peers being established or closed.
- `command_rx`: A listener for `TransactionsCommand`s sent from the `TransactionsHandle`
- `pending`: A listener for new pending transactions added to the `TransactionPool`

Let's get a view into the transactions task's operation by walking through the `TransactionManager::poll` method.

### The Operation of the Transactions Task

The `poll` method lays out an order of operations for the transactions task. It begins by draining the `TransactionsManager.network_events`, `TransactionsManager.command_rx`, and `TransactionsManager.transaction_events` streams, in this order.
Then, it checks on all the current `TransactionsManager.inflight_requests`, which are requests sent by the node to its peers for full transaction objects. After this, it checks on the status of completed `TransactionsManager.pool_imports` events, which are transactions that are being imported into the node's transaction pool. Finally, it drains the new `TransactionsManager.pending_transactions` events from the transaction pool.

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
    let this = self.get_mut();

    // drain network/peer related events
    while let Poll::Ready(Some(event)) = this.network_events.poll_next_unpin(cx) {
        this.on_network_event(event);
    }

    // drain commands
    while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
        this.on_command(cmd);
    }

    // drain incoming transaction events
    while let Poll::Ready(Some(event)) = this.transaction_events.poll_next_unpin(cx) {
        this.on_network_tx_event(event);
    }

    // Advance all requests.
    // We remove each request one by one and add them back.
    for idx in (0..this.inflight_requests.len()).rev() {
        let mut req = this.inflight_requests.swap_remove(idx);
        match req.response.poll_unpin(cx) {
            Poll::Pending => {
                this.inflight_requests.push(req);
            }
            Poll::Ready(Ok(Ok(txs))) => {
                this.import_transactions(req.peer_id, txs.0);
            }
            Poll::Ready(Ok(Err(_))) => {
                this.report_bad_message(req.peer_id);
            }
            Poll::Ready(Err(_)) => {
                this.report_bad_message(req.peer_id);
            }
        }
    }

    // Advance all imports
    while let Poll::Ready(Some(import_res)) = this.pool_imports.poll_next_unpin(cx) {
        match import_res {
            Ok(hash) => {
                this.on_good_import(hash);
            }
            Err(err) => {
                this.on_bad_import(*err.hash());
            }
        }
    }

    // handle and propagate new transactions
    let mut new_txs = Vec::new();
    while let Poll::Ready(Some(hash)) = this.pending_transactions.poll_next_unpin(cx) {
        new_txs.push(hash);
    }
    if !new_txs.is_empty() {
        this.on_new_transactions(new_txs);
    }

    // all channels are fully drained and import futures pending

    Poll::Pending
}
```

Let's go through the handling occurring during each of these steps, in order, starting with the draining of the `TransactionsManager.network_events` stream.

#### Handling `NetworkEvent`s

The `TransactionsManager.network_events` stream is the first to have all of its events processed because it contains events concerning peer sessions opening and closing. This ensures, for example, that new peers are tracked in the `TransactionsManager` before events sent from them are processed.

The events received in this channel are of type `NetworkEvent`:

[File: crates/net/network/src/manager.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/manager.rs)
```rust,ignore
pub enum NetworkEvent {
    /// Closed the peer session.
    SessionClosed {
        /// The identifier of the peer to which a session was closed.
        peer_id: PeerId,
        /// Why the disconnect was triggered
        reason: Option<DisconnectReason>,
    },
    /// Established a new session with the given peer.
    SessionEstablished {
        /// The identifier of the peer to which a session was established.
        peer_id: PeerId,
        /// Capabilities the peer announced
        capabilities: Arc<Capabilities>,
        /// A request channel to the session task.
        messages: PeerRequestSender,
        /// The status of the peer to which a session was established.
        status: Status,
    },
    /// Event emitted when a new peer is added
    PeerAdded(PeerId),
    /// Event emitted when a new peer is removed
    PeerRemoved(PeerId),
}
```

They're handled with the `on_network_event` method, which responds to the two variants of the `NetworkEvent` enum in the following ways:

**`NetworkEvent::SessionClosed`**
Removes the peer given by `NetworkEvent::SessionClosed.peer_id` from the `TransactionsManager.peers` map.

**`NetworkEvent::SessionEstablished`**
Begins by inserting a `Peer` into `TransactionsManager.peers` by `peer_id`, which is a struct of the following form:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
struct Peer {
    /// Keeps track of transactions that we know the peer has seen.
    transactions: LruCache<B256>,
    /// A communication channel directly to the session task.
    request_tx: PeerRequestSender,
}
```

Note that the `Peer` struct contains a field `transactions`, which is an [LRU cache](https://en.wikipedia.org/wiki/Cache_replacement_policies#Least_recently_used_(LRU)) of the transactions this peer is aware of. 

The `request_tx` field on the `Peer` is used at the sender end of a channel to send requests to the session with the peer.

After the `Peer` is added to `TransactionsManager.peers`, the hashes of all of the transactions in the node's transaction pool are sent to the peer in a [`NewPooledTransactionHashes` message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08).

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn on_network_event(&mut self, event: NetworkEvent) {
    match event {
        NetworkEvent::SessionClosed { peer_id, .. } => {
            // remove the peer
            self.peers.remove(&peer_id);
        }
        NetworkEvent::SessionEstablished { peer_id, messages, .. } => {
            // insert a new peer
            self.peers.insert(
                peer_id,
                Peer {
                    transactions: LruCache::new(
                        NonZeroUsize::new(PEER_TRANSACTION_CACHE_LIMIT).unwrap(),
                    ),
                    request_tx: messages,
                },
            );

            // Send a `NewPooledTransactionHashes` to the peer with _all_ transactions in the
            // pool
            let msg = NewPooledTransactionHashes(self.pool.pooled_transactions());
            self.network.send_message(NetworkHandleMessage::SendPooledTransactionHashes {
                peer_id,
                msg,
            })
        }
        _ => {}
    }
}
```

#### Handling `TransactionsCommand`s

Next in the `poll` method, `TransactionsCommand`s sent through the `TransactionsManager.command_rx` stream are handled. These are the next to be handled as they are those sent manually via the `TransactionsHandle`, giving them precedence over transactions-related requests picked up from the network. The `TransactionsCommand` enum has the following form:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
enum TransactionsCommand {
    PropagateHash(B256),
}
```

`TransactionsCommand`s are handled by the `on_command` method. This method responds to the, at the time of writing, sole variant of the `TransactionsCommand` enum, `TransactionsCommand::PropagateHash`, with the `on_new_transactions` method, passing in an iterator consisting of the single hash contained by the variant (though this method can be called with many transaction hashes).

`on_new_transactions` propagates the full transaction object, with the signer attached, to a small random sample of peers using the `propagate_transactions` method. Then, it notifies all other peers of the hash of the new transaction, so that they can request the full transaction object if they don't already have it.

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn on_new_transactions(&mut self, hashes: impl IntoIterator<Item = TxHash>) {
    trace!(target: "net::tx", "Start propagating transactions");

    let propagated = self.propagate_transactions(
        self.pool
            .get_all(hashes)
            .into_iter()
            .map(|tx| {
                (*tx.hash(), Arc::new(tx.transaction.to_recovered_transaction().into_signed()))
            })
            .collect(),
    );

    // notify pool so events get fired
    self.pool.on_propagated(propagated);
}

fn propagate_transactions(
    &mut self,
    txs: Vec<(TxHash, Arc<TransactionSigned>)>,
) -> PropagatedTransactions {
    let mut propagated = PropagatedTransactions::default();

    // send full transactions to a fraction fo the connected peers (square root of the total
    // number of connected peers)
    let max_num_full = (self.peers.len() as f64).sqrt() as usize + 1;

    // Note: Assuming ~random~ order due to random state of the peers map hasher
    for (idx, (peer_id, peer)) in self.peers.iter_mut().enumerate() {
        let (hashes, full): (Vec<_>, Vec<_>) =
            txs.iter().filter(|(hash, _)| peer.transactions.insert(*hash)).cloned().unzip();

        if !full.is_empty() {
            if idx > max_num_full {
                for hash in &hashes {
                    propagated.0.entry(*hash).or_default().push(PropagateKind::Hash(*peer_id));
                }
                // send hashes of transactions
                self.network.send_transactions_hashes(*peer_id, hashes);
            } else {
                // send full transactions
                self.network.send_transactions(*peer_id, full);

                for hash in hashes {
                    propagated.0.entry(hash).or_default().push(PropagateKind::Full(*peer_id));
                }
            }
        }
    }

    propagated
}
```

#### Handling `NetworkTransactionEvent`s

After `TransactionsCommand`s, it's time to take care of transactions-related requests sent by peers in the network, so the `poll` method handles `NetworkTransactionEvent`s received through the `TransactionsManager.transaction_events` stream. `NetworkTransactionEvent` has the following form:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
pub enum NetworkTransactionEvent {
    /// Received list of transactions from the given peer.
    IncomingTransactions { peer_id: PeerId, msg: Transactions },
    /// Received list of transactions hashes to the given peer.
    IncomingPooledTransactionHashes { peer_id: PeerId, msg: NewPooledTransactionHashes },
    /// Incoming `GetPooledTransactions` request from a peer.
    GetPooledTransactions {
        peer_id: PeerId,
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
}
```

These events are handled with the `on_network_tx_event` method, which responds to the variants of the `NetworkTransactionEvent` enum in the following ways:

**`NetworkTransactionEvent::IncomingTransactions`**

This event is generated from the [`Transactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#transactions-0x02), and is handled by the `import_transactions` method.

Here, for each transaction in the variant's `msg` field, we attempt to recover the signer, insert the transaction into LRU cache of the `Peer` identified by the variant's `peer_id` field, and add the `peer_id` to the vector of peer IDs keyed by the transaction's hash in `TransactionsManager.transactions_by_peers`. If an entry does not already exist for the transaction hash, then it begins importing the transaction object into the node's transaction pool, adding a `PoolImportFuture` to `TransactionsManager.pool_imports`. If there was an issue recovering the signer, `report_bad_message` is called for the `peer_id`, which decreases the peer's reputation.

To understand this a bit better, let's double back and examine what `TransactionsManager.transactions_by_peers` and `TransactionsManager.pool_imports` are used for.

`TransactionsManager.transactions_by_peers` is a `HashMap<TxHash, Vec<PeerId>>`, tracks which peers have sent us a transaction with the given hash. This has two uses: the first being that it prevents us from redundantly importing transactions into the transaction pool for which we've already begun this process (this check occurs in `import_transactions`), and the second being that if a transaction we receive is malformed in some way and ends up erroring when imported to the transaction pool, we can reduce the reputation score for all of the peers that sent us this transaction (this occurs in `on_bad_import`, which we'll touch on soon).

`TransactionsManager.pool_imports` is a set of futures representing the transactions which are currently in the process of being imported to the node's transaction pool. This process is asynchronous due to the validation of the transaction that must occur, thus we need to keep a handle on the generated future.

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn import_transactions(&mut self, peer_id: PeerId, transactions: Vec<TransactionSigned>) {
    let mut has_bad_transactions = false;
    if let Some(peer) = self.peers.get_mut(&peer_id) {
        for tx in transactions {
            // recover transaction
            let tx = if let Some(tx) = tx.into_ecrecovered() {
                tx
            } else {
                has_bad_transactions = true;
                continue
            };

            // track that the peer knows this transaction
            peer.transactions.insert(tx.hash);

            match self.transactions_by_peers.entry(tx.hash) {
                Entry::Occupied(mut entry) => {
                    // transaction was already inserted
                    entry.get_mut().push(peer_id);
                }
                Entry::Vacant(entry) => {
                    // this is a new transaction that should be imported into the pool
                    let pool_transaction = <Pool::Transaction as FromRecoveredTransaction>::from_recovered_transaction(tx);

                    let pool = self.pool.clone();
                    let import = Box::pin(async move {
                        pool.add_external_transaction(pool_transaction).await
                    });

                    self.pool_imports.push(import);
                    entry.insert(vec![peer_id]);
                }
            }
        }
    }

    if has_bad_transactions {
        self.report_bad_message(peer_id);
    }
}
```

**`NetworkTransactionEvent::IncomingPooledTransactionHashes`**

This event is generated from the [`NewPooledTransactionHashes` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08), and is handled by the `on_new_pooled_transactions` method.

Here, it begins by adding the transaction hashes included in the `NewPooledTransactionHashes` payload to the LRU cache for the `Peer` identified by `peer_id` in `TransactionsManager.peers`. Next, it filters the list of hashes to those that are not already present in the transaction pool, and for each such hash, requests its full transaction object from the peer by sending it a [`GetPooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09) through the `Peer.request_tx` channel. If the request was successfully sent, a `GetPooledTxRequest` gets added to `TransactionsManager.inflight_requests` vector:

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
struct GetPooledTxRequest {
    peer_id: PeerId,
    response: oneshot::Receiver<RequestResult<PooledTransactions>>,
}
```

As you can see, this struct also contains a `response` channel from which the peer's response can later be polled.

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn on_new_pooled_transactions(&mut self, peer_id: PeerId, msg: NewPooledTransactionHashes) {
    if let Some(peer) = self.peers.get_mut(&peer_id) {
        let mut transactions = msg.0;

        // keep track of the transactions the peer knows
        peer.transactions.extend(transactions.clone());

        self.pool.retain_unknown(&mut transactions);

        if transactions.is_empty() {
            // nothing to request
            return
        }

        // request the missing transactions
        let (response, rx) = oneshot::channel();
        let req = PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(transactions),
            response,
        };

        if peer.request_tx.try_send(req).is_ok() {
            self.inflight_requests.push(GetPooledTxRequest { peer_id, response: rx })
        }
    }
}
```

**`NetworkTransactionEvent::GetPooledTransactions`**

This event is generated from the [`GetPooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09), and is handled by the `on_get_pooled_transactions` method.

Here, it collects _all_ the transactions in the node's transaction pool, recovers their signers, adds their hashes to the LRU cache of the requesting peer, and sends them to the peer in a [`PooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#pooledtransactions-0x0a). This is sent through the `response` channel that's stored as a field of the `NetworkTransaction::GetPooledTransactions` variant itself.

[File: crates/net/network/src/transactions.rs](https://github.com/paradigmxyz/reth/blob/1563506aea09049a85e5cc72c2894f3f7a371581/crates/net/network/src/transactions.rs)
```rust,ignore
fn on_get_pooled_transactions(
    &mut self,
    peer_id: PeerId,
    request: GetPooledTransactions,
    response: oneshot::Sender<RequestResult<PooledTransactions>>,
) {
    if let Some(peer) = self.peers.get_mut(&peer_id) {
        let transactions = self
            .pool
            .get_all(request.0)
            .into_iter()
            .map(|tx| tx.transaction.to_recovered_transaction().into_signed())
            .collect::<Vec<_>>();

        // we sent a response at which point we assume that the peer is aware of the transaction
        peer.transactions.extend(transactions.iter().map(|tx| tx.hash()));

        let resp = PooledTransactions(transactions);
        let _ = response.send(Ok(resp));
    }
}
```

#### Checking on `inflight_requests`

Once all the network activity is handled by draining `TransactionsManager.network_events`, `TransactionsManager.command_rx`, and `TransactionsManager.transaction_events` streams, the `poll` method moves on to checking the status of all `inflight_requests`.

Here, for each in-flight request, `GetPooledTxRequest.response` field gets polled. If the request is still pending, it remains in the `TransactionsManager.inflight_requests` vector. If the request successfully received a `PooledTransactions` response from the peer, they get handled by the `import_transactions` method (described above). Otherwise, if there was some error in polling the response, we call `report_bad_message` (also described above) on the peer's ID.

#### Checking on `pool_imports`

When the last round of `PoolImportFuture`s has been added to `TransactionsManager.pool_imports` after handling the completed `inflight_requests`, the `poll` method continues by checking the status of the `pool_imports`.

It iterates over `TransactionsManager.pool_imports`, polling each one, and if it's ready (i.e., the future has resolved), it handles successful and unsuccessful import results respectively with `on_good_import` and `on_bad_import`.

`on_good_import`, called when the transaction was successfully imported into the transaction pool, removes the entry for the given transaction hash from `TransactionsManager.transactions_by_peers`.

`on_bad_import` also removes the entry for the given transaction hash from `TransactionsManager.transactions_by_peers`, but also calls `report_bad_message` for each peer in the entry, decreasing all of their reputation scores as they were propagating a transaction that could not validated.

#### Checking on `pending_transactions`

Finally, the last thing for the `poll` method to do is to drain the `TransactionsManager.pending_transactions` stream. These transactions are those that were added either via propagation from a peer, the handling of which has been laid out above, or via RPC on the node itself, and which were successfully validated and added to the transaction pool.

It polls `TransactionsManager.pending_transactions`, collecting each resolved transaction into a vector, and calls `on_new_transactions` with said vector. The functionality of the `on_new_transactions` method is described above in the handling of `TransactionsCommand::PropagateHash`.
