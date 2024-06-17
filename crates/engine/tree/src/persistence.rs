use crate::tree::ExecutedBlock;
use futures::ready;
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::Receiver;

// TODO: design questions:
// * to support "unwinds" or not? ie, what do we do when we get a request to persist a chain that is
// overlapping with the one on disk
/// Writes parts of reth's in memory tree state to the database.
pub struct Persistence<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    // TODO: handles for pushing requests
    /// Incoming requests to persist stuff
    incoming: Receiver<PersistenceAction>,
}

impl<Writer> Persistence<Writer> {
    // TODO: initialization
    /// Writes the cloned tree state to the database
    fn write(&mut self, blocks: Vec<ExecutedBlock>) {
        todo!("implement this")
    }
}

impl<Writer> Persistence<Writer>
where
    Writer: Unpin,
{
    /// Internal method to poll the persistence task. This returns [`None`] if the channel for
    /// incoming [`PersistenceAction`]s is closed.
    #[tracing::instrument(level = "debug", name = "Persistence::poll", skip(self, cx))]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<PersistenceOutput>> {
        let this = self.get_mut();

        // TODO: should we do this?
        // if there are multiple, we can just get the most recent action
        let mut most_recent_save_action = None;
        while let Poll::Ready(action) = this.incoming.poll_recv(cx) {
            let action = match action {
                None => return Poll::Ready(None),
                Some(action) => action,
            };

            // TODO: if we introduce other actions, logic here would be different
            let PersistenceAction::SaveFinalizedBlocks(blocks) = action;
            most_recent_save_action = Some(blocks);
        }

        if let Some(blocks) = most_recent_save_action {
            if blocks.is_empty() {
                todo!("return error or something");
            }
            let last_block_hash = blocks.last().unwrap().block().hash();
            this.write(blocks);
            Poll::Ready(Some(PersistenceOutput::RemoveBlocksBefore(last_block_hash)))
        } else {
            Poll::Pending
        }
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    SaveFinalizedBlocks(Vec<ExecutedBlock>),
}

/// An output of the persistence task, that tells the tree that it needs something.
pub enum PersistenceOutput {
    /// Tells the tree that it can remove the blocks before the given hash, as they have been
    /// persisted.
    RemoveBlocksBefore(B256),
}
