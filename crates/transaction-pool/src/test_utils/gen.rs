use crate::EthPooledTransaction;
use rand::Rng;
use reth_primitives::{
    constants::MIN_PROTOCOL_BASE_FEE, sign_message, AccessList, Address, Bytes,
    FromRecoveredTransaction, Transaction, TransactionKind, TransactionSigned, TxEip1559,
    TxEip4844, TxLegacy, B256, MAINNET, U256,
};

/// A generator for transactions for testing purposes.
#[derive(Debug)]
pub struct TransactionGenerator<R> {
    /// The random number generator used for generating keys and selecting signers.
    pub rng: R,
    /// The set of signer keys available for transaction generation.
    pub signer_keys: Vec<B256>,
    /// The base fee for transactions.
    pub base_fee: u128,
    /// The gas limit for transactions.
    pub gas_limit: u64,
}

impl<R: Rng> TransactionGenerator<R> {
    /// Initializes the generator with 10 random signers
    pub fn new(rng: R) -> Self {
        Self::with_num_signers(rng, 10)
    }

    /// Generates random random signers
    pub fn with_num_signers(rng: R, num_signers: usize) -> Self {
        Self {
            rng,
            signer_keys: (0..num_signers).map(|_| B256::random()).collect(),
            base_fee: MIN_PROTOCOL_BASE_FEE as u128,
            gas_limit: 300_000,
        }
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
    pub const fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the base fee for the generated transactions
    pub fn set_base_fee(&mut self, base_fee: u64) -> &mut Self {
        self.base_fee = base_fee as u128;
        self
    }

    /// Sets the base fee for the generated transactions
    pub const fn with_base_fee(mut self, base_fee: u64) -> Self {
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

    /// Creates a new transaction with a random signer
    pub fn gen_eip4844(&mut self) -> TransactionSigned {
        self.transaction().into_eip4844()
    }

    /// Generates and returns a pooled EIP-1559 transaction with a random signer.
    pub fn gen_eip1559_pooled(&mut self) -> EthPooledTransaction {
        EthPooledTransaction::from_recovered_transaction(
            self.gen_eip1559().into_ecrecovered().unwrap(),
        )
    }
    /// Generates and returns a pooled EIP-4844 transaction with a random signer.
    pub fn gen_eip4844_pooled(&mut self) -> EthPooledTransaction {
        EthPooledTransaction::from_recovered_transaction(
            self.gen_eip4844().into_ecrecovered().unwrap(),
        )
    }
}

/// A Builder type to configure and create a transaction.
#[derive(Debug)]
pub struct TransactionBuilder {
    /// The signer used to sign the transaction.
    pub signer: B256,
    /// The chain ID on which the transaction will be executed.
    pub chain_id: u64,
    /// The nonce value for the transaction to prevent replay attacks.
    pub nonce: u64,
    /// The maximum amount of gas units that the transaction can consume.
    pub gas_limit: u64,
    /// The maximum fee per gas unit that the sender is willing to pay.
    pub max_fee_per_gas: u128,
    /// The maximum priority fee per gas unit that the sender is willing to pay for faster
    /// processing.
    pub max_priority_fee_per_gas: u128,
    /// The recipient or contract address of the transaction.
    pub to: TransactionKind,
    /// The value to be transferred in the transaction.
    pub value: U256,
    /// The list of addresses and storage keys that the transaction can access.
    pub access_list: AccessList,
    /// The input data for the transaction, typically containing function parameters for contract
    /// calls.
    pub input: Bytes,
}

impl TransactionBuilder {
    /// Converts the transaction builder into a legacy transaction format.
    pub fn into_legacy(self) -> TransactionSigned {
        TransactionBuilder::signed(
            TxLegacy {
                chain_id: Some(self.chain_id),
                nonce: self.nonce,
                gas_limit: self.gas_limit,
                gas_price: self.max_fee_per_gas,
                to: self.to,
                value: self.value,
                input: self.input,
            }
            .into(),
            self.signer,
        )
    }

    /// Converts the transaction builder into a transaction format using EIP-1559.
    pub fn into_eip1559(self) -> TransactionSigned {
        TransactionBuilder::signed(
            TxEip1559 {
                chain_id: self.chain_id,
                nonce: self.nonce,
                gas_limit: self.gas_limit,
                max_fee_per_gas: self.max_fee_per_gas,
                max_priority_fee_per_gas: self.max_priority_fee_per_gas,
                to: self.to,
                value: self.value,
                access_list: self.access_list,
                input: self.input,
            }
            .into(),
            self.signer,
        )
    }
    /// Converts the transaction builder into a transaction format using EIP-4844.
    pub fn into_eip4844(self) -> TransactionSigned {
        TransactionBuilder::signed(
            TxEip4844 {
                chain_id: self.chain_id,
                nonce: self.nonce,
                gas_limit: self.gas_limit,
                max_fee_per_gas: self.max_fee_per_gas,
                max_priority_fee_per_gas: self.max_priority_fee_per_gas,
                to: self.to,
                value: self.value,
                access_list: self.access_list,
                input: self.input,
                blob_versioned_hashes: Default::default(),
                max_fee_per_blob_gas: Default::default(),
            }
            .into(),
            self.signer,
        )
    }

    /// Signs the provided transaction using the specified signer and returns a signed transaction.
    fn signed(transaction: Transaction, signer: B256) -> TransactionSigned {
        let signature = sign_message(signer, transaction.signature_hash()).unwrap();
        TransactionSigned::from_transaction_and_signature(transaction, signature)
    }

    /// Sets the signer for the transaction builder.
    pub const fn signer(mut self, signer: B256) -> Self {
        self.signer = signer;
        self
    }

    /// Sets the gas limit for the transaction builder.
    pub const fn gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the nonce for the transaction builder.
    pub const fn nonce(mut self, nonce: u64) -> Self {
        self.nonce = nonce;
        self
    }

    /// Increments the nonce value of the transaction builder by 1.
    pub fn inc_nonce(mut self) -> Self {
        self.nonce += 1;
        self
    }

    /// Decrements the nonce value of the transaction builder by 1, avoiding underflow.
    pub fn decr_nonce(mut self) -> Self {
        self.nonce = self.nonce.saturating_sub(1);
        self
    }

    /// Sets the maximum fee per gas for the transaction builder.
    pub const fn max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.max_fee_per_gas = max_fee_per_gas;
        self
    }

    /// Sets the maximum priority fee per gas for the transaction builder.
    pub const fn max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.max_priority_fee_per_gas = max_priority_fee_per_gas;
        self
    }

    /// Sets the recipient or contract address for the transaction builder.
    pub const fn to(mut self, to: Address) -> Self {
        self.to = TransactionKind::Call(to);
        self
    }

    /// Sets the value to be transferred in the transaction.
    pub fn value(mut self, value: u128) -> Self {
        self.value = U256::from(value);
        self
    }

    /// Sets the access list for the transaction builder.
    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.access_list = access_list;
        self
    }

    /// Sets the transaction input data.
    pub fn input(mut self, input: impl Into<Bytes>) -> Self {
        self.input = input.into();
        self
    }

    /// Sets the chain ID for the transaction.
    pub const fn chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Sets the chain ID for the transaction, mutable reference version.
    pub fn set_chain_id(&mut self, chain_id: u64) -> &mut Self {
        self.chain_id = chain_id;
        self
    }

    /// Sets the nonce for the transaction, mutable reference version.
    pub fn set_nonce(&mut self, nonce: u64) -> &mut Self {
        self.nonce = nonce;
        self
    }

    /// Sets the gas limit for the transaction, mutable reference version.
    pub fn set_gas_limit(&mut self, gas_limit: u64) -> &mut Self {
        self.gas_limit = gas_limit;
        self
    }

    /// Sets the maximum fee per gas for the transaction, mutable reference version.
    pub fn set_max_fee_per_gas(&mut self, max_fee_per_gas: u128) -> &mut Self {
        self.max_fee_per_gas = max_fee_per_gas;
        self
    }

    /// Sets the maximum priority fee per gas for the transaction, mutable reference version.
    pub fn set_max_priority_fee_per_gas(&mut self, max_priority_fee_per_gas: u128) -> &mut Self {
        self.max_priority_fee_per_gas = max_priority_fee_per_gas;
        self
    }

    /// Sets the recipient or contract address for the transaction, mutable reference version.
    pub fn set_to(&mut self, to: Address) -> &mut Self {
        self.to = TransactionKind::Call(to);
        self
    }

    /// Sets the value to be transferred in the transaction, mutable reference version.
    pub fn set_value(&mut self, value: u128) -> &mut Self {
        self.value = U256::from(value);
        self
    }

    /// Sets the access list for the transaction, mutable reference version.
    pub fn set_access_list(&mut self, access_list: AccessList) -> &mut Self {
        self.access_list = access_list;
        self
    }

    /// Sets the signer for the transaction, mutable reference version.
    pub fn set_signer(&mut self, signer: B256) -> &mut Self {
        self.signer = signer;
        self
    }

    /// Sets the transaction input data, mutable reference version.
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
