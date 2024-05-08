use alloy_signer::Signer;
use alloy_signer_wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder};

/// One of the accounts of the genesis allocations.
pub struct Wallet {
    pub inner: LocalWallet,
    pub inner_nonce: u64,
    pub chain_id: u64,
    amount: usize,
    derivation_path: Option<String>,
}

impl Wallet {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    pub fn new(amount: usize) -> Self {
        let inner = MnemonicBuilder::<English>::default().phrase(TEST_MNEMONIC).build().unwrap();
        Self { inner, chain_id: 1, amount, derivation_path: None, inner_nonce: 0 }
    }

    /// Sets chain id
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    fn get_derivation_path(&self) -> &str {
        self.derivation_path.as_deref().unwrap_or("m/44'/60'/0'/0/")
    }

    pub fn gen(&self) -> Vec<LocalWallet> {
        let builder = MnemonicBuilder::<English>::default().phrase(TEST_MNEMONIC);

        // use the derivation path
        let derivation_path = self.get_derivation_path();

        let mut wallets = Vec::with_capacity(self.amount);
        for idx in 0..self.amount {
            let builder =
                builder.clone().derivation_path(&format!("{derivation_path}{idx}")).unwrap();
            let wallet = builder.build().unwrap().with_chain_id(Some(self.chain_id));
            wallets.push(wallet)
        }
        wallets
    }
}

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

impl Default for Wallet {
    fn default() -> Self {
        Wallet::new(1)
    }
}
