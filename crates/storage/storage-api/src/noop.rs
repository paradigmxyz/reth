//! Various noop implementations for traits.

use std::sync::Arc;

use crate::{BlockHashReader, BlockNumReader};
use alloy_primitives::{BlockNumber, B256};
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec};
use reth_storage_errors::provider::ProviderResult;

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NoopBlockReader<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> NoopBlockReader<ChainSpec> {
    /// Create a new instance of the `NoopBlockReader`.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

/// Noop implementation for testing purposes
impl<ChainSpec: Send + Sync> BlockHashReader for NoopBlockReader<ChainSpec> {
    fn block_hash(&self, _number: u64) -> ProviderResult<Option<B256>> {
        Ok(None)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        Ok(vec![])
    }
}

impl<ChainSpec: Send + Sync> BlockNumReader for NoopBlockReader<ChainSpec> {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl<ChainSpec: EthChainSpec + 'static> ChainSpecProvider for NoopBlockReader<ChainSpec> {
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}
