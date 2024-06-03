//! A task that maintains the blockchain tree.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use futures::Stream;
use futures::stream::FuturesUnordered;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Re-export of the blockchain tree API.
pub use reth_blockchain_tree_api::*;

/// The type that drives the chain forward
///
/// A state machine that orchestrates the components responsible for advancing the chain
// Reacts to custom requests
struct EngineOrchestrator {

    // consensus actions (Egnine messages)

    // tree handler

    // pipeline

    // pruning

    // live sync

}

impl EngineOrchestrator {

    fn handle_incoming_request(&mut self) {
        // TODO
    }
}

impl Stream for EngineOrchestrator {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

///
pub enum EngineEvent {


}


/// A trait that manages the tree.
///
// TODO this is effectively a tower layer?
// TODO move to tree-api
pub trait EngineBehaviour: Send + Sync {
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
///
// TODO do we want request response or send + poll?
#[derive(Debug, Clone)]
pub struct TreeEngine {
    to_service: UnboundedSender<EngineAction>,
}

/// A generic task that manages the state of the blockchain tree by handling incoming actions that
/// must be applied to the tree.
///
/// This type is an endless future that listens for incoming messages from the [TreeEngine].
/// It handles the actions
#[must_use = "Future does nothing unless polled"]
pub struct TreeEngineService<T>
where
    T: EngineBehaviour,
{
    /// Keeps track of the state the service is in
    state: (),
    /// Manages the tree.
    handler: T,
    /// Actions that are currently in progress.
    pending: FuturesUnordered<Pin<Box< dyn Future< Output = Result<T::ActionOutcome, T::Error>>>>>,
    /// Sender half of the action channel.
    action_tx: UnboundedSender<EngineAction>,
    /// Receiver half of the action channel.
    action_rx: UnboundedReceiverStream<EngineAction>,
}

impl<T> Future for TreeEngineService<T>
where
    T: EngineBehaviour,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}



/// Incoming actions the tree task can handle
#[derive(Debug, Clone)]
pub enum EngineAction {
    // TODO FCU + Payload this must be generic over engine types

}
