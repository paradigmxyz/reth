//! Loads chain metadata.

use alloy_consensus::BlockHeader as _;
use alloy_eips::{
    eip2124::Head,
    eip2935, eip4788, eip6110, eip7002, eip7251,
    eip7910::{EthBaseForkConfig, EthConfig, EthForkConfig, SystemContract},
};
use alloy_primitives::{Address, U256, U64};
use alloy_rpc_types_eth::{Stage, SyncInfo, SyncStatus};
use futures::Future;
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec, EthereumHardforks, Hardforks};
use reth_errors::{RethError, RethResult};
use reth_network_api::NetworkInfo;
use reth_rpc_convert::{RpcTxReq, RpcTypes};
use reth_storage_api::{
    BlockNumReader, BlockReaderIdExt, StageCheckpointReader, TransactionsProvider,
};
use std::collections::BTreeMap;

use crate::{helpers::EthSigner, RpcNodeCore};

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthApiSpec:
    RpcNodeCore<
    Provider: ChainSpecProvider<ChainSpec: Hardforks + EthereumHardforks>
                  + BlockNumReader
                  + StageCheckpointReader,
    Network: NetworkInfo,
>
{
    /// The transaction type signers are using.
    type Transaction;

    /// The RPC requests and responses.
    type Rpc: RpcTypes;

    /// Returns the block node is started on.
    fn starting_block(&self) -> U256;

    /// Returns a handle to the signers owned by provider.
    fn signers(&self) -> &SignersForApi<Self>;

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

    /// Returns provider fork configuration
    fn chain_config(&self) -> RethResult<EthConfig> {
        let chain_spec = self.provider().chain_spec();
        let head_timestamp = self.provider().latest_header()?.map_or(0, |h| h.timestamp());

        // Short-circuit if Cancun is not active.
        if !chain_spec.is_cancun_active_at_timestamp(head_timestamp) {
            return Err(RethError::msg("cancun has not been activated"))
        }

        let fork_timestamps = chain_spec.fork_timestamps();
        let (current_fork_idx, current_fork_timestamp) = fork_timestamps
            .iter()
            .position(|ts| &head_timestamp < ts)
            .and_then(|idx| idx.checked_sub(1))
            .or_else(|| fork_timestamps.len().checked_sub(1))
            .and_then(|idx| fork_timestamps.get(idx).map(|ts| (idx, *ts)))
            .ok_or_else(|| RethError::msg("no active timestamp fork found"))?;

        let current_base_config = chain_spec
            .fork_config_at_timestamp(current_fork_timestamp)
            .ok_or_else(|| RethError::msg("no base config for current fork"))?;
        let current = base_to_fork_config(&chain_spec, current_base_config);
        let current_hash = current.fork_hash().map_err(RethError::other)?;
        let current_fork_id = chain_spec.fork_id(&Head {
            number: u64::MAX,
            timestamp: current_fork_timestamp,
            ..Default::default()
        });

        let mut config = EthConfig {
            current,
            current_hash,
            current_fork_id: current_fork_id.hash,
            next: None,
            next_hash: None,
            next_fork_id: None,
            last: None,
            last_hash: None,
            last_fork_id: None,
        };

        if let Some(last_fork_idx) = current_fork_idx.checked_sub(1) {
            if let Some(last_fork_timestamp) = fork_timestamps.get(last_fork_idx).copied() {
                if let Some(last_base_config) =
                    chain_spec.fork_config_at_timestamp(last_fork_timestamp)
                {
                    let last = base_to_fork_config(&chain_spec, last_base_config);
                    let last_hash = last.fork_hash().map_err(RethError::other)?;
                    let last_fork_id = chain_spec.fork_id(&Head {
                        number: u64::MAX,
                        timestamp: last_fork_timestamp,
                        ..Default::default()
                    });
                    config.last = Some(last);
                    config.last_hash = Some(last_hash);
                    config.last_fork_id = Some(last_fork_id.hash);
                }
            }
        }

        if let Some(next_fork_timestamp) = fork_timestamps.get(current_fork_idx + 1).copied() {
            if let Some(next_base_config) = chain_spec.fork_config_at_timestamp(next_fork_timestamp)
            {
                let next = base_to_fork_config(&chain_spec, next_base_config);
                let next_hash = next.fork_hash().map_err(RethError::other)?;
                let next_fork_id = chain_spec.fork_id(&Head {
                    number: u64::MAX,
                    timestamp: next_fork_timestamp,
                    ..Default::default()
                });
                config.next = Some(next);
                config.next_hash = Some(next_hash);
                config.next_fork_id = Some(next_fork_id.hash);
            }
        }

        Ok(config)
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

/// A handle to [`EthSigner`]s with its generics set from [`EthApiSpec`].
pub type SignersForApi<Api> = parking_lot::RwLock<
    Vec<Box<dyn EthSigner<<Api as EthApiSpec>::Transaction, RpcTxReq<<Api as EthApiSpec>::Rpc>>>>,
>;

/// A handle to [`EthSigner`]s with its generics set from [`TransactionsProvider`] and
/// [`reth_rpc_convert::RpcTypes`].
pub type SignersForRpc<Provider, Rpc> = parking_lot::RwLock<
    Vec<Box<dyn EthSigner<<Provider as TransactionsProvider>::Transaction, RpcTxReq<Rpc>>>>,
>;

// TODO: move
fn base_to_fork_config<ChainSpec: EthChainSpec + EthereumHardforks>(
    chain_spec: &ChainSpec,
    config: EthBaseForkConfig,
) -> EthForkConfig {
    let mut precompiles = Default::default(); // TODO:
    let mut system_contracts = BTreeMap::<SystemContract, Address>::default();

    if chain_spec.is_cancun_active_at_timestamp(config.activation_time) {
        system_contracts.insert(SystemContract::BeaconRoots, eip4788::BEACON_ROOTS_ADDRESS);
    }

    if chain_spec.is_prague_active_at_timestamp(config.activation_time) {
        system_contracts.extend([
            (SystemContract::HistoryStorage, eip2935::HISTORY_STORAGE_ADDRESS),
            (
                SystemContract::ConsolidationRequestPredeploy,
                eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
            ),
            (
                SystemContract::WithdrawalRequestPredeploy,
                eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            ),
            (
                SystemContract::DepositContract,
                chain_spec
                    .deposit_contract()
                    .map(|c| c.address)
                    .unwrap_or(eip6110::MAINNET_DEPOSIT_CONTRACT_ADDRESS),
            ),
        ]);
    }

    EthForkConfig {
        activation_time: config.activation_time,
        blob_schedule: config.blob_schedule,
        chain_id: config.chain_id,
        precompiles,
        system_contracts,
    }
}
