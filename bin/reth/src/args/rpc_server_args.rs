//! clap [Args](clap::Args) for RPC related arguments.

use crate::dirs::{JwtSecretPath, PlatformPath};
use clap::Args;
use jsonrpsee::{core::Error as RpcError, server::ServerHandle};
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc::{JwtError, JwtSecret};
use reth_rpc_builder::{
    constants, IpcServerBuilder, RethRpcModule, RpcModuleSelection, RpcServerConfig,
    RpcServerHandle, ServerBuilder, TransportRpcModuleConfig,
};
use reth_rpc_engine_api::EngineApiHandle;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
};

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
    pub http_api: Option<RpcModuleSelection>,

    /// Http Corsdomain to allow request from
    #[arg(long = "http.corsdomain")]
    pub http_corsdomain: Option<String>,

    /// Enable the WS-RPC server
    #[arg(long)]
    pub ws: bool,

    /// Ws server address to listen on
    #[arg(long = "ws.addr")]
    pub ws_addr: Option<IpAddr>,

    /// Ws server port to listen on
    #[arg(long = "ws.port")]
    pub ws_port: Option<u16>,

    /// Rpc Modules to be configured for Ws server
    #[arg(long = "ws.api")]
    pub ws_api: Option<RpcModuleSelection>,

    /// Disable the IPC-RPC  server
    #[arg(long)]
    pub ipcdisable: bool,

    /// Filename for IPC socket/pipe within the datadir
    #[arg(long)]
    pub ipcpath: Option<String>,

    /// Auth server address to listen on
    #[arg(long = "authrpc.addr")]
    pub auth_addr: Option<IpAddr>,

    /// Auth server port to listen on
    #[arg(long = "authrpc.port")]
    pub auth_port: Option<u16>,

    /// Path to a JWT secret to use for authenticated RPC endpoints
    #[arg(long = "authrpc.jwtsecret", value_name = "PATH", global = true, required = false)]
    auth_jwtsecret: Option<PlatformPath<JwtSecretPath>>,
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
        let arg = self.auth_jwtsecret.as_ref();
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

    /// Convenience function for starting a rpc server with configs which extracted from cli args.
    pub(crate) async fn start_rpc_server<Client, Pool, Network, Tasks>(
        &self,
        client: Client,
        pool: Pool,
        network: Network,
        executor: Tasks,
    ) -> Result<RpcServerHandle, RpcError>
    where
        Client: BlockProvider
            + HeaderProvider
            + StateProviderFactory
            + EvmEnvProvider
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        reth_rpc_builder::launch(
            client,
            pool,
            network,
            self.transport_rpc_module_config(),
            self.rpc_server_config(),
            executor,
        )
        .await
    }

    /// Create Engine API server.
    pub(crate) async fn start_auth_server<Client, Pool, Network, Tasks>(
        &self,
        client: Client,
        pool: Pool,
        network: Network,
        executor: Tasks,
        handle: EngineApiHandle,
    ) -> Result<ServerHandle, RpcError>
    where
        Client: BlockProvider
            + HeaderProvider
            + StateProviderFactory
            + EvmEnvProvider
            + Clone
            + Unpin
            + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
        Tasks: TaskSpawner + Clone + 'static,
    {
        let socket_address = SocketAddr::new(
            self.auth_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            self.auth_port.unwrap_or(constants::DEFAULT_AUTH_PORT),
        );
        let secret = self.jwt_secret().map_err(|err| RpcError::Custom(err.to_string()))?;
        reth_rpc_builder::auth::launch(
            client,
            pool,
            network,
            executor,
            handle,
            socket_address,
            secret,
        )
        .await
    }

    /// Creates the [TransportRpcModuleConfig] from cli args.
    fn transport_rpc_module_config(&self) -> TransportRpcModuleConfig {
        let mut config = TransportRpcModuleConfig::default();
        let rpc_modules =
            RpcModuleSelection::Selection(vec![RethRpcModule::Admin, RethRpcModule::Eth]);
        if self.http {
            config = config.with_http(self.http_api.as_ref().unwrap_or(&rpc_modules).clone());
        }

        if self.ws {
            config = config.with_ws(self.ws_api.as_ref().unwrap_or(&rpc_modules).clone());
        }

        config
    }

    /// Creates the [RpcServerConfig] from cli args.
    fn rpc_server_config(&self) -> RpcServerConfig {
        let mut config = RpcServerConfig::default();

        if self.http {
            let socket_address = SocketAddr::new(
                self.http_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                self.http_port.unwrap_or(constants::DEFAULT_HTTP_RPC_PORT),
            );
            config = config
                .with_http_address(socket_address)
                .with_http(ServerBuilder::new())
                .with_cors(self.http_corsdomain.clone().unwrap_or_default());
        }

        if self.ws {
            let socket_address = SocketAddr::new(
                self.ws_addr.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                self.ws_port.unwrap_or(constants::DEFAULT_WS_RPC_PORT),
            );
            config = config.with_ws_address(socket_address).with_http(ServerBuilder::new());
        }

        if !self.ipcdisable {
            let ipc_builder = IpcServerBuilder::default();
            config = config.with_ipc(ipc_builder).with_ipc_endpoint(
                self.ipcpath.as_ref().unwrap_or(&constants::DEFAULT_IPC_ENDPOINT.to_string()),
            );
        }

        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::net::SocketAddrV4;

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
        let expected = RpcModuleSelection::try_from_selection(["eth", "admin", "debug"]).unwrap();

        assert_eq!(apis, expected);
    }

    #[test]
    fn test_transport_rpc_module_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            "eth,admin,debug",
            "--http",
            "--ws",
        ])
        .args;
        let config = args.transport_rpc_module_config();
        let expected = vec![RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected);
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            vec![RethRpcModule::Admin, RethRpcModule::Eth]
        );
    }

    #[test]
    fn test_rpc_server_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            "eth,admin,debug",
            "--http",
            "--ws",
            "--ws.addr",
            "127.0.0.1",
            "--ws.port",
            "8888",
        ])
        .args;
        let config = args.rpc_server_config();
        assert_eq!(
            config.http_address().unwrap(),
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::UNSPECIFIED,
                constants::DEFAULT_HTTP_RPC_PORT
            ))
        );
        assert_eq!(
            config.ws_address().unwrap(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8888))
        );
        assert_eq!(config.ipc_endpoint().unwrap().path(), constants::DEFAULT_IPC_ENDPOINT);
    }
}
