//! Loads and formats OP receipt RPC response.   

use op_alloy_rpc_types::{receipt::L1BlockInfo, OptimismTransactionReceiptFields};
use reth_chainspec::{ChainSpec, OptimismHardforks};
use reth_evm_optimism::RethL1BlockInfo;
use reth_node_api::FullNodeComponents;
use reth_primitives::{Receipt, TransactionMeta, TransactionSigned};
use reth_provider::ChainSpecProvider;
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, LoadReceipt, LoadTransaction},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, EthStateCache, ReceiptBuilder};
use reth_rpc_types::AnyTransactionReceipt;

use crate::{OpEthApi, OpEthApiError};

impl<N> LoadReceipt for OpEthApi<N>
where
    Self: EthApiSpec + LoadTransaction<Error = OpEthApiError>,
    N: FullNodeComponents,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> Result<AnyTransactionReceipt, Self::Error> {
        let (block, receipts) = LoadReceipt::cache(self)
            .get_block_and_receipts(meta.block_hash)
            .await
            .map_err(Self::Error::from_eth_err)?
            .ok_or(Self::Error::from_eth_err(EthApiError::UnknownBlockNumber))?;

        let block = block.unseal();
        let l1_block_info =
            reth_evm_optimism::extract_l1_info(&block).map_err(OpEthApiError::from)?;

        let op_receipt_meta = self
            .build_op_receipt_meta(&tx, l1_block_info, &receipt)
            .map_err(OpEthApiError::from)?;

        let receipt_resp = ReceiptBuilder::new(&tx, meta, &receipt, &receipts)
            .map_err(Self::Error::from_eth_err)?
            .add_other_fields(op_receipt_meta.into())
            .build();

        Ok(receipt_resp)
    }
}

impl<N> OpEthApi<N>
where
    N: FullNodeComponents<Provider: ChainSpecProvider<ChainSpec = ChainSpec>>,
{
    /// Builds a receipt w.r.t. chain spec.
    pub fn build_op_receipt_meta(
        &self,
        tx: &TransactionSigned,
        l1_block_info: revm::L1BlockInfo,
        receipt: &Receipt,
    ) -> Result<OptimismTransactionReceiptFields, OpEthApiError> {
        Ok(OpReceiptFieldsBuilder::default()
            .l1_block_info(&self.inner.provider().chain_spec(), tx, l1_block_info)?
            .deposit_nonce(receipt.deposit_nonce)
            .deposit_version(receipt.deposit_receipt_version)
            .build())
    }
}

/// L1 fee and data gas for a non-deposit transaction, or deposit nonce and receipt version for a
/// deposit transaction.
#[derive(Debug, Default, Clone)]
pub struct OpReceiptFieldsBuilder {
    /// Block timestamp.
    pub l1_block_timestamp: u64,
    /// The L1 fee for transaction.
    pub l1_fee: Option<u128>,
    /// L1 gas used by transaction.
    pub l1_data_gas: Option<u128>,
    /// L1 fee scalar.
    pub l1_fee_scalar: Option<f64>,
    /* ---------------------------------------- Bedrock ---------------------------------------- */
    /// The base fee of the L1 origin block.
    pub l1_base_fee: Option<u128>,
    /* --------------------------------------- Regolith ---------------------------------------- */
    /// Deposit nonce, if this is a deposit transaction.
    pub deposit_nonce: Option<u64>,
    /* ---------------------------------------- Canyon ----------------------------------------- */
    /// Deposit receipt version, if this is a deposit transaction.
    pub deposit_receipt_version: Option<u64>,
    /* ---------------------------------------- Ecotone ---------------------------------------- */
    /// The current L1 fee scalar.
    pub l1_base_fee_scalar: Option<u128>,
    /// The current L1 blob base fee.
    pub l1_blob_base_fee: Option<u128>,
    /// The current L1 blob base fee scalar.
    pub l1_blob_base_fee_scalar: Option<u128>,
}

impl OpReceiptFieldsBuilder {
    /// Returns a new builder.
    pub fn new(block_timestamp: u64) -> Self {
        Self { l1_block_timestamp: block_timestamp, ..Default::default() }
    }

    /// Applies [`L1BlockInfo`](revm::L1BlockInfo).
    pub fn l1_block_info(
        mut self,
        chain_spec: &ChainSpec,
        tx: &TransactionSigned,
        l1_block_info: revm::L1BlockInfo,
    ) -> Result<Self, OpEthApiError> {
        let envelope_buf = tx.envelope_encoded();
        let timestamp = self.l1_block_timestamp;

        self.l1_fee = Some(
            l1_block_info
                .l1_tx_data_fee(chain_spec, timestamp, &tx.envelope_encoded(), tx.is_deposit())
                .map_err(|_| OpEthApiError::L1BlockFeeError)?
                .saturating_to(),
        );

        self.l1_data_gas = Some(
            l1_block_info
                .l1_data_gas(chain_spec, timestamp, &envelope_buf)
                .map_err(|_| OpEthApiError::L1BlockGasError)?
                .saturating_add(l1_block_info.l1_fee_overhead.unwrap_or_default())
                .saturating_to(),
        );

        self.l1_fee_scalar = (!chain_spec.hardforks.is_ecotone_active_at_timestamp(timestamp))
            .then_some(f64::from(l1_block_info.l1_base_fee_scalar) / 1_000_000.0);

        self.l1_base_fee = Some(l1_block_info.l1_base_fee.saturating_to());
        self.l1_base_fee_scalar = Some(l1_block_info.l1_base_fee_scalar.saturating_to());
        self.l1_blob_base_fee = l1_block_info.l1_blob_base_fee.map(|fee| fee.saturating_to());
        self.l1_blob_base_fee_scalar =
            l1_block_info.l1_blob_base_fee_scalar.map(|scalar| scalar.saturating_to());

        Ok(self)
    }

    /// Applies deposit transaction metadata: deposit nonce.
    pub const fn deposit_nonce(mut self, nonce: Option<u64>) -> Self {
        self.deposit_nonce = nonce;
        self
    }

    /// Applies deposit transaction metadata: deposit receipt version.
    pub const fn deposit_version(mut self, version: Option<u64>) -> Self {
        self.deposit_receipt_version = version;
        self
    }

    /// Builds the [`OptimismTransactionReceiptFields`] object.
    pub const fn build(self) -> OptimismTransactionReceiptFields {
        let Self {
            l1_block_timestamp: _, // used to compute other fields
            l1_fee,
            l1_data_gas: l1_gas_used,
            l1_fee_scalar,
            l1_base_fee,
            deposit_nonce,
            deposit_receipt_version,
            l1_base_fee_scalar,
            l1_blob_base_fee,
            l1_blob_base_fee_scalar,
        } = self;

        OptimismTransactionReceiptFields {
            l1_block_info: L1BlockInfo {
                l1_gas_price: l1_base_fee,
                l1_gas_used,
                l1_fee,
                l1_fee_scalar,
                l1_base_fee_scalar,
                l1_blob_base_fee,
                l1_blob_base_fee_scalar,
            },
            deposit_nonce,
            deposit_receipt_version,
        }
    }
}
