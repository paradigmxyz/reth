//! Collection of payload test utilities

use reth_basic_payload_builder::{BuildArguments, PayloadConfig};
use reth_beacon_consensus::BeaconConsensus;
use reth_blockchain_tree::{
    config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree, ShareableBlockchainTree,
};
use reth_db::{
    database::Database,
    tables,
    test_utils::create_test_rw_db,
    transaction::{DbTx, DbTxMut},
    DatabaseEnv, DatabaseError,
};
use reth_interfaces::blockchain_tree::BlockchainTreeViewer;
use reth_payload_builder::{database::CachedReads, PayloadBuilderAttributes};
use reth_primitives::{Bytes, ChainSpec, ChainSpecBuilder};
use reth_provider::{providers::BlockchainProvider, ProviderFactory, StateProviderFactory};
use reth_revm::Factory;
use reth_rpc_types::engine::PayloadAttributes;
use reth_transaction_pool::{noop::NoopTransactionPool, TransactionPool};
use revm::primitives::{BlockEnv, CfgEnv};
use std::sync::Arc;
pub fn get_tx_pool() -> NoopTransactionPool {
    NoopTransactionPool::default()
}

/// Inserts the genesis header information into the specified database.
pub fn commit_genesis_header<DB: Database>(
    db: Arc<DB>,
    chain: Arc<ChainSpec>,
) -> Result<(), DatabaseError> {
    // Insert the genesis header information
    let tx = db.tx_mut()?;
    let header = chain.sealed_genesis_header();
    tx.put::<tables::CanonicalHeaders>(0, header.hash)?;
    tx.put::<tables::HeaderNumbers>(header.hash, 0)?;
    tx.put::<tables::BlockBodyIndices>(0, Default::default())?;
    tx.put::<tables::HeaderTD>(0, header.difficulty.into())?;
    tx.put::<tables::Headers>(0, header.header)?;
    tx.commit()?;
    Ok(())
}

/// Constructs a test chain spec.
pub fn construct_chain_spec() -> Arc<ChainSpec> {
    Arc::new(ChainSpecBuilder::mainnet().build())
}

/// Constructs a test database.
pub fn construct_db(chain: Arc<ChainSpec>) -> Result<Arc<DatabaseEnv>, DatabaseError> {
    let db = create_test_rw_db();
    commit_genesis_header(db.clone(), chain)?;
    Ok(db)
}

/// Construct default test build args.
pub fn build_provider_factory(
    db: Arc<DatabaseEnv>,
    chain: Arc<ChainSpec>,
) -> ProviderFactory<Arc<DatabaseEnv>> {
    ProviderFactory::new(db, chain)
}

/// Constructs a blockchain tree.
pub fn build_tree(
    db: Arc<DatabaseEnv>,
    chain: Arc<ChainSpec>,
) -> ShareableBlockchainTree<Arc<DatabaseEnv>, Arc<BeaconConsensus>, Factory> {
    let consensus = Arc::new(BeaconConsensus::new(Arc::clone(&chain)));
    let tree_externals = TreeExternals::new(
        db.clone(),
        Arc::clone(&consensus),
        Factory::new(chain.clone()),
        Arc::clone(&chain),
    );
    let tree_config = BlockchainTreeConfig::default();
    let (canon_state_notification_sender, _receiver) =
        tokio::sync::broadcast::channel(tree_config.max_reorg_depth() as usize * 2);

    ShareableBlockchainTree::new(
        BlockchainTree::new(tree_externals, canon_state_notification_sender.clone(), tree_config)
            .unwrap(),
    )
}

/// Constructs a test [StateProviderFactory] for building payload [BuildArguments].
pub fn get_test_client<Tree: BlockchainTreeViewer>(
    database: ProviderFactory<Arc<DatabaseEnv>>,
    tree: Tree,
) -> reth_interfaces::Result<BlockchainProvider<Arc<DatabaseEnv>, Tree>> {
    BlockchainProvider::new(database, tree)
}

/// Construct test build args for the payload builder.
pub fn get_build_args<C, P>(client: C, pool: P) -> BuildArguments<P, C>
where
    C: StateProviderFactory,
    P: TransactionPool,
{
    BuildArguments {
        client,
        pool,
        cached_reads: CachedReads::default(),
        config: PayloadConfig {
            initialized_block_env: BlockEnv { ..Default::default() },
            initialized_cfg: CfgEnv { ..Default::default() },
            parent_block: Arc::new(Default::default()),
            extra_data: Bytes::default().into(),
            attributes: PayloadBuilderAttributes::new(
                Default::default(),
                PayloadAttributes {
                    timestamp: Default::default(),
                    prev_randao: Default::default(),
                    suggested_fee_recipient: Default::default(),
                    withdrawals: None,
                    parent_beacon_block_root: None,
                },
            ),
            chain_spec: Arc::new(ChainSpec::default()),
        },
        cancel: Default::default(),
        best_payload: None,
    }
}
