//! clap [Args](clap::Args) for RPC related arguments.

use clap::Args;
use reth_rpc_builder::RpcModuleConfig;
use std::net::IpAddr;

/// Parameters for configuring the rpc more granularity via CLI
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Rpc")]
pub struct RpcServerArgs {
    /// Enable the HTTP-RPC server
    #[arg(long)]
    pub http: bool,

    /// Http server address to listen on
    #[arg(long = "http.addr")]
    pub http_addr: Option<IpAddr>,

    /// Http server port to listen on
    #[arg(long = "http.port")]
    pub http_port: Option<u16>,

    /// Rpc Modules to be configured for http server
    #[arg(long = "http.api")]
    pub http_api: Option<RpcModuleConfig>,

    /// Enable the WS-RPC server
    #[arg(long)]
    pub ws: bool,

    /// Ws server address to listen on
    #[arg(long = "ws.addr")]
    pub ws_addr: Option<IpAddr>,

    /// Http server port to listen on
    #[arg(long = "ws.port")]
    pub ws_port: Option<u16>,

    /// Rpc Modules to be configured for Ws server
    #[arg(long = "ws.api")]
    pub ws_api: Option<RpcModuleConfig>,

    /// Disable the IPC-RPC  server
    #[arg(long)]
    pub ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long)]
    pub ipcpath: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_server_args_parser() {
        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--http.api", "eth,admin,debug"])
                .args;

        let apis = args.http_api.unwrap();
        let expected = RpcModuleConfig::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }
}
