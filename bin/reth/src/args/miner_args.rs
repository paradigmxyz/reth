use reth_primitives::Address;

#[derive(Debug, Args, PartialEq, Default)]
pub struct MinerArgs {
    #[arg(long = "mine", help_heading = "Miner", default_value = false)]
    pub mine: bool,
    #[arg(long = "miner.etherbase", help_heading = "Miner", default_value = "0x000000000")]
    pub etherbase: Address,

    #[arg(long = "miner.extradata", help_heading = "Miner")]
    pub extradata: String,

    #[arg(long = "miner.gaslimit", help_heading = "Miner", default_value = 30000000)]
    pub gaslimit: u64,

    #[arg(long = "miner.gasprice", help_heading = "Miner", default_value = 0)]
    pub gasprice: u64,

    #[arg(long = "miner.notify", help_heading = "Miner")]
    pub notify: String,

    #[arg(long = "miner.notify.full", help_heading = "Miner", default_value = false)]
    pub notify_full: bool,

    /// Time interval to recreate the block being mined
    #[arg(long = "miner.recommit", help_heading = "Miner", default_value = 3)]
    pub recommit: u32,

    #[arg(long = "threads", help_heading = "Miner", default_value = 0)]
    pub num_threads: u32,
}
