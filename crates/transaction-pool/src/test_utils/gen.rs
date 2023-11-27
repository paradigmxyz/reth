#![allow(missing_docs)]

use crate::{
    test_utils::{MockTransactionFactory, MockValidTx},
    EthPooledTransaction,
};
use rand::Rng;
use reth_primitives::{
    constants::MIN_PROTOCOL_BASE_FEE, sign_message, AccessList, Address, Bytes,
    FromRecoveredTransaction, Transaction, TransactionKind, TransactionSigned, TxEip1559, TxLegacy,
    TxValue, B256, MAINNET,
};

/// A generator for transactions for testing purposes
pub struct TransactionGenerator<R> {
    rng: R,
    signer_keys: Vec<B256>,
    base_fee: u128,
    gas_limit: u64,
}

impl<R: Rng> TransactionGenerator<R> {
    /// Initializes the generator with 10 random signers
    pub fn new(rng: R) -> Self {
        Self::with_num_signers(rng, 10)
    }

    /// Generates random random signers
    pub fn with_num_signers(mut rng: R, num_signers: usize) -> Self {
        let mut signer_keys = Vec::with_capacity(num_signers);
        for _ in 0..num_signers {
            signer_keys.push(B256::random());
        }
        Self { rng, signer_keys, base_fee: MIN_PROTOCOL_BASE_FEE as u128, gas_limit: 300_000 }
    }

    /// Adds a new signer to the set
    pub fn push_signer(&mut self, signer: B256) -> &mut Self {
        self.signer_keys.push(signer);
        self
    }

    /// Sets the default gas limit for all generated transactions
    pub fn set_gas_limit(&mut self, gas_limit: u64) -> &mut Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the default gas limit for all generated transactions
    pub fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the base fee for the generated transactions
    pub fn set_base_fee(&mut self, base_fee: u64) -> &mut Self {
        self.base_fee = base_fee as u128;
        self
    }

    /// Sets the base fee for the generated transactions
    pub fn with_base_fee(mut self, base_fee: u64) -> Self {
        self.base_fee = base_fee as u128;
        self
    }

    /// Adds the given signers to the set.
    pub fn extend_signers(&mut self, signers: impl IntoIterator<Item = B256>) -> &mut Self {
        self.signer_keys.extend(signers);
        self
    }

    /// Returns a random signer from the set
    fn rng_signer(&mut self) -> B256 {
        let idx = self.rng.gen_range(0..self.signer_keys.len());
        self.signer_keys[idx]
    }

    /// Creates a new transaction with a random signer
    pub fn transaction(&mut self) -> TransactionBuilder {
        TransactionBuilder::default()
            .signer(self.rng_signer())
            .max_fee_per_gas(self.base_fee)
            .max_priority_fee_per_gas(self.base_fee)
            .gas_limit(self.gas_limit)
    }

    /// Creates a new transaction with a random signer
    pub fn gen_eip1559(&mut self) -> TransactionSigned {
        self.transaction().into_eip1559()
    }

    pub fn gen_eip1559_pooled(&mut self) -> EthPooledTransaction {
        let tx = self.gen_eip1559().into_ecrecovered().unwrap();
        EthPooledTransaction::from_recovered_transaction(tx)
    }
}

/// A Builder type to configure and create a transaction.
pub struct TransactionBuilder {
    signer: B256,
    chain_id: u64,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    to: TransactionKind,
    value: TxValue,
    access_list: AccessList,
    input: Bytes,
}

impl TransactionBuilder {
    pub fn into_legacy(self) -> TransactionSigned {
        let Self {
            signer,
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        } = self;
        let tx: Transaction = TxLegacy {
            chain_id: Some(chain_id),
            nonce,
            gas_limit,
            gas_price: max_fee_per_gas,
            to,
            value,
            input,
        }
        .into();
        TransactionBuilder::signed(tx, signer)
    }

    pub fn into_eip1559(self) -> TransactionSigned {
        let Self {
            signer,
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        } = self;
        let tx: Transaction = TxEip1559 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        }
        .into();
        TransactionBuilder::signed(tx, signer)
    }

    fn signed(transaction: Transaction, signer: B256) -> TransactionSigned {
        let signature = sign_message(signer, transaction.signature_hash()).unwrap();
        TransactionSigned::from_transaction_and_signature(transaction, signature)
    }

    pub fn signer(mut self, signer: B256) -> Self {
        self.signer = signer;
        self
    }

    pub fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    pub fn nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    pub fn inc_nonce(mut self) -> Self {
        self.nonce += 1;
        self
    }

    pub fn decr_nonce(mut self) -> Self {
        self.nonce = self.nonce.saturating_sub(1);
        self
    }

    pub fn max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.max_fee_per_gas = max_fee_per_gas;
        self
    }

    pub fn max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.max_priority_fee_per_gas = max_priority_fee_per_gas;
        self
    }

    pub fn to(mut self, to: Address) -> Self {
        self.to = TransactionKind::Call(to);
        self
    }

    pub fn value(mut self, value: u128) -> Self {
        self.value = value.into();
        self
    }

    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.access_list = access_list;
        self
    }

    pub fn input(mut self, input: impl Into<Bytes>) -> Self {
        self.input = input.into();
        self
    }

    pub fn chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    pub fn set_chain_id(&mut self, chain_id: u64) -> &mut Self {
        self.chain_id = chain_id;
        self
    }

    pub fn set_nonce(&mut self, nonce: u64) -> &mut Self {
        self.nonce = nonce;
        self
    }

    pub fn set_gas_limit(&mut self, gas_limit: u64) -> &mut Self {
        self.gas_limit = gas_limit;
        self
    }

    pub fn set_max_fee_per_gas(&mut self, max_fee_per_gas: u128) -> &mut Self {
        self.max_fee_per_gas = max_fee_per_gas;
        self
    }

    pub fn set_max_priority_fee_per_gas(&mut self, max_priority_fee_per_gas: u128) -> &mut Self {
        self.max_priority_fee_per_gas = max_priority_fee_per_gas;
        self
    }

    pub fn set_to(&mut self, to: Address) -> &mut Self {
        self.to = TransactionKind::Call(to);
        self
    }

    pub fn set_value(&mut self, value: u128) -> &mut Self {
        self.value = value.into();
        self
    }

    pub fn set_access_list(&mut self, access_list: AccessList) -> &mut Self {
        self.access_list = access_list;
        self
    }

    pub fn set_signer(&mut self, signer: B256) -> &mut Self {
        self.signer = signer;
        self
    }

    pub fn set_input(&mut self, input: impl Into<Bytes>) -> &mut Self {
        self.input = input.into();
        self
    }
}

impl Default for TransactionBuilder {
    fn default() -> Self {
        Self {
            signer: B256::random(),
            chain_id: MAINNET.chain.id(),
            nonce: 0,
            gas_limit: 0,
            max_fee_per_gas: 0,
            max_priority_fee_per_gas: 0,
            to: Default::default(),
            value: Default::default(),
            access_list: Default::default(),
            input: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    #[test]
    fn test_generate_transaction() {
        let rng = thread_rng();
        let mut gen = TransactionGenerator::new(rng);
        let _tx = gen.transaction().into_legacy();
        let _tx = gen.transaction().into_eip1559();
    }
}
