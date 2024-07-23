use crate::{providers::StaticFileProvider, ProviderFactory};
use reth_chainspec::{ChainSpec, MAINNET};
use reth_db::{
    test_utils::{create_test_rw_db, create_test_static_files_dir, TempDatabase},
    DatabaseEnv,
};
use std::sync::Arc;

pub mod blocks;
mod mock;
mod noop;

pub use mock::{ExtendedAccount, MockEthProvider};
pub use noop::NoopProvider;
pub use reth_chain_state::test_utils::TestCanonStateSubscriptions;

/// Creates test provider factory with mainnet chain spec.
pub fn create_test_provider_factory() -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    create_test_provider_factory_with_chain_spec(MAINNET.clone())
}

/// Creates test provider factory with provided chain spec.
pub fn create_test_provider_factory_with_chain_spec(
    chain_spec: Arc<ChainSpec>,
) -> ProviderFactory<Arc<TempDatabase<DatabaseEnv>>> {
    let (static_dir, _) = create_test_static_files_dir();
    let db = create_test_rw_db();
    ProviderFactory::new(
        db,
        chain_spec,
        StaticFileProvider::read_write(static_dir.into_path()).expect("static file provider"),
    )
}
