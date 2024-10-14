use crate::{ChainSpec, DepositContract};
use alloc::vec::Vec;
use alloy_chains::Chain;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_genesis::Genesis;
use alloy_primitives::B256;
use core::fmt::{Debug, Display};
use reth_network_peers::NodeRecord;
use reth_primitives_traits::Header;

/// Trait representing type configuring a chain spec.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthChainSpec: Send + Sync + Unpin + Debug {
    // todo: make chain spec type generic over hardfork
    //type Hardfork: Clone + Copy + 'static;

    /// Chain id.
    fn chain(&self) -> Chain;

    /// Get the [`BaseFeeParams`] for the chain at the given block.
    fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams;

    /// Get the [`BaseFeeParams`] for the chain at the given timestamp.
    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams;

    /// Returns the deposit contract data for the chain, if it's present
    fn deposit_contract(&self) -> Option<&DepositContract>;

    /// The genesis hash.
    fn genesis_hash(&self) -> B256;

    /// The delete limit for pruner, per run.
    fn prune_delete_limit(&self) -> usize;

    /// Returns a string representation of the hardforks.
    fn display_hardforks(&self) -> impl Display;

    /// The genesis header.
    fn genesis_header(&self) -> &Header;

    /// The genesis block specification.
    fn genesis(&self) -> &Genesis;

    /// The block gas limit.
    fn max_gas_limit(&self) -> u64;

    /// The bootnodes for the chain, if any.
    fn bootnodes(&self) -> Option<Vec<NodeRecord>>;

    /// Returns `true` if this chain contains Optimism configuration.
    fn is_optimism(&self) -> bool {
        self.chain().is_optimism()
    }
}

impl EthChainSpec for ChainSpec {
    fn chain(&self) -> Chain {
        self.chain
    }

    fn base_fee_params_at_block(&self, block_number: u64) -> BaseFeeParams {
        self.base_fee_params_at_block(block_number)
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

    fn display_hardforks(&self) -> impl Display {
        self.display_hardforks()
    }

    fn genesis_header(&self) -> &Header {
        self.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.genesis()
    }

    fn max_gas_limit(&self) -> u64 {
        self.max_gas_limit
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.bootnodes()
    }

    fn is_optimism(&self) -> bool {
        Self::is_optimism(self)
    }
}
