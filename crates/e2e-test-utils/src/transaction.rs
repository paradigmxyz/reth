use alloy_consensus::{
    BlobTransactionSidecar, EnvKzgSettings, SidecarBuilder, SimpleCoder, TxEip4844Variant,
    TxEnvelope,
};
use alloy_network::{eip2718::Encodable2718, EthereumWallet, TransactionBuilder};
use alloy_rpc_types::{TransactionInput, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use eyre::Ok;
use reth_primitives::{hex, Address, Bytes, U256};

use reth_primitives::B256;

/// Helper for transaction operations
#[derive(Debug)]
pub struct TransactionTestContext;

impl TransactionTestContext {
    /// Creates a static transfer and signs it, returning bytes
    pub async fn transfer_tx(chain_id: u64, wallet: PrivateKeySigner) -> TxEnvelope {
        let tx = tx(chain_id, None, 0);
        Self::sign_tx(wallet, tx).await
    }

    /// Creates a static transfer and signs it, returning bytes
    pub async fn transfer_tx_bytes(chain_id: u64, wallet: PrivateKeySigner) -> Bytes {
        let signed = Self::transfer_tx(chain_id, wallet).await;
        signed.encoded_2718().into()
    }

    /// Creates a tx with blob sidecar and sign it
    pub async fn tx_with_blobs(
        chain_id: u64,
        wallet: PrivateKeySigner,
    ) -> eyre::Result<TxEnvelope> {
        let mut tx = tx(chain_id, None, 0);

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");
        let sidecar: BlobTransactionSidecar = builder.build()?;

        tx.set_blob_sidecar(sidecar);
        tx.set_max_fee_per_blob_gas(15e9 as u128);

        let signed = Self::sign_tx(wallet, tx).await;
        Ok(signed)
    }

    /// Signs an arbitrary [`TransactionRequest`] using the provided wallet
    pub async fn sign_tx(wallet: PrivateKeySigner, tx: TransactionRequest) -> TxEnvelope {
        let signer = EthereumWallet::from(wallet);
        tx.build(&signer).await.unwrap()
    }

    /// Creates a tx with blob sidecar and sign it, returning bytes
    pub async fn tx_with_blobs_bytes(
        chain_id: u64,
        wallet: PrivateKeySigner,
    ) -> eyre::Result<Bytes> {
        let signed = Self::tx_with_blobs(chain_id, wallet).await?;

        Ok(signed.encoded_2718().into())
    }

    /// Creates and encodes an Optimism L1 block information transaction.
    pub async fn optimism_l1_block_info_tx(
        chain_id: u64,
        wallet: PrivateKeySigner,
        nonce: u64,
    ) -> Bytes {
        let l1_block_info = Bytes::from_static(&hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"));
        let tx = tx(chain_id, Some(l1_block_info), nonce);
        let signer = EthereumWallet::from(wallet);
        tx.build(&signer).await.unwrap().encoded_2718().into()
    }

    /// Validates the sidecar of a given tx envelope and returns the versioned hashes
    pub fn validate_sidecar(tx: TxEnvelope) -> Vec<B256> {
        let proof_setting = EnvKzgSettings::Default;

        match tx {
            TxEnvelope::Eip4844(signed) => match signed.tx() {
                TxEip4844Variant::TxEip4844WithSidecar(tx) => {
                    tx.validate_blob(proof_setting.get()).unwrap();
                    tx.sidecar.versioned_hashes().collect()
                }
                _ => panic!("Expected Eip4844 transaction with sidecar"),
            },
            _ => panic!("Expected Eip4844 transaction"),
        }
    }
}

/// Creates a type 2 transaction
fn tx(chain_id: u64, data: Option<Bytes>, nonce: u64) -> TransactionRequest {
    TransactionRequest {
        nonce: Some(nonce),
        value: Some(U256::from(100)),
        to: Some(reth_primitives::TxKind::Call(Address::random())),
        gas: Some(210000),
        max_fee_per_gas: Some(20e9 as u128),
        max_priority_fee_per_gas: Some(20e9 as u128),
        chain_id: Some(chain_id),
        input: TransactionInput { input: None, data },
        ..Default::default()
    }
}
