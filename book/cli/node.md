# `reth node`

Start the node

```bash
$ reth node --help

Usage: reth node [OPTIONS]

Options:
      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.
          
          Defaults to the OS-specific data directory:
          
          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - Windows: `{FOLDERID_RoamingAppData}/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`
          
          [default: default]

      --config <FILE>
          The path to the configuration file to use.

      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          
          Possible values are either a built-in chain or the path to a chain specification file.
          
          Built-in chains:
          - mainnet
          - goerli
          - sepolia
          
          [default: mainnet]

  -h, --help
          Print help (see a summary with '-h')

Metrics:
      --metrics <SOCKET>
          Enable Prometheus metrics.
          
          The metrics will be served at the given interface and port.

Networking:
  -d, --disable-discovery
          Disable the discovery service

      --disable-dns-discovery
          Disable the DNS discovery

      --disable-discv4-discovery
          Disable Discv4 discovery

      --discovery.port <DISCOVERY_PORT>
          The UDP port to use for P2P discovery/networking. default: 30303

      --trusted-peers <TRUSTED_PEERS>
          Target trusted peer enodes --trusted-peers enode://abcd@192.168.0.1:30303

      --trusted-only
          Connect only to trusted peers

      --bootnodes <BOOTNODES>
          Bootnodes to connect to initially.
          
          Will fall back to a network-specific default if not specified.

      --peers-file <FILE>
          The path to the known peers file. Connected peers are dumped to this file on nodes
          shutdown, and read on startup. Cannot be used with `--no-persist-peers`.

      --identity <IDENTITY>
          Custom node identity
          
          [default: reth/v0.1.0-alpha.1/aarch64-apple-darwin]

      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This will also deterministically set the peer ID. If not specified, it will be set in the data dir for the chain being used.

      --no-persist-peers
          Do not persist peers.

      --nat <NAT>
          NAT resolution method (any|none|upnp|publicip|extip:<IP>)
          
          [default: any]

      --port <PORT>
          Network listening port. default: 30303

RPC:
      --http
          Enable the HTTP-RPC server

      --http.addr <HTTP_ADDR>
          Http server address to listen on

      --http.port <HTTP_PORT>
          Http server port to listen on

      --http.api <HTTP_API>
          Rpc Modules to be configured for the HTTP server
          
          [possible values: admin, debug, eth, net, trace, txpool, web3, rpc]

      --http.corsdomain <HTTP_CORSDOMAIN>
          Http Corsdomain to allow request from

      --ws
          Enable the WS-RPC server

      --ws.addr <WS_ADDR>
          Ws server address to listen on

      --ws.port <WS_PORT>
          Ws server port to listen on

      --ws.origins <ws.origins>
          Origins from which to accept WebSocket requests

      --ws.api <WS_API>
          Rpc Modules to be configured for the WS server
          
          [possible values: admin, debug, eth, net, trace, txpool, web3, rpc]

      --ipcdisable
          Disable the IPC-RPC  server

      --ipcpath <IPCPATH>
          Filename for IPC socket/pipe within the datadir

      --authrpc.addr <AUTH_ADDR>
          Auth server address to listen on

      --authrpc.port <AUTH_PORT>
          Auth server port to listen on

      --authrpc.jwtsecret <PATH>
          Path to a JWT secret to use for authenticated RPC endpoints

      --rpc-max-request-size <RPC_MAX_REQUEST_SIZE>
          Set the maximum RPC request payload size for both HTTP and WS in megabytes
          
          [default: 15]

      --rpc-max-response-size <RPC_MAX_RESPONSE_SIZE>
          Set the maximum RPC response payload size for both HTTP and WS in megabytes
          
          [default: 100]

      --rpc-max-subscriptions-per-connection <RPC_MAX_SUBSCRIPTIONS_PER_CONNECTION>
          Set the the maximum concurrent subscriptions per connection
          
          [default: 1024]

      --rpc-max-connections <COUNT>
          Maximum number of RPC server connections
          
          [default: 100]

      --rpc-max-tracing-requests <COUNT>
          Maximum number of concurrent tracing requests
          
          [default: 25]

Gas Price Oracle:
      --gpo.blocks <BLOCKS>
          Number of recent blocks to check for gas price
          
          [default: 20]

      --gpo.ignoreprice <IGNORE_PRICE>
          Gas Price below which gpo will ignore transactions
          
          [default: 2]

      --gpo.maxprice <MAX_PRICE>
          Maximum transaction priority fee(or gasprice before London Fork) to be recommended by gpo
          
          [default: 500000000000]

      --gpo.percentile <PERCENTILE>
          The percentile of gas prices to use for the estimate
          
          [default: 60]

      --block-cache-len <BLOCK_CACHE_LEN>
          Maximum number of block cache entries
          
          [default: 5000]

      --receipt-cache-len <RECEIPT_CACHE_LEN>
          Maximum number of receipt cache entries
          
          [default: 2000]

      --env-cache-len <ENV_CACHE_LEN>
          Maximum number of env cache entries
          
          [default: 1000]

TxPool:
      --txpool.pending_max_count <PENDING_MAX_COUNT>
          Max number of transaction in the pending sub-pool
          
          [default: 10000]

      --txpool.pending_max_size <PENDING_MAX_SIZE>
          Max size of the pending sub-pool in megabytes
          
          [default: 20]

      --txpool.basefee_max_count <BASEFEE_MAX_COUNT>
          Max number of transaction in the basefee sub-pool
          
          [default: 10000]

      --txpool.basefee_max_size <BASEFEE_MAX_SIZE>
          Max size of the basefee sub-pool in megabytes
          
          [default: 20]

      --txpool.queued_max_count <QUEUED_MAX_COUNT>
          Max number of transaction in the queued sub-pool
          
          [default: 10000]

      --txpool.queued_max_size <QUEUED_MAX_SIZE>
          Max size of the queued sub-pool in megabytes
          
          [default: 20]

      --txpool.max_account_slots <MAX_ACCOUNT_SLOTS>
          Max number of executable transaction slots guaranteed per account
          
          [default: 16]

Builder:
      --builder.extradata <EXTRADATA>
          Block extra data set by the payload builder
          
          [default: reth/v0.1.0-alpha.1/macos]

      --builder.gaslimit <GAS_LIMIT>
          Target gas ceiling for built blocks
          
          [default: 30000000]

      --builder.interval <SECONDS>
          The interval at which the job should build a new payload after the last (in seconds)
          
          [default: 1]

      --builder.deadline <SECONDS>
          The deadline for when the payload builder job should resolve
          
          [default: 12]

      --builder.max-tasks <MAX_PAYLOAD_TASKS>
          Maximum number of tasks to spawn for building a payload
          
          [default: 3]

Debug:
      --debug.continuous
          Prompt the downloader to download blocks one at a time.
          
          NOTE: This is for testing purposes only.

      --debug.terminate
          Flag indicating whether the node should be terminated after the pipeline sync

      --debug.tip <TIP>
          Set the chain tip manually for testing purposes.
          
          NOTE: This is a temporary flag

      --debug.max-block <MAX_BLOCK>
          Runs the sync only up to the specified block

      --debug.print-inspector
          Print opcode level traces directly to console during execution

      --debug.hook-block <HOOK_BLOCK>
          Hook on a specific block during execution

      --debug.hook-transaction <HOOK_TRANSACTION>
          Hook on a specific transaction during execution

      --debug.hook-all
          Hook on every transaction in a block

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

      --auto-mine
          Automatically mine blocks for new transactions

Logging:
      --log.persistent
          The flag to enable persistent logs

      --log.directory <PATH>
          The path to put log files in
          
          [default: /reth/logs]

      --log.journald
          Log events to journald

      --log.filter <FILTER>
          The filter to use for logs written to the log file
          
          [default: error]

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
