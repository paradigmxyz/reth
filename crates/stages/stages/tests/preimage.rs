//! Preimage-specific pipeline tests for storage v2 selfdestruct behavior around Cancun.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{keccak256, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{
    ChainSpecBuilder, ChainSpecProvider, EthereumHardfork, ForkCondition, MAINNET,
};
use reth_config::config::StageConfig;
use reth_consensus::noop::NoopConsensus;
use reth_db_common::init::{init_genesis, init_genesis_with_settings};
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder, file_client::FileClient,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_ethereum_primitives::{Block, BlockBody, Transaction, TransactionSigned};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_evm_ethereum::EthEvmConfig;
use reth_libmdbx::{Environment, EnvironmentFlags, Mode};
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
    DatabaseProviderFactory, HeaderProvider, OriginalValuesKnown, StateWriter, StoragePath,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages::{
    sets::{ExecutionStages, HashingStages, OnlineStages},
    stages::FinishStage,
};
use reth_stages_api::{Pipeline, StageSet};
use reth_static_file::StaticFileProducer;
use reth_storage_api::{StorageChangeSetReader, StorageSettings, StorageSettingsCache};
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::watch;

type TestProviderFactory =
    reth_provider::ProviderFactory<reth_provider::test_utils::MockNodeTypesWithDB>;

/// Verifies v2 selfdestruct handling across a pre-/post-Cancun boundary.
///
/// Test flow:
/// 1. Run block 1 (pre-Cancun) and assert the `preimage/` MDBX directory exists and contains
///    `keccak(slot) -> slot` rows for the two written storage slots.
/// 2. Run block 2 (pre-Cancun selfdestruct) and assert storage changesets for the destroyed account
///    contain exactly those two slots as **plain** keys with the expected prior values (`0x2a`,
///    `0x99`).
/// 3. Run block 3 (post-Cancun) and assert `preimage/` is removed, since this auxiliary DB is no
///    longer needed after Cancun semantics are active.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_v2_selfdestruct_changesets_use_plain_slots() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Build a 3-block scenario:
    // - block 1/2 are pre-Cancun (selfdestruct still clears storage)
    // - block 3 is post-Cancun (no storage-destroy semantics; preimage DB should be cleaned up)
    let scenario = setup_selfdestruct_scenario()?;

    let (pipeline_provider_factory, pipeline_genesis) =
        init_v2_pipeline_provider_factory(scenario.chain_spec.clone())?;

    run_pipeline_range(
        pipeline_provider_factory.clone(),
        create_file_client_from_blocks(vec![scenario.blocks[0].clone()]),
        pipeline_genesis,
        1..=1,
        1,
    )
    .await?;

    // Phase 1 (pre-Cancun): preimage DB should be created and contain slot preimages.
    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, 1, "pipeline should sync block 1");
    assert!(provider.cached_storage_settings().storage_v2, "test requires storage.v2 mode");

    let preimage_path = provider.storage_path().join("preimage");
    let expected_slots = scenario.expected_slots;
    assert!(preimage_path.exists(), "preimage dir should exist after first pre-Cancun run");
    assert_preimage_rows(&preimage_path, &expected_slots)?;

    let local_head =
        pipeline_provider_factory.sealed_header(1)?.expect("block 1 header should exist");
    run_pipeline_range(
        pipeline_provider_factory.clone(),
        create_file_client_from_blocks(vec![
            scenario.blocks[0].clone(),
            scenario.blocks[1].clone(),
        ]),
        local_head,
        2..=2,
        2,
    )
    .await?;

    // Phase 2 (pre-Cancun selfdestruct): changeset keys for destroyed account must be plain slots.
    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, 2, "pipeline should sync block 2");
    assert!(preimage_path.exists(), "preimage dir should still exist after second pre-Cancun run");
    assert_preimage_rows(&preimage_path, &expected_slots)?;
    assert_destroyed_changeset_entries(&provider, scenario.selfdestruct_contract)?;

    let third_local_head =
        pipeline_provider_factory.sealed_header(2)?.expect("block 2 header should exist");
    run_pipeline_range(
        pipeline_provider_factory.clone(),
        create_file_client_from_blocks(scenario.blocks),
        third_local_head,
        3..=3,
        3,
    )
    .await?;

    // Phase 3 (post-Cancun): execution path removes the now-unneeded preimage DB directory.
    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, 3, "pipeline should sync block 3");
    assert!(!preimage_path.exists(), "preimage dir should be removed after post-Cancun execution");

    Ok(())
}

/// Regression coverage for single execution-batch behavior:
/// blocks 1 and 2 are executed together (`1..=2`) where block 1 writes storage and block 2
/// selfdestructs the account. The resulting block-2 wipe changeset must still contain plain keys.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_v2_single_batch_write_then_selfdestruct_changesets_plain_slots(
) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let scenario = setup_selfdestruct_scenario()?;
    let (pipeline_provider_factory, pipeline_genesis) =
        init_v2_pipeline_provider_factory(scenario.chain_spec.clone())?;

    run_pipeline_range(
        pipeline_provider_factory.clone(),
        create_file_client_from_blocks(vec![
            scenario.blocks[0].clone(),
            scenario.blocks[1].clone(),
        ]),
        pipeline_genesis,
        1..=2,
        2,
    )
    .await?;

    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, 2, "pipeline should sync blocks 1..=2");
    assert_destroyed_changeset_entries(&provider, scenario.selfdestruct_contract)?;

    Ok(())
}

/// Covers the edge case where a slot appears in intermediate block reverts but not in the final
/// bundle state, then gets wiped by a later selfdestruct in the same execution batch.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_v2_single_batch_reverted_slot_then_selfdestruct_changesets_plain_slots(
) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let scenario = setup_reverted_slot_selfdestruct_scenario()?;
    let (pipeline_provider_factory, pipeline_genesis) =
        init_v2_pipeline_provider_factory(scenario.chain_spec.clone())?;

    run_pipeline_range(
        pipeline_provider_factory.clone(),
        create_file_client_from_blocks(scenario.blocks),
        pipeline_genesis,
        1..=3,
        3,
    )
    .await?;

    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, 3, "pipeline should sync blocks 1..=3");
    assert_destroyed_changeset_entries_in_block(
        &provider,
        3,
        scenario.selfdestruct_contract,
        &[scenario.expected_slot],
    )?;

    Ok(())
}

struct SelfdestructScenario {
    chain_spec: Arc<reth_chainspec::ChainSpec>,
    blocks: Vec<SealedBlock<Block>>,
    selfdestruct_contract: Address,
    expected_slots: [B256; 2],
}

fn setup_selfdestruct_scenario() -> eyre::Result<SelfdestructScenario> {
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());
    let beneficiary = Address::new([0x77; 20]);
    let selfdestruct_contract = Address::new([0x66; 20]);
    let chain_spec =
        build_selfdestruct_chain_spec(signer_address, beneficiary, selfdestruct_contract);
    let blocks = {
        // Build blocks via direct execution first, so each header has a valid state root.
        // The pipeline test then replays these exact blocks in phase-separated ranges.
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory).expect("init genesis");

        let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let mut blocks = Vec::new();
        let mut parent_hash = genesis.hash();
        let gas_price = INITIAL_BASE_FEE as u128;

        for (block_num, timestamp, nonce, input, to, gas_limit, value) in [
            (
                1_u64,
                12_u64,
                0_u64,
                Bytes::new(),
                TxKind::Call(selfdestruct_contract),
                100_000_u64,
                U256::ZERO,
            ),
            (
                2_u64,
                24_u64,
                1_u64,
                Bytes::from(vec![0x01]),
                TxKind::Call(selfdestruct_contract),
                100_000_u64,
                U256::ZERO,
            ),
            (
                3_u64,
                36_u64,
                2_u64,
                Bytes::new(),
                TxKind::Call(beneficiary),
                21_000_u64,
                U256::from(1),
            ),
        ] {
            // Block behavior by timestamp:
            // - block 1 (ts=12): writes two storage slots
            // - block 2 (ts=24): triggers SELFDESTRUCT (pre-Cancun semantics)
            // - block 3 (ts=36): post-Cancun no-op transfer path
            let tx = sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip1559(TxEip1559 {
                    chain_id: chain_spec.chain.id(),
                    nonce,
                    gas_limit,
                    max_fee_per_gas: gas_price,
                    max_priority_fee_per_gas: 0,
                    to,
                    value,
                    input,
                    ..Default::default()
                }),
            );
            let block = execute_and_commit_block(
                &provider_factory,
                &evm_config,
                signer_address,
                parent_hash,
                block_num,
                timestamp,
                vec![tx],
            )?;
            parent_hash = block.hash();
            blocks.push(block);
        }

        blocks
    };

    Ok(SelfdestructScenario {
        chain_spec,
        blocks,
        selfdestruct_contract,
        expected_slots: expected_destroyed_slots(),
    })
}

struct RevertedSlotSelfdestructScenario {
    chain_spec: Arc<reth_chainspec::ChainSpec>,
    blocks: Vec<SealedBlock<Block>>,
    selfdestruct_contract: Address,
    expected_slot: (B256, U256),
}

fn setup_reverted_slot_selfdestruct_scenario() -> eyre::Result<RevertedSlotSelfdestructScenario> {
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());
    let beneficiary = Address::new([0x55; 20]);
    let selfdestruct_contract = Address::new([0x33; 20]);
    let slot = B256::with_last_byte(0x03);
    let original_value = B256::with_last_byte(0x07);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [
                    (
                        signer_address,
                        GenesisAccount {
                            balance: U256::from(ETH_TO_WEI) * U256::from(1000),
                            ..Default::default()
                        },
                    ),
                    (
                        selfdestruct_contract,
                        GenesisAccount {
                            code: Some(write_restore_or_selfdestruct_runtime_code(beneficiary)),
                            storage: Some(BTreeMap::from([(slot, original_value)])),
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .shanghai_activated()
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(30))
            .build(),
    );

    let blocks = {
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory).expect("init genesis");

        let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let mut blocks = Vec::new();
        let mut parent_hash = genesis.hash();
        let gas_price = INITIAL_BASE_FEE as u128;

        for (block_num, timestamp, nonce, value) in [
            (1_u64, 12_u64, 0_u64, U256::ZERO),
            (2_u64, 18_u64, 1_u64, U256::from(1_u64)),
            (3_u64, 24_u64, 2_u64, U256::from(2_u64)),
        ] {
            let tx = sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip1559(TxEip1559 {
                    chain_id: chain_spec.chain.id(),
                    nonce,
                    gas_limit: 120_000,
                    max_fee_per_gas: gas_price,
                    max_priority_fee_per_gas: 0,
                    to: TxKind::Call(selfdestruct_contract),
                    value,
                    input: Bytes::new(),
                    ..Default::default()
                }),
            );
            let block = execute_and_commit_block(
                &provider_factory,
                &evm_config,
                signer_address,
                parent_hash,
                block_num,
                timestamp,
                vec![tx],
            )?;
            parent_hash = block.hash();
            blocks.push(block);
        }

        blocks
    };

    Ok(RevertedSlotSelfdestructScenario {
        chain_spec,
        blocks,
        selfdestruct_contract,
        expected_slot: (slot, U256::from(0x07)),
    })
}

fn init_v2_pipeline_provider_factory(
    chain_spec: Arc<reth_chainspec::ChainSpec>,
) -> eyre::Result<(TestProviderFactory, reth_primitives_traits::SealedHeader<Header>)> {
    let pipeline_provider_factory = create_test_provider_factory_with_chain_spec(chain_spec);
    init_genesis_with_settings(&pipeline_provider_factory, StorageSettings::v2())?;
    pipeline_provider_factory.set_storage_settings_cache(StorageSettings::v2());
    let pipeline_genesis = pipeline_provider_factory
        .sealed_header(0)?
        .ok_or_else(|| eyre::eyre!("genesis should exist"))?;
    Ok((pipeline_provider_factory, pipeline_genesis))
}

fn execute_and_commit_block(
    provider_factory: &TestProviderFactory,
    evm_config: &EthEvmConfig,
    signer_address: Address,
    parent_hash: B256,
    block_num: u64,
    timestamp: u64,
    transactions: Vec<TransactionSigned>,
) -> eyre::Result<SealedBlock<Block>> {
    let tx_root = calculate_transaction_root(&transactions);
    let temp_header = build_execution_header(parent_hash, block_num, timestamp);
    let provider = provider_factory.database_provider_rw()?;
    let block_with_senders = RecoveredBlock::new_unhashed(
        Block::new(
            temp_header.clone(),
            BlockBody { transactions: transactions.clone(), ommers: Vec::new(), withdrawals: None },
        ),
        vec![signer_address; transactions.len()],
    );

    let output = {
        let state_provider = provider.latest();
        let db = StateProviderDatabase::new(&*state_provider);
        let executor = evm_config.batch_executor(db);
        executor.execute(&block_with_senders)?
    };

    let gas_used = output.gas_used;
    let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
    let (state_root, _trie_updates) = StateRoot::overlay_root_with_updates(
        provider.tx_ref(),
        &hashed_state.clone().into_sorted(),
    )?;

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
        timestamp,
        parent_beacon_block_root: (timestamp >= 30).then_some(B256::ZERO),
        blob_gas_used: (timestamp >= 30).then_some(0),
        excess_blob_gas: (timestamp >= 30).then_some(0),
        ..Default::default()
    };

    let block: SealedBlock<Block> = SealedBlock::seal_parts(
        header,
        BlockBody { transactions, ommers: Vec::new(), withdrawals: None },
    );

    let plain_state = output.state.to_plain_state(OriginalValuesKnown::Yes);
    provider.write_state_changes(plain_state)?;
    provider.write_hashed_state(&hashed_state.into_sorted())?;
    provider.commit()?;

    Ok(block)
}

fn build_selfdestruct_chain_spec(
    signer_address: Address,
    beneficiary: Address,
    selfdestruct_contract: Address,
) -> Arc<reth_chainspec::ChainSpec> {
    let initial_balance = U256::from(ETH_TO_WEI) * U256::from(1000);

    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [
                    (
                        signer_address,
                        GenesisAccount { balance: initial_balance, ..Default::default() },
                    ),
                    (
                        selfdestruct_contract,
                        GenesisAccount {
                            code: Some(write_or_selfdestruct_runtime_code(beneficiary)),
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .shanghai_activated()
            .with_fork(EthereumHardfork::Cancun, ForkCondition::Timestamp(30))
            .build(),
    )
}

fn build_execution_header(parent_hash: B256, number: u64, timestamp: u64) -> Header {
    Header {
        parent_hash,
        number,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(INITIAL_BASE_FEE),
        timestamp,
        parent_beacon_block_root: (timestamp >= 30).then_some(B256::ZERO),
        blob_gas_used: (timestamp >= 30).then_some(0),
        excess_blob_gas: (timestamp >= 30).then_some(0),
        ..Default::default()
    }
}

const fn expected_destroyed_slots() -> [B256; 2] {
    [B256::with_last_byte(0x01), B256::with_last_byte(0x02)]
}

fn assert_destroyed_changeset_entries<P>(
    provider: &P,
    selfdestruct_contract: Address,
) -> eyre::Result<()>
where
    P: StorageChangeSetReader,
{
    let expected = [
        (B256::with_last_byte(0x01), U256::from(0x2a)),
        (B256::with_last_byte(0x02), U256::from(0x99)),
    ];
    assert_destroyed_changeset_entries_in_block(provider, 2, selfdestruct_contract, &expected)
}

fn assert_destroyed_changeset_entries_in_block<P>(
    provider: &P,
    block: u64,
    selfdestruct_contract: Address,
    expected: &[(B256, U256)],
) -> eyre::Result<()>
where
    P: StorageChangeSetReader,
{
    let storage_changesets = provider.storage_changesets_range(block..=block)?;
    let destroyed_entries: Vec<_> = storage_changesets
        .into_iter()
        .filter_map(|(key, entry)| {
            (key.address() == selfdestruct_contract).then_some((entry.key, entry.value))
        })
        .collect();

    assert_eq!(
        destroyed_entries.len(),
        expected.len(),
        "expected exactly {} storage changeset entries for destroyed account at block {}",
        expected.len(),
        block
    );

    for (slot, _) in &destroyed_entries {
        assert_ne!(*slot, keccak256(*slot), "storage changeset key should be plain (not hashed)");
    }

    for pair in expected {
        assert!(
            destroyed_entries.contains(pair),
            "missing expected storage changeset entry for destroyed account: {:?}",
            pair
        );
    }

    Ok(())
}

fn create_file_client_from_blocks(blocks: Vec<SealedBlock<Block>>) -> Arc<FileClient<Block>> {
    Arc::new(FileClient::from_blocks(blocks))
}

fn build_pipeline_without_history<H, B>(
    provider_factory: TestProviderFactory,
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

    let stages = OnlineStages::new(
        provider_factory.clone(),
        tip_rx,
        header_downloader,
        body_downloader,
        stages_config.clone(),
        None,
    )
    .builder()
    .add_set(ExecutionStages::new(
        evm_config,
        consensus,
        stages_config,
        PruneModes::default().sender_recovery,
    ))
    .add_set(HashingStages::default())
    .add_stage(FinishStage::default());

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(stages)
        .build(provider_factory, static_file_producer);
    pipeline.set_tip(tip);
    pipeline
}

async fn run_pipeline_range(
    provider_factory: TestProviderFactory,
    file_client: Arc<FileClient<Block>>,
    local_head: reth_primitives_traits::SealedHeader<Header>,
    download_range: std::ops::RangeInclusive<u64>,
    max_block: u64,
) -> eyre::Result<()> {
    // Run a narrow range intentionally so the test can assert per-phase behavior.
    let tip = file_client.tip().expect("tip");
    let consensus = NoopConsensus::arc();
    let stages_config = StageConfig::default();
    let runtime = reth_tasks::Runtime::test();

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), consensus.clone())
        .into_task_with(&runtime);
    header_downloader.update_local_head(local_head);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, consensus, provider_factory.clone())
        .into_task_with(&runtime);
    body_downloader.set_download_range(download_range).expect("set download range");

    let pipeline = build_pipeline_without_history(
        provider_factory,
        header_downloader,
        body_downloader,
        max_block,
        tip,
    );
    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;
    Ok(())
}

/// Builds tiny runtime bytecode that branches on calldata:
/// - empty calldata: writes two known slots and stops
/// - non-empty calldata: selfdestructs to `beneficiary` and stops
///
/// The known slot/value pairs are used for deterministic assertions in changesets and preimages.
fn write_or_selfdestruct_runtime_code(beneficiary: Address) -> Bytes {
    let mut runtime = Vec::with_capacity(40);
    runtime.extend_from_slice(&[0x36, 0x15, 0x60, 0x1c, 0x57]); // CALLDATASIZE; ISZERO; PUSH1 0x1c; JUMPI
    runtime.push(0x73); // PUSH20
    runtime.extend_from_slice(beneficiary.as_slice());
    runtime.extend_from_slice(&[0xff, 0x00]); // SELFDESTRUCT; STOP
    runtime.push(0x5b); // JUMPDEST (0x1c)
    runtime.extend_from_slice(&[0x60, 0x2a, 0x60, 0x01, 0x55]); // SSTORE(1, 0x2a)
    runtime.extend_from_slice(&[0x60, 0x99, 0x60, 0x02, 0x55]); // SSTORE(2, 0x99)
    runtime.push(0x00); // STOP
    runtime.into()
}

/// Builds tiny runtime bytecode with three value-based paths:
/// - `msg.value == 0`: SSTORE(3, 0x2b)
/// - `msg.value == 1`: SSTORE(3, 0x07)
/// - `msg.value == 2`: SELFDESTRUCT to `beneficiary`
fn write_restore_or_selfdestruct_runtime_code(beneficiary: Address) -> Bytes {
    let mut runtime = Vec::with_capacity(64);
    runtime.extend_from_slice(&[0x34, 0x60, 0x02, 0x14, 0x60, 0x1b, 0x57]); // if callvalue == 2 jump selfdestruct
    runtime.extend_from_slice(&[0x34, 0x60, 0x01, 0x14, 0x60, 0x14, 0x57]); // if callvalue == 1 jump restore
    runtime.extend_from_slice(&[0x60, 0x2b, 0x60, 0x03, 0x55, 0x00]); // default: SSTORE(3, 0x2b); STOP
    runtime.push(0x5b); // JUMPDEST (0x14)
    runtime.extend_from_slice(&[0x60, 0x07, 0x60, 0x03, 0x55, 0x00]); // restore: SSTORE(3, 0x07); STOP
    runtime.push(0x5b); // JUMPDEST (0x1b)
    runtime.push(0x73); // PUSH20
    runtime.extend_from_slice(beneficiary.as_slice());
    runtime.extend_from_slice(&[0xff, 0x00]); // SELFDESTRUCT; STOP
    runtime.into()
}

fn assert_preimage_rows(preimage_path: &Path, slots: &[B256]) -> eyre::Result<()> {
    let mut builder = Environment::builder();
    builder.set_max_dbs(1);
    builder.set_flags(EnvironmentFlags {
        no_sub_dir: false,
        mode: Mode::ReadOnly,
        ..Default::default()
    });

    let env = builder.open(preimage_path)?;
    let tx = env.begin_ro_txn()?;
    let db = tx.open_db(None)?;

    for slot in slots {
        let hashed = keccak256(*slot);
        let found: Option<[u8; 32]> = tx.get(db.dbi(), hashed.as_slice())?;
        assert_eq!(
            found.map(B256::from),
            Some(*slot),
            "missing/invalid preimage row for slot {:?}",
            slot
        );
    }

    Ok(())
}
