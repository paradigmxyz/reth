use std::marker::PhantomData;

use crate::{
    validate::ValidTransaction, EthPooledTransaction, PoolTransaction, TransactionOrigin,
    TransactionValidationOutcome, TransactionValidator,
};
use reth_storage_api::{
    errors::provider::{ProviderError, ProviderResult},
    StateProvider, StateProviderBox,
};

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

    async fn validate_transaction_stateless(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> Result<Self::Transaction, TransactionValidationOutcome<Self::Transaction>> {
        // Always return valid
        let authorities = transaction.authorization_list().map(|auths| {
            auths.iter().flat_map(|auth| auth.recover_authority()).collect::<Vec<_>>()
        });
        let outcome = TransactionValidationOutcome::Valid {
            balance: *transaction.cost(),
            state_nonce: transaction.nonce(),
            bytecode_hash: None,
            transaction: ValidTransaction::Valid(transaction),
            propagate: self.propagate,
            authorities,
        };
        Err(outcome)
    }

    async fn validate_transaction_stateful(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
        _state: &dyn StateProvider,
    ) -> TransactionValidationOutcome<Self::Transaction> {
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

    fn latest_state_provider(&self) -> ProviderResult<StateProviderBox> {
        Err(ProviderError::UnsupportedProvider)
    }
}
