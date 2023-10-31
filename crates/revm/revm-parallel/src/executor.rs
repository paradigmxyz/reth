//! Implementation of parallel executor.

use crate::{
    queue::{BlockQueueStore, TransactionBatch},
    shared::{LockedSharedState, SharedState},
};
use futures::{stream::FuturesOrdered, Future, FutureExt, StreamExt};
use rayon::ThreadPoolBuildError;
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    RethError,
};
use reth_primitives::{
    revm::env::{fill_cfg_and_block_env, fill_tx_env},
    Address, Block, ChainSpec, Hardfork, Receipts, TransactionSigned, B256, U256,
};
use reth_provider::BundleStateWithReceipts;
use revm::{
    primitives::{EVMResult, Env, ResultAndState},
    DatabaseRef, EVM,
};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::oneshot::{self, error::RecvError};

/// Database boxed with a lifetime and Send.
pub type DatabaseRefBox<'a, E> = Box<dyn DatabaseRef<Error = E> + Send + Sync + 'a>;

/// TODO:
#[allow(missing_debug_implementations)]
pub struct ParallelExecutor<'a> {
    /// The configured chain-spec
    chain_spec: Arc<ChainSpec>,
    /// Store for transaction execution order.
    store: BlockQueueStore,
    /// EVM state database.
    pub state: Arc<LockedSharedState<DatabaseRefBox<'a, RethError>>>,

    pool: rayon::ThreadPool,
}

impl<'a> ParallelExecutor<'a> {
    /// Create new parallel executor.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        store: BlockQueueStore,
        database: DatabaseRefBox<'a, RethError>,
        num_threads: Option<usize>,
    ) -> Result<Self, ThreadPoolBuildError> {
        let state = Arc::new(LockedSharedState::new(SharedState::new(database)));
        Ok(Self {
            chain_spec,
            store,
            state,
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads.unwrap_or_else(num_cpus::get))
                .build()?,
        })
    }

    /// Execute a batch of transactions in parallel.
    pub async fn execute_batch(
        &mut self,
        env: &Env,
        batch: &TransactionBatch,
        transactions: &[TransactionSigned],
        senders: &[Address],
    ) -> Result<(), BlockExecutionError> {
        let mut fut_batch = FuturesOrdered::default();
        for tx_idx in batch.iter() {
            let tx_idx = *tx_idx as usize;
            let transaction = transactions.get(tx_idx).unwrap(); // TODO:
            let sender = senders.get(tx_idx).unwrap();
            let mut env = env.clone();
            fill_tx_env(&mut env.tx, transaction, *sender);

            let (tx, rx) = oneshot::channel();
            self.pool.scope(|scope| {
                let state = self.state.clone();
                scope.spawn(move |_scope| {
                    let mut evm = EVM::with_env(env);
                    evm.database(state);
                    let _result = tx.send(evm.transact_ref());
                });
            });
            fut_batch.push_back(TransactionExecutionFut::new(tx_idx, transaction.hash, rx));
        }

        let mut results = Vec::with_capacity(batch.len());
        while let Some((tx_idx, hash, result)) = fut_batch.next().await {
            let ResultAndState { state, result: _result } = result.unwrap().map_err(|e| {
                BlockExecutionError::Validation(BlockValidationError::EVM { hash, error: e.into() })
            })?;
            results.push((tx_idx, state));
        }
        self.state.write().unwrap().commit(results);

        Ok(())
    }

    /// Execute block in parallel.
    pub async fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        let block_queue = self.store.get_queue(block.number).cloned().unwrap(); // TODO:

        // Set state clear flag.
        let state_clear_flag =
            self.chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(block.number);
        self.state.write().unwrap().set_state_clear_flag(state_clear_flag);

        let mut env = Env::default();
        fill_cfg_and_block_env(
            &mut env.cfg,
            &mut env.block,
            &self.chain_spec,
            &block.header,
            total_difficulty,
        );

        for batch in block_queue.iter() {
            self.execute_batch(&env, batch, &block.body, senders.as_ref().unwrap()).await?;
        }

        // TODO: self.state.write().unwrap().merge_transitions(BundleRetention::Reverts);
        Ok(())
    }

    /// TODO:
    pub async fn execute_and_verify_receipt(
        &mut self,
        _block: &Block,
        _total_difficulty: U256,
        _senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError> {
        unimplemented!()
    }

    /// Return the bundle state.
    pub fn take_output_state(&mut self) -> BundleStateWithReceipts {
        let bundle_state = self.state.write().unwrap().take_bundle();
        BundleStateWithReceipts::new(
            bundle_state,
            Receipts::default(), // TODO:
            0,                   // TODO:
        )
    }
}

struct TransactionExecutionFut {
    tx_idx: usize,
    tx_hash: B256,
    rx: oneshot::Receiver<EVMResult<RethError>>,
}

impl TransactionExecutionFut {
    fn new(tx_idx: usize, tx_hash: B256, rx: oneshot::Receiver<EVMResult<RethError>>) -> Self {
        Self { tx_idx, tx_hash, rx }
    }
}

impl Future for TransactionExecutionFut {
    type Output = (usize, B256, Result<EVMResult<RethError>, RecvError>);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        this.rx.poll_unpin(cx).map(|result| (this.tx_idx, this.tx_hash, result))
    }
}
