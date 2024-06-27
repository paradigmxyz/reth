//! Loads and formats OP transaction RPC response.   

use jsonrpsee_types::error::ErrorObject;
use reth_evm::ConfigureEvm;
use reth_evm_optimism::RethL1BlockInfo;
use reth_primitives::{
    BlockNumber, Receipt, TransactionMeta, TransactionSigned, TransactionSignedEcRecovered, B256,
};
use reth_provider::{
    BlockIdReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome,
    StateProviderFactory,
};
use reth_rpc_types::{AnyTransactionReceipt, OptimismTransactionReceiptFields, ToRpcError};
use reth_transaction_pool::TransactionPool;
use revm::L1BlockInfo;
use revm_primitives::{BlockEnv, ExecutionResult};

use reth_rpc_eth_api::helpers::{LoadPendingBlock, LoadReceipt, SpawnBlocking};
use reth_rpc_eth_types::{EthApiError, EthResult, EthStateCache, PendingBlock, ReceiptBuilder};
use reth_rpc_server_types::result::internal_rpc_err;

use crate::EthApi;

/// L1 fee and data gas for a transaction, along with the L1 block info.
#[derive(Debug, Default, Clone)]
pub struct OptimismTxMeta {
    /// The L1 block info.
    pub l1_block_info: Option<L1BlockInfo>,
    /// The L1 fee for the block.
    pub l1_fee: Option<u128>,
    /// The L1 data gas for the block.
    pub l1_data_gas: Option<u128>,
}

impl OptimismTxMeta {
    /// Creates a new [`OptimismTxMeta`].
    pub const fn new(
        l1_block_info: Option<L1BlockInfo>,
        l1_fee: Option<u128>,
        l1_data_gas: Option<u128>,
    ) -> Self {
        Self { l1_block_info, l1_fee, l1_data_gas }
    }
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider: BlockIdReader + ChainSpecProvider,
{
    /// Builds [`OptimismTxMeta`] object using the provided [`TransactionSigned`], L1 block
    /// info and block timestamp. The [`L1BlockInfo`] is used to calculate the l1 fee and l1 data
    /// gas for the transaction. If the [`L1BlockInfo`] is not provided, the meta info will be
    /// empty.
    pub fn build_op_tx_meta(
        &self,
        tx: &TransactionSigned,
        l1_block_info: Option<L1BlockInfo>,
        block_timestamp: u64,
    ) -> EthResult<OptimismTxMeta> {
        let Some(l1_block_info) = l1_block_info else { return Ok(OptimismTxMeta::default()) };

        let (l1_fee, l1_data_gas) = if !tx.is_deposit() {
            let envelope_buf = tx.envelope_encoded();

            let inner_l1_fee = l1_block_info
                .l1_tx_data_fee(
                    &self.inner.provider().chain_spec(),
                    block_timestamp,
                    &envelope_buf,
                    tx.is_deposit(),
                )
                .map_err(|_| OptimismEthApiError::L1BlockFeeError)?;
            let inner_l1_data_gas = l1_block_info
                .l1_data_gas(&self.inner.provider().chain_spec(), block_timestamp, &envelope_buf)
                .map_err(|_| OptimismEthApiError::L1BlockGasError)?;
            (
                Some(inner_l1_fee.saturating_to::<u128>()),
                Some(inner_l1_data_gas.saturating_to::<u128>()),
            )
        } else {
            (None, None)
        };

        Ok(OptimismTxMeta::new(Some(l1_block_info), l1_fee, l1_data_gas))
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadReceipt for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: Send + Sync,
    Provider: BlockIdReader + ChainSpecProvider,
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
    ) -> EthResult<AnyTransactionReceipt> {
        let (block, receipts) = self
            .cache()
            .get_block_and_receipts(meta.block_hash)
            .await?
            .ok_or(EthApiError::UnknownBlockNumber)?;

        let block = block.unseal();
        let l1_block_info = reth_evm_optimism::extract_l1_info(&block).ok();
        let optimism_tx_meta = self.build_op_tx_meta(&tx, l1_block_info, block.timestamp)?;

        let resp_builder = ReceiptBuilder::new(&tx, meta, &receipt, &receipts)?;
        let resp_builder = op_receipt_fields(resp_builder, &tx, &receipt, optimism_tx_meta);

        Ok(resp_builder.build())
    }
}

/// Applies OP specific fields to a receipt.
fn op_receipt_fields(
    resp_builder: ReceiptBuilder,
    tx: &TransactionSigned,
    receipt: &Receipt,
    optimism_tx_meta: OptimismTxMeta,
) -> ReceiptBuilder {
    let mut op_fields = OptimismTransactionReceiptFields::default();

    if tx.is_deposit() {
        op_fields.deposit_nonce = receipt.deposit_nonce.map(reth_primitives::U64::from);
        op_fields.deposit_receipt_version =
            receipt.deposit_receipt_version.map(reth_primitives::U64::from);
    } else if let Some(l1_block_info) = optimism_tx_meta.l1_block_info {
        op_fields.l1_fee = optimism_tx_meta.l1_fee;
        op_fields.l1_gas_used = optimism_tx_meta.l1_data_gas.map(|dg| {
            dg + l1_block_info.l1_fee_overhead.unwrap_or_default().saturating_to::<u128>()
        });
        op_fields.l1_fee_scalar = Some(f64::from(l1_block_info.l1_base_fee_scalar) / 1_000_000.0);
        op_fields.l1_gas_price = Some(l1_block_info.l1_base_fee.saturating_to());
    }

    resp_builder.add_other_fields(op_fields.into())
}

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking,
    Provider: BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory,
    Pool: TransactionPool,
    EvmConfig: ConfigureEvm,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory {
        self.inner.provider()
    }

    #[inline]
    fn pool(&self) -> impl reth_transaction_pool::TransactionPool {
        self.inner.pool()
    }

    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock>> {
        self.inner.pending_block()
    }

    #[inline]
    fn evm_config(&self) -> &impl reth_evm::ConfigureEvm {
        self.inner.evm_config()
    }

    fn assemble_receipt(
        &self,
        tx: &TransactionSignedEcRecovered,
        result: ExecutionResult,
        cumulative_gas_used: u64,
    ) -> Receipt {
        Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            deposit_nonce: None,
            deposit_receipt_version: None,
        }
    }

    fn receipts_root(
        &self,
        _block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome
            .optimism_receipts_root_slow(
                block_number,
                self.provider().chain_spec().as_ref(),
                _block_env.timestamp.to::<u64>(),
            )
            .expect("Block is present")
    }
}

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
    /// Thrown when calculating L1 gas fee.
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
}

impl ToRpcError for OptimismEthApiError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        match self {
            Self::L1BlockFeeError | Self::L1BlockGasError => internal_rpc_err(self.to_string()),
        }
    }
}

impl From<OptimismEthApiError> for EthApiError {
    fn from(err: OptimismEthApiError) -> Self {
        Self::other(err)
    }
}
