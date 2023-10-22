//! Config traits for various node components.

use alloy_rlp::Encodable;
use reth_primitives::{Bytes, BytesMut};
use reth_rpc::{eth::gas_oracle::GasPriceOracleConfig, JwtError, JwtSecret};
use reth_rpc_builder::{
    auth::AuthServerConfig, error::RpcError, EthConfig, IpcServerBuilder, RpcServerConfig,
    ServerBuilder, TransportRpcModuleConfig,
};
use std::{borrow::Cow, path::PathBuf, time::Duration};

/// A trait that provides configured RPC server.
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
    fn jwt_secret(&self, default_jwt_path: PathBuf) -> Result<JwtSecret, JwtError>;
}

/// A trait that provides payload builder settings.
///
/// This provides all basic payload builder settings and is implemented by the
/// [PayloadBuilderArgs](crate::args::PayloadBuilderArgs) type.
pub trait PayloadBuilderConfig {
    /// Block extra data set by the payload builder.
    fn extradata(&self) -> Cow<'_, str>;

    /// Returns the rlp-encoded extradata bytes.
    fn extradata_rlp_bytes(&self) -> Bytes {
        let mut extradata = BytesMut::new();
        self.extradata().as_bytes().encode(&mut extradata);
        extradata.freeze().into()
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
