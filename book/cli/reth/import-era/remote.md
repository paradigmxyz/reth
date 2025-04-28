# reth import-era remote

Read ERA1 files from a remote URL

```bash
$ reth import-era remote --help
```
```txt
Usage: reth import-era remote [OPTIONS] <IMPORT_URL>

Arguments:
  <IMPORT_URL>
          The URL to a remote host where the ERA1 files are hosted.

          The ERA1 files are read from the remote host using HTTP GET requests parsing headers
          and bodies.

Options:
      --chain <CHAIN_OR_PATH>
          The chain this node is running.
          Possible values are either a built-in chain or the path to a chain specification file.

          Built-in chains:
              mainnet, sepolia, holesky, hoodi, dev

          [default: mainnet]

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