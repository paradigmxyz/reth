//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Types used in block building.

use std::{fmt, time::Instant};

use derive_more::Constructor;
use reth_chainspec::ChainSpec;
use reth_primitives::{BlockId, BlockNumberOrTag, SealedBlockWithSenders, SealedHeader, B256};
use reth_revm::state_change::apply_blockhashes_update;
use reth_storage_api::errors::provider::ProviderError;
use revm_primitives::{
    db::{Database, DatabaseCommit},
    BlockEnv, CfgEnvWithHandlerCfg,
};

use super::{EthApiError, EthResult};

/// Configured [`BlockEnv`] and [`CfgEnvWithHandlerCfg`] for a pending block
#[derive(Debug, Clone, Constructor)]
pub struct PendingBlockEnv {
    /// Configured [`CfgEnvWithHandlerCfg`] for the pending block.
    pub cfg: CfgEnvWithHandlerCfg,
    /// Configured [`BlockEnv`] for the pending block.
    pub block_env: BlockEnv,
    /// Origin block for the config
    pub origin: PendingBlockEnvOrigin,
}

/// Apply the [EIP-2935](https://eips.ethereum.org/EIPS/eip-2935) pre block state transitions.
///
/// This constructs a new [Evm](revm::Evm) with the given DB, and environment
/// [`CfgEnvWithHandlerCfg`] and [`BlockEnv`].
///
/// This uses [`apply_blockhashes_update`].
pub fn pre_block_blockhashes_update<DB: Database<Error = ProviderError> + DatabaseCommit>(
    db: &mut DB,
    chain_spec: &ChainSpec,
    initialized_block_env: &BlockEnv,
    block_number: u64,
    parent_block_hash: B256,
) -> EthResult<()>
where
    DB::Error: fmt::Display,
{
    apply_blockhashes_update(
        db,
        chain_spec,
        initialized_block_env.timestamp.to::<u64>(),
        block_number,
        parent_block_hash,
    )
    .map_err(|err| EthApiError::Internal(err.into()))
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

/// In memory pending block for `pending` tag
#[derive(Debug, Constructor)]
pub struct PendingBlock {
    /// The cached pending block
    pub block: SealedBlockWithSenders,
    /// Timestamp when the pending block is considered outdated
    pub expires_at: Instant,
}
