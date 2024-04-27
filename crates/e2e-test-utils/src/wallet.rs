use crate::transaction::TransactionTestContext;
use alloy_signer::Signer;
use alloy_signer_wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder};
use reth_primitives::{Bytes, ChainId, MAINNET};

/// Default test mnemonic used by the accounts in the test genesis allocations
const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

/// Wallet generator that can generate wallets from a given phrase, chain id and amount
#[derive(Clone, Debug)]
pub struct WalletGenerator {
    chain_id: u64,
    amount: usize,
    phrase: String,
    derivation_path: String,
}

impl WalletGenerator {
    /// Creates a new wallet generator defaulting to MAINNET specs
    pub fn new(amount: usize) -> Self {
        Self {
            chain_id: 1,
            amount,
            phrase: TEST_MNEMONIC.to_string(),
            derivation_path: "m/44'/60'/0'/0/".to_string(),
        }
    }

    /// Sets the mnemonic phrase that will be used to generate the wallets
    pub fn phrase(mut self, phrase: impl Into<String>) -> Self {
        self.phrase = phrase.into();
        self
    }

    /// Sets the chain id that will be used to generate the wallets
    pub fn chain_id(mut self, chain_id: impl Into<u64>) -> Self {
        self.chain_id = chain_id.into();
        self
    }

    /// Sets the derivation path that will be used to generate the wallets, following the BIP44
    /// <https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki>
    pub fn derivation_path(mut self, derivation_path: impl Into<String>) -> Self {
        let mut path = derivation_path.into();
        if !path.ends_with('/') {
            path.push('/');
        }
        self.derivation_path = path;
        self
    }

    fn get_derivation_path(&self) -> &str {
        &self.derivation_path
    }
}

impl WalletGenerator {
    /// Generates wallets from a previously set phrase, chain id and amount
    pub fn gen(&self) -> Vec<Wallet> {
        let builder = MnemonicBuilder::<English>::default().phrase(self.phrase.as_str());

        // use the derivation path
        let derivation_path = self.get_derivation_path();

        let mut wallets = Vec::with_capacity(self.amount);
        for idx in 0..self.amount {
            let builder =
                builder.clone().derivation_path(&format!("{derivation_path}{idx}")).unwrap();
            let inner = builder.build().unwrap().with_chain_id(Some(self.chain_id));
            let wallet = Wallet::new(inner, self.chain_id);
            wallets.push(wallet)
        }
        wallets
    }
}
/// Helper struct that wraps interaction to a local wallet and a transaction generator
#[derive(Clone)]
pub struct Wallet {
    inner: LocalWallet,
    pub tx_gen: TransactionTestContext,
    pub nonce: u64,
}

impl Wallet {
    /// Create a new wallet with a given chain id
    pub fn new(inner: LocalWallet, chain_id: u64) -> Self {
        let tx_gen = TransactionTestContext::new(chain_id, inner.clone());
        Self { inner, tx_gen, nonce: 0 }
    }
    /// Get the address of the wallet
    pub fn address(&self) -> String {
        self.inner.address().to_string()
    }

    /// Get an EIP1559 transaction
    pub async fn eip1559(&self) -> Bytes {
        self.tx_gen.eip1559().await
    }

    /// Get an EIP4844 transaction
    pub async fn eip4844(&self) -> Bytes {
        self.tx_gen.eip4844().await.unwrap()
    }
    /// Get an optimism block info transaction
    pub async fn optimism_block_info(&self, nonce: u64) -> Bytes {
        self.tx_gen.optimism_block_info(nonce).await
    }
}

/// As deafult we use the test mnemonic and mainnet specs.
impl Default for Wallet {
    fn default() -> Self {
        let builder = MnemonicBuilder::<English>::default().phrase(TEST_MNEMONIC);
        let inner = builder.build().unwrap();
        let chain_id: ChainId = MAINNET.chain.into();
        let tx_gen = TransactionTestContext::new(chain_id, inner.clone());
        Self { inner, tx_gen, nonce: 0 }
    }
}
