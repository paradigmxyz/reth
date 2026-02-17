//! Engine pipeline forward sync test.
//! Uses only HeaderStage + BodyStage + EngineStage with 15k blocks.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{bytes, Address, Bytes, TxKind, B256, U256};
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider, MAINNET};
use reth_config::config::{EtlConfig, SenderRecoveryConfig, StageConfig, TransactionLookupConfig};
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
use reth_stages::stages::{BodyStage, EngineStage, HeaderStage, SenderRecoveryStage, TransactionLookupStage};
use reth_stages_api::{Pipeline, StageId};
use reth_static_file::StaticFileProducer;
use reth_storage_api::StateProvider;
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use reth_trie::{HashedPostState, KeccakKeyHasher, StateRoot};
use reth_trie_db::DatabaseStateRoot;
use std::sync::Arc;
use tokio::sync::watch;

/// Counter contract deployed bytecode
const COUNTER_DEPLOYED_BYTECODE: Bytes = bytes!(
    "6080604052348015600e575f5ffd5b50600436106030575f3560e01c806306661abd146034578063d09de08a14604e575b5f5ffd5b603a6056565b604051604591906089565b60405180910390f35b6054605b565b005b5f5481565b60015f5f828254606a919060cd565b92505081905550565b5f819050919050565b6083816073565b82525050565b5f602082019050609a5f830184607c565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52601160045260245ffd5b5f60d5826073565b915060de836073565b925082820190508082111560f35760f260a0565b5b9291505056fea2646970667358221220576016d010ec2f4f83b992fb97d16efd1bc54110c97aa5d5cb47d20d3b39a35264736f6c634300081f0033"
);

const INCREMENT_SELECTOR: [u8; 4] = [0xd0, 0x9d, 0xe0, 0x8a];
const CONTRACT_ADDRESS: Address = Address::new([0x42; 20]);

fn create_file_client_from_blocks(blocks: Vec<SealedBlock<Block>>) -> Arc<FileClient<Block>> {
    Arc::new(FileClient::from_blocks(blocks))
}

/// Build the engine pipeline: HeaderStage -> BodyStage -> EngineStage
fn build_engine_pipeline<H, B>(
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
    let stages_config = StageConfig::default();
    let evm_config = EthEvmConfig::new(provider_factory.chain_spec());

    let (tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let static_file_producer =
        StaticFileProducer::new(provider_factory.clone(), PruneModes::default());

    // EngineStage with default 5k batch size - uses the engine's execution path
    let engine_stage = EngineStage::with_defaults(evm_config, provider_factory.clone());

    let pipeline = Pipeline::builder()
        .with_tip_sender(tip_tx)
        .with_max_block(max_block)
        .with_fail_on_unwind(true)
        .add_stage(HeaderStage::new(
            provider_factory.clone(),
            header_downloader,
            tip_rx,
            stages_config.etl,
        ))
        .add_stage(BodyStage::new(body_downloader))
        .add_stage(SenderRecoveryStage::new(SenderRecoveryConfig::default(), None))
        .add_stage(TransactionLookupStage::new(TransactionLookupConfig::default(), EtlConfig::default(), None))
        .add_stage(engine_stage)
        .build(provider_factory, static_file_producer);
    pipeline.set_tip(tip);
    pipeline
}

/// Test the engine pipeline with 15k blocks (triggers 3 batches of 5k persistence).
#[tokio::test(flavor = "multi_thread")]
async fn test_engine_pipeline() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let num_blocks: u64 = 15_000;

    let mut rng = generators::rng();
    let key_pair = generate_key(&mut rng);
    let signer_address = public_key_to_address(key_pair.public_key());
    let recipient_address = Address::new([0x11; 20]);

    // Large initial balance to cover 15k blocks of transfers + gas
    let initial_balance = U256::from(ETH_TO_WEI) * U256::from(1_000_000);
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
    let transfer_value = U256::from(1_000_000_000u64);

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
                value: transfer_value,
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
                temp_header.clone(),
                BlockBody { transactions: transactions.clone(), ommers: Vec::new(), withdrawals: None },
            ),
            vec![signer_address, signer_address],
        );

        let output = {
            let state_provider = provider.latest();
            let db = StateProviderDatabase::new(&*state_provider);
            let executor = evm_config.batch_executor(db);
            executor.execute(&block_with_senders)?
        };

        let gas_used = output.gas_used;
        let hashed_state = HashedPostState::from_bundle_state::<KeccakKeyHasher>(output.state.state());
        let (state_root, _) = StateRoot::overlay_root_with_updates(
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
            timestamp: block_num * 12,
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

        parent_hash = block.hash();
        blocks.push(block);

        if block_num % 5000 == 0 {
            eprintln!("Built block {block_num}/{num_blocks}");
        }
    }

    eprintln!("All {num_blocks} blocks built, starting pipeline...");

    // Fresh provider factory for the pipeline (clean state from genesis)
    let pipeline_provider_factory =
        create_test_provider_factory_with_chain_spec(chain_spec.clone());
    init_genesis(&pipeline_provider_factory).expect("init genesis");
    let pipeline_genesis =
        pipeline_provider_factory.sealed_header(0)?.expect("genesis should exist");
    let pipeline_consensus = NoopConsensus::arc();

    let file_client = create_file_client_from_blocks(blocks);
    let max_block = file_client.max_block().unwrap();
    let tip = file_client.tip().expect("tip");

    let stages_config = StageConfig::default();

    let mut header_downloader = ReverseHeadersDownloaderBuilder::new(stages_config.headers)
        .build(file_client.clone(), pipeline_consensus.clone())
        .into_task();
    header_downloader.update_local_head(pipeline_genesis);
    header_downloader.update_sync_target(SyncTarget::Tip(tip));

    let mut body_downloader = BodiesDownloaderBuilder::new(stages_config.bodies)
        .build(file_client, pipeline_consensus, pipeline_provider_factory.clone())
        .into_task();
    body_downloader.set_download_range(1..=max_block).expect("set download range");

    let pipeline = build_engine_pipeline(
        pipeline_provider_factory.clone(),
        header_downloader,
        body_downloader,
        max_block,
        tip,
    );

    let (_pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    eprintln!("Pipeline finished, verifying...");

    // Verify forward sync
    {
        let provider = pipeline_provider_factory.provider()?;
        let last_block = provider.last_block_number()?;
        assert_eq!(last_block, num_blocks, "should have synced {num_blocks} blocks");

        // Check all stage checkpoints set by update_pipeline_stages
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

        // Verify counter contract storage
        let state = provider.latest();
        let counter_storage = state.storage(CONTRACT_ADDRESS, B256::ZERO)?;
        assert_eq!(
            counter_storage,
            Some(U256::from(num_blocks)),
            "Counter storage slot 0 should be {num_blocks} after {num_blocks} increments"
        );
    }

    eprintln!("All assertions passed!");
    Ok(())
}
