//! Builds an RPC receipt response w.r.t. data layout of network.

use alloy_consensus::transaction::TransactionMeta;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{ReceiptTy, TxTy};
use reth_rpc_eth_api::{helpers::LoadReceipt, FromEthApiError, RpcNodeCoreExt, RpcReceipt};
use reth_rpc_eth_types::{EthApiError, EthReceiptBuilder};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadReceipt for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: RpcNodeCoreExt<Primitives = EthPrimitives, Provider = Provider>,
    Provider: ChainSpecProvider,
{
    async fn build_transaction_receipt(
        &self,
        tx: TxTy<Self::Primitives>,
        meta: TransactionMeta,
        receipt: ReceiptTy<Self::Primitives>,
    ) -> Result<RpcReceipt<Self::NetworkTypes>, Self::Error> {
        let hash = meta.block_hash;
        // get all receipts for the block
        let all_receipts = self
            .cache()
            .get_receipts(hash)
            .await
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(hash.into()))?;
        let blob_params = self.provider().chain_spec().blob_params_at_timestamp(meta.timestamp);

        Ok(EthReceiptBuilder::new(&tx, meta, &receipt, &all_receipts, blob_params)?.build())
    }
}
