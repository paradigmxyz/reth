# `reth db`

Database debugging utilities

```bash
$ reth db --help

Usage: reth db [OPTIONS] <COMMAND>

Commands:
  stats
          Lists all the tables, their entry count and their size
  list
          Lists the contents of a table
  drop
          Deletes all database entries
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

Display:
  -v, --verbosity...
          Set the minimum log level.
          
          -v      Errors
          -vv     Warnings
          -vvv    Info
          -vvvv   Debug
          -vvvvv  Traces (warning: very verbose!)
```

## `reth db stats`

```bash
$ reth db stats --help
Lists all the tables, their entry count and their size

Usage: reth db stats [OPTIONS]

Options:
  -h, --help
```

## `reth db list`

```bash
$ reth db list --help
Lists the contents of a table

Usage: reth db list [OPTIONS] <TABLE>

Arguments:
  <TABLE>
          The table name

Options:
  -s, --start <START>
          Where to start iterating
          
          [default: 0]

  -l, --len <LEN>
          How many items to take from the walker
          
          [default: 5]

  -h, --help
          Print help (see a summary with '-h')
```

## `reth db drop`

```bash
$ reth db drop --help
Deletes all database entries

Usage: reth db drop [OPTIONS]

Options:
  -h, --help
          Print help (see a summary with '-h')
```
