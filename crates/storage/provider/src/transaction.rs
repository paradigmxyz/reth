use reth_interfaces::{db::DatabaseError as DbError, provider::ProviderError};
use reth_primitives::{BlockHash, BlockNumber, H256};
use reth_trie::StateRootError;
use std::fmt::Debug;

/// An error that can occur when using the transaction container
#[derive(Debug, PartialEq, Eq, Clone, thiserror::Error)]
pub enum TransactionError {
    /// The transaction encountered a database error.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The transaction encountered a database integrity error.
    #[error(transparent)]
    DatabaseIntegrity(#[from] ProviderError),
    /// The trie error.
    #[error(transparent)]
    TrieError(#[from] StateRootError),
    /// Root mismatch
    #[error("Merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    StateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
    /// Root mismatch during unwind
    #[error("Unwind merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    UnwindStateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Target block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
}

#[cfg(test)]
mod test {
    use crate::{
        insert_canonical_block, test_utils::blocks::*, ShareableDatabase, TransactionsProvider,
    };
    use reth_db::{
        mdbx::test_utils::create_test_rw_db,
        models::{storage_sharded_key::StorageShardedKey, ShardedKey},
        tables,
    };
    use reth_primitives::{ChainSpecBuilder, IntegerList, H160, MAINNET, U256};
    use std::sync::Arc;

    #[test]
    fn insert_block_and_hashes_get_take() {
        let db = create_test_rw_db();

        // setup
        let chain_spec = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .shanghai_activated()
            .build();

        let factory = ShareableDatabase::new(db.as_ref(), Arc::new(chain_spec.clone()));
        let mut provider = factory.provider_rw().unwrap();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        let acc1_shard_key = ShardedKey::new(H160([0x60; 20]), u64::MAX);
        let acc2_shard_key = ShardedKey::new(H160([0x61; 20]), u64::MAX);
        let storage1_shard_key =
            StorageShardedKey::new(H160([0x60; 20]), U256::from(5).into(), u64::MAX);

        insert_canonical_block(provider.tx_ref(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&provider, data.genesis);

        provider.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        assert_eq!(
            provider.table::<tables::AccountHistory>().unwrap(),
            vec![
                (acc1_shard_key.clone(), IntegerList::new(vec![1]).unwrap()),
                (acc2_shard_key.clone(), IntegerList::new(vec![1]).unwrap())
            ]
        );
        assert_eq!(
            provider.table::<tables::StorageHistory>().unwrap(),
            vec![(storage1_shard_key.clone(), IntegerList::new(vec![1]).unwrap())]
        );

        // get one block
        let get = provider.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        let get_block = get[0].0.clone();
        let get_state = get[0].1.clone();
        assert_eq!(get_block, block1);
        assert_eq!(get_state, exec_res1);

        // take one block
        let take = provider.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&provider, genesis.clone());

        // check if history is empty.
        assert_eq!(provider.table::<tables::AccountHistory>().unwrap(), vec![]);
        assert_eq!(provider.table::<tables::StorageHistory>().unwrap(), vec![]);

        provider.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();
        provider.append_blocks_with_post_state(vec![block2.clone()], exec_res2.clone()).unwrap();

        // check history of two blocks
        assert_eq!(
            provider.table::<tables::AccountHistory>().unwrap(),
            vec![
                (acc1_shard_key, IntegerList::new(vec![1, 2]).unwrap()),
                (acc2_shard_key, IntegerList::new(vec![1]).unwrap())
            ]
        );
        assert_eq!(
            provider.table::<tables::StorageHistory>().unwrap(),
            vec![(storage1_shard_key, IntegerList::new(vec![1, 2]).unwrap())]
        );
        provider.commit().unwrap();

        // Check that transactions map onto blocks correctly.
        {
            let provider = factory.provider_rw().unwrap();
            assert_eq!(
                provider.transaction_block(0).unwrap(),
                Some(1),
                "Transaction 0 should be in block 1"
            );
            assert_eq!(
                provider.transaction_block(1).unwrap(),
                Some(2),
                "Transaction 1 should be in block 2"
            );
            assert_eq!(
                provider.transaction_block(2).unwrap(),
                None,
                "Transaction 0 should not exist"
            );
        }

        let provider = factory.provider_rw().unwrap();
        // get second block
        let get = provider.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = provider.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get[0].0, block1);
        assert_eq!(get[1].0, block2);
        assert_eq!(get[0].1, exec_res1);
        assert_eq!(get[1].1, exec_res2);

        // take two blocks
        let get = provider.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&provider, genesis);
    }

    #[test]
    fn insert_get_take_multiblocks() {
        let db = create_test_rw_db();

        // setup

        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .shanghai_activated()
                .build(),
        );

        let factory = ShareableDatabase::new(db.as_ref(), chain_spec.clone());
        let mut provider = factory.provider_rw().unwrap();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        insert_canonical_block(provider.tx_mut(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&provider, data.genesis);

        provider.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        // get one block
        let get = provider.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(get, vec![(block1.clone(), exec_res1.clone())]);

        // take one block
        let take = provider.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&provider, genesis.clone());

        // insert two blocks
        let mut merged_state = exec_res1.clone();
        merged_state.extend(exec_res2.clone());
        provider
            .append_blocks_with_post_state(
                vec![block1.clone(), block2.clone()],
                merged_state.clone(),
            )
            .unwrap();

        // get second block
        let get = provider.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = provider.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(
            get,
            vec![(block1.clone(), exec_res1.clone()), (block2.clone(), exec_res2.clone())]
        );

        // take two blocks
        let get = provider.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&provider, genesis);
    }
}
