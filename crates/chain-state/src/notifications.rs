//! Canonical chain state notification trait and types.

use alloy_eips::eip2718::Encodable2718;
use derive_more::{Deref, DerefMut};
use reth_execution_types::{BlockReceipts, Chain};
use reth_primitives::{NodePrimitives, SealedBlockWithSenders, SealedHeader};
use reth_storage_api::NodePrimitivesProvider;
use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{broadcast, watch};
use tokio_stream::{
    wrappers::{BroadcastStream, WatchStream},
    Stream,
};
use tracing::debug;

/// Type alias for a receiver that receives [`CanonStateNotification`]
pub type CanonStateNotifications<N = reth_primitives::EthPrimitives> =
    broadcast::Receiver<CanonStateNotification<N>>;

/// Type alias for a sender that sends [`CanonStateNotification`]
pub type CanonStateNotificationSender<N = reth_primitives::EthPrimitives> =
    broadcast::Sender<CanonStateNotification<N>>;

/// A type that allows to register chain related event subscriptions.
pub trait CanonStateSubscriptions: NodePrimitivesProvider + Send + Sync {
    /// Get notified when a new canonical chain was imported.
    ///
    /// A canonical chain be one or more blocks, a reorg or a revert.
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<Self::Primitives>;

    /// Convenience method to get a stream of [`CanonStateNotification`].
    fn canonical_state_stream(&self) -> CanonStateNotificationStream<Self::Primitives> {
        CanonStateNotificationStream {
            st: BroadcastStream::new(self.subscribe_to_canonical_state()),
        }
    }
}

impl<T: CanonStateSubscriptions> CanonStateSubscriptions for &T {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications<Self::Primitives> {
        (*self).subscribe_to_canonical_state()
    }

    fn canonical_state_stream(&self) -> CanonStateNotificationStream<Self::Primitives> {
        (*self).canonical_state_stream()
    }
}

/// A Stream of [`CanonStateNotification`].
#[derive(Debug)]
#[pin_project::pin_project]
pub struct CanonStateNotificationStream<N: NodePrimitives = reth_primitives::EthPrimitives> {
    #[pin]
    st: BroadcastStream<CanonStateNotification<N>>,
}

impl<N: NodePrimitives> Stream for CanonStateNotificationStream<N> {
    type Item = CanonStateNotification<N>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Ok(notification)) => Poll::Ready(Some(notification)),
                Some(Err(err)) => {
                    debug!(%err, "canonical state notification stream lagging behind");
                    continue
                }
                None => Poll::Ready(None),
            }
        }
    }
}

/// A notification that is sent when a new block is imported, or an old block is reverted.
///
/// The notification contains at least one [`Chain`] with the imported segment. If some blocks were
/// reverted (e.g. during a reorg), the old chain is also returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonStateNotification<N: NodePrimitives = reth_primitives::EthPrimitives> {
    /// The canonical chain was extended.
    Commit {
        /// The newly added chain segment.
        new: Arc<Chain<N>>,
    },
    /// A chain segment was reverted or reorged.
    ///
    /// - In the case of a reorg, the reverted blocks are present in `old`, and the new blocks are
    ///   present in `new`.
    /// - In the case of a revert, the reverted blocks are present in `old`, and `new` is an empty
    ///   chain segment.
    Reorg {
        /// The chain segment that was reverted.
        old: Arc<Chain<N>>,
        /// The chain segment that was added on top of the canonical chain, minus the reverted
        /// blocks.
        ///
        /// In the case of a revert, not a reorg, this chain segment is empty.
        new: Arc<Chain<N>>,
    },
}

impl<N: NodePrimitives> CanonStateNotification<N> {
    /// Get the chain segment that was reverted, if any.
    pub fn reverted(&self) -> Option<Arc<Chain<N>>> {
        match self {
            Self::Commit { .. } => None,
            Self::Reorg { old, .. } => Some(old.clone()),
        }
    }

    /// Get the newly imported chain segment, if any.
    pub fn committed(&self) -> Arc<Chain<N>> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.clone(),
        }
    }

    /// Get the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &SealedBlockWithSenders<N::Block> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.tip(),
        }
    }

    /// Get receipts in the reverted and newly imported chain segments with their corresponding
    /// block numbers and transaction hashes.
    ///
    /// The boolean in the tuple (2nd element) denotes whether the receipt was from the reverted
    /// chain segment.
    pub fn block_receipts(&self) -> Vec<(BlockReceipts<N::Receipt>, bool)>
    where
        N::SignedTx: Encodable2718,
    {
        let mut receipts = Vec::new();

        // get old receipts
        if let Some(old) = self.reverted() {
            receipts
                .extend(old.receipts_with_attachment().into_iter().map(|receipt| (receipt, true)));
        }
        // get new receipts
        receipts.extend(
            self.committed().receipts_with_attachment().into_iter().map(|receipt| (receipt, false)),
        );
        receipts
    }
}

/// Wrapper around a broadcast receiver that receives fork choice notifications.
#[derive(Debug, Deref, DerefMut)]
pub struct ForkChoiceNotifications(pub watch::Receiver<Option<SealedHeader>>);

/// A trait that allows to register to fork choice related events
/// and get notified when a new fork choice is available.
pub trait ForkChoiceSubscriptions: Send + Sync {
    /// Get notified when a new safe block of the chain is selected.
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications;

    /// Get notified when a new finalized block of the chain is selected.
    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications;

    /// Convenience method to get a stream of the new safe blocks of the chain.
    fn safe_block_stream(&self) -> ForkChoiceStream<SealedHeader> {
        ForkChoiceStream::new(self.subscribe_safe_block().0)
    }

    /// Convenience method to get a stream of the new finalized blocks of the chain.
    fn finalized_block_stream(&self) -> ForkChoiceStream<SealedHeader> {
        ForkChoiceStream::new(self.subscribe_finalized_block().0)
    }
}

/// A stream for fork choice watch channels (pending, safe or finalized watchers)
#[derive(Debug)]
#[pin_project::pin_project]
pub struct ForkChoiceStream<T> {
    #[pin]
    st: WatchStream<Option<T>>,
}

impl<T: Clone + Sync + Send + 'static> ForkChoiceStream<T> {
    /// Creates a new `ForkChoiceStream`
    pub fn new(rx: watch::Receiver<Option<T>>) -> Self {
        Self { st: WatchStream::from_changes(rx) }
    }
}

impl<T: Clone + Sync + Send + 'static> Stream for ForkChoiceStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Some(notification)) => return Poll::Ready(Some(notification)),
                Some(None) => continue,
                None => return Poll::Ready(None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, B256};
    use reth_execution_types::ExecutionOutcome;
    use reth_primitives::{Receipt, Receipts, TransactionSigned, TxType};

    #[test]
    fn test_commit_notification() {
        let block: SealedBlockWithSenders = Default::default();
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);

        let mut block1 = block.clone();
        block1.set_block_number(1);
        block1.set_hash(block1_hash);

        let mut block2 = block;
        block2.set_block_number(2);
        block2.set_hash(block2_hash);

        let chain: Arc<Chain> = Arc::new(Chain::new(
            vec![block1.clone(), block2.clone()],
            ExecutionOutcome::default(),
            None,
        ));

        // Create a commit notification
        let notification = CanonStateNotification::Commit { new: chain.clone() };

        // Test that `committed` returns the correct chain
        assert_eq!(notification.committed(), chain);

        // Test that `reverted` returns None for `Commit`
        assert!(notification.reverted().is_none());

        // Test that `tip` returns the correct block
        assert_eq!(*notification.tip(), block2);
    }

    #[test]
    fn test_reorg_notification() {
        let block: SealedBlockWithSenders = Default::default();
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);
        let block3_hash = B256::new([0x03; 32]);

        let mut block1 = block.clone();
        block1.set_block_number(1);
        block1.set_hash(block1_hash);

        let mut block2 = block.clone();
        block2.set_block_number(2);
        block2.set_hash(block2_hash);

        let mut block3 = block;
        block3.set_block_number(3);
        block3.set_hash(block3_hash);

        let old_chain: Arc<Chain> =
            Arc::new(Chain::new(vec![block1.clone()], ExecutionOutcome::default(), None));
        let new_chain = Arc::new(Chain::new(
            vec![block2.clone(), block3.clone()],
            ExecutionOutcome::default(),
            None,
        ));

        // Create a reorg notification
        let notification =
            CanonStateNotification::Reorg { old: old_chain.clone(), new: new_chain.clone() };

        // Test that `reverted` returns the old chain
        assert_eq!(notification.reverted(), Some(old_chain));

        // Test that `committed` returns the new chain
        assert_eq!(notification.committed(), new_chain);

        // Test that `tip` returns the tip of the new chain (last block in the new chain)
        assert_eq!(*notification.tip(), block3);
    }

    #[test]
    fn test_block_receipts_commit() {
        // Create a default block instance for use in block definitions.
        let block: SealedBlockWithSenders = Default::default();

        // Define unique hashes for two blocks to differentiate them in the chain.
        let block1_hash = B256::new([0x01; 32]);
        let block2_hash = B256::new([0x02; 32]);

        // Create a default transaction to include in block1's transactions.
        let tx = TransactionSigned::default();

        // Create a clone of the default block and customize it to act as block1.
        let mut block1 = block.clone();
        block1.set_block_number(1);
        block1.set_hash(block1_hash);
        // Add the transaction to block1's transactions.
        block1.block.body.transactions.push(tx);

        // Clone the default block and customize it to act as block2.
        let mut block2 = block;
        block2.set_block_number(2);
        block2.set_hash(block2_hash);

        // Create a receipt for the transaction in block1.
        #[allow(clippy::needless_update)]
        let receipt1 = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 12345,
            logs: vec![],
            success: true,
            ..Default::default()
        };

        // Wrap the receipt in a `Receipts` structure, as expected in the `ExecutionOutcome`.
        let receipts = Receipts { receipt_vec: vec![vec![Some(receipt1.clone())]] };

        // Define an `ExecutionOutcome` with the created receipts.
        let execution_outcome = ExecutionOutcome { receipts, ..Default::default() };

        // Create a new chain segment with `block1` and `block2` and the execution outcome.
        let new_chain: Arc<Chain> =
            Arc::new(Chain::new(vec![block1.clone(), block2.clone()], execution_outcome, None));

        // Create a commit notification containing the new chain segment.
        let notification = CanonStateNotification::Commit { new: new_chain };

        // Call `block_receipts` on the commit notification to retrieve block receipts.
        let block_receipts = notification.block_receipts();

        // Assert that only one receipt entry exists in the `block_receipts` list.
        assert_eq!(block_receipts.len(), 1);

        // Verify that the first entry matches block1's hash and transaction receipt.
        assert_eq!(
            block_receipts[0].0,
            BlockReceipts {
                block: block1.num_hash(),
                tx_receipts: vec![(
                    // Transaction hash of a Transaction::default()
                    b256!("20b5378c6fe992c118b557d2f8e8bbe0b7567f6fe5483a8f0f1c51e93a9d91ab"),
                    receipt1
                )]
            }
        );

        // Assert that the receipt is from the committed segment (not reverted).
        assert!(!block_receipts[0].1);
    }

    #[test]
    fn test_block_receipts_reorg() {
        // Define block1 for the old chain segment, which will be reverted.
        let mut old_block1: SealedBlockWithSenders = Default::default();
        old_block1.set_block_number(1);
        old_block1.set_hash(B256::new([0x01; 32]));
        old_block1.block.body.transactions.push(TransactionSigned::default());

        // Create a receipt for a transaction in the reverted block.
        #[allow(clippy::needless_update)]
        let old_receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 54321,
            logs: vec![],
            success: false,
            ..Default::default()
        };
        let old_receipts = Receipts { receipt_vec: vec![vec![Some(old_receipt.clone())]] };

        let old_execution_outcome =
            ExecutionOutcome { receipts: old_receipts, ..Default::default() };

        // Create an old chain segment to be reverted, containing `old_block1`.
        let old_chain: Arc<Chain> =
            Arc::new(Chain::new(vec![old_block1.clone()], old_execution_outcome, None));

        // Define block2 for the new chain segment, which will be committed.
        let mut new_block1: SealedBlockWithSenders = Default::default();
        new_block1.set_block_number(2);
        new_block1.set_hash(B256::new([0x02; 32]));
        new_block1.block.body.transactions.push(TransactionSigned::default());

        // Create a receipt for a transaction in the new committed block.
        #[allow(clippy::needless_update)]
        let new_receipt = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 12345,
            logs: vec![],
            success: true,
            ..Default::default()
        };
        let new_receipts = Receipts { receipt_vec: vec![vec![Some(new_receipt.clone())]] };

        let new_execution_outcome =
            ExecutionOutcome { receipts: new_receipts, ..Default::default() };

        // Create a new chain segment to be committed, containing `new_block1`.
        let new_chain = Arc::new(Chain::new(vec![new_block1.clone()], new_execution_outcome, None));

        // Create a reorg notification with both reverted (old) and committed (new) chain segments.
        let notification = CanonStateNotification::Reorg { old: old_chain, new: new_chain };

        // Retrieve receipts from both old (reverted) and new (committed) segments.
        let block_receipts = notification.block_receipts();

        // Assert there are two receipt entries, one from each chain segment.
        assert_eq!(block_receipts.len(), 2);

        // Verify that the first entry matches old_block1 and its receipt from the reverted segment.
        assert_eq!(
            block_receipts[0].0,
            BlockReceipts {
                block: old_block1.num_hash(),
                tx_receipts: vec![(
                    // Transaction hash of a Transaction::default()
                    b256!("20b5378c6fe992c118b557d2f8e8bbe0b7567f6fe5483a8f0f1c51e93a9d91ab"),
                    old_receipt
                )]
            }
        );
        // Confirm this is from the reverted segment.
        assert!(block_receipts[0].1);

        // Verify that the second entry matches new_block1 and its receipt from the committed
        // segment.
        assert_eq!(
            block_receipts[1].0,
            BlockReceipts {
                block: new_block1.num_hash(),
                tx_receipts: vec![(
                    // Transaction hash of a Transaction::default()
                    b256!("20b5378c6fe992c118b557d2f8e8bbe0b7567f6fe5483a8f0f1c51e93a9d91ab"),
                    new_receipt
                )]
            }
        );
        // Confirm this is from the committed segment.
        assert!(!block_receipts[1].1);
    }
}
