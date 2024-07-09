//! Loads OP pending block for a RPC response.   

use reth_primitives::{
    revm_primitives::{BlockEnv, ExecutionResult},
    BlockNumber, Receipt, TransactionSignedEcRecovered, B256,
};
use reth_provider::{ChainSpecProvider, ExecutionOutcome};
use reth_rpc_eth_api::helpers::LoadPendingBlock;
use reth_rpc_eth_types::PendingBlock;

use crate::OpEthApi;

impl<Eth> LoadPendingBlock for OpEthApi<Eth>
where
    Eth: LoadPendingBlock,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl reth_provider::BlockReaderIdExt
           + reth_provider::EvmEnvProvider
           + reth_provider::ChainSpecProvider
           + reth_provider::StateProviderFactory {
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
