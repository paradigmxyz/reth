//! L1 `eth` API types.

use alloy_consensus::SignableTransaction;
use alloy_network::Ethereum;
use alloy_primitives::Signature;
use alloy_rpc_types_eth::TransactionRequest;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm_ethereum::EthEvmConfig;
use reth_rpc_convert::{RpcConverter, TransactionConversionError};
use reth_rpc_eth_types::receipt::EthReceiptConverter;

/// Converter function used to build fake signed transactions for `eth_simulateV1`.
pub type EthSimulateTxConverter =
    fn(TransactionRequest) -> Result<TransactionSigned, TransactionConversionError>;

/// An [`RpcConverter`] with its generics set to Ethereum specific.
pub type EthRpcConverter<ChainSpec> = RpcConverter<
    Ethereum,
    EthEvmConfig,
    EthReceiptConverter<ChainSpec>,
    (),
    (),
    EthSimulateTxConverter,
>;

/// Builds a fake signed Ethereum transaction for `eth_simulateV1`.
pub fn build_simulate_v1_transaction(
    request: TransactionRequest,
) -> Result<TransactionSigned, TransactionConversionError> {
    let tx = request
        .build_consensus_tx()
        .map_err(|err| TransactionConversionError::FromTxReq(err.error))?;
    let signature = Signature::new(Default::default(), Default::default(), false);

    Ok(tx.into_signed(signature).into())
}

//tests for simulate
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxType};
    use alloy_primitives::{Address, TxKind, B256};
    use alloy_rpc_types_eth::TransactionRequest;
    use reth_chainspec::MAINNET;
    use reth_rpc_eth_types::simulate::resolve_transaction;
    use revm::database::CacheDB;

    fn test_rpc_converter() -> EthRpcConverter<reth_chainspec::ChainSpec> {
        RpcConverter::new(EthReceiptConverter::new(MAINNET.clone()))
            .with_sim_tx_converter(build_simulate_v1_transaction as EthSimulateTxConverter)
    }

    #[test]
    fn test_resolve_transaction_empty_request() {
        let builder = test_rpc_converter();
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
        let builder = test_rpc_converter();

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
        let rpc_converter = test_rpc_converter();

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

    #[test]
    fn test_resolve_simulate_blob_transaction_without_sidecar() {
        let mut db = CacheDB::<reth_revm::db::EmptyDBTyped<reth_errors::ProviderError>>::default();
        let rpc_converter = test_rpc_converter();

        let tx = TransactionRequest {
            max_fee_per_gas: Some(16),
            max_priority_fee_per_gas: Some(0),
            max_fee_per_blob_gas: Some(10),
            blob_versioned_hashes: Some(vec![B256::repeat_byte(1)]),
            to: Some(TxKind::Call(Address::ZERO)),
            ..Default::default()
        };

        let result = resolve_transaction(tx, 21_000, 15, 1, &mut db, &rpc_converter)
            .expect("blob transaction without sidecar should be valid for simulation");

        assert_eq!(result.tx_type(), TxType::Eip4844);
    }
}
