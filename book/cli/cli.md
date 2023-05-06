# CLI Reference

The Reth node is operated via the CLI by running the `reth node` command. To stop it, press `ctrl-c`. You may need to wait a bit as Reth tears down existing p2p connections or other cleanup tasks.

However, Reth has more commands than that:

```bash
reth --help
```

Some of the most useful commands as a node developer are:
* [`reth node`](./node.md): Starts the Reth node's components, including the JSON-RPC. 
* [`reth db`](./db.md): Administrative TUI to the key-value store
* [`reth p2p`](./p2p.md): P2P-related utilities
* [`reth stage`](./stage.md): Runs a stage in isolation. Useful for testing and benchmarking.
* [`reth drop-stage`](./drop-stage.md): Drops all the tables associated with a stage. Useful for resetting state.
* [`reth dump-stage`](./dump-stage.md): Dumps all the tables associated with a stage to a new database. Useful for creating snapshots

See below for the full list of commands.

## Commands

```bash
$ reth --help
Reth

Usage: reth [OPTIONS] <COMMAND>

Commands:
  node
          Start the node
  init
          Initialize the database from a genesis file
  import
          This syncs RLP encoded blocks from a file
  db
          Database debugging utilities
  stage
          Run a single stage
  dump-stage
          Dumps a stage from a range into a new database
  drop-stage
          Drops a stage's tables from the database
  p2p
          P2P Debugging utilities
  test-chain
          Run Ethereum blockchain tests
  test-vectors
          Generate Test Vectors
  config
          Write config to stdout
  merkle-debug
          Debug state root calculation
  help
          Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

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
