use crate::{provider::SenderTxReader, CustomNode};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_ethereum::{
    chainspec::ChainSpec,
    evm::revm::primitives::{alloy_primitives::U64, Address},
    node::{
        api::{FullNodeTypes, NodeTypesWithDBAdapter},
        builder::NodeAdapter,
    },
    provider::{
        db::{database_metrics::DatabaseMetrics, Database},
        providers::BlockchainProvider,
    },
    rpc::{
        eth::{
            helpers::types::EthRpcConverter, primitives::Transaction, EthApiError, EthApiServer,
        },
        EthApi,
    },
};

/// jsonrpsee trait for overriding `eth_transactionBySenderAndNonce`.
#[rpc(server, namespace = "eth")]
pub trait EthApiOverride {
    /// Returns information about a transaction by sender and nonce.
    #[method(name = "getTransactionBySenderAndNonce")]
    async fn transaction_by_sender_and_nonce(
        &self,
        address: Address,
        nonce: U64,
    ) -> RpcResult<Option<Transaction>>;
}

pub struct EthApiOverrides<N: FullNodeTypes<Types = CustomNode>> {
    pub(crate) inner: EthApi<NodeAdapter<N>, EthRpcConverter<ChainSpec>>,
}

#[async_trait::async_trait]
impl<DB, N> EthApiOverrideServer for EthApiOverrides<N>
where
    DB: Database + Clone + Unpin + DatabaseMetrics + 'static,
    N: FullNodeTypes<
        DB = DB,
        Types = CustomNode,
        Provider = BlockchainProvider<NodeTypesWithDBAdapter<CustomNode, DB>>,
    >,
{
    async fn transaction_by_sender_and_nonce(
        &self,
        address: Address,
        nonce: U64,
    ) -> RpcResult<Option<Transaction>> {
        if let Some(tx) = self
            .inner
            .provider()
            .transaction_by_sender_and_nonce(address, nonce.to())
            .map_err(EthApiError::from)?
        {
            // note: redundant because we already have the tx
            return self.inner.transaction_by_hash(*tx.hash()).await
        }
        self.inner.transaction_by_sender_and_nonce(address, nonce).await
    }
}
