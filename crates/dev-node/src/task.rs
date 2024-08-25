use crate::{mode::MiningMode, Storage};
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::{BeaconEngineMessage, ForkchoiceStatus};
use reth_blockchain_tree_api::BlockchainTreeEngine;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::ChainSpec;
use reth_engine_primitives::EngineTypes;
use reth_evm::execute::BlockExecutorProvider;
use reth_primitives::{Block, BlockNumber, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{errors::provider, providers, CanonChainTracker, StateProviderFactory};
use reth_rpc_types::{engine::{ForkchoiceState, PayloadAttributes}, BlockNumberOrTag};
use reth_stages_api::PipelineEvent;
use reth_tokio_util::EventStream;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tracing::{debug, error, instrument::WithSubscriber, span::Attributes, warn};

pub struct DevMiningTask<Client, Pool, Executor, E: EngineTypes + Sized> {
    client: Client,
    pool: Pool,
    executor: Executor,
    payload_builder: PayloadBuilderHandle<E>,
    current_block: BlockNumber,
    tree: &'static dyn BlockchainTreeEngine,
}

impl<Client, Pool, Executor, E> DevMiningTask<Client, Pool, Executor, E>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Executor: BlockExecutorProvider,
    E: EngineTypes,
{
    pub fn new(
        client: Client,
        pool: Pool,
        executor: Executor,
        payload_builder: PayloadBuilderHandle<E>,
        tree: &'static dyn  BlockchainTreeEngine,
    ) -> Self {
        let current_block = client.best_block_number().unwrap_or(0);
        Self { client, pool, executor, payload_builder, current_block, tree }
    }

    async fn create_next_block(&mut self) -> Result<ExecutedBlock, ()> {
        let origin = self.tree.block_by_hash(self.tree.canonical_tip().hash);
        let parent_beacon_block_root = origin.clone().unwrap().parent_beacon_block_root;
        let withdrawals = Some(origin.clone().unwrap().withdrawals.unwrap().to_vec());
        let timestamp = origin.clone().unwrap().timestamp;
        let suggested_fee_recipient = origin.clone().unwrap().beneficiary;
        let prev_randao = origin.unwrap().hash();
    

        let attributes =   //PayloadAttributes {parent_beacon_block_root, withdrawals, timestamp, suggested_fee_recipient, prev_randao};
        // let pending_payload_id = self.payload_builder.send_new_payload(attributes);
        todo!();
    }
}

impl<Client, Pool, Executor, E> futures_util::Stream for DevMiningTask<Client, Pool, Executor, E>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Executor: BlockExecutorProvider,
    E: EngineTypes,
{
    type Item = Result<ExecutedBlock, ()>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let future = self.create_next_block();
        futures_util::pin_mut!(future);
        match future.poll(cx) {
            Poll::Ready(result) => Poll::Ready(Some(result)),
            Poll::Pending => Poll::Pending,
        }
    }
}
