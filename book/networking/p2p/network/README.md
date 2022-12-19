# Network

The `network` crate is responsible for managing the node's connection to the Ethereum peer-to-peer (P2P) network, enabling communication with other nodes via the [various P2P subprotocols](https://github.com/ethereum/devp2p).

Reth's P2P networking consists primarily of 4 ongoing tasks:
- **Discovery**: Discovers new peers in the network
- **Transactions**: Accepts, requests, and broadcasts mempool transactions
- **ETH Requests**: Responds to incoming requests for headers and bodies
- **Network Management**: Handles incoming & outgoing connections with peers, and routes requests between peers and the other tasks

We'll leave most of the discussion of the discovery task for the [discv4](../discv4/README.md) chapter, and will focus on the other three here.

Let's take a look at how the main Reth CLI (i.e., a default-configured full node) makes use of the P2P layer to explore the primary interfaces and entrypoints into the `network` crate.

## Use of the Network in the Node

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

## Interacting with the Network Management Task Using `NetworkHandle`

The `NetworkHandle` struct is a client for the network management task that can be shared across threads. It wraps an `Arc` around the `NetworkInner` struct, defined as follows:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/network.rs anchor=struct-NetworkInner}}

The field of note here is `to_manager_tx`, which is a handle that can be used to send messages in a channel to an instance of the `NetworkManager` struct.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/manager.rs anchor=struct-NetworkManager}}

Now we're getting to the meat of the `network` crate! The `NetworkManager` struct represents the "Network Management" task described above. It is implemented as an endless [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) that can be thought of as a "hub" process which listens for messages from the `NetworkHandle` or the node's peers and dispatches messages to the other tasks, while keeping track of the state of the network.

### Instantiating the `NetworkHandle`

Back in the `execute` method of the `"node"` CLI command, we saw the `NetworkHandle` get returned from a call to `start_network`. Sounds important, doesn't it?

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=bin/reth/src/node/mod.rs anchor=fn-start_network}}

At a high level, this function is responsible for starting the tasks listed at the start of this chapter.

It gets the handles for the network management, transactions, and ETH requests tasks downstream of the `NetworkManager::builder` method call, and spawns them.

The discovery task progresses as the network management task is polled, handling events regarding peer management through the `Swarm` struct which is stored as a field on the `NetworkManager`:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/swarm.rs anchor=struct-Swarm}}

The transactions task is represented by the `TransactionsManager` struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=struct-TransactionsManager}}

And the ETH requests task by the `EthRequestHandler` struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=struct-EthRequestHandler}}

Please take a look at these structs (`NetworkManager`, `Swarm`, `TransactionsManager`, and `EthRequestHandler`) for lower-level insight on how these tasks function.

### Usage of `NetworkHandle` in the Pipeline

In the pipeline, the `NetworkHandle` is used to instantiate the `FetchClient` - which we'll get into next - and is used in the `HeaderStage` to update the node's ["status"](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#status-0x00) (record the the total difficulty, hash, and height of the last processed block).

But first, let's take a look at what happens when the `NetworkHandle` itself gets instantiated.

## Using `FetchClient` to Get Data in the Pipeline Stages

The `FetchClient` struct, similar to `NetworkHandle`, can be shared across threads, and is a client for fetching data from the network. It's a fairly lightweight struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/client.rs anchor=struct-FetchClient}}

The `request_tx` field is a handle to a channel that can be used to send requests for downloading data, and 
The `peers_handle` field is a wrapper struct around a handle to a channel that can be used to send messages for applying manual changes to the peer set.

### Instantiating the `FetchClient`

The fields `request_tx` and `peers_handle` are cloned off of the `StateFetcher` struct when instantiating the `FetchClient`, which is the lower-level struct responsible for managing data fetching operations over the network:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/mod.rs anchor=struct-StateFetcher}}

This struct itself is nested deeply within the `NetworkManager`: its `Swarm` struct (shown earlier in the chapter) contains a `NetworkState` struct that has the `StateFetcher` as a field:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/state.rs anchor=struct-NetworkState}}

### Usage of `FetchClient` in the Pipeline

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

TODO: Add other tasks (Transactions, ETH Requests) to structure of chapter
