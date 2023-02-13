//! clap [Args](clap::Args) for RPC related arguments.

use crate::dirs::{JwtSecretPath, PlatformPath};
use clap::Args;
use reth_rpc::{JwtError, JwtSecret};
use reth_rpc_builder::RpcModuleConfig;
use std::{net::IpAddr, path::Path};

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

    /// Path to a JWT secret to use for authenticated RPC endpoints
    #[arg(long = "authrpc.jwtsecret", value_name = "PATH", global = true, required = false)]
    authrpc_jwtsecret: Option<PlatformPath<JwtSecretPath>>,
}

impl RpcServerArgs {
    /// The execution layer and consensus layer clients SHOULD accept a configuration parameter:
    /// jwt-secret, which designates a file containing the hex-encoded 256 bit secret key to be used
    /// for verifying/generating JWT tokens.
    ///
    /// If such a parameter is given, but the file cannot be read, or does not contain a hex-encoded
    /// key of 256 bits, the client SHOULD treat this as an error.
    ///
    /// If such a parameter is not given, the client SHOULD generate such a token, valid for the
    /// duration of the execution, and SHOULD store the hex-encoded secret as a jwt.hex file on
    /// the filesystem. This file can then be used to provision the counterpart client.
    pub(crate) fn jwt_secret(&self) -> Result<JwtSecret, JwtError> {
        let arg = self.authrpc_jwtsecret.as_ref();
        let path: Option<&Path> = arg.map(|p| p.as_ref());
        match path {
            Some(fpath) => JwtSecret::from_file(fpath),
            None => {
                let default_path = PlatformPath::<JwtSecretPath>::default();
                let fpath = default_path.as_ref();
                JwtSecret::try_create(fpath)
            }
        }
    }
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
