use crate::{BlockNumReader, DatabaseProviderFactory, HeaderProvider};
use alloy_primitives::B256;
use reth_storage_api::StateCommitmentProvider;
pub use reth_storage_errors::provider::ConsistentViewError;
use reth_storage_errors::provider::ProviderResult;

/// A consistent view over state in the database.
///
/// View gets initialized with the latest or provided tip.
/// Upon every attempt to create a database provider, the view will
/// perform a consistency check of current tip against the initial one.
///
/// ## Usage
///
/// The view should only be used outside of staged-sync.
/// Otherwise, any attempt to create a provider will result in [`ConsistentViewError::Syncing`].
///
/// When using the view, the consumer should either
/// 1) have a failover for when the state changes and handle [`ConsistentViewError::Inconsistent`]
///    appropriately.
/// 2) be sure that the state does not change.
#[derive(Clone, Debug)]
pub struct ConsistentDbView<Factory> {
    factory: Factory,
    tip: Option<(B256, u64)>,
}

impl<Factory> ConsistentDbView<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockNumReader + HeaderProvider>
        + StateCommitmentProvider,
{
    /// Creates new consistent database view.
    pub const fn new(factory: Factory, tip: Option<(B256, u64)>) -> Self {
        Self { factory, tip }
    }

    /// Creates new consistent database view with latest tip.
    pub fn new_with_latest_tip(provider: Factory) -> ProviderResult<Self> {
        let provider_ro = provider.database_provider_ro()?;
        let last_num = provider_ro.last_block_number()?;
        let tip = provider_ro.sealed_header(last_num)?.map(|h| (h.hash(), last_num));
        Ok(Self::new(provider, tip))
    }

    /// Creates new read-only provider and performs consistency checks on the current tip.
    pub fn provider_ro(&self) -> ProviderResult<Factory::Provider> {
        // Create a new provider.
        let provider_ro = self.factory.database_provider_ro()?;

        // Check that the currently stored tip is included on-disk.
        // This means that the database may have moved, but the view was not reorged.
        //
        // NOTE: We must use `sealed_header` with the block number here, because if we are using
        // the consistent view provider while we're persisting blocks, we may enter a race
        // condition. Recall that we always commit to static files first, then the database, and
        // that block hash to block number indexes are contained in the database. If we were to
        // fetch the block by hash while we're persisting, the following situation may occur:
        //
        // 1. Persistence appends the latest block to static files.
        // 2. We initialize the consistent view provider, which fetches based on `last_block_number`
        //    and `sealed_header`, which both check static files, setting the tip to the newly
        //    committed block.
        // 3. We attempt to fetch a header by hash, using for example the `header` method. This
        //    checks the database first, to fetch the number corresponding to the hash. Because the
        //    database has not been committed yet, this fails, and we return
        //    `ConsistentViewError::Reorged`.
        // 4. Some time later, the database commits.
        //
        // To ensure this doesn't happen, we just have to make sure that we fetch from the same
        // data source that we used during initialization. In this case, that is static files
        if let Some((hash, number)) = self.tip {
            if provider_ro.sealed_header(number)?.is_none_or(|header| header.hash() != hash) {
                return Err(ConsistentViewError::Reorged { block: hash }.into())
            }
        }

        Ok(provider_ro)
    }
}

#[cfg(test)]
mod tests {
    use reth_errors::ProviderError;
    use std::str::FromStr;

    use super::*;
    use crate::{
        test_utils::create_test_provider_factory_with_chain_spec, BlockWriter,
        StaticFileProviderFactory, StaticFileWriter,
    };
    use alloy_primitives::Bytes;
    use assert_matches::assert_matches;
    use reth_chainspec::{EthChainSpec, MAINNET};
    use reth_ethereum_primitives::{Block, BlockBody};
    use reth_primitives_traits::{block::TestBlock, RecoveredBlock, SealedBlock};
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::StorageLocation;

    #[test]
    fn test_consistent_view_extend() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        let genesis_header = MAINNET.genesis_header();
        let genesis_block =
            SealedBlock::<Block>::seal_parts(genesis_header.clone(), BlockBody::default());
        let genesis_hash: B256 = genesis_block.hash();
        let genesis_block = RecoveredBlock::new_sealed(genesis_block, vec![]);

        // insert the block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(genesis_block, StorageLocation::StaticFiles).unwrap();
        provider_rw.commit().unwrap();

        // create a consistent view provider and check that a ro provider can be made
        let view = ConsistentDbView::new_with_latest_tip(provider_factory.clone()).unwrap();

        // ensure successful creation of a read-only provider.
        assert_matches!(view.provider_ro(), Ok(_));

        // generate a block that extends the genesis
        let mut block = Block::default();
        block.header_mut().parent_hash = genesis_hash;
        block.header_mut().number = 1;
        let sealed_block = SealedBlock::seal_slow(block);
        let recovered_block = RecoveredBlock::new_sealed(sealed_block, vec![]);

        // insert the block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(recovered_block, StorageLocation::StaticFiles).unwrap();
        provider_rw.commit().unwrap();

        // ensure successful creation of a read-only provider, based on this new db state.
        assert_matches!(view.provider_ro(), Ok(_));

        // generate a block that extends that block
        let mut block = Block::default();
        block.header_mut().parent_hash = genesis_hash;
        block.header_mut().number = 2;
        let sealed_block = SealedBlock::seal_slow(block);
        let recovered_block = RecoveredBlock::new_sealed(sealed_block, vec![]);

        // insert the block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(recovered_block, StorageLocation::StaticFiles).unwrap();
        provider_rw.commit().unwrap();

        // check that creation of a read-only provider still works
        assert_matches!(view.provider_ro(), Ok(_));
    }

    #[test]
    fn test_consistent_view_remove() {
        let provider_factory = create_test_provider_factory_with_chain_spec(MAINNET.clone());

        let genesis_header = MAINNET.genesis_header();
        let genesis_block =
            SealedBlock::<Block>::seal_parts(genesis_header.clone(), BlockBody::default());
        let genesis_hash: B256 = genesis_block.hash();
        let genesis_block = RecoveredBlock::new_sealed(genesis_block, vec![]);

        // insert the block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(genesis_block, StorageLocation::Both).unwrap();
        provider_rw.0.static_file_provider().commit().unwrap();
        provider_rw.commit().unwrap();

        // create a consistent view provider and check that a ro provider can be made
        let view = ConsistentDbView::new_with_latest_tip(provider_factory.clone()).unwrap();

        // ensure successful creation of a read-only provider.
        assert_matches!(view.provider_ro(), Ok(_));

        // generate a block that extends the genesis
        let mut block = Block::default();
        block.header_mut().parent_hash = genesis_hash;
        block.header_mut().number = 1;
        let sealed_block = SealedBlock::seal_slow(block);
        let recovered_block = RecoveredBlock::new_sealed(sealed_block.clone(), vec![]);

        // insert the block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(recovered_block, StorageLocation::Both).unwrap();
        provider_rw.0.static_file_provider().commit().unwrap();
        provider_rw.commit().unwrap();

        // create a second consistent view provider and check that a ro provider can be made
        let view = ConsistentDbView::new_with_latest_tip(provider_factory.clone()).unwrap();
        let initial_tip_hash = sealed_block.hash();

        // ensure successful creation of a read-only provider, based on this new db state.
        assert_matches!(view.provider_ro(), Ok(_));

        // remove the block above the genesis block
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.remove_blocks_above(0, StorageLocation::Both).unwrap();
        let sf_provider = provider_rw.0.static_file_provider();
        sf_provider.get_writer(1, StaticFileSegment::Headers).unwrap().prune_headers(1).unwrap();
        sf_provider.commit().unwrap();
        provider_rw.commit().unwrap();

        // ensure unsuccessful creation of a read-only provider, based on this new db state.
        let Err(ProviderError::ConsistentView(boxed_consistent_view_err)) = view.provider_ro()
        else {
            panic!("expected reorged consistent view error, got success");
        };
        let unboxed = *boxed_consistent_view_err;
        assert_eq!(unboxed, ConsistentViewError::Reorged { block: initial_tip_hash });

        // generate a block that extends the genesis with a different hash
        let mut block = Block::default();
        block.header_mut().parent_hash = genesis_hash;
        block.header_mut().number = 1;
        block.header_mut().extra_data =
            Bytes::from_str("6a6f75726e657920746f20697468616361").unwrap();
        let sealed_block = SealedBlock::seal_slow(block);
        let recovered_block = RecoveredBlock::new_sealed(sealed_block, vec![]);

        // reinsert the block at the same height, but with a different hash
        let provider_rw = provider_factory.provider_rw().unwrap();
        provider_rw.insert_block(recovered_block, StorageLocation::Both).unwrap();
        provider_rw.0.static_file_provider().commit().unwrap();
        provider_rw.commit().unwrap();

        // ensure unsuccessful creation of a read-only provider, based on this new db state.
        let Err(ProviderError::ConsistentView(boxed_consistent_view_err)) = view.provider_ro()
        else {
            panic!("expected reorged consistent view error, got success");
        };
        let unboxed = *boxed_consistent_view_err;
        assert_eq!(unboxed, ConsistentViewError::Reorged { block: initial_tip_hash });
    }
}
