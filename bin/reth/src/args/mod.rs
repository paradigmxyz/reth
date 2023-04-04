//! Parameters for configuring the rpc more granularity via CLI

/// NetworkArg struct for configuring the network
mod network_args;
pub use network_args::{DiscoveryArgs, NetworkArgs};

/// RpcServerArg struct for configuring the RPC
mod rpc_server_args;
pub use rpc_server_args::RpcServerArgs;

/// DebugArgs struct for debugging purposes
mod debug_args;
pub use debug_args::DebugArgs;

mod secret_key;
pub use secret_key::{get_secret_key, SecretKeyError};
