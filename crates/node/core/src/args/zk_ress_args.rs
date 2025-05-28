use clap::Args;

/// Parameters for configuring the `zkress` subprotocol.
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "ZkRess")]
pub struct ZkRessArgs {
    /// Protocols to enable.
    #[arg(long = "zkress.protocol")]
    pub protocols: Vec<String>,
}

impl Default for ZkRessArgs {
    fn default() -> Self {
        Self { protocols: Vec::new() }
    }
}
