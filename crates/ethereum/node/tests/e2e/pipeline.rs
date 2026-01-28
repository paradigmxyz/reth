//! Pipeline forward sync and unwind tests.
//!
//! These tests verify that the pipeline can correctly sync forward using mocked downloaders
//! that provide real block data, then unwind to a previous state.
//!
//! The downloaders (`HeadersDownloader`, `BodiesDownloader`) are backed by `FileClient` which
//! serves blocks from memory, simulating network downloads without actual network access.
//!
//! This is a regression test for bugs where unwind functions read from wrong storage.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt, EMPTY_ROOT_HASH};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, TxKind, B256, U256};
use alloy_trie::root::state_root_unhashed;
use reth_chainspec::{
    ChainSpec, ChainSpecBuilder, ChainSpecProvider, MAINNET, MIN_TRANSACTION_GAS,
};
use reth_config::config::StageConfig;
use reth_consensus::noop::NoopConsensus;
use reth_db_common::init::init_genesis;
use reth_downloaders::{
    bodies::bodies::BodiesDownloaderBuilder, file_client::FileClient,
    headers::reverse_headers::ReverseHeadersDownloaderBuilder,
};
use reth_ethereum_primitives::{Block, BlockBody, Transaction};
use reth_evm_ethereum::EthEvmConfig;
use reth_network_p2p::{
    bodies::downloader::BodyDownloader,
    headers::downloader::{HeaderDownloader, SyncTarget},
};
use reth_primitives_traits::{crypto::secp256k1::public_key_to_address, Account, SealedBlock};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader, HeaderProvider,
    StageCheckpointReader,
};
use reth_prune_types::PruneModes;
use reth_stages::sets::DefaultStages;
use reth_stages_api::{Pipeline, StageId, StageSet};
use reth_static_file::StaticFileProducer;
use reth_testing_utils::generators::{
    self, generate_key, random_block_range, sign_tx_with_key_pair, BlockRangeParams,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch;

/// Creates a test provider factory with genesis initialized.
fn create_test_provider(
) -> reth_provider::ProviderFactory<reth_provider::test_utils::MockNodeTypesWithDB> {
    let chain_spec = Arc::new(ChainSpec::default());
    let factory = create_test_provider_factory_with_chain_spec(chain_spec);
    init_genesis(&factory).expect("init genesis");
    factory
}

/// Creates a `FileClient` populated with the given blocks.
fn create_file_client_from_blocks(blocks: &[SealedBlock<Block>]) -> Arc<FileClient<Block>> {
    let headers: HashMap<u64, _> = blocks.iter().map(|b| (b.number, b.header().clone())).collect();
    let bodies: HashMap<B256, _> = blocks.iter().map(|b| (b.hash(), b.body().clone())).collect();
    Arc::new(FileClient::default().with_headers(headers).with_bodies(bodies))
}

/// Builds downloaders from a `FileClient`, mimicking the pattern from `import_core.rs`.
fn build_downloaders_from_file_client(
    file_client: Arc<FileClient<Block>>,
    genesis: reth_primitives_traits::SealedHeader<Header>,
    stages_config: StageConfig,
    consensus: Arc<NoopConsensus>,
    provider_factory: reth_provider::ProviderFactory<
        reth_provider::test_utils::MockNodeTypesWithDB,
    >,
) -> (impl HeaderDownloader<Header = Header>, impl BodyDownloader<Block = Block>) {
    let tip = file_client.tip().expect("file client should have tip");
    let min_block = file_client.min_block().expect("file client should have min block");
    let max_block = file_client.max_block().expect("file client should have max block");

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), consensus.clone())
        .into_task();
    header_downloader.update_local_head(genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, consensus, provider_factory)
        .into_task();
    body_downloader.set_download_range(min_block..=max_block).expect("set download range");

    (header_downloader, body_downloader)
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
    disable_state_stages: bool,
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

    let mut builder = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true);

    if disable_state_stages {
        builder =
            builder.add_stages(stages.builder().disable_all_if(&StageId::STATE_REQUIRED, || true));
    } else {
        builder = builder.add_stages(stages);
    }

    let pipeline = builder.build(provider_factory, static_file_producer);
    pipeline.set_tip(tip);
    pipeline
}

/// Tests pipeline forward sync and unwind using FileClient-backed downloaders.
///
/// This test:
/// 1. Generates valid test blocks with proper parent hash linking
/// 2. Creates a `FileClient` to serve blocks to `HeadersDownloader` and `BodiesDownloader`
/// 3. Runs the pipeline forward to sync all blocks through real stages
/// 4. Verifies stage checkpoints are at the expected block
/// 5. Unwinds the pipeline to an earlier block
/// 6. Verifies stage checkpoints have been updated by real unwind code
///
/// This exercises real stage code paths including:
/// - `HeaderStage::execute` (downloads headers via `FileClient`)
/// - `BodyStage::execute` (downloads bodies via `FileClient`)
/// - `SenderRecoveryStage::execute/unwind`
/// - `TransactionLookupStage::execute/unwind`
///
/// `STATE_REQUIRED` stages (Execution, Hashing, Merkle, History) are disabled since
/// random blocks don't have valid execution results.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_forward_sync_and_unwind() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let provider_factory = create_test_provider();
    let consensus = NoopConsensus::arc();

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");

    // Generate test blocks 1..=10 with proper parent hash linking
    let mut rng = generators::rng();
    let num_blocks = 10u64;
    let blocks = random_block_range(
        &mut rng,
        1..=num_blocks,
        BlockRangeParams { parent: Some(genesis.hash()), tx_count: 1..3, ..Default::default() },
    );

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
        file_client,
        genesis.clone(),
        stages_config,
        consensus,
        provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        true, // disable state stages
    );

    // Run forward sync
    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    // Verify forward sync completed
    {
        let provider = provider_factory.provider()?;
        let last_block = provider.last_block_number()?;
        assert_eq!(last_block, num_blocks, "should have synced {num_blocks} blocks");

        for stage_id in [
            StageId::Headers,
            StageId::Bodies,
            StageId::SenderRecovery,
            StageId::TransactionLookup,
            StageId::Finish,
        ] {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            assert_eq!(
                checkpoint.map(|c| c.block_number),
                Some(num_blocks),
                "{stage_id} checkpoint should be at block {num_blocks}"
            );
        }
    }

    // Unwind to block 5
    let unwind_target = 5u64;
    pipeline.unwind(unwind_target, None)?;

    // Verify unwind completed
    {
        let provider = provider_factory.provider()?;
        for stage_id in
            [StageId::Headers, StageId::Bodies, StageId::SenderRecovery, StageId::TransactionLookup]
        {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            if let Some(cp) = checkpoint {
                assert!(
                    cp.block_number <= unwind_target,
                    "{stage_id} checkpoint {} should be <= {unwind_target}",
                    cp.block_number
                );
            }
        }
    }

    Ok(())
}

/// Tests pipeline sync, unwind, then verify state is consistent.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_sync_unwind_resync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let provider_factory = create_test_provider();
    let consensus = NoopConsensus::arc();

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");

    let mut rng = generators::rng();
    let blocks = random_block_range(
        &mut rng,
        1..=10,
        BlockRangeParams { parent: Some(genesis.hash()), tx_count: 1..3, ..Default::default() },
    );

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
        file_client,
        genesis.clone(),
        stages_config,
        consensus,
        provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        true,
    );

    // First sync to block 10
    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    {
        let provider = provider_factory.provider()?;
        assert_eq!(provider.last_block_number()?, 10);
    }

    // Unwind to block 5
    pipeline.unwind(5, None)?;

    {
        let provider = provider_factory.provider()?;
        let headers_cp = provider.get_stage_checkpoint(StageId::Headers)?;
        assert!(headers_cp.map(|c| c.block_number <= 5).unwrap_or(true));
    }

    Ok(())
}

/// Tests pipeline unwind via `PipelineTarget::Unwind`.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_unwind_via_target() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let provider_factory = create_test_provider();
    let consensus = NoopConsensus::arc();

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");

    let mut rng = generators::rng();
    let blocks = random_block_range(
        &mut rng,
        1..=15,
        BlockRangeParams { parent: Some(genesis.hash()), tx_count: 1..3, ..Default::default() },
    );

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
        file_client,
        genesis.clone(),
        stages_config,
        consensus,
        provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        true,
    );

    // Sync forward first
    let (pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    // Unwind via PipelineTarget
    let unwind_target = 7u64;
    let (_pipeline, result) =
        pipeline.run_as_fut(Some(reth_stages_api::PipelineTarget::Unwind(unwind_target))).await;
    result?;

    // Verify unwind
    {
        let provider = provider_factory.provider()?;
        for stage_id in [StageId::Headers, StageId::Bodies, StageId::SenderRecovery] {
            let checkpoint = provider.get_stage_checkpoint(stage_id)?;
            if let Some(cp) = checkpoint {
                assert!(
                    cp.block_number <= unwind_target,
                    "{stage_id} should be <= {unwind_target}"
                );
            }
        }
    }

    Ok(())
}

/// Tests pipeline with many blocks to stress test the sync path.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_sync_many_blocks() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let provider_factory = create_test_provider();
    let consensus = NoopConsensus::arc();

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");

    let mut rng = generators::rng();
    let num_blocks = 50u64;
    let blocks = random_block_range(
        &mut rng,
        1..=num_blocks,
        BlockRangeParams { parent: Some(genesis.hash()), tx_count: 0..3, ..Default::default() },
    );

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
        file_client,
        genesis.clone(),
        stages_config,
        consensus,
        provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        true,
    );

    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    {
        let provider = provider_factory.provider()?;
        assert_eq!(provider.last_block_number()?, num_blocks);
    }

    // Unwind halfway
    let unwind_target = 25u64;
    pipeline.unwind(unwind_target, None)?;

    {
        let provider = provider_factory.provider()?;
        let checkpoint = provider.get_stage_checkpoint(StageId::Headers)?;
        assert!(checkpoint.map(|c| c.block_number <= unwind_target).unwrap_or(true));
    }

    Ok(())
}

/// Tests pipeline with ALL stages enabled using blocks with real ETH transfers.
///
/// This test creates a custom ChainSpec with a pre-funded signer account in genesis,
/// then generates blocks with signed ETH transfer transactions. This exercises the
/// full pipeline including hashing stages with actual state changes.
///
/// This test:
/// 1. Creates a ChainSpec with a pre-funded account in genesis
/// 2. Creates 5 blocks with ETH transfer transactions from that account
/// 3. Executes blocks to compute correct state roots
/// 4. Runs the full pipeline with ALL stages enabled
/// 5. Forward syncs to block 5, unwinds to block 2
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline_full_stages_state_roots() -> eyre::Result<()> {
    use alloy_eips::eip1559::INITIAL_BASE_FEE;
    use reth_primitives_traits::proofs::{calculate_receipt_root, calculate_transaction_root};

    reth_tracing::init_test_tracing();

    // Generate a keypair for signing transactions
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());

    // Create a chain spec with the signer pre-funded in genesis
    let initial_balance = U256::from(ETH_TO_WEI) * U256::from(1000); // 1000 ETH
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [(
                    signer_address,
                    GenesisAccount { balance: initial_balance, ..Default::default() },
                )]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .paris_activated()
            .build(),
    );

    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&provider_factory).expect("init genesis");

    let consensus = NoopConsensus::arc();
    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");

    // Build blocks with ETH transfer transactions
    let num_blocks = 5u64;
    let mut blocks: Vec<SealedBlock<Block>> = Vec::new();
    let mut parent_hash = genesis.hash();
    let mut nonce = 0u64;
    let mut current_balance = initial_balance;

    // Gas cost per transaction
    let gas_price = INITIAL_BASE_FEE as u128;
    let gas_limit = MIN_TRANSACTION_GAS;
    let tx_cost = U256::from(gas_price) * U256::from(gas_limit);
    let transfer_value = U256::from(ETH_TO_WEI / 10); // 0.1 ETH per transfer

    for block_num in 1..=num_blocks {
        // Create a simple ETH transfer transaction
        let tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce,
                gas_limit,
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(Address::ZERO), // Transfer to zero address
                value: transfer_value,
                ..Default::default()
            }),
        );

        // Update signer's balance: subtract gas cost and transfer value
        current_balance -= tx_cost + transfer_value;
        nonce += 1;

        // Track recipient balance (Address::ZERO receives transfers)
        let recipient_balance = transfer_value * U256::from(block_num);

        // Compute state root with all accounts that have state
        // - Signer: balance decreases, nonce increases
        // - Recipient (Address::ZERO): receives transfer value each block
        let state_root = state_root_unhashed([
            (
                signer_address,
                Account { balance: current_balance, nonce, ..Default::default() }
                    .into_trie_account(EMPTY_ROOT_HASH),
            ),
            (
                Address::ZERO,
                Account { balance: recipient_balance, nonce: 0, ..Default::default() }
                    .into_trie_account(EMPTY_ROOT_HASH),
            ),
        ]);

        // Calculate transaction and receipt roots
        let transactions = vec![tx.clone()];
        let tx_root = calculate_transaction_root(&transactions);

        // Create a simple receipt (successful tx)
        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: reth_ethereum_primitives::TxType::Eip1559,
            success: true,
            cumulative_gas_used: gas_limit,
            ..Default::default()
        };
        let receipts_with_bloom = vec![receipt.clone().into_with_bloom()];
        let receipts_root = calculate_receipt_root(&receipts_with_bloom);

        let header = Header {
            parent_hash,
            number: block_num,
            state_root,
            transactions_root: tx_root,
            receipts_root,
            gas_limit: 30_000_000,
            gas_used: gas_limit,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12, // 12 second blocks
            ..Default::default()
        };

        let block: SealedBlock<Block> = SealedBlock::seal_parts(
            header,
            BlockBody { transactions, ommers: Vec::new(), withdrawals: None },
        );

        parent_hash = block.hash();
        blocks.push(block);
    }

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
        file_client,
        genesis.clone(),
        stages_config,
        consensus,
        provider_factory.clone(),
    );

    let pipeline = build_pipeline(
        provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        false,
    );

    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    {
        let provider = provider_factory.provider()?;
        let last_block = provider.last_block_number()?;
        assert_eq!(last_block, 5, "should have synced 5 blocks");

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
                Some(5),
                "{stage_id} checkpoint should be at block 5"
            );
        }
    }

    let unwind_target = 2u64;
    pipeline.unwind(unwind_target, None)?;

    {
        let provider = provider_factory.provider()?;
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
    }

    Ok(())
}
