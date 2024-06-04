use futures::stream::FuturesUnordered;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

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
