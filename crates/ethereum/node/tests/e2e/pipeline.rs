//! Pipeline forward sync and unwind tests.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
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
    RecoveredBlock, SealedBlock,
};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader, DBProvider,
    DatabaseProviderFactory, HeaderProvider, OriginalValuesKnown, StageCheckpointReader,
    StateWriter,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages::sets::DefaultStages;
use reth_stages_api::{Pipeline, StageId};
use reth_static_file::StaticFileProducer;
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch;

/// Counter contract deployed bytecode (runtime code after deployment)
/// Contract: uint256 public count; function increment() { count += 1; }
const COUNTER_DEPLOYED_BYTECODE: &[u8] = &[
    0x60, 0x80, 0x60, 0x40, 0x52, 0x34, 0x80, 0x15, 0x61, 0x00, 0x0f, 0x57, 0x5f, 0x5f, 0xfd, 0x5b,
    0x50, 0x60, 0x04, 0x36, 0x10, 0x61, 0x00, 0x3f, 0x57, 0x5f, 0x35, 0x60, 0xe0, 0x1c, 0x80, 0x63,
    0x06, 0x66, 0x1a, 0xbd, 0x14, 0x61, 0x00, 0x43, 0x57, 0x80, 0x63, 0xa8, 0x7d, 0x94, 0x2c, 0x14,
    0x61, 0x00, 0x61, 0x57, 0x80, 0x63, 0xd0, 0x9d, 0xe0, 0x8a, 0x14, 0x61, 0x00, 0x7f, 0x57, 0x5b,
    0x5f, 0x5f, 0xfd, 0x5b, 0x61, 0x00, 0x4b, 0x61, 0x00, 0x89, 0x56, 0x5b, 0x60, 0x40, 0x51, 0x61,
    0x00, 0x58, 0x91, 0x90, 0x61, 0x00, 0xc8, 0x56, 0x5b, 0x60, 0x40, 0x51, 0x80, 0x91, 0x03, 0x90,
    0xf3, 0x5b, 0x61, 0x00, 0x69, 0x61, 0x00, 0x8e, 0x56, 0x5b, 0x60, 0x40, 0x51, 0x61, 0x00, 0x76,
    0x91, 0x90, 0x61, 0x00, 0xc8, 0x56, 0x5b, 0x60, 0x40, 0x51, 0x80, 0x91, 0x03, 0x90, 0xf3, 0x5b,
    0x61, 0x00, 0x87, 0x61, 0x00, 0x96, 0x56, 0x5b, 0x00, 0x5b, 0x5f, 0x54, 0x81, 0x56, 0x5b, 0x5f,
    0x5f, 0x54, 0x90, 0x50, 0x90, 0x56, 0x5b, 0x60, 0x01, 0x5f, 0x5f, 0x82, 0x82, 0x54, 0x61, 0x00,
    0xa7, 0x91, 0x90, 0x61, 0x01, 0x0e, 0x56, 0x5b, 0x92, 0x50, 0x50, 0x81, 0x90, 0x55, 0x50, 0x56,
    0x5b, 0x5f, 0x81, 0x90, 0x50, 0x91, 0x90, 0x50, 0x56, 0x5b, 0x61, 0x00, 0xc2, 0x81, 0x61, 0x00,
    0xb0, 0x56, 0x5b, 0x82, 0x52, 0x50, 0x50, 0x56, 0x5b, 0x5f, 0x60, 0x20, 0x82, 0x01, 0x90, 0x50,
    0x61, 0x00, 0xdb, 0x5f, 0x83, 0x01, 0x84, 0x61, 0x00, 0xb9, 0x56, 0x5b, 0x92, 0x91, 0x50, 0x50,
    0x56, 0x5b, 0x7f, 0x4e, 0x48, 0x7b, 0x71, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x5f, 0x52, 0x60, 0x11, 0x60, 0x04, 0x52, 0x60, 0x24, 0x5f, 0xfd, 0x5b, 0x5f,
    0x61, 0x01, 0x18, 0x82, 0x61, 0x00, 0xb0, 0x56, 0x5b, 0x91, 0x50, 0x61, 0x01, 0x23, 0x83, 0x61,
    0x00, 0xb0, 0x56, 0x5b, 0x92, 0x50, 0x82, 0x82, 0x01, 0x90, 0x50, 0x80, 0x82, 0x11, 0x15, 0x61,
    0x01, 0x3b, 0x57, 0x61, 0x01, 0x3a, 0x61, 0x00, 0xe1, 0x56, 0x5b, 0x5b, 0x92, 0x91, 0x50, 0x50,
    0x56, 0xfe, 0xa2, 0x64, 0x69, 0x70, 0x66, 0x73, 0x58, 0x22, 0x12, 0x20, 0x41, 0xc6, 0x2c, 0x65,
    0xcc, 0xf4, 0xb2, 0x0a, 0xcb, 0xc6, 0x3a, 0x65, 0x36, 0xde, 0x7f, 0xdc, 0x8a, 0x59, 0x42, 0x7c,
    0x5b, 0xe4, 0x86, 0xfd, 0x28, 0xca, 0xb3, 0x3a, 0xd2, 0x0a, 0x09, 0x5c, 0x64, 0x73, 0x6f, 0x6c,
    0x63, 0x43, 0x00, 0x08, 0x1f, 0x00, 0x33,
];

/// increment() function selector: keccak256("increment()")[:4]
const INCREMENT_SELECTOR: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];

/// Contract address (deterministic for test)
const CONTRACT_ADDRESS: Address = Address::new([0x42; 20]);

/// Creates a `FileClient` populated with the given blocks.
fn create_file_client_from_blocks(blocks: &[SealedBlock<Block>]) -> Arc<FileClient<Block>> {
    let headers: HashMap<u64, _> = blocks.iter().map(|b| (b.number, b.header().clone())).collect();
    let bodies: HashMap<B256, _> = blocks.iter().map(|b| (b.hash(), b.body().clone())).collect();
    Arc::new(FileClient::default().with_headers(headers).with_bodies(bodies))
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

/// Tests pipeline with ALL stages enabled using a pre-deployed Counter contract.
///
/// This test:
/// 1. Deploys a Counter contract in genesis
/// 2. Calls increment() in each block, creating storage changes
/// 3. Runs the full pipeline with ALL stages enabled
/// 4. Forward syncs to block 5, unwinds to block 2
///
/// This exercises storage hashing and history stages with real contract state.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Generate a keypair for signing transactions
    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());

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
                            code: Some(Bytes::from_static(COUNTER_DEPLOYED_BYTECODE)),
                            ..Default::default()
                        },
                    ),
                ]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .paris_activated()
            .build(),
    );

    let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&provider_factory).expect("init genesis");

    let genesis = provider_factory.sealed_header(0)?.expect("genesis should exist");
    let evm_config = EthEvmConfig::new(chain_spec.clone());

    // Build blocks by actually executing transactions to get correct state roots
    let num_blocks = 5u64;
    let mut blocks: Vec<SealedBlock<Block>> = Vec::new();
    let mut parent_hash = genesis.hash();

    let gas_price = INITIAL_BASE_FEE as u128;

    for block_num in 1..=num_blocks {
        let nonce = block_num - 1;

        // Create increment() call transaction
        let tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce,
                gas_limit: 50_000,
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(CONTRACT_ADDRESS),
                value: U256::ZERO,
                input: Bytes::from(INCREMENT_SELECTOR.to_vec()),
                ..Default::default()
            }),
        );

        let transactions = vec![tx.clone()];
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
            vec![signer_address],
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

        // Create receipt for receipt root calculation
        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: reth_ethereum_primitives::TxType::Eip1559,
            success: true,
            cumulative_gas_used: gas_used,
            ..Default::default()
        };
        let receipts = [receipt];
        let receipts_root = calculate_receipt_root(
            &receipts.iter().map(|r| r.with_bloom_ref()).collect::<Vec<_>>(),
        );

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
        // Note: We skip write_state() and update_history_indices() as those require block indices
        // to exist in the database. We only need plain state for subsequent block execution.
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
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis =
        pipeline_provider_factory.sealed_header(0)?.expect("genesis should exist");
    let pipeline_consensus = NoopConsensus::arc();

    let file_client = create_file_client_from_blocks(&blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();
    let (header_downloader, body_downloader) = build_downloaders_from_file_client(
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

    // Unwind to block 2
    let unwind_target = 2u64;
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
    }

    Ok(())
}
