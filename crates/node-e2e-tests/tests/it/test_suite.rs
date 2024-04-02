use reth_primitives::{
    keccak256, revm_primitives::fixed_bytes, sign_message, AccessList, Address, Bytes, ChainConfig,
    ChainSpec, Genesis, GenesisAccount, Transaction, TransactionKind, TransactionSigned, TxEip1559,
    B256, U256,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use std::{collections::BTreeMap, sync::Arc};

/// Helper struct to customize the chain spec during e2e tests
pub struct TestSuite {
    pub account: Account,
    pub chain_spec: Arc<ChainSpec>,
}

impl TestSuite {
    /// Creates a new e2e test suit with a random account and a custom chain spec
    pub fn new() -> Self {
        let account = Account::new();
        let chain_spec = TestSuite::chain_spec(&account);
        Self { account, chain_spec }
    }
    /// Returns the raw transfer transaction
    pub fn transfer_tx(&self) -> TransactionSigned {
        self.account.transfer_tx()
    }
    /// Creates a custom chain spec and allocates the initial balance to the given account
    fn chain_spec(account: &Account) -> Arc<ChainSpec> {
        let sk = B256::from_slice(&account.secret_key.secret_bytes());
        let mut alloc = BTreeMap::new();
        let genesis_acc = GenesisAccount {
            balance: U256::from(1_000_000_000_000_000_000_000_000u128),
            code: None,
            storage: None,
            nonce: Some(0),
            private_key: Some(sk),
        };
        alloc.insert(account.pubkey, genesis_acc);

        let genesis = Genesis {
            nonce: 0,
            timestamp: 0,
            extra_data: fixed_bytes!("00").into(),
            gas_limit: 30_000_000,
            difficulty: U256::from(0),
            mix_hash: B256::ZERO,
            coinbase: Address::ZERO,
            alloc,
            number: Some(0),
            config: TestSuite::chain_config(),
            base_fee_per_gas: None,
            blob_gas_used: None,
            excess_blob_gas: None,
        };

        Arc::new(genesis.into())
    }

    fn chain_config() -> ChainConfig {
        let chain_config = r#"
{
		"chainId": 1,
		"homesteadBlock": 0,
		"daoForkSupport": true,
		"eip150Block": 0,
		"eip155Block": 0,
		"eip158Block": 0,
		"byzantiumBlock": 0,
		"constantinopleBlock": 0,
		"petersburgBlock": 0,
		"istanbulBlock": 0,
		"muirGlacierBlock": 0,
		"berlinBlock": 0,
		"londonBlock": 0,
		"arrowGlacierBlock": 0,
		"grayGlacierBlock": 0,
		"shanghaiTime": 0,
        "cancunTime":0,
		"terminalTotalDifficulty": 0,
		"terminalTotalDifficultyPassed": true
}
"#;
        serde_json::from_str(chain_config).unwrap()
    }
}

/// The main account used for the e2e tests
pub struct Account {
    pubkey: Address,
    secret_key: SecretKey,
}

impl Account {
    /// Creates a new account from a random secret key and pub key
    fn new() -> Self {
        let (secret_key, pubkey) = Account::random();
        Self { pubkey, secret_key }
    }

    /// Generates a random secret key and pub key
    fn random() -> (SecretKey, Address) {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let secp = Secp256k1::new();
        let public_key = PublicKey::from_secret_key(&secp, &secret_key);
        let hash = keccak256(&public_key.serialize_uncompressed()[1..]);
        let pubkey = Address::from_slice(&hash[12..]);
        (secret_key, pubkey)
    }

    /// Creates a new transfer transaction
    pub fn transfer_tx(&self) -> TransactionSigned {
        let tx = Transaction::Eip1559(TxEip1559 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 21000,
            to: TransactionKind::Call(Address::random()),
            value: U256::from(1000),
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
