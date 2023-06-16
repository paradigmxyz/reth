# CLI Reference

The Reth node is operated via the CLI by running the `reth node` command. To stop it, press `ctrl-c`. You may need to wait a bit as Reth tears down existing p2p connections or other cleanup tasks.

However, Reth has more commands than that:

```bash
reth --help
```

Some of the most useful commands as a node developer are:
* [`reth node`](./node.md): Starts the Reth node's components, including the JSON-RPC.
* [`reth init`](./init.md): Initialize the database from a genesis file.
* [`reth import`](./import.md): This syncs RLP encoded blocks from a file.
* [`reth db`](./db.md): Administrative TUI to the key-value store.
* [`reth stage`](./stage.md): Runs a stage in isolation. Useful for testing and benchmarking.
* [`reth p2p`](./p2p.md): P2P-related utilities
* [`reth test-vectors`](./test-vectors.md): Generate Test Vectors
* [`reth config`](./config.md): Write config to stdout
* [`reth debug`](./debug.md): Various debug routines

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
          Manipulate individual stages.
  p2p
          P2P Debugging utilities
  test-vectors
          Generate Test Vectors
  config
          Write config to stdout
  debug
          Various debug routines
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
