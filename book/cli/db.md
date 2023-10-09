# `reth db`

Database debugging utilities

```bash
$ reth db --help

Usage: reth db [OPTIONS] <COMMAND>

Commands:
  stats    Lists all the tables, their entry count and their size
  list     Lists the contents of a table
  diff     Create a diff between two database tables or two entire databases
  get      Gets the content of a table for the given key
  drop     Deletes all database entries
  clear    Deletes all table entries
  version  Lists current and local database versions
  path     Returns the full database path
  help     Print this message or the help of the given subcommand(s)

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

## `reth db clear`

Deletes all table entries

```bash
$ reth db clear --help

Usage: reth db clear [OPTIONS] <TABLE>

Arguments:
  <TABLE>
          Table name
```

## `reth db diff`

Create a diff between two database tables or two entire databases

```bash
$ reth db diff --help

Usage: reth db diff [OPTIONS] --secondary-datadir <SECONDARY_DATADIR> --output <OUTPUT>

Options:
      --secondary-datadir <SECONDARY_DATADIR>
          The path to the data dir for all reth files and subdirectories.
```

## `reth db drop`

Deletes all database entries

```bash
$ reth db drop --help

Usage: reth db drop [OPTIONS]

Options:
  -f, --force
          Bypasses the interactive confirmation and drops the database directly
```

## `reth db get`

Gets the content of a table for the given key

```bash
$ reth db get --help

Usage: reth db get [OPTIONS] <TABLE> <KEY>

Arguments:
  <TABLE>
          The table name
          
          NOTE: The dupsort tables are not supported now.

  <KEY>
          The key to get content for
```

## `reth db list`

Lists the contents of a table

```bash
$ reth db list --help

Usage: reth db list [OPTIONS] <TABLE>

Arguments:
  <TABLE>
          The table name

Options:
  -s, --skip <SKIP>
          Skip first N entries
          
          [default: 0]

  -r, --reverse
          Reverse the order of the entries. If enabled last table entries are read

  -l, --len <LEN>
          How many items to take from the walker
          
          [default: 5]

      --search <SEARCH>
          Search parameter for both keys and values. Prefix it with `0x` to search for binary data, and text otherwise.
          
          ATTENTION! For compressed tables (`Transactions` and `Receipts`), there might be missing results since the search uses the raw uncompressed value from the database.

  -c, --count
          Returns the number of rows found

  -j, --json
          Dump as JSON instead of using TUI
```

## `reth db path`

Returns the full database path

```bash
$ reth db path --help

Usage: reth db path [OPTIONS]
```

## `reth db stats`

Lists all the tables, their entry count and their size

```bash
$ reth db stats --help

Usage: reth db stats [OPTIONS]
```

## `reth db version`

Lists current and local database versions

```bash
$ reth db version --help

Usage: reth db version [OPTIONS]
```
