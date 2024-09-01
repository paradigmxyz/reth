use futures_util::StreamExt;
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_engine_primitives::{EngineTypes, PayloadTypes};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::{BlockNumHash, SealedBlock};
use reth_provider::{CanonChainTracker, ProviderFactory, StateProviderFactory};
use reth_rpc_types::engine::PayloadAttributes;
use reth_transaction_pool::TransactionPool;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use reth_evm::execute::BlockExecutorProvider;

use crate::task::DevMiningTask;

pub struct DevOrchestrator<Client, Pool, Executor, P, E>
where
    E: EngineTypes,
{
    mining_task: DevMiningTask<Client, Pool, Executor, E>,
    persistence: P,
}

pub enum OrchestratorProgress {
    Finished,
    InProgress,
    BlockAdded(BlockNumHash)
}


impl<Client, Pool, Executor, P, E> DevOrchestrator<Client, Pool, Executor, P, E>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Executor: BlockExecutorProvider,
    P: StateProviderFactory,
    E: EngineTypes + PayloadTypes<PayloadBuilderAttributes = PayloadAttributes> ,
    reth_primitives::Block: From<<E as PayloadTypes>::BuiltPayload>
{
    pub fn new(
        client: Client,
        pool: Pool,
        executor: Executor,
        persistence: P,
        payload_builder: PayloadBuilderHandle<E>,
        tree:&'static dyn BlockchainTreeEngine,
    ) -> Self {
        let mining_task = DevMiningTask::new(client, pool, executor, payload_builder, tree);
        Self { mining_task, persistence }
    }
}


impl<Client, Pool, Executor, P, E> DevOrchestrator<Client, Pool, Executor, P, E>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Executor: BlockExecutorProvider,
    P: StateProviderFactory,
    E: EngineTypes,
{

    async fn orchestrate(&mut self) -> Result<OrchestratorProgress, ()> {
        // match self.mining_task.next().await {
        //     // Some(Ok(executed_block)) => {
        //     //     let block = executed_block.block().clone();
     
        //     // Some(Err(e)) => Err(e),
        //     // None => Ok(OrchestratorProgress::Finished),
        //     todo!()
        // }
        todo!()
    }

    async fn shutdown(&mut self) -> Result<(), ()> {
        // Implement shutdown logic
        todo!()
    }
}
