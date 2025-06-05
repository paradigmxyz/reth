//! L1 `eth` API types.

use alloy_consensus::TxEnvelope;
use alloy_network::{Ethereum, Network};
use alloy_rpc_types::TransactionRequest;
use alloy_rpc_types_eth::{Transaction, TransactionInfo};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{Recovered, TxTy};
use reth_rpc_eth_api::EthApiTypes;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_types_compat::TransactionCompat;

/// A standalone [`EthApiTypes`] implementation for Ethereum.
#[derive(Debug, Clone, Copy, Default)]
pub struct EthereumEthApiTypes(EthTxBuilder);

impl EthApiTypes for EthereumEthApiTypes {
    type Error = EthApiError;
    type NetworkTypes = Ethereum;
    type TransactionCompat = EthTxBuilder;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        &self.0
    }
}

/// Builds RPC transaction response for l1.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EthTxBuilder;

impl TransactionCompat for EthTxBuilder
where
    Self: Send + Sync,
{
    type Primitives = EthPrimitives;
    type Transaction = <Ethereum as Network>::TransactionResponse;

    type Error = EthApiError;

    fn fill(
        &self,
        tx: Recovered<TxTy<Self::Primitives>>,
        tx_info: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error> {
        let tx = tx.convert::<TxEnvelope>();
        Ok(Transaction::from_transaction(tx, tx_info))
    }

    fn build_simulate_v1_transaction(
        &self,
        request: TransactionRequest,
    ) -> Result<TxTy<Self::Primitives>, Self::Error> {
        TransactionRequest::build_typed_simulate_transaction(request)
            .map_err(|_| EthApiError::TransactionConversionError)
    }
}

//tests for simulate
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxType};
    use reth_rpc_eth_types::simulate::resolve_transaction;
    use revm::database::CacheDB;

    #[test]
    fn test_resolve_transaction_empty_request() {
        let builder = EthTxBuilder::default();
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
        let builder = EthTxBuilder::default();

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
        let builder = EthTxBuilder::default();

        let tx = TransactionRequest {
            max_fee_per_gas: Some(200),
            max_priority_fee_per_gas: Some(10),
            ..Default::default()
        };

        let result = resolve_transaction(tx, 21000, 0, 1, &mut db, &builder).unwrap();

        assert_eq!(result.tx_type(), TxType::Eip1559);
        let tx = result.into_inner();
        assert_eq!(tx.max_fee_per_gas(), 200);
        assert_eq!(tx.max_priority_fee_per_gas(), Some(10));
        assert_eq!(tx.gas_price(), None);
    }
}
