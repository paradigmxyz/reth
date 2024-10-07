# reth debug merkle

Debug the clean & incremental state root calculations

```bash
$ reth debug merkle --help
```
```txt
Usage: reth debug merkle [OPTIONS] --to <TO>

Options:
      --instance <INSTANCE>
          Add a new instance of a node.

          Configures the ports of the node to avoid conflicts with the defaults. This is useful for running multiple nodes on the same machine.

          Max number of instances is 200. It is chosen in a way so that it's not possible to have port numbers that conflict with each other.

          Changes to the following port numbers: - `DISCOVERY_PORT`: default + `instance` - 1 - `AUTH_PORT`: default + `instance` * 100 - 100 - `HTTP_RPC_PORT`: default - `instance` + 1 - `WS_RPC_PORT`: default + `instance` * 2 - 2

          [default: 1]

  -h, --help
          Print help (see a summary with '-h')

Datadir:
      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.

          Defaults to the OS-specific data directory:

          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - Windows: `{FOLDERID_RoamingAppData}/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`

          [default: default]

      --datadir.static-files <PATH>
          The absolute path to store static files in.

      --config <FILE>
          The path to the configuration file to use

      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          Possible values are either a built-in chain or the path to a chain specification file.

          Built-in chains:
              mainnet, sepolia, holesky, dev

          [default: mainnet]

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

      --db.exclusive <EXCLUSIVE>
          Open environment in exclusive/monopolistic mode. Makes it possible to open a database on an NFS volume

          [possible values: true, false]

Networking:
  -d, --disable-discovery
          Disable the discovery service

      --disable-dns-discovery
          Disable the DNS discovery

      --disable-discv4-discovery
          Disable Discv4 discovery

      --enable-discv5-discovery
          Enable Discv5 discovery

      --disable-nat
          Disable Nat discovery

      --discovery.addr <DISCOVERY_ADDR>
          The UDP address to use for devp2p peer discovery version 4

          [default: 0.0.0.0]

      --discovery.port <DISCOVERY_PORT>
          The UDP port to use for devp2p peer discovery version 4

          [default: 30303]

      --discovery.v5.addr <DISCOVERY_V5_ADDR>
          The UDP IPv4 address to use for devp2p peer discovery version 5. Overwritten by `RLPx` address, if it's also IPv4

      --discovery.v5.addr.ipv6 <DISCOVERY_V5_ADDR_IPV6>
          The UDP IPv6 address to use for devp2p peer discovery version 5. Overwritten by `RLPx` address, if it's also IPv6

      --discovery.v5.port <DISCOVERY_V5_PORT>
          The UDP IPv4 port to use for devp2p peer discovery version 5. Not used unless `--addr` is IPv4, or `--discovery.v5.addr` is set

          [default: 9200]

      --discovery.v5.port.ipv6 <DISCOVERY_V5_PORT_IPV6>
          The UDP IPv6 port to use for devp2p peer discovery version 5. Not used unless `--addr` is IPv6, or `--discovery.addr.ipv6` is set

          [default: 9200]

      --discovery.v5.lookup-interval <DISCOVERY_V5_LOOKUP_INTERVAL>
          The interval in seconds at which to carry out periodic lookup queries, for the whole run of the program

          [default: 60]

      --discovery.v5.bootstrap.lookup-interval <DISCOVERY_V5_BOOTSTRAP_LOOKUP_INTERVAL>
          The interval in seconds at which to carry out boost lookup queries, for a fixed number of times, at bootstrap

          [default: 5]

      --discovery.v5.bootstrap.lookup-countdown <DISCOVERY_V5_BOOTSTRAP_LOOKUP_COUNTDOWN>
          The number of times to carry out boost lookup queries at bootstrap

          [default: 100]

      --trusted-peers <TRUSTED_PEERS>
          Comma separated enode URLs of trusted peers for P2P connections.

          --trusted-peers enode://abcd@192.168.0.1:30303

      --trusted-only
          Connect to or accept from trusted peers only

      --bootnodes <BOOTNODES>
          Comma separated enode URLs for P2P discovery bootstrap.

          Will fall back to a network-specific default if not specified.

      --dns-retries <DNS_RETRIES>
          Amount of DNS resolution requests retries to perform when peering

          [default: 0]

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

      --max-tx-reqs <COUNT>
          Max concurrent `GetPooledTransactions` requests.

          [default: 130]

      --max-tx-reqs-peer <COUNT>
          Max concurrent `GetPooledTransactions` requests per peer.

          [default: 1]

      --max-seen-tx-history <COUNT>
          Max number of seen transactions to remember per peer.

          Default is 320 transaction hashes.

          [default: 320]

      --max-pending-imports <COUNT>
          Max number of transactions to import concurrently.

          [default: 4096]

      --pooled-tx-response-soft-limit <BYTES>
          Experimental, for usage in research. Sets the max accumulated byte size of transactions
          to pack in one response.
          Spec'd at 2MiB.

          [default: 2097152]

      --pooled-tx-pack-soft-limit <BYTES>
          Experimental, for usage in research. Sets the max accumulated byte size of transactions to
          request in one request.

          Since `RLPx` protocol version 68, the byte size of a transaction is shared as metadata in a
          transaction announcement (see `RLPx` specs). This allows a node to request a specific size
          response.

          By default, nodes request only 128 KiB worth of transactions, but should a peer request
          more, up to 2 MiB, a node will answer with more than 128 KiB.

          Default is 128 KiB.

          [default: 131072]

      --max-tx-pending-fetch <COUNT>
          Max capacity of cache of hashes for transactions pending fetch.

          [default: 25600]

      --net-if.experimental <IF_NAME>
          Name of network interface used to communicate with peers.

          If flag is set, but no value is passed, the default interface for docker `eth0` is tried.

      --retries <RETRIES>
          The number of retries per request

          [default: 5]

      --to <TO>
          The height to finish at

      --skip-node-depth <SKIP_NODE_DEPTH>
          The depth after which we should start comparing branch nodes

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