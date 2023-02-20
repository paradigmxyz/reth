//! Parameters for configuring the rpc more granularity via CLI

/// NetworkArg struct
mod network_args;
pub use network_args::{DiscoveryArgs, NetworkArgs};

/// RpcServerArg struct
mod rpc_server_args;
pub use rpc_server_args::RpcServerArgs;
