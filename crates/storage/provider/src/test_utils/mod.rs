use crate::ProviderFactory;
use reth_db::{
    test_utils::{create_test_rw_db, TempDatabase},
    DatabaseEnv,
};
use reth_primitives::{ChainSpec, MAINNET};
use std::sync::Arc;

pub mod blocks;
mod events;
mod executor;
mod mock;
mod noop;

pub use events::TestCanonStateSubscriptions;
pub use executor::{TestExecutor, TestExecutorFactory};
pub use mock::{ExtendedAccount, MockEthProvider};
pub use noop::NoopProvider;

/// Creates test provider factory with mainnet chain spec.
pub fn create_test_provider_factory() -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    create_test_provider_factory_with_chain_spec(MAINNET.clone())
}

/// Creates test provider factory with provided chain spec.
pub fn create_test_provider_factory_with_chain_spec(
    chain_spec: Arc<ChainSpec>,
) -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    let db = create_test_rw_db();
    ProviderFactory::new(db, chain_spec)
}
