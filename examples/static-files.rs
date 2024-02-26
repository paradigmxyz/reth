use std::time::Duration;

use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
use reth_primitives::{SealedBlockWithSenders, MAINNET};
use reth_provider::ProviderFactory;

#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() -> eyre::Result<()> {
    for _ in 0..100 {
        let db = create_test_rw_db();
        let static_files_dir = create_test_static_files_dir();
        let provider_factory =
            ProviderFactory::new(db.as_ref(), MAINNET.clone(), static_files_dir.clone())?;
        let provider = provider_factory.provider_rw()?;

        provider.insert_historical_block(SealedBlockWithSenders::default(), None)?;

        drop(provider_factory);
    }

    // std::thread::sleep(Duration::from_secs(10 * 60));

    Ok(())
}
