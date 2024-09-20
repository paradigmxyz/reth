//! clap [Args](clap::Args) for telos configuration

use reth_telos_rpc::eth::telos_client::TelosClientArgs;

#[derive(Debug, Clone, Default, PartialEq, Eq, clap::Args)]
#[clap(next_help_heading = "Telos")]
/// Telos arguments
pub struct TelosArgs {
    /// TelosZero endpoint to use for API calls (send_transaction, get gas price from table)
    #[arg(long = "telos.telos_endpoint", value_name = "HTTP_URL")]
    pub telos_endpoint: Option<String>,

    /// Signer account name
    #[arg(long = "telos.signer_account")]
    pub signer_account: Option<String>,

    /// Signer permission name
    #[arg(long = "telos.signer_permission")]
    pub signer_permission: Option<String>,

    /// Signer private key
    #[arg(long = "telos.signer_key")]
    pub signer_key: Option<String>,

    /// Seconds to cache gas price
    #[arg(long = "telos.gas_cache_seconds")]
    pub gas_cache_seconds: Option<u32>,
}

impl From<TelosArgs> for TelosClientArgs {
    fn from(args: TelosArgs) -> Self {
        TelosClientArgs {
            telos_endpoint: args.telos_endpoint,
            signer_account: args.signer_account,
            signer_permission: args.signer_permission,
            signer_key: args.signer_key,
        }
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
    fn test_parse_database_args() {
        let default_args = TelosArgs::default();
        let args = CommandParser::<TelosArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
