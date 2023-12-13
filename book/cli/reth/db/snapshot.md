# reth db snapshot

Snapshots tables from database

```bash
$ reth db snapshot --help
Usage: reth db snapshot [OPTIONS] [SEGMENTS]...

Arguments:
  [SEGMENTS]...
          Snapshot segments to generate

          Possible values:
          - headers:      Snapshot segment responsible for the `CanonicalHeaders`, `Headers`, `HeaderTD` tables
          - transactions: Snapshot segment responsible for the `Transactions` table
          - receipts:     Snapshot segment responsible for the `Receipts` table

Options:
      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.
          
          Defaults to the OS-specific data directory:
          
          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - Windows: `{FOLDERID_RoamingAppData}/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`
          
          [default: default]

  -f, --from <FROM>
          Starting block for the snapshot
          
          [default: 0]

  -b, --block-interval <BLOCK_INTERVAL>
          Number of blocks in the snapshot
          
          [default: 500000]

      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          Possible values are either a built-in chain or the path to a chain specification file.
          
          Built-in chains:
              mainnet, sepolia, goerli, holesky, dev
          
          [default: mainnet]

  -p, --parallel <PARALLEL>
          Sets the number of snapshots built in parallel. Note: Each parallel build is memory-intensive
          
          [default: 1]

      --only-stats
          Flag to skip snapshot creation and print snapshot files stats

      --bench
          Flag to enable database-to-snapshot benchmarking

      --only-bench
          Flag to skip snapshot creation and only run benchmarks on existing snapshots

  -c, --compression <COMPRESSION>
          Compression algorithms to use
          
          [default: lz4]
          [possible values: lz4, zstd, zstd-with-dictionary, uncompressed]

      --with-filters
          Flag to enable inclusion list filters and PHFs

      --phf <PHF>
          Specifies the perfect hashing function to use

          Possible values:
          - fmph:    Fingerprint-Based Minimal Perfect Hash Function
          - go-fmph: Fingerprint-Based Minimal Perfect Hash Function with Group Optimization

      --instance <INSTANCE>
          Add a new instance of a node.
          
          Configures the ports of the node to avoid conflicts with the defaults. This is useful for running multiple nodes on the same machine.
          
          Max number of instances is 200. It is chosen in a way so that it's not possible to have port numbers that conflict with each other.
          
          Changes to the following port numbers: - DISCOVERY_PORT: default + `instance` - 1 - AUTH_PORT: default + `instance` * 100 - 100 - HTTP_RPC_PORT: default - `instance` + 1 - WS_RPC_PORT: default + `instance` * 2 - 2
          
          [default: 1]

  -h, --help
          Print help (see a summary with '-h')

Logging:
      --log.file.directory <PATH>
          The path to put log files in
          
          [default: <CACHE_DIR>/logs]

      --log.file.max-size <SIZE>
          The maximum size (in MB) of one log file
          
          [default: 200]

      --log.file.max-files <COUNT>
          The maximum amount of log files that will be stored. If set to 0, background file logging is disabled
          
          [default: 5]

      --log.file.filter <FILTER>
          The filter to use for logs written to the log file
          
          [default: debug]

      --log.journald
          Write logs to journald

      --log.journald.filter <FILTER>
          The filter to use for logs written to journald
          
          [default: error]

      --color <COLOR>
          Sets whether or not the formatter emits ANSI terminal escape codes for colors and other text formatting
          
          [default: always]

          Possible values:
          - always: Colors on
          - auto:   Colors on
          - never:  Colors off

Display:
  -v, --verbosity...
          Set the minimum log level.
          
          -v      Errors
          -vv     Warnings
          -vvv    Info
          -vvvv   Debug
          -vvvvv  Traces (warning: very verbose!)

  -q, --quiet
          Silence all log output
```