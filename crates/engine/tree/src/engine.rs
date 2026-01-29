//! An engine API handler for the chain.

use crate::{
    backfill::BackfillAction,
    chain::{ChainHandler, FromOrchestrator, HandlerEvent},
    download::{BlockDownloader, DownloadAction, DownloadOutcome},
};
use alloy_primitives::B256;
use crossbeam_channel::Sender;
use futures::{Stream, StreamExt};
use reth_chain_state::ExecutedBlock;
use reth_engine_primitives::{BeaconEngineMessage, ConsensusEngineEvent};
use reth_ethereum_primitives::EthPrimitives;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{Block, NodePrimitives, SealedBlock};
use std::{
    collections::HashSet,
    fmt::Display,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::UnboundedReceiver;

/// A [`ChainHandler`] that advances the chain based on incoming requests (CL engine API).
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
/// - Delegating incoming requests to the [`EngineRequestHandler`].
/// - Advancing the [`EngineRequestHandler`] by polling it and emitting events.
/// - Downloading blocks on demand from the network if requested by the [`EngineApiRequestHandler`].
///
/// The core logic is part of the [`EngineRequestHandler`], which is responsible for processing the
/// incoming requests.
#[derive(Debug)]
pub struct EngineHandler<T, S, D> {
    /// Processes requests.
    ///
    /// This type is responsible for processing incoming requests.
    handler: T,
    /// Receiver for incoming requests (from the engine API endpoint) that need to be processed.
    incoming_requests: S,
    /// A downloader to download blocks on demand.
    downloader: D,
}

impl<T, S, D> EngineHandler<T, S, D> {
    /// Creates a new [`EngineHandler`] with the given handler and downloader and incoming stream of
    /// requests.
    pub const fn new(handler: T, downloader: D, incoming_requests: S) -> Self
    where
        T: EngineRequestHandler,
    {
        Self { handler, incoming_requests, downloader }
    }

    /// Returns a mutable reference to the request handler.
    pub const fn handler_mut(&mut self) -> &mut T {
        &mut self.handler
    }
}

impl<T, S, D> ChainHandler for EngineHandler<T, S, D>
where
    T: EngineRequestHandler<Block = D::Block>,
    S: Stream + Send + Sync + Unpin + 'static,
    <S as Stream>::Item: Into<T::Request>,
    D: BlockDownloader,
{
    type Event = T::Event;

    fn on_event(&mut self, event: FromOrchestrator) {
        // delegate event to the handler
        self.handler.on_event(event.into());
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<HandlerEvent<Self::Event>> {
        loop {
            // drain the handler first
            while let Poll::Ready(ev) = self.handler.poll(cx) {
                match ev {
                    RequestHandlerEvent::HandlerEvent(ev) => {
                        return match ev {
                            HandlerEvent::BackfillAction(target) => {
                                // bubble up backfill sync request
                                self.downloader.on_action(DownloadAction::Clear);
                                Poll::Ready(HandlerEvent::BackfillAction(target))
                            }
                            HandlerEvent::Event(ev) => {
                                // bubble up the event
                                Poll::Ready(HandlerEvent::Event(ev))
                            }
                            HandlerEvent::FatalError => Poll::Ready(HandlerEvent::FatalError),
                        }
                    }
                    RequestHandlerEvent::Download(req) => {
                        // delegate download request to the downloader
                        self.downloader.on_action(DownloadAction::Download(req));
                    }
                }
            }

            // pop the next incoming request
            if let Poll::Ready(Some(req)) = self.incoming_requests.poll_next_unpin(cx) {
                // and delegate the request to the handler
                self.handler.on_event(FromEngine::Request(req.into()));
                // skip downloading in this iteration to allow the handler to process the request
                continue
            }

            // advance the downloader
            if let Poll::Ready(outcome) = self.downloader.poll(cx) {
                if let DownloadOutcome::Blocks(blocks) = outcome {
                    // delegate the downloaded blocks to the handler
                    self.handler.on_event(FromEngine::DownloadedBlocks(blocks));
                }
                continue
            }

            return Poll::Pending
        }
    }
}

/// A type that processes incoming requests (e.g. requests from the consensus layer, engine API,
/// such as newPayload).
///
/// ## Control flow
///
/// Requests and certain updates, such as a change in backfill sync status, are delegated to this
/// type via [`EngineRequestHandler::on_event`]. This type is responsible for processing the
/// incoming requests and advancing the chain and emit events when it is polled.
pub trait EngineRequestHandler: Send + Sync {
    /// Event type this handler can emit
    type Event: Send;
    /// The request type this handler can process.
    type Request;
    /// Type of the block sent in [`FromEngine::DownloadedBlocks`] variant.
    type Block: Block;

    /// Informs the handler about an event from the [`EngineHandler`].
    fn on_event(&mut self, event: FromEngine<Self::Request, Self::Block>);

    /// Advances the handler.
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<RequestHandlerEvent<Self::Event>>;
}

/// An [`EngineRequestHandler`] that processes engine API requests by delegating to an execution
/// task.
///
/// This type is responsible for advancing the chain during live sync (following the tip of the
/// chain).
///
/// It advances the chain based on received engine API requests by delegating them to the tree
/// executor.
///
/// There are two types of requests that can be processed:
///
/// - `on_new_payload`: Executes the payload and inserts it into the tree. These are allowed to be
///   processed concurrently.
/// - `on_forkchoice_updated`: Updates the fork choice based on the new head. These require write
///   access to the database and are skipped if the handler can't acquire exclusive access to the
///   database.
///
/// In case required blocks are missing, the handler will request them from the network, by emitting
/// a download request upstream.
#[derive(Debug)]
pub struct EngineApiRequestHandler<Request, N: NodePrimitives> {
    /// channel to send messages to the tree to execute the payload.
    to_tree: Sender<FromEngine<Request, N::Block>>,
    /// channel to receive messages from the tree.
    from_tree: UnboundedReceiver<EngineApiEvent<N>>,
}

impl<Request, N: NodePrimitives> EngineApiRequestHandler<Request, N> {
    /// Creates a new `EngineApiRequestHandler`.
    pub const fn new(
        to_tree: Sender<FromEngine<Request, N::Block>>,
        from_tree: UnboundedReceiver<EngineApiEvent<N>>,
    ) -> Self {
        Self { to_tree, from_tree }
    }
}

impl<Request, N: NodePrimitives> EngineRequestHandler for EngineApiRequestHandler<Request, N>
where
    Request: Send,
{
    type Event = ConsensusEngineEvent<N>;
    type Request = Request;
    type Block = N::Block;

    fn on_event(&mut self, event: FromEngine<Self::Request, Self::Block>) {
        // delegate to the tree
        let _ = self.to_tree.send(event);
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<RequestHandlerEvent<Self::Event>> {
        let Some(ev) = ready!(self.from_tree.poll_recv(cx)) else {
            return Poll::Ready(RequestHandlerEvent::HandlerEvent(HandlerEvent::FatalError))
        };

        let ev = match ev {
            EngineApiEvent::BeaconConsensus(ev) => {
                RequestHandlerEvent::HandlerEvent(HandlerEvent::Event(ev))
            }
            EngineApiEvent::BackfillAction(action) => {
                RequestHandlerEvent::HandlerEvent(HandlerEvent::BackfillAction(action))
            }
            EngineApiEvent::Download(action) => RequestHandlerEvent::Download(action),
        };
        Poll::Ready(ev)
    }
}

/// The type for specifying the kind of engine api.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EngineApiKind {
    /// The chain contains Ethereum configuration.
    #[default]
    Ethereum,
    /// The chain contains Optimism configuration.
    OpStack,
}

impl EngineApiKind {
    /// Returns true if this is the ethereum variant
    pub const fn is_ethereum(&self) -> bool {
        matches!(self, Self::Ethereum)
    }

    /// Returns true if this is the ethereum variant
    pub const fn is_opstack(&self) -> bool {
        matches!(self, Self::OpStack)
    }
}

/// The request variants that the engine API handler can receive.
#[derive(Debug)]
pub enum EngineApiRequest<T: PayloadTypes, N: NodePrimitives> {
    /// A request received from the consensus engine.
    Beacon(BeaconEngineMessage<T>),
    /// Request to insert an already executed block, e.g. via payload building.
    InsertExecutedBlock(ExecutedBlock<N>),
}

impl<T: PayloadTypes, N: NodePrimitives> Display for EngineApiRequest<T, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Beacon(msg) => msg.fmt(f),
            Self::InsertExecutedBlock(block) => {
                write!(f, "InsertExecutedBlock({:?})", block.recovered_block().num_hash())
            }
        }
    }
}

impl<T: PayloadTypes, N: NodePrimitives> From<BeaconEngineMessage<T>> for EngineApiRequest<T, N>
where
    T::BuiltPayload: BuiltPayload<Primitives = N>,
{
    fn from(msg: BeaconEngineMessage<T>) -> Self {
        match msg {
            BeaconEngineMessage::FlashblocksSequence { block } => Self::InsertExecutedBlock(block),
            _ => Self::Beacon(msg),
        }
    }
}

impl<T: PayloadTypes, N: NodePrimitives> From<EngineApiRequest<T, N>>
    for FromEngine<EngineApiRequest<T, N>, N::Block>
{
    fn from(req: EngineApiRequest<T, N>) -> Self {
        Self::Request(req)
    }
}

/// Events emitted by the engine API handler.
#[derive(Debug)]
pub enum EngineApiEvent<N: NodePrimitives = EthPrimitives> {
    /// Event from the consensus engine.
    // TODO(mattsse): find a more appropriate name for this variant, consider phasing it out.
    BeaconConsensus(ConsensusEngineEvent<N>),
    /// Backfill action is needed.
    BackfillAction(BackfillAction),
    /// Block download is needed.
    Download(DownloadRequest),
}

impl<N: NodePrimitives> EngineApiEvent<N> {
    /// Returns `true` if the event is a backfill action.
    pub const fn is_backfill_action(&self) -> bool {
        matches!(self, Self::BackfillAction(_))
    }
}

impl<N: NodePrimitives> From<ConsensusEngineEvent<N>> for EngineApiEvent<N> {
    fn from(event: ConsensusEngineEvent<N>) -> Self {
        Self::BeaconConsensus(event)
    }
}

/// Events received from the engine.
#[derive(Debug)]
pub enum FromEngine<Req, B: Block> {
    /// Event from the top level orchestrator.
    Event(FromOrchestrator),
    /// Request from the engine.
    Request(Req),
    /// Downloaded blocks from the network.
    DownloadedBlocks(Vec<SealedBlock<B>>),
}

impl<Req: Display, B: Block> Display for FromEngine<Req, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Event(ev) => write!(f, "Event({ev:?})"),
            Self::Request(req) => write!(f, "Request({req})"),
            Self::DownloadedBlocks(blocks) => {
                write!(f, "DownloadedBlocks({} blocks)", blocks.len())
            }
        }
    }
}

impl<Req, B: Block> From<FromOrchestrator> for FromEngine<Req, B> {
    fn from(event: FromOrchestrator) -> Self {
        Self::Event(event)
    }
}

/// Requests produced by a [`EngineRequestHandler`].
#[derive(Debug)]
pub enum RequestHandlerEvent<T> {
    /// An event emitted by the handler.
    HandlerEvent(HandlerEvent<T>),
    /// Request to download blocks.
    Download(DownloadRequest),
}

/// A request to download blocks from the network.
#[derive(Debug)]
pub enum DownloadRequest {
    /// Download the given set of blocks.
    BlockSet(HashSet<B256>),
    /// Download the given range of blocks.
    BlockRange(B256, u64),
}

impl DownloadRequest {
    /// Returns a [`DownloadRequest`] for a single block.
    pub fn single_block(hash: B256) -> Self {
        Self::BlockSet(HashSet::from([hash]))
    }
}
