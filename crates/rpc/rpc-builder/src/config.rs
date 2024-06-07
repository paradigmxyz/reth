use std::{net::SocketAddr, path::PathBuf};

use jsonrpsee::server::ServerBuilder;

use reth_node_core::{args::RpcServerArgs, utils::get_or_create_jwt_secret_from_path};
use reth_rpc::eth::{cache::EthStateCacheConfig, gas_oracle::GasPriceOracleConfig};
use reth_rpc_layer::{JwtError, JwtSecret};
use reth_rpc_server_types::RpcModuleSelection;
use tower::layer::util::Identity;
use tracing::debug;

use crate::{
    auth::AuthServerConfig, error::RpcError, EthConfig, IpcServerBuilder, RpcModuleConfig,
    RpcServerConfig, TransportRpcModuleConfig,
};

/// A trait that provides a configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [`RpcServerArgs`](crate::args::RpcServerArgs) type.
pub trait RethRpcConfig {
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

impl RethRpcConfig for RpcServerArgs {
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
            .rpc_gas_cap(self.rpc_gas_cap)
            .state_cache(self.state_cache_config())
            .gpo_config(self.gas_price_oracle_config())
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
