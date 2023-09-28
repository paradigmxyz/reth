# `reth config`

Write config to stdout

```bash
$ reth config --help

Usage: reth config [OPTIONS]

Options:
      --config <FILE>
          The path to the configuration file to use.

      --default
          Show the default config

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
