//! Benchmark comparison: DefaultStages pipeline vs EngineStage pipeline.
//! Both use 15k blocks with the same chain spec and block content.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider, MAINNET};
use reth_config::config::{EtlConfig, StageConfig, TransactionLookupConfig};
use reth_config::config::SenderRecoveryConfig;
use reth_consensus::noop::NoopConsensus;
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
use reth_stages::stages::{BodyStage, EngineStage, HeaderStage, SenderRecoveryStage, TransactionLookupStage};
use reth_stages_api::{Pipeline, StageId};
use reth_static_file::StaticFileProducer;
use reth_storage_api::StateProvider;
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

const COUNTER_DEPLOYED_BYTECODE: Bytes = bytes!(
    "6080604052348015600e575f5ffd5b50600436106030575f3560e01c806306661abd146034578063d09de08a14604e575b5f5ffd5b603a6056565b604051604591906089565b60405180910390f35b6054605b565b005b5f5481565b60015f5f828254606a919060cd565b92505081905550565b5f819050919050565b6083816073565b82525050565b5f602082019050609a5f830184607c565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f60d5826073565b915060de836073565b925082820190508082111560f35760f260a0565b5b9291505056fea2646970667358221220576016d010ec2f4f83b992fb97d16efd1bc54110c97aa5d5cb47d20d3b39a35264736f6c634300081f0033"
);

const INCREMENT_SELECTOR: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];
const CONTRACT_ADDRESS: Address = Address::new([0x42; 20]);
const NUM_BLOCKS: u64 = 100_000;

/// Build all 15k test blocks. Returns (blocks, chain_spec).
fn build_test_blocks() -> eyre::Result<(
    Vec<SealedBlock<Block>>,
    Arc<reth_chainspec::ChainSpec>,
)> {
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());
    let recipient_address = Address::new([0x11; 20]);

    let initial_balance = U256::from(ETH_TO_WEI) * U256::from(1_000_000);
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [
                    (signer_address, GenesisAccount { balance: initial_balance, ..Default::default() }),
                    (CONTRACT_ADDRESS, GenesisAccount { code: Some(COUNTER_DEPLOYED_BYTECODE), ..Default::default() }),
                ].into(),
                ..MAINNET.genesis.clone()
            })
            .shanghai_activated()
            .build(),
    );

    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&provider_factory).expect("init genesis");
    let genesis = provider_factory.sealed_header(0)?.expect("genesis");
    let evm_config = EthEvmConfig::new(chain_spec.clone());

    let mut blocks = Vec::with_capacity(NUM_BLOCKS as usize);
    let mut parent_hash = genesis.hash();
    let gas_price = INITIAL_BASE_FEE as u128;
    let transfer_value = U256::from(1_000_000_000u64);

    for block_num in 1..=NUM_BLOCKS {
        let base_nonce = (block_num - 1) * 2;
        let eth_tx = sign_tx_with_key_pair(key_pair, Transaction::Eip1559(TxEip1559 {
            chain_id: chain_spec.chain.id(), nonce: base_nonce, gas_limit: 21_000,
            max_fee_per_gas: gas_price, max_priority_fee_per_gas: 0,
            to: TxKind::Call(recipient_address), value: transfer_value,
            input: Bytes::new(), ..Default::default()
        }));
        let counter_tx = sign_tx_with_key_pair(key_pair, Transaction::Eip1559(TxEip1559 {
            chain_id: chain_spec.chain.id(), nonce: base_nonce + 1, gas_limit: 100_000,
            max_fee_per_gas: gas_price, max_priority_fee_per_gas: 0,
            to: TxKind::Call(CONTRACT_ADDRESS), value: U256::ZERO,
            input: Bytes::from(INCREMENT_SELECTOR.to_vec()), ..Default::default()
        }));

        let transactions = vec![eth_tx, counter_tx];
        let tx_root = calculate_transaction_root(&transactions);
        let temp_header = Header {
            parent_hash, number: block_num, gas_limit: 30_000_000,
            base_fee_per_gas: Some(INITIAL_BASE_FEE), timestamp: block_num * 12,
            ..Default::default()
        };

        let provider = provider_factory.database_provider_rw()?;
        let block_with_senders = RecoveredBlock::new_unhashed(
            Block::new(temp_header.clone(), BlockBody { transactions: transactions.clone(), ommers: Vec::new(), withdrawals: None }),
            vec![signer_address, signer_address],
        );

        let output = {
            let sp = provider.latest();
            let db = StateProviderDatabase::new(&*sp);
            let executor = evm_config.batch_executor(db);
            executor.execute(&block_with_senders)?
        };

        let gas_used = output.gas_used;
        let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
        let (state_root, _) = StateRoot::overlay_root_with_updates(provider.tx_ref(), &hashed_state.clone().into_sorted())?;
        let receipts: Vec<_> = output.receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let receipts_root = calculate_receipt_root(&receipts);

        let header = Header {
            parent_hash, number: block_num, state_root, transactions_root: tx_root,
            receipts_root, gas_limit: 30_000_000, gas_used,
            base_fee_per_gas: Some(INITIAL_BASE_FEE), timestamp: block_num * 12,
            ..Default::default()
        };

        let block: SealedBlock<Block> = SealedBlock::seal_parts(header, BlockBody { transactions, ommers: Vec::new(), withdrawals: None });
        let plain_state = output.state.to_plain_state(OriginalValuesKnown::Yes);
        provider.write_state_changes(plain_state)?;
        provider.write_hashed_state(&hashed_state.into_sorted())?;
        provider.commit()?;
        parent_hash = block.hash();
        blocks.push(block);
    }

    Ok((blocks, chain_spec))
}

/// Run the DefaultStages pipeline (all stages) on 15k blocks.
#[tokio::test(flavor = "multi_thread")]
async fn bench_default_pipeline_15k() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    eprintln!("\n=== BUILDING 15k BLOCKS ===");
    let build_start = Instant::now();
    let (blocks, chain_spec) = build_test_blocks()?;
    eprintln!("Block generation: {:?}", build_start.elapsed());

    let pipeline_provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis = pipeline_provider_factory.sealed_header(0)?.expect("genesis");
    let pipeline_consensus = NoopConsensus::arc();

    let file_client = Arc::new(FileClient::from_blocks(blocks));
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");
    let stages_config = StageConfig::default();
    let evm_config = EthEvmConfig::new(pipeline_provider_factory.chain_spec());

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), pipeline_consensus.clone()).into_task();
    header_downloader.update_local_head(pipeline_genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, pipeline_consensus.clone(), pipeline_provider_factory.clone()).into_task();
    body_downloader.set_download_range(1..=max_block).expect("set range");

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer = StaticFileProducer::new(pipeline_provider_factory.clone(), PruneModes::default());

    let stages = DefaultStages::new(
        pipeline_provider_factory.clone(), tip_rx, pipeline_consensus,
        header_downloader, body_downloader, evm_config, stages_config, PruneModes::default(), None,
    );

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(stages)
        .build(pipeline_provider_factory.clone(), static_file_producer);
    pipeline.set_tip(tip);

    eprintln!("\n=== DEFAULT PIPELINE (ALL STAGES) — 15k blocks ===");
    let pipeline_start = Instant::now();
    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;
    let pipeline_elapsed = pipeline_start.elapsed();
    eprintln!("=== DEFAULT PIPELINE DONE: {:?} ===\n", pipeline_elapsed);

    // Verify
    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, NUM_BLOCKS);
    for stage_id in [StageId::Headers, StageId::Bodies, StageId::Execution, StageId::Finish] {
        let cp = provider.get_stage_checkpoint(stage_id)?;
        assert_eq!(cp.map(|c| c.block_number), Some(NUM_BLOCKS), "{stage_id} checkpoint");
    }
    let state = provider.latest();
    assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(NUM_BLOCKS)));

    Ok(())
}

/// Run the EngineStage pipeline (3 stages only) on 15k blocks.
#[tokio::test(flavor = "multi_thread")]
async fn bench_engine_pipeline_15k() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    eprintln!("\n=== BUILDING 15k BLOCKS ===");
    let build_start = Instant::now();
    let (blocks, chain_spec) = build_test_blocks()?;
    eprintln!("Block generation: {:?}", build_start.elapsed());

    let pipeline_provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis = pipeline_provider_factory.sealed_header(0)?.expect("genesis");
    let pipeline_consensus = NoopConsensus::arc();

    let file_client = Arc::new(FileClient::from_blocks(blocks));
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");
    let stages_config = StageConfig::default();
    let evm_config = EthEvmConfig::new(pipeline_provider_factory.chain_spec());

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), pipeline_consensus.clone()).into_task();
    header_downloader.update_local_head(pipeline_genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, pipeline_consensus, pipeline_provider_factory.clone()).into_task();
    body_downloader.set_download_range(1..=max_block).expect("set range");

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer = StaticFileProducer::new(pipeline_provider_factory.clone(), PruneModes::default());

    let engine_stage = EngineStage::with_defaults(evm_config, pipeline_provider_factory.clone());

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stage(HeaderStage::new(
            pipeline_provider_factory.clone(), header_downloader, tip_rx, stages_config.etl,
        ))
        .add_stage(BodyStage::new(body_downloader))
        .add_stage(SenderRecoveryStage::new(SenderRecoveryConfig::default(), None))
        .add_stage(TransactionLookupStage::new(TransactionLookupConfig::default(), EtlConfig::default(), None))
        .add_stage(engine_stage)
        .build(pipeline_provider_factory.clone(), static_file_producer);
    pipeline.set_tip(tip);

    eprintln!("\n=== ENGINE PIPELINE (3 stages) — 15k blocks ===");
    let pipeline_start = Instant::now();
    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;
    let pipeline_elapsed = pipeline_start.elapsed();
    eprintln!("=== ENGINE PIPELINE DONE: {:?} ===\n", pipeline_elapsed);

    // Verify
    let provider = pipeline_provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, NUM_BLOCKS);
    for stage_id in [StageId::Headers, StageId::Bodies, StageId::Execution, StageId::Finish] {
        let cp = provider.get_stage_checkpoint(stage_id)?;
        assert_eq!(cp.map(|c| c.block_number), Some(NUM_BLOCKS), "{stage_id} checkpoint");
    }
    let state = provider.latest();
    assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(NUM_BLOCKS)));

    Ok(())
}

/// Helper: Run H→B→SR→TxLookup setup pipeline (prefix stages only).
/// Returns the provider factory with all prefix stage checkpoints at max_block.
async fn run_prefix_pipeline(
    blocks: Vec<SealedBlock<Block>>,
    chain_spec: Arc<reth_chainspec::ChainSpec>,
) -> eyre::Result<reth_provider::ProviderFactory<reth_provider::test_utils::MockNodeTypesWithDB>> {
    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&provider_factory).expect("init genesis");
    let genesis = provider_factory.sealed_header(0)?.expect("genesis");
    let consensus = NoopConsensus::arc();

    let file_client = Arc::new(FileClient::from_blocks(blocks));
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");
    let stages_config = StageConfig::default();

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), consensus.clone()).into_task();
    header_downloader.update_local_head(genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, consensus, provider_factory.clone()).into_task();
    body_downloader.set_download_range(1..=max_block).expect("set range");

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer = StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stage(HeaderStage::new(
            provider_factory.clone(), header_downloader, tip_rx, stages_config.etl.clone(),
        ))
        .add_stage(BodyStage::new(body_downloader))
        .add_stage(SenderRecoveryStage::new(SenderRecoveryConfig::default(), None))
        .add_stage(TransactionLookupStage::new(TransactionLookupConfig::default(), EtlConfig::default(), None))
        .build(provider_factory.clone(), static_file_producer);
    pipeline.set_tip(tip);

    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    Ok(provider_factory)
}

/// Bench ONLY the post-TxLookup stages of the DEFAULT pipeline (Execution+Hashing+Merkle+History+Finish).
/// Prefix stages (H→B→SR→TxLookup) are run as untimed setup.
#[tokio::test(flavor = "multi_thread")]
async fn bench_default_post_txlookup() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    eprintln!("\n=== BUILDING BLOCKS ===");
    let build_start = Instant::now();
    let (blocks, chain_spec) = build_test_blocks()?;
    eprintln!("Block generation: {:?}", build_start.elapsed());

    eprintln!("Running prefix pipeline (H→B→SR→TxLookup)...");
    let provider_factory = run_prefix_pipeline(blocks, chain_spec.clone()).await?;
    eprintln!("Prefix done.");

    let max_block = NUM_BLOCKS;
    let tip = provider_factory.sealed_header(max_block)?.expect("tip").hash();
    let stages_config = StageConfig::default();
    let evm_config = EthEvmConfig::new(provider_factory.chain_spec());
    let consensus = NoopConsensus::arc();

    // Create dummy downloaders (won't be used since H/B stages will be skipped)
    let file_client: Arc<FileClient<Block>> = Arc::new(FileClient::from_blocks(vec![]));
    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), consensus.clone()).into_task();
    header_downloader.update_local_head(provider_factory.sealed_header(0)?.unwrap());
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, consensus.clone(), provider_factory.clone()).into_task();
    body_downloader.set_download_range(1..=0).ok(); // empty range

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer = StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    let stages = DefaultStages::new(
        provider_factory.clone(), tip_rx, consensus,
        header_downloader, body_downloader, evm_config, stages_config, PruneModes::default(), None,
    );

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(stages)
        .build(provider_factory.clone(), static_file_producer);
    pipeline.set_tip(tip);

    eprintln!("\n=== DEFAULT POST-TXLOOKUP STAGES — {NUM_BLOCKS} blocks ===");
    let start = Instant::now();
    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;
    let elapsed = start.elapsed();
    eprintln!("=== DEFAULT POST-TXLOOKUP DONE: {elapsed:?} ===\n");

    // Verify
    let provider = provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, NUM_BLOCKS);
    let state = provider.latest();
    assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(NUM_BLOCKS)));

    Ok(())
}

/// Bench ONLY the EngineStage (post-TxLookup).
/// Prefix stages (H→B→SR→TxLookup) are run as untimed setup.
#[tokio::test(flavor = "multi_thread")]
async fn bench_engine_post_txlookup() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    eprintln!("\n=== BUILDING BLOCKS ===");
    let build_start = Instant::now();
    let (blocks, chain_spec) = build_test_blocks()?;
    eprintln!("Block generation: {:?}", build_start.elapsed());

    eprintln!("Running prefix pipeline (H→B→SR→TxLookup)...");
    let provider_factory = run_prefix_pipeline(blocks, chain_spec.clone()).await?;
    eprintln!("Prefix done.");

    let max_block = NUM_BLOCKS;
    let tip = provider_factory.sealed_header(max_block)?.expect("tip").hash();
    let evm_config = EthEvmConfig::new(provider_factory.chain_spec());

    let (tip_tx, _tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer = StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    let engine_stage = EngineStage::with_defaults(evm_config, provider_factory.clone());

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stage(engine_stage)
        .build(provider_factory.clone(), static_file_producer);
    pipeline.set_tip(tip);

    eprintln!("\n=== ENGINE POST-TXLOOKUP — {NUM_BLOCKS} blocks ===");
    let start = Instant::now();
    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;
    let elapsed = start.elapsed();
    eprintln!("=== ENGINE POST-TXLOOKUP DONE: {elapsed:?} ===\n");

    // Verify
    let provider = provider_factory.provider()?;
    assert_eq!(provider.last_block_number()?, NUM_BLOCKS);
    let state = provider.latest();
    assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(NUM_BLOCKS)));

    Ok(())
}
