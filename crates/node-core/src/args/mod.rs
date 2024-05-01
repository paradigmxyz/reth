//! Parameters for configuring the rpc more granularity via CLI

/// NetworkArg struct for configuring the network
mod network;
pub use network::{DiscoveryArgs, NetworkArgs};

/// RpcServerArg struct for configuring the RPC
mod rpc_server;
pub use rpc_server::RpcServerArgs;

/// RpcStateCacheArgs struct for configuring RPC state cache
mod rpc_state_cache;
pub use rpc_state_cache::RpcStateCacheArgs;

/// DebugArgs struct for debugging purposes
mod debug;
pub use debug::DebugArgs;

/// DatabaseArgs struct for configuring the database
mod database;
pub use database::DatabaseArgs;

/// LogArgs struct for configuring the logger
mod log;
pub use log::{ColorMode, LogArgs};

mod secret_key;
pub use secret_key::{get_secret_key, SecretKeyError};

/// PayloadBuilderArgs struct for configuring the payload builder
mod payload_builder;
pub use payload_builder::PayloadBuilderArgs;

/// Stage related arguments
mod stage;
pub use stage::StageEnum;

/// Gas price oracle related arguments
mod gas_price_oracle;
pub use gas_price_oracle::GasPriceOracleArgs;

/// TxPoolArgs for configuring the transaction pool
mod txpool;
pub use txpool::TxPoolArgs;

/// DevArgs for configuring the dev testnet
mod dev;
pub use dev::DevArgs;

/// PruneArgs for configuring the pruning and full node
mod pruning;
pub use pruning::PruningArgs;

pub mod utils;

pub mod types;
