use alloy::{
    consensus::TxEnvelope,
    network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder},
    primitives::{Address, B256},
    rpc::types::eth::TransactionRequest,
    signers::wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder},
};
use reth_primitives::{Bytes, ChainSpec, Genesis, U256};
use std::sync::Arc;

/// Helper struct to customize the chain spec during e2e tests
pub struct TestSuite {
    pub chain_spec: Arc<ChainSpec>,
    test_account: Account,
}

impl TestSuite {
    /// Creates a new e2e test suite with a test account prefunded with 10_000 ETH from genesis
    /// allocations and the eth mainnet latest chainspec.
    pub fn new() -> Self {
        let chain_spec = TestSuite::chain_spec();
        let test_account = Account::new();
        Self { chain_spec, test_account }
    }

    /// Creates a signed transfer tx and returns its hash and raw bytes
    pub async fn transfer_tx(&self) -> (B256, Bytes) {
        let tx = self.test_account.transfer_tx().await;
        (tx.trie_hash(), tx.encoded_2718().into())
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
    wallet: LocalWallet,
}

impl Account {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    fn new() -> Self {
        let phrase = "test test test test test test test test test test test junk";
        let wallet = MnemonicBuilder::<English>::default().phrase(phrase).build().unwrap();
        Self { wallet }
    }

    /// Creates a static transfer and signs it
    async fn transfer_tx(&self) -> TxEnvelope {
        let tx = TransactionRequest {
            nonce: Some(0),
            value: Some(U256::from(100)),
            to: Some(Address::random()),
            gas_price: Some(U256::from(20e9)),
            gas: Some(U256::from(21000)),
            chain_id: Some(1),
            ..Default::default()
        };
        let signer = EthereumSigner::from(self.wallet.clone());
        tx.build(&signer).await.unwrap()
    }
}
