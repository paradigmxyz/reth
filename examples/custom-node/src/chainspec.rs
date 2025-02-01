use reth_chainspec::EthChainSpec;
use reth_optimism_chainspec::OpChainSpec;
use reth_network_peers::NodeRecord;
use reth_primitives_traits::SealedHeader;
use crate::primitives::CustomHeader;
use alloy_genesis::Genesis;

#[derive(Debug)]
pub struct CustomChainSpec {
    inner: OpChainSpec,
    genesis_header: SealedHeader<CustomHeader>,
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

    fn genesis_header(&self) ->  &Self::Header {
        &self.genesis_header
    }
}
