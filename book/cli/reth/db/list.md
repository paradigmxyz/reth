# reth db list

Lists the contents of a table

```bash
$ reth db list --help
```
```txt
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

      --min-row-size <MIN_ROW_SIZE>
          Minimum size of row in bytes

          [default: 0]

      --min-key-size <MIN_KEY_SIZE>
          Minimum size of key in bytes

          [default: 0]

      --min-value-size <MIN_VALUE_SIZE>
          Minimum size of value in bytes

          [default: 0]

  -c, --count
          Returns the number of rows found

  -j, --json
          Dump as JSON instead of using TUI

      --raw
          Output bytes instead of human-readable decoded value

      --instance <INSTANCE>
          Add a new instance of a node.

          Configures the ports of the node to avoid conflicts with the defaults. This is useful for running multiple nodes on the same machine.

          Max number of instances is 200. It is chosen in a way so that it's not possible to have port numbers that conflict with each other.

          Changes to the following port numbers: - `DISCOVERY_PORT`: default + `instance` - 1 - `AUTH_PORT`: default + `instance` * 100 - 100 - `HTTP_RPC_PORT`: default - `instance` + 1 - `WS_RPC_PORT`: default + `instance` * 2 - 2

          [default: 1]

  -h, --help
          Print help (see a summary with '-h')

Logging:
      --log.stdout.format <FORMAT>
          The format to use for logs written to stdout

          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.stdout.filter <FILTER>
          The filter to use for logs written to stdout

          [default: ]

      --log.file.format <FORMAT>
          The format to use for logs written to the log file

          [default: terminal]

          Possible values:
          - json:     Represents JSON formatting for logs. This format outputs log records as JSON objects, making it suitable for structured logging
          - log-fmt:  Represents logfmt (key=value) formatting for logs. This format is concise and human-readable, typically used in command-line applications
          - terminal: Represents terminal-friendly formatting for logs

      --log.file.filter <FILTER>
          The filter to use for logs written to the log file

          [default: debug]

      --log.file.directory <PATH>
          The path to put log files in

          [default: <CACHE_DIR>/logs]

      --log.file.max-size <SIZE>
          The maximum size (in MB) of one log file

          [default: 200]

      --log.file.max-files <COUNT>
          The maximum amount of log files that will be stored. If set to 0, background file logging is disabled

          [default: 5]

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