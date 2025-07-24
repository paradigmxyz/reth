//! L1 `eth` API types.

use std::fmt;
use reth_chainspec::ChainSpec;
use reth_rpc_convert::{RpcConverter, RpcConvert, transaction::{ConvertReceiptInput, ReceiptConverter}};
use reth_rpc_eth_types::{receipt::EthReceiptConverter, EthApiError};
use reth_primitives_traits::{SealedHeaderFor, TxTy};
use reth_rpc_convert::{RpcHeader, RpcReceipt, RpcTransaction, RpcTxReq};
use alloy_rpc_types_eth::TransactionInfo;
use alloy_consensus::transaction::Recovered;
use revm::{context::{BlockEnv, CfgEnv, TxEnv}};
use reth_ethereum_primitives::EthPrimitives;

/// An adapter wrapper around [`RpcConverter`] that converts errors to [`EthApiError`].
#[derive(Debug, Clone)]
pub struct EthRpcConverter<ChainSpecT = ChainSpec> {
    inner: RpcConverter<(), (), (), EthReceiptConverter<ChainSpecT>, ()>,
}

impl<ChainSpecT> EthRpcConverter<ChainSpecT> {
    /// Creates a new [`EthRpcConverter`] with the given receipt converter.
    pub fn new(receipt_converter: EthReceiptConverter<ChainSpecT>) -> Self {
        Self {
            inner: RpcConverter::new().with_receipt_converter(receipt_converter),
        }
    }
}

impl<ChainSpecT> RpcConvert for EthRpcConverter<ChainSpecT>
where
    ChainSpecT: Send + Sync + Clone + fmt::Debug + 'static,
    EthReceiptConverter<ChainSpecT>: ReceiptConverter<EthPrimitives>
        + Clone
        + fmt::Debug
        + Send
        + Sync
        + Unpin
        + 'static,
    <EthReceiptConverter<ChainSpecT> as ReceiptConverter<EthPrimitives>>::Error: core::error::Error + Send + Sync + 'static,
    <EthReceiptConverter<ChainSpecT> as ReceiptConverter<EthPrimitives>>::RpcReceipt: Into<alloy_rpc_types_eth::TransactionReceipt>,
{
    type Primitives = EthPrimitives;
    type Network = alloy_network::Ethereum;
    type TxEnv = TxEnv;
    type Error = EthApiError;

    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<RpcTransaction<Self::Network>, Self::Error> {
        self.inner.fill(tx, tx_info).map_err(EthApiError::from)
    }

    fn build_simulate_v1_transaction(
        &self,
        request: RpcTxReq<Self::Network>,
    ) -> Result<TxTy<Self::Primitives>, Self::Error> {
        self.inner.build_simulate_v1_transaction(request).map_err(EthApiError::from)
    }

    fn tx_env<Spec>(
        &self,
        request: RpcTxReq<Self::Network>,
        cfg_env: &CfgEnv<Spec>,
        block_env: &BlockEnv,
    ) -> Result<Self::TxEnv, Self::Error> {
        self.inner.tx_env(request, cfg_env, block_env).map_err(EthApiError::from)
    }

    fn convert_receipts(
        &self,
        receipts: Vec<ConvertReceiptInput<'_, Self::Primitives>>,
    ) -> Result<Vec<RpcReceipt<Self::Network>>, Self::Error> {
        self.inner.convert_receipts(receipts).map_err(EthApiError::from)
    }

    fn convert_header(
        &self,
        header: SealedHeaderFor<Self::Primitives>,
        block_size: usize,
    ) -> Result<RpcHeader<Self::Network>, Self::Error> {
        self.inner.convert_header(header, block_size).map_err(EthApiError::from)
    }
}

//tests for simulate
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxType};
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chainspec::MAINNET;
    use reth_rpc_eth_types::{receipt::EthReceiptConverter, simulate::resolve_transaction};
    use revm::database::CacheDB;

    #[test]
    fn test_resolve_transaction_empty_request() {
        let builder = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));
        let mut db = CacheDB::<reth_revm::db::EmptyDBTyped<reth_errors::ProviderError>>::default();
        let tx = TransactionRequest::default();
        let result = resolve_transaction(tx, 21000, 0, 1, &mut db, &builder).unwrap();

        // For an empty request, we should get a valid transaction with defaults
        let tx = result.into_inner();
        assert_eq!(tx.max_fee_per_gas(), 0);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(0));
        assert_eq!(tx.gas_price(), None);
    }

    #[test]
    fn test_resolve_transaction_legacy() {
        let mut db = CacheDB::<reth_revm::db::EmptyDBTyped<reth_errors::ProviderError>>::default();
        let builder = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));

        let tx = TransactionRequest { gas_price: Some(100), ..Default::default() };

        let tx = resolve_transaction(tx, 21000, 0, 1, &mut db, &builder).unwrap();

        assert_eq!(tx.tx_type(), TxType::Legacy);

        let tx = tx.into_inner();
        assert_eq!(tx.gas_price(), Some(100));
        assert_eq!(tx.max_priority_fee_per_gas(), None);
    }

    #[test]
    fn test_resolve_transaction_partial_eip1559() {
        let mut db = CacheDB::<reth_revm::db::EmptyDBTyped<reth_errors::ProviderError>>::default();
        let rpc_converter =
            RpcConverter::new().with_receipt_converter(EthReceiptConverter::new(MAINNET.clone()));

        let tx = TransactionRequest {
            max_fee_per_gas: Some(200),
            max_priority_fee_per_gas: Some(10),
            ..Default::default()
        };

        let result = resolve_transaction(tx, 21000, 0, 1, &mut db, &rpc_converter).unwrap();

        assert_eq!(result.tx_type(), TxType::Eip1559);
        let tx = result.into_inner();
        assert_eq!(tx.max_fee_per_gas(), 200);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(10));
        assert_eq!(tx.gas_price(), None);
    }
}
