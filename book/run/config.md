# Configuring Reth

Reth places a configuration file named `reth.toml` in the data directory specified when starting the node. It is written in the [TOML] format.

The default data directory is platform dependent:

- Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
- Windows: `{FOLDERID_RoamingAppData}/reth/`
- macOS: `$HOME/Library/Application Support/reth/`

The configuration file contains the following sections:

- [`[stages]`](#the-stages-section) -- Configuration of the individual sync stages
  - [`headers`](#headers)
  - [`total_difficulty`](#total_difficulty)
  - [`bodies`](#bodies)
  - [`sender_recovery`](#sender_recovery)
  - [`execution`](#execution)
  - [`account_hashing`](#account_hashing)
  - [`storage_hashing`](#storage_hashing)
  - [`merkle`](#merkle)
  - [`transaction_lookup`](#transaction_lookup)
  - [`index_account_history`](#index_account_history)
  - [`index_storage_history`](#index_storage_history)
- [`[peers]`](#the-peers-section)
  - [`connection_info`](#connection_info)
  - [`reputation_weights`](#reputation_weights)
  - [`backoff_durations`](#backoff_durations)
- [`[sessions]`](#the-sessions-section)
- [`[prune]`](#the-prune-section)

## The `[stages]` section

The stages section is used to configure how individual stages in reth behave, which has a direct impact on resource utilization and sync speed.

The defaults shipped with Reth try to be relatively reasonable, but may not be optimal for your specific set of hardware.

### `headers`

The headers section controls both the behavior of the header stage, which download historical headers, as well as the primary downloader that fetches headers over P2P.

```toml
[stages.headers]
# The minimum and maximum number of concurrent requests to have in flight at a time.
#
# The downloader uses these as best effort targets, which means that the number
# of requests may be outside of these thresholds within a reasonable degree.
#
# Increase these for faster sync speeds at the cost of additional bandwidth and memory
downloader_max_concurrent_requests = 100
downloader_min_concurrent_requests = 5
# The maximum number of responses to buffer in the downloader at any one time.
#
# If the buffer is full, no more requests will be sent until room opens up.
#
# Increase the value for a larger buffer at the cost of additional memory consumption
downloader_max_buffered_responses = 100
# The maximum number of headers to request from a peer at a time.
downloader_request_limit = 1000
# The amount of headers to persist to disk at a time.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 10000
```

### `total_difficulty`

The total difficulty stage calculates the total difficulty reached for each header in the chain.

```toml
[stages.total_difficulty]
# The amount of headers to calculate the total difficulty for
# before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

### `bodies`

The bodies section controls both the behavior of the bodies stage, which download historical block bodies, as well as the primary downloader that fetches block bodies over P2P.

```toml
[stages.bodies]
# The maximum number of bodies to request from a peer at a time.
downloader_request_limit = 200
# The maximum amount of bodies to download before writing them to disk.
#
# A lower value means more frequent disk I/O (writes), but also
# lowers memory usage.
downloader_stream_batch_size = 1000
# The size of the internal block buffer in bytes.
#
# A bigger buffer means that bandwidth can be saturated for longer periods,
# but also increases memory consumption.
#
# If the buffer is full, no more requests will be made to peers until
# space is made for new blocks in the buffer.
#
# Defaults to around 2GB.
downloader_max_buffered_blocks_size_bytes = 2147483648
# The minimum and maximum number of concurrent requests to have in flight at a time.
#
# The downloader uses these as best effort targets, which means that the number
# of requests may be outside of these thresholds within a reasonable degree.
#
# Increase these for faster sync speeds at the cost of additional bandwidth and memory
downloader_min_concurrent_requests = 5
downloader_max_concurrent_requests = 100
```

### `sender_recovery`

The sender recovery stage recovers the address of transaction senders using transaction signatures.

```toml
[stages.sender_recovery]
# The amount of transactions to recover senders for before
# writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

### `execution`

The execution stage executes historical transactions. This stage is generally very I/O and memory intensive, since executing transactions involves reading block headers, transactions, accounts and account storage.

Each executed transaction also generates a number of changesets, and mutates the current state of accounts and storage.

For this reason, there are two ways to control how much work to perform before the results are written to disk.

```toml
[stages.execution]
# The maximum amount of blocks to execute before writing the results to disk.
max_blocks = 500000
# The maximum amount of account and storage changes to collect before writing
# the results to disk.
max_changes = 5000000
```

Either one of `max_blocks` or `max_changes` must be specified, and both can also be specified at the same time:

- If only `max_blocks` is specified, reth will execute (up to) that amount of blocks before writing to disk.
- If only `max_changes` is specified, reth will execute as many blocks as possible until the target amount of state transitions have occurred before writing to disk.
- If both are specified, then the first threshold to be hit will determine when the results are written to disk.

Lower values correspond to more frequent disk writes, but also lower memory consumption. A lower value also negatively impacts sync speed, since reth keeps a cache around for the entire duration of blocks executed in the same range.

### `account_hashing`

The account hashing stage builds a secondary table of accounts, where the key is the hash of the address instead of the raw address.

This is used to later compute the state root.

```toml
[stages.account_hashing]
# The threshold in number of blocks before the stage starts from scratch
# and re-hashes all accounts as opposed to just the accounts that changed.
clean_threshold = 500000
# The amount of accounts to process before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

### `storage_hashing`

The storage hashing stage builds a secondary table of account storages, where the key is the hash of the address and the slot, instead of the raw address and slot.

This is used to later compute the state root.

```toml
[stages.storage_hashing]
# The threshold in number of blocks before the stage starts from scratch
# and re-hashes all storages as opposed to just the storages that changed.
clean_threshold = 500000
# The amount of storage slots to process before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

### `merkle`

The merkle stage uses the indexes built in the hashing stages (storage and account hashing) to compute the state root of the latest block.

```toml
[stages.merkle]
# The threshold in number of blocks before the stage starts from scratch
# and re-computes the state root, discarding the trie that has already been built,
# as opposed to incrementally updating the trie.
clean_threshold = 50000
```

### `transaction_lookup`

The transaction lookup stage builds an index of transaction hashes to their sequential transaction ID.

```toml
[stages.transaction_lookup]
# The maximum number of transactions to process before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 5000000
```

### `index_account_history`

The account history indexing stage builds an index of what blocks a particular account changed.

```toml
[stages.index_account_history]
# The maximum amount of blocks to process before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

### `index_storage_history`

The storage history indexing stage builds an index of what blocks a particular storage slot changed.

```toml
[stages.index_storage_history]
# The maximum amount of blocks to process before writing the results to disk.
#
# Lower thresholds correspond to more frequent disk I/O (writes),
# but lowers memory usage
commit_threshold = 100000
```

## The `[peers]` section

The peers section is used to configure how the networking component of reth establishes and maintains connections to peers.

In the top level of the section you can configure trusted nodes, and how often reth will try to connect to new peers.

```toml
[peers]
# How often reth will attempt to make outgoing connections,
# if there is room for more peers
refill_slots_interval = '1s'
# A list of ENRs for trusted peers, which are peers reth will always try to connect to.
trusted_nodes = []
# Whether reth will only attempt to connect to the peers specified above,
# or if it will connect to other peers in the network
connect_trusted_nodes_only = false
# The duration for which a badly behaving peer is banned
ban_duration = '12h'
```

### `connection_info`

This section configures how many peers reth will connect to.

```toml
[peers.connection_info]
# The maximum number of outbound peers (peers we connect to)
max_outbound = 100
# The maximum number of inbound peers (peers that connect to us)
max_inbound = 30
```

### `reputation_weights`

This section configures the penalty for various offences peers can commit.

All peers start out with a reputation of 0, which increases over time as the peer stays connected to us.

If the peer misbehaves, various penalties are exacted to their reputation, and if it falls below a certain threshold (currently `50 * -1024`), reth will disconnect and ban the peer temporarily (except for protocol violations which constitute a permanent ban).

```toml
[peers.reputation_weights]
bad_message = -16384
bad_block = -16384
bad_transactions = -16384
already_seen_transactions = 0
timeout = -4096
bad_protocol = -2147483648
failed_to_connect = -25600
dropped = -4096
```

### `backoff_durations`

If reth fails to establish a connection to a peer, it will not re-attempt for some amount of time, depending on the reason the connection failed.

```toml
[peers.backoff_durations]
low = '30s'
medium = '3m'
high = '15m'
max = '1h'
```

## The `[sessions]` section

The sessions section configures the internal behavior of a single peer-to-peer connection.

You can configure the session buffer sizes, which limits the amount of pending events (incoming messages) and commands (outgoing messages) each session can hold before it will start to ignore messages.

> **Note**
> 
> These buffers are allocated *per peer*, which means that increasing the buffer sizes can have large impact on memory consumption.

```toml
[sessions]
session_command_buffer = 32
session_event_buffer = 260
```

You can also configure request timeouts:

```toml
[sessions.initial_internal_request_timeout]
secs = 20
nanos = 0

# The amount of time before the peer will be penalized for
# being in violation of the protocol. This exacts a permaban on the peer.
[sessions.protocol_breach_request_timeout]
secs = 120
nanos = 0
```

## The `[prune]` section

The prune section configures the pruning configuration.

You can configure the pruning of different parts of the data independently of others.
For any unspecified parts, the default setting is no pruning.

### Default config

No pruning, run as archive node.

### Example of the custom pruning configuration

This configuration will:
- Run pruning every 5 blocks
- Continuously prune all transaction senders, account history and storage history before the block `head-128`, i.e. keep the data for the last 129 blocks
- Prune all receipts before the block 1920000, i.e. keep receipts from the block 1920000

```toml
[prune]
# Minimum pruning interval measured in blocks
block_interval = 5

[prune.parts]
# Sender Recovery pruning configuration
sender_recovery = { distance = 128 } # Prune all transaction senders before the block `head-128`, i.e. keep transaction senders for the last 129 blocks

# Transaction Lookup pruning configuration
transaction_lookup = "full" # Prune all TxNumber => TxHash mappings

# Receipts pruning configuration. This setting overrides `receipts_log_filter`.
receipts = { before = 1920000 } # Prune all receipts from transactions before the block 1920000, i.e. keep receipts from the block 1920000

# Account History pruning configuration
account_history = { distance = 128 } # Prune all historical account states before the block `head-128`

# Storage History pruning configuration
storage_history = { distance = 128 } # Prune all historical storage states before the block `head-128`
```

We can also prune receipts more granular, using the logs filtering:
```toml
# Receipts pruning configuration by retaining only those receipts that contain logs emitted
# by the specified addresses, discarding all others. This setting is overridden by `receipts`.
[prune.parts.receipts_log_filter]
# Prune all receipts, leaving only those which:
# - Contain logs from address `0x7ea2be2df7ba6e54b1a9c70676f668455e329d29`, starting from the block 17000000
# - Contain logs from address `0xdac17f958d2ee523a2206206994597c13d831ec7` in the last 1001 blocks
"0x7ea2be2df7ba6e54b1a9c70676f668455e329d29" = { before = 17000000 }
"0xdac17f958d2ee523a2206206994597c13d831ec7" = { distance = 1000 }
```

[TOML]: https://toml.io/
