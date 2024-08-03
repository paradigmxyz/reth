//! clap [Args](clap::Args) for Engine related arguments.

use clap::Args;

/// Parameters for configuring the engine
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Engine")]
pub struct EngineArgs {
    /// Enable the engine2 experimental features on reth binary
    #[arg(long = "engine.experimental", default_value = "false")]
    pub experimental: bool,
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
    fn test_parse_engine_args() {
        let default_args = EngineArgs::default();
        let args = CommandParser::<EngineArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
