use std::sync::Arc;

use alloy_consensus::{constants::ETH_TO_WEI, BlockHeader, Header, TxEip2930};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{b256, Address, TxKind, U256};
use reth_chainspec::{ChainSpec, ChainSpecBuilder, EthereumHardfork, MAINNET, MIN_TRANSACTION_GAS};
use reth_evm::execute::{BatchExecutor, BlockExecutionOutput, BlockExecutorProvider, Executor};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_node_api::FullNodePrimitives;
use reth_primitives::{Block, BlockBody, Receipt, RecoveredBlock, Transaction};
use reth_primitives_traits::Block as _;
use reth_provider::{
    providers::ProviderNodeTypes, BlockWriter as _, ExecutionOutcome, LatestStateProviderRef,
    ProviderFactory,
};
use reth_revm::database::StateProviderDatabase;
use reth_testing_utils::generators::sign_tx_with_key_pair;
use secp256k1::Keypair;

pub(crate) fn to_execution_outcome(
    block_number: u64,
    block_execution_output: &BlockExecutionOutput<Receipt>,
) -> ExecutionOutcome {
    ExecutionOutcome {
        bundle: block_execution_output.state.clone(),
        receipts: block_execution_output.receipts.clone().into(),
        first_block: block_number,
        requests: vec![block_execution_output.requests.clone()],
    }
}

pub(crate) fn chain_spec(address: Address) -> Arc<ChainSpec> {
    // Create a chain spec with a genesis state that contains the
    // provided sender
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(Genesis {
                alloc: [(
                    address,
                    GenesisAccount { balance: U256::from(ETH_TO_WEI), ..Default::default() },
                )]
                .into(),
                ..MAINNET.genesis.clone()
            })
            .paris_activated()
            .build(),
    )
}

pub(crate) fn execute_block_and_commit_to_database<N>(
    provider_factory: &ProviderFactory<N>,
    chain_spec: Arc<ChainSpec>,
    block: &RecoveredBlock<reth_primitives::Block>,
) -> eyre::Result<BlockExecutionOutput<Receipt>>
where
    N: ProviderNodeTypes<
        Primitives: FullNodePrimitives<
            Block = reth_primitives::Block,
            BlockBody = reth_primitives::BlockBody,
            Receipt = reth_primitives::Receipt,
        >,
    >,
{
    let provider = provider_factory.provider()?;

    // Execute the block to produce a block execution output
    let mut block_execution_output = EthExecutorProvider::ethereum(chain_spec)
        .executor(StateProviderDatabase::new(LatestStateProviderRef::new(&provider)))
        .execute(block)?;
    block_execution_output.state.reverts.sort();

    // Convert the block execution output to an execution outcome for committing to the database
    let execution_outcome = to_execution_outcome(block.number(), &block_execution_output);

    // Commit the block's execution outcome to the database
    let provider_rw = provider_factory.provider_rw()?;
    provider_rw.append_blocks_with_state(
        vec![block.clone()],
        &execution_outcome,
        Default::default(),
        Default::default(),
    )?;
    provider_rw.commit()?;

    Ok(block_execution_output)
}

fn blocks(
    chain_spec: Arc<ChainSpec>,
    key_pair: Keypair,
) -> eyre::Result<(RecoveredBlock<reth_primitives::Block>, RecoveredBlock<reth_primitives::Block>)>
{
    // First block has a transaction that transfers some ETH to zero address
    let block1 = Block {
        header: Header {
            parent_hash: chain_spec.genesis_hash(),
            receipts_root: b256!(
                "d3a6acf9a244d78b33831df95d472c4128ea85bf079a1d41e32ed0b7d2244c9e"
            ),
            difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
            number: 1,
            gas_limit: MIN_TRANSACTION_GAS,
            gas_used: MIN_TRANSACTION_GAS,
            ..Default::default()
        },
        body: BlockBody {
            transactions: vec![sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip2930(TxEip2930 {
                    chain_id: chain_spec.chain.id(),
                    nonce: 0,
                    gas_limit: MIN_TRANSACTION_GAS,
                    gas_price: 1_500_000_000,
                    to: TxKind::Call(Address::ZERO),
                    value: U256::from(0.1 * ETH_TO_WEI as f64),
                    ..Default::default()
                }),
            )],
            ..Default::default()
        },
    }
    .try_into_recovered()?;

    // Second block resends the same transaction with increased nonce
    let block2 = Block {
        header: Header {
            parent_hash: block1.hash(),
            receipts_root: b256!(
                "d3a6acf9a244d78b33831df95d472c4128ea85bf079a1d41e32ed0b7d2244c9e"
            ),
            difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
            number: 2,
            gas_limit: MIN_TRANSACTION_GAS,
            gas_used: MIN_TRANSACTION_GAS,
            ..Default::default()
        },
        body: BlockBody {
            transactions: vec![sign_tx_with_key_pair(
                key_pair,
                Transaction::Eip2930(TxEip2930 {
                    chain_id: chain_spec.chain.id(),
                    nonce: 1,
                    gas_limit: MIN_TRANSACTION_GAS,
                    gas_price: 1_500_000_000,
                    to: TxKind::Call(Address::ZERO),
                    value: U256::from(0.1 * ETH_TO_WEI as f64),
                    ..Default::default()
                }),
            )],
            ..Default::default()
        },
    }
    .try_into_recovered()?;

    Ok((block1, block2))
}

pub(crate) fn blocks_and_execution_outputs<N>(
    provider_factory: ProviderFactory<N>,
    chain_spec: Arc<ChainSpec>,
    key_pair: Keypair,
) -> eyre::Result<Vec<(RecoveredBlock<reth_primitives::Block>, BlockExecutionOutput<Receipt>)>>
where
    N: ProviderNodeTypes<
        Primitives: FullNodePrimitives<
            Block = reth_primitives::Block,
            BlockBody = reth_primitives::BlockBody,
            Receipt = reth_primitives::Receipt,
        >,
    >,
{
    let (block1, block2) = blocks(chain_spec.clone(), key_pair)?;

    let block_output1 =
        execute_block_and_commit_to_database(&provider_factory, chain_spec.clone(), &block1)?;
    let block_output2 =
        execute_block_and_commit_to_database(&provider_factory, chain_spec, &block2)?;

    Ok(vec![(block1, block_output1), (block2, block_output2)])
}

pub(crate) fn blocks_and_execution_outcome<N>(
    provider_factory: ProviderFactory<N>,
    chain_spec: Arc<ChainSpec>,
    key_pair: Keypair,
) -> eyre::Result<(Vec<RecoveredBlock<reth_primitives::Block>>, ExecutionOutcome)>
where
    N: ProviderNodeTypes,
    N::Primitives:
        FullNodePrimitives<Block = reth_primitives::Block, Receipt = reth_primitives::Receipt>,
{
    let (block1, block2) = blocks(chain_spec.clone(), key_pair)?;

    let provider = provider_factory.provider()?;

    let executor = EthExecutorProvider::ethereum(chain_spec)
        .batch_executor(StateProviderDatabase::new(LatestStateProviderRef::new(&provider)));

    let mut execution_outcome = executor.execute_and_verify_batch(vec![&block1, &block2])?;
    execution_outcome.state_mut().reverts.sort();

    // Commit the block's execution outcome to the database
    let provider_rw = provider_factory.provider_rw()?;
    provider_rw.append_blocks_with_state(
        vec![block1.clone(), block2.clone()],
        &execution_outcome,
        Default::default(),
        Default::default(),
    )?;
    provider_rw.commit()?;

    Ok((vec![block1, block2], execution_outcome))
}
