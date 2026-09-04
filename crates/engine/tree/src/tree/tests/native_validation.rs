//! Differential check between native parallel validation and the sequential simulation profile.

use super::{node_storage::NodeStorage, *};
use crate::tree::payload_validator::BlockOrPayload;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_db_common::init::init_genesis_with_settings;
use reth_evm_ethereum::EthEvmConfig;
use reth_node_ethereum::EthereumEngineValidator;
use reth_provider::{providers::BlockchainProvider, StorageSettings, TrieWriter};

/// Run outside the simulator: native validation waits for real worker threads.
pub(super) fn assert_native_validation(chain: Arc<ChainSpec>, block: SealedBlock<Block>) {
    assert_eq!(block.number(), 1);
    let runtime = reth_tasks::Runtime::test();
    let mut results = Vec::new();
    for sequential in [false, true] {
        let mut storage = NodeStorage::new(chain.clone());
        let overlay = OverlayManager::default();
        let factory = storage.open(overlay.clone(), runtime.clone());
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();
        let provider = BlockchainProvider::new(factory.clone()).unwrap();
        let config = TreeConfig::default()
            .with_has_enough_parallelism(true)
            .with_cross_block_cache_size(1024 * 1024);
        let mut validator = BasicEngineValidator::new(
            provider,
            Arc::new(EthBeaconConsensus::new(chain.clone())),
            EthEvmConfig::new(chain.clone()),
            EthereumEngineValidator::new(chain.clone()),
            config.clone(),
            Box::new(NoopInvalidBlockHook::default()),
            overlay.clone(),
            runtime.clone(),
        );
        if sequential {
            validator = validator.with_sequential_execution();
        }
        let genesis = SealedHeader::seal_slow(chain.genesis_header().clone());
        let mut state = EngineApiTreeState::new(
            10,
            10,
            config.invalid_header_hit_eviction_threshold(),
            genesis.num_hash(),
            EngineApiKind::Ethereum,
            overlay,
        );
        let canonical = CanonicalInMemoryState::with_head(genesis, None, None);
        // Downstream synchronous validator wrappers can be called from an async engine driver.
        // The native entry point must work inside an existing executor without nesting block_on.
        let validated = futures::executor::block_on(async {
            validator.validate_block_with_state::<EthEngineTypes>(
                BlockOrPayload::Block(block.clone().into()),
                TreeCtx::new(&mut state, &canonical),
            )
        })
        .unwrap()
        .executed_block;
        if !sequential {
            // Cache finalization follows the validation result. Drain it before this fixture
            // drops the provider and datadir, including its speculative transaction workers.
            runtime.spawn_blocking_named("prewarm", || ()).get();
        }
        // Force deferred native work to finish before comparing and releasing its datadir.
        let hashed_state = validated.hashed_state();
        let trie_updates = validated.trie_updates();
        // The synchronous strategy can re-emit unchanged nodes which the sparse strategy omits.
        // Compare their effects on the same initial database, rather than their patch contents.
        let writer = factory.provider_rw().unwrap();
        writer.write_trie_updates_sorted(&trie_updates).unwrap();
        writer.commit().unwrap();
        let reader = factory.provider().unwrap();
        let accounts = reader
            .tx_ref()
            .cursor_read::<tables::PackedAccountsTrie>()
            .unwrap()
            .walk(None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let storages = reader
            .tx_ref()
            .cursor_read::<tables::PackedStoragesTrie>()
            .unwrap()
            .walk(None)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        results.push((validated, hashed_state, accounts, storages));
    }
    assert_eq!(results[0], results[1], "native and sequential validation differ");
}
