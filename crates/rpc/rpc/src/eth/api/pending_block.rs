//! Support for building a pending block via local txpool.

use reth_primitives::{SealedBlock, SealedHeader};
use revm_primitives::{BlockEnv, CfgEnv};
use std::time::Instant;

/// Configured [BlockEnv] and [CfgEnv] for a pending block
#[derive(Debug, Clone)]
pub(crate) struct PendingBlockEnv {
    /// Configured [CfgEnv] for the pending block.
    pub(crate) cfg: CfgEnv,
    /// Configured [BlockEnv] for the pending block.
    pub(crate) block_env: BlockEnv,
    /// Origin block for the config
    pub(crate) origin: PendingBlockEnvOrigin,
}

/// The origin for a configured [PendingBlockEnv]
#[derive(Clone, Debug)]
pub(crate) enum PendingBlockEnvOrigin {
    /// The pending block as received from the CL.
    ActualPending(SealedBlock),
    /// The header of the latest block
    DerivedFromLatest(SealedHeader),
}

impl PendingBlockEnvOrigin {
    /// Returns true if the origin is the actual pending block as received from the CL.
    pub(crate) fn is_actual_pending(&self) -> bool {
        matches!(self, PendingBlockEnvOrigin::ActualPending(_))
    }

    /// Consumes the type and returns the actual pending block.
    pub(crate) fn into_actual_pending(self) -> Option<SealedBlock> {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => Some(block),
            _ => None,
        }
    }

    /// Returns the header this pending block is based on.
    pub(crate) fn header(&self) -> &SealedHeader {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => &block.header,
            PendingBlockEnvOrigin::DerivedFromLatest(header) => header,
        }
    }

    /// Consumes the type and returns the header this pending block is based on.
    pub(crate) fn into_header(self) -> SealedHeader {
        match self {
            PendingBlockEnvOrigin::ActualPending(block) => block.header,
            PendingBlockEnvOrigin::DerivedFromLatest(header) => header,
        }
    }
}

/// In memory pending block for `pending` tag
#[derive(Debug)]
pub(crate) struct PendingBlock {
    /// The cached pending block
    pub(crate) block: SealedBlock,
    /// Timestamp when the pending block is considered outdated
    pub(crate) expires_at: Instant,
}

impl PendingBlock {}
