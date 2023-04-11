use clap::{builder::RangedU64ValueParser, Args};
use reth_primitives::Address;

/// Parameters for configuring the Payload Builder
#[derive(Debug, Args, PartialEq, Default)]
pub struct PayloadBuilderArgs {
    /// Public address for block mining rewards
    #[arg(long = "builder.etherbase", help_heading = "Builder")]
    pub etherbase: Option<Address>,

    /// Block extra data set by the builder
    #[arg(long = "builder.extradata", help_heading = "Builder")]
    pub extradata: Option<String>,

    /// Target gas ceiling for mined blocks
    #[arg(long = "builder.gaslimit", help_heading = "Builder")]
    pub gaslimit: Option<u64>,

    /// Minimum gas price for mining a transaction
    #[arg(long = "builder.gasprice", help_heading = "Builder")]
    pub gasprice: Option<u64>,

    /// Time interval to recreate the block being mined in seconds
    #[arg(long = "builder.recommit", help_heading = "Builder")]
    pub recommit: Option<u32>,

    /// Number of CPU threads to use for mining
    #[arg(long = "builder.threads", help_heading = "Builder", value_parser = RangedU64ValueParser::<usize>::new().range(1..=num_cpus::get() as u64))]
    pub num_threads: Option<usize>,
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
    fn test_args_with_valid_num_threads() {
        let num_cpus = num_cpus::get();
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.threads",
            &format!("{}", num_cpus),
        ])
        .args;
        assert!(args.num_threads.is_some())
    }
}
