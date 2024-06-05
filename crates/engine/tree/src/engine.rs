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
/// - Downloading blocks on demand
///
/// The handler is responsible for delegating the received messages and downloading blocks from the
/// network on demand. The core logic is part of the [EngineRequestHandler].
// TODO: maybe this abstraction is not actually useful.
pub struct EngineHandler<T>
where
    T: EngineRequestHandler,
{
    /// Processes requests.
    ///
    /// This type is responsible for processing incoming requests.
    handler: T,
    /// Receiver for incoming requests that need to be processed.
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

/// A type that processes incoming requests.
pub trait EngineRequestHandler: Send + Sync {
    /// The request type this handler can process.
    type Request;

    /// Informs the handler about an event from the [`EngineHandler`].
    fn on_event(&mut self, event: FromEngine<Self::Request>);

    /// Advances the handler.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<RequestHandlerEvent>;
}

/// Advances the chain based on received engine API requests.
///
/// - `on_new_payload`: Executes the payload and inserts it into the tree. These are allowed to be
///   processed concurrently.
/// - `on_forkchoice_updated`: Updates the fork choice based on the new head. These require write
///   access to the database and are skipped if the handler can't acquire exclusive access to the
///   database.
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
        // TODO check state of the node, spawn tree task if idle

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
