use crate::{BackfillJobFactory, ExExNotification, StreamBackfillJob, WalHandle};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use futures::{Stream, StreamExt};
use reth_evm::ConfigureEvm;
use reth_exex_types::ExExHead;
use reth_node_api::NodePrimitives;
use reth_provider::{BlockReader, Chain, HeaderProvider, StateProviderFactory};
use reth_tracing::tracing::debug;
use std::{
    fmt::Debug,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::Receiver;

/// Configuration for ExEx notification handling and backfill behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExExConfig {
    /// Skip notifications from the pipeline.
    ///
    /// When true, only `BlockchainTree` notifications will be delivered.
    pub skip_pipeline_notifications: bool,

    /// Maximum blocks to backfill before a notification.
    ///
    /// If the gap between the ExEx head and the next notification exceeds
    /// this limit, only the most recent `max_backfill_distance` blocks will
    /// be backfilled. Older blocks are skipped.
    ///
    /// Set to None for unlimited backfill (backfill all missing blocks).
    pub max_backfill_distance: Option<u64>,
}

/// A stream of [`ExExNotification`]s. The stream will emit notifications for all blocks. If the
/// stream is configured with a head via [`ExExNotifications::set_with_head`] or
/// [`ExExNotifications::with_head`], it will run backfill jobs to catch up to the node head.
///
/// The behavior is determined by the `exex_head` field and `config`:
/// - `exex_head.is_none() && !config.skip_pipeline_notifs`: simple pass-through of notifications
///   without checks
/// - `exex_head.is_some()`: performs canonical checks, backfill, and filtering
/// - If `config.skip_pipeline_notifs` is true, continuously checks for gaps on every notification
///   and backfills as needed.
pub struct ExExNotifications<P, E>
where
    E: ConfigureEvm,
{
    /// The node's head at the time of creation.
    initial_node_head: BlockNumHash,
    provider: P,
    evm_config: E,
    notifications: Receiver<(crate::ExExNotificationSource, ExExNotification<E::Primitives>)>,
    wal_handle: WalHandle<E::Primitives>,
    exex_head: Option<ExExHead>,
    pending_check_canonical: bool,
    pending_check_backfill: bool,
    config: ExExConfig,
    backfill_job: Option<StreamBackfillJob<E, P, Chain<E::Primitives>>>,
    pending_notification: Option<ExExNotification<E::Primitives>>,
}

impl<P: Debug, E> Debug for ExExNotifications<P, E>
where
    E: ConfigureEvm + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExNotifications")
            .field("initial_node_head", &self.initial_node_head)
            .field("provider", &self.provider)
            .field("evm_config", &self.evm_config)
            .field("notifications", &self.notifications)
            .field("exex_head", &self.exex_head)
            .field("pending_check_canonical", &self.pending_check_canonical)
            .field("pending_check_backfill", &self.pending_check_backfill)
            .field("config", &self.config)
            .field("pending_notification", &self.pending_notification)
            .finish()
    }
}

impl<P, E> ExExNotifications<P, E>
where
    E: ConfigureEvm,
{
    /// Creates a new stream of [`ExExNotifications`].
    pub const fn new(
        node_head: BlockNumHash,
        provider: P,
        evm_config: E,
        notifications: Receiver<(crate::ExExNotificationSource, ExExNotification<E::Primitives>)>,
        wal_handle: WalHandle<E::Primitives>,
        config: ExExConfig,
    ) -> Self {
        Self {
            initial_node_head: node_head,
            provider,
            evm_config,
            notifications,
            wal_handle,
            exex_head: None,
            pending_check_canonical: false,
            pending_check_backfill: false,
            config,
            backfill_job: None,
            pending_notification: None,
        }
    }

    /// Sets to the stream to process all notifications sent from the manager without canonical or
    /// backfill checks.
    pub fn set_without_head(&mut self) {
        self.exex_head = None;
        self.pending_check_canonical = false;
        self.pending_check_backfill = false;
        self.backfill_job = None;
        self.pending_notification = None;
    }

    /// Sets the stream to process notifications with canonical and backfill checks based on the
    /// provided head.
    ///
    /// The stream will only emit notifications for blocks that are committed or reverted after the
    /// given head.
    pub fn set_with_head(&mut self, exex_head: ExExHead) {
        self.exex_head = Some(exex_head);
        self.pending_check_canonical = true;
        self.pending_check_backfill = true;
        self.backfill_job = None;
        self.pending_notification = None;
    }

    /// Sets the stream to process all notifications sent from the manager without canonical or
    /// backfill checks, and returns a new modified stream.
    pub fn without_head(mut self) -> Self {
        self.set_without_head();
        self
    }

    /// Sets the stream to process notifications with canonical and backfill checks based on the
    /// provided head, and returns a new modified stream.
    ///
    /// The stream will only emit notifications for blocks that are committed or reverted after the
    /// given head.
    pub fn with_head(mut self, exex_head: ExExHead) -> Self {
        self.set_with_head(exex_head);
        self
    }
}

impl<P, E> ExExNotifications<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: ConfigureEvm<Primitives: NodePrimitives<Block = P::Block>> + Clone + Unpin + 'static,
{
    /// Checks if the ExEx head is on the canonical chain.
    ///
    /// If the head block is not found in the database or it's ahead of the node head, it means
    /// we're not on the canonical chain and we need to revert the notification with the ExEx
    /// head block.
    fn check_canonical(&mut self) -> eyre::Result<Option<ExExNotification<E::Primitives>>> {
        let exex_head = self.exex_head.as_ref().expect("exex_head should be set");
        if self.provider.is_known(exex_head.block.hash)? &&
            exex_head.block.number <= self.initial_node_head.number
        {
            // we have the targeted block and that block is below the current head
            debug!(target: "exex::notifications", "ExEx head is on the canonical chain");
            return Ok(None)
        }

        // If the head block is not found in the database, it means we're not on the canonical
        // chain.

        // Get the committed notification for the head block from the WAL.
        let Some(notification) =
            self.wal_handle.get_committed_notification_by_block_hash(&exex_head.block.hash)?
        else {
            // it's possible that the exex head is further ahead
            if exex_head.block.number > self.initial_node_head.number {
                debug!(target: "exex::notifications", "ExEx head is ahead of the canonical chain");
                return Ok(None);
            }

            return Err(eyre::eyre!(
                "Could not find notification for block hash {:?} in the WAL",
                exex_head.block.hash
            ))
        };

        // Update the head block hash to the parent hash of the first committed block.
        let old_block = exex_head.block;
        let new_block = notification
            .committed_chain()
            .expect("committed_chain should be set")
            .first()
            .parent_num_hash();
        self.exex_head = Some(ExExHead { block: new_block });
        debug!(target: "exex::notifications", old_exex_head = ?old_block, new_exex_head = ?new_block, "ExEx head updated");

        // Return an inverted notification. See the documentation for
        // `ExExNotification::into_inverted`.
        Ok(Some(notification.into_inverted()))
    }

    /// Updates the ExEx head based on the delivered notification.
    ///
    /// This should be called when delivering a notification to track what the ExEx has processed.
    /// Only updates the head if `skip_pipeline_notifs` is true as if its false notifications are
    /// guaranteed to be in order and not have gaps.
    fn update_exex_head(&mut self, notification: &ExExNotification<E::Primitives>) {
        if !self.config.skip_pipeline_notifications {
            return
        }

        // Update head based on notification type
        let head = match notification {
            ExExNotification::ChainCommitted { new } |
            ExExNotification::ChainReorged { new, .. } => new.tip().num_hash(),
            ExExNotification::ChainReverted { old } => {
                // After a revert, the head should be the parent of the first reverted block
                old.first().parent_num_hash()
            }
        };

        self.exex_head = Some(ExExHead { block: head });
    }

    /// Compares the ExEx head against a target block number, and backfills if needed.
    ///
    /// CAUTION: This method assumes that the ExEx head is on the canonical chain.
    ///
    /// # Arguments
    /// * `target_block` - The block number to backfill up to (inclusive)
    ///
    /// Possible situations are:
    /// - ExEx is behind the target block (`exex_head.number < target_block`). Backfill from the
    ///   node database.
    /// - ExEx is at the same block number as the target (`exex_head.number == target_block`).
    ///   Nothing to do.
    /// - ExEx is ahead of the target block (`exex_head.number > target_block`). Nothing to do.
    fn check_backfill(&mut self, target_block: u64) -> eyre::Result<()> {
        let exex_head = self.exex_head.as_ref().expect("exex_head should be set");

        match exex_head.block.number.cmp(&target_block) {
            std::cmp::Ordering::Less => {
                let gap = target_block - exex_head.block.number;

                // Apply max_backfill_distance limit
                let (backfill_start, backfill_end) = if let Some(max_distance) =
                    self.config.max_backfill_distance &&
                    gap > max_distance
                {
                    debug!(
                        target: "exex::notifications",
                        exex_head = exex_head.block.number,
                        target_block,
                        gap,
                        max_distance,
                        "Gap exceeds max backfill distance, backfilling only recent blocks"
                    );
                    // Backfill only the last max_distance blocks before target
                    let start = target_block
                        .saturating_sub(max_distance.saturating_sub(1))
                        .max(exex_head.block.number + 1);
                    (start, target_block)
                } else {
                    debug!(
                        target: "exex::notifications",
                        exex_head = exex_head.block.number,
                        target_block,
                        "ExEx is behind the target block, backfilling all missing blocks"
                    );
                    (exex_head.block.number + 1, target_block)
                };

                let backfill_job_factory =
                    BackfillJobFactory::new(self.evm_config.clone(), self.provider.clone());
                let backfill =
                    backfill_job_factory.backfill(backfill_start..=backfill_end).into_stream();
                self.backfill_job = Some(backfill);
            }
            std::cmp::Ordering::Equal => {
                debug!(target: "exex::notifications", "ExEx is at the target block");
            }
            std::cmp::Ordering::Greater => {
                debug!(target: "exex::notifications", "ExEx is ahead of the target block");
            }
        };

        Ok(())
    }
}

impl<P, E> Stream for ExExNotifications<P, E>
where
    P: BlockReader + HeaderProvider + StateProviderFactory + Clone + Unpin + 'static,
    E: ConfigureEvm<Primitives: NodePrimitives<Block = P::Block>> + Clone + Unpin + 'static,
{
    type Item = eyre::Result<ExExNotification<E::Primitives>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // 1. Check canonical state (one-time if true)
        if this.pending_check_canonical {
            if let Some(notification) = this.check_canonical()? {
                this.update_exex_head(&notification);
                return Poll::Ready(Some(Ok(notification)))
            }
            this.pending_check_canonical = false;
        }

        // 2. Check backfill need (one-time if true)
        if this.pending_check_backfill {
            this.check_backfill(this.initial_node_head.number)?;
            this.pending_check_backfill = false;
        }

        // 3. Poll backfill job if running
        if let Some(backfill_job) = &mut this.backfill_job {
            if let Some(chain) = ready!(backfill_job.poll_next_unpin(cx)).transpose()? {
                let notification = ExExNotification::ChainCommitted { new: Arc::new(chain) };
                this.update_exex_head(&notification);
                return Poll::Ready(Some(Ok(notification)))
            }
            this.backfill_job = None;

            // If we just finished backfill and have a pending notification, deliver it
            if let Some(pending) = this.pending_notification.take() {
                this.update_exex_head(&pending);
                return Poll::Ready(Some(Ok(pending)))
            }
        }

        // 4. Poll regular notifications with gap detection
        loop {
            let Some((source, notification)) = ready!(this.notifications.poll_recv(cx)) else {
                return Poll::Ready(None)
            };

            if !this.config.skip_pipeline_notifications {
                return Poll::Ready(Some(Ok(notification)))
            }

            // Only config.skip_pipeline_notifications = true reaches beyond this point
            if source.is_pipeline() {
                debug!(
                    target: "exex::notifications",
                    "Skipping pipeline notification"
                );
                continue
            }

            if let Some(committed) = notification.committed_chain() {
                // Check if we need to backfill up to the block before this notification
                this.check_backfill(committed.first().number() - 1)?;

                // If a backfill job was created, store the notification and return
                // Pending
                if this.backfill_job.is_some() {
                    // Store this notification to deliver after backfill
                    this.pending_notification = Some(notification);

                    // Wake to poll the backfill job immediately
                    cx.waker().wake_by_ref();
                    return Poll::Pending
                }
            }

            this.update_exex_head(&notification);
            return Poll::Ready(Some(Ok(notification)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Wal;
    use alloy_consensus::Header;
    use alloy_eips::BlockNumHash;
    use eyre::OptionExt;
    use futures::StreamExt;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Block;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::Block as _;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory, BlockWriter,
        Chain, DBProvider, DatabaseProviderFactory,
    };
    use reth_testing_utils::generators::{self, random_block, BlockParams};
    use std::collections::BTreeMap;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn exex_notifications_behind_head_canonical() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider::new(provider_factory.clone())?;

        let node_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        )
        .try_recover()?;
        let node_head = node_head_block.num_hash();
        let provider_rw = provider_factory.provider_rw()?;
        provider_rw.insert_block(&node_head_block)?;
        provider_rw.commit()?;
        let exex_head =
            ExExHead { block: BlockNumHash { number: genesis_block.number, hash: genesis_hash } };

        let notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    node_head.number + 1,
                    BlockParams { parent: Some(node_head.hash), ..Default::default() },
                )
                .try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send((crate::ExExNotificationSource::BlockchainTree, notification.clone()))
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthEvmConfig::mainnet(),
            notifications_rx,
            wal.handle(),
            crate::ExExConfig::default(),
        )
        .with_head(exex_head);

        // First notification is the backfill of missing blocks from the canonical chain
        assert_eq!(
            notifications.next().await.transpose()?,
            Some(ExExNotification::ChainCommitted {
                new: Arc::new(
                    BackfillJobFactory::new(
                        notifications.evm_config.clone(),
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

    #[tokio::test]
    async fn exex_notifications_same_head_canonical() -> eyre::Result<()> {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider::new(provider_factory)?;

        let node_head = BlockNumHash { number: genesis_block.number, hash: genesis_hash };
        let exex_head = ExExHead { block: node_head };

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
                .try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send((crate::ExExNotificationSource::BlockchainTree, notification.clone()))
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthEvmConfig::mainnet(),
            notifications_rx,
            wal.handle(),
            crate::ExExConfig::default(),
        )
        .with_head(exex_head);

        let new_notification = notifications.next().await.transpose()?;
        assert_eq!(new_notification, Some(notification));

        Ok(())
    }

    #[tokio::test]
    async fn exex_notifications_same_head_non_canonical() -> eyre::Result<()> {
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider::new(provider_factory)?;

        let node_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        )
        .try_recover()?;
        let node_head = node_head_block.num_hash();
        let provider_rw = provider.database_provider_rw()?;
        provider_rw.insert_block(&node_head_block)?;
        provider_rw.commit()?;
        let node_head_notification = ExExNotification::ChainCommitted {
            new: Arc::new(
                BackfillJobFactory::new(EthEvmConfig::mainnet(), provider.clone())
                    .backfill(node_head.number..=node_head.number)
                    .next()
                    .ok_or_else(|| eyre::eyre!("failed to backfill"))??,
            ),
        };

        let exex_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );
        let exex_head = ExExHead { block: exex_head_block.num_hash() };
        let exex_head_notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![exex_head_block.clone().try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };
        wal.commit(&exex_head_notification)?;

        let new_notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    node_head.number + 1,
                    BlockParams { parent: Some(node_head.hash), ..Default::default() },
                )
                .try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send((crate::ExExNotificationSource::BlockchainTree, new_notification.clone()))
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthEvmConfig::mainnet(),
            notifications_rx,
            wal.handle(),
            crate::ExExConfig::default(),
        )
        .with_head(exex_head);

        // First notification is the revert of the ExEx head block to get back to the canonical
        // chain
        assert_eq!(
            notifications.next().await.transpose()?,
            Some(exex_head_notification.into_inverted())
        );
        // Second notification is the backfilled block from the canonical chain to get back to the
        // canonical tip
        assert_eq!(notifications.next().await.transpose()?, Some(node_head_notification));
        // Third notification is the actual notification that we sent before
        assert_eq!(notifications.next().await.transpose()?, Some(new_notification));

        Ok(())
    }

    #[tokio::test]
    async fn test_notifications_ahead_of_head() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider::new(provider_factory)?;

        let exex_head_block = random_block(
            &mut rng,
            genesis_block.number + 1,
            BlockParams { parent: Some(genesis_hash), tx_count: Some(0), ..Default::default() },
        );
        let exex_head_notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![exex_head_block.clone().try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };
        wal.commit(&exex_head_notification)?;

        let node_head = BlockNumHash { number: genesis_block.number, hash: genesis_hash };
        let exex_head = ExExHead {
            block: BlockNumHash { number: exex_head_block.number, hash: exex_head_block.hash() },
        };

        let new_notification = ExExNotification::ChainCommitted {
            new: Arc::new(Chain::new(
                vec![random_block(
                    &mut rng,
                    genesis_block.number + 1,
                    BlockParams { parent: Some(genesis_hash), ..Default::default() },
                )
                .try_recover()?],
                Default::default(),
                BTreeMap::new(),
            )),
        };

        let (notifications_tx, notifications_rx) = mpsc::channel(1);

        notifications_tx
            .send((crate::ExExNotificationSource::BlockchainTree, new_notification.clone()))
            .await?;

        let mut notifications = ExExNotifications::new(
            node_head,
            provider,
            EthEvmConfig::mainnet(),
            notifications_rx,
            wal.handle(),
            crate::ExExConfig::default(),
        )
        .with_head(exex_head);

        // First notification is the revert of the ExEx head block to get back to the canonical
        // chain
        assert_eq!(
            notifications.next().await.transpose()?,
            Some(exex_head_notification.into_inverted())
        );

        // Second notification is the actual notification that we sent before
        assert_eq!(notifications.next().await.transpose()?, Some(new_notification));

        Ok(())
    }

    #[tokio::test]
    async fn test_notifications_with_continuous_backfill_for_skip_pipeline_notifications(
    ) -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let mut rng = generators::rng();

        let temp_dir = tempfile::tempdir().unwrap();
        let wal = Wal::new(temp_dir.path()).unwrap();

        let provider_factory = create_test_provider_factory();
        let genesis_hash = init_genesis(&provider_factory)?;
        let genesis_block = provider_factory
            .block(genesis_hash.into())?
            .ok_or_else(|| eyre::eyre!("genesis block not found"))?;

        let provider = BlockchainProvider::new(provider_factory.clone())?;

        // Create blocks 1-7 in the database
        let mut blocks = Vec::new();
        let mut parent_hash = genesis_hash;
        let mut parent_number = genesis_block.number;

        for _ in 0..7 {
            let block = random_block(
                &mut rng,
                parent_number + 1,
                BlockParams { parent: Some(parent_hash), tx_count: Some(0), ..Default::default() },
            )
            .try_recover()?;
            let provider_rw = provider_factory.provider_rw()?;
            provider_rw.insert_block(&block)?;
            provider_rw.commit()?;

            parent_hash = block.hash();
            parent_number = block.number;
            blocks.push(block);
        }

        // Set node_head to genesis to avoid initial backfill
        let node_head = BlockNumHash { number: genesis_block.number, hash: genesis_hash };

        // ExEx is at genesis (block 0)
        let exex_head = ExExHead { block: node_head };

        // Test 1: Full backfill (max_backfill_distance = None)
        {
            let (notifications_tx, notifications_rx) = mpsc::channel(2);

            // Send a notification for block 3 (skipping blocks 1 and 2)
            let notification_block_3 = ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![blocks[2].clone()],
                    Default::default(),
                    BTreeMap::new(),
                )),
            };
            notifications_tx
                .send((crate::ExExNotificationSource::BlockchainTree, notification_block_3.clone()))
                .await?;

            // Send a notification for block 7 (skipping blocks 4, 5, 6)
            let notification_block_7 = ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![blocks[6].clone()],
                    Default::default(),
                    BTreeMap::new(),
                )),
            };
            notifications_tx
                .send((crate::ExExNotificationSource::BlockchainTree, notification_block_7.clone()))
                .await?;

            let mut notifications_full_backfill = ExExNotifications::new(
                node_head,
                provider.clone(),
                EthEvmConfig::mainnet(),
                notifications_rx,
                wal.handle(),
                crate::ExExConfig {
                    skip_pipeline_notifications: true,
                    max_backfill_distance: None,
                },
            )
            .with_head(exex_head);

            // First notification should be backfill for blocks 1-2 (gap before block 3)
            let backfill_1_2 = notifications_full_backfill.next().await.transpose()?;
            assert!(backfill_1_2.is_some());
            if let Some(ExExNotification::ChainCommitted { new }) = &backfill_1_2 {
                assert_eq!(new.first().number(), 1);
                assert_eq!(new.tip().number(), 2);
            } else {
                panic!("Expected backfill notification for blocks 1-2");
            }

            // Second notification should be the original block 3 notification
            assert_eq!(
                notifications_full_backfill.next().await.transpose()?,
                Some(notification_block_3)
            );

            // Third notification should be backfill for blocks 4-6 (gap before block 7)
            let backfill_4_6 = notifications_full_backfill.next().await.transpose()?;
            assert!(backfill_4_6.is_some());
            if let Some(ExExNotification::ChainCommitted { new }) = &backfill_4_6 {
                assert_eq!(new.first().number(), 4);
                assert_eq!(new.tip().number(), 6);
            } else {
                panic!("Expected backfill notification for blocks 4-6");
            }

            // Fourth notification should be the original block 7 notification
            assert_eq!(
                notifications_full_backfill.next().await.transpose()?,
                Some(notification_block_7)
            );
        }

        // Test 2: Constrained backfill (max_backfill_distance = Some(2))
        {
            let (notifications_tx, notifications_rx) = mpsc::channel(2);

            // Send a notification for block 3 (skipping blocks 1 and 2)
            let notification_block_3 = ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![blocks[2].clone()],
                    Default::default(),
                    BTreeMap::new(),
                )),
            };
            notifications_tx
                .send((crate::ExExNotificationSource::BlockchainTree, notification_block_3.clone()))
                .await?;

            // Send a notification for block 7 (skipping blocks 4, 5, 6)
            let notification_block_7 = ExExNotification::ChainCommitted {
                new: Arc::new(Chain::new(
                    vec![blocks[6].clone()],
                    Default::default(),
                    BTreeMap::new(),
                )),
            };
            notifications_tx
                .send((crate::ExExNotificationSource::BlockchainTree, notification_block_7.clone()))
                .await?;

            let mut notifications_constrained_backfill = ExExNotifications::new(
                node_head,
                provider.clone(),
                EthEvmConfig::mainnet(),
                notifications_rx,
                wal.handle(),
                crate::ExExConfig {
                    skip_pipeline_notifications: true,
                    max_backfill_distance: Some(2),
                },
            )
            .with_head(exex_head);

            // First notification should be backfill for blocks 1-2 (gap equals max_distance, so
            // backfills all)
            let backfill_1_2 = notifications_constrained_backfill.next().await.transpose()?;
            assert!(backfill_1_2.is_some());
            if let Some(ExExNotification::ChainCommitted { new }) = &backfill_1_2 {
                assert_eq!(new.first().number(), 1);
                assert_eq!(new.tip().number(), 2);
            } else {
                panic!("Expected backfill notification for blocks 1-2");
            }

            // Second notification should be the original block 3 notification
            assert_eq!(
                notifications_constrained_backfill.next().await.transpose()?,
                Some(notification_block_3)
            );

            // Third notification should be backfill for blocks 5-6 (constrained to 2 blocks)
            let backfill_5_6 = notifications_constrained_backfill.next().await.transpose()?;
            assert!(backfill_5_6.is_some());
            if let Some(ExExNotification::ChainCommitted { new }) = &backfill_5_6 {
                assert_eq!(new.first().number(), 5);
                assert_eq!(new.tip().number(), 6);
            } else {
                panic!("Expected backfill notification for blocks 5-6");
            }

            // Fourth notification should be the original block 7 notification
            assert_eq!(
                notifications_constrained_backfill.next().await.transpose()?,
                Some(notification_block_7)
            );
        }

        Ok(())
    }
}
