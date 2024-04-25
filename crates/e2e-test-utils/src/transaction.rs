use alloy_consensus::{
    BlobTransactionSidecar, SidecarBuilder, SimpleCoder, TxEip1559, TxEip4844, TxEip4844Variant,
    TxEip4844WithSidecar, TxEnvelope,
};
use alloy_network::{eip2718::Encodable2718, EthereumSigner, TransactionBuilder};
use alloy_rpc_types::TransactionRequest;
use alloy_signer_wallet::LocalWallet;
use eyre::Ok;
use reth_primitives::{
    alloy_primitives::TxKind, constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, hex, Address, Bytes,
    B256, U256,
};

#[derive(Clone)]
pub struct TransactionTestContext {
    chain_id: u64,
    wallet: LocalWallet,
}

impl TransactionTestContext {
    pub fn new(chain_id: u64, wallet: LocalWallet) -> Self {
        Self { chain_id, wallet }
    }

    pub async fn eip1559(&self) -> Bytes {
        let tx = TxEip1559 {
            chain_id: self.chain_id,
            nonce: 0,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            max_priority_fee_per_gas: 20e9 as u128,
            max_fee_per_gas: 20e9 as u128,
            value: U256::from(1),
            ..Default::default()
        };
        let mut tx_request: TransactionRequest = tx.into();
        tx_request.access_list = None;
        self.sign_and_encode(tx_request).await.unwrap()
    }

    pub async fn eip4844(&self) -> eyre::Result<Bytes> {
        let tx = self.dummy_eip4844(0);
        let tx_request: TransactionRequest = tx.into();
        self.sign_and_encode(tx_request).await
    }

    pub async fn optimism_block_info(&self, nonce: u64) -> Bytes {
        let data = Bytes::from_static(&hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240"));

        let tx = TxEip1559 {
            chain_id: self.chain_id,
            nonce,
            gas_limit: 210_000,
            to: TxKind::Call(Address::ZERO),
            max_priority_fee_per_gas: 20e9 as u128,
            max_fee_per_gas: 20e9 as u128,
            value: U256::from(1),
            input: data,
            ..Default::default()
        };
        let mut tx_req: TransactionRequest = tx.into();
        tx_req.access_list = None;
        self.sign_and_encode(tx_req).await.unwrap()
    }

    async fn sign_and_encode(&self, tx: TransactionRequest) -> eyre::Result<Bytes> {
        let signer = EthereumSigner::from(self.wallet.clone());
        let signed = tx.build(&signer).await?;
        Ok(signed.encoded_2718().into())
    }

    fn dummy_eip4844(&self, nonce: u64) -> TxEip4844WithSidecar {
        let tx = TxEip4844 {
            chain_id: self.chain_id,
            nonce,
            max_priority_fee_per_gas: 20e9 as u128,
            max_fee_per_gas: 20e9 as u128,
            gas_limit: 21_000,
            to: Default::default(),
            value: U256::from(1),
            access_list: Default::default(),
            blob_versioned_hashes: vec![Default::default()],
            max_fee_per_blob_gas: 1,
            input: Default::default(),
        };

        let mut builder = SidecarBuilder::<SimpleCoder>::new();
        builder.ingest(b"dummy blob");
        let sidecar: BlobTransactionSidecar = builder.build().unwrap();

        TxEip4844WithSidecar { tx, sidecar }
    }

    pub fn validate_sidecar(tx: TxEnvelope) -> Vec<B256> {
        let proof_setting = MAINNET_KZG_TRUSTED_SETUP.clone();

        match tx {
            TxEnvelope::Eip4844(signed) => match signed.tx() {
                TxEip4844Variant::TxEip4844WithSidecar(tx) => {
                    tx.validate_blob(&proof_setting).unwrap();
                    tx.sidecar.versioned_hashes().collect()
                }
                _ => panic!("Expected Eip4844 transaction with sidecar"),
            },
            _ => panic!("Expected Eip4844 transaction"),
        }
    }
}
