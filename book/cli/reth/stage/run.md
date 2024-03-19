# reth stage run

Run a single stage.

```bash
$ reth stage run --help
Usage: reth stage run [OPTIONS] --from <FROM> --to <TO> <STAGE>

Arguments:
  <STAGE>
          The name of the stage to run

          Possible values:
          - headers:         The headers stage within the pipeline
          - bodies:          The bodies stage within the pipeline
          - senders:         The senders stage within the pipeline
          - execution:       The execution stage within the pipeline
          - account-hashing: The account hashing stage within the pipeline
          - storage-hashing: The storage hashing stage within the pipeline
          - hashing:         The hashing stage within the pipeline
          - merkle:          The Merkle stage within the pipeline
          - tx-lookup:       The transaction lookup stage within the pipeline
          - account-history: The account history stage within the pipeline
          - storage-history: The storage history stage within the pipeline

Options:
      --config <FILE>
          The path to the configuration file to use.

      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.
          
          Defaults to the OS-specific data directory:
          
          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - Windows: `{FOLDERID_RoamingAppData}/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`
          
          [default: default]

      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          Possible values are either a built-in chain or the path to a chain specification file.
          
          Built-in chains:
              mainnet, sepolia, goerli, holesky, dev
          
          [default: mainnet]

      --metrics <SOCKET>
          Enable Prometheus metrics.
          
          The metrics will be served at the given interface and port.

      --from <FROM>
          The height to start at

  -t, --to <TO>
          The end of the stage

      --batch-size <BATCH_SIZE>
          Batch size for stage execution and unwind

      --etl-file-size <ETL_FILE_SIZE>
          The maximum size in bytes of data held in memory before being flushed to disk as a file

      --etl-dir <ETL_DIR>
          Directory where to collect ETL files

  -s, --skip-unwind
          Normally, running the stage requires unwinding for stages that already have been run, in order to not rewrite to the same database slots.
          
          You can optionally skip the unwinding phase if you're syncing a block range that has not been synced before.

      --instance <INSTANCE>
          Add a new instance of a node.
          
          Configures the ports of the node to avoid conflicts with the defaults. This is useful for running multiple nodes on the same machine.
          
          Max number of instances is 200. It is chosen in a way so that it's not possible to have port numbers that conflict with each other.
          
          Changes to the following port numbers: - DISCOVERY_PORT: default + `instance` - 1 - AUTH_PORT: default + `instance` * 100 - 100 - HTTP_RPC_PORT: default - `instance` + 1 - WS_RPC_PORT: default + `instance` * 2 - 2
          
          [default: 1]

  -h, --help
          Print help (see a summary with '-h')

Networking:
  -d, --disable-discovery
          Disable the discovery service

      --disable-dns-discovery
          Disable the DNS discovery

      --disable-discv4-discovery
          Disable Discv4 discovery

      --discovery.addr <DISCOVERY_ADDR>
          The UDP address to use for P2P discovery/networking
          
          [default: 0.0.0.0]

      --discovery.port <DISCOVERY_PORT>
          The UDP port to use for P2P discovery/networking
          
          [default: 30303]

      --trusted-peers <TRUSTED_PEERS>
          Comma separated enode URLs of trusted peers for P2P connections.
          
          --trusted-peers enode://abcd@192.168.0.1:30303

      --trusted-only
          Connect only to trusted peers

      --bootnodes <BOOTNODES>
          Comma separated enode URLs for P2P discovery bootstrap.
          
          Will fall back to a network-specific default if not specified.

      --peers-file <FILE>
          The path to the known peers file. Connected peers are dumped to this file on nodes
          shutdown, and read on startup. Cannot be used with `--no-persist-peers`.

      --identity <IDENTITY>
          Custom node identity
          
          [default: reth/<VERSION>-<SHA>/<ARCH>]

      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This will also deterministically set the peer ID. If not specified, it will be set in the data dir for the chain being used.

      --no-persist-peers
          Do not persist peers.

      --nat <NAT>
          NAT resolution method (any|none|upnp|publicip|extip:\<IP\>)
          
          [default: any]

      --addr <ADDR>
          Network listening address
          
          [default: 0.0.0.0]

      --port <PORT>
          Network listening port
          
          [default: 30303]

      --max-outbound-peers <MAX_OUTBOUND_PEERS>
          Maximum number of outbound requests. default: 100

      --max-inbound-peers <MAX_INBOUND_PEERS>
          Maximum number of inbound requests. default: 30

      --pooled-tx-response-soft-limit <BYTES>
          Soft limit for the byte size of a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on assembling a [`GetPooledTransactions`](reth_eth_wire::GetPooledTransactions) request. Spec'd at 2 MiB.
          
          <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#protocol-messages>.
          
          [default: 2097152]

      --pooled-tx-pack-soft-limit <BYTES>
          Default soft limit for the byte size of a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response on assembling a [`GetPooledTransactions`](reth_eth_wire::PooledTransactions) request. This defaults to less than the [`SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE`], at 2 MiB, used when assembling a [`PooledTransactions`](reth_eth_wire::PooledTransactions) response. Default is 128 KiB
          
          [default: 131072]

Database:
      --db.log-level <LOG_LEVEL>
          Database logging level. Levels higher than "notice" require a debug build

          Possible values:
          - fatal:   Enables logging for critical conditions, i.e. assertion failures
          - error:   Enables logging for error conditions
          - warn:    Enables logging for warning conditions
          - notice:  Enables logging for normal but significant condition
          - verbose: Enables logging for verbose informational
          - debug:   Enables logging for debug-level messages
          - trace:   Enables logging for trace debug-level messages
          - extra:   Enables logging for extra debug-level messages

  -c, --commit
          Commits the changes in the database. WARNING: potentially destructive.
          
          Useful when you want to run diagnostics on the database.

      --checkpoints
          Save stage checkpoints

Logging:
      --log.stdout.format <FORMAT>
          The format to use for logs written to stdout
          
          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.stdout.filter <FILTER>
          The filter to use for logs written to stdout
          
          [default: ]

      --log.file.format <FORMAT>
          The format to use for logs written to the log file
          
          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.file.filter <FILTER>
          The filter to use for logs written to the log file
          
          [default: debug]

      --log.file.directory <PATH>
          The path to put log files in
          
          [default: <CACHE_DIR>/logs]

      --log.file.max-size <SIZE>
          The maximum size (in MB) of one log file
          
          [default: 200]

      --log.file.max-files <COUNT>
          The maximum amount of log files that will be stored. If set to 0, background file logging is disabled
          
          [default: 5]

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