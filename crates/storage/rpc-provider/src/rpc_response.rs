//! Unified converter for RPC network responses to primitive types.

use alloy_network::Network;
use alloy_rpc_types::eth::{Block, Transaction, TransactionReceipt};
use core::fmt::Debug;
use reth_ethereum_primitives::{Receipt, TransactionSigned};

/// Error type used by [`RpcResponseConverter`].
#[derive(Debug)]
pub struct RpcResponseConverterError(pub Box<dyn core::error::Error + Send + Sync>);

impl core::fmt::Display for RpcResponseConverterError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&self.0, f)
    }
}

impl core::error::Error for RpcResponseConverterError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        self.0.source()
    }
}

/// Converts RPC network responses (blocks, transactions, receipts) into primitive types.
pub trait RpcResponseConverter<N: Network>: Send + Sync + Debug + 'static {
    /// Primitive block type.
    type Block;
    /// Primitive transaction type.
    type Transaction;
    /// Primitive receipt type.
    type Receipt;

    /// Converts a network block response to a primitive block.
    fn block(&self, response: N::BlockResponse) -> Result<Self::Block, RpcResponseConverterError>;

    /// Converts a network transaction response to a primitive transaction.
    fn transaction(
        &self,
        response: N::TransactionResponse,
    ) -> Result<Self::Transaction, RpcResponseConverterError>;

    /// Converts a network receipt response to a primitive receipt.
    fn receipt(
        &self,
        response: N::ReceiptResponse,
    ) -> Result<Self::Receipt, RpcResponseConverterError>;
}

/// Default Ethereum converter using upstream `From` impls.
#[derive(Debug, Clone, Copy, Default)]
pub struct EthRpcConverter;

impl RpcResponseConverter<alloy_network::Ethereum> for EthRpcConverter {
    type Block = alloy_consensus::Block<TransactionSigned>;
    type Transaction = TransactionSigned;
    type Receipt = Receipt;

    fn block(&self, response: Block) -> Result<Self::Block, RpcResponseConverterError> {
        Ok(response.into())
    }

    fn transaction(
        &self,
        response: Transaction,
    ) -> Result<Self::Transaction, RpcResponseConverterError> {
        Ok(response.into_inner().into())
    }

    fn receipt(
        &self,
        response: TransactionReceipt,
    ) -> Result<Self::Receipt, RpcResponseConverterError> {
        Ok(response.into_inner().into())
    }
}
