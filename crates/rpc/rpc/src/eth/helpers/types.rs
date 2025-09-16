//! L1 `eth` API types.

use alloy_consensus::{ReceiptWithBloom, TxType};
use reth_ethereum_primitives::Receipt;
use reth_evm_ethereum::EthEvmConfig;
use reth_rpc_convert::{RpcConverter, RpcTypes};
use reth_rpc_eth_types::receipt::EthReceiptConverter;

/// Ethereum specific RPC types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EthereumRpcTypes;

impl RpcTypes for EthereumRpcTypes {
    type Header = alloy_rpc_types_eth::Header;
    type Receipt = alloy_rpc_types_eth::TransactionReceipt<
        ReceiptWithBloom<Receipt<TxType, alloy_rpc_types_eth::Log>>,
    >;
    type TransactionRequest = alloy_rpc_types_eth::TransactionRequest;
    type TransactionResponse = alloy_rpc_types_eth::Transaction;
}

/// An [`RpcConverter`] with its generics set to Ethereum specific.
pub type EthRpcConverter<ChainSpec> =
    RpcConverter<EthereumRpcTypes, EthEvmConfig, EthReceiptConverter<ChainSpec>>;

//tests for simulate
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxType};
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chainspec::MAINNET;
    use reth_rpc_eth_types::simulate::resolve_transaction;
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
        let rpc_converter = EthRpcConverter::new(EthReceiptConverter::new(MAINNET.clone()));

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
