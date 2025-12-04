//! Conversion traits for receipt responses to primitive receipt types.

use alloy_network::Network;
use std::convert::Infallible;

/// Trait for converting network receipt responses to primitive receipt types.
pub trait TryFromReceiptResponse<N: Network> {
    /// The error type returned if the conversion fails.
    type Error: core::error::Error + Send + Sync + Unpin;

    /// Converts a network receipt response to a primitive receipt type.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Self)` on successful conversion, or `Err(Self::Error)` if the conversion fails.
    fn from_receipt_response(receipt_response: N::ReceiptResponse) -> Result<Self, Self::Error>
    where
        Self: Sized;
}

impl TryFromReceiptResponse<alloy_network::Ethereum> for reth_ethereum_primitives::Receipt {
    type Error = Infallible;

    fn from_receipt_response(
        receipt_response: alloy_rpc_types_eth::TransactionReceipt,
    ) -> Result<Self, Self::Error> {
        Ok(receipt_response.into_inner().into())
    }
}

#[cfg(feature = "op")]
impl TryFromReceiptResponse<op_alloy_network::Optimism> for reth_optimism_primitives::OpReceipt {
    type Error = Infallible;

    fn from_receipt_response(
        receipt_response: op_alloy_rpc_types::OpTransactionReceipt,
    ) -> Result<Self, Self::Error> {
        Ok(receipt_response.inner.inner.map_logs(Into::into).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::ReceiptEnvelope;
    use alloy_network::Ethereum;
    use reth_ethereum_primitives::Receipt;

    #[test]
    fn test_try_from_receipt_response() {
        let rpc_receipt = alloy_rpc_types_eth::TransactionReceipt {
            inner: ReceiptEnvelope::Eip1559(Default::default()),
            transaction_hash: Default::default(),
            transaction_index: None,
            block_hash: None,
            block_number: None,
            gas_used: 0,
            effective_gas_price: 0,
            blob_gas_used: None,
            blob_gas_price: None,
            from: Default::default(),
            to: None,
            contract_address: None,
        };
        let result =
            <Receipt as TryFromReceiptResponse<Ethereum>>::from_receipt_response(rpc_receipt);
        assert!(result.is_ok());
    }

    #[cfg(feature = "op")]
    #[test]
    fn test_try_from_receipt_response_optimism() {
        use op_alloy_consensus::OpReceiptEnvelope;
        use op_alloy_network::Optimism;
        use op_alloy_rpc_types::OpTransactionReceipt;
        use reth_optimism_primitives::OpReceipt;

        let op_receipt = OpTransactionReceipt {
            inner: alloy_rpc_types_eth::TransactionReceipt {
                inner: OpReceiptEnvelope::Eip1559(Default::default()),
                transaction_hash: Default::default(),
                transaction_index: None,
                block_hash: None,
                block_number: None,
                gas_used: 0,
                effective_gas_price: 0,
                blob_gas_used: None,
                blob_gas_price: None,
                from: Default::default(),
                to: None,
                contract_address: None,
            },
            l1_block_info: Default::default(),
        };
        let result =
            <OpReceipt as TryFromReceiptResponse<Optimism>>::from_receipt_response(op_receipt);
        assert!(result.is_ok());
    }
}
