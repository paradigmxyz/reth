use alloy_consensus::{BlobTransactionSidecar, SidecarBuilder, SimpleCoder};
use alloy_network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder};
use alloy_rpc_types::{TransactionInput, TransactionRequest};
use alloy_signer_wallet::LocalWallet;
use reth_primitives::{Address, Bytes, U256};

pub struct TransactionTestContext;

impl TransactionTestContext {
    /// Creates a static transfer and signs it
    pub async fn transfer_tx(chain_id: u64, wallet: LocalWallet, data: Option<Bytes>) -> Bytes {
        let tx = tx(chain_id, data);
        let signer = EthereumSigner::from(wallet);
        tx.build(&signer).await.unwrap().encoded_2718().into()
    }

    /// Creates a tx with blob sidecar and sign it
    pub async fn tx_with_blobs(chain_id: u64, wallet: LocalWallet) -> eyre::Result<Bytes> {
        let mut tx = tx(chain_id, None);

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");
        let sidecar: BlobTransactionSidecar = builder.build()?;

        tx.set_blob_sidecar(sidecar);
        tx.set_max_fee_per_blob_gas(15e9 as u128);

        let signer = EthereumSigner::from(wallet);
        let signed = tx.clone().build(&signer).await.unwrap();

        Ok(signed.encoded_2718().into())
    }
}

/// Creates a type 2 transaction
fn tx(chain_id: u64, data: Option<Bytes>) -> TransactionRequest {
    TransactionRequest {
        nonce: Some(0),
        value: Some(U256::from(100)),
        to: Some(Address::random()),
        gas: Some(21000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(chain_id),
        input: TransactionInput { input: None, data },
        ..Default::default()
    }
}
