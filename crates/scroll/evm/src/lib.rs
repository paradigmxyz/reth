//! Scroll evm execution implementation.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod build;

mod config;

pub use execute::{ScrollBlockExecutionInput, ScrollExecutorProvider};
mod execute;

mod l1;
pub use l1::RethL1BlockInfo;

pub use receipt::ScrollRethReceiptBuilder;
mod receipt;

use crate::build::ScrollBlockAssembler;
use alloc::sync::Arc;

use alloy_primitives::{Address, BlockNumber, BlockTimestamp};
use reth_primitives_traits::NodePrimitives;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_primitives::ScrollPrimitives;
use revm_scroll::ScrollSpecId;
pub use scroll_alloy_evm::{
    compute_compression_ratio, ScrollBlockExecutorFactory, ScrollDefaultPrecompilesFactory,
    ScrollEvmFactory, ScrollTxCompressionRatios,
};
pub use scroll_alloy_hardforks::{ScrollHardfork, ScrollHardforks};

/// Scroll EVM configuration.
#[derive(Debug)]
pub struct ScrollEvmConfig<
    ChainSpec = ScrollChainSpec,
    N: NodePrimitives = ScrollPrimitives,
    R = ScrollRethReceiptBuilder,
    P = ScrollDefaultPrecompilesFactory,
> {
    /// Executor factory.
    executor_factory: ScrollBlockExecutorFactory<R, Arc<ChainSpec>, P>,
    /// Block assembler.
    block_assembler: ScrollBlockAssembler<ChainSpec>,
    /// Node primitives marker.
    _pd: core::marker::PhantomData<N>,
}

impl<ChainSpec: ScrollHardforks> ScrollEvmConfig<ChainSpec> {
    /// Creates a new [`ScrollEvmConfig`] with the given chain spec for Scroll chains.
    pub fn scroll(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new(chain_spec, ScrollRethReceiptBuilder::default())
    }
}

impl<ChainSpec, N: NodePrimitives, R: Clone, P: Clone> Clone
    for ScrollEvmConfig<ChainSpec, N, R, P>
{
    fn clone(&self) -> Self {
        Self {
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler.clone(),
            _pd: self._pd,
        }
    }
}

impl<ChainSpec: ScrollHardforks, N: NodePrimitives, R, P: Default>
    ScrollEvmConfig<ChainSpec, N, R, P>
{
    /// Creates a new [`ScrollEvmConfig`] with the given chain spec.
    pub fn new(chain_spec: Arc<ChainSpec>, receipt_builder: R) -> Self {
        Self {
            block_assembler: ScrollBlockAssembler::new(chain_spec.clone()),
            executor_factory: ScrollBlockExecutorFactory::new(
                receipt_builder,
                chain_spec,
                ScrollEvmFactory::default(),
            ),
            _pd: core::marker::PhantomData,
        }
    }

    /// Returns the chain spec associated with this configuration.
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }

    /// Returns the spec id at the given head.
    pub fn spec_id_at_timestamp_and_number(
        &self,
        timestamp: BlockTimestamp,
        number: BlockNumber,
    ) -> ScrollSpecId {
        let chain_spec = self.chain_spec();
        spec_id_at_timestamp_and_number(timestamp, number, chain_spec)
    }
}

/// Returns the spec id at the given timestamp and block number for the provided chain spec.
pub fn spec_id_at_timestamp_and_number(
    timestamp: u64,
    number: u64,
    chain_spec: impl ScrollHardforks,
) -> ScrollSpecId {
    if chain_spec
        .scroll_fork_activation(ScrollHardfork::Feynman)
        .active_at_timestamp_or_number(timestamp, number)
    {
        ScrollSpecId::FEYNMAN
    } else if chain_spec
        .scroll_fork_activation(ScrollHardfork::EuclidV2)
        .active_at_timestamp_or_number(timestamp, number)
    {
        ScrollSpecId::EUCLID
    } else if chain_spec
        .scroll_fork_activation(ScrollHardfork::Euclid)
        .active_at_timestamp_or_number(timestamp, number) ||
        chain_spec
            .scroll_fork_activation(ScrollHardfork::DarwinV2)
            .active_at_timestamp_or_number(timestamp, number) ||
        chain_spec
            .scroll_fork_activation(ScrollHardfork::Darwin)
            .active_at_timestamp_or_number(timestamp, number)
    {
        ScrollSpecId::DARWIN
    } else if chain_spec
        .scroll_fork_activation(ScrollHardfork::Curie)
        .active_at_timestamp_or_number(timestamp, number)
    {
        ScrollSpecId::CURIE
    } else if chain_spec
        .scroll_fork_activation(ScrollHardfork::Bernoulli)
        .active_at_timestamp_or_number(timestamp, number)
    {
        ScrollSpecId::BERNOULLI
    } else {
        ScrollSpecId::SHANGHAI
    }
}

/// The attributes for the next block env.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollNextBlockEnvAttributes {
    /// The timestamp of the next block.
    pub timestamp: u64,
    /// The suggested fee recipient for the next block.
    pub suggested_fee_recipient: Address,
    /// Block gas limit.
    pub gas_limit: u64,
    /// The base fee of the next block.
    pub base_fee: u64,
}
