//! OP transaction pool types
use reth_primitives::SealedBlock;
use reth_provider::{BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{EthPoolTransaction, EthTransactionValidator, TransactionOrigin, TransactionValidationOutcome, TransactionValidator};

/// Validator for Ethereum transactions.
#[derive(Debug, Clone)]
pub struct OpTransactionValidator<Client, T>
{
    /// The type that performs the actual validation.
    inner: EthTransactionValidator<Client, T>,
}


#[async_trait::async_trait]
impl<Client, Tx> TransactionValidator for OpTransactionValidator<Client, Tx>
    where
        Client: StateProviderFactory + BlockReaderIdExt,
        Tx: EthPoolTransaction,
{
    type Transaction = Tx;

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        self.inner.validate_one(origin, transaction).await
    }

    async fn validate_transactions(
        &self,
        transactions: Vec<(TransactionOrigin, Self::Transaction)>,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.inner.validate_all(transactions).await
    }

    fn on_new_head_block(&self, new_tip_block: &SealedBlock) {
        self.inner.on_new_head_block(new_tip_block)
    }
}