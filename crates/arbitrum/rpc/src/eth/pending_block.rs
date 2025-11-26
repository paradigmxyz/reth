use std::sync::Arc;

use crate::{error::ArbEthApiError, eth::ArbEthApi};
use reth_primitives_traits::RecoveredBlock;
use reth_rpc_eth_api::{
    helpers::pending_block::{LoadPendingBlock, PendingEnvBuilder},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_rpc_eth_types::{builder::config::PendingBlockKind, EthApiError, PendingBlock};
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, ProviderBlock, ProviderReceipt, ReceiptProvider,
};

impl<N, Rpc> LoadPendingBlock for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<N::Primitives>>> {
        self.eth_api().pending_block()
    }

    #[inline]
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.eth_api().pending_env_builder()
    }

    #[inline]
    fn pending_block_kind(&self) -> PendingBlockKind {
        self.eth_api().pending_block_kind()
    }
}
