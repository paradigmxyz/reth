//! OP-Reth `eth_` endpoint implementation.

pub mod receipt;
pub mod transaction;

mod block;
mod call;
mod pending_block;

pub use receipt::{OpReceiptBuilder, OpReceiptFieldsBuilder};
use reth_optimism_primitives::OpPrimitives;

use std::{fmt, sync::Arc};

use alloy_consensus::Header;
use alloy_eips::BlockId;
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::EIP1186AccountProofResponse;
use alloy_serde::JsonStorageKey;
use derive_more::Deref;
use op_alloy_network::Optimism;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_errors::RethError;
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_node_builder::EthApiBuilderCtx;
use reth_provider::{
    BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt, CanonStateSubscriptions,
    ChainSpecProvider, EvmEnvProvider, StageCheckpointReader, StateProviderFactory,
};
use reth_rpc::eth::{core::EthApiInner, DevSigner};
use reth_rpc_eth_api::{
    helpers::{
        AddDevSigners, EthApiSpec, EthFees, EthSigner, EthState, LoadBlock, LoadFee, LoadState,
        SpawnBlocking, Trace,
    },
    EthApiTypes, FromEthApiError, RpcNodeCore, RpcNodeCoreExt,
};
use reth_rpc_eth_types::{EthApiError, EthStateCache, FeeHistoryCache, GasPriceOracle};
use reth_rpc_types_compat::proof::from_primitive_account_proof;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner,
};
use reth_transaction_pool::TransactionPool;
use reth_trie_common::AccountProof;
use std::future::Future;

use crate::{OpEthApiError, SequencerClient};

/// Adapter for [`EthApiInner`], which holds all the data required to serve core `eth_` API.
pub type EthApiNodeBackend<N> = EthApiInner<
    <N as RpcNodeCore>::Provider,
    <N as RpcNodeCore>::Pool,
    <N as RpcNodeCore>::Network,
    <N as RpcNodeCore>::Evm,
>;

/// OP-Reth `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
///
/// This wraps a default `Eth` implementation, and provides additional functionality where the
/// optimism spec deviates from the default (ethereum) spec, e.g. transaction forwarding to the
/// sequencer, receipts, additional RPC fields for transaction receipts.
///
/// This type implements the [`FullEthApi`](reth_rpc_eth_api::helpers::FullEthApi) by implemented
/// all the `Eth` helper traits and prerequisite traits.
#[derive(Deref, Clone)]
pub struct OpEthApi<N: RpcNodeCore> {
    /// Gateway to node's core components.
    #[deref]
    inner: Arc<EthApiNodeBackend<N>>,
    /// Sequencer client, configured to forward submitted transactions to sequencer of given OP
    /// network.
    sequencer_client: Option<SequencerClient>,
    /// List of addresses that _ONLY_ return storage proofs _WITHOUT_ an account proof when called
    /// with `eth_getProof`.
    storage_proof_only: Vec<Address>,
}

impl<N> OpEthApi<N>
where
    N: RpcNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider
                      + CanonStateSubscriptions<Primitives = OpPrimitives>
                      + Clone
                      + 'static,
    >,
{
    /// Creates a new instance for given context.
    pub fn new(ctx: &EthApiBuilderCtx<N>, sequencer_http: Option<String>, storage_proof_only: Vec<Address>) -> Self {
        let blocking_task_pool =
            BlockingTaskPool::build().expect("failed to build blocking task pool");

        let inner = EthApiInner::new(
            ctx.provider.clone(),
            ctx.pool.clone(),
            ctx.network.clone(),
            ctx.cache.clone(),
            ctx.new_gas_price_oracle(),
            ctx.config.rpc_gas_cap,
            ctx.config.rpc_max_simulate_blocks,
            ctx.config.eth_proof_window,
            blocking_task_pool,
            ctx.new_fee_history_cache(),
            ctx.evm_config.clone(),
            ctx.executor.clone(),
            ctx.config.proof_permits,
        );

        Self {
            inner: Arc::new(inner),
            sequencer_client: sequencer_http.map(SequencerClient::new),
            storage_proof_only,
        }
    }
}

impl<N> EthApiTypes for OpEthApi<N>
where
    Self: Send + Sync,
    N: RpcNodeCore,
{
    type Error = OpEthApiError;
    type NetworkTypes = Optimism;
    type TransactionCompat = Self;

    fn tx_resp_builder(&self) -> &Self::TransactionCompat {
        self
    }
}

impl<N> RpcNodeCore for OpEthApi<N>
where
    N: RpcNodeCore,
{
    type Provider = N::Provider;
    type Pool = N::Pool;
    type Evm = <N as RpcNodeCore>::Evm;
    type Network = <N as RpcNodeCore>::Network;
    type PayloadBuilder = ();

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        self.inner.evm_config()
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        self.inner.network()
    }

    #[inline]
    fn payload_builder(&self) -> &Self::PayloadBuilder {
        &()
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        self.inner.provider()
    }
}

impl<N> RpcNodeCoreExt for OpEthApi<N>
where
    N: RpcNodeCore,
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}

impl<N> EthApiSpec for OpEthApi<N>
where
    N: RpcNodeCore<
        Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>
                      + BlockNumReader
                      + StageCheckpointReader,
        Network: NetworkInfo,
    >,
{
    #[inline]
    fn starting_block(&self) -> U256 {
        self.inner.starting_block()
    }

    #[inline]
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>> {
        self.inner.signers()
    }
}

impl<N> SpawnBlocking for OpEthApi<N>
where
    Self: Send + Sync + Clone + 'static,
    N: RpcNodeCore,
{
    #[inline]
    fn io_task_spawner(&self) -> impl TaskSpawner {
        self.inner.task_spawner()
    }

    #[inline]
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.blocking_task_pool()
    }

    #[inline]
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.blocking_task_guard()
    }
}

impl<N> LoadFee for OpEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: RpcNodeCore<
        Provider: BlockReaderIdExt
                      + EvmEnvProvider
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.fee_history_cache()
    }
}

impl<N> LoadState for OpEthApi<N> where
    N: RpcNodeCore<
        Provider: StateProviderFactory + ChainSpecProvider<ChainSpec: EthereumHardforks>,
        Pool: TransactionPool,
    >
{
}

impl<N> EthState for OpEthApi<N>
where
    Self: LoadState + SpawnBlocking,
    N: RpcNodeCore,
{
    #[inline]
    fn max_proof_window(&self) -> u64 {
        self.inner.eth_proof_window()
    }

    fn get_proof(
        &self,
        address: Address,
        keys: Vec<JsonStorageKey>,
        block_id: Option<BlockId>,
    ) -> Result<
        impl Future<Output = Result<EIP1186AccountProofResponse, Self::Error>> + Send,
        Self::Error,
    >
    where
        Self: EthApiSpec,
    {
        Ok(async move {
            let _permit = self
                .acquire_owned()
                .await
                .map_err(RethError::other)
                .map_err(EthApiError::Internal)?;

            let chain_info = self.chain_info().map_err(Self::Error::from_eth_err)?;
            let block_id = block_id.unwrap_or_default();

            // Check whether the distance to the block exceeds the maximum configured window.
            let block_number = self
                .provider()
                .block_number_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(block_id))?;

            if self.storage_proof_only.contains(&address) {
                self.spawn_blocking_io(move |this| {
                    let b256_keys: Vec<B256> = keys.iter().map(|k| k.as_b256()).collect();
                    let state = this.state_at_block_id(block_number.into())?;

                    let proofs = state
                        .storage_multiproof(address, &b256_keys, Default::default())
                        .map_err(EthApiError::from_eth_err)?;

                    let account_proof = AccountProof {
                        address,
                        storage_root: proofs.root,
                        storage_proofs: b256_keys
                            .into_iter()
                            .map(|k| proofs.storage_proof(k))
                            .collect::<Result<_, _>>()
                            .map_err(RethError::other)
                            .map_err(Self::Error::from_eth_err)?,
                        ..Default::default()
                    };
                    Ok(from_primitive_account_proof(account_proof, keys))
                })
                .await
            } else {
                let max_window = self.max_proof_window();
                if chain_info.best_number.saturating_sub(block_number) > max_window {
                    return Err(EthApiError::ExceedsMaxProofWindow.into())
                }

                self.spawn_blocking_io(move |this| {
                    let state = this.state_at_block_id(block_id)?;
                    let storage_keys = keys.iter().map(|key| key.as_b256()).collect::<Vec<_>>();
                    let proof = state
                        .proof(Default::default(), address, &storage_keys)
                        .map_err(Self::Error::from_eth_err)?;
                    Ok(from_primitive_account_proof(proof, keys))
                })
                .await
            }
        })
    }
}

impl<N> EthFees for OpEthApi<N>
where
    Self: LoadFee,
    N: RpcNodeCore,
{
}

impl<N> Trace for OpEthApi<N>
where
    Self: RpcNodeCore<Provider: BlockReader> + LoadState<Evm: ConfigureEvm<Header = Header>>,
    N: RpcNodeCore,
{
}

impl<N> AddDevSigners for OpEthApi<N>
where
    N: RpcNodeCore,
{
    fn with_dev_accounts(&self) {
        *self.inner.signers().write() = DevSigner::random_signers(20)
    }
}

impl<N: RpcNodeCore> fmt::Debug for OpEthApi<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpEthApi").finish_non_exhaustive()
    }
}
