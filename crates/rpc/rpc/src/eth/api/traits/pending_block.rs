//! Loads a pending block from database. Helper trait for `eth_` block and transaction RPC methods.

use std::time::{Duration, Instant};

use futures::Future;
use reth_evm::ConfigureEvm;
use reth_primitives::{SealedBlockWithSenders, SealedHeader};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderFactory,
};
use reth_transaction_pool::TransactionPool;
use revm_primitives::{BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, SpecId};
use tokio::sync::Mutex;
use tracing::debug;

use crate::eth::{
    api::{
        pending_block::{PendingBlock, PendingBlockEnv, PendingBlockEnvOrigin},
        SpawnBlocking,
    },
    error::{EthApiError, EthResult},
};

/// Loads a pending block from database.
pub trait LoadPendingBlock: SpawnBlocking {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(
        &self,
    ) -> &(impl BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory);

    /// Returns a handle for reading data from transaction pool.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pool(&self) -> &impl TransactionPool;

    /// Returns a handle to the pending block.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pending_block(&self) -> &Mutex<Option<PendingBlock>>;

    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm;

    /// Configures the [`CfgEnvWithHandlerCfg`] and [`BlockEnv`] for the pending block
    ///
    /// If no pending block is available, this will derive it from the `latest` block
    fn pending_block_env_and_cfg(&self) -> EthResult<PendingBlockEnv> {
        let origin: PendingBlockEnvOrigin = if let Some(pending) =
            self.provider().pending_block_with_senders()?
        {
            PendingBlockEnvOrigin::ActualPending(pending)
        } else {
            // no pending block from the CL yet, so we use the latest block and modify the env
            // values that we can
            let latest =
                self.provider().latest_header()?.ok_or_else(|| EthApiError::UnknownBlockNumber)?;

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
        self.provider().fill_env_with_header(
            &mut cfg,
            &mut block_env,
            origin.header(),
            self.evm_config().clone(),
        )?;

        Ok(PendingBlockEnv::new(cfg, block_env, origin))
    }

    /// Returns the locally built pending block
    fn local_pending_block(
        &self,
    ) -> impl Future<Output = EthResult<Option<SealedBlockWithSenders>>> {
        async move {
            let pending = self.pending_block_env_and_cfg()?;
            if pending.origin.is_actual_pending() {
                return Ok(pending.origin.into_actual_pending())
            }

            let mut lock = self.pending_block().lock().await;

            let now = Instant::now();

            // check if the block is still good
            if let Some(pending_block) = lock.as_ref() {
                // this is guaranteed to be the `latest` header
                if pending.block_env.number.to::<u64>() == pending_block.block.number &&
                    pending.origin.header().hash() == pending_block.block.parent_hash &&
                    now <= pending_block.expires_at
                {
                    return Ok(Some(pending_block.block.clone()))
                }
            }

            // no pending block from the CL yet, so we need to build it ourselves via txpool
            let pending_block = match self
                .spawn_blocking_io(move |this| {
                    // we rebuild the block
                    pending.build_block(this.provider(), this.pool())
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
            *lock = Some(PendingBlock::new(pending_block.clone(), now + Duration::from_secs(1)));

            Ok(Some(pending_block))
        }
    }
}
