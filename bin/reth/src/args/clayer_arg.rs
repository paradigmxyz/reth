//! clap [Args](clap::Args) for Dev testnet configuration

use clap::Args;
/// Parameters for Dev testnet configuration
#[derive(Debug, Args, PartialEq, Default, Clone, Copy)]
#[clap(next_help_heading = "Clayer")]
pub struct ClayerArgs {
    /// This is a temporary parameter used to configure the role of the consensus node. it will be deleted later
    #[arg(long = "clayer.mine", value_name = "CAN_MINE", default_value_t = false)]
    pub mine: bool,
}
