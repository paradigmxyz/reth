# `reth debug`

Various debug routines

```bash
$ reth debug --help

Usage: reth debug [OPTIONS] <COMMAND>

Commands:
  execution         Debug the roundtrip execution of blocks as well as the generated data
  merkle            Debug the clean & incremental state root calculations
  in-memory-merkle  Debug in-memory state root calculation
  help              Print this message or the help of the given subcommand(s)

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
      --log.directory <PATH>
          The path to put log files in
          
          [default: /reth/logs]

      --log.max-size <SIZE>
          The maximum size (in MB) of log files
          
          [default: 200]

      --log.max-files <COUNT>
          The maximum amount of log files that will be stored. If set to 0, background file logging is disabled
          
          [default: 5]

      --log.journald
          Log events to journald

      --log.filter <FILTER>
          The filter to use for logs written to the log file
          
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

## `reth debug execution`

Debug the roundtrip execution of blocks as well as the generated data

```bash
$ reth debug execution --help

Usage: reth debug execution [OPTIONS] --to <TO>
```

## `reth debug merkle`

Debug the clean & incremental state root calculations

```bash
$ reth debug merkle --help

Usage: reth debug merkle [OPTIONS] --to <TO>
```

## `reth debug in-memory-merkle`

Debug in-memory state root calculation

```bash
$ reth debug in-memory-merkle --help

Usage: reth debug in-memory-merkle [OPTIONS]
```