# `reth node`

The main node operator command.

```bash
$ reth node --help

Start the node

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

      --auto-mine
          Automatically mine blocks for new transactions

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

      --p2p-secret-key <PATH>
          Secret key to use for this node.

          This will also deterministically set the peer ID. If not specified, it will be set in the data dir for the chain being used.

      --no-persist-peers
          Do not persist peers.

      --nat <NAT>
          NAT resolution method

          [default: any]

      --port <PORT>
          Network listening port. default: 30303

Rpc:
      --http
          Enable the HTTP-RPC server

      --http.addr <HTTP_ADDR>
          Http server address to listen on

      --http.port <HTTP_PORT>
          Http server port to listen on

      --http.api <HTTP_API>
          Rpc Modules to be configured for http server

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
          Rpc Modules to be configured for Ws server

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

      --rpc-max-request-size
          Set the maximum RPC request payload size for both HTTP and WS in megabytes.

      --rpc-max-response-size
          Set the maximum RPC response payload size for both HTTP and WS in megabytes.

      --rpc-max-subscriptions-per-connection
          Set the the maximum concurrent subscriptions per connection.

      --rpc-max-connections
          Maximum number of RPC server connections.

      --rpc-max-tracing-requests
          Maximum number of concurrent tracing requests.

      --gas-price-oracle
          Gas price oracle configuration.

      --block-cache-size
          Max size for cached block data in megabytes.

      --receipt-cache-size
          Max size for cached receipt data in megabytes.

      --env-cache-size
          Max size for cached evm env data in megabytes.

Builder:
      --builder.extradata
          Block extra data set by the payload builder.

      --builder.gaslimit
          Target gas ceiling for built blocks.

      --builder.interval
          The interval at which the job should build a new payload after the last (in seconds).

      --builder.deadline
          The deadline for when the payload builder job should resolve.

      --builder.max-tasks
          Maximum number of tasks to spawn for building a payload.

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

Logging:
      --log.persistent
          The flag to enable persistent logs

      --log.directory <PATH>
          The path to put log files in

          [default: /Users/georgios/Library/Caches/reth/logs]

      --log.journald
          Log events to journald

      --log.filter <FILTER>
          The filter to use for logs written to the log file

          [default: debug]

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
