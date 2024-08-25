// use futures_util::StreamExt;
// use reth_primitives::{BlockNumHash, SealedBlock};
// use reth_provider::PersistenceProvider;
// use std::{
//     pin::Pin,
//     task::{Context, Poll},
// };

// pub struct DevOrchestrator<Client, Pool, Executor, P, E>
// where
//     E: EngineTypes,
// {
//     mining_task: DevMiningTask<Client, Pool, Executor, E>,
//     persistence: P,
// }

// impl<Client, Pool, Executor, P, E> DevOrchestrator<Client, Pool, Executor, P, E>
// where
//     Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
//     Pool: TransactionPool + Unpin + 'static,
//     Executor: BlockExecutorProvider,
//     P: PersistenceProvider,
//     E: EngineTypes,
// {
//     pub fn new(
//         client: Client,
//         pool: Pool,
//         executor: Executor,
//         persistence: P,
//         payload_builder: PayloadBuilderHandle<E>,
//         tree: BlockchainTreeEngine,
//     ) -> Self {
//         let mining_task = DevMiningTask::new(client, pool, executor, payload_builder, tree);
//         Self { mining_task, persistence }
//     }
// }

// #[async_trait::async_trait]
// impl<Client, Pool, Executor, P, E> Orchestrator for DevOrchestrator<Client, Pool, Executor, P, E>
// where
//     Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
//     Pool: TransactionPool + Unpin + 'static,
//     Executor: BlockExecutorProvider,
//     P: PersistenceProvider,
//     E: EngineTypes,
// {
//     type Error = DevOrchestratorError;

//     async fn orchestrate(&mut self) -> Result<OrchestratorProgress, Self::Error> {
//         match self.mining_task.next().await {
//             Some(Ok(executed_block)) => {
//                 let block = executed_block.block().clone();
//                 self.persistence.insert_block(block.clone(), None, None).await?;

//                 // Update chain state
//                 // This is simplified; you might need more complex logic here
//                 self.persistence.set_canonical_head(block.num_hash())?;

//                 Ok(OrchestratorProgress::BlockAdded(block.num_hash()))
//             }
//             Some(Err(e)) => Err(DevOrchestratorError::MiningError(e)),
//             None => Ok(OrchestratorProgress::Finished),
//         }
//     }

//     async fn shutdown(&mut self) -> Result<(), Self::Error> {
//         // Implement shutdown logic
//         Ok(())
//     }
// }
