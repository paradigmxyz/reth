//! database utils

use reth_provider::StateProviderBox;
use reth_revm::{database::StateProviderDatabase, db::CacheDB};

/// Helper alias type for the state's [`CacheDB`]
pub type StateCacheDb = CacheDB<StateProviderDatabase<StateProviderBox>>;
