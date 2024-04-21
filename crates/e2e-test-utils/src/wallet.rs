use alloy_network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder};
use reth_primitives::{Address, Bytes, U256};
/// One of the accounts of the genesis allocations.
pub struct Wallet {
    inner: LocalWallet,
    nonce: u64,
}

impl Wallet {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    pub(crate) fn new(phrase: &str) -> Self {
        let inner = MnemonicBuilder::<English>::default().phrase(phrase).build().unwrap();
        Self { inner, nonce: 0 }
    }

    /// Creates a static transfer and signs it
    pub async fn transfer_tx(&mut self) -> Bytes {
        let tx = TransactionRequest {
            nonce: Some(self.nonce),
            value: Some(U256::from(100)),
            to: Some(Address::random()),
            gas_price: Some(20e9 as u128),
            gas: Some(21000),
            chain_id: Some(1),
            ..Default::default()
        };
        self.nonce += 1;
        let signer = EthereumSigner::from(self.inner.clone());
        tx.build(&signer).await.unwrap().encoded_2718().into()
    }
}

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

impl Default for Wallet {
    fn default() -> Self {
        Wallet::new(TEST_MNEMONIC)
    }
}
