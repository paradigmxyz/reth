use std::error::Error;

use clap::Args;
use reth_primitives::Address;

/// Parameters for configuring the builder
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
    #[arg(long = "builder.threads", help_heading = "Builder")]
    pub num_threads: Option<usize>,
}

impl PayloadBuilderArgs {
    /// Validate the arguments
    pub fn validate(&self) -> Result<(), Box<dyn Error>> {
        if let Some(num_threads) = self.num_threads {
            let num_cpus = num_cpus::get();
            dbg!(num_cpus);
            if num_threads < 1 || num_threads > num_cpus {
                return Err(format!(
                    "Invalid number of threads: {}. Must be between 1 and {}",
                    num_threads, num_cpus
                )
                .into())
            }
        }
        Ok(())
    }
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
            "--builder.etherbase",
            "0x5408ad45fd5db107766ff88f5faa4b8d1aab6121",
            "--builder.threads",
            &format!("{}", num_cpus - 1),
        ])
        .args;

        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_args_with_invalid_num_threads() {
        let num_cpus = num_cpus::get();
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.etherbase",
            "0x5408ad45fd5db107766ff88f5faa4b8d1aab6121",
            "--builder.threads",
            &format!("{}", num_cpus + 1),
        ])
        .args;

        assert!(args.validate().is_err());
    }
}
