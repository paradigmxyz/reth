use crate::{ChainSpec, DepositContract};
use alloy_chains::Chain;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::B256;
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

    /// Returns the deposit contract data for the chain, if it's present
    fn deposit_contract(&self) -> Option<&DepositContract>;

    /// The genesis hash.
    fn genesis_hash(&self) -> B256;

    /// The delete limit for pruner, per run.
    fn prune_delete_limit(&self) -> usize;
}

impl EthChainSpec for ChainSpec {
    fn chain(&self) -> Chain {
        self.chain
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.base_fee_params_at_timestamp(timestamp)
    }

    fn deposit_contract(&self) -> Option<&DepositContract> {
        self.deposit_contract.as_ref()
    }

    fn genesis_hash(&self) -> B256 {
        self.genesis_hash()
    }

    fn prune_delete_limit(&self) -> usize {
        self.prune_delete_limit
    }
}
