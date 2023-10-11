//! Utils for testing purposes.

use crate::{
    error::PayloadBuilderError, traits::KeepPayloadJobAlive, BuiltPayload,
    PayloadBuilderAttributes, PayloadBuilderHandle, PayloadBuilderService, PayloadJob,
    PayloadJobGenerator,
};
use reth_primitives::{Block, U256,ChainSpec};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};


/// Creates a new [PayloadBuilderService] for testing purposes.
pub fn test_payload_service(
) -> (PayloadBuilderService<TestPayloadJobGenerator>, PayloadBuilderHandle) {
    PayloadBuilderService::new(Default::default())
}

/// Creates a new [PayloadBuilderService] for testing purposes and spawns it in the background.
pub fn spawn_test_payload_service() -> PayloadBuilderHandle {
    let (service, handle) = test_payload_service();
    tokio::spawn(service);
    handle
}

/// A [PayloadJobGenerator] for testing purposes
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct TestPayloadJobGenerator;

impl PayloadJobGenerator for TestPayloadJobGenerator {
    type Job = TestPayloadJob;

    fn new_payload_job(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> Result<Self::Job, PayloadBuilderError> {
        Ok(TestPayloadJob { attr })
    }
}

/// A [PayloadJobGenerator] for testing purposes
#[derive(Debug)]
pub struct TestPayloadJob {
    attr: PayloadBuilderAttributes,
}

impl Future for TestPayloadJob {
    type Output = Result<(), PayloadBuilderError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl PayloadJob for TestPayloadJob {
    type ResolvePayloadFuture =
        futures_util::future::Ready<Result<Arc<BuiltPayload>, PayloadBuilderError>>;

    fn best_payload(&self) -> Result<Arc<BuiltPayload>, PayloadBuilderError> {
        Ok(Arc::new(BuiltPayload::new(
            self.attr.payload_id(),
            Block::default().seal_slow(),
            U256::ZERO,
        )))
    }

    fn resolve(&mut self) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
        let fut = futures_util::future::ready(self.best_payload());
        (fut, KeepPayloadJobAlive::No)
    }
}

/// Collection of payload test utilities
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils {
    use super::*;

    use reth_beacon_consensus::BeaconConsensus;
    use reth_blockchain_tree::{
        config::BlockchainTreeConfig, externals::TreeExternals, BlockchainTree,
        ShareableBlockchainTree,
    };
    use reth_db::{
        database::Database,
        tables,
        test_utils::create_test_rw_db,
        transaction::{DbTx, DbTxMut},
        DatabaseEnv, DatabaseError,
    };
    use reth_interfaces::blockchain_tree::BlockchainTreeViewer;
    use reth_primitives::ChainSpecBuilder;
    use reth_provider::{providers::BlockchainProvider, ProviderFactory};
    use reth_revm::Factory;
    use reth_rpc_types::engine::PayloadAttributes;
    use reth_transaction_pool::noop::NoopTransactionPool;


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
        BlockchainTree::new(
            tree_externals,
            canon_state_notification_sender.clone(),
            tree_config,
        )
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
            extra_data: Bytes::default(),
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
}


#[cfg(test)]
mod test {
use super::*;

#[test]
fn test_default_payload_builder() {
    let pool = test_utils::get_tx_pool();
    let chain = test_utils::construct_chain_spec();
    let db = test_utils::construct_db(chain.clone()).unwrap();

    let provider = test_utils::build_provider_factory(db.clone(), chain.clone());
    let tree = test_utils::build_tree(db, chain.clone());
    let client = test_utils::get_test_client(provider, tree).unwrap();

    // {
    //     let parent_block: SealedBlock = Default::default();
    //     println!("Fetching state for block hash: {:?}", parent_block.hash);
    //     let state = client.state_by_block_hash(parent_block.hash).unwrap();
    //     // println!("state: {:?}", state);
    // }

    let args = test_utils::get_build_args(client, pool);

    let build_result = default_payload_builder(args);
    println!("build_result: {:?}", build_result);

    let err = build_result.err().unwrap();
    println!("PayloadBuilderError: {:?}", err);
    // assert_eq!(err.kind(), Some(PayloadBuilderError::SystemTransactionPostRegolith));
}
}


