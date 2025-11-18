//! Transaction trace arguments

use clap::Args;
use std::path::PathBuf;

/// Transaction tracing parameters
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Transaction Trace")]
pub struct TransactionTraceArgs {
    /// Enable transaction tracing
    #[arg(long = "tx-trace.enable", help_heading = "Transaction Trace")]
    pub enable: bool,

    /// Path to write transaction trace output file
    ///
    /// If the path ends with a directory separator or has no extension, `trace.log` will be
    /// appended. Otherwise, the path is used directly as the output file.
    #[arg(long = "tx-trace.output-path", help_heading = "Transaction Trace", value_name = "PATH")]
    pub output_path: Option<PathBuf>,
}

impl Default for TransactionTraceArgs {
    fn default() -> Self {
        Self { enable: false, output_path: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn transaction_trace_args_default_sanity_test() {
        let default_args = TransactionTraceArgs::default();
        let args = CommandParser::<TransactionTraceArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn transaction_trace_parse_enable() {
        let args =
            CommandParser::<TransactionTraceArgs>::parse_from(["reth", "--tx-trace.enable"]).args;
        assert!(args.enable);
    }

    #[test]
    fn transaction_trace_parse_output_path() {
        let args = CommandParser::<TransactionTraceArgs>::parse_from([
            "reth",
            "--tx-trace.output-path",
            "/tmp/tx-traces",
        ])
        .args;
        assert_eq!(args.output_path, Some(PathBuf::from("/tmp/tx-traces")));
    }

    #[test]
    fn transaction_trace_parse_both() {
        let args = CommandParser::<TransactionTraceArgs>::parse_from([
            "reth",
            "--tx-trace.enable",
            "--tx-trace.output-path",
            "/tmp/tx-traces",
        ])
        .args;
        assert!(args.enable);
        assert_eq!(args.output_path, Some(PathBuf::from("/tmp/tx-traces")));
    }
}
