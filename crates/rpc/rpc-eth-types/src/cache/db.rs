//! EVM database types used by the RPC state cache.

use reth_revm::database::StateProviderDatabase;
use reth_storage_api::StateProviderBox;
use revm::database::State;

/// Helper alias type for the state's [`State`]
pub type StateCacheDb = State<StateProviderDatabase<StateProviderBox>>;
