use crate::ChainSpec;
use alloy_chains::Chain;
use alloy_eips::eip1559::BaseFeeParams;
use core::fmt::Debug;

/// Trait representing type configuring a chain spec.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthChainSpec: Send + Sync + Unpin + Debug + 'static {
    // todo: make chain spec type generic over hardfork
    //type Hardfork: Clone + Copy + 'static;

    /// Chain id.
    fn chain(&self) -> Chain;

    /// Get the [`BaseFeeParams`] for the chain at the given timestamp.
    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams;
}

impl EthChainSpec for ChainSpec {
    fn chain(&self) -> Chain {
        self.chain
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.base_fee_params_at_timestamp(timestamp)
    }
}
