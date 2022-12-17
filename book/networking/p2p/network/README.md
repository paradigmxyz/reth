# Network

TODO: Some high-level overview of peer-to-peer networking, either here or in a short chapter one level up in the tree for `p2p`

The `network` crate is responsible for managing the node's connection to the Ethereum peer-to-peer (P2P) network, enabling communication with other nodes via the [various P2P subprotocols](https://github.com/ethereum/devp2p).

TODO: Link to the "bird's eye docs" in `crates/net/network/src/lib.rs`?

Reth's P2P networking consists primarily of 4 ongoing tasks:
- **Discovery**: Discovers new peers in the network
- **Transactions**: Accepts, requests, and broadcasts mempool transactions
- **ETH Requests**: Responds to incoming requests for headers and bodies
    - TODO: Doesn't also issue requests for these?
    - TODO: Are headers and bodies all that it's concerned with?
- **Network Management**: Handles incoming & outgoing connections with peers, and routes requests between peers and the other tasks

TODO: Add link to docs.rs page with mermaid graph in `network.rs`

We'll leave most of the discussion of the discovery task for the [discv4](../discv4/README.md) chapter, and will focus on the other three here.

Let's take a look at how the main Reth CLI (i.e., a default-configured full node) makes use of the P2P layer to explore the primary interfaces and entrypoints into the `network` crate.

## Use of the network in the node

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

## `NetworkHandle` and The Network Management Task

The `NetworkHandle` struct is a client for the entire suite of P2P network functionality (TODO: check this definition) that can be shared across threads. It wraps an `Arc` around the `NetworkInner` struct, defined as follows:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/network.rs anchor=struct-NetworkInner}}

The field of note here is `to_manager_tx`, which is a handle that can be used to send messages in a channel to an instance of the `NetworkManager` struct.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/manager.rs anchor=struct-NetworkManager}}

Now we're getting to the meat of the `network` crate! The `NetworkManager` struct represents the "Network Management" task described above. It is implemented as an endless [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) that can be thought of as a "hub" process which listens for messages from the `NetworkHandle` or the node's peers (TODO: Mention `Swarm` struct?) and dispatches messages tp the other tasks, while keeping track of the state of the network.

TODO: Go into more detail about the `poll` method here?
TODO: Talk about instantiating the `NetworkManager` / `NetworkHandle` & what goes on under the hood? (E.g. `SessionManager`, `Swarm`, etc.)

In the pipeline, the `NetworkHandle` is used to instantiate the `FetchClient` - which we'll get into next - and is used in the `HeaderStage` to update the node's ["status"](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#status-0x00) (record the the total difficulty, hash, and height of the last processed block).

## `FetchClient` (TODO: Better title)

The `FetchClient` struct, similar to `NetworkHandle`, can be shared across threads, and is a client for fetching data from the network. It's a fairly lightweight struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/client.rs anchor=struct-FetchClient}}

TODO: Talk about how these fields get populated / how this struct gets instantiated (touches `NetworkManager` and `StateFetcher`) when talking about usage in pipeline

---

TODO: Add other tasks (Transactions, ETH Requests) to structure of chapter
