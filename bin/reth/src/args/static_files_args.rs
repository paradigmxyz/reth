//! Static files arguments

use clap::Args;

/// Parameters for static files
#[derive(Debug, Args, PartialEq, Default)]
#[clap(next_help_heading = "Static Files")]
pub struct StaticFilesArgs {
    /// Use static files. Enables the usage in RPC, historical and live syncs.
    #[arg(long, default_value_t = false, hide = true)]
    pub static_files: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn static_files_args_sanity_check() {
        let default_args = StaticFilesArgs::default();
        let args = CommandParser::<StaticFilesArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
