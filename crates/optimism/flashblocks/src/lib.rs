//! A downstream integration of Flashblocks.

pub use payload::{
    ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1, FlashBlock, Metadata,
};
use reth_rpc_eth_types::PendingBlock;
pub use service::FlashBlockService;
pub use ws::{WsConnect, WsFlashBlockStream};

use crate::sequence::FlashBlockCompleteSequence;

mod payload;
mod sequence;
mod service;
mod ws;

/// Receiver of the most recent [`PendingBlock`] built out of [`FlashBlock`]s.
///
/// [`FlashBlock`]: crate::FlashBlock
pub type PendingBlockRx<N> = tokio::sync::watch::Receiver<Option<PendingBlock<N>>>;

/// Receiver of the sequences of [`FlashBlock`]s built.
///
/// [`FlashBlock`]: crate::FlashBlock
pub type FlashBlockCompleteSequenceRx =
    tokio::sync::broadcast::Receiver<FlashBlockCompleteSequence>;
