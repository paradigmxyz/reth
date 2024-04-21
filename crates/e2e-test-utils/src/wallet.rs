use alloy_consensus::{BlobTransactionSidecar, SidecarBuilder, SimpleCoder, TxType};
use alloy_network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder};
use alloy_rpc_types::{TransactionInput, TransactionRequest};
use alloy_signer_wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder};
use reth_primitives::{Address, Bytes, U256};
/// One of the accounts of the genesis allocations.
pub struct Wallet {
    inner: LocalWallet,
    nonce: u64,
    chain_id: u64,
}

impl Wallet {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    pub(crate) fn new(phrase: &str) -> Self {
        let inner = MnemonicBuilder::<English>::default().phrase(phrase).build().unwrap();
        Self { inner, chain_id: 1, nonce: 0 }
    }

    /// Sets chain id
    pub fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Creates a static transfer and signs it
    pub async fn transfer_tx(&mut self, data: Option<Bytes>) -> Bytes {
        let tx = self.tx(data);
        let signer = EthereumSigner::from(self.inner.clone());
        tx.build(&signer).await.unwrap().encoded_2718().into()
    }

    /// Creates a transaction with data and signs it
    fn tx(&mut self, data: Option<Bytes>) -> TransactionRequest {
        let tx = TransactionRequest {
            nonce: Some(self.nonce),
            value: Some(U256::from(100)),
            to: Some(Address::random()),
            gas_price: Some(20e9 as u128),
            gas: Some(210000),
            chain_id: Some(self.chain_id),
            input: TransactionInput { input: None, data },
            ..Default::default()
        };
        self.nonce += 1;
        tx
    }

    /// Creates a tx with blob sidecar and sign it
    pub async fn tx_with_blobs(&mut self) -> eyre::Result<Bytes> {
        let mut tx = self.tx(None);
        self.nonce += 1;
        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");
        let sidecar: BlobTransactionSidecar = builder.build()?;
        tx.set_blob_sidecar(sidecar);
        tx.set_max_fee_per_blob_gas(1500000000);
        tx.clone().transaction_type(TxType::Eip4844 as u8);

        let signer = EthereumSigner::from(self.inner.clone());
        let signed = tx.clone().build(&signer).await.unwrap();

        Ok(signed.encoded_2718().into())
    }
}

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

impl Default for Wallet {
    fn default() -> Self {
        Wallet::new(TEST_MNEMONIC)
    }
}
