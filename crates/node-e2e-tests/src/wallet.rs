use alloy_consensus::{BlobTransactionSidecar, SidecarBuilder, SimpleCoder, TxType};
use alloy_network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_wallet::{coins_bip39::English, LocalWallet, MnemonicBuilder};
use reth_primitives::{Address, Bytes, U256};
/// One of the accounts of the genesis allocations.
pub struct Wallet {
    inner: LocalWallet,
}

impl Wallet {
    /// Creates a new account from one of the secret/pubkeys of the genesis allocations (test.json)
    pub(crate) fn new(phrase: &str) -> Self {
        let inner = MnemonicBuilder::<English>::default().phrase(phrase).build().unwrap();
        Self { inner }
    }

    /// Creates a static transfer and signs it
    pub async fn transfer_tx(&self) -> Bytes {
        let tx = self.tx();
        let signer = EthereumSigner::from(self.inner.clone());
        tx.build(&signer).await.unwrap().encoded_2718().into()
    }

    /// Creates a static tx
    fn tx(&self) -> TransactionRequest {
        TransactionRequest {
            transaction_type: Some(TxType::Eip4844 as u8),
            nonce: Some(0),
            value: Some(U256::from(100)),
            to: Some(Address::random()),
            gas_price: Some(20e9 as u128),
            gas: Some(21000),
            chain_id: Some(1),
            ..Default::default()
        }
    }

    /// Creates a tx with blob sidecar and sign it
    pub async fn tx_with_blobs(&self) -> eyre::Result<Bytes> {
        let mut tx = self.tx();

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");
        let sidecar: BlobTransactionSidecar = builder.build()?;
        tx.set_blob_sidecar(sidecar);
        tx.clone().transaction_type(TxType::Eip4844 as u8);

        let signer = EthereumSigner::from(self.inner.clone());
        let built_tx = tx.clone().build(&signer).await.unwrap();
        let built_tx_2 = tx.build_unsigned().unwrap();

        println!("is_legacy: {}", built_tx.is_legacy());
        println!("is_legacy_2: {:?}", built_tx_2);

        assert!(!built_tx.is_legacy()); // legacy -> true

        assert!(!built_tx.type_flag().is_none()); // type_flag -> None
        Ok(built_tx.encoded_2718().into())
    }
}

const TEST_MNEMONIC: &str = "test test test test test test test test test test test junk";

impl Default for Wallet {
    fn default() -> Self {
        Wallet::new(TEST_MNEMONIC)
    }
}
