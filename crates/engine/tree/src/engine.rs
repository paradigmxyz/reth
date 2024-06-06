//! An engine API handler for the chain.

use crate::{
    chain::{ChainHandler, FromOrchestrator, HandlerEvent, OrchestratorState},
    tree::EngineApiTreeHandler,
};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_primitives::{SealedBlockWithSenders, B256};
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

/// Advances the chain based on incoming requests.
///
/// This is a general purpose request handler with network access.
/// This type listens for incoming messages and processes them via the configured request handler.
///
/// ## Overview
///
/// This type is an orchestrator for incoming messages and responsible for delegating requests
/// received from the CL to the handler.
///
/// It is responsible for handling the following:
/// - Downloading blocks on demand from the network if requested by the [`EngineApiRequestHandler`].
///
/// The core logic is part of the [EngineRequestHandler], which is responsible for processing the
/// incoming requests.
pub struct EngineHandler<T>
where
    T: EngineRequestHandler,
{
    /// Processes requests.
    ///
    /// This type is responsible for processing incoming requests.
    handler: T,
    /// Receiver for incoming requests that need to be processed.
    // TODO add stream type for T::Request,
    incoming_requests: (),
    /// Access to the network sync to download blocks on demand.
    network_sync: (),
    /// Requests that are buffered and need to be processed.
    buffered_events: VecDeque<()>,
}

impl<T> ChainHandler for EngineHandler<T>
where
    T: EngineRequestHandler,
{
    fn on_event(&mut self, event: FromOrchestrator) {
        todo!()
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<HandlerEvent> {
        todo!()
    }
}

/// A type that processes incoming requests (e.g. requests from the consensus layer, engine API)
pub trait EngineRequestHandler: Send + Sync {
    /// The request type this handler can process.
    type Request;

    /// Informs the handler about an event from the [`EngineHandler`].
    fn on_event(&mut self, event: FromEngine<Self::Request>);

    /// Advances the handler.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<RequestHandlerEvent>;
}

/// An [`EngineRequestHandler`] that processes engine API requests.
///
/// This type is responsible for advancing the chain during live sync (following the tip of the
/// chain).
///
/// It advances the chain based on received engine API requests:
///
/// - `on_new_payload`: Executes the payload and inserts it into the tree. These are allowed to be
///   processed concurrently.
/// - `on_forkchoice_updated`: Updates the fork choice based on the new head. These require write
///   access to the database and are skipped if the handler can't acquire exclusive access to the
///   database.
///
/// The [`EngineApiTreeHandler`] is used to execute the incoming payloads, storing them in a tree
/// structure and committing new chains to the database on fork choice updates.
///
/// In case required blocks are missing, the handler will request them from the network, by emitting
/// a download request upstream.
pub struct EngineApiRequestHandler<T>
where
    T: EngineApiTreeHandler,
{
    /// The state of the top level orchestrator.
    orchestrator_state: OrchestratorState,
    /// Needs to keep track of requested state changes by the orchestrator.
    state: (),
    /// Currently in progress payload requests.
    pending_payloads: Vec<()>,
    /// Currently in progress fork choice updates.
    pending_fcu: Option<()>,
    /// Next FCU to process
    next_fcu: VecDeque<()>,
    /// Manages the tree
    tree_handler: T,
    /// Events to yield.
    buffered_events: VecDeque<()>,
}

impl<T> EngineRequestHandler for EngineApiRequestHandler<T>
where
    T: EngineApiTreeHandler,
{
    type Request = BeaconEngineMessage<T::Engine>;

    fn on_event(&mut self, event: FromEngine<Self::Request>) {
        // TODO check if we're currently syncing, or mutable access is currently held by the
        // orchestrator then we respond with SYNCING or delay forkchoice updates.  otherwise
        // we tell the tree to handle the requests

        // TODO: should this type spawn the jobs and have access to tree internals or should this be
        // entirely handled by the tree handler? basically

        /*
           let (tx, req) = event;
           let handler = self.tree_handler.clone();
           spawn(move {
               let resp = handler.on_new_payload(req);
               tx.send(resp).unwrap();
           });
        */
        // here the handler must contain shareable state internally

        // or

        /*
         let fut = handler.handle(event);
         self.pending.push(fut);
        */

        // the latter would give the handler more control over the execution of the requests, with
        // this model the logic of this type and the tree handler is very similar

        todo!()
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<RequestHandlerEvent> {
        // advance tree tasks, trigger
        todo!()
    }
}

#[derive(Debug)]
pub enum FromEngine<Req> {
    Event(FromOrchestrator),
    Request(Req),
    DownloadedBlocks(Vec<SealedBlockWithSenders>),
}

#[derive(Debug)]
pub enum RequestHandlerEvent {
    Idle,
    HandlerEvent(HandlerEvent),
    Download(DownloadRequest),
}

/// A request to download blocks from the network.
#[derive(Debug)]
pub enum DownloadRequest {
    Blocks(Vec<B256>),
    Range(B256, usize),
}
