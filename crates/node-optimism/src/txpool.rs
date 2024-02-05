//! OP transaction pool types

use reth_provider::BlockReaderIdExt;
use reth_transaction_pool::EthTransactionValidator;

/// Validator for Ethereum transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, T>
    where
        Client: BlockReaderIdExt,
{
    /// The type that performs the actual validation.
    inner: EthTransactionValidator<Client, T>,
}