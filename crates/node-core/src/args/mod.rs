//! Parameters for configuring the rpc more granularity via CLI

/// NetworkArg struct for configuring the network
mod network_args;
pub use network_args::{DiscoveryArgs, NetworkArgs};

/// RpcServerArg struct for configuring the RPC
mod rpc_server_args;
pub use rpc_server_args::RpcServerArgs;

/// RpcStateCacheArgs struct for configuring RPC state cache
mod rpc_state_cache_args;
pub use rpc_state_cache_args::RpcStateCacheArgs;

/// DebugArgs struct for debugging purposes
mod debug_args;
pub use debug_args::DebugArgs;

/// DatabaseArgs struct for configuring the database
mod database_args;
pub use database_args::DatabaseArgs;

/// LogArgs struct for configuring the logger
mod log_args;
pub use log_args::{ColorMode, LogArgs};

mod secret_key;
pub use secret_key::{get_secret_key, SecretKeyError};

/// PayloadBuilderArgs struct for configuring the payload builder
mod payload_builder_args;
pub use payload_builder_args::PayloadBuilderArgs;

/// Stage related arguments
mod stage_args;
pub use stage_args::StageEnum;

/// Gas price oracle related arguments
mod gas_price_oracle_args;
pub use gas_price_oracle_args::GasPriceOracleArgs;

/// TxPoolArgs for configuring the transaction pool
mod txpool_args;
pub use txpool_args::TxPoolArgs;

/// DevArgs for configuring the dev testnet
mod dev_args;
pub use dev_args::DevArgs;

/// PruneArgs for configuring the pruning and full node
mod pruning_args;
pub use pruning_args::PruningArgs;

pub mod utils;

pub mod types;
