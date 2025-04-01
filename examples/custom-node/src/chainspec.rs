use crate::primitives::CustomHeader;
use alloy_genesis::Genesis;
use reth_chainspec::{EthChainSpec, EthereumHardforks, Hardfork, Hardforks};
use reth_network_peers::NodeRecord;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OpHardforks;
use reth_primitives_traits::SealedHeader;

#[derive(Debug, Clone)]
pub struct CustomChainSpec {
    inner: OpChainSpec,
    genesis_header: SealedHeader<CustomHeader>,
}

impl Hardforks for CustomChainSpec {
    fn fork<H: Hardfork>(&self, fork: H) -> reth_chainspec::ForkCondition {
        self.inner.fork(fork)
    }

    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, reth_chainspec::ForkCondition)> {
        self.inner.forks_iter()
    }

    fn fork_id(&self, head: &reth_chainspec::Head) -> reth_chainspec::ForkId {
        self.inner.fork_id(head)
    }

    fn latest_fork_id(&self) -> reth_chainspec::ForkId {
        self.inner.latest_fork_id()
    }

    fn fork_filter(&self, head: reth_chainspec::Head) -> reth_chainspec::ForkFilter {
        self.inner.fork_filter(head)
    }
}

impl EthChainSpec for CustomChainSpec {
    type Header = CustomHeader;

    fn base_fee_params_at_block(&self, block_number: u64) -> reth_chainspec::BaseFeeParams {
        self.inner.base_fee_params_at_block(block_number)
    }

    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<alloy_eips::eip7840::BlobParams> {
        self.inner.blob_params_at_timestamp(timestamp)
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> reth_chainspec::BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.inner.bootnodes()
    }

    fn chain(&self) -> reth_chainspec::Chain {
        self.inner.chain()
    }

    fn deposit_contract(&self) -> Option<&reth_chainspec::DepositContract> {
        self.inner.deposit_contract()
    }

    fn display_hardforks(&self) -> Box<dyn std::fmt::Display> {
        self.inner.display_hardforks()
    }

    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }

    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }

    fn genesis_hash(&self) -> revm_primitives::B256 {
        self.genesis_header.hash()
    }

    fn genesis_header(&self) -> &Self::Header {
        &self.genesis_header
    }

    fn final_paris_total_difficulty(&self) -> Option<revm_primitives::U256> {
        self.inner.get_final_paris_total_difficulty()
    }
}

impl EthereumHardforks for CustomChainSpec {
    fn ethereum_fork_activation(
        &self,
        fork: reth_chainspec::EthereumHardfork,
    ) -> reth_chainspec::ForkCondition {
        self.inner.ethereum_fork_activation(fork)
    }
}

impl OpHardforks for CustomChainSpec {
    fn op_fork_activation(
        &self,
        fork: reth_optimism_forks::OpHardfork,
    ) -> reth_chainspec::ForkCondition {
        self.inner.op_fork_activation(fork)
    }
}
