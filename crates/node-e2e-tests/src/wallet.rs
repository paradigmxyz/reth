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
            nonce: Some(0),
            value: Some(U256::from(100)),
            to: Some(Address::random()),
            gas: Some(21000),
            chain_id: Some(1),
            max_priority_fee_per_gas: Some(1500000000),
            max_fee_per_gas: Some(1500000000),
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
        tx.set_max_fee_per_blob_gas(1);
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
