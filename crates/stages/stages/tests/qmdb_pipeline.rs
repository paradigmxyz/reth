//! `QMDb` staged-pipeline end-to-end tests.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider, MAINNET};
use reth_config::config::StageConfig;
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
    AlloyBlockHeader, RecoveredBlock, SealedBlock,
};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader, DBProvider,
    DatabaseProviderFactory, HeaderProvider, OriginalValuesKnown, StageCheckpointReader,
    StateWriter, StorageSettingsCache,
};
use reth_prune_types::PruneModes;
use reth_qmdb::{genesis_hashed_state, QmdbBlock, QmdbConfig, QmdbStage, QmdbState, QMDB_STAGE_ID};
use reth_revm::database::StateProviderDatabase;
use reth_stages::{sets::DefaultStages, Pipeline, StageId, StageSet};
use reth_static_file::StaticFileProducer;
use reth_storage_api::{StateProvider, StorageSettings};
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher};
use std::sync::Arc;
use tokio::sync::watch;

/// Counter contract deployed bytecode compiled with Solidity 0.8.31.
const COUNTER_DEPLOYED_BYTECODE: Bytes = bytes!(
    "6080604052348015600e575f5ffd5b50600436106030575f3560e01c806306661abd146034578063d09de08a14604e575b5f5ffd5b603a6056565b604051604591906089565b60405180910390f35b6054605b565b005b5f5481565b60015f5f828254606a919060cd565b92505081905550565b5f819050919050565b6083816073565b82525050565b5f602082019050609a5f830184607c565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f60d5826073565b915060de836073565b925082820190508082111560f35760f260a0565b5b9291505056fea2646970667358221220576016d010ec2f4f83b992fb97d16efd1bc54110c97aa5d5cb47d20d3b39a35264736f6c634300081f0033"
);

/// `increment()` function selector: `keccak256("increment()")`[:4].
const INCREMENT_SELECTOR: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];

const CONTRACT_ADDRESS: Address = Address::new([0x42; 20]);

fn create_file_client_from_blocks(blocks: Vec<SealedBlock<Block>>) -> Arc<FileClient<Block>> {
    Arc::new(FileClient::from_blocks(blocks))
}

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

fn build_qmdb_pipeline<H, B>(
    provider_factory: reth_provider::ProviderFactory<
        reth_provider::test_utils::MockNodeTypesWithDB,
    >,
    header_downloader: H,
    body_downloader: B,
    max_block: u64,
    tip: B256,
    qmdb: QmdbState,
    qmdb_batch_blocks: Option<u64>,
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
    let qmdb_stage = match qmdb_batch_blocks {
        Some(batch) => QmdbStage::new(qmdb).with_batch_blocks(batch),
        None => QmdbStage::new(qmdb),
    };

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
    )
    .builder()
    .disable_all(&[
        StageId::MerkleUnwind,
        StageId::AccountHashing,
        StageId::StorageHashing,
        StageId::MerkleExecute,
    ])
    .add_after(qmdb_stage, StageId::Execution);

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stages(stages)
        .build(provider_factory, static_file_producer);
    pipeline.set_tip(tip);
    pipeline
}

async fn run_qmdb_pipeline_forward_reopen_unwind(
    storage_settings: Option<StorageSettings>,
    num_blocks: u64,
    unwind_target: u64,
    qmdb_batch_blocks: Option<u64>,
) -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());
    let recipient_address = Address::new([0x11; 20]);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [
                    (signer_address, GenesisAccount { balance: U256::MAX, ..Default::default() }),
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
    let generation_qmdb_dir = tempfile::tempdir()?;
    let generation_qmdb = QmdbState::open(
        QmdbConfig::new(generation_qmdb_dir.path()).with_partition_prefix("generation"),
    )?;
    generation_qmdb.commit_block(
        QmdbBlock { number: 0, hash: genesis.hash(), parent_hash: genesis.parent_hash() },
        genesis_hashed_state(chain_spec.genesis()),
    )?;

    let mut blocks = Vec::with_capacity(num_blocks as usize);
    let mut parent_hash = genesis.hash();
    let gas_price = INITIAL_BASE_FEE as u128;

    for block_num in 1..=num_blocks {
        let base_nonce = (block_num - 1) * 2;
        let eth_transfer_tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: base_nonce,
                gas_limit: 21_000,
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(recipient_address),
                value: U256::from(ETH_TO_WEI),
                input: Bytes::new(),
                ..Default::default()
            }),
        );

        let counter_tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce: base_nonce + 1,
                gas_limit: 100_000,
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
        let temp_header = Header {
            parent_hash,
            number: block_num,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12,
            ..Default::default()
        };

        let provider = provider_factory.database_provider_rw()?;
        let block_with_senders = RecoveredBlock::new_unhashed(
            Block::new(
                temp_header,
                BlockBody {
                    transactions: transactions.clone(),
                    ommers: Vec::new(),
                    withdrawals: None,
                },
            ),
            vec![signer_address, signer_address],
        );

        let output = {
            let state_provider = provider.latest();
            let db = StateProviderDatabase::new(&*state_provider);
            let executor = evm_config.batch_executor(db);
            executor.execute(&block_with_senders)?
        };

        let hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
        let qmdb_root = generation_qmdb.overlay_root(hashed_state.clone())?.root;
        let receipts: Vec<_> = output.receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let receipts_root = calculate_receipt_root(&receipts);

        let header = Header {
            parent_hash,
            number: block_num,
            state_root: qmdb_root,
            transactions_root: tx_root,
            receipts_root,
            gas_limit: 30_000_000,
            gas_used: output.gas_used,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12,
            ..Default::default()
        };
        let block: SealedBlock<Block> = SealedBlock::seal_parts(
            header,
            BlockBody { transactions, ommers: Vec::new(), withdrawals: None },
        );
        generation_qmdb.commit_block(
            QmdbBlock { number: block_num, hash: block.hash(), parent_hash },
            hashed_state.clone(),
        )?;

        let plain_state = output.state.to_plain_state(OriginalValuesKnown::Yes);
        provider.write_state_changes(plain_state)?;
        provider.write_hashed_state(&hashed_state.into_sorted())?;
        provider.commit()?;

        parent_hash = block.hash();
        blocks.push(block);
    }

    let expected_tip_root =
        blocks.last().expect("test should build at least one block").header().state_root();

    let pipeline_provider_factory =
        create_test_provider_factory_with_chain_spec(chain_spec.clone());
    if let Some(settings) = storage_settings {
        pipeline_provider_factory.set_storage_settings_cache(settings);
    }
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis =
        pipeline_provider_factory.sealed_header(0)?.expect("genesis should exist");
    let pipeline_qmdb_dir = tempfile::tempdir()?;
    let pipeline_qmdb_config =
        QmdbConfig::new(pipeline_qmdb_dir.path()).with_partition_prefix("pipeline");
    let pipeline_qmdb = QmdbState::open(pipeline_qmdb_config.clone())?;
    pipeline_qmdb.commit_block(
        QmdbBlock {
            number: 0,
            hash: pipeline_genesis.hash(),
            parent_hash: pipeline_genesis.parent_hash(),
        },
        genesis_hashed_state(chain_spec.genesis()),
    )?;

    let blocks_clone = blocks.clone();
    let file_client = create_file_client_from_blocks(blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let (header_downloader, body_downloader, _runtime) = build_downloaders_from_file_client(
        file_client,
        pipeline_genesis,
        StageConfig::default(),
        NoopConsensus::arc(),
        pipeline_provider_factory.clone(),
    );
    let pipeline = build_qmdb_pipeline(
        pipeline_provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
        pipeline_qmdb.clone(),
        qmdb_batch_blocks,
    );

    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    {
        let provider = pipeline_provider_factory.provider()?;
        assert_eq!(provider.last_block_number()?, num_blocks);
        assert_eq!(
            provider.get_stage_checkpoint(QMDB_STAGE_ID)?.map(|c| c.block_number),
            Some(num_blocks)
        );
        assert_eq!(
            provider.get_stage_checkpoint(StageId::Finish)?.map(|c| c.block_number),
            Some(num_blocks)
        );

        let state = provider.latest();
        assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(num_blocks)));
    }

    let head = pipeline_qmdb.head()?.expect("QMDb head should exist after pipeline sync");
    assert_eq!(head.number, num_blocks);
    assert_eq!(head.hash, tip);
    assert_eq!(head.root, expected_tip_root);

    pipeline.unwind(unwind_target, None)?;
    {
        let provider = pipeline_provider_factory.provider()?;
        let state = provider.latest();
        assert_eq!(state.storage(CONTRACT_ADDRESS, B256::ZERO)?, Some(U256::from(unwind_target)));
    }
    let unwound = pipeline_qmdb.head()?.expect("QMDb head should exist after unwind");
    assert_eq!(unwound.number, unwind_target);

    let resync_file_client = create_file_client_from_blocks(blocks_clone);
    let unwind_head = pipeline_provider_factory
        .sealed_header(unwind_target)?
        .expect("unwind target header should exist");
    let resync_runtime = reth_tasks::Runtime::test();
    let mut resync_header_downloader =
        ReverseHeadersDownloaderBuilder::new(StageConfig::default().headers)
            .build(resync_file_client.clone(), NoopConsensus::arc())
            .into_task_with(&resync_runtime);
    resync_header_downloader.update_local_head(unwind_head);
    resync_header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut resync_body_downloader = BodiesDownloaderBuilder::new(StageConfig::default().bodies)
        .build(resync_file_client, NoopConsensus::arc(), pipeline_provider_factory.clone())
        .into_task_with(&resync_runtime);
    resync_body_downloader
        .set_download_range(unwind_target + 1..=max_block)
        .expect("set download range");

    let resync_pipeline = build_qmdb_pipeline(
        pipeline_provider_factory.clone(),
        resync_header_downloader,
        resync_body_downloader,
        max_block,
        tip,
        pipeline_qmdb.clone(),
        qmdb_batch_blocks,
    );
    let (resync_pipeline, resync_result) = resync_pipeline.run_as_fut(None).await;
    resync_result?;

    let final_head = pipeline_qmdb.head()?.expect("QMDb head should exist after resync");
    assert_eq!(final_head.number, num_blocks);
    assert_eq!(final_head.hash, tip);
    assert_eq!(final_head.root, expected_tip_root);

    drop(resync_pipeline);
    drop(pipeline);
    drop(pipeline_qmdb);
    let reopened = QmdbState::open(pipeline_qmdb_config)?;
    assert_eq!(reopened.head()?, Some(final_head));
    assert_eq!(reopened.root()?, final_head.root);

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn qmdb_pipeline_e2e_small() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(None, 5, 2, Some(1)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn qmdb_pipeline_e2e_storage_v2_small() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 5, 2, Some(1)).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "QMDb pipeline e2e scale check; run explicitly for timing"]
async fn qmdb_pipeline_e2e_10() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 10, 5, None).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "QMDb pipeline e2e scale check; run explicitly for timing"]
async fn qmdb_pipeline_e2e_100() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 100, 50, None).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "QMDb pipeline e2e scale check; run explicitly for timing"]
async fn qmdb_pipeline_e2e_1000() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 1_000, 500, None).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "QMDb pipeline e2e scale check; run explicitly for timing"]
async fn qmdb_pipeline_e2e_10k() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 10_000, 5_000, None).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "100k QMDb pipeline e2e; run explicitly for long-chain validation"]
async fn qmdb_pipeline_e2e_100k() -> eyre::Result<()> {
    run_qmdb_pipeline_forward_reopen_unwind(Some(StorageSettings::v2()), 100_000, 50_000, None)
        .await
}
