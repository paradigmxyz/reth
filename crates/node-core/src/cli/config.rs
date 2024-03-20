//! Config traits for various node components.

use reth_network::protocol::IntoRlpxSubProtocol;
use reth_primitives::Bytes;
use reth_rpc::{
    eth::{cache::EthStateCacheConfig, gas_oracle::GasPriceOracleConfig},
    JwtError, JwtSecret,
};
use reth_rpc_builder::{
    auth::AuthServerConfig, error::RpcError, EthConfig, IpcServerBuilder, RpcServerConfig,
    ServerBuilder, TransportRpcModuleConfig,
};
use reth_transaction_pool::PoolConfig;
use std::{borrow::Cow, path::PathBuf, time::Duration};

/// A trait that provides a configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [RpcServerArgs](crate::args::RpcServerArgs) type.
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

    /// Creates the [TransportRpcModuleConfig] from cli args.
    ///
    /// This sets all the api modules, and configures additional settings like gas price oracle
    /// settings in the [TransportRpcModuleConfig].
    fn transport_rpc_module_config(&self) -> TransportRpcModuleConfig;

    /// Returns the default server builder for http/ws
    fn http_ws_server_builder(&self) -> ServerBuilder;

    /// Returns the default ipc server builder
    fn ipc_server_builder(&self) -> IpcServerBuilder;

    /// Creates the [RpcServerConfig] from cli args.
    fn rpc_server_config(&self) -> RpcServerConfig;

    /// Creates the [AuthServerConfig] from cli args.
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

/// A trait that provides payload builder settings.
///
/// This provides all basic payload builder settings and is implemented by the
/// [PayloadBuilderArgs](crate::args::PayloadBuilderArgs) type.
pub trait PayloadBuilderConfig {
    /// Block extra data set by the payload builder.
    fn extradata(&self) -> Cow<'_, str>;

    /// Returns the extradata as bytes.
    fn extradata_bytes(&self) -> Bytes {
        self.extradata().as_bytes().to_vec().into()
    }

    /// The interval at which the job should build a new payload after the last.
    fn interval(&self) -> Duration;

    /// The deadline for when the payload builder job should resolve.
    fn deadline(&self) -> Duration;

    /// Target gas ceiling for built blocks.
    fn max_gas_limit(&self) -> u64;

    /// Maximum number of tasks to spawn for building a payload.
    fn max_payload_tasks(&self) -> usize;
}

/// A trait that represents the configured network and can be used to apply additional configuration
/// to the network.
pub trait RethNetworkConfig {
    /// Adds a new additional protocol to the RLPx sub-protocol list.
    ///
    /// These additional protocols are negotiated during the RLPx handshake.
    /// If both peers share the same protocol, the corresponding handler will be included alongside
    /// the `eth` protocol.
    ///
    /// See also [ProtocolHandler](reth_network::protocol::ProtocolHandler)
    fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol);

    /// Returns the secret key used for authenticating sessions.
    fn secret_key(&self) -> secp256k1::SecretKey;

    // TODO add more network config methods here
}

impl<C> RethNetworkConfig for reth_network::NetworkManager<C> {
    fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        reth_network::NetworkManager::add_rlpx_sub_protocol(self, protocol);
    }

    fn secret_key(&self) -> secp256k1::SecretKey {
        self.secret_key()
    }
}

/// A trait that provides all basic config values for the transaction pool and is implemented by the
/// [TxPoolArgs](crate::args::TxPoolArgs) type.
pub trait RethTransactionPoolConfig {
    /// Returns transaction pool configuration.
    fn pool_config(&self) -> PoolConfig;
}
