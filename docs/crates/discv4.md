# Discv4

The `discv4` crate plays an important role in Reth, enabling discovery of other peers across the network. It is recommended to know how [Kademlia distributed hash tables](https://en.wikipedia.org/wiki/Kademlia) and [Ethereum's node discovery protocol](https://github.com/ethereum/devp2p/blob/master/discv4.md) work before reading through this chapter. While all concepts will be explained through the following sections, reading through the links above will make understanding this chapter much easier! With that note out of the way, lets jump into `disc4`.

## Starting the Node Discovery Protocol
As mentioned in the network and stages chapters, when the node is first started up, the `node::Command::execute()` function is called, which initializes the node and starts to run the Reth pipeline. Throughout the initialization of the node, there are many processes that are are started. One of the processes that is initialized is the p2p network which starts the node discovery protocol amongst other tasks.  

[File: bin/reth/src/node/mod.rs](https://github.com/paradigmxyz/reth/blob/main/bin/reth/src/node/mod.rs#L95)
```rust ignore
  pub async fn execute(&self) -> eyre::Result<()> {
    //--snip--
    let network = config
        .network_config(db.clone(), chain_id, genesis_hash, self.network.disable_discovery)
        .start_network()
        .await?;

    info!(peer_id = ?network.peer_id(), local_addr = %network.local_addr(), "Started p2p networking");

    //--snip--
    }
```

During this process, a new `NetworkManager` is created through the `NetworkManager::new()` function, which starts the discovery protocol through a handful of newly spawned tasks. Lets take a look at how this actually works under the hood. 

[File: crates/net/network/src/manager.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/network/src/manager.rs#L147)
```rust ignore
impl<C> NetworkManager<C>
where
    C: BlockProvider,
{
    //--snip--

    pub async fn new(config: NetworkConfig<C>) -> Result<Self, NetworkError> {
        let NetworkConfig {
            //--snip--
            secret_key,
            mut discovery_v4_config,
            discovery_addr,
            boot_nodes,
            //--snip--
            ..
        } = config;

        //--snip--

        discovery_v4_config = discovery_v4_config.map(|mut disc_config| {
            // merge configured boot nodes
            disc_config.bootstrap_nodes.extend(boot_nodes.clone());
            disc_config.add_eip868_pair("eth", status.forkid);
            disc_config
        });

        let discovery = Discovery::new(discovery_addr, secret_key, discovery_v4_config).await?;

    //--snip--
    }
}
```

First, the `NetworkConfig` is deconstructed and the `disc_config` is updated to merge configured [bootstrap nodes](https://github.com/paradigmxyz/reth/blob/main/crates/net/discv4/src/bootnodes.rs#L8) and add the `forkid` to adhere to [EIP 868](https://eips.ethereum.org/EIPS/eip-868). This updated configuration variable is then passed into the `Discovery::new()` function. Note that `Discovery` is a catch all for all discovery services, which include discv4, DNS discovery and others in the future.

[File: crates/net/network/src/discovery.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/network/src/discovery.rs#L51)
```rust ignore
impl Discovery {
    /// Spawns the discovery service.
    ///
    /// This will spawn the [`reth_discv4::Discv4Service`] onto a new task and establish a listener
    /// channel to receive all discovered nodes.
    pub async fn new(
        discovery_addr: SocketAddr,
        sk: SecretKey,
        discv4_config: Option<Discv4Config>,
    ) -> Result<Self, NetworkError> {
        let local_enr = NodeRecord::from_secret_key(discovery_addr, &sk);
        
        let (discv4, discv4_updates, _discv4_service) = if let Some(disc_config) = discv4_config {
            let (discv4, mut discv4_service) =
                Discv4::bind(discovery_addr, local_enr, sk, disc_config)
                    .await
                    .map_err(NetworkError::Discovery)?;
            let discv4_updates = discv4_service.update_stream();
            // spawn the service
            let _discv4_service = discv4_service.spawn();
            (Some(discv4), Some(discv4_updates), Some(_discv4_service))
        } else {
            (None, None, None)
        };

        Ok(Self {
            local_enr,
            discv4,
            discv4_updates,
            _discv4_service,
            discovered_nodes: Default::default(),
            queued_events: Default::default(),
        })
    }
}
```

Within this function, the [Ethereum Node Record](https://github.com/ethereum/devp2p/blob/master/enr.md) is created. Participants in the discovery protocol are expected to maintain a node record (ENR) containing up-to-date information of different nodes in the network. All records must use the "v4" identity scheme. Other nodes may request the local record at any time by sending an ["ENRRequest" packet](https://github.com/ethereum/devp2p/blob/master/discv4.md#enrrequest-packet-0x05).

The `NodeRecord::from_secret_key()` takes the socket address used for discovery and the secret key. The secret key is used to derive a `secp256k1` public key and the peer id is then derived from the public key. These values are then used to create an ENR. Ethereum Node Records are used to locate and communicate with other nodes in the network.

If the `discv4_config` supplied to the `Discovery::new()` function is `None`, the discv4 service will not be spawned. In this case, no new peers will be discovered across the network. The node will have to rely on manually added peers. However, if the `discv4_config` contains a `Some(Discv4Config)` value, then the `Discv4::bind()` function is called to bind to a new UdpSocket and create the disc_v4 service.

[File: crates/net/discv4/src/lib.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/discv4/src/lib.rs#L188)
```rust ignore
impl Discv4 {
    //--snip--
    pub async fn bind(
        local_address: SocketAddr,
        mut local_node_record: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
    ) -> io::Result<(Self, Discv4Service)> {
        let socket = UdpSocket::bind(local_address).await?;
        let local_addr = socket.local_addr()?;
        local_node_record.udp_port = local_addr.port();
        trace!( target : "discv4",  ?local_addr,"opened UDP socket");

        let (to_service, rx) = mpsc::channel(100);
        
        let service =
            Discv4Service::new(socket, local_addr, local_node_record, secret_key, config, Some(rx));
        
        let discv4 = Discv4 { local_addr, to_service };
        Ok((discv4, service))
    }
    //--snip--
}
```

To better understand what is actually happening when the disc_v4 service is created, lets take a deeper look at the `Discv4Service::new()` function.

[File: crates/net/discv4/src/lib.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/discv4/src/lib.rs#L392)
```rust ignore
impl Discv4Service {
    /// Create a new instance for a bound [`UdpSocket`].
    pub(crate) fn new(
        socket: UdpSocket,
        local_address: SocketAddr,
        local_node_record: NodeRecord,
        secret_key: SecretKey,
        config: Discv4Config,
        commands_rx: Option<mpsc::Receiver<Discv4Command>>,
    ) -> Self {
        let socket = Arc::new(socket);
        let (ingress_tx, ingress_rx) = mpsc::channel(config.udp_ingress_message_buffer);
        let (egress_tx, egress_rx) = mpsc::channel(config.udp_egress_message_buffer);
        let mut tasks = JoinSet::<()>::new();

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { receive_loop(udp, ingress_tx, local_node_record.id).await });

        let udp = Arc::clone(&socket);
        tasks.spawn(async move { send_loop(udp, egress_rx).await });

        let kbuckets = KBucketsTable::new(
            NodeKey::from(&local_node_record).into(),
            Duration::from_secs(60),
            MAX_NODES_PER_BUCKET,
            None,
            None,
        );

        let self_lookup_interval = tokio::time::interval(config.lookup_interval);

        // Wait `ping_interval` and then start pinging every `ping_interval`
        let ping_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + config.ping_interval,
            config.ping_interval,
        );

        let evict_expired_requests_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + config.request_timeout,
            config.request_timeout,
        );

        //--snip--
    }
//--snip--
}
```

First, new channels are initialized to handle incoming and outgoing traffic related to node discovery. New tasks are then spawned to handle the `receive_loop()` and `send_loop()` functions, which perpetually send incoming disc4 traffic to the `ingress_rx` and outgoing discv4 traffic to the `egress_rx`. Following this, a [`KBucketsTable`](https://docs.rs/discv5/0.1.0/discv5/kbucket/struct.KBucketsTable.html) is initialized to keep track of the node's neighbors throughout the network. If you are unfamiliar with k-buckets, feel free to [follow this link to learn more](https://en.wikipedia.org/wiki/Kademlia#Fixed-size_routing_tables). Following this, the `self_lookup_interval`, `ping_interval` and `evict_expired_requests_interval` are initialized which determines the interval used for periodically querying the network for new neighboring nodes with a low [distance](https://github.com/ethereum/devp2p/blob/master/discv4.md#node-identities) relative to ours, outgoing ping packets and the time elapsed to remove inactive neighbors from the node's records.

Once the `Discv4Service::new()` function completes, allowing the `Discv4::bind()` function to complete as well, the `discv4_service` is then spawned into its own task and the `Discovery::new()` function returns an new instance of `Discovery`. The newly created `Discovery` type is passed into the `NetworkState::new()` function along with a few other arguments to initialize the network state. The network state is added to the `NetworkManager` where the `NetworkManager` is then spawned into its own task as well.



## Polling the Discv4Service and Discovery Events
In Rust, the owner of a [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html#) is responsible for advancing the computation by polling the future. This is done by calling `Future::poll`.

Lets take a detailed look at how `Discv4Service::poll` works under the hood. This function has many moving parts, so we will break it up into smaller sections.

[File: crates/net/discv4/src/lib.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/discv4/src/lib.rs#L1302)
```rust ignore
impl Discv4Service {
    //--snip--
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Discv4Event> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_events.pop_front() {
                return Poll::Ready(event);
            }

            // trigger self lookup
            if self.config.enable_lookup
                && !self.is_lookup_in_progress()
                && self.lookup_interval.poll_tick(cx).is_ready()
            {
                let target = self.lookup_rotator.next(&self.local_node_record.id);
                self.lookup_with(target, None);
            }

            // evict expired nodes
            if self.evict_expired_requests_interval.poll_tick(cx).is_ready() {
                self.evict_expired_requests(Instant::now())
            }

            // re-ping some peers
            if self.ping_interval.poll_tick(cx).is_ready() {
                self.re_ping_oldest();
            }

            if let Some(Poll::Ready(Some(ip))) =
                self.resolve_external_ip_interval.as_mut().map(|r| r.poll_tick(cx))
            {
                self.set_external_ip_addr(ip);
            }
        // --snip--
        }
    }
}
```

As the function starts, a `loop` is entered and the `Discv4Service.queued_events` are evaluated to see if there are any events ready to be processed. If there is an event ready, the function immediately returns the event wrapped in `Poll::Ready()`. The `queued_events` field is a `VecDeque<Discv4Event>` where `Discv4Event` is an enum containing one of the following variants.

[File: crates/net/discv4/src/lib.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/discv4/src/lib.rs#L1455)
```rust ignore
pub enum Discv4Event {
    /// A `Ping` message was handled.
    Ping,
    /// A `Pong` message was handled.
    Pong,
    /// A `FindNode` message was handled.
    FindNode,
    /// A `Neighbours` message was handled.
    Neighbours,
    /// A `EnrRequest` message was handled.
    EnrRequest,
    /// A `EnrResponse` message was handled.
    EnrResponse,
}
```

If there is not a `Discv4Event` immediately ready, the function continues triggering self lookup, and removing any nodes that have not sent a ping request in the allowable time elapsed (`now.duration_since(enr_request.sent_at) < Discv4Service.config.ping_expiration`). Additionally, the node re-pings all nodes whose endpoint proofs (explained below) are considered expired. If the node fails to respond to the "ping" with a "pong", the node will be removed from the `KbucketsTable`.

To prevent traffic amplification attacks (ie. DNS attacks), implementations must verify that the sender of a query participates in the discovery protocol. The sender of a packet is considered verified if it has sent a valid Pong response with matching ping hash within the last 12 hours. This is called the ["endpoint proof"](https://github.com/ethereum/devp2p/blob/master/discv4.md#endpoint-proof).

Next, the Discv4Service handles all incoming `Discv4Command`s until there are no more commands to be processed. Following this, All `IngressEvent`s are handled, which represent all incoming datagrams related to the discv4 protocol. After all events are handled, the node pings to active nodes in it's network. This process is repeated until all of the `Discv4Event`s in `queued_events` are processed. 

In Reth, once a new `NetworkState` is initialized as the node starts up and a new task is spawned to handle the network, the `poll()` function is used to advance the state of the network.

[File: crates/net/network/src/state.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/network/src/state.rs#L377)
```rust ignore
impl<C> NetworkState<C>
where
    C: BlockProvider,
{
    /// Advances the state
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<StateAction> {
        loop {
            //--snip--
            while let Poll::Ready(discovery) = self.discovery.poll(cx) {
                self.on_discovery_event(discovery);
            }
            //--snip--
        }
    }
}
```

Every time that the network is polled, the `Discovery::poll()` is also called to handle all `DiscoveryEvent`s ready to be processed. There are two types of `DiscoveryEvent`s, `DiscoveryEvent::Discovered` and `DiscoveryEvent::EnrForkId`. If a new node is discovered, the new peer is added to the node's peers. If an ENR fork Id is received, the event is pushed to a queue of messages that will later be handled by the `NetworkState`.

Lets recap everything that we covered. The `discv4` crate enables the node to discover new peers across the network. When the node is started, a `NetworkManager` is created which initializes a new `Discovery` type, which initializes the `Discv4Service`. When the `Discv4Service` is polled, all `Discv4Command`s, `IngressEvent`s and `DiscV4Event`s are handled until the `queued_events` are empty. This process repeats every time the `NetworkState` is polled to allow the node discover and communicate with new peers across the network.