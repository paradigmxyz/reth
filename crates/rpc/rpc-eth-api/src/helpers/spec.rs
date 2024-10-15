//! Loads chain metadata.

use alloy_primitives::{Address, U256, U64};
use alloy_rpc_types::{Stage, SyncInfo, SyncStatus};
use futures::Future;
use reth_chainspec::{ChainInfo, EthereumHardforks};
use reth_errors::{RethError, RethResult};
use reth_network_api::NetworkInfo;
use reth_provider::{BlockNumReader, ChainSpecProvider, StageCheckpointReader};

use super::EthSigner;

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthApiSpec: Send + Sync {
    /// Returns a handle for reading data from disk.
    fn provider(
        &self,
    ) -> impl ChainSpecProvider<ChainSpec: EthereumHardforks> + BlockNumReader + StageCheckpointReader;

    /// Returns a handle for reading network data summary.
    fn network(&self) -> impl NetworkInfo;

    /// Returns the block node is started on.
    fn starting_block(&self) -> U256;

    /// Returns a handle to the signers owned by provider.
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>>;

    /// Returns the current ethereum protocol version.
    fn protocol_version(&self) -> impl Future<Output = RethResult<U64>> + Send {
        async move {
            let status = self.network().network_status().await.map_err(RethError::other)?;
            Ok(U64::from(status.protocol_version))
        }
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        U64::from(self.network().chain_id())
    }

    /// Returns provider chain info
    fn chain_info(&self) -> RethResult<ChainInfo> {
        Ok(self.provider().chain_info()?)
    }

    /// Returns a list of addresses owned by provider.
    fn accounts(&self) -> Vec<Address> {
        self.signers().read().iter().flat_map(|s| s.accounts()).collect()
    }

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool {
        self.network().is_syncing()
    }

    /// Returns the [`SyncStatus`] of the network
    fn sync_status(&self) -> RethResult<SyncStatus> {
        let status = if self.is_syncing() {
            let current_block = U256::from(
                self.provider().chain_info().map(|info| info.best_number).unwrap_or_default(),
            );

            let stages = self
                .provider()
                .get_all_checkpoints()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, checkpoint)| Stage { name, block: checkpoint.block_number })
                .collect();

            SyncStatus::Info(Box::new(SyncInfo {
                starting_block: self.starting_block(),
                current_block,
                highest_block: current_block,
                warp_chunks_amount: None,
                warp_chunks_processed: None,
                stages: Some(stages),
            }))
        } else {
            SyncStatus::None
        };
        Ok(status)
    }
}
