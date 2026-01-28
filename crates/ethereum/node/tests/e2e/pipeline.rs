//! Pipeline forward sync and unwind tests.

use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip1559, TxReceipt, EMPTY_ROOT_HASH};
use alloy_eips::eip1559::INITIAL_BASE_FEE;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Address, TxKind, B256, U256};
use alloy_trie::root::state_root_unhashed;
use reth_chainspec::{ChainSpecBuilder, ChainSpecProvider, MAINNET, MIN_TRANSACTION_GAS};
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
use reth_primitives_traits::{
    crypto::secp256k1::public_key_to_address,
    proofs::{calculate_receipt_root, calculate_transaction_root},
    Account, SealedBlock,
};
use reth_provider::{
    test_utils::create_test_provider_factory_with_chain_spec, BlockNumReader, HeaderProvider,
    StageCheckpointReader,
};
use reth_prune_types::PruneModes;
use reth_stages::sets::DefaultStages;
use reth_stages_api::{Pipeline, StageId};
use reth_static_file::StaticFileProducer;
use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch;

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

/// Tests pipeline with ALL stages enabled using blocks with real ETH transfers.
///
/// This test creates a custom `ChainSpec` with a pre-funded signer account in genesis,
/// then generates blocks with signed ETH transfer transactions. This exercises the
/// full pipeline including hashing stages with actual state changes.
#[tokio::test(flavor = "multi_thread")]
async fn test_pipeline() -> eyre::Result<()> {
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
    let mut current_balance = initial_balance;

    // Gas cost per transaction
    let gas_price = INITIAL_BASE_FEE as u128;
    let gas_limit = MIN_TRANSACTION_GAS;
    let tx_cost = U256::from(gas_price) * U256::from(gas_limit);
    let transfer_value = U256::from(ETH_TO_WEI / 10); // 0.1 ETH per transfer

    for block_num in 1..=num_blocks {
        let nonce = block_num - 1;

        // Create a simple ETH transfer transaction
        let tx = sign_tx_with_key_pair(
            key_pair,
            Transaction::Eip1559(TxEip1559 {
                chain_id: chain_spec.chain.id(),
                nonce,
                gas_limit,
                max_fee_per_gas: gas_price,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(Address::ZERO),
                value: transfer_value,
                ..Default::default()
            }),
        );

        // Update signer's balance
        current_balance -= tx_cost + transfer_value;

        // Track recipient balance
        let recipient_balance = transfer_value * U256::from(block_num);

        // Compute state root
        let state_root = state_root_unhashed([
            (
                signer_address,
                Account { balance: current_balance, nonce: block_num, ..Default::default() }
                    .into_trie_account(EMPTY_ROOT_HASH),
            ),
            (
                Address::ZERO,
                Account { balance: recipient_balance, nonce: 0, ..Default::default() }
                    .into_trie_account(EMPTY_ROOT_HASH),
            ),
        ]);

        let transactions = vec![tx.clone()];
        let tx_root = calculate_transaction_root(&transactions);

        let receipt = reth_ethereum_primitives::Receipt {
            tx_type: reth_ethereum_primitives::TxType::Eip1559,
            success: true,
            cumulative_gas_used: gas_limit,
            ..Default::default()
        };
        let receipts = [receipt];
        let receipts_root =
            calculate_receipt_root(&receipts.iter().map(|r| r.with_bloom_ref()).collect::<Vec<_>>());

        let header = Header {
            parent_hash,
            number: block_num,
            state_root,
            transactions_root: tx_root,
            receipts_root,
            gas_limit: 30_000_000,
            gas_used: gas_limit,
            base_fee_per_gas: Some(INITIAL_BASE_FEE),
            timestamp: block_num * 12,
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

    let pipeline =
        build_pipeline(provider_factory.clone(), header_downloader, body_downloader, max_block, tip);

    let (mut pipeline, result) = pipeline.run_as_fut(None).await;
    result?;

    // Verify forward sync
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

    // Unwind to block 2
    let unwind_target = 2u64;
    pipeline.unwind(unwind_target, None)?;

    // Verify unwind
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
