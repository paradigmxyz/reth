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
  node          Start the node
  init          Initialize the database from a genesis file
  import        This syncs RLP encoded blocks from a file
  db            Database debugging utilities
  stage         Manipulate individual stages
  p2p           P2P Debugging utilities
  test-vectors  Generate Test Vectors
  config        Write config to stdout
  debug         Various debug routines
  recover       Scripts for node recovery
  help          Print this message or the help of the given subcommand(s)

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

  -V, --version
          Print version

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
