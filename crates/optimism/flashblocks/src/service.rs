use crate::FlashBlock;
use futures_util::{Stream, StreamExt};
use reth_chain_state::ExecutedBlock;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tracing::{debug, error, info};

/// The `FlashBlockService` maintains an in-memory [`ExecutedBlock`] built out of a sequence of
/// [`FlashBlock`]s.
#[derive(Debug)]
pub struct FlashBlockService<N: NodePrimitives, S> {
    rx: S,
    current: Option<ExecutedBlock<N>>,
    blocks: Vec<FlashBlock>,
}

impl<N: NodePrimitives, S> FlashBlockService<N, S> {
    /// Adds the `block` into the collection.
    ///
    /// Depending on its index and associated block number, it may:
    /// * Be added to all the flashblocks received prior using this function.
    /// * Cause a reset of the flashblocks and become the sole member of the collection.
    /// * Be ignored.
    pub fn add_flash_block(&mut self, flashblock: FlashBlock) {
        // Flash block at index zero resets the whole state
        if flashblock.index == 0 {
            self.blocks = vec![flashblock];
            self.current.take();
        }
        // Flash block at the following index adds to the collection and invalidates built block
        else if flashblock.index == self.blocks.last().map(|last| last.index + 1).unwrap_or(0) {
            self.blocks.push(flashblock);
            self.current.take();
        }
        // Flash block at a different index is ignored
        else if let Some(pending_block) = self.current.as_ref() {
            // Delete built block if it corresponds to a different height
            if pending_block.block_number() == flashblock.metadata.block_number {
                info!(
                    message = "None sequential Flashblocks, keeping cache",
                    curr_block = %pending_block.block_number(),
                    new_block = %flashblock.metadata.block_number,
                );
            } else {
                error!(
                    message = "Received Flashblock for new block, zeroing Flashblocks until we receive a base Flashblock",
                    curr_block = %pending_block.recovered_block().header().number(),
                    new_block = %flashblock.metadata.block_number,
                );

                self.blocks.clear();
                self.current.take();
            }
        } else {
            debug!("ignoring {flashblock:?}");
        }
    }

    /// Returns the [`ExecutedBlock`] made purely out of [`FlashBlock`]s that were received using
    /// [`Self::add_flash_block`].
    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    pub fn execute(&mut self) -> eyre::Result<ExecutedBlock<N>> {
        todo!()
    }
}

impl<N: NodePrimitives, S: Stream<Item = FlashBlock> + Unpin> Stream for FlashBlockService<N, S> {
    type Item = eyre::Result<ExecutedBlock<N>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.rx.poll_next_unpin(cx) {
                Poll::Ready(Some(flashblock)) => self.add_flash_block(flashblock),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Ready(Some(self.execute())),
            }
        }
    }
}
