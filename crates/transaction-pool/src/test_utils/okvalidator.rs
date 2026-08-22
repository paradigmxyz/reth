use std::marker::PhantomData;

use crate::{
    error::InvalidPoolTransactionError,
    validate::{ValidTransaction, NOOP_ACCOUNT_READER},
    EthPooledTransaction, PoolTransaction, TransactionOrigin, TransactionValidationOutcome,
    TransactionValidator,
};
use reth_ethereum_primitives::Block;

/// A transaction validator that determines all transactions to be valid.
#[derive(Debug)]
#[non_exhaustive]
pub struct OkValidator<T = EthPooledTransaction> {
    _phantom: PhantomData<T>,
    /// Whether to mark transactions as propagatable.
    propagate: bool,
}

impl<T> OkValidator<T> {
    /// Determines whether transactions should be allowed to be propagated
    pub const fn set_propagate_transactions(mut self, propagate: bool) -> Self {
        self.propagate = propagate;
        self
    }

    fn valid_outcome(&self, transaction: T) -> TransactionValidationOutcome<T>
    where
        T: PoolTransaction,
    {
        let authorities = transaction.authorization_list().map(|auths| {
            auths.iter().flat_map(|auth| auth.recover_authority()).collect::<Vec<_>>()
        });
        TransactionValidationOutcome::Valid {
            balance: *transaction.cost(),
            state_nonce: transaction.nonce(),
            bytecode_hash: None,
            transaction: ValidTransaction::Valid(transaction),
            propagate: self.propagate,
            authorities,
        }
    }
}

impl<T> Default for OkValidator<T> {
    fn default() -> Self {
        Self { _phantom: Default::default(), propagate: false }
    }
}

impl<T> TransactionValidator for OkValidator<T>
where
    T: PoolTransaction,
{
    type Transaction = T;
    type Block = Block;

    fn validate_stateless(
        &self,
        _origin: TransactionOrigin,
        _transaction: &Self::Transaction,
    ) -> Result<(), InvalidPoolTransactionError> {
        Ok(())
    }

    fn validate_stateful<S>(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
        _state: &S,
    ) -> TransactionValidationOutcome<Self::Transaction>
    where
        S: reth_storage_api::AccountReader + reth_storage_api::BytecodeReader + ?Sized,
    {
        self.valid_outcome(transaction)
    }

    async fn validate_transaction(
        &self,
        origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        match self.validate_stateless(origin, &transaction) {
            Err(err) => TransactionValidationOutcome::Invalid(transaction, err),
            Ok(()) => self.validate_stateful(origin, transaction, &NOOP_ACCOUNT_READER),
        }
    }

    async fn validate_transactions(
        &self,
        transactions: impl IntoIterator<Item = (TransactionOrigin, Self::Transaction), IntoIter: Send>
            + Send,
    ) -> Vec<TransactionValidationOutcome<Self::Transaction>> {
        self.validate_transactions_with_state(transactions, &NOOP_ACCOUNT_READER)
    }
}
