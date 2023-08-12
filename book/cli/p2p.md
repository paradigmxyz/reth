# `reth p2p`

P2P Debugging utilities

```bash
$ reth p2p --help

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

## `reth p2p body`

Download block body

```bash
$ reth p2p body --help

Usage: reth p2p body [OPTIONS] <ID>

Arguments:
  <ID>
          The block number or hash

Options:
  -h, --help
          Print help (see a summary with '-h')

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

## `reth p2p header`

Download block header

```bash
$ reth p2p header --help

Usage: reth p2p header [OPTIONS] <ID>

Arguments:
  <ID>
          The header number or hash

Options:
  -h, --help
          Print help (see a summary with '-h')

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
