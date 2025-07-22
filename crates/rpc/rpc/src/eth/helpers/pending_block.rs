//! Support for building a pending block with transactions from local view of mempool.

use crate::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{pending_block::PendingEnvBuilder, LoadPendingBlock},
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};

impl<N, Rpc> LoadPendingBlock for EthApi<N, Rpc>
where
    N: RpcNodeCore,
    EthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives>,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<Self::Primitives>>> {
        self.inner.pending_block()
    }

    #[inline]
    fn pending_block_mode(&self) -> reth_rpc_eth_types::PendingBlockMode {
        self.inner.pending_block_mode()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<ProviderHeader<Self::Provider>>,
    ) -> Result<<Self::Evm as reth_evm::ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(NextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root().map(|_| B256::ZERO),
            withdrawals: parent.withdrawals_root().map(|_| Default::default()),
        }
        .into())
    }
}
