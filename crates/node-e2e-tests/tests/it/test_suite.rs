use reth_primitives::{
    hex, sign_message, AccessList, Address, Bytes, ChainSpec, Genesis, Transaction,
    TransactionKind, TransactionSigned, TxEip1559, B256, U256,
};
use secp256k1::SecretKey;
use std::sync::Arc;

/// Helper struct to customize the chain spec during e2e tests
pub struct TestSuite {
    pub chain_spec: Arc<ChainSpec>,
    test_account: Account,
}

impl TestSuite {
    /// Creates a new e2e test suit with a random account and a custom chain spec
    pub fn new() -> Self {
        let chain_spec = TestSuite::chain_spec();
        let test_account = Account::new();

        Self { chain_spec, test_account }
    }

    /// Creates a new transfer tx
    pub fn transfer_tx(&self) -> TransactionSigned {
        self.test_account.transfer_tx()
    }

    /// Chain spec for e2e eth tests
    ///
    /// Includes 20 prefunded accounts with 10_000 ETH each derived from mnemonic "test test test
    /// test test test test test test test test junk".
    fn chain_spec() -> Arc<ChainSpec> {
        let genesis: Genesis = serde_json::from_str(include_str!("./genesis.json")).unwrap();
        Arc::new(genesis.into())
    }
}

/// One of the accounts of the genesis allocations.
pub struct Account {
    secret_key: SecretKey,
}

impl Account {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    fn new() -> Self {
        let secret_key = SecretKey::from_slice(&hex!(
            "609a8293c4cad0e409a52232dc325cab474e8f41c3285332259627cd101d27fb"
        ))
        .unwrap();
        Self { secret_key }
    }

    /// Creates a new transfer transaction
    pub fn transfer_tx(&self) -> TransactionSigned {
        let tx = Transaction::Eip1559(TxEip1559 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 21000,
            to: TransactionKind::Call(Address::random()),
            value: U256::from(1),
            input: Bytes::default(),
            max_fee_per_gas: 875000000,
            max_priority_fee_per_gas: 0,
            access_list: AccessList::default(),
        });
        Account::sign_transaction(&self.secret_key, tx)
    }
    /// Helper function to sign a transaction
    fn sign_transaction(secret_key: &SecretKey, transaction: Transaction) -> TransactionSigned {
        let tx_signature_hash = transaction.signature_hash();
        let signature =
            sign_message(B256::from_slice(secret_key.as_ref()), tx_signature_hash).unwrap();
        TransactionSigned::from_transaction_and_signature(transaction, signature)
    }
}
