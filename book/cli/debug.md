# `reth debug`

```bash
$ reth debug --help
Various debug routines

Usage: reth debug <COMMAND>

Commands:
  execution
          Debug the roundtrip execution of blocks as well as the generated data.
  merkle
          Debug the clean & incremental state root calculations.
  help
          Print this message or the help of the given subcommand(s)

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

## `reth debug execution`

```bash
$ reth debug execution --help
Debug the roundtrip execution of blocks as well as the generated data.

Usage: reth debug execution [OPTIONS]

Options:
          --datadir <DATA_DIR>
                  The path to the data dir for all reth files and subdirectories.

                  Defaults to the OS-specific data directory:

                  - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
                  - Windows: `{FOLDERID_RoamingAppData}/reth/`
                  - macOS: `$HOME/Library/Application Support/reth/`

          --chain <CHAIN_OR_PATH>
                  The chain this node is running.

                  Possible values are either a built-in chain or the path to a chain specification file.

                  Built-in chains:
                  - mainnet
                  - goerli
                  - sepolia

                  [default: mainnet]

          --debug.tip
                  Set the chain tip manually for testing purposes.

          --to
                  The maximum block height.

          --interval
                  The block interval for sync and unwind.

                  [default: 1000]

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

```

## `reth debug merkle`

```bash
$ reth debug merkle --help
Debug the clean & incremental state root calculations.

Usage: reth debug merkle [OPTIONS]

Options:
          --datadir <DATA_DIR>
                  The path to the data dir for all reth files and subdirectories.

                  Defaults to the OS-specific data directory:

                  - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
                  - Windows: `{FOLDERID_RoamingAppData}/reth/`
                  - macOS: `$HOME/Library/Application Support/reth/`

          --chain <CHAIN_OR_PATH>
                  The chain this node is running.

                  Possible values are either a built-in chain or the path to a chain specification file.

                  Built-in chains:
                  - mainnet
                  - goerli
                  - sepolia

                  [default: mainnet]

          --to
                  The height to finish at

          --skip-node-depth
                  The depth after which we should start comparing branch nodes
```
