use futures_util::{future::BoxFuture, FutureExt};
use reth_db::database::Database;
use reth_primitives::BlockNumber;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateNotification, CanonStateNotifications,
    CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    EvmEnvProvider, StateProviderFactory,
};
use std::future::Future;
use tokio::sync::watch;

/// The handle to the ExEx.
///
/// The spawner of the ExEx is responsible for monitoring the ExEx and reacting to the changes in
/// the height of the last processed block.
#[derive(Debug, Clone)]
pub struct ExExHandle {
    /// Meta information about the ExEx.
    pub meta: ExExMeta,
    /// A watch channel that will receive the height of the last block processed by the ExEx.
    ///
    /// ExEx should guarantee that it will not require all earlier blocks in the future, meaning
    /// that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    pub finished_height: watch::Receiver<BlockNumber>,
}

/// The main trait for the custom ExEx.
///
/// The ExEx should listen to the canonical state notifications and process them, updating the
/// `finished_height` in [ExExHandle] accordingly.
pub trait IntoExEx<DB: Database, P: FullProvider<DB>> {
    /// Spawns the ExEx.
    ///
    /// Returns a future that should be polled to advance the ExEx and a handle to the ExEx.
    fn spawn_exex(
        self,
        provider: P,
        canon_state_notifications: CanonStateNotifications,
    ) -> (BoxFuture<'static, ()>, ExExHandle);
}

/// The meta information about the ExEx.
#[derive(Debug, Clone)]
pub struct ExExMeta {
    /// The identifier of the ExEx. It will appear in logs and metrics.
    pub id: String,
    /// The height from which the ExEx should start processing blocks. Earlier blocks will not be
    /// sent to the ExEx.
    ///
    /// In case when the ExEx needs all blocks from the genesis, it should be set to `0`.
    pub from_height: BlockNumber,
}

/// The managed version of the [IntoExEx] trait that covers the common use case and simplifies the
/// usage.
pub trait ExEx<DB: Database, P: FullProvider<DB>>: Send + Sync + Unpin + 'static {
    /// Initializes the ExEx. Called once when the ExEx is spawned. ExEx should persist the
    /// dependencies such as the provider to re-use them in the future when processing state
    /// notifications.
    ///
    /// Returns the meta information about the ExEx.
    fn init(&self, provider: P) -> ExExMeta;

    /// Processes the state notification. Once processed, the ExEx will not ever receive the same
    /// notification again, and Reth is allowed to prune the blocks from notification.
    ///
    /// [Result::Err] does not stop the ExEx, and used only for observability purposes. The ExEx
    /// will continue to receive new notifications.
    fn on_state_notification(
        &self,
        notification: CanonStateNotification,
    ) -> impl Future<Output = Result<(), ExExError>> + Send;
}

impl<DB: Database, P: FullProvider<DB>, EE: ExEx<DB, P>> IntoExEx<DB, P> for EE {
    fn spawn_exex(
        self,
        provider: P,
        mut canon_state_notifications: CanonStateNotifications,
    ) -> (BoxFuture<'static, ()>, ExExHandle) {
        let meta = self.init(provider);
        let (finished_height_tx, finished_height_rx) = watch::channel(0);

        let handle = ExExHandle { finished_height: finished_height_rx, meta: meta.clone() };

        let future = async move {
            loop {
                let notification = canon_state_notifications.recv().await.unwrap();
                let new_height = notification.tip().number;

                match self.on_state_notification(notification).await {
                    Ok(()) => {
                        tracing::debug!(target: "exex", id = %meta.id, %new_height, "Processed state notification");
                    }
                    Err(err) => {
                        tracing::error!(target: "exex", id = %meta.id, %err, "Failed to process state notification");
                    }
                }

                // Always update the finished hieght channel, no matter if the notification was
                // processed successfully or not.
                finished_height_tx.send(new_height).unwrap();
            }
        };

        (future.boxed(), handle)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ExExError {}

/// Helper trait to unify all provider traits for simplicity.
/// TODO: unify with `node-builder`
pub trait FullProvider<DB: Database>:
    DatabaseProviderFactory<DB>
    + BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider
    + ChangeSetReader
    + CanonStateSubscriptions
    + Clone
    + Unpin
    + 'static
{
}

impl<T, DB: Database> FullProvider<DB> for T where
    T: DatabaseProviderFactory<DB>
        + BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + CanonStateSubscriptions
        + Clone
        + Unpin
        + 'static
{
}
