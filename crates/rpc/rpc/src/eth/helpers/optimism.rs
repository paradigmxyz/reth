//! Loads and formats OP transaction RPC response.   

use jsonrpsee_types::error::ErrorObject;
use reth_evm::ConfigureEvm;
use reth_primitives::{BlockNumber, Receipt, TransactionSignedEcRecovered, B256};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome, StateProviderFactory,
};
use reth_rpc_types::ToRpcError;
use reth_transaction_pool::TransactionPool;
use revm_primitives::{BlockEnv, ExecutionResult};

use reth_rpc_eth_api::helpers::{LoadPendingBlock, SpawnBlocking};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_rpc_server_types::result::internal_rpc_err;

use crate::EthApi;

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
