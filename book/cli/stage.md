# `reth stage`

Manipulate individual stages

```bash
$ reth stage --help

Usage: reth stage [OPTIONS] <COMMAND>

Commands:
  run
          Run a single stage
  drop
          Drop a stage's tables from the database
  dump
          Dumps a stage from a range into a new database
  unwind
          Unwinds a certain block range, deleting it from the database
  help
          Print this message or the help of the given subcommand(s)

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

## `reth stage drop`

Drop a stage's tables from the database

```bash
$ reth stage drop --help

Usage: reth stage drop [OPTIONS] <STAGE>

Arguments:
  <STAGE>
          [possible values: headers, bodies, senders, execution, account-hashing, storage-hashing, hashing, merkle, tx-lookup, history, account-history, storage-history, total-difficulty]

Options:
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

## `reth stage dump`

Dumps a stage from a range into a new database

```bash
$ reth stage dump --help

Usage: reth stage dump [OPTIONS] <COMMAND>

Commands:
  execution
          Execution stage
  storage-hashing
          StorageHashing stage
  account-hashing
          AccountHashing stage
  merkle
          Merkle stage
  help
          Print this message or the help of the given subcommand(s)

Options:
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

### `reth stage dump execution`

Execution stage

```bash
$ reth stage dump execution --help

Usage: reth stage dump execution [OPTIONS] --output-db <OUTPUT_PATH> --from <FROM> --to <TO>

Options:
      --output-db <OUTPUT_PATH>
          The path to the new database folder.

  -f, --from <FROM>
          From which block

  -t, --to <TO>
          To which block

  -d, --dry-run
          If passed, it will dry-run a stage execution from the newly created database right after dumping

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

### `reth stage dump storage-hashing`

StorageHashing stage

```bash
$ reth stage dump storage-hashing --help

Usage: reth stage dump storage-hashing [OPTIONS] --output-db <OUTPUT_PATH> --from <FROM> --to <TO>

Options:
      --output-db <OUTPUT_PATH>
          The path to the new database folder.

  -f, --from <FROM>
          From which block

  -t, --to <TO>
          To which block

  -d, --dry-run
          If passed, it will dry-run a stage execution from the newly created database right after dumping

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

### `reth stage dump account-hashing`

AccountHashing stage

```bash
$ reth stage dump account-hashing --help

Usage: reth stage dump account-hashing [OPTIONS] --output-db <OUTPUT_PATH> --from <FROM> --to <TO>

Options:
      --output-db <OUTPUT_PATH>
          The path to the new database folder.

  -f, --from <FROM>
          From which block

  -t, --to <TO>
          To which block

  -d, --dry-run
          If passed, it will dry-run a stage execution from the newly created database right after dumping

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

### `reth stage dump merkle`

Merkle stage

```bash
$ reth stage dump merkle --help

Usage: reth stage dump merkle [OPTIONS] --output-db <OUTPUT_PATH> --from <FROM> --to <TO>

Options:
      --output-db <OUTPUT_PATH>
          The path to the new database folder.

  -f, --from <FROM>
          From which block

  -t, --to <TO>
          To which block

  -d, --dry-run
          If passed, it will dry-run a stage execution from the newly created database right after dumping

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

## `reth stage run`

Run a single stage.

```bash
$ reth stage run --help

Note that this won't use the Pipeline and as a result runs stages assuming that all the data can be held in memory. It is not recommended to run a stage for really large block ranges if your computer does not have a lot of memory to store all the data.

Usage: reth stage run [OPTIONS] --from <FROM> --to <TO> <STAGE>

Arguments:
  <STAGE>
          The name of the stage to run
          
          [possible values: headers, bodies, senders, execution, account-hashing, storage-hashing, hashing, merkle, tx-lookup, history, account-history, storage-history, total-difficulty]

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

      --metrics <SOCKET>
          Enable Prometheus metrics.
          
          The metrics will be served at the given interface and port.

      --from <FROM>
          The height to start at

  -t, --to <TO>
          The end of the stage

      --batch-size <BATCH_SIZE>
          Batch size for stage execution and unwind

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

  -c, --commit
          Commits the changes in the database. WARNING: potentially destructive.
          
          Useful when you want to run diagnostics on the database.

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

## `reth stage unwind`

Unwinds a certain block range, deleting it from the database

```bash
$ reth stage unwind --help

Usage: reth stage unwind [OPTIONS] <COMMAND>

Commands:
  to-block
          Unwinds the database until the given block number (range is inclusive)
  num-blocks
          Unwinds the given number of blocks from the database
  help
          Print this message or the help of the given subcommand(s)

Options:
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

### `reth stage unwind to-block`

Unwinds the database until the given block number (range is inclusive)

```bash
$ reth stage unwind to-block --help

Usage: reth stage unwind to-block [OPTIONS] <TARGET>

Arguments:
  <TARGET>
          

Options:
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

### `reth stage unwind num-blocks`

Unwinds the given number of blocks from the database

```bash
$ reth stage unwind num-blocks --help

Usage: reth stage unwind num-blocks [OPTIONS] <AMOUNT>

Arguments:
  <AMOUNT>
          

Options:
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
