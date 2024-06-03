//! The [`ChainOrchestrator`] contains the state of the chain and orchestrates the components
//! responsible for advancing the chain.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use futures::{stream::FuturesUnordered, Stream};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Re-export of the blockchain tree API.
pub use reth_blockchain_tree_api::*;
use reth_primitives::B256;

/// The type that drives the chain forward.
///
/// A state machine that orchestrates the components responsible for advancing the chain
// Reacts to custom requests
pub struct ChainOrchestrator<T>
where
    T: ChainHandler,
{
    /// The handler responsible for advancing the chain.
    handler: T, /* pipeline */

                /* pruning */

                /* live sync */
}

impl<T> ChainOrchestrator<T>
where
    T: ChainHandler,
{
    /// Returns the handler
    pub const fn handler(&self) -> &T {
        &self.handler
    }

    /// Returns a mutable reference to the handler
    pub fn handler_mut(&mut self) -> &mut T {
        &mut self.handler
    }
}

impl<T> Stream for ChainOrchestrator<T>
where
    T: ChainHandler,
{
    type Item = ChainEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

/// Event emitted by the [`ChainOrchestrator`]
pub enum ChainEvent {
    /// Synced new head.
    Synced(B256),

    SyncPipeline(B256),

    NetworkSync(B256),
}

/// Internal events issued by the [`ChainOrchestrator`].
pub enum FromOrchestrator {
    /// Started syncing or pruning which requires mutable access to the db.
    Pausing,
    /// Finished syncing or pruning
    Active,
}

/// A trait that advances the chain by handling actions.
///
/// This is intended to be implement the chain consensus logic, for example `engine` API.
pub trait ChainHandler: Send + Sync {
    /// Informs the handler about an event from the [`ChainOrchestrator`].
    fn on_event(&mut self, event: FromOrchestrator);

    /// Polls for actions that [`ChainOrchestrator`] should handle.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<ChainEvent> {
        todo!()
    }
}

/// A trait that manages the tree by handling actions.
pub trait TreeHandler: Send + Sync {
    /// The action this type handles
    type Action;
    /// The outcome
    type ActionOutcome;
    /// Error this handler can throw when handling an action.
    type Error;

    /// Invoked when receiving an action.
    fn on_action(
        &mut self,
        action: Self::Action,
    ) -> impl Future<Output = Result<Self::ActionOutcome, Self::Error>> + Send;
}

/// Provides access to [TreeEngineService]
///
/// This is the frontend for the tree service task.
// TODO do we want request response or send + poll?
#[derive(Debug, Clone)]
pub struct TreeEngine {
    to_service: UnboundedSender<TreeAction>,
}

/// A generic task that manages the state of the blockchain tree by handling incoming actions that
/// must be applied to the tree.
///
/// This type is an endless future that listens for incoming messages from the [TreeEngine].
/// It handles the actions
#[must_use = "Future does nothing unless polled"]
pub struct TreeEngineService<T>
where
    T: TreeHandler,
{
    /// Keeps track of the state the service is in
    state: (),
    /// Manages the tree.
    handler: T,
    /// Actions that are currently in progress.
    pending: FuturesUnordered<Pin<Box<dyn Future<Output = Result<T::ActionOutcome, T::Error>>>>>,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<TreeAction>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<TreeAction>,
}

impl<T> Future for TreeEngineService<T>
where
    T: TreeHandler,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

/// Incoming actions the tree task can handle
#[derive(Debug, Clone)]
pub enum TreeAction {
    // TODO FCU + Payload this must be generic over engine types
}
