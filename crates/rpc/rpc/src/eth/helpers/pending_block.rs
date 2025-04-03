//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::BlockHeader;
use reth_ethereum_primitives::EthPrimitives;
use reth_evm::{ConfigureEvm, NextBlockEnvAttributes};
use reth_primitives_traits::{BlockTy, HeaderTy, ReceiptTy, SealedHeader};
use reth_rpc_eth_api::{helpers::LoadPendingBlock, FullEthApiTypes};
use reth_rpc_eth_types::PendingBlock;
use reth_storage_api::BlockReader;
use revm_primitives::B256;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: FullEthApiTypes<Primitives = EthPrimitives, Evm = EvmConfig>,
    Provider: BlockReader<Block = BlockTy<Self::Primitives>, Receipt = ReceiptTy<Self::Primitives>>,
    EvmConfig: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes>,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<BlockTy<Self::Primitives>, ReceiptTy<Self::Primitives>>>,
    > {
        self.inner.pending_block()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
    ) -> Result<<Self::Evm as ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(NextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root(),
            withdrawals: None,
        })
    }
}
