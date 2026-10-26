use crate::{ChainSpec, DepositContract};
use alloc::{boxed::Box, vec::Vec};
use alloy_chains::Chain;
use alloy_eips::{eip1559::BaseFeeParams, eip7840::BlobParams};
use alloy_genesis::Genesis;
use alloy_primitives::{B256, U256};
use core::fmt::{Debug, Display};
use reth_ethereum_forks::EthereumHardforks;
use reth_network_peers::NodeRecord;
use reth_primitives_traits::{AlloyBlockHeader, BlockHeader};

/// Trait representing type configuring a chain spec.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthChainSpec: Send + Sync + Unpin + Debug {
    /// The header type of the network.
    type Header: BlockHeader;

    /// Returns the [`Chain`] object this spec targets.
    fn chain(&self) -> Chain;

    /// Returns the chain id number
    fn chain_id(&self) -> u64 {
        self.chain().id()
    }

    /// Get the [`BaseFeeParams`] for the chain at the given timestamp.
    ///
    /// Use [`Self::next_block_base_fee_with_slot`] for slot-dependent updates that cannot be
    /// represented by these parameters.
    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams;

    /// Get the [`BlobParams`] for the given timestamp
    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<BlobParams>;

    /// Returns the deposit contract data for the chain, if it's present
    fn deposit_contract(&self) -> Option<&DepositContract>;

    /// The genesis hash.
    fn genesis_hash(&self) -> B256;

    /// The delete limit for pruner, per run.
    fn prune_delete_limit(&self) -> usize;

    /// Returns a string representation of the hardforks.
    fn display_hardforks(&self) -> Box<dyn Display>;

    /// The genesis header.
    fn genesis_header(&self) -> &Self::Header;

    /// The genesis block specification.
    fn genesis(&self) -> &Genesis;

    /// The bootnodes for the chain, if any.
    fn bootnodes(&self) -> Option<Vec<NodeRecord>>;

    /// Returns `true` if this chain contains Optimism configuration.
    fn is_optimism(&self) -> bool {
        self.chain().is_optimism()
    }

    /// Returns `true` if this chain contains Ethereum configuration.
    fn is_ethereum(&self) -> bool {
        self.chain().is_ethereum()
    }

    /// Returns the final total difficulty if the Paris hardfork is known.
    fn final_paris_total_difficulty(&self) -> Option<U256>;

    /// See [`AlloyBlockHeader::next_block_base_fee`].
    fn next_block_base_fee(&self, parent: &Self::Header, target_timestamp: u64) -> Option<u64> {
        parent.next_block_base_fee(self.base_fee_params_at_timestamp(target_timestamp))
    }

    /// Calculates the next base fee using the child slot number for EIP-8198.
    ///
    /// Chains without slot-dependent fees use the ordinary EIP-1559 calculation.
    fn next_block_base_fee_with_slot(
        &self,
        parent: &Self::Header,
        target_timestamp: u64,
        _target_slot_number: Option<u64>,
    ) -> Option<u64> {
        self.next_block_base_fee(parent, target_timestamp)
    }
}

impl<H: BlockHeader> EthChainSpec for ChainSpec<H> {
    type Header = H;

    fn chain(&self) -> Chain {
        self.chain
    }

    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.base_fee_params_at_timestamp(timestamp)
    }

    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<BlobParams> {
        if let Some(blob_param) = self.blob_params.active_scheduled_params_at_timestamp(timestamp) {
            Some(*blob_param)
        } else if self.is_osaka_active_at_timestamp(timestamp) {
            Some(self.blob_params.osaka)
        } else if self.is_prague_active_at_timestamp(timestamp) {
            Some(self.blob_params.prague)
        } else if self.is_cancun_active_at_timestamp(timestamp) {
            Some(self.blob_params.cancun)
        } else {
            None
        }
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

    fn display_hardforks(&self) -> Box<dyn Display> {
        Box::new(Self::display_hardforks(self))
    }

    fn genesis_header(&self) -> &Self::Header {
        self.genesis_header()
    }

    fn genesis(&self) -> &Genesis {
        self.genesis()
    }

    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.bootnodes()
    }

    fn is_optimism(&self) -> bool {
        false
    }

    fn final_paris_total_difficulty(&self) -> Option<U256> {
        self.get_final_paris_total_difficulty()
    }

    fn next_block_base_fee_with_slot(
        &self,
        parent: &Self::Header,
        target_timestamp: u64,
        target_slot_number: Option<u64>,
    ) -> Option<u64> {
        if !self.is_bogota_active_at_timestamp(target_timestamp) {
            return self.next_block_base_fee(parent, target_timestamp);
        }

        let slots_elapsed = target_slot_number?.checked_sub(parent.slot_number()?)?;
        if slots_elapsed == 0 {
            return None;
        }
        let seconds_elapsed = target_timestamp.checked_sub(parent.timestamp())?;
        let base_fee = parent.base_fee_per_gas()?;
        let gas_target = parent.gas_limit() / 2;
        if gas_target == 0 {
            return Some(base_fee);
        }
        // Divide by the gas target first to preserve the execution spec's rounding.
        // Wide arithmetic also covers slot gaps whose 96 * slots denominator exceeds u64.
        let target_fee_gas_delta = U256::from(base_fee) *
            U256::from(parent.gas_used().abs_diff(gas_target)) /
            U256::from(gas_target);
        let reference_seconds = U256::from(slots_elapsed) * U256::from(12);
        let seconds_elapsed = U256::from(seconds_elapsed).min(reference_seconds);
        let delta = u64::try_from(
            target_fee_gas_delta * seconds_elapsed / (U256::from(8) * reference_seconds),
        )
        .ok()?;
        Some(if parent.gas_used() > gas_target {
            base_fee.saturating_add(delta.max(1))
        } else {
            base_fee.saturating_sub(delta)
        })
    }
}
