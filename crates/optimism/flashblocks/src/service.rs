use crate::{
    sequence::FlashBlockSequence,
    worker::{BuildArgs, FlashBlockBuilder},
    ExecutionPayloadBaseV1, FlashBlock,
};
use futures_util::{FutureExt, Stream, StreamExt};
use reth_chain_state::{CanonStateNotification, CanonStateNotifications, CanonStateSubscriptions};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{AlloyBlockHeader, BlockTy, HeaderTy, NodePrimitives, ReceiptTy};
use reth_rpc_eth_types::PendingBlock;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
use tokio::pin;
use tracing::{debug, trace, warn};

/// The `FlashBlockService` maintains an in-memory [`PendingBlock`] built out of a sequence of
/// [`FlashBlock`]s.
#[derive(Debug)]
pub struct FlashBlockService<
    N: NodePrimitives,
    S,
    EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: Unpin>,
    Provider,
> {
    rx: S,
    current: Option<PendingBlock<N>>,
    blocks: FlashBlockSequence<N::SignedTx>,
    rebuild: bool,
    builder: FlashBlockBuilder<EvmConfig, Provider>,
    canon_receiver: CanonStateNotifications<N>,
}

impl<N, S, EvmConfig, Provider> FlashBlockService<N, S, EvmConfig, Provider>
where
    N: NodePrimitives,
    S: Stream<Item = eyre::Result<FlashBlock>> + Unpin,
    EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: From<ExecutionPayloadBaseV1> + Unpin>,
    Provider: StateProviderFactory
        + CanonStateSubscriptions<Primitives = N>
        + BlockReaderIdExt<
            Header = HeaderTy<N>,
            Block = BlockTy<N>,
            Transaction = N::SignedTx,
            Receipt = ReceiptTy<N>,
        > + Unpin,
{
    /// Constructs a new `FlashBlockService` that receives [`FlashBlock`]s from `rx` stream.
    pub fn new(rx: S, evm_config: EvmConfig, provider: Provider) -> Self {
        Self {
            rx,
            current: None,
            blocks: FlashBlockSequence::new(),
            canon_receiver: provider.subscribe_to_canonical_state(),
            builder: FlashBlockBuilder::new(evm_config, provider),
            rebuild: false,
        }
    }

    /// Drives the services and sends new blocks to the receiver
    ///
    /// Note: this should be spawned
    pub async fn run(mut self, tx: tokio::sync::watch::Sender<Option<PendingBlock<N>>>) {
        while let Some(block) = self.next().await {
            if let Ok(block) = block.inspect_err(|e| tracing::error!("{e}")) {
                let _ = tx.send(block).inspect_err(|e| tracing::error!("{e}"));
            }
        }

        warn!("Flashblock service has stopped");
    }

    /// Returns the [`PendingBlock`] made purely out of [`FlashBlock`]s that were received earlier.
    ///
    /// Returns None if the flashblock doesn't attach to the latest header.
    fn execute(&mut self) -> eyre::Result<Option<PendingBlock<N>>> {
        let Some(base) = self.blocks.payload_base() else {
            trace!(
                flashblock_number = ?self.blocks.block_number(),
                count = %self.blocks.count(),
                "Missing flashblock payload base"
            );

            return Ok(None)
        };

        self.builder.execute(BuildArgs { base, transactions: self.blocks.ready_transactions() })
    }

    /// Takes out `current` [`PendingBlock`] if `state` is not preceding it.
    fn on_new_tip(&mut self, state: CanonStateNotification<N>) -> Option<PendingBlock<N>> {
        let latest = state.tip_checked()?.hash();
        self.current.take_if(|current| current.parent_hash() != latest)
    }
}

impl<N, S, EvmConfig, Provider> Stream for FlashBlockService<N, S, EvmConfig, Provider>
where
    N: NodePrimitives,
    S: Stream<Item = eyre::Result<FlashBlock>> + Unpin,
    EvmConfig: ConfigureEvm<Primitives = N, NextBlockEnvCtx: From<ExecutionPayloadBaseV1> + Unpin>,
    Provider: StateProviderFactory
        + CanonStateSubscriptions<Primitives = N>
        + BlockReaderIdExt<
            Header = HeaderTy<N>,
            Block = BlockTy<N>,
            Transaction = N::SignedTx,
            Receipt = ReceiptTy<N>,
        > + Unpin,
{
    type Item = eyre::Result<Option<PendingBlock<N>>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // consume new flashblocks while they're ready
        while let Poll::Ready(Some(result)) = this.rx.poll_next_unpin(cx) {
            match result {
                Ok(flashblock) => match this.blocks.insert(flashblock) {
                    Ok(_) => this.rebuild = true,
                    Err(err) => debug!(%err, "Failed to prepare flashblock"),
                },
                Err(err) => return Poll::Ready(Some(Err(err))),
            }
        }

        if let Poll::Ready(Ok(state)) = {
            let fut = this.canon_receiver.recv();
            pin!(fut);
            fut.poll_unpin(cx)
        } {
            if let Some(current) = this.on_new_tip(state) {
                trace!(
                    parent_hash = %current.block().parent_hash(),
                    block_number = current.block().number(),
                    "Clearing current flashblock on new canonical block"
                );

                return Poll::Ready(Some(Ok(None)))
            }
        }

        if !this.rebuild && this.current.is_some() {
            return Poll::Pending
        }

        let now = Instant::now();
        // try to build a block on top of latest
        match this.execute() {
            Ok(Some(new_pending)) => {
                // built a new pending block
                this.current = Some(new_pending.clone());
                this.rebuild = false;
                trace!(parent_hash=%new_pending.block().parent_hash(), block_number=new_pending.block().number(), flash_blocks=this.blocks.count(), elapsed=?now.elapsed(), "Built new block with flashblocks");
                return Poll::Ready(Some(Ok(Some(new_pending))));
            }
            Ok(None) => {
                // nothing to do because tracked flashblock doesn't attach to latest
            }
            Err(err) => {
                // we can ignore this error
                debug!(%err, "failed to execute flashblock");
            }
        }

        Poll::Pending
    }
}
