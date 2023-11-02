# `reth stage`

Manipulate individual stages

```bash
$ reth stage --help

Usage: reth stage [OPTIONS] <COMMAND>

Commands:
  run     Run a single stage
  drop    Drop a stage's tables from the database
  dump    Dumps a stage from a range into a new database
  unwind  Unwinds a certain block range, deleting it from the database
  help    Print this message or the help of the given subcommand(s)

Options:
      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          
          Possible values are either a built-in chain or the path to a chain specification file.
          
          Built-in chains:
          - mainnet
          - goerli
          - sepolia
          - holesky
          
          [default: mainnet]

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
          
          [default: /reth/logs]

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

## `reth stage drop`

Drop a stage's tables from the database

```bash
$ reth stage drop --help

Usage: reth stage drop [OPTIONS] <STAGE>
```

## `reth stage dump`

Dumps a stage from a range into a new database

```bash
$ reth stage dump --help

Usage: reth stage dump [OPTIONS] <COMMAND>

Commands:
  execution        Execution stage
  storage-hashing  StorageHashing stage
  account-hashing  AccountHashing stage
  merkle           Merkle stage
  help             Print this message or the help of the given subcommand(s)
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
          
          [possible values: headers, bodies, senders, execution, account-hashing, storage-hashing, hashing, merkle, tx-lookup, account-history, storage-history, total-difficulty]

Options:
      --config <FILE>
          The path to the configuration file to use.

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
```

### `reth stage unwind to-block`

Unwinds the database until the given block number (range is inclusive)

```bash
$ reth stage unwind to-block --help

Usage: reth stage unwind to-block [OPTIONS] <TARGET>

Arguments:
  <TARGET>
```

### `reth stage unwind num-blocks`

Unwinds the given number of blocks from the database

```bash
$ reth stage unwind num-blocks --help

Usage: reth stage unwind num-blocks [OPTIONS] <AMOUNT>

Arguments:
  <AMOUNT>
```
