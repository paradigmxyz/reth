# Network

The `network` crate is responsible for managing the node's connection to the Ethereum peer-to-peer (P2P) network, enabling communication with other nodes via the [various P2P subprotocols](https://github.com/ethereum/devp2p).

Reth's P2P networking consists primarily of 4 ongoing tasks:
- **Discovery**: Discovers new peers in the network
- **Transactions**: Accepts, requests, and broadcasts mempool transactions
- **ETH Requests**: Responds to incoming requests for headers and bodies
- **Network Management**: Handles incoming & outgoing connections with peers, and routes requests between peers and the other tasks

We'll leave most of the discussion of the discovery task for the [discv4](../discv4/README.md) chapter, and will focus on the other three here.

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

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=bin/reth/src/node/mod.rs anchor=snippet-execute}}

The variables `network` and `fetch_client` are of types `NetworkHandle` and `FetchClient`, respectively. These are the two main interfaces for interacting with the P2P network, and are currently used in the `HeaderStage` and `BodyStage`.

Let's walk through how each is implemented, and then apply that knowledge to understand how they are used in the pipeline.

### Interacting with the Network Management Task Using `NetworkHandle`

The `NetworkHandle` struct is a client for the network management task that can be shared across threads. It wraps an `Arc` around the `NetworkInner` struct, defined as follows:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/network.rs anchor=struct-NetworkInner}}

The field of note here is `to_manager_tx`, which is a handle that can be used to send messages in a channel to an instance of the `NetworkManager` struct.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/manager.rs anchor=struct-NetworkManager}}

Now we're getting to the meat of the `network` crate! The `NetworkManager` struct represents the "Network Management" task described above. It is implemented as an endless [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) that can be thought of as a "hub" process which listens for messages from the `NetworkHandle` or the node's peers and dispatches messages to the other tasks, while keeping track of the state of the network.

#### Instantiating the `NetworkHandle`

Back in the `execute` method of the `"node"` CLI command, we saw the `NetworkHandle` get returned from a call to `start_network`. Sounds important, doesn't it?

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=bin/reth/src/node/mod.rs anchor=fn-start_network}}

At a high level, this function is responsible for starting the tasks listed at the start of this chapter.

It gets the handles for the network management, transactions, and ETH requests tasks downstream of the `NetworkManager::builder` method call, and spawns them.

TODO: Go into more depth regarding `NetworkBuilder`, `NetworkConfig`, etc?

The discovery task progresses as the network management task is polled, handling events regarding peer management through the `Swarm` struct which is stored as a field on the `NetworkManager`:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/swarm.rs anchor=struct-Swarm}}

TODO: Go into more depth about `Swarm`?

More information about the discovery task can be found in the [discv4](../discv4/README.md) chapter.

The ETH requests and transactions task will be explained in their own sections, following this one.

#### Usage of `NetworkHandle` in the Pipeline

In the pipeline, the `NetworkHandle` is used to instantiate the `FetchClient` - which we'll get into next - and is used in the `HeaderStage` to update the node's ["status"](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#status-0x00) (record the the total difficulty, hash, and height of the last processed block).

Now that we have some understanding about the internals of the network management task, let's look at a higher-level abstraction that can be used to retrieve data from other peers: the `FetchClient`.

### Using `FetchClient` to Get Data in the Pipeline Stages

The `FetchClient` struct, similar to `NetworkHandle`, can be shared across threads, and is a client for fetching data from the network. It's a fairly lightweight struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/client.rs anchor=struct-FetchClient}}

The `request_tx` field is a handle to a channel that can be used to send requests for downloading data, and 
The `peers_handle` field is a wrapper struct around a handle to a channel that can be used to send messages for applying manual changes to the peer set.

#### Instantiating the `FetchClient`

The fields `request_tx` and `peers_handle` are cloned off of the `StateFetcher` struct when instantiating the `FetchClient`, which is the lower-level struct responsible for managing data fetching operations over the network:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/mod.rs anchor=struct-StateFetcher}}

This struct itself is nested deeply within the `NetworkManager`: its `Swarm` struct (shown earlier in the chapter) contains a `NetworkState` struct that has the `StateFetcher` as a field:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/state.rs anchor=struct-NetworkState}}

#### Usage of `FetchClient` in the Pipeline

The `FetchClient` implements the `HeadersClient` and `BodiesClient` traits, defining the funcionality to get headers and block bodies from available peers.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/client.rs anchor=trait-HeadersClient-BodiesClient}}

This functionality is used in the `HeaderStage` and `BodyStage`, respectively.

In the pipeline used by the main Reth binary, the `HeaderStage` uses a `LinearDownloader` to stream headers from the network:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/downloaders/src/headers/linear.rs anchor=struct-LinearDownloader}}

A `FetchClient` is passed in to the `client` field, and the `get_headers` method it implements gets used when polling the stream created by the `LinearDownloader` in the `execute` method of the `HeaderStage`.

In the `BodyStage` configured by the main binary, a `ConcurrentDownloader` is used:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/downloaders/src/bodies/concurrent.rs anchor=struct-ConcurrentDownloader}}

Here, similarly, a `FetchClient` is passed in to the `client` field, and the `get_block_bodies` method it implements is used when constructing the stream created by the `ConcurrentDownloader` in the `execute` method of the `BodyStage`.

---

## ETH Requests Task

- Only serves _incoming_ requests for headers/bodies from _peers_ (?)
    - Outbound `BlockRequest`s (for headers or bodies) are dispatched to a peer in the `handle_block_request` method of the `NetworkState` struct
- Runs as a background task, not to be interacted with by the pipeline
- I can see where the task gets spawned in the pipeline (`start_network`), but how does it establish channels w/ other peers that it listens on?
    - Probably need to look back to the constructors of the `NetworkManager` struct where this struct gets created
    - Yup, look to `request_handler` method on `NetworkBuilder`
        - Spins up a channel, saves the sender endpoint with `set_eth_request_channel`, instantiates `EthRequestHandler` with the receiver endpoint
        - Sender endpoint is stored on the `NetworkManager` struct, still interesting to see where it gets used (i.e how incoming requests are listened for)
            - This is accomplished with `delegate_eth_request`, sends an `IncomingEthRequest` into the channel's sender endpoint
            - Used in `on_eth_request`, which is used in `on_peer_message`, which is used in `poll` (all on `NetworkManager`)
- Generic over some client type "that can interact with the chain," this is unclear to me
    - Isn't the node what interacts with the chain? Does this mean local stored state?
    - After digging around (follow from `network_config` -> `ProviderImpl::new()`), the "client" is a client for fetching data from the database
- Similar to the network management task, is an endless future
    - `poll` method:
        - Pops from a queue of incoming requests
        - Currently is able to handle `GetBlockHeaders` and `GetBlockBodies` requests
- `on_headers_request`
    - `GetBlockHeaders` request comes with a oneshot response channel attached to it
        - Should mention the `GetBlockHeaders` message payload w/ link to spec in `devp2p`
    - `get_headers_response`
        - Get block hash of requested start block
            - Why do we convert a block number into a hash if we fetch using `client.header_by_hash_or_number` and we seem to proceed by fetching with the number in the loop?
        - For as many blocks as requested (`limit`):
            - Fetch the header by hash from the DB, then increment/decrement (depending on `direction`) the header number by `skip` (with checks for overflow/underflow)
            - Add header to result vec
            - Check for bounds on max # headers to send & max # of bytes to send
- `on_bodies_request`
    - `GetBlockBodies` request is simpler in structure, just a vec of requested hashes
    - For each requested hash (in requested order):
        - Try to fetch block from DB, package into `BlockBody` struct, add to result vec
        - Check for bounds on max # of blocks & max # of bytes

The ETH requests task serves _incoming_ requests related to blocks in the [`eth` P2P subprotocol](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages) from other peers.

Similar to the network management task, it's implemented as an endless future, but it is meant to run as a background task and not to be interacted with directly from the pipeline. It's represented by the following `EthRequestHandler` struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=struct-EthRequestHandler}}

The `client` field here is a client that's used to fetch data from the database, not to be confused with the `client` field on a downloader like the `LinearDownloader` discussed above, which is a `FetchClient`.

The `incoming_requests` field is the receiver end of a channel that accepts, as you might have guessed, incoming ETH requests from peers. The sender end of this channel is stored on the `NetworkManager` struct as the `to_eth_request_handler` field.

As the `NetworkManager` is polled and listens for events from peers passed through the `Swarm` struct it holds, it sends any received ETH requests into the channel.

Then, as the `EthRequestHandler` is polled, it listens for any ETH requests coming through the channel, and handles them accordingly. At the time of writing, the ETH requests task can handle the [`GetBlockHeaders`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03) and [`GetBlockBodies`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockbodies-0x05) requests.

The handling of these requests is fairly straightforward. The `GetBlockHeaders` payload is the following:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/eth-wire/src/types/blocks.rs anchor=struct-GetBlockHeaders}}

In handling this request, the ETH requests task attempts, starting with `start_block`, to fetch the associated header from the database, increment/decrement the block number to fetch by `skip` depending on the `direction` while checking for overflow/underflow, and checks that bounds specifying the maximum numbers of headers or bytes to send have not been breached.

The `GetBlockBodies` payload is simpler, it just contains a vector of requested block hashes:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/eth-wire/src/types/blocks.rs anchor=struct-GetBlockBodies}}

In handling this request, similarly, the ETH requests task attempts, for each hash in the requested order, to fetch the block body (transactions & ommers), while checking that bounds specifying the maximum numbers of bodies or bytes to send have not been breached.

---

## Transactions Task