use clap::Args;
use reth_primitives::Address;

/// Parameters for configuring the miner
#[derive(Debug, Args, PartialEq, Default)]
pub struct MinerArgs {
    /// Enable mining
    #[arg(long = "mine", help_heading = "Miner")]
    pub mine: bool,

    /// Public address for block mining rewards
    #[arg(long = "miner.etherbase", help_heading = "Miner")]
    pub etherbase: Option<Address>,

    /// Block extra data set by the miner
    #[arg(long = "miner.extradata", help_heading = "Miner")]
    pub extradata: Option<String>,

    /// Target gas ceiling for mined blocks
    #[arg(long = "miner.gaslimit", help_heading = "Miner")]
    pub gaslimit: Option<u64>,

    /// Minimum gas price for mining a transaction
    #[arg(long = "miner.gasprice", help_heading = "Miner")]
    pub gasprice: Option<u64>,

    /// Comma separated HTTP URL list to notify of new work packages
    #[arg(long = "miner.notify", help_heading = "Miner")]
    pub notify: Option<String>,

    /// Comma separated HTTP URL list to notify of new work packages
    #[arg(long = "miner.notify.full", help_heading = "Miner")]
    pub notify_full: bool,

    /// Disable remote sealing verification
    #[arg(long = "miner.noverify", help_heading = "Miner")]
    pub noverify: bool,

    /// Time interval to recreate the block being mined
    #[arg(long = "miner.recommit", help_heading = "Miner")]
    pub recommit: Option<u32>,

    /// Number of CPU threads to use for mining
    #[arg(long = "threads", help_heading = "Miner")]
    pub num_threads: Option<u32>,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_server_config() {
        let args = CommandParser::<MinerArgs>::parse_from([
            "reth",
            "--miner.etherbase",
            "0x4E53051c6Bd7dA2Ad2aa22430AD8543431007D23",
        ])
        .args;

        assert_eq!(
            args.etherbase.unwrap(),
            Address::from_str("0x4E53051c6Bd7dA2Ad2aa22430AD8543431007D23").unwrap()
        );
        assert!(args.extradata.is_none());
        assert!(args.gaslimit.is_none());
    }
}
