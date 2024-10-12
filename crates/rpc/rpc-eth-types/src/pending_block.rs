//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Types used in block building.

use std::time::Instant;

use alloy_primitives::B256;
use derive_more::Constructor;
use reth_primitives::{BlockId, BlockNumberOrTag, Receipt, SealedBlockWithSenders, SealedHeader};
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg};

/// Configured [`BlockEnv`] and [`CfgEnvWithHandlerCfg`] for a pending block.
#[derive(Debug, Clone, Constructor)]
pub struct PendingBlockEnv {
    /// Configured [`CfgEnvWithHandlerCfg`] for the pending block.
    pub cfg: CfgEnvWithHandlerCfg,
    /// Configured [`BlockEnv`] for the pending block.
    pub block_env: BlockEnv,
    /// Origin block for the config
    pub origin: PendingBlockEnvOrigin,
}

/// The origin for a configured [`PendingBlockEnv`]
#[derive(Clone, Debug)]
pub enum PendingBlockEnvOrigin {
    /// The pending block as received from the CL.
    ActualPending(SealedBlockWithSenders),
    /// The _modified_ header of the latest block.
    ///
    /// This derives the pending state based on the latest header by modifying:
    ///  - the timestamp
    ///  - the block number
    ///  - fees
    DerivedFromLatest(SealedHeader),
}

impl PendingBlockEnvOrigin {
    /// Returns true if the origin is the actual pending block as received from the CL.
    pub const fn is_actual_pending(&self) -> bool {
        matches!(self, Self::ActualPending(_))
    }

    /// Consumes the type and returns the actual pending block.
    pub fn into_actual_pending(self) -> Option<SealedBlockWithSenders> {
        match self {
            Self::ActualPending(block) => Some(block),
            _ => None,
        }
    }

    /// Returns the [`BlockId`] that represents the state of the block.
    ///
    /// If this is the actual pending block, the state is the "Pending" tag, otherwise we can safely
    /// identify the block by its hash (latest block).
    pub fn state_block_id(&self) -> BlockId {
        match self {
            Self::ActualPending(_) => BlockNumberOrTag::Pending.into(),
            Self::DerivedFromLatest(header) => BlockId::Hash(header.hash().into()),
        }
    }

    /// Returns the hash of the block the pending block should be built on.
    ///
    /// For the [`PendingBlockEnvOrigin::ActualPending`] this is the parent hash of the block.
    /// For the [`PendingBlockEnvOrigin::DerivedFromLatest`] this is the hash of the _latest_
    /// header.
    pub fn build_target_hash(&self) -> B256 {
        match self {
            Self::ActualPending(block) => block.parent_hash,
            Self::DerivedFromLatest(header) => header.hash(),
        }
    }

    /// Returns the header this pending block is based on.
    pub fn header(&self) -> &SealedHeader {
        match self {
            Self::ActualPending(block) => &block.header,
            Self::DerivedFromLatest(header) => header,
        }
    }
}

/// Locally built pending block for `pending` tag.
#[derive(Debug, Constructor)]
pub struct PendingBlock {
    /// Timestamp when the pending block is considered outdated.
    pub expires_at: Instant,
    /// The locally built pending block.
    pub block: SealedBlockWithSenders,
    /// The receipts for the pending block
    pub receipts: Vec<Receipt>,
}
