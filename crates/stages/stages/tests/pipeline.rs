//! Pipeline forward sync and unwind tests.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, keccak256, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider, MAINNET};
use reth_config::config::StageConfig;
use reth_consensus::noop::NoopConsensus;
use reth_db_api::{cursor::DbCursorRO, models::BlockNumberAddress, transaction::DbTx};
use reth_db_common::init::init_genesis;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder, file_client::FileClient,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_ethereum_primitives::{Block, BlockBody, Transaction};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::EthEvmConfig;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_primitives_traits::{
    crypto::secp256k1::public_key_to_address,
    proofs::{calculate_receipt_root, calculate_transaction_root},
    RecoveredBlock, SealedBlock,
};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader, DBProvider,
    DatabaseProviderFactory, HeaderProvider, OriginalValuesKnown, StageCheckpointReader,
    StateWriter, StaticFileProviderFactory,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages::sets::DefaultStages;
use reth_stages_api::{Pipeline, StageId};
use reth_static_file::StaticFileProducer;
use reth_storage_api::{
    ChangeSetReader, StateProvider, StorageChangeSetReader, StorageSettings, StorageSettingsCache,
};
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::sync::Arc;
use tokio::sync::watch;

/// Counter contract deployed bytecode compiled with Solidity 0.8.31.
/// ```solidity
/// contract Counter {
///     uint256 public count;
///     function increment() public { count += 1; }
/// }
/// ```
const COUNTER_DEPLOYED_BYTECODE: Bytes = bytes!(
    "6080604052348015600e575f5ffd5b50600436106030575f3560e01c806306661abd146034578063d09de08a14604e575b5f5ffd5b603a6056565b604051604591906089565b60405180910390f35b6054605b565b005b5f5481565b60015f5f828254606a919060cd565b92505081905550565b5f819050919050565b6083816073565b82525050565b5f602082019050609a5f830184607c565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f60d5826073565b915060de836073565b925082820190508082111560f35760f260a0565b5b9291505056fea2646970667358221220576016d010ec2f4f83b992fb97d16efd1bc54110c97aa5d5cb47d20d3b39a35264736f6c634300081f0033"
);

/// `increment()` function selector: `keccak256("increment()")`[:4]
const INCREMENT_SELECTOR: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];

/// Contract address (deterministic for test)
const CONTRACT_ADDRESS: Address = Address::new([0x42; 20]);

/// Creates a `FileClient` populated with the given blocks.
fn create_file_client_from_blocks(blocks: Vec<SealedBlock<Block>>) -> Arc<FileClient<Block>> {
    Arc::new(FileClient::from_blocks(blocks))
}

/// Verifies that changesets are queryable from the correct source based on storage settings.
///
/// Queries static files when changesets are configured to be stored there, otherwise queries MDBX.
fn assert_changesets_queryable(
    provider_factory: &reth_provider::ProviderFactory<
        reth_provider::test_utils::MockNodeTypesWithDB,
    >,
    block_range: std::ops::RangeInclusive<u64>,
) -> eyre::Result<()> {
    let provider = provider_factory.provider()?;
    let settings = provider.cached_storage_settings();

    // Verify storage changesets
    if settings.storage_v2 {
        let static_file_provider = provider_factory.static_file_provider();
        static_file_provider.initialize_index()?;
        let storage_changesets =
            static_file_provider.storage_changesets_range(block_range.clone())?;
        assert!(
            !storage_changesets.is_empty(),
            "storage changesets should be queryable from static files for blocks {:?}",
            block_range
        );

        // Verify keys are in hashed format (v2 mode)
        for (_, entry) in &storage_changesets {
            assert!(entry.key.is_hashed(), "v2: storage changeset keys should be tagged as hashed");
        }
    } else {
        let storage_changesets: Vec<_> = provider
            .tx_ref()
            .cursor_dup_read::<reth_db::tables::StorageChangeSets>()?
            .walk_range(BlockNumberAddress::range(block_range.clone()))?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            !storage_changesets.is_empty(),
            "storage changesets should be queryable from MDBX for blocks {:?}",
            block_range
        );

        // Verify keys are plain (not hashed) in v1 mode
        for (_, entry) in &storage_changesets {
            let key = entry.key;
            assert_ne!(
                key,
                keccak256(key),
                "v1: storage changeset key should be plain (not its own keccak256)"
            );
        }
    }

    // Verify account changesets
    if settings.storage_v2 {
        let static_file_provider = provider_factory.static_file_provider();
        static_file_provider.initialize_index()?;
        let account_changesets =
            static_file_provider.account_changesets_range(block_range.clone())?;
        assert!(
            !account_changesets.is_empty(),
            "account changesets should be queryable from static files for blocks {:?}",
            block_range
        );
    } else {
        let account_changesets: Vec<_> = provider
            .tx_ref()
            .cursor_read::<reth_db::tables::AccountChangeSets>()?
            .walk_range(block_range.clone())?
            .collect::<Result<Vec<_>, _>>()?;
        assert!(
            !account_changesets.is_empty(),
            "account changesets should be queryable from MDBX for blocks {:?}",
            block_range
        );
    }

    Ok(())
}

/// Builds downloaders from a `FileClient`.
fn build_downloaders_from_file_client(
    file_client: Arc<FileClient<Block>>,
    genesis: reth_primitives_traits::SealedHeader<Header>,
    stages_config: StageConfig,
    consensus: Arc<NoopConsensus>,
    provider_factory: reth_provider::ProviderFactory<
        reth_provider::test_utils::MockNodeTypesWithDB,
    >,
) -> (impl HeaderDownloader<Header = Header>, impl BodyDownloader<Block = Block>, reth_tasks::Runtime)
{
    let tip = file_client.tip().expect("file client should have tip");
    let min_block = file_client.min_block().expect("file client should have min block");
    let max_block = file_client.max_block().expect("file client should have max block");

    let runtime = reth_tasks::Runtime::test();

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), consensus.clone())
        .into_task_with(&runtime);
    header_downloader.update_local_head(genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, consensus, provider_factory)
        .into_task_with(&runtime);
    body_downloader.set_download_range(min_block..=max_block).expect("set download range");

    (header_downloader, body_downloader, runtime)
}

/// Builds a pipeline with `DefaultStages`.
fn build_pipeline<H, B>(
    provider_factory: reth_provider::ProviderFactory<
        reth_provider::test_utils::MockNodeTypesWithDB,
    >,
    header_downloader: H,
    body_downloader: B,
    max_block: u64,
    tip: B256,
) -> Pipeline<reth_provider::test_utils::MockNodeTypesWithDB>
where
    H: HeaderDownloader<Header = Header> + 'static,
    B: BodyDownloader<Block = Block> + 'static,
{
    let consensus = NoopConsensus::arc();
    let stages_config = StageConfig::default();
    let evm_config = EthEvmConfig::new(provider_factory.chain_spec());

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer =
        StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    let stages = DefaultStages::new(
        provider_factory.clone(),
        tip_rx,
        consensus,
        header_downloader,
        body_downloader,
        evm_config,
        stages_config,
        PruneModes::default(),
        None,
    );

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(stages)
        .build(provider_factory, static_file_producer);
    pipeline.set_tip(tip);
    pipeline
}

/// Shared helper for pipeline forward sync and unwind tests.
///
/// 1. Pre-funds a signer account and deploys a Counter contract in genesis
/// 2. Each block contains two transactions:
///    - ETH transfer to a recipient (account state changes)
///    - Counter `increment()` call (storage state changes)
/// 3. Runs the full pipeline with ALL stages enabled
/// 4. Forward syncs to `num_blocks`, unwinds to `unwind_target`, then re-syncs back to `num_blocks`
///
/// When `storage_settings` is `Some`, the pipeline provider factory is configured with the given
/// settings before genesis initialization (e.g. v2 storage mode).
async fn run_pipeline_forward_and_unwind(
    storage_settings: Option<StorageSettings>,
    num_blocks: u64,
    unwind_target: u64,
) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Generate a keypair for signing transactions
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());

    // Recipient address for ETH transfers
    let recipient_address = Address::new([0x11; 20]);

    // Create a chain spec with:
    // - Signer pre-funded with 1000 ETH
    // - Counter contract pre-deployed at CONTRACT_ADDRESS
    let initial_balance = U256::from(ETH_TO_WEI) * U256::from(1000);
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [
                    (
                        signer_address,
                        GenesisAccount { balance: initial_balance, ..Default::default() },
                    ),
                    (
                        CONTRACT_ADDRESS,
                        GenesisAccount {
                            code: Some(COUNTER_DEPLOYED_BYTECODE),
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .shanghai_activated()
            .build(),
    );

    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&provider_factory).expect("init genesis");

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");
    let evm_config = EthEvmConfig::new(chain_spec.clone());

    // Build blocks by actually executing transactions to get correct state roots
    let mut blocks: Vec<SealedBlock<Block>> = Vec::new();
    let mut parent_hash = genesis.hash();

    let gas_price = INITIAL_BASE_FEE as u128;
    let transfer_value = U256::from(ETH_TO_WEI); // 1 ETH per block

    for block_num in 1..=num_blocks {
        // Each block has 2 transactions: ETH transfer + Counter increment
        let base_nonce = (block_num - 1) * 2;

        // Transaction 1: ETH transfer
        let eth_transfer_tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: base_nonce,
                gas_limit: 21_000,
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(recipient_address),
                value: transfer_value,
                input: Bytes::new(),
                ..Default::default()
            }),
        );

        // Transaction 2: Counter increment
        let counter_tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: base_nonce + 1,
                gas_limit: 100_000, // Enough gas for SSTORE operations
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(CONTRACT_ADDRESS),
                value: U256::ZERO,
                input: Bytes::from(INCREMENT_SELECTOR.to_vec()),
                ..Default::default()
            }),
        );

        let transactions = vec![eth_transfer_tx, counter_tx];
        let tx_root = calculate_transaction_root(&transactions);

        // Build a temporary header for execution
        let temp_header = Header {
            parent_hash,
            number: block_num,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12,
            ..Default::default()
        };

        // Execute the block to get the state changes
        let provider = provider_factory.database_provider_rw()?;

        let block_with_senders = RecoveredBlock::new_unhashed(
            Block::new(
                temp_header.clone(),
                BlockBody {
                    transactions: transactions.clone(),
                    ommers: Vec::new(),
                    withdrawals: None,
                },
            ),
            vec![signer_address, signer_address], // Both txs from same sender
        );

        // Execute in a scope so state_provider is dropped before we use provider for writes
        let output = {
            let state_provider = provider.latest();
            let db = StateProviderDatabase::new(&*state_provider);
            let executor = evm_config.batch_executor(db);
            executor.execute(&block_with_senders)?
        };

        let gas_used = output.gas_used;

        // Convert bundle state to hashed post state and compute state root
        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
        let (state_root, _trie_updates) = StateRoot::overlay_root_with_updates(
            provider.tx_ref(),
            &hashed_state.clone().into_sorted(),
        )?;

        // Create receipts for receipt root calculation (one per transaction)
        let receipts: Vec<_> = output.receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let receipts_root = calculate_receipt_root(&receipts);

        let header = Header {
            parent_hash,
            number: block_num,
            state_root,
            transactions_root: tx_root,
            receipts_root,
            gas_limit: 30_000_000,
            gas_used,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12,
            ..Default::default()
        };

        let block: SealedBlock<Block> = SealedBlock::seal_parts(
            header.clone(),
            BlockBody { transactions, ommers: Vec::new(), withdrawals: None },
        );

        // Write the plain state to database so subsequent blocks build on it
        let plain_state = output.state.to_plain_state(OriginalValuesKnown::Yes);
        provider.write_state_changes(plain_state)?;
        provider.write_hashed_state(&hashed_state.into_sorted())?;
        provider.commit()?;

        parent_hash = block.hash();
        blocks.push(block);
    }

    // Create a fresh provider factory for the pipeline (clean state from genesis)
    // This is needed because we wrote state during block generation for computing state roots
    let pipeline_provider_factory =
        create_test_provider_factory_with_chain_spec(chain_spec.clone());
    if let Some(settings) = storage_settings {
        pipeline_provider_factory.set_storage_settings_cache(settings);
    }
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis =
        pipeline_provider_factory.sealed_header(0)?.expect("genesis should exist");
    let pipeline_consensus = NoopConsensus::arc();

    let blocks_clone = blocks.clone();
    let file_client = create_file_client_from_blocks(blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader, _runtime) = build_downloaders_from_file_client(
        file_client,
        pipeline_genesis,
        stages_config,
        pipeline_consensus,
        pipeline_provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        pipeline_provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
    );

    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    // Verify forward sync
    {
        let provider = pipeline_provider_factory.provider()?;
        let last_block = provider.last_block_number()?;
        assert_eq!(last_block, num_blocks, "should have synced {num_blocks} blocks");

        for stage_id in [
            StageId::Headers,
            StageId::Bodies,
            StageId::SenderRecovery,
            StageId::Execution,
            StageId::AccountHashing,
            StageId::StorageHashing,
            StageId::MerkleExecute,
            StageId::TransactionLookup,
            StageId::IndexAccountHistory,
            StageId::IndexStorageHistory,
            StageId::Finish,
        ] {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            assert_eq!(
                checkpoint.map(|c| c.block_number),
                Some(num_blocks),
                "{stage_id} checkpoint should be at block {num_blocks}"
            );
        }

        // Verify the counter contract's storage was updated
        // After num_blocks blocks with 1 increment each, slot 0 should be num_blocks
        let state = provider.latest();
        let counter_storage = state.storage(CONTRACT_ADDRESS, B256::ZERO)?;
        assert_eq!(
            counter_storage,
            Some(U256::from(num_blocks)),
            "Counter storage slot 0 should be {num_blocks} after {num_blocks} increments"
        );
    }

    // Verify changesets are queryable before unwind
    // This validates that the #21561 fix works - unwind needs to read changesets from the correct
    // source
    assert_changesets_queryable(&pipeline_provider_factory, 1..=num_blocks)?;

    // Unwind to unwind_target
    pipeline.unwind(unwind_target, None)?;

    // Verify unwind
    {
        let provider = pipeline_provider_factory.provider()?;
        for stage_id in [
            StageId::Headers,
            StageId::Bodies,
            StageId::SenderRecovery,
            StageId::Execution,
            StageId::AccountHashing,
            StageId::StorageHashing,
            StageId::MerkleExecute,
            StageId::TransactionLookup,
            StageId::IndexAccountHistory,
            StageId::IndexStorageHistory,
        ] {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            if let Some(cp) = checkpoint {
                assert!(
                    cp.block_number <= unwind_target,
                    "{stage_id} checkpoint {} should be <= {unwind_target}",
                    cp.block_number
                );
            }
        }

        let state = provider.latest();
        let counter_storage = state.storage(CONTRACT_ADDRESS, B256::ZERO)?;
        assert_eq!(
            counter_storage,
            Some(U256::from(unwind_target)),
            "Counter storage slot 0 should be {unwind_target} after unwinding to block {unwind_target}"
        );
    }

    // Re-sync: build a new pipeline starting from unwind_target and sync back to num_blocks
    let resync_file_client = create_file_client_from_blocks(blocks_clone);
    let resync_consensus = NoopConsensus::arc();
    let resync_stages_config = StageConfig::default();

    let unwind_head = pipeline_provider_factory
        .sealed_header(unwind_target)?
        .expect("unwind target header should exist");

    let resync_runtime = reth_tasks::Runtime::test();

    let mut resync_header_downloader =
        ReverseHeadersDownloaderBuilder::new(resync_stages_config.headers)
            .build(resync_file_client.clone(), resync_consensus.clone())
            .into_task_with(&resync_runtime);
    resync_header_downloader.update_local_head(unwind_head);
    resync_header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut resync_body_downloader = BodiesDownloaderBuilder::new(resync_stages_config.bodies)
        .build(resync_file_client, resync_consensus, pipeline_provider_factory.clone())
        .into_task_with(&resync_runtime);
    resync_body_downloader
        .set_download_range(unwind_target + 1..=max_block)
        .expect("set download range");

    let resync_pipeline = build_pipeline(
        pipeline_provider_factory.clone(),
        resync_header_downloader,
        resync_body_downloader,
        max_block,
        tip,
    );

    let (_resync_pipeline, resync_result) = resync_pipeline.run_as_fut(None).await;
    resync_result?;

    // Verify re-sync
    {
        let provider = pipeline_provider_factory.provider()?;
        let last_block = provider.last_block_number()?;
        assert_eq!(last_block, num_blocks, "should have re-synced to {num_blocks} blocks");

        for stage_id in [
            StageId::Headers,
            StageId::Bodies,
            StageId::SenderRecovery,
            StageId::Execution,
            StageId::AccountHashing,
            StageId::StorageHashing,
            StageId::MerkleExecute,
            StageId::TransactionLookup,
            StageId::IndexAccountHistory,
            StageId::IndexStorageHistory,
            StageId::Finish,
        ] {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            assert_eq!(
                checkpoint.map(|c| c.block_number),
                Some(num_blocks),
                "{stage_id} checkpoint should be at block {num_blocks} after re-sync"
            );
        }

        let state = provider.latest();
        let counter_storage = state.storage(CONTRACT_ADDRESS, B256::ZERO)?;
        assert_eq!(
            counter_storage,
            Some(U256::from(num_blocks)),
            "Counter storage slot 0 should be {num_blocks} after re-sync"
        );
    }

    Ok(())
}

/// Tests pipeline with ALL stages enabled using both ETH transfers and contract storage changes.
///
/// This test:
/// 1. Pre-funds a signer account and deploys a Counter contract in genesis
/// 2. Each block contains two transactions:
///    - ETH transfer to a recipient (account state changes)
///    - Counter `increment()` call (storage state changes)
/// 3. Runs the full pipeline with ALL stages enabled
/// 4. Forward syncs to block 5, unwinds to block 2, then re-syncs to block 5
///
/// This exercises both account and storage hashing/history stages.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline() -> eyre::Result<()> {
    run_pipeline_forward_and_unwind(None, 5, 2).await
}

/// Same as [`test_pipeline`] but runs with v2 storage settings (`use_hashed_state=true`,
/// `is_v2()=true`, etc.).
///
/// In v2 mode:
/// - The execution stage writes directly to `HashedAccounts`/`HashedStorages`
/// - `AccountHashingStage` and `StorageHashingStage` are no-ops during forward execution
/// - Changesets are stored in static files with pre-hashed storage keys
/// - Unwind must still revert hashed state via the hashing stages before `MerkleUnwind` validates
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_v2() -> eyre::Result<()> {
    run_pipeline_forward_and_unwind(Some(StorageSettings::v2()), 5, 2).await
}
