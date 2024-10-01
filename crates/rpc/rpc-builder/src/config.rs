use std::{net::SocketAddr, path::PathBuf};

use jsonrpsee::server::ServerBuilder;
use reth_node_core::{args::RpcServerArgs, utils::get_or_create_jwt_secret_from_path};
use reth_rpc_eth_types::{EthConfig, EthStateCacheConfig, GasPriceOracleConfig};
use reth_rpc_layer::{JwtError, JwtSecret};
use reth_rpc_server_types::RpcModuleSelection;
use tower::layer::util::Identity;
use tracing::{debug, warn};

use crate::{
    auth::AuthServerConfig, error::RpcError, IpcServerBuilder, RpcModuleConfig, RpcServerConfig,
    TransportRpcModuleConfig,
};

/// A trait that provides a configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [`RpcServerArgs`] type.
pub trait RethRpcServerConfig {
    /// Returns whether ipc is enabled.
    fn is_ipc_enabled(&self) -> bool;

    /// Returns the path to the target ipc socket if enabled.
    fn ipc_path(&self) -> &str;

    /// The configured ethereum RPC settings.
    fn eth_config(&self) -> EthConfig;

    /// Returns state cache configuration.
    fn state_cache_config(&self) -> EthStateCacheConfig;

    /// Returns the max request size in bytes.
    fn rpc_max_request_size_bytes(&self) -> u32;

    /// Returns the max response size in bytes.
    fn rpc_max_response_size_bytes(&self) -> u32;

    /// Extracts the gas price oracle config from the args.
    fn gas_price_oracle_config(&self) -> GasPriceOracleConfig;

    /// Creates the [`TransportRpcModuleConfig`] from cli args.
    ///
    /// This sets all the api modules, and configures additional settings like gas price oracle
    /// settings in the [`TransportRpcModuleConfig`].
    fn transport_rpc_module_config(&self) -> TransportRpcModuleConfig;

    /// Returns the default server builder for http/ws
    fn http_ws_server_builder(&self) -> ServerBuilder<Identity, Identity>;

    /// Returns the default ipc server builder
    fn ipc_server_builder(&self) -> IpcServerBuilder<Identity, Identity>;

    /// Creates the [`RpcServerConfig`] from cli args.
    fn rpc_server_config(&self) -> RpcServerConfig;

    /// Creates the [`AuthServerConfig`] from cli args.
    fn auth_server_config(&self, jwt_secret: JwtSecret) -> Result<AuthServerConfig, RpcError>;

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
    ///
    /// The `default_jwt_path` provided as an argument will be used as the default location for the
    /// jwt secret in case the `auth_jwtsecret` argument is not provided.
    fn auth_jwt_secret(&self, default_jwt_path: PathBuf) -> Result<JwtSecret, JwtError>;

    /// Returns the configured jwt secret key for the regular rpc servers, if any.
    ///
    /// Note: this is not used for the auth server (engine API).
    fn rpc_secret_key(&self) -> Option<JwtSecret>;
}

impl RethRpcServerConfig for RpcServerArgs {
    fn is_ipc_enabled(&self) -> bool {
        // By default IPC is enabled therefore it is enabled if the `ipcdisable` is false.
        !self.ipcdisable
    }

    fn ipc_path(&self) -> &str {
        self.ipcpath.as_str()
    }

    fn eth_config(&self) -> EthConfig {
        EthConfig::default()
            .max_tracing_requests(self.rpc_max_tracing_requests)
            .max_blocks_per_filter(self.rpc_max_blocks_per_filter.unwrap_or_max())
            .max_logs_per_response(self.rpc_max_logs_per_response.unwrap_or_max() as usize)
            .eth_proof_window(self.rpc_eth_proof_window)
            .rpc_gas_cap(self.rpc_gas_cap)
            .rpc_max_simulate_blocks(self.rpc_max_simulate_blocks)
            .state_cache(self.state_cache_config())
            .gpo_config(self.gas_price_oracle_config())
            .proof_permits(self.rpc_proof_permits)
    }

    fn state_cache_config(&self) -> EthStateCacheConfig {
        EthStateCacheConfig {
            max_blocks: self.rpc_state_cache.max_blocks,
            max_receipts: self.rpc_state_cache.max_receipts,
            max_envs: self.rpc_state_cache.max_envs,
            max_concurrent_db_requests: self.rpc_state_cache.max_concurrent_db_requests,
        }
    }

    fn rpc_max_request_size_bytes(&self) -> u32 {
        self.rpc_max_request_size.get().saturating_mul(1024 * 1024)
    }

    fn rpc_max_response_size_bytes(&self) -> u32 {
        self.rpc_max_response_size.get().saturating_mul(1024 * 1024)
    }

    fn gas_price_oracle_config(&self) -> GasPriceOracleConfig {
        self.gas_price_oracle.gas_price_oracle_config()
    }

    fn transport_rpc_module_config(&self) -> TransportRpcModuleConfig {
        let mut config = TransportRpcModuleConfig::default()
            .with_config(RpcModuleConfig::new(self.eth_config()));

        if self.http {
            config = config.with_http(
                self.http_api
                    .clone()
                    .unwrap_or_else(|| RpcModuleSelection::standard_modules().into()),
            );
        }

        if self.ws {
            config = config.with_ws(
                self.ws_api
                    .clone()
                    .unwrap_or_else(|| RpcModuleSelection::standard_modules().into()),
            );
        }

        if self.is_ipc_enabled() {
            config = config.with_ipc(RpcModuleSelection::default_ipc_modules());
        }

        config
    }

    fn http_ws_server_builder(&self) -> ServerBuilder<Identity, Identity> {
        ServerBuilder::new()
            .max_connections(self.rpc_max_connections.get())
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection.get())
    }

    fn ipc_server_builder(&self) -> IpcServerBuilder<Identity, Identity> {
        IpcServerBuilder::default()
            .max_subscriptions_per_connection(self.rpc_max_subscriptions_per_connection.get())
            .max_request_body_size(self.rpc_max_request_size_bytes())
            .max_response_body_size(self.rpc_max_response_size_bytes())
            .max_connections(self.rpc_max_connections.get())
    }

    fn rpc_server_config(&self) -> RpcServerConfig {
        let mut config = RpcServerConfig::default().with_jwt_secret(self.rpc_secret_key());

        if self.http_api.is_some() && !self.http {
            warn!(
                target: "reth::cli",
                "The --http.api flag is set but --http is not enabled. HTTP RPC API will not be exposed."
            );
        }

        if self.http {
            let socket_address = SocketAddr::new(self.http_addr, self.http_port);
            config = config
                .with_http_address(socket_address)
                .with_http(self.http_ws_server_builder())
                .with_http_cors(self.http_corsdomain.clone())
                .with_ws_cors(self.ws_allowed_origins.clone());
        }

        if self.ws {
            let socket_address = SocketAddr::new(self.ws_addr, self.ws_port);
            config = config.with_ws_address(socket_address).with_ws(self.http_ws_server_builder());
        }

        if self.is_ipc_enabled() {
            config =
                config.with_ipc(self.ipc_server_builder()).with_ipc_endpoint(self.ipcpath.clone());
        }

        config
    }

    fn auth_server_config(&self, jwt_secret: JwtSecret) -> Result<AuthServerConfig, RpcError> {
        let address = SocketAddr::new(self.auth_addr, self.auth_port);

        let mut builder = AuthServerConfig::builder(jwt_secret).socket_addr(address);
        if self.auth_ipc {
            builder = builder
                .ipc_endpoint(self.auth_ipc_path.clone())
                .with_ipc_config(self.ipc_server_builder());
        }
        Ok(builder.build())
    }

    fn auth_jwt_secret(&self, default_jwt_path: PathBuf) -> Result<JwtSecret, JwtError> {
        match self.auth_jwtsecret.as_ref() {
            Some(fpath) => {
                debug!(target: "reth::cli", user_path=?fpath, "Reading JWT auth secret file");
                JwtSecret::from_file(fpath)
            }
            None => get_or_create_jwt_secret_from_path(&default_jwt_path),
        }
    }

    fn rpc_secret_key(&self) -> Option<JwtSecret> {
        self.rpc_jwtsecret
    }
}

#[cfg(test)]
mod tests {
    use clap::{Args, Parser};
    use reth_node_core::args::RpcServerArgs;
    use reth_rpc_eth_types::RPC_DEFAULT_GAS_CAP;
    use reth_rpc_server_types::{constants, RethRpcModule, RpcModuleSelection};
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    use crate::config::RethRpcServerConfig;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_rpc_gas_cap() {
        let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;
        let config = args.eth_config();
        assert_eq!(config.rpc_gas_cap, Into::<u64>::into(RPC_DEFAULT_GAS_CAP));

        let args =
            CommandParser::<RpcServerArgs>::parse_from(["reth", "--rpc.gascap", "1000"]).args;
        let config = args.eth_config();
        assert_eq!(config.rpc_gas_cap, 1000);

        let args = CommandParser::<RpcServerArgs>::try_parse_from(["reth", "--rpc.gascap", "0"]);
        assert!(args.is_err());
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
        let expected = [RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected.into());
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            RpcModuleSelection::standard_modules()
        );
    }

    #[test]
    fn test_transport_rpc_module_trim_config() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            " eth, admin, debug",
            "--http",
            "--ws",
        ])
        .args;
        let config = args.transport_rpc_module_config();
        let expected = [RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected.into());
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            RpcModuleSelection::standard_modules()
        );
    }

    #[test]
    fn test_unique_rpc_modules() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--http.api",
            " eth, admin, debug, eth,admin",
            "--http",
            "--ws",
        ])
        .args;
        let config = args.transport_rpc_module_config();
        let expected = [RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Debug];
        assert_eq!(config.http().cloned().unwrap().into_selection(), expected.into());
        assert_eq!(
            config.ws().cloned().unwrap().into_selection(),
            RpcModuleSelection::standard_modules()
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
                Ipv4Addr::LOCALHOST,
                constants::DEFAULT_HTTP_RPC_PORT
            ))
        );
        assert_eq!(
            config.ws_address().unwrap(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8888))
        );
        assert_eq!(config.ipc_endpoint().unwrap(), constants::DEFAULT_IPC_ENDPOINT);
    }

    #[test]
    fn test_zero_filter_limits() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--rpc-max-blocks-per-filter",
            "0",
            "--rpc-max-logs-per-response",
            "0",
        ])
        .args;

        let config = args.eth_config().filter_config();
        assert_eq!(config.max_blocks_per_filter, Some(u64::MAX));
        assert_eq!(config.max_logs_per_response, Some(usize::MAX));
    }

    #[test]
    fn test_custom_filter_limits() {
        let args = CommandParser::<RpcServerArgs>::parse_from([
            "reth",
            "--rpc-max-blocks-per-filter",
            "100",
            "--rpc-max-logs-per-response",
            "200",
        ])
        .args;

        let config = args.eth_config().filter_config();
        assert_eq!(config.max_blocks_per_filter, Some(100));
        assert_eq!(config.max_logs_per_response, Some(200));
    }
}
