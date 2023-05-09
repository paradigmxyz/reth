# `reth p2p`

```bash
$ reth p2p --help
P2P Debugging utilities

Usage: reth p2p [OPTIONS] <COMMAND>

Commands:
  header
          Download block header
  body
          Download block body
  help
          Print this message or the help of the given subcommand(s)

Options:
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

      --datadir <DATA_DIR>
          The path to the data dir for all reth files and subdirectories.
          
          Defaults to the OS-specific data directory:
          
          - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
          - Windows: `{FOLDERID_RoamingAppData}/reth/`
          - macOS: `$HOME/Library/Application Support/reth/`
          
          [default: default]

      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This also will deterministically set the peer ID.

  -d, --disable-discovery
          Disable the discovery service

      --disable-dns-discovery
          Disable the DNS discovery

      --disable-discv4-discovery
          Disable Discv4 discovery

      --discovery.port <DISCOVERY_PORT>
          The UDP port to use for P2P discovery/networking. default: 30303

      --trusted-peer <TRUSTED_PEER>
          Target trusted peer

      --trusted-only
          Connect only to trusted peers

      --retries <RETRIES>
          The number of retries per request
          
          [default: 5]

      --nat <NAT>
          [default: any]

  -h, --help
          Print help (see a summary with '-h')

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

## `reth p2p header`

```bash
$ reth p2p header --help
Download block header

Usage: reth p2p header [OPTIONS] <ID>

Arguments:
  <ID>
          The header number or hash

Options:
      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This also will deterministically set the peer ID.

  -h, --help
          Print help (see a summary with '-h')
```


## `reth p2p body`


```bash
$ reth p2p body --help
Download block body

Usage: reth p2p body [OPTIONS] <ID>

Arguments:
  <ID>
          The block number or hash

Options:
      --p2p-secret-key <PATH>
          Secret key to use for this node.
          
          This also will deterministically set the peer ID.

  -h, --help
          Print help (see a summary with '-h')
```
