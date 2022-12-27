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

Let's begin by taking a look at the line where the network is started, with the call, unsurprisingly, to `start_network`. Sounds important, doesn't it?

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=bin/reth/src/node/mod.rs anchor=fn-start_network}}

At a high level, this function is responsible for starting the tasks listed at the start of this chapter.

It gets the handles for the network management, transactions, and ETH requests tasks downstream of the `NetworkManager::builder` method call, and spawns them.

The `NetworkManager::builder` constructor requires a `NetworkConfig` struct to be passed in as a parameter, which can be used as the main entrypoint for setting up the entire network layer:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/config.rs anchor=struct-NetworkConfig}}

The discovery task progresses as the network management task is polled, handling events regarding peer management through the `Swarm` struct which is stored as a field on the `NetworkManager`:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/swarm.rs anchor=struct-Swarm}}

The `Swarm` struct glues together incoming connections from peers, managing sessions with peers, and recording the network's state (e.g. number of active peers, genesis hash of the network, etc.). It emits these as `SwarmEvent`s to the `NetworkManager`, and routes commands and events between the `SessionManager` and `NetworkState` structs that it holds.

We'll touch more on the `NetworkManager` shortly! It's perhaps the most important struct in this crate.

More information about the discovery task can be found in the [discv4](../discv4/README.md) chapter.

The ETH requests and transactions task will be explained in their own sections, following this one.

The variable `network` returned from `start_network` and the variable `fetch_client` returned from `network.fetch_client` are of types `NetworkHandle` and `FetchClient`, respectively. These are the two main interfaces for interacting with the P2P network, and are currently used in the `HeaderStage` and `BodyStage`.

Let's walk through how each is implemented, and then apply that knowledge to understand how they are used in the pipeline. In doing so, we'll dig deeper under the hood inside the network management task to get a sense of what's going on.

### Interacting with the Network Management Task Using `NetworkHandle`

The `NetworkHandle` struct is a client for the network management task that can be shared across threads. It wraps an `Arc` around the `NetworkInner` struct, defined as follows:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/network.rs anchor=struct-NetworkInner}}

The field of note here is `to_manager_tx`, which is a handle that can be used to send messages in a channel to an instance of the `NetworkManager` struct.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/manager.rs anchor=struct-NetworkManager}}

Now we're getting to the meat of the `network` crate! The `NetworkManager` struct represents the "Network Management" task described above. It is implemented as an endless [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) that can be thought of as a "hub" process which listens for messages from the `NetworkHandle` or the node's peers and dispatches messages to the other tasks, while keeping track of the state of the network.

While the `NetworkManager` is meant to be spawned as a standalone [`tokio::task`](https://docs.rs/tokio/0.2.4/tokio/task/index.html), the `NetworkHandle` can be passed around and shared, enabling access to the `NetworkManager` from anywhere by sending requests & commands through the appropriate channels.

#### Usage of `NetworkHandle` in the Pipeline

In the pipeline, the `NetworkHandle` is used to instantiate the `FetchClient` - which we'll get into next - and is used in the `HeaderStage` to update the node's ["status"](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#status-0x00) (record the the total difficulty, hash, and height of the last processed block).

[File: crates/stages/src/stages/headers.rs](https://github.com/paradigmxyz/reth/blob/main/crates/stages/src/stages/headers.rs)
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

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/fetch/client.rs anchor=struct-FetchClient}}

The `request_tx` field is a handle to a channel that can be used to send requests for downloading data, and the `peers_handle` field is a wrapper struct around a handle to a channel that can be used to send messages for applying manual changes to the peer set.

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

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/downloaders/src/headers/linear.rs anchor=fn-get_or_init_fut}}

In the `BodyStage` configured by the main binary, a `ConcurrentDownloader` is used:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/downloaders/src/bodies/concurrent.rs anchor=struct-ConcurrentDownloader}}

Here, similarly, a `FetchClient` is passed in to the `client` field, and the `get_block_bodies` method it implements is used when constructing the stream created by the `ConcurrentDownloader` in the `execute` method of the `BodyStage`.

[File: crates/net/downloaders/src/bodies/concurrent.rs](https://github.com/paradigmxyz/reth/blob/main/crates/net/downloaders/src/bodies/concurrent.rs)
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

---

## ETH Requests Task

The ETH requests task serves _incoming_ requests related to blocks in the [`eth` P2P subprotocol](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages) from other peers.

Similar to the network management task, it's implemented as an endless future, but it is meant to run as a background task (on a standalone `tokio::task`) and not to be interacted with directly from the pipeline. It's represented by the following `EthRequestHandler` struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=struct-EthRequestHandler}}

The `client` field here is a client that's used to fetch data from the database, not to be confused with the `client` field on a downloader like the `LinearDownloader` discussed above, which is a `FetchClient`.

### Input Streams to the ETH Requests Task

The `incoming_requests` field is the receiver end of a channel that accepts, as you might have guessed, incoming ETH requests from peers. The sender end of this channel is stored on the `NetworkManager` struct as the `to_eth_request_handler` field.

As the `NetworkManager` is polled and listens for events from peers passed through the `Swarm` struct it holds, it sends any received ETH requests into the channel.

### The Operation of the ETH Requests Task

Being an endless future, the core of the ETH requests task's functionality is in its `poll` method implementation. As the `EthRequestHandler` is polled, it listens for any ETH requests coming through the channel, and handles them accordingly. At the time of writing, the ETH requests task can handle the [`GetBlockHeaders`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03) and [`GetBlockBodies`](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockbodies-0x05) requests.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=fn-poll}}

The handling of these requests is fairly straightforward. The `GetBlockHeaders` payload is the following:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/eth-wire/src/types/blocks.rs anchor=struct-GetBlockHeaders}}

In handling this request, the ETH requests task attempts, starting with `start_block`, to fetch the associated header from the database, increment/decrement the block number to fetch by `skip` depending on the `direction` while checking for overflow/underflow, and checks that bounds specifying the maximum numbers of headers or bytes to send have not been breached.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=fn-get_headers_response}}

The `GetBlockBodies` payload is simpler, it just contains a vector of requested block hashes:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/eth-wire/src/types/blocks.rs anchor=struct-GetBlockBodies}}

In handling this request, similarly, the ETH requests task attempts, for each hash in the requested order, to fetch the block body (transactions & ommers), while checking that bounds specifying the maximum numbers of bodies or bytes to send have not been breached.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/eth_requests.rs anchor=fn-on_bodies_request}}

---

## Transactions Task

The transactions task listens for, requests, and propagates transactions both from the node's peers, and those that are added locally (e.g., submitted via RPC). Note that this task focuses solely on the network communication involved with Ethereum transactions, we will talk more about the structure of the transaction pool itself 
in the [transaction-pool](../../../ethereum/transaction-pool/README.md) chapter.

Again, like the network management and ETH requests tasks, the transactions task is implemented as an endless future that runs as a background task on a standalone `tokio::task`. It's represented by the `TransactionsManager` struct:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=struct-TransactionsManager}}

Unlike the ETH requests task, but like the network management task's `NetworkHandle`, the transactions task can also be accessed via a shareable "handle" struct, the `TransactionsHandle`:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=struct-TransactionsHandle}}

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

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-poll}}

Let's go through the handling occurring during each of these steps, in order, starting with the draining of the `TransactionsManager.network_events` stream.

#### Handling `NetworkEvent`s

The `TransactionsManager.network_events` stream is the first to have all of its events processed because it contains events concerning peer sessions opening and closing. This ensures, for example, that new peers are tracked in the `TransactionsManager` before events sent from them are processed.

The events received in this channel are of type `NetworkEvent`:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/manager.rs anchor=enum-NetworkEvent}}

They're handled with the `on_network_event` method, which responds to the two variants of the `NetworkEvent` enum in the following ways:

**`NetworkEvent::SessionClosed`**
Removes the peer given by `NetworkEvent::SessionClosed.peer_id` from the `TransactionsManager.peers` map.

**`NetworkEvent::SessionEstablished`**
Begins by inserting a `Peer` into `TransactionsManager.peers` by `peer_id`, which is a struct of the following form:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=struct-Peer}}

Note that the `Peer` struct contains a field `transactions`, which is an [LRU cache](https://en.wikipedia.org/wiki/Cache_replacement_policies#Least_recently_used_(LRU)) of the transactions this peer is aware of. 

The `request_tx` field on the `Peer` is used the sender end of a channel to send requests to the session with the peer.

After the `Peer` is added to `TransactionsManager.peers`, the hashes of all of the transactions in the node's transaction pool are sent to the peer in a [`NewPooledTransactionHashes` message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08).

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-on_network_event}}

#### Handling `TransactionsCommand`s

Next in the `poll` method, `TransactionsCommand`s sent through the `TransactionsManager.command_rx` stream are handled. These are the next to be handled as they are those sent manually via the `TransactionsHandle`, giving them precedence over transactions-related requests picked up from the network. The `TransactionsCommand` enum has the following form:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=enum-TransactionsCommand}}

`TransactionsCommand`s are handled by the `on_command` method. This method responds to the, at the time of writing, sole variant of the `TransactionsCommand` enum, `TransactionsCommand::PropagateHash`, with the `on_new_transactions` method, passing in an iterator consisting of the single hash contained by the variant (though this method can be called with many transaction hashes).

`on_new_transactions` propagates the full transaction object, with the signer attached, to a small random sample of peers using the `propagate_transactions` method. Then, it notifies all other peers of the hash of the new transaction, so that they can request the full transaction object if they don't already have it.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-on_new_transactions-propagate_transactions}}

#### Handling `NetworkTransactionEvent`s

After `TransactionsCommand`s, it's time to take care of transactions-related requests sent by peers in the network, so the `poll` method handles `NetworkTransactionEvent`s received through the `TransactionsManager.transaction_events` stream. `NetworkTransactionEvent` has the following form:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=enum-NetworkTransactionEvent}}

These events are handled with the `on_network_tx_event` method, which responds to the variants of the `NetworkTransactionEvent` enum in the following ways:

**`NetworkTransactionEvent::IncomingTransactions`**

This event is generated from the [`Transactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#transactions-0x02), and is handled by the `import_transactions` method.

Here, for each transaction in the variant's `msg` field, we attempt to recover the signer, insert the transaction into LRU cache of the `Peer` identified by the variant's `peer_id` field, and add the `peer_id` to the vector of peer IDs keyed by the transaction's hash in `TransactionsManager.transactions_by_peers`. If an entry does not already exist for the transaction hash, then it begins importing the transaction object into the node's transaction pool, adding a `PoolImportFuture` to `TransactionsManager.pool_imports`. If there was an issue recovering the signer, `report_bad_message` is called for the `peer_id`, which decreases the peer's reputation.

To understand this a bit better, let's double back and examine what `TransactionsManager.transactions_by_peers` and `TransactionsManager.pool_imports` are used for.

`TransactionsManager.transactions_by_peers` is a `HashMap<TxHash, Vec<PeerId>>`, tracks which peers have sent us a transaction with the given hash. This has two uses: the first being that it prevents us from redundantly importing transactions into the transaction pool for which we've already begun this process (this check occurs in `import_transactions`), and the second being that if a transaction we receive is malformed in some way and ends up erroring when imported to the transaction pool, we can reduce the reputation score for all of the peers that sent us this transaction (this occurs in `on_bad_import`, which we'll touch on soon).

`TransactionsManager.pool_imports` is a set of futures representing the transactions which are currently in the process of being imported to the node's transaction pool. This process is asynchronous due to the validation of the transaction that must occur, thus we need to keep a handle on the generated future.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-import_transactions}}

**`NetworkTransactionEvent::IncomingPooledTransactionHashes`**

This event is generated from the [`NewPooledTransactionHashes` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#newpooledtransactionhashes-0x08), and is handled by the `on_new_pooled_transactions` method.

Here, it begins by adding the transaction hashes included in the `NewPooledTransactionHashes` payload to the LRU cache for the `Peer` identified by `peer_id` in `TransactionsManager.peers`. Next, it filters the list of hashes to those that are not already present in the transaction pool, and for each such hash, requests its full transaction object from the peer by sending it a [`GetPooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09) through the `Peer.request_tx` channel. If the request was successfully sent, a `GetPooledTxRequest` gets added to `TransactionsManager.inflight_requests` vector:

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=struct-GetPooledTxRequest}}

As you can see, this struct also contains a `response` channel from which the peer's response can later be polled.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-on_new_pooled_transactions}}

**`NetworkTransactionEvent::GetPooledTransactions`**

This event is generated from the [`GetPooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getpooledtransactions-0x09), and is handled by the `on_get_pooled_transactions` method.

Here, it collects _all_ the transactions in the node's transaction pool, recovers their signers, adds their hashes to the LRU cache of the requesting peer, and sends them to the peer in a [`PooledTransactions` protocol message](https://github.com/ethereum/devp2p/blob/master/caps/eth.md#pooledtransactions-0x0a). This is sent through the `response` channel that's stored as a field of the `NetworkTransaction::GetPooledTransactions` variant itself.

{{#template ../../../templates/source_and_github.md path_to_root=../../../../ path=crates/net/network/src/transactions.rs anchor=fn-on_get_pooled_transactions}}

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
