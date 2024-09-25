use crate::{BackfillJobFactory, ExExNotification, StreamBackfillJob};
use alloy_primitives::U256;
use eyre::OptionExt;
use futures::{Stream, StreamExt};
use reth_chainspec::Head;
use reth_evm::execute::BlockExecutorProvider;
use reth_exex_types::ExExHead;
use reth_provider::{BlockReader, Chain, HeaderProvider, StateProviderFactory};
use reth_tracing::tracing::debug;
use std::{
    fmt::Debug,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::Receiver;

/// A stream of [`ExExNotification`]s. The stream will emit notifications for all blocks.
pub struct ExExNotifications<P, E> {
    node_head: Head,
    provider: P,
    executor: E,
    notifications: Receiver<ExExNotification>,
}

impl<P: Debug, E: Debug> Debug for ExExNotifications<P, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExNotifications")
            .field("provider", &self.provider)
            .field("executor", &self.executor)
            .field("notifications", &self.notifications)
            .finish()
    }
}

impl<P, E> ExExNotifications<P, E> {
    /// Creates a new instance of [`ExExNotifications`].
    pub const fn new(
        node_head: Head,
        provider: P,
        executor: E,
        notifications: Receiver<ExExNotification>,
    ) -> Self {
        Self { node_head, provider, executor, notifications }
    }

    /// Receives the next value for this receiver.
    ///
    /// This method returns `None` if the channel has been closed and there are
    /// no remaining messages in the channel's buffer. This indicates that no
    /// further values can ever be received from this `Receiver`. The channel is
    /// closed when all senders have been dropped, or when [`Receiver::close`] is called.
    ///
    /// # Cancel safety
    ///
    /// This method is cancel safe. If `recv` is used as the event in a
    /// [`tokio::select!`] statement and some other branch
    /// completes first, it is guaranteed that no messages were received on this
    /// channel.
    ///
    /// For full documentation, see [`Receiver::recv`].
    #[deprecated(note = "use `ExExNotifications::next` and its `Stream` implementation instead")]
    pub async fn recv(&mut self) -> Option<ExExNotification> {
        self.notifications.recv().await
    }

    /// Polls to receive the next message on this channel.
    ///
    /// This method returns:
    ///
    ///  * `Poll::Pending` if no messages are available but the channel is not closed, or if a
    ///    spurious failure happens.
    ///  * `Poll::Ready(Some(message))` if a message is available.
    ///  * `Poll::Ready(None)` if the channel has been closed and all messages sent before it was
    ///    closed have been received.
    ///
    /// When the method returns `Poll::Pending`, the `Waker` in the provided
    /// `Context` is scheduled to receive a wakeup when a message is sent on any
    /// receiver, or when the channel is closed.  Note that on multiple calls to
    /// `poll_recv` or `poll_recv_many`, only the `Waker` from the `Context`
    /// passed to the most recent call is scheduled to receive a wakeup.
    ///
    /// If this method returns `Poll::Pending` due to a spurious failure, then
    /// the `Waker` will be notified when the situation causing the spurious
    /// failure has been resolved. Note that receiving such a wakeup does not
    /// guarantee that the next call will succeed — it could fail with another
    /// spurious failure.
    ///
    /// For full documentation, see [`Receiver::poll_recv`].
    #[deprecated(
        note = "use `ExExNotifications::poll_next` and its `Stream` implementation instead"
    )]
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<ExExNotification>> {
        self.notifications.poll_recv(cx)
    }
}

impl<P, E> ExExNotifications<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    /// Subscribe to notifications with the given head. This head is the ExEx's
    /// latest view of the host chain.
    ///
    /// Notifications will be sent starting from the head, not inclusive. For
    /// example, if `head.number == 10`, then the first notification will be
    /// with `block.number == 11`. A `head.number` of 10 indicates that the ExEx
    /// has processed up to block 10, and is ready to process block 11.
    pub fn with_head(self, head: ExExHead) -> ExExNotificationsWithHead<P, E> {
        ExExNotificationsWithHead::new(
            self.node_head,
            self.provider,
            self.executor,
            self.notifications,
            head,
        )
    }
}

impl<P: Unpin, E: Unpin> Stream for ExExNotifications<P, E> {
    type Item = ExExNotification;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().notifications.poll_recv(cx)
    }
}

/// A stream of [`ExExNotification`]s. The stream will only emit notifications for blocks that are
/// committed or reverted after the given head.
#[derive(Debug)]
pub struct ExExNotificationsWithHead<P, E> {
    node_head: Head,
    provider: P,
    executor: E,
    notifications: Receiver<ExExNotification>,
    exex_head: ExExHead,
    pending_sync: bool,
    /// The backfill job to run before consuming any notifications.
    backfill_job: Option<StreamBackfillJob<E, P, Chain>>,
    /// Whether we're currently waiting for the node head to catch up to the same height as the
    /// ExEx head.
    node_head_catchup_in_progress: bool,
}

impl<P, E> ExExNotificationsWithHead<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    /// Creates a new [`ExExNotificationsWithHead`].
    pub const fn new(
        node_head: Head,
        provider: P,
        executor: E,
        notifications: Receiver<ExExNotification>,
        exex_head: ExExHead,
    ) -> Self {
        Self {
            node_head,
            provider,
            executor,
            notifications,
            exex_head,
            pending_sync: true,
            backfill_job: None,
            node_head_catchup_in_progress: false,
        }
    }

    /// Compares the node head against the ExEx head, and synchronizes them in case of a mismatch.
    ///
    /// Possible situations are:
    /// - ExEx is behind the node head (`node_head.number < exex_head.number`).
    ///   - ExEx is on the canonical chain (`exex_head.hash` is found in the node database).
    ///     Backfill from the node database.
    ///   - ExEx is not on the canonical chain (`exex_head.hash` is not found in the node database).
    ///     Unwind the ExEx to the first block matching between the ExEx and the node, and then
    ///     bacfkill from the node database.
    /// - ExEx is at the same block number (`node_head.number == exex_head.number`).
    ///   - ExEx is on the canonical chain (`exex_head.hash` is found in the node database). Nothing
    ///     to do.
    ///   - ExEx is not on the canonical chain (`exex_head.hash` is not found in the node database).
    ///     Unwind the ExEx to the first block matching between the ExEx and the node, and then
    ///     backfill from the node database.
    /// - ExEx is ahead of the node head (`node_head.number > exex_head.number`). Wait until the
    ///   node head catches up to the ExEx head, and then repeat the synchronization process.
    fn synchronize(&mut self) -> eyre::Result<()> {
        debug!(target: "exex::manager", "Synchronizing ExEx head");

        let backfill_job_factory =
            BackfillJobFactory::new(self.executor.clone(), self.provider.clone());
        match self.exex_head.block.number.cmp(&self.node_head.number) {
            std::cmp::Ordering::Less => {
                // ExEx is behind the node head

                if let Some(exex_header) = self.provider.header(&self.exex_head.block.hash)? {
                    // ExEx is on the canonical chain
                    debug!(target: "exex::manager", "ExEx is behind the node head and on the canonical chain");

                    if exex_header.number != self.exex_head.block.number {
                        eyre::bail!("ExEx head number does not match the hash")
                    }

                    // ExEx is on the canonical chain, start backfill
                    let backfill = backfill_job_factory
                        .backfill(self.exex_head.block.number + 1..=self.node_head.number)
                        .into_stream();
                    self.backfill_job = Some(backfill);
                } else {
                    debug!(target: "exex::manager", "ExEx is behind the node head and not on the canonical chain");
                    // ExEx is not on the canonical chain, first unwind it and then backfill

                    // TODO(alexey): unwind and backfill
                    self.backfill_job = None;
                }
            }
            #[allow(clippy::branches_sharing_code)]
            std::cmp::Ordering::Equal => {
                // ExEx is at the same block height as the node head

                if let Some(exex_header) = self.provider.header(&self.exex_head.block.hash)? {
                    // ExEx is on the canonical chain
                    debug!(target: "exex::manager", "ExEx is at the same block height as the node head and on the canonical chain");

                    if exex_header.number != self.exex_head.block.number {
                        eyre::bail!("ExEx head number does not match the hash")
                    }

                    // ExEx is on the canonical chain and the same as the node head, no need to
                    // backfill
                    self.backfill_job = None;
                } else {
                    // ExEx is not on the canonical chain, first unwind it and then backfill
                    debug!(target: "exex::manager", "ExEx is at the same block height as the node head but not on the canonical chain");

                    // TODO(alexey): unwind and backfill
                    self.backfill_job = None;
                }
            }
            std::cmp::Ordering::Greater => {
                debug!(target: "exex::manager", "ExEx is ahead of the node head");

                // ExEx is ahead of the node head

                // TODO(alexey): wait until the node head is at the same height as the ExEx head
                // and then repeat the process above
                self.node_head_catchup_in_progress = true;
            }
        };

        Ok(())
    }
}

impl<P, E> Stream for ExExNotificationsWithHead<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: BlockExecutorProvider + Clone + Unpin + 'static,
{
    type Item = eyre::Result<ExExNotification>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.pending_sync {
            this.synchronize()?;
            this.pending_sync = false;
        }

        if let Some(backfill_job) = &mut this.backfill_job {
            if let Some(chain) = ready!(backfill_job.poll_next_unpin(cx)) {
                return Poll::Ready(Some(Ok(ExExNotification::ChainCommitted {
                    new: Arc::new(chain?),
                })))
            }

            // Backfill job is done, remove it
            this.backfill_job = None;
        }

        loop {
            let Some(notification) = ready!(this.notifications.poll_recv(cx)) else {
                return Poll::Ready(None)
            };

            // 1. Either committed or reverted chain from the notification.
            // 2. Block number of the tip of the canonical chain:
            //   - For committed chain, it's the tip block number.
            //   - For reverted chain, it's the block number preceding the first block in the chain.
            let (chain, tip) = notification
                .committed_chain()
                .map(|chain| (chain.clone(), chain.tip().number))
                .or_else(|| {
                    notification
                        .reverted_chain()
                        .map(|chain| (chain.clone(), chain.first().number - 1))
                })
                .unzip();

            if this.node_head_catchup_in_progress {
                // If we are waiting for the node head to catch up to the same height as the ExEx
                // head, then we need to check if the ExEx is on the canonical chain.

                // Query the chain from the new notification for the ExEx head block number.
                let exex_head_block = chain
                    .as_ref()
                    .and_then(|chain| chain.blocks().get(&this.exex_head.block.number));

                // Compare the hash of the block from the new notification to the ExEx head
                // hash.
                if let Some((block, tip)) = exex_head_block.zip(tip) {
                    if block.hash() == this.exex_head.block.hash {
                        // ExEx is on the canonical chain, proceed with the notification
                        this.node_head_catchup_in_progress = false;
                    } else {
                        // ExEx is not on the canonical chain, synchronize
                        let tip =
                            this.provider.sealed_header(tip)?.ok_or_eyre("node head not found")?;
                        this.node_head = Head::new(
                            tip.number,
                            tip.hash(),
                            tip.difficulty,
                            U256::MAX,
                            tip.timestamp,
                        );
                        this.synchronize()?;
                    }
                }
            }

            if notification
                .committed_chain()
                .or_else(|| notification.reverted_chain())
                .map_or(false, |chain| chain.first().number > this.exex_head.block.number)
            {
                return Poll::Ready(Some(Ok(notification)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;

    use super::*;
    use alloy_consensus::Header;
    use eyre::OptionExt;
    use futures::StreamExt;
    use reth_db_common::init::init_genesis;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::{Block, BlockNumHash};
    use reth_provider::{
        providers::BlockchainProvider2, test_utils::create_test_provider_factory, BlockWriter,
        Chain,
    };
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn exex_notifications_behind_head_canonical() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory.clone())?;

        let node_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );
        let provider_rw = provider_factory.provider_rw()?;
        provider_rw.insert_block(
            node_head_block.clone().seal_with_senders().ok_or_eyre("failed to recover senders")?,
        )?;
        provider_rw.commit()?;

        let node_head = Head {
            number: node_head_block.number,
            hash: node_head_block.hash(),
            ..Default::default()
        };
        let exex_head =
            ExExHead { block: BlockNumHash { number: genesis_block.number, hash: genesis_hash } };

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    node_head.number + 1,
                    BlockParams { parent: Some(node_head.hash), ..Default::default() },
                )
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx.send(notification.clone()).await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        // First notification is the backfill of missing blocks from the canonical chain
        assert_eq!(
            notifications.next().await.transpose()?,
            Some(ExExNotification::ChainCommitted {
                new: Arc::new(
                    BackfillJobFactory::new(
                        notifications.executor.clone(),
                        notifications.provider.clone()
                    )
                    .backfill(1..=1)
                    .next()
                    .ok_or_eyre("failed to backfill")??
                )
            })
        );

        // Second notification is the actual notification that we sent before
        assert_eq!(notifications.next().await.transpose()?, Some(notification));

        Ok(())
    }

    #[ignore]
    #[tokio::test]
    async fn exex_notifications_behind_head_non_canonical() -> eyre::Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn exex_notifications_same_head_canonical() -> eyre::Result<()> {
        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory)?;

        let node_head =
            Head { number: genesis_block.number, hash: genesis_hash, ..Default::default() };
        let exex_head =
            ExExHead { block: BlockNumHash { number: node_head.number, hash: node_head.hash } };

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![Block {
                    header: Header {
                        parent_hash: node_head.hash,
                        number: node_head.number + 1,
                        ..Default::default()
                    },
                    ..Default::default()
                }
                .seal_slow()
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx.send(notification.clone()).await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        let new_notification = notifications.next().await.transpose()?;
        assert_eq!(new_notification, Some(notification));

        Ok(())
    }

    #[ignore]
    #[tokio::test]
    async fn exex_notifications_same_head_non_canonical() -> eyre::Result<()> {
        Ok(())
    }

    #[tokio::test]
    async fn test_notifications_ahead_of_head() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider2::new(provider_factory)?;

        let exex_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );

        let node_head =
            Head { number: genesis_block.number, hash: genesis_hash, ..Default::default() };
        let exex_head = ExExHead {
            block: BlockNumHash { number: exex_head_block.number, hash: exex_head_block.hash() },
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send(ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![exex_head_block
                        .clone()
                        .seal_with_senders()
                        .ok_or_eyre("failed to recover senders")?],
                    Default::default(),
                    None,
                )),
            })
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthExecutorProvider::mainnet(),
            notifications_rx,
        )
        .with_head(exex_head);

        // First notification is skipped because the node is catching up with the ExEx
        let new_notification = poll_fn(|cx| Poll::Ready(notifications.poll_next_unpin(cx))).await;
        assert!(new_notification.is_pending());

        // Imitate the node catching up with the ExEx by sending a notification for the missing
        // block
        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    exex_head_block.number + 1,
                    BlockParams { parent: Some(exex_head_block.hash()), ..Default::default() },
                )
                .seal_with_senders()
                .ok_or_eyre("failed to recover senders")?],
                Default::default(),
                None,
            )),
        };
        notifications_tx.send(notification.clone()).await?;

        // Second notification is received because the node caught up with the ExEx
        assert_eq!(notifications.next().await.transpose()?, Some(notification));

        Ok(())
    }
}
