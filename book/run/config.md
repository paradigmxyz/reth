# Configuring Reth

Reth places a configuration file named `reth.toml` in the data directory specified when starting the node. It is written in the [TOML] format.

The default data directory is platform dependent and can be found by running `reth node --help`.

The configuration file contains the following sections:

- [`[stages]`](#the-stages-section) -- Configuration of the individual sync stages
    - [`headers`](#headers)
    - [`total_difficulty`](#total_difficulty)
    - [`bodies`](#bodies)
    - [`sender_recovery`](#sender_recovery)
    - [`execution`](#execution)
- [`[peers]`](#the-peers-section)
- [`[sessions]`](#the-sessions-section)

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
[stages.headers]
# The maximum number of bodies to request from a peer at a time.
downloader_request_limit = 200
# The maximum amount of bodies to download before writing them to disk.
#
# A lower value means more frequent disk I/O (writes), but also
# lowers memory usage.
downloader_stream_batch_size = 10000
# The maximum amount of blocks to keep in the internal buffer of the downloader.
#
# A bigger buffer means that bandwidth can be saturated for longer periods,
# but also increases memory consumption.
#
# If the buffer is full, no more requests will be made to peers until
# space is made for new blocks in the buffer.
downloader_max_buffered_blocks = 42949
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
- If only `max_changes` is specified, reth will execute as many blocks as possible until the target amount of state transitions have occured before writing to disk.
- If both are specified, then the first threshold to be hit will determine when the results are written to disk.

Lower values correspond to more frequent disk writes, but also lower memory consumption. A lower value also negatively impacts sync speed, since reth keeps a cache around for the entire duration of blocks executed in the same range.

## The `[peers]` section

## The `[sessions]` section

[TOML]: https://toml.io/