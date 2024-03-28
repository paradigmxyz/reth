//! Node builder setup tests.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use futures_util::StreamExt;
use secp256k1::KeyPair;

use reth_db::test_utils::create_test_rw_db;
use reth_interfaces::{
    consensus::ForkchoiceState,
    executor::{BlockExecutionError, BlockValidationError},
    test_utils::{generators, generators::sign_tx_with_key_pair},
};
use reth_node_api::ConfigureEvm;
use reth_node_builder::{components::FullNodeComponents, NodeBuilder, NodeConfig, NodeHandle};
use reth_node_core::{
    args::RpcServerArgs,
    primitives::{Address, Bloom, Bytes, Chain, Genesis, U256},
    rpc::{
        api::EngineApiClient, compat::engine::try_block_to_payload_v1,
        types::engine::PayloadStatusEnum,
    },
};
use reth_node_ethereum::{node::EthereumNode, EthEngineTypes};
use reth_primitives::{
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS},
    genesis::GenesisAllocator,
    proofs, Block, BlockWithSenders, ChainSpec, Header, ReceiptWithBloom, SealedBlock, Transaction,
    TransactionKind, TransactionSigned, TxLegacy, EMPTY_OMMER_ROOT_HASH, GOERLI,
};
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, CanonStateSubscriptions,
    StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
    processor::EVMProcessor, State,
};
use reth_rpc_types::{engine::PayloadStatus, BlockNumberOrTag};
use reth_tasks::TaskManager;
use reth_tracing::{tracing::trace, RethTracer, Tracer};

/// create an account and add it to the genesis block.
fn genesis_and_account(genesis: Genesis) -> (Genesis, KeyPair, Address) {
    let mut rng = generators::rng();
    let amount = U256::from(1000000000000000000u64);
    let mut allocator = GenesisAllocator::default().with_rng(&mut rng);
    let (key_pair, address) = allocator.new_funded_account(amount);
    let genesis = genesis.extend_accounts(allocator.build());
    (genesis, key_pair, address)
}

/// Fills in the post-execution header fields based on the given BundleState and gas used.
/// In doing this, the state root is calculated and the final header is returned.
fn complete_header<S: StateProviderFactory>(
    mut header: Header,
    bundle_state: &BundleStateWithReceipts,
    client: &S,
    gas_used: u64,
) -> Result<Header, BlockExecutionError> {
    let receipts = bundle_state.receipts_by_block(header.number);
    header.receipts_root = if receipts.is_empty() {
        EMPTY_RECEIPTS
    } else {
        let receipts_with_bloom = receipts
            .iter()
            .map(|r| (*r).clone().expect("receipts have not been pruned").into())
            .collect::<Vec<ReceiptWithBloom>>();
        header.logs_bloom =
            receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);
        proofs::calculate_receipt_root(&receipts_with_bloom)
    };

    header.gas_used = gas_used;

    // calculate the state root
    let state_root = client
        .latest()
        .map_err(|_| BlockExecutionError::ProviderError)?
        .state_root(bundle_state)
        .unwrap();
    header.state_root = state_root;
    Ok(header)
}

/// Executes the block with the given block and senders, on the provided [EVMProcessor].
///
/// This returns the poststate from execution and post-block changes, as well as the gas used.
fn execute<EvmConfig>(
    block: &BlockWithSenders,
    executor: &mut EVMProcessor<'_, EvmConfig>,
) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError>
where
    EvmConfig: ConfigureEvm,
{
    trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");

    // set the first block to find the correct index in bundle state
    executor.set_first_block(block.number);

    let (receipts, gas_used) = executor.execute_transactions(block, U256::ZERO)?;

    // Save receipts.
    executor.save_receipts(receipts)?;

    // add post execution state change
    // Withdrawals, rewards etc.
    executor.apply_post_execution_state_change(block, U256::ZERO)?;

    // merge transitions
    executor.db_mut().merge_transitions(BundleRetention::Reverts);

    // apply post block changes
    Ok((executor.take_output_state(), gas_used))
}

/// Fills in pre-execution header fields based on the current best block and given
/// transactions.
fn build_header_template(
    parent: Header,
    transactions: &[TransactionSigned],
    chain_spec: Arc<ChainSpec>,
) -> Header {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

    // check previous block for base fee
    let base_fee_per_gas = parent.next_block_base_fee(chain_spec.base_fee_params(timestamp));

    let mut header = Header {
        parent_hash: parent.hash_slow(),
        ommers_hash: EMPTY_OMMER_ROOT_HASH,
        beneficiary: Default::default(),
        state_root: Default::default(),
        transactions_root: Default::default(),
        receipts_root: Default::default(),
        withdrawals_root: None,
        logs_bloom: Default::default(),
        difficulty: Default::default(),
        number: parent.number + 1,
        gas_limit: parent.gas_limit,
        gas_used: 0,
        timestamp,
        mix_hash: Default::default(),
        nonce: 0,
        base_fee_per_gas,
        blob_gas_used: None,
        excess_blob_gas: None,
        extra_data: Default::default(),
        parent_beacon_block_root: None,
    };

    header.transactions_root = if transactions.is_empty() {
        EMPTY_TRANSACTIONS
    } else {
        proofs::calculate_transaction_root(transactions)
    };

    header
}

/// Builds and executes a new block with the given transactions, on the provided [EVMProcessor].
///
/// This returns the header of the executed block, as well as the poststate from execution.
fn build_and_execute<EvmConfig>(
    parent: Header,
    transactions: Vec<TransactionSigned>,
    client: &impl StateProviderFactory,
    chain_spec: Arc<ChainSpec>,
    evm_config: EvmConfig,
) -> Result<(SealedBlock, BundleStateWithReceipts), BlockExecutionError>
where
    EvmConfig: ConfigureEvm,
{
    let header = build_header_template(parent, &transactions, chain_spec.clone());

    let block = Block { header, body: transactions, ommers: vec![], withdrawals: None }
        .with_recovered_senders()
        .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

    // now execute the block
    let db = State::builder()
        .with_database_boxed(Box::new(StateProviderDatabase::new(client.latest().unwrap())))
        .with_bundle_update()
        .build();
    let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db, evm_config);

    let (bundle_state, gas_used) = execute(&block, &mut executor)?;

    let Block { header, body, ommers, withdrawals, .. } = block.block;

    // fill in the rest of the fields
    let header = complete_header(header, &bundle_state, client, gas_used)?;

    let sealed_block = SealedBlock { header: header.seal_slow(), body, ommers, withdrawals };

    Ok((sealed_block, bundle_state))
}

#[test]
fn test_basic_setup() {
    // parse CLI -> config
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let msg = "On components".to_string();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types(EthereumNode::default())
        .with_components(EthereumNode::components())
        .on_component_initialized(move |ctx| {
            let _provider = ctx.provider();
            println!("{msg}");
            Ok(())
        })
        .on_node_started(|_full_node| Ok(()))
        .on_rpc_started(|_ctx, handles| {
            let _client = handles.rpc.http_client();
            Ok(())
        })
        .extend_rpc_modules(|ctx| {
            let _ = ctx.config();
            let _ = ctx.node().provider();

            Ok(())
        })
        .check_launch();
}

#[tokio::test]
async fn test_basic_launch() -> eyre::Result<()> {
    let _guard = RethTracer::new().init()?;
    let tasks = TaskManager::current();

    // create an account and add it to the genesis account
    let (genesis, key_pair, address) = genesis_and_account(GOERLI.genesis.clone());

    // create node config
    let chain = Chain::goerli();
    let spec = ChainSpec::builder()
        .chain(chain)
        .genesis(genesis)
        .london_activated()
        .paris_activated()
        .build();
    let node_config =
        NodeConfig::test().with_rpc(RpcServerArgs::default().with_http()).with_chain(spec.clone());

    // launch a testing node
    let NodeHandle { node, .. } = NodeBuilder::new(node_config)
        .testing_node(tasks.executor())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut notifications = node.provider.canonical_state_stream();

    // generate a signed tx
    let transfer_tx = Transaction::Legacy(TxLegacy {
        chain_id: Some(chain.id()),
        nonce: 0,
        input: Bytes::default(),
        gas_price: 1000000000,
        gas_limit: 21000u64,
        to: TransactionKind::Call(address),
        value: U256::from(10000000000u64),
    });
    let transaction_signed = sign_tx_with_key_pair(key_pair, transfer_tx);

    // generate a block containing the generated tx
    let parent = node.provider.header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();
    let (block1, _bundle_state) = build_and_execute(
        parent,
        vec![transaction_signed],
        &node.provider,
        Arc::new(spec),
        node.evm_config,
    )?;

    // sends a new block via engine API
    let client = node.rpc_server_handles.auth.http_client();
    let result = EngineApiClient::<EthEngineTypes>::new_payload_v1(
        &client,
        try_block_to_payload_v1(block1.clone()),
    )
    .await?;
    // check if new block was accepted
    assert_eq!(result, PayloadStatus::new(PayloadStatusEnum::Valid, Some(block1.hash())));

    // sends a forkchoice update via engine API
    let result1 = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v1(
        &client,
        ForkchoiceState {
            head_block_hash: block1.hash(),
            safe_block_hash: block1.hash(),
            finalized_block_hash: block1.hash(),
        },
        None,
    )
    .await?;
    // check if forkchoice update was successful
    assert_eq!(
        result1.payload_status,
        PayloadStatus::new(PayloadStatusEnum::Valid, Some(block1.hash()))
    );

    // checks if chain committed block
    let head = notifications.next().await.expect("no notifications");
    assert_eq!(head.tip().block.hash(), block1.hash());

    Ok(())
}
