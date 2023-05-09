# `reth stage`

```bash
Run a single stage.

Usage: reth stage [OPTIONS] --from <FROM> --to <TO> <STAGE>

Arguments:
  <STAGE>
          The name of the stage to run
          
          [possible values: headers, bodies, senders, execution, hashing, merkle, tx-lookup, history]

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
          - mainnet
          - goerli
          - sepolia
          
          [default: mainnet]

      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This also will deterministically set the peer ID.

      --metrics <SOCKET>
          Enable Prometheus metrics.
          
          The metrics will be served at the given interface and port.

      --from <FROM>
          The height to start at

  -t, --to <TO>
          The end of the stage

  -s, --skip-unwind
          Normally, running the stage requires unwinding for stages that already have been run, in order to not rewrite to the same database slots.
          
          You can optionally skip the unwinding phase if you're syncing a block range that has not been synced before.

  -h, --help
          Print help (see a summary with '-h')

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

      --no-persist-peers
          Do not persist peers.

      --nat <NAT>
          NAT resolution method
          
          [default: any]

      --port <PORT>
          Network listening port. default: 30303

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
