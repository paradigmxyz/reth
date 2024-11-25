//! Loads a pending block from database. Helper trait for `eth_` block, transaction, call and trace
//! RPC methods.

use super::SpawnBlocking;
use crate::{EthApiTypes, FromEthApiError, RpcNodeCore};
use alloy_consensus::Header;
use alloy_primitives::{BlockNumber, B256};
use alloy_rpc_types_eth::BlockNumberOrTag;
use futures::Future;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    proofs::calculate_transaction_root, Block, BlockBody, BlockExt, Receipt,
    SealedBlockWithSenders, SealedHeader, TransactionSignedEcRecovered,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderError,
    ReceiptProvider, StateProviderFactory,
};
use reth_revm::primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, ExecutionResult, SpecId};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin};
use reth_transaction_pool::TransactionPool;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;

/// Loads a pending block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadPendingBlock:
    EthApiTypes
    + RpcNodeCore<
        Provider: BlockReaderIdExt
                      + EvmEnvProvider
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool,
        Evm: ConfigureEvm<Header = Header>,
    >
{
    /// Returns a handle to the pending block.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pending_block(&self) -> &Mutex<Option<PendingBlock>>;

    /// Configures the [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    fn pending_block_env_and_cfg(&self) -> Result<PendingBlockEnv, Self::Error> {
        let origin: PendingBlockEnvOrigin = if let Some(pending) =
            self.provider().pending_block_with_senders().map_err(Self::Error::from_eth_err)?
        {
            PendingBlockEnvOrigin::ActualPending(pending)
        } else {
            // no pending block from the CL yet, so we use the latest block and modify the env
            // values that we can
            let latest = self
                .provider()
                .latest_header()
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;

            let (mut latest_header, block_hash) = latest.split();
            // child block
            latest_header.number += 1;
            // assumed child block is in the next slot: 12s
            latest_header.timestamp += 12;
            // base fee of the child block
            let chain_spec = self.provider().chain_spec();

            latest_header.base_fee_per_gas = latest_header.next_block_base_fee(
                chain_spec.base_fee_params_at_timestamp(latest_header.timestamp),
            );

            // update excess blob gas consumed above target
            latest_header.excess_blob_gas = latest_header.next_block_excess_blob_gas();

            // we're reusing the same block hash because we need this to lookup the block's state
            let latest = SealedHeader::new(latest_header, block_hash);

            PendingBlockEnvOrigin::DerivedFromLatest(latest)
        };

        let mut cfg = CfgEnvWithHandlerCfg::new_with_spec_id(CfgEnv::default(), SpecId::LATEST);

        let mut block_env = BlockEnv::default();
        // Note: for the PENDING block we assume it is past the known merge block and thus this will
        // not fail when looking up the total difficulty value for the blockenv.
        self.provider()
            .fill_env_with_header(
                &mut cfg,
                &mut block_env,
                origin.header(),
                self.evm_config().clone(),
            )
            .map_err(Self::Error::from_eth_err)?;

        Ok(PendingBlockEnv::new(cfg, block_env, origin))
    }

    /// Returns the locally built pending block
    fn local_pending_block(
        &self,
    ) -> impl Future<Output = Result<Option<(SealedBlockWithSenders, Vec<Receipt>)>, Self::Error>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            let pending = self.pending_block_env_and_cfg()?;
            if pending.origin.is_actual_pending() {
                if let Some(block) = pending.origin.clone().into_actual_pending() {
                    // we have the real pending block, so we should also have its receipts
                    if let Some(receipts) = self
                        .provider()
                        .receipts_by_block(block.hash().into())
                        .map_err(Self::Error::from_eth_err)?
                    {
                        return Ok(Some((block, receipts)))
                    }
                }
            }

            // we couldn't find the real pending block, so we need to build it ourselves
            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.block_env.number.to::<u64>() == pending_block.block.number &&
                    pending.origin.header().hash() == pending_block.block.parent_hash &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some((pending_block.block.clone(), pending_block.receipts.clone())));
                }
            }

            // no pending block from the CL yet, so we need to build it ourselves via txpool
            let (sealed_block, receipts) = match self
                .spawn_blocking_io(move |this| {
                    // we rebuild the block
                    this.build_block(pending)
                })
                .await
            {
                Ok(block) => block,
                Err(err) => {
                    debug!(target: "rpc", "Failed to build pending block: {:?}", err);
                    return Ok(None)
                }
            };

            let now = Instant::now();
            *lock = Some(PendingBlock::new(
                now + Duration::from_secs(1),
                sealed_block.clone(),
                receipts.clone(),
            ));

            Ok(Some((sealed_block, receipts)))
        }
    }

    /// Assembles a [`Receipt`] for a transaction, based on its [`ExecutionResult`].
    fn assemble_receipt(
        &self,
        tx: &TransactionSignedEcRecovered,
        result: ExecutionResult,
        cumulative_gas_used: u64,
    ) -> Receipt {
        #[allow(clippy::needless_update)]
        Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }

    /// Calculates receipts root in block building.
    ///
    /// Panics if block is not in the [`ExecutionOutcome`]'s block range.
    fn receipts_root(
        &self,
        _block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome.receipts_root_slow(block_number).expect("Block is present")
    }

    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    fn build_block(
        &self,
        _env: PendingBlockEnv,
    ) -> Result<(SealedBlockWithSenders, Vec<Receipt>), Self::Error>
    where
        EthApiError: From<ProviderError>;
}
