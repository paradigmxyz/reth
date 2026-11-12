//! Opt-in PBT node integration, durable checkpoints, and staged verification.
//!
//! BLAKE3 is the reference/devnet hash. Configure a shared `binaryTrieTime` in genesis
//! and install the returned add-ons, backfill, and authenticated RPC methods together.
//! This node integration is available on native targets; portable tree APIs live in `pbt`.

#![cfg(not(target_family = "wasm"))]

use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{Bytes, B256, U256};
use eyre::WrapErr;
use reth_engine_tree::tree::{
    state_root_strategy::{pbt::PbtStateRootStrategy, StateRootStrategy},
    BasicEngineValidator, TreeConfig,
};
use reth_ethereum::{
    chainspec::{ChainSpec, ForkCondition, Hardfork},
    network::api::BlockDownloaderProvider,
    node::{
        builder::{
            rpc::{
                BasicEngineApiBuilder, BasicEngineValidatorBuilder, EngineValidatorBuilder,
                EthApiBuilder, Identity, RpcAddOns,
            },
            FullNodeComponents, NodeTypes,
        },
        EthereumAddOns, EthereumEngineValidatorBuilder, EthereumEthApiBuilder, EthereumNode,
    },
    primitives::{AlloyBlockHeader, SealedHeader},
    provider::{
        BalProvider, BlockIdReader, BlockNumReader, HeaderProvider, RawBal, StageCheckpointReader,
    },
    EthPrimitives,
};
use reth_storage_overlay::OverlayManager;
use reth_trie_pbt::{
    migration::{mpt_root, DecodeLimits},
    persistence::SnapshotStore,
    AccountData, StateTree,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{File, OpenOptions, TryLockError},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tracing::Span;

mod backfill;
pub use backfill::PbtBackfill;

mod conversion;
mod download;
mod retirement;
mod rpc;

/// Shared PBT configuration for a full Ethereum node.
///
/// Keep this value alive until node shutdown: it owns the checkpoint-directory lock.
/// Install its add-ons and backfill together, disable `eth_getProof`, and expose
/// [`Self::rpc_module`] only on authenticated RPC.
#[derive(Debug)]
pub struct PbtNode {
    chain: Arc<ChainSpec>,
    store: Arc<SnapshotStore>,
    strategy: Arc<PbtStateRootStrategy>,
    startup: Startup,
    _directory_lock: File,
}

impl PbtNode {
    /// Opens durable PBT checkpoints and pins genesis and the activation timestamp.
    ///
    /// Migration artifacts, pruning, and catch-up budgets use the documented `PBT_*`
    /// environment variables. Missing or inconsistent persisted state fails startup.
    pub fn new(chain: Arc<ChainSpec>, directory: &Path, activation: u64) -> eyre::Result<Self> {
        let directory_lock = lock_directory(directory)?;
        let mut limits = DecodeLimits::default();
        for (name, limit) in [
            ("PBT_IMPORT_MAX_ACCOUNTS", &mut limits.max_accounts),
            ("PBT_IMPORT_MAX_LEAVES", &mut limits.max_leaves),
            ("PBT_IMPORT_MAX_CODE_SIZE", &mut limits.max_code_size),
            ("PBT_IMPORT_MAX_TOTAL_CODE_SIZE", &mut limits.max_total_code_size),
        ] {
            if let Some(value) = std::env::var_os(name) {
                *limit = parse_resource_budget(name, value)?;
            }
        }
        let mut store =
            SnapshotStore::open_with_limits(directory.join("pbt"), limits, DecodeLimits::native())?;
        if let Some(value) = std::env::var_os("PBT_CACHE_MAX_BYTES") {
            store.set_cache_budget(parse_resource_budget("PBT_CACHE_MAX_BYTES", value)?)?;
        }
        let store = Arc::new(store);
        let chain = prepare_chain((*chain).clone(), activation, &store)?;
        let mut startup = Startup::new(
            std::env::var_os("PBT_MIGRATION_SNAPSHOT"),
            std::env::var_os("PBT_MIGRATION_PREIMAGES"),
            std::env::var_os("PBT_MIGRATION_ANCHOR"),
            std::env::var_os("PBT_PRUNE_BLOCKS"),
            CatchupLimits::new(
                std::env::var_os("PBT_CATCHUP_MAX_BLOCKS"),
                std::env::var_os("PBT_CATCHUP_MAX_BAL_BYTES"),
            )?,
        )?;
        if let Some(value) = std::env::var_os("PBT_CHECKPOINT_HISTORY") {
            startup.checkpoint_history =
                u64::try_from(parse_resource_budget("PBT_CHECKPOINT_HISTORY", value)?)?;
        }
        startup.keep_mpt = match std::env::var("PBT_KEEP_MPT") {
            Ok(value) => value.parse().wrap_err("PBT_KEEP_MPT must be true or false")?,
            Err(std::env::VarError::NotPresent) => false,
            Err(error) => return Err(error.into()),
        };
        let strategy = Arc::new(PbtStateRootStrategy::new(store.clone(), activation));
        Ok(Self { chain, store, strategy, startup, _directory_lock: directory_lock })
    }

    /// Returns the chain with its PBT genesis commitment when activation is at genesis.
    pub fn chain_spec(&self) -> Arc<ChainSpec> {
        self.chain.clone()
    }

    /// Selects parallel hashing for dense PBT block and BAL updates.
    /// Configure this before obtaining add-ons or backfill; sparse updates stay incremental.
    pub fn with_parallel(mut self, parallel: bool) -> Self {
        self.strategy = Arc::new(
            PbtStateRootStrategy::new(self.store.clone(), self.strategy.activation_timestamp())
                .with_parallel(parallel),
        );
        self
    }

    /// Builds engine add-ons that maintain shadow roots and select PBT at activation.
    pub fn add_ons<N>(&self) -> PbtAddOns<N>
    where
        N: FullNodeComponents<
            Types: NodeTypes<
                Primitives = EthPrimitives,
                ChainSpec = ChainSpec,
                Payload = <EthereumNode as NodeTypes>::Payload,
            >,
            Evm: reth_ethereum::node::builder::ConfigureEngineEvm<
                alloy_rpc_types_engine::ExecutionData,
            >,
        >,
        EthereumEthApiBuilder: EthApiBuilder<N>,
    {
        addons(self.strategy.clone(), self.startup.clone())
    }

    /// Builds staged verification and unwind against PBT checkpoints.
    pub fn backfill(&self) -> PbtBackfill {
        PbtBackfill::new(self.strategy.clone(), self.startup.catchup)
    }

    /// Returns experimental proof methods for the authenticated transport only.
    pub fn rpc_module(
        &self,
    ) -> Result<jsonrpsee::RpcModule<()>, jsonrpsee::core::RegisterMethodError> {
        Ok(rpc::ProofRpc::new(self.store.clone()).into_rpc()?.remove_context())
    }
}

/// Reads the shared devnet fork timestamp. An absent or null field disables PBT.
pub fn activation_timestamp(chain: &ChainSpec) -> eyre::Result<Option<u64>> {
    match chain.genesis.config.extra_fields.get("binaryTrieTime") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => Ok(Some(serde_json::from_value(value.clone())?)),
    }
}

/// Applies the shared PBT genesis commitment and EIP-2124 fork schedule.
///
/// Call while parsing genesis, including for `reth init`, so initialization and
/// networking use the same chain identity. This does not enable the node backend.
pub fn configure_chain(chain: Arc<ChainSpec>) -> eyre::Result<Arc<ChainSpec>> {
    let Some(activation) = activation_timestamp(&chain)? else { return Ok(chain) };
    let mut chain = (*chain).clone();
    chain.hardforks.insert(BinaryTrie, ForkCondition::Timestamp(activation));
    if chain.genesis_header().timestamp >= activation {
        let root = genesis_state(&chain)?.root();
        chain = chain.map_header(|mut header| {
            header.state_root = root;
            header
        });
    }
    Ok(Arc::new(chain))
}

#[derive(Clone, Copy, Debug)]
struct BinaryTrie;

impl Hardfork for BinaryTrie {
    fn name(&self) -> &'static str {
        "BinaryTrie"
    }
}

/// Installs fork-aware PBT validation after authenticating the persisted checkpoint.
#[derive(Clone, Debug)]
pub struct PbtValidatorBuilder {
    strategy: Arc<PbtStateRootStrategy>,
    startup: Startup,
}

impl<N> EngineValidatorBuilder<N> for PbtValidatorBuilder
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Primitives = EthPrimitives,
            ChainSpec = ChainSpec,
            Payload = <EthereumNode as NodeTypes>::Payload,
        >,
        Evm: reth_ethereum::node::builder::ConfigureEngineEvm<
            alloy_rpc_types_engine::ExecutionData,
        >,
    >,
{
    type EngineValidator = BasicEngineValidator<N::Provider, N::Evm,
        <EthereumEngineValidatorBuilder as reth_ethereum::node::builder::rpc::PayloadValidatorBuilder<N>>::Validator>;

    async fn build_tree_validator(
        self,
        ctx: &reth_ethereum::node::builder::AddOnsContext<'_, N>,
        tree_config: TreeConfig,
        overlay_manager: OverlayManager<EthPrimitives>,
    ) -> eyre::Result<Self::EngineValidator> {
        eyre::ensure!(
            !ctx.config.rpc.rpc_compute_state_root_for_eth_simulate,
            "--rpc.compute-state-root-for-eth-simulate is unsupported with PBT"
        );
        let strategy = self.strategy.clone();
        let keep_mpt = self.startup.keep_mpt;
        let checkpoint_history = self.startup.checkpoint_history;
        let provider = ctx.node.provider().clone();
        let network = ctx.node.network().clone();
        let runtime = tokio::runtime::Handle::current();
        let shutdown = ctx.node.task_executor().on_shutdown_signal().clone();
        let stop = Arc::new(AtomicBool::new(false));
        let conversion_stop = stop.clone();
        let span = Span::current();
        let download_shutdown = shutdown.clone();
        let mut startup = tokio::task::spawn_blocking(move || {
            let _entered = span.enter();
            retirement::validate_startup(&strategy, &provider)?;
            self.startup.initialize_with_sources(
                &strategy,
                &provider,
                |hash, commitment, budget| {
                    runtime.block_on(async {
                        tokio::select! {
                            _ = download_shutdown.clone() => eyre::bail!("PBT catch-up interrupted by shutdown"),
                            result = async {
                                let client = tokio::time::timeout(
                                    download::REQUEST_TIMEOUT,
                                    network.fetch_client(),
                                ).await??;
                                download::download_bal(&client, hash, commitment, budget).await
                            } => result,
                        }
                    })
                },
                |anchor, preimages| conversion::convert_anchor(
                    strategy.store(),
                    &provider,
                    anchor,
                    preimages,
                    self.startup.catchup.max_blocks.get(),
                    || conversion_stop.load(Ordering::Relaxed),
                ),
            )
        });
        tokio::select! {
            result = &mut startup => result??,
            _ = shutdown => {
                stop.store(true, Ordering::Relaxed);
                eyre::bail!("PBT onboarding interrupted by shutdown");
            }
        }
        let validator = BasicEngineValidatorBuilder::<EthereumEngineValidatorBuilder>::default()
            .build_tree_validator(ctx, tree_config, overlay_manager)
            .await?;
        let strategy = self.strategy.clone();
        let provider = ctx.node.provider().clone();
        ctx.node.task_executor().spawn_critical_with_graceful_shutdown_signal(
            "pbt-checkpoint-maintenance",
            move |shutdown| async move {
                let mut retired = keep_mpt;
                let mut last_pruned = None;
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                tokio::pin!(shutdown);
                loop {
                    tokio::select! {
                        _ = &mut shutdown => return,
                        _ = interval.tick() => {}
                    }
                    let strategy = strategy.clone();
                    let provider = provider.clone();
                    let span = Span::current();
                    // Join blocking maintenance before releasing the shutdown guard.
                    let result = tokio::task::spawn_blocking(move || -> eyre::Result<_> {
                        let _entered = span.enter();
                        let retired = retired || retirement::try_retire(&strategy, &provider)?;
                        let pruned = retirement::prune_finalized(&strategy, &provider, last_pruned, checkpoint_history)?;
                        Ok((retired, pruned))
                    }).await;
                    match result {
                        Ok(Ok((is_retired, pruned))) => { retired = is_retired; last_pruned = pruned; }
                        Ok(Err(error)) => tracing::error!(target: "pbt::persistence", %error, "PBT checkpoint maintenance deferred"),
                        Err(error) => {
                            tracing::error!(target: "pbt::persistence", %error, "PBT checkpoint maintenance task failed");
                            return
                        }
                    }
                }
            },
        );
        let strategy: Arc<dyn StateRootStrategy<EthPrimitives, N::Provider, N::Evm>> =
            self.strategy;
        Ok(validator.with_state_root_strategy(strategy))
    }
}

/// Ethereum add-ons with PBT validation and payload commitments.
pub type PbtAddOns<N> = EthereumAddOns<
    N,
    EthereumEthApiBuilder,
    EthereumEngineValidatorBuilder,
    BasicEngineApiBuilder<EthereumEngineValidatorBuilder>,
    PbtValidatorBuilder,
>;

fn addons<N>(strategy: Arc<PbtStateRootStrategy>, startup: Startup) -> PbtAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            Primitives = EthPrimitives,
            ChainSpec = ChainSpec,
            Payload = <EthereumNode as NodeTypes>::Payload,
        >,
        Evm: reth_ethereum::node::builder::ConfigureEngineEvm<
            alloy_rpc_types_engine::ExecutionData,
        >,
    >,
    EthereumEthApiBuilder: EthApiBuilder<N>,
{
    EthereumAddOns::new(RpcAddOns::new(
        EthereumEthApiBuilder::default(),
        EthereumEngineValidatorBuilder::default(),
        BasicEngineApiBuilder::default(),
        PbtValidatorBuilder { strategy, startup },
        Default::default(),
        Identity::new(),
    ))
}

fn genesis_state(chain: &ChainSpec) -> eyre::Result<StateTree> {
    let mut state = StateTree::new();
    for (address, account) in &chain.genesis.alloc {
        let account_data = AccountData {
            nonce: account.nonce.unwrap_or_default(),
            balance: account.balance,
            code: account.code.clone().unwrap_or_default(),
        };
        eyre::ensure!(!account_data.is_empty(), "PBT devnet genesis cannot contain empty accounts");
        state.set_account(*address, account_data)?;
        for (slot, value) in account.storage.iter().flatten() {
            state.set_storage(*address, U256::from_be_slice(slot.as_slice()), *value)?;
        }
    }
    Ok(state)
}

fn prepare_chain(
    mut chain: ChainSpec,
    activation: u64,
    store: &SnapshotStore,
) -> eyre::Result<Arc<ChainSpec>> {
    let state = genesis_state(&chain)?;
    if chain.genesis_header().timestamp >= activation {
        chain = chain.map_header(|mut header| {
            header.state_root = state.root();
            header
        });
    }
    let configuration =
        [b"PBT1".as_slice(), chain.genesis_hash().as_slice(), &activation.to_be_bytes()].concat();
    let path = store.path().join("chain-config");
    if path.try_exists()? {
        let mut file = reth_fs_util::open(&path)?;
        eyre::ensure!(
            file.metadata()?.len() == CONFIGURATION_LENGTH as u64,
            "invalid PBT checkpoint configuration length"
        );
        let mut saved = [0; CONFIGURATION_LENGTH];
        std::io::Read::read_exact(&mut file, &mut saved)
            .map_err(|error| reth_fs_util::FsPathError::read(error, &path))?;
        eyre::ensure!(
            saved.as_slice() == configuration,
            "PBT checkpoint directory belongs to a different genesis or activation timestamp"
        );
    } else {
        eyre::ensure!(
            store.is_empty()?,
            "PBT checkpoint directory is populated but chain-config is missing"
        );
        store.save_block_with_number(chain.genesis_hash(), 0, &state)?;
        reth_fs_util::atomic_write_file(&path, |file| {
            std::io::Write::write_all(file, &configuration)
        })?;
    }
    Ok(Arc::new(chain))
}

const CONFIGURATION_LENGTH: usize = 4 + 32 + 8;
const DEFAULT_CHECKPOINT_HISTORY: u64 = 64;

fn parse_resource_budget(name: &str, value: OsString) -> eyre::Result<usize> {
    value
        .to_str()
        .ok_or_else(|| eyre::eyre!("{name} must be a nonnegative integer"))?
        .parse()
        .wrap_err_with(|| format!("{name} must be a nonnegative integer"))
}

fn lock_directory(directory: &Path) -> eyre::Result<File> {
    reth_fs_util::create_dir_all(directory)?;
    let path = directory.join("pbt.lock");
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| reth_fs_util::FsPathError::open(error, &path))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => eyre::bail!("PBT data directory is already in use"),
        Err(TryLockError::Error(error)) => Err(reth_fs_util::FsPathError::open(error, path).into()),
    }
}

#[derive(Clone, Debug)]
struct Startup {
    migration: Option<MigrationFiles>,
    obsolete_checkpoints: Vec<B256>,
    catchup: CatchupLimits,
    keep_mpt: bool,
    checkpoint_history: u64,
}

impl Default for Startup {
    fn default() -> Self {
        Self {
            migration: None,
            obsolete_checkpoints: Vec::new(),
            catchup: CatchupLimits::default(),
            keep_mpt: false,
            checkpoint_history: DEFAULT_CHECKPOINT_HISTORY,
        }
    }
}

impl Startup {
    fn new(
        snapshot: Option<OsString>,
        preimages: Option<OsString>,
        anchor: Option<OsString>,
        obsolete: Option<OsString>,
        catchup: CatchupLimits,
    ) -> eyre::Result<Self> {
        let migration = match (snapshot, preimages, anchor) {
            (snapshot, Some(preimages), Some(anchor)) => {
                let anchor = anchor
                    .into_string()
                    .map_err(|_| eyre::eyre!("PBT_MIGRATION_ANCHOR must contain a UTF-8 block hash"))?
                    .parse()?;
                Some(MigrationFiles {
                    source: snapshot.map_or(MigrationSource::Mpt, |path| MigrationSource::Snapshot(path.into())),
                    preimages: preimages.into(),
                    anchor,
                })
            }
            (None, None, None) => None,
            _ => eyre::bail!(
                "PBT_MIGRATION_PREIMAGES and PBT_MIGRATION_ANCHOR are required together; PBT_MIGRATION_SNAPSHOT is optional for self-conversion"
            ),
        };
        let obsolete_checkpoints = obsolete
            .map(|hashes| {
                let hashes = hashes
                    .into_string()
                    .map_err(|_| eyre::eyre!("PBT_PRUNE_BLOCKS must contain UTF-8 block hashes"))?;
                hashes
                    .split(',')
                    .map(|hash| hash.parse::<B256>().map_err(Into::into))
                    .collect::<eyre::Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self { migration, obsolete_checkpoints, catchup, ..Default::default() })
    }

    #[cfg(test)]
    fn initialize<P>(&self, strategy: &PbtStateRootStrategy, provider: &P) -> eyre::Result<()>
    where
        P: BalProvider + BlockIdReader + BlockNumReader + HeaderProvider + StageCheckpointReader,
    {
        self.initialize_with_bal_fetch(strategy, provider, |_, _, _| {
            eyre::bail!("PBT catch-up BAL is unavailable")
        })
    }

    #[cfg(test)]
    fn initialize_with_bal_fetch<P>(
        &self,
        strategy: &PbtStateRootStrategy,
        provider: &P,
        fetch: impl FnMut(B256, B256, usize) -> eyre::Result<Bytes>,
    ) -> eyre::Result<()>
    where
        P: BalProvider + BlockIdReader + BlockNumReader + HeaderProvider + StageCheckpointReader,
    {
        self.initialize_with_sources(strategy, provider, fetch, |_, _| {
            eyre::bail!("MPT self-conversion is unavailable without a retained-state provider")
        })
    }

    fn initialize_with_sources<P>(
        &self,
        strategy: &PbtStateRootStrategy,
        provider: &P,
        fetch: impl FnMut(B256, B256, usize) -> eyre::Result<Bytes>,
        mut convert: impl FnMut(&SealedHeader<P::Header>, &Path) -> eyre::Result<B256>,
    ) -> eyre::Result<()>
    where
        P: BalProvider + BlockIdReader + BlockNumReader + HeaderProvider + StageCheckpointReader,
    {
        let finalized = if self.migration.is_some() || !self.obsolete_checkpoints.is_empty() {
            let hash = provider.finalized_block_hash()?.ok_or_else(|| {
                eyre::eyre!("PBT onboarding or pruning requires a locally finalized block")
            })?;
            Some(
                provider
                    .sealed_header_by_hash(hash)?
                    .ok_or_else(|| eyre::eyre!("missing local finalized header"))?,
            )
        } else {
            None
        };
        // Headers can be ahead of verified state after an interrupted staged sync.
        let number = match provider.get_stage_checkpoint(reth_stages::StageId::Finish)? {
            Some(checkpoint) => checkpoint.block_number,
            None => provider.last_block_number()?,
        };
        let tip = provider
            .sealed_header(number)?
            .ok_or_else(|| eyre::eyre!("missing persisted tip header"))?;
        if let Some(migration) = &self.migration {
            let finalized =
                finalized.as_ref().ok_or_else(|| eyre::eyre!("missing local finalized header"))?;
            let header = migration.anchor_header(provider, finalized)?;
            eyre::ensure!(
                header.timestamp() < strategy.activation_timestamp(),
                "PBT migration anchor must precede activation"
            );
            // A completed migration must remain restartable after source history is pruned.
            // The persisted tip is authenticated below, even when its reference already exists.
            if !strategy.store().contains_block(tip.hash())? {
                if strategy.store().contains_block(header.hash())? {
                    strategy.restore_parent(
                        header.hash(),
                        header.state_root(),
                        header.timestamp(),
                    )?;
                } else {
                    match &migration.source {
                        MigrationSource::Snapshot(snapshot) => {
                            strategy.store().import_anchor(
                                header.hash(),
                                header.state_root(),
                                snapshot,
                                &migration.preimages,
                            )?;
                        }
                        MigrationSource::Mpt => {
                            convert(&header, &migration.preimages)?;
                        }
                    }
                }
                self.catch_up_with_bal_fetch(strategy, provider, &header, &tip, fetch)?;
            }
        }
        strategy
            .restore_parent(tip.hash(), tip.state_root(), tip.timestamp())
            .wrap_err("PBT persisted tip checkpoint is missing or inconsistent")?;
        if !self.obsolete_checkpoints.is_empty() {
            let finalized =
                finalized.ok_or_else(|| eyre::eyre!("missing local finalized header"))?;
            let trusted = strategy.restore_parent(
                finalized.hash(),
                finalized.state_root(),
                finalized.timestamp(),
            )?;
            eyre::ensure!(
                strategy.store().load_block(finalized.hash())?.root() == trusted.root(),
                "PBT finalized checkpoint reference does not match its header"
            );
            let mut unmatched = self.obsolete_checkpoints.iter().copied().collect::<BTreeSet<_>>();
            eyre::ensure!(
                !unmatched.contains(&finalized.hash()),
                "cannot prune the finalized PBT checkpoint"
            );
            let mut ancestor = finalized.clone();
            while !unmatched.is_empty() && ancestor.number() != 0 {
                let parent = provider
                    .sealed_header_by_hash(ancestor.parent_hash())?
                    .ok_or_else(|| eyre::eyre!("missing finalized ancestor header"))?;
                eyre::ensure!(
                    parent.number().checked_add(1) == Some(ancestor.number()),
                    "inconsistent finalized header ancestry"
                );
                unmatched.remove(&parent.hash());
                ancestor = parent;
            }
            eyre::ensure!(
                unmatched.is_empty(),
                "PBT prune candidates must be strict ancestors of the finalized block"
            );
            strategy.store().prune_finalized(finalized.hash(), &self.obsolete_checkpoints)?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn catch_up<P>(
        &self,
        strategy: &PbtStateRootStrategy,
        provider: &P,
        anchor: &SealedHeader<P::Header>,
        tip: &SealedHeader<P::Header>,
    ) -> eyre::Result<()>
    where
        P: BalProvider + HeaderProvider,
    {
        self.catch_up_with_bal_fetch(strategy, provider, anchor, tip, |_, _, _| {
            eyre::bail!("PBT catch-up BAL is unavailable")
        })
    }

    fn catch_up_with_bal_fetch<P>(
        &self,
        strategy: &PbtStateRootStrategy,
        provider: &P,
        anchor: &SealedHeader<P::Header>,
        tip: &SealedHeader<P::Header>,
        mut fetch: impl FnMut(B256, B256, usize) -> eyre::Result<Bytes>,
    ) -> eyre::Result<()>
    where
        P: BalProvider + HeaderProvider,
    {
        let mut headers = Vec::new();
        let mut ancestor = tip.clone();
        while ancestor.hash() != anchor.hash() {
            eyre::ensure!(
                ancestor.number() > anchor.number(),
                "PBT migration anchor is not an ancestor of the persisted tip"
            );
            eyre::ensure!(
                headers.len() < self.catchup.max_blocks.get(),
                "PBT catch-up block budget exceeded"
            );
            let parent = provider
                .sealed_header_by_hash(ancestor.parent_hash())?
                .ok_or_else(|| eyre::eyre!("PBT catch-up header is unavailable"))?;
            eyre::ensure!(
                parent.number().checked_add(1) == Some(ancestor.number()) &&
                    parent.timestamp() < ancestor.timestamp(),
                "inconsistent PBT catch-up header ancestry"
            );
            headers.push(ancestor);
            ancestor = parent;
        }
        let mut state =
            strategy.restore_parent(anchor.hash(), anchor.state_root(), anchor.timestamp())?;
        let mut remaining_bytes = self.catchup.max_bal_bytes.get();
        for header in headers.into_iter().rev() {
            let commitment = header
                .block_access_list_hash()
                .ok_or_else(|| eyre::eyre!("PBT catch-up header has no BAL commitment"))?;
            let local = provider.bal_store().get_by_hash(header.hash())?;
            let fetched = local.is_none();
            let bytes = match local {
                Some(bytes) => bytes,
                None => fetch(header.hash(), commitment, remaining_bytes)?,
            };
            remaining_bytes = remaining_bytes
                .checked_sub(bytes.len())
                .ok_or_else(|| eyre::eyre!("PBT catch-up BAL byte budget exceeded"))?;
            let raw = RawBal::new(bytes);
            raw.ensure_hash(commitment)?;
            let bal = DecodedBal::from_raw_bal(raw.clone())?;
            let next = strategy.replay(&state, bal.as_bal().as_slice(), commitment)?;
            let root = if header.timestamp() >= strategy.activation_timestamp() {
                next.root()
            } else {
                mpt_root(&next)
            };
            if root != header.state_root() {
                return Err(reth_stages::StageError::Block {
                    block: Box::new(header.block_with_parent()),
                    error: reth_stages::BlockErrorKind::Validation(
                        reth_ethereum::consensus::ConsensusError::BodyStateRootDiff(
                            reth_ethereum::primitives::GotExpected {
                                got: root,
                                expected: header.state_root(),
                            }
                            .into(),
                        ),
                    ),
                }
                .into());
            }
            if fetched {
                provider.bal_store().insert(header.num_hash(), raw)?;
                provider.bal_store().flush(&[header.num_hash()])?;
            }
            strategy.store().save_block_with_number(header.hash(), header.number(), &next)?;
            state = next;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct CatchupLimits {
    max_blocks: NonZeroUsize,
    max_bal_bytes: NonZeroUsize,
}

impl Default for CatchupLimits {
    fn default() -> Self {
        Self { max_blocks: DEFAULT_CATCHUP_BLOCKS, max_bal_bytes: DEFAULT_CATCHUP_BAL_BYTES }
    }
}

impl CatchupLimits {
    fn new(blocks: Option<OsString>, bytes: Option<OsString>) -> eyre::Result<Self> {
        let parse = |value: Option<OsString>, default| -> eyre::Result<NonZeroUsize> {
            match value {
                Some(value) => Ok(value
                    .into_string()
                    .map_err(|_| eyre::eyre!("PBT catch-up budgets must be UTF-8 integers"))?
                    .parse()?),
                None => Ok(default),
            }
        };
        Ok(Self {
            max_blocks: parse(blocks, DEFAULT_CATCHUP_BLOCKS)?,
            max_bal_bytes: parse(bytes, DEFAULT_CATCHUP_BAL_BYTES)?,
        })
    }
}

const DEFAULT_CATCHUP_BLOCKS: NonZeroUsize =
    NonZeroUsize::new(1024).expect("block budget is nonzero");
const DEFAULT_CATCHUP_BAL_BYTES: NonZeroUsize =
    NonZeroUsize::new(64 * 1024 * 1024).expect("BAL byte budget is nonzero");

#[derive(Clone, Debug)]
struct MigrationFiles {
    source: MigrationSource,
    preimages: PathBuf,
    anchor: B256,
}

impl MigrationFiles {
    fn anchor_header<P: HeaderProvider>(
        &self,
        provider: &P,
        finalized: &SealedHeader<P::Header>,
    ) -> eyre::Result<SealedHeader<P::Header>> {
        let anchor = provider
            .sealed_header_by_hash(self.anchor)?
            .ok_or_else(|| eyre::eyre!("PBT migration anchor header is unavailable"))?;
        eyre::ensure!(
            anchor.number() <= finalized.number(),
            "PBT migration anchor is not finalized"
        );
        let mut ancestor = finalized.clone();
        while ancestor.number() > anchor.number() {
            let parent = provider
                .sealed_header_by_hash(ancestor.parent_hash())?
                .ok_or_else(|| eyre::eyre!("PBT migration finalized ancestry is unavailable"))?;
            eyre::ensure!(
                parent.number().checked_add(1) == Some(ancestor.number()) &&
                    parent.timestamp() < ancestor.timestamp(),
                "inconsistent PBT migration finalized ancestry"
            );
            ancestor = parent;
        }
        eyre::ensure!(
            ancestor.hash() == anchor.hash(),
            "PBT migration anchor is not an ancestor of the finalized block"
        );
        Ok(anchor)
    }
}

#[derive(Clone, Debug)]
enum MigrationSource {
    Snapshot(PathBuf),
    Mpt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{hex, Address, Bytes};
    use alloy_rpc_types_engine::ForkchoiceState;
    use jsonrpsee::{core::client::ClientT, rpc_params};
    use reth_e2e_test_utils::{
        eth_payload_attributes,
        node::{Finality, NodeTestContext},
        test_chain_spec, test_node_config,
        wallet::Wallet,
        Adapter, E2ETestSetupBuilder,
    };
    use reth_ethereum::{
        chainspec::{EthereumHardfork, Hardforks, Head},
        node::{
            builder::{
                components::{BasicPayloadServiceBuilder, ComponentsBuilder},
                FullNodeTypes, Node, NodeAdapter, NodeBuilder, NodeHandle,
            },
            core::args::RpcServerArgs,
            EthereumConsensusBuilder, EthereumExecutorBuilder, EthereumNetworkBuilder,
            EthereumPayloadBuilder, EthereumPoolBuilder,
        },
        provider::{
            db::{tables, transaction::DbTx},
            test_utils::{MockEthProvider, NoopProvider},
            BalStoreHandle, DBProvider, DatabaseProviderFactory, InMemoryBalStore,
            StageCheckpointWriter, StateProviderFactory,
        },
        tasks::Runtime,
    };
    use reth_payload_primitives::BuiltPayload;
    use reth_trie_pbt::{
        key,
        migration::{ArtifactError, Artifacts},
        persistence::StoreError,
        Blake3Hasher, TreeHasher,
    };
    use std::{
        cell::RefCell,
        sync::{mpsc, Mutex},
        time::Duration,
    };

    thread_local! {
        static RETIREMENT_PAUSE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Debug)]
    struct RetirementHasher;

    impl TreeHasher for RetirementHasher {
        fn hash(parts: &[&[u8]]) -> B256 {
            if let Some(pause) = RETIREMENT_PAUSE.with(|pause| pause.borrow_mut().take()) {
                pause();
            }
            Blake3Hasher::hash(parts)
        }
    }

    type TestNode = NodeTestContext<Adapter<EthereumNode>, PbtAddOns<Adapter<EthereumNode>>>;

    async fn setup(
        activation: u64,
    ) -> eyre::Result<(TestNode, Arc<SnapshotStore>, tempfile::TempDir)> {
        setup_for(activation, EthereumHardfork::Cancun).await
    }

    async fn setup_for(
        activation: u64,
        fork: EthereumHardfork,
    ) -> eyre::Result<(TestNode, Arc<SnapshotStore>, tempfile::TempDir)> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let chain = prepare_chain((*test_chain_spec(fork)).clone(), activation, &store)?;
        let node = launch_test_node(chain, store.clone(), activation, 64).await?;
        Ok((node, store, directory))
    }

    async fn launch_test_node(
        chain: Arc<ChainSpec>,
        store: Arc<SnapshotStore>,
        activation: u64,
        backfill_threshold: u64,
    ) -> eyre::Result<TestNode> {
        launch_test_node_with_parallel(chain, store, activation, backfill_threshold, false).await
    }

    async fn launch_test_node_with_parallel(
        chain: Arc<ChainSpec>,
        store: Arc<SnapshotStore>,
        activation: u64,
        backfill_threshold: u64,
        parallel: bool,
    ) -> eyre::Result<TestNode> {
        let mut config = test_node_config(chain.clone())
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
        config.engine.backfill_run_threshold = backfill_threshold;
        let strategy =
            Arc::new(PbtStateRootStrategy::new(store.clone(), activation).with_parallel(parallel));
        let backfill = PbtBackfill::new(strategy.clone(), CatchupLimits::default());
        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(config)
            .testing_node(Runtime::test())
            .with_types::<EthereumNode>()
            .with_components(EthereumNode::components())
            .with_add_ons(addons(strategy, Startup::default()))
            .extend_rpc_modules(move |ctx| {
                ctx.modules.remove_method_from_configured("eth_getProof");
                ctx.auth_module.remove_auth_method("eth_getProof");
                ctx.auth_module.merge_auth_methods(rpc::ProofRpc::new(store).into_rpc()?)?;
                Ok(())
            })
            .launch_with_debug_capabilities_and_backfill(backfill)
            .await?;
        let node =
            NodeTestContext::new(node, move |timestamp| eth_payload_attributes(&*chain, timestamp))
                .await?;
        Ok(node)
    }

    #[tokio::test]
    async fn builds_imports_and_reorgs_pbt_blocks() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let (mut node, store, _directory) = setup(0).await?;
        let genesis = node.block_hash(0);
        node.set_finality(Finality::Keep);
        let mut sender = Wallet::new(1).account(0).with_gas_limit(1_000_000);
        let recipient = Address::repeat_byte(0x77);
        let transfer = sender.transfer(recipient, U256::from(7)).await;
        let mined = node.mine([transfer]).await?.ensure_success()?;
        assert_eq!(
            store.load_block(mined.block().hash())?.account(recipient).unwrap().balance,
            U256::from(7)
        );
        let first = node.advance_blocks(2).await?;
        for block in &first {
            assert_eq!(store.load_block(block.block().hash())?.root(), block.block().state_root);
        }
        let fork = node.advance_fork(genesis, 3).await?;
        node.reorg_to(first[1].block().hash()).await?;
        assert_eq!(node.current_forkchoice_state()?.head_block_hash, first[1].block().hash());
        node.reorg_to(fork[2].block().hash()).await?;
        assert_eq!(node.current_forkchoice_state()?.head_block_hash, fork[2].block().hash());
        Ok(())
    }

    #[tokio::test]
    async fn parallel_payload_building_matches_checkpoint_and_bal_roots() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let chain =
            prepare_chain((*test_chain_spec(EthereumHardfork::Amsterdam)).clone(), 0, &store)?;
        let mut node = launch_test_node_with_parallel(chain, store.clone(), 0, 64, true).await?;
        node.set_finality(Finality::Keep);
        let parent = store.load_block(node.block_hash(0))?;
        let mut sender = Wallet::new(1).account(0).with_gas_limit(1_000_000);
        let recipient = Address::repeat_byte(0x77);
        let transfer = sender.transfer(recipient, U256::from(7)).await;
        let mined = node.mine([transfer]).await?.ensure_success()?;
        let state = store.load_block(mined.block().hash())?;
        assert_eq!(state.account(recipient).unwrap().balance, U256::from(7));
        assert_eq!(state.root(), mined.block().state_root);
        let raw = node.inner.provider.bal_store().get_by_hash(mined.block().hash())?.unwrap();
        let bal = DecodedBal::from_raw_bal(RawBal::new(raw))?;
        let strategy = PbtStateRootStrategy::new(store, 0).with_parallel(true);
        let replayed = strategy.replay(
            &parent,
            bal.as_bal().as_slice(),
            mined.block().block_access_list_hash().unwrap(),
        )?;
        assert_eq!(replayed.root(), state.root());
        Ok(())
    }

    #[tokio::test]
    async fn activation_switches_commitment_and_keeps_pre_fork_state() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let activation = 1_710_338_137;
        let (mut node, store, _directory) = setup(activation).await?;
        node.set_finality(Finality::Keep);
        let first = node.advance_block().await?;
        let pre = store.load_block(first.block().hash())?;
        assert_eq!(reth_trie_pbt::migration::mpt_root(&pre), first.block().state_root);
        let second = node.advance_block().await?;
        assert_eq!(store.load_block(second.block().hash())?.root(), second.block().state_root);
        assert_ne!(
            reth_trie_pbt::migration::mpt_root(&store.load_block(second.block().hash())?),
            second.block().state_root
        );
        let fork = node.advance_fork(node.block_hash(0), 3).await?;
        assert_eq!(store.load_block(fork[2].block().hash())?.root(), fork[2].block().state_root);
        Ok(())
    }

    #[tokio::test]
    async fn deployment_and_storage_write_commit_code_and_overflow_storage() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let (mut node, store, _directory) = setup(0).await?;
        let mut sender = Wallet::new(1).account(0).with_gas_limit(1_000_000);
        // Runtime writes 1 to slot 64, which belongs to the overflow storage zone.
        let contract = sender.next_contract_address();
        let deployment = sender.deploy(hex!("6006600c60003960066000f3600160405500").to_vec()).await;
        node.mine([deployment]).await?.ensure_success()?;
        let call = sender.call(contract, Bytes::new()).await;
        let mined = node.mine([call]).await?.ensure_success()?;
        let state = store.load_block(mined.block().hash())?;
        assert_eq!(state.account(contract).unwrap().code.as_ref(), hex!("600160405500"));
        assert_eq!(state.storage(contract, U256::from(64)), B256::from(U256::from(1)));
        assert_eq!(state.root(), mined.block().state_root);
        Ok(())
    }

    #[tokio::test]
    async fn builds_and_validates_pbt_with_block_access_lists() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let (mut node, store, _directory) = setup_for(0, EthereumHardfork::Amsterdam).await?;
        let recipient = Address::repeat_byte(0x77);
        let mut sender = Wallet::new(1).account(0);
        let transfer = sender.transfer(recipient, U256::from(9)).await;
        let mined = node.mine([transfer]).await?.ensure_success()?;
        let state = store.load_block(mined.block().hash())?;
        assert_eq!(state.root(), mined.block().state_root);
        assert_eq!(state.account(recipient).unwrap().balance, U256::from(9));
        let bytes = mined.payload.block_access_list().expect("Amsterdam payload carries a BAL");
        let mut input = bytes.as_ref();
        let bal: Vec<alloy_eip7928::AccountChanges> = alloy_rlp::Decodable::decode(&mut input)?;
        assert!(input.is_empty());
        let parent = store.load_block(mined.block().parent_hash)?;
        let replayed = reth_trie_pbt::replay::replay(
            &parent,
            &bal,
            mined.block().block_access_list_hash.expect("Amsterdam header commits to its BAL"),
        )?;
        assert_eq!(replayed.root(), state.root());
        Ok(())
    }

    #[tokio::test]
    async fn earliest_payload_uses_pbt_even_when_racing_an_empty_build() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let (mut node, store, _directory) = setup(0).await?;
        let attributes = node.payload.next_attributes();
        let choice = node
            .engine
            .forkchoice_updated_with_attributes(node.current_forkchoice_state()?, attributes)
            .await?;
        let id = choice.payload_id.ok_or_else(|| eyre::eyre!("missing payload id"))?;
        let payload = node
            .inner
            .payload_builder_handle
            .resolve_kind(id, reth_payload_primitives::PayloadKind::Earliest)
            .await
            .ok_or_else(|| eyre::eyre!("missing payload job"))??;
        assert_eq!(store.load(payload.block().state_root)?.root(), payload.block().state_root);
        node.import_payload(payload).await?;
        Ok(())
    }

    #[tokio::test]
    async fn missing_checkpoint_rejects_startup_without_an_mpt_fallback() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let directory = tempfile::tempdir()?;
        let anchor = SnapshotStore::open(directory.path().join("anchor"), DecodeLimits::default())?;
        let chain =
            prepare_chain((*test_chain_spec(EthereumHardfork::Cancun)).clone(), 0, &anchor)?;
        let missing = Arc::new(SnapshotStore::open(
            directory.path().join("missing"),
            DecodeLimits::default(),
        )?);
        let error = match launch_test_node(chain, missing, 0, 64).await {
            Ok(_) => eyre::bail!("startup accepted a missing PBT checkpoint"),
            Err(error) => error,
        };
        assert_eq!(error.to_string(), "PBT persisted tip checkpoint is missing or inconsistent");
        Ok(())
    }

    #[derive(Clone, Debug, Default)]
    struct RestartNode {
        checkpoints: PathBuf,
        activation: u64,
        startup: Startup,
    }

    impl NodeTypes for RestartNode {
        type Primitives = <EthereumNode as NodeTypes>::Primitives;
        type ChainSpec = <EthereumNode as NodeTypes>::ChainSpec;
        type Storage = <EthereumNode as NodeTypes>::Storage;
        type Payload = <EthereumNode as NodeTypes>::Payload;
    }

    impl<N: FullNodeTypes<Types = Self>> Node<N> for RestartNode {
        type ComponentsBuilder = ComponentsBuilder<
            N,
            EthereumPoolBuilder,
            BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
            EthereumNetworkBuilder,
            EthereumExecutorBuilder,
            EthereumConsensusBuilder,
        >;
        type AddOns = PbtAddOns<NodeAdapter<N>>;

        fn components_builder(&self) -> Self::ComponentsBuilder {
            EthereumNode::components()
        }

        fn add_ons(&self) -> Self::AddOns {
            let store =
                Arc::new(SnapshotStore::open(&self.checkpoints, DecodeLimits::default()).unwrap());
            addons(
                Arc::new(PbtStateRootStrategy::new(store, self.activation)),
                self.startup.clone(),
            )
        }
    }

    #[tokio::test]
    async fn finalized_anchor_onboarding_survives_restart_and_activation_reorg() -> eyre::Result<()>
    {
        anchor_onboarding(false).await
    }

    #[tokio::test]
    async fn mpt_self_conversion_survives_restart_and_activation_reorg() -> eyre::Result<()> {
        anchor_onboarding(true).await
    }

    #[tokio::test]
    async fn staged_peer_sync_crosses_activation() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let activation = 1_710_338_138;
        let (mut source, _, _source_directory) =
            setup_for(activation, EthereumHardfork::Amsterdam).await?;
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let chain = prepare_chain(
            (*test_chain_spec(EthereumHardfork::Amsterdam)).clone(),
            activation,
            &store,
        )?;
        let mut follower = launch_test_node(chain, store.clone(), activation, 1).await?;
        let recipient = Address::repeat_byte(0x79);
        let mut sender = Wallet::new(1).account(0);
        let transaction = sender.transfer(recipient, U256::from(13)).await;
        source.mine([transaction]).await?.ensure_success()?;
        let blocks = source.advance_blocks(6).await?;
        let tip = blocks.last().unwrap().block();
        assert!(tip.timestamp >= activation);
        tokio::time::timeout(Duration::from_secs(30), source.connect(&mut follower)).await?;
        follower.sync_to(tip.hash()).await?;
        let checkpoint = reth_ethereum::provider::StageCheckpointReader::get_stage_checkpoint(
            &follower.inner.provider,
            reth_stages::StageId::MerkleExecute,
        )?
        .unwrap();
        assert!(checkpoint.block_number > 1, "staged verification must have run");
        assert!(
            checkpoint.entities_stage_checkpoint().is_some(),
            "staged Merkle verification must have run"
        );
        let state = store.load_block(tip.hash())?;
        assert_eq!(state.root(), tip.state_root);
        assert_eq!(state.account(recipient).unwrap().balance, U256::from(13));
        follower.set_next_payload_timestamp(tip.timestamp + 1)?;
        let next = follower.advance_block().await?;
        assert_eq!(store.load_block(next.block().hash())?.root(), next.block().state_root);
        Ok(())
    }

    async fn anchor_onboarding(self_convert: bool) -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        let activation = 1_710_338_138;
        let directory = tempfile::tempdir()?;
        let checkpoints = directory.path().join("pbt");
        let store = SnapshotStore::open(&checkpoints, DecodeLimits::default())?;
        let chain = prepare_chain(
            (*test_chain_spec(EthereumHardfork::Amsterdam)).clone(),
            activation,
            &store,
        )?;
        let startup = Arc::new(Mutex::new(Startup::default()));
        let node_checkpoints = checkpoints.clone();
        let node_startup = startup.clone();
        let backfill_checkpoints = checkpoints.clone();
        let (mut node, _) = E2ETestSetupBuilder::<RestartNode>::new(1, chain)
            .with_node(move |_| RestartNode {
                checkpoints: node_checkpoints.clone(),
                activation,
                startup: node_startup.lock().unwrap().clone(),
            })
            .with_restartable_nodes()
            .with_backfill(move |_| {
                PbtBackfill::new(
                    Arc::new(PbtStateRootStrategy::new(
                        Arc::new(
                            SnapshotStore::open(&backfill_checkpoints, DecodeLimits::default())
                                .unwrap(),
                        ),
                        activation,
                    )),
                    CatchupLimits::default(),
                )
            })
            .with_node_builder_modifier(|builder| {
                builder.extend_rpc_modules(|ctx| {
                    ctx.modules.remove_method_from_configured("eth_getProof");
                    ctx.auth_module.remove_auth_method("eth_getProof");
                    Ok(())
                })
            })
            .build_single()
            .await?;
        let mut sender = Wallet::new(1).account(0);
        let recipient = Address::repeat_byte(0x77);
        let transaction = sender.transfer(recipient, U256::from(9)).await;
        let anchor = node.mine([transaction]).await?.ensure_success()?;
        assert!(anchor.block().timestamp < activation);
        let anchor_hash = anchor.block().hash();
        node.set_finality(Finality::Keep);
        let catchup = node.advance_blocks(3).await?;
        assert!(catchup[0].block().timestamp < activation);
        assert!(catchup[2].block().timestamp >= activation);
        let persisted_tip = catchup[2].block().hash();
        let stopped = node.stop().await?;
        let state = store.load_block(anchor_hash)?;
        let artifacts = Artifacts::from_state(&state)?;
        let snapshot = directory.path().join("snapshot");
        reth_fs_util::write(&snapshot, &artifacts.snapshot)?;
        let files = MigrationFiles {
            source: if self_convert {
                MigrationSource::Mpt
            } else {
                MigrationSource::Snapshot(snapshot)
            },
            preimages: directory.path().join("preimages"),
            anchor: anchor_hash,
        };
        reth_fs_util::write(&files.preimages, &artifacts.preimages)?;
        drop(store);
        // Remove the old shadow checkpoints: onboarding must reconstruct from the artifacts.
        reth_fs_util::remove_dir_all(checkpoints.join("states"))?;
        reth_fs_util::remove_dir_all(checkpoints.join("blocks"))?;
        startup.lock().unwrap().migration = Some(files);
        let mut node = stopped.start().await?;
        assert_eq!(node.current_forkchoice_state()?.head_block_hash, persisted_tip);
        node.set_finality(Finality::Keep);
        let canonical = node.advance_block().await?;
        let reopened = SnapshotStore::<Blake3Hasher>::open(&checkpoints, DecodeLimits::default())?;
        assert_eq!(reopened.load_block(anchor_hash)?.root(), state.root());
        assert_eq!(
            mpt_root(&reopened.load_block(catchup[0].block().hash())?),
            catchup[0].block().state_root
        );
        assert_eq!(reopened.load_block(persisted_tip)?.root(), catchup[2].block().state_root);
        assert_eq!(
            reopened.load_block(canonical.block().hash())?.root(),
            canonical.block().state_root
        );
        assert_eq!(
            reopened.load_block(canonical.block().hash())?.account(recipient).unwrap().balance,
            U256::from(9)
        );
        let fork = node.advance_fork(anchor_hash, 2).await?;
        node.reorg_to(canonical.block().hash()).await?;
        node.reorg_to(fork[1].block().hash()).await?;
        let tip = fork[1].block().hash();
        assert!(fork[1].block().timestamp >= activation);
        // The same artifacts must remain usable after finality crosses activation.
        let response = node
            .engine
            .forkchoice_updated(ForkchoiceState {
                head_block_hash: tip,
                safe_block_hash: tip,
                finalized_block_hash: tip,
            })
            .await?;
        assert!(response.payload_status.is_valid());
        let stopped = node.stop().await?;
        drop(reopened);
        let files = startup.lock().unwrap().migration.clone().unwrap();
        reth_fs_util::remove_file(files.preimages)?;
        if let MigrationSource::Snapshot(snapshot) = files.source {
            reth_fs_util::remove_file(snapshot)?;
        }
        startup.lock().unwrap().catchup.max_blocks = NonZeroUsize::new(1).unwrap();
        let mut node = stopped.start().await?;
        assert_eq!(node.current_forkchoice_state()?.head_block_hash, tip);
        let next = node.advance_block().await?;
        assert_eq!(
            SnapshotStore::<Blake3Hasher>::open(&checkpoints, DecodeLimits::default())?
                .load_block(next.block().hash())?
                .root(),
            next.block().state_root
        );
        node.stop().await?;
        Ok(())
    }

    #[test]
    fn startup_requires_complete_migration_configuration() {
        let snapshot = Some(OsString::from("snapshot"));
        let preimages = Some(OsString::from("preimages"));
        let anchor = Some(OsString::from(B256::ZERO.to_string()));
        for (snapshot, preimages, anchor) in [
            (snapshot.clone(), None, None),
            (None, preimages.clone(), None),
            (None, None, anchor.clone()),
            (snapshot.clone(), preimages.clone(), None),
            (snapshot.clone(), None, anchor.clone()),
        ] {
            assert_eq!(
                Startup::new(snapshot, preimages, anchor, None, CatchupLimits::default())
                    .unwrap_err()
                    .to_string(),
                "PBT_MIGRATION_PREIMAGES and PBT_MIGRATION_ANCHOR are required together; PBT_MIGRATION_SNAPSHOT is optional for self-conversion"
            );
        }
        assert!(Startup::new(None, None, None, None, CatchupLimits::default())
            .unwrap()
            .migration
            .is_none());
        assert!(matches!(
            Startup::new(None, preimages.clone(), anchor.clone(), None, CatchupLimits::default())
                .unwrap()
                .migration
                .unwrap()
                .source,
            MigrationSource::Mpt
        ));
        assert!(Startup::new(snapshot, preimages, anchor, None, CatchupLimits::default())
            .unwrap()
            .migration
            .is_some());
    }

    #[test]
    fn checkpoint_directory_pins_genesis_and_activation() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default())?;
        let chain = (*test_chain_spec(EthereumHardfork::Cancun)).clone();
        prepare_chain(chain.clone(), 1, &store)?;
        prepare_chain(chain.clone(), 1, &store)?;
        assert_eq!(
            prepare_chain(chain.clone(), 2, &store).unwrap_err().to_string(),
            "PBT checkpoint directory belongs to a different genesis or activation timestamp"
        );
        let changed = chain.map_header(|mut header| {
            header.extra_data = Bytes::from_static(b"other");
            header
        });
        assert_eq!(
            prepare_chain(changed, 1, &store).unwrap_err().to_string(),
            "PBT checkpoint directory belongs to a different genesis or activation timestamp"
        );
        for bytes in [vec![0; CONFIGURATION_LENGTH - 1], vec![0; CONFIGURATION_LENGTH + 1]] {
            reth_fs_util::write(store.path().join("chain-config"), bytes)?;
            assert_eq!(
                prepare_chain((*test_chain_spec(EthereumHardfork::Cancun)).clone(), 1, &store)
                    .unwrap_err()
                    .to_string(),
                "invalid PBT checkpoint configuration length"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn finalized_pruning_only_removes_proven_ancestors() -> eyre::Result<()> {
        let (mut node, store, _directory) = setup(0).await?;
        node.set_finality(Finality::Keep);
        let genesis = node.block_hash(0);
        let sibling = node.advance_block().await?;
        let finalized = node.advance_fork(genesis, 1).await?;
        let finalized_hash = finalized[0].block().hash();
        let response = node
            .engine
            .forkchoice_updated(ForkchoiceState {
                head_block_hash: finalized_hash,
                safe_block_hash: finalized_hash,
                finalized_block_hash: finalized_hash,
            })
            .await?;
        assert!(response.payload_status.is_valid());
        let strategy = PbtStateRootStrategy::new(store.clone(), 0);
        let active_anchor = Startup {
            migration: Some(MigrationFiles {
                source: MigrationSource::Snapshot("snapshot".into()),
                preimages: "preimages".into(),
                anchor: finalized_hash,
            }),
            ..Default::default()
        };
        assert_eq!(
            active_anchor.initialize(&strategy, &node.inner.provider).unwrap_err().to_string(),
            "PBT migration anchor must precede activation"
        );
        for (candidate, error) in [
            (finalized_hash, "cannot prune the finalized PBT checkpoint"),
            (
                sibling.block().hash(),
                "PBT prune candidates must be strict ancestors of the finalized block",
            ),
        ] {
            let root = store.load_block(candidate)?.root();
            let startup = Startup { obsolete_checkpoints: vec![candidate], ..Default::default() };
            assert_eq!(
                startup.initialize(&strategy, &node.inner.provider).unwrap_err().to_string(),
                error
            );
            assert_eq!(store.load_block(candidate)?.root(), root);
        }
        let canonical = store.load_block(finalized_hash)?;
        let mut forged = canonical.clone();
        forged.set_account(
            Address::repeat_byte(0x88),
            AccountData { balance: U256::ONE, ..Default::default() },
        )?;
        store.save_block(finalized_hash, &forged)?;
        let prune = Startup { obsolete_checkpoints: vec![genesis], ..Default::default() };
        assert_eq!(
            prune.initialize(&strategy, &node.inner.provider).unwrap_err().to_string(),
            "PBT finalized checkpoint reference does not match its header"
        );
        assert!(store.path().join("blocks").join(genesis.to_string()).try_exists()?);
        store.save_block(finalized_hash, &canonical)?;
        prune.initialize(&strategy, &node.inner.provider)?;
        assert!(!store.path().join("blocks").join(genesis.to_string()).try_exists()?);
        assert_eq!(store.load_block(finalized_hash)?.root(), finalized[0].block().state_root);
        assert_eq!(store.load_block(sibling.block().hash())?.root(), sibling.block().state_root);
        Ok(())
    }

    #[test]
    fn onboarding_and_pruning_require_local_finality() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let strategy = PbtStateRootStrategy::new(store, 1);
        let provider = NoopProvider::default();
        for startup in [
            Startup {
                migration: Some(MigrationFiles {
                    source: MigrationSource::Snapshot("snapshot".into()),
                    preimages: "preimages".into(),
                    anchor: B256::ZERO,
                }),
                ..Default::default()
            },
            Startup { obsolete_checkpoints: vec![B256::repeat_byte(1)], ..Default::default() },
        ] {
            assert_eq!(
                startup.initialize(&strategy, &provider).unwrap_err().to_string(),
                "PBT onboarding or pruning requires a locally finalized block"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn checkpoint_proofs_are_exposed_only_on_authenticated_rpc() -> eyre::Result<()> {
        let (mut node, store, _directory) = setup(0).await?;
        let recipient = Address::repeat_byte(0x77);
        let mut sender = Wallet::new(1).account(0);
        let transaction = sender.transfer(recipient, U256::from(9)).await;
        let mined = node.mine([transaction]).await?.ensure_success()?;
        let state = store.load_block(mined.block().hash())?;
        let key = Bytes::copy_from_slice(&key::header_key::<Blake3Hasher>(
            recipient,
            key::BASIC_DATA_LEAF_KEY,
        ));
        let response: serde_json::Value = node
            .inner
            .auth_server_handle()
            .http_client()
            .request("pbt_getProof", rpc_params![mined.block().hash(), vec![key.clone()]])
            .await?;
        assert_eq!(response["version"], 1);
        assert_eq!(
            serde_json::from_value::<B256>(response["blockHash"].clone())?,
            mined.block().hash()
        );
        assert_eq!(
            serde_json::from_value::<B256>(response["pbtStateRoot"].clone())?,
            mined.block().state_root
        );
        assert_eq!(serde_json::from_value::<Bytes>(response["proofs"][0]["key"].clone())?, key);
        assert_eq!(
            serde_json::from_value::<B256>(response["proofs"][0]["value"].clone())?,
            state.account(recipient).unwrap().basic_data()?
        );
        let public = node.rpc_client().ok_or_else(|| eyre::eyre!("HTTP RPC is not enabled"))?;
        let error = public
            .request::<serde_json::Value, _>(
                "pbt_getProof",
                rpc_params![mined.block().hash(), vec![key]],
            )
            .await
            .unwrap_err();
        let jsonrpsee::core::client::Error::Call(error) = error else {
            eyre::bail!("public RPC returned a transport error instead of method-not-found");
        };
        assert_eq!(error.code(), jsonrpsee::types::error::METHOD_NOT_FOUND_CODE);
        assert_eq!(error.message(), "Method not found");
        assert!(error.data().is_none());
        Ok(())
    }

    #[test]
    fn directory_lock_excludes_concurrent_launches_and_releases_on_drop() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let lock = lock_directory(directory.path())?;
        assert_eq!(
            lock_directory(directory.path()).unwrap_err().to_string(),
            "PBT data directory is already in use"
        );
        drop(lock);
        let reopened = lock_directory(directory.path())?;
        assert_eq!(
            lock_directory(directory.path()).unwrap_err().to_string(),
            "PBT data directory is already in use"
        );
        drop(reopened);
        Ok(())
    }

    #[test]
    fn populated_checkpoint_directory_requires_configuration_marker() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SnapshotStore::open(directory.path(), DecodeLimits::default())?;
        let chain = (*test_chain_spec(EthereumHardfork::Cancun)).clone();
        let prepared = prepare_chain(chain.clone(), 1, &store)?;
        let root = store.load_block(prepared.genesis_hash())?.root();
        reth_fs_util::remove_file(store.path().join("chain-config"))?;
        assert_eq!(
            prepare_chain(chain, 2, &store).unwrap_err().to_string(),
            "PBT checkpoint directory is populated but chain-config is missing"
        );
        assert!(!store.path().join("chain-config").try_exists()?);
        assert_eq!(store.load_block(prepared.genesis_hash())?.root(), root);
        Ok(())
    }

    fn catchup_provider(
        anchor: &SealedHeader,
        tip: &SealedHeader,
        bytes: Option<Bytes>,
    ) -> eyre::Result<MockEthProvider> {
        let mut provider = MockEthProvider::new();
        provider.bal_store = BalStoreHandle::new(InMemoryBalStore::default());
        provider.add_header(anchor.hash(), anchor.clone().into_header());
        provider.add_header(tip.hash(), tip.clone().into_header());
        if let Some(bytes) = bytes {
            provider.bal_store().insert(tip.num_hash(), RawBal::new(bytes))?;
        }
        Ok(provider)
    }

    #[tokio::test]
    async fn local_catchup_rejects_missing_tampered_and_over_budget_inputs() -> eyre::Result<()> {
        let (mut source, source_store, _source_directory) =
            setup_for(0, EthereumHardfork::Amsterdam).await?;
        let anchor = source.inner.provider.sealed_header(0)?.unwrap();
        let mut sender = Wallet::new(1).account(0);
        let recipient = Address::repeat_byte(0x77);
        let transaction = sender.transfer(recipient, U256::from(9)).await;
        let first = source.mine([transaction]).await?.ensure_success()?;
        let tip = source.inner.provider.sealed_header_by_hash(first.block().hash())?.unwrap();
        let bytes = first.payload.block_access_list().unwrap().clone();
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        store.save_block(anchor.hash(), &source_store.load_block(anchor.hash())?)?;
        let strategy = PbtStateRootStrategy::new(store.clone(), 0);
        let startup = Startup::default();
        let missing_header = catchup_provider(&anchor, &tip, Some(bytes.clone()))?;
        assert!(missing_header.headers.lock().remove(&anchor.hash()).is_some());
        assert_eq!(
            startup.catch_up(&strategy, &missing_header, &anchor, &tip).unwrap_err().to_string(),
            "PBT catch-up header is unavailable"
        );
        let missing_bal = catchup_provider(&anchor, &tip, None)?;
        assert_eq!(
            startup.catch_up(&strategy, &missing_bal, &anchor, &tip).unwrap_err().to_string(),
            "PBT catch-up BAL is unavailable"
        );
        let short_budget = Startup {
            catchup: CatchupLimits {
                max_bal_bytes: NonZeroUsize::new(bytes.len() - 1).unwrap(),
                ..Default::default()
            },
            ..Default::default()
        };
        let valid = catchup_provider(&anchor, &tip, Some(bytes.clone()))?;
        assert_eq!(
            short_budget.catch_up(&strategy, &valid, &anchor, &tip).unwrap_err().to_string(),
            "PBT catch-up BAL byte budget exceeded"
        );
        let corrupted = Bytes::from_static(&[0xc0]);
        let tampered = catchup_provider(&anchor, &tip, Some(corrupted.clone()))?;
        let error = startup.catch_up(&strategy, &tampered, &anchor, &tip).unwrap_err();
        let error = error.downcast_ref::<alloy_eip7928::BlockAccessListHashMismatch>().unwrap();
        assert_eq!(error.computed, alloy_primitives::keccak256(corrupted));
        assert_eq!(error.expected, tip.block_access_list_hash().unwrap());
        let mut bad_root = tip.clone().into_header();
        bad_root.state_root = B256::repeat_byte(0xff);
        let bad_root = SealedHeader::seal_slow(bad_root);
        let invalid = catchup_provider(&anchor, &bad_root, Some(bytes.clone()))?;
        let error = startup.catch_up(&strategy, &invalid, &anchor, &bad_root).unwrap_err();
        let reth_stages::StageError::Block { block, error } =
            error.downcast_ref::<reth_stages::StageError>().unwrap()
        else {
            panic!("expected block validation error")
        };
        assert_eq!(block.block.hash, bad_root.hash());
        let reth_stages::BlockErrorKind::Validation(
            reth_ethereum::consensus::ConsensusError::BodyStateRootDiff(roots),
        ) = error
        else {
            panic!("expected state root mismatch")
        };
        assert_eq!(roots.got, tip.state_root());
        assert_eq!(roots.expected, bad_root.state_root());
        assert!(!store.path().join("blocks").join(bad_root.hash().to_string()).try_exists()?);
        let mut no_commitment = tip.clone().into_header();
        no_commitment.block_access_list_hash = None;
        let no_commitment = SealedHeader::seal_slow(no_commitment);
        let invalid = catchup_provider(&anchor, &no_commitment, None)?;
        assert_eq!(
            startup.catch_up(&strategy, &invalid, &anchor, &no_commitment).unwrap_err().to_string(),
            "PBT catch-up header has no BAL commitment"
        );
        let next = source.advance_block().await?;
        let next = source.inner.provider.sealed_header_by_hash(next.block().hash())?.unwrap();
        valid.add_header(next.hash(), next.clone().into_header());
        let short_chain = Startup {
            catchup: CatchupLimits {
                max_blocks: NonZeroUsize::new(1).unwrap(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            short_chain.catch_up(&strategy, &valid, &anchor, &next).unwrap_err().to_string(),
            "PBT catch-up block budget exceeded"
        );
        assert!(!store.path().join("blocks").join(tip.hash().to_string()).try_exists()?);
        startup.catch_up(&strategy, &valid, &anchor, &tip)?;
        assert_eq!(store.load_block(tip.hash())?.root(), tip.state_root());
        assert_eq!(
            store.load_block(tip.hash())?.account(recipient).unwrap().balance,
            U256::from(9)
        );
        Ok(())
    }

    #[test]
    fn catchup_budgets_are_nonzero_operator_limits() {
        let defaults = CatchupLimits::new(None, None).unwrap();
        assert_eq!(defaults.max_blocks.get(), 1024);
        assert_eq!(defaults.max_bal_bytes.get(), 64 * 1024 * 1024);
        let configured = CatchupLimits::new(Some("3".into()), Some("4096".into())).unwrap();
        assert_eq!(configured.max_blocks.get(), 3);
        assert_eq!(configured.max_bal_bytes.get(), 4096);
        for (blocks, bytes) in [(Some("0".into()), None), (None, Some("0".into()))] {
            let error = CatchupLimits::new(blocks, bytes).unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::num::ParseIntError>().unwrap().kind(),
                &std::num::IntErrorKind::Zero
            );
        }
    }

    #[test]
    fn migration_anchor_requires_finalized_ancestry() -> eyre::Result<()> {
        let chain = test_chain_spec(EthereumHardfork::Amsterdam);
        let anchor = SealedHeader::seal_slow(chain.genesis_header().clone());
        let mut child = anchor.clone().into_header();
        child.parent_hash = anchor.hash();
        child.number += 1;
        child.timestamp += 1;
        let child = SealedHeader::seal_slow(child);
        let mut sibling = anchor.clone().into_header();
        sibling.extra_data = Bytes::from_static(b"sibling");
        let sibling = SealedHeader::seal_slow(sibling);
        let files = MigrationFiles {
            source: MigrationSource::Snapshot("snapshot".into()),
            preimages: "preimages".into(),
            anchor: anchor.hash(),
        };
        let provider = catchup_provider(&anchor, &child, None)?;
        assert_eq!(files.anchor_header(&provider, &anchor)?.hash(), anchor.hash());
        assert_eq!(files.anchor_header(&provider, &child)?.hash(), anchor.hash());
        assert_eq!(
            MigrationFiles { anchor: child.hash(), ..files.clone() }
                .anchor_header(&provider, &anchor)
                .unwrap_err()
                .to_string(),
            "PBT migration anchor is not finalized"
        );
        assert_eq!(
            MigrationFiles { anchor: B256::repeat_byte(1), ..files.clone() }
                .anchor_header(&provider, &child)
                .unwrap_err()
                .to_string(),
            "PBT migration anchor header is unavailable"
        );
        provider.add_header(sibling.hash(), sibling.clone().into_header());
        assert_eq!(
            MigrationFiles { anchor: sibling.hash(), ..files.clone() }
                .anchor_header(&provider, &child)
                .unwrap_err()
                .to_string(),
            "PBT migration anchor is not an ancestor of the finalized block"
        );
        let mut grandchild = child.clone().into_header();
        grandchild.parent_hash = child.hash();
        grandchild.number += 1;
        grandchild.timestamp += 1;
        let grandchild = SealedHeader::seal_slow(grandchild);
        assert!(provider.headers.lock().remove(&child.hash()).is_some());
        assert_eq!(
            files.anchor_header(&provider, &grandchild).unwrap_err().to_string(),
            "PBT migration finalized ancestry is unavailable"
        );
        let mut invalid = child.into_header();
        invalid.number += 1;
        let invalid = SealedHeader::seal_slow(invalid);
        assert_eq!(
            files.anchor_header(&provider, &invalid).unwrap_err().to_string(),
            "inconsistent PBT migration finalized ancestry"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_bals_are_authenticated_before_retention() -> eyre::Result<()> {
        let (mut source, source_store, _source_directory) =
            setup_for(0, EthereumHardfork::Amsterdam).await?;
        let anchor = source.inner.provider.sealed_header(0)?.unwrap();
        let mut sender = Wallet::new(1).account(0);
        let transaction = sender.transfer(Address::repeat_byte(0x77), U256::from(9)).await;
        let first = source.mine([transaction]).await?.ensure_success()?;
        let tip = source.inner.provider.sealed_header_by_hash(first.block().hash())?.unwrap();
        let bytes = first.payload.block_access_list().unwrap().clone();
        let provider = catchup_provider(&anchor, &tip, None)?;
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        store.save_block(anchor.hash(), &source_store.load_block(anchor.hash())?)?;
        let strategy = PbtStateRootStrategy::new(store.clone(), 0);
        let startup = Startup::default();
        let error = startup
            .catch_up_with_bal_fetch(&strategy, &provider, &anchor, &tip, |_, _, _| {
                eyre::bail!("peer unavailable")
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "peer unavailable");
        assert!(provider.bal_store().get_by_hash(tip.hash())?.is_none());
        assert!(!store.path().join("blocks").join(tip.hash().to_string()).try_exists()?);
        let error = startup
            .catch_up_with_bal_fetch(&strategy, &provider, &anchor, &tip, |_, _, _| {
                Ok(Bytes::from_static(&[0xc0]))
            })
            .unwrap_err();
        let mismatch = error.downcast_ref::<alloy_eip7928::BlockAccessListHashMismatch>().unwrap();
        assert_eq!(mismatch.expected, tip.block_access_list_hash().unwrap());
        assert_eq!(mismatch.computed, alloy_primitives::keccak256([0xc0]));
        assert!(provider.bal_store().get_by_hash(tip.hash())?.is_none());
        assert!(!store.path().join("blocks").join(tip.hash().to_string()).try_exists()?);
        startup.catch_up_with_bal_fetch(
            &strategy,
            &provider,
            &anchor,
            &tip,
            |hash, commitment, budget| {
                assert_eq!(hash, tip.hash());
                assert_eq!(commitment, tip.block_access_list_hash().unwrap());
                assert_eq!(budget, startup.catchup.max_bal_bytes.get());
                Ok(bytes.clone())
            },
        )?;
        assert_eq!(provider.bal_store().get_by_hash(tip.hash())?, Some(bytes));
        assert_eq!(store.load_block(tip.hash())?.root(), tip.state_root());
        startup.catch_up_with_bal_fetch(&strategy, &provider, &anchor, &tip, |_, _, _| {
            panic!("locally retained BAL must not be downloaded twice")
        })?;
        Ok(())
    }

    #[test]
    fn startup_checks_completed_state_when_headers_are_ahead() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SnapshotStore::open(directory.path(), DecodeLimits::default())?);
        let mut state = StateTree::new();
        state.set_account(Address::ZERO, AccountData { nonce: 1, ..Default::default() })?;
        let chain = test_chain_spec(EthereumHardfork::Amsterdam);
        let mut header = chain.genesis_header().clone();
        header.state_root = state.root();
        let header = SealedHeader::seal_slow(header);
        store.save_block_with_number(header.hash(), header.number(), &state)?;
        let mut next = header.clone().into_header();
        next.parent_hash = header.hash();
        next.number += 1;
        next.timestamp += 1;
        next.state_root = B256::repeat_byte(0xff);
        let next = SealedHeader::seal_slow(next);
        let provider = MockEthProvider::default();
        provider.add_header(header.hash(), header.into_header());
        provider.add_header(next.hash(), next.into_header());
        provider.add_stage_checkpoint(
            reth_stages::StageId::Finish,
            reth_stages::StageCheckpoint::new(0),
        );
        Startup::default().initialize(&PbtStateRootStrategy::new(store, 0), &provider)?;
        Ok(())
    }

    #[test]
    fn chain_configuration_preserves_normal_chains_and_tracks_the_shared_fork() {
        let ordinary = test_chain_spec(EthereumHardfork::Amsterdam);
        assert!(Arc::ptr_eq(&configure_chain(ordinary.clone()).unwrap(), &ordinary));
        let timestamp = ordinary.genesis_timestamp() + 3;
        let mut chain = (*ordinary).clone();
        chain
            .genesis
            .config
            .extra_fields
            .insert("binaryTrieTime".into(), serde_json::json!(timestamp));
        let configured = configure_chain(Arc::new(chain)).unwrap();
        assert_eq!(configured.genesis_hash(), ordinary.genesis_hash());
        let before = configured.fork_id(&Head { timestamp: timestamp - 1, ..Default::default() });
        let after = configured.fork_id(&Head { timestamp, ..Default::default() });
        assert_eq!(before.next, timestamp);
        assert_eq!(after.next, 0);
        assert_ne!(before.hash, after.hash);
        assert_eq!(activation_timestamp(&configured).unwrap(), Some(timestamp));
        assert_eq!(
            configure_chain(configured.clone()).unwrap().genesis_hash(),
            configured.genesis_hash()
        );
    }

    #[test]
    fn pbt_genesis_matches_independent_geth_state_root() {
        let mut genesis = test_chain_spec(EthereumHardfork::Amsterdam).genesis.clone();
        genesis.alloc =
            serde_json::from_str(include_str!("../testdata/geth-anchor-alloc.json")).unwrap();
        genesis.config.extra_fields.insert("binaryTrieTime".into(), serde_json::json!(0));
        let ordinary = Arc::new(ChainSpec::from(genesis));
        let configured = configure_chain(ordinary.clone()).unwrap();
        let expected: B256 =
            "0xd7936293b851534fa6af126bdc2f5db471069aeea9b614856e0e1e871524522e".parse().unwrap();
        assert_eq!(configured.genesis_header().state_root, expected);
        assert_ne!(configured.genesis_hash(), ordinary.genesis_hash());
        assert_eq!(
            configure_chain(configured.clone()).unwrap().genesis_hash(),
            configured.genesis_hash()
        );
    }

    #[test]
    fn absent_null_and_invalid_activation_are_distinguished() {
        let mut chain = (*test_chain_spec(EthereumHardfork::Amsterdam)).clone();
        assert_eq!(activation_timestamp(&chain).unwrap(), None);
        chain.genesis.config.extra_fields.insert("binaryTrieTime".into(), serde_json::Value::Null);
        assert_eq!(activation_timestamp(&chain).unwrap(), None);
        chain
            .genesis
            .config
            .extra_fields
            .insert("binaryTrieTime".into(), serde_json::json!("invalid"));
        assert_eq!(
            activation_timestamp(&chain).unwrap_err().to_string(),
            "invalid type: string \"invalid\", expected u64"
        );
    }

    #[tokio::test]
    async fn shared_node_components_build_and_validate_across_activation() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        for from_genesis in [true, false] {
            let mut chain = (*test_chain_spec(EthereumHardfork::Amsterdam)).clone();
            let activation = if from_genesis { 0 } else { chain.genesis_timestamp() + 2 };
            chain
                .genesis
                .config
                .extra_fields
                .insert("binaryTrieTime".into(), serde_json::json!(activation));
            let directory = tempfile::tempdir()?;
            let pbt =
                PbtNode::new(configure_chain(Arc::new(chain))?, directory.path(), activation)?;
            let chain = pbt.chain_spec();
            let config = test_node_config(chain.clone())
                .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
            let methods = pbt.rpc_module()?;
            let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(config)
                .testing_node(Runtime::test())
                .with_types::<EthereumNode>()
                .with_components(EthereumNode::components())
                .with_add_ons(pbt.add_ons())
                .extend_rpc_modules(move |ctx| {
                    ctx.modules.remove_method_from_configured("eth_getProof");
                    ctx.auth_module.remove_auth_method("eth_getProof");
                    ctx.auth_module.merge_auth_methods(methods)?;
                    Ok(())
                })
                .launch_with_debug_capabilities_and_backfill(pbt.backfill())
                .await?;
            let mut node = NodeTestContext::new(node, move |timestamp| {
                eth_payload_attributes(&*chain, timestamp)
            })
            .await?;
            node.set_finality(Finality::Keep);
            let mut sender = Wallet::new(1).account(0);
            let recipient = Address::repeat_byte(0x75);
            let tx = sender.transfer(recipient, U256::from(17)).await;
            node.mine([tx]).await?.ensure_success()?;
            let mined = node.advance_blocks(2).await?;
            let tip = mined.last().unwrap().block();
            let state = pbt.store.load_block(tip.hash())?;
            assert_eq!(state.root(), tip.state_root);
            assert_eq!(state.account(recipient).unwrap().balance, U256::from(17));
        }
        Ok(())
    }

    #[test]
    fn resource_budgets_reject_invalid_values() {
        assert_eq!(parse_resource_budget("PBT_CACHE_MAX_BYTES", "0".into()).unwrap(), 0);
        assert_eq!(
            parse_resource_budget("PBT_IMPORT_MAX_LEAVES", "1048576".into()).unwrap(),
            1_048_576
        );
        for value in ["-1", "many", "184467440737095516160"] {
            assert_eq!(
                parse_resource_budget("PBT_IMPORT_MAX_LEAVES", value.into())
                    .unwrap_err()
                    .to_string(),
                "PBT_IMPORT_MAX_LEAVES must be a nonnegative integer"
            );
        }
    }

    #[tokio::test]
    async fn pbt_rejects_mpt_simulation_root_configuration() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("pbt");
        let store = SnapshotStore::open(&path, DecodeLimits::default())?;
        let chain =
            prepare_chain((*test_chain_spec(EthereumHardfork::Amsterdam)).clone(), 0, &store)?;
        let result = E2ETestSetupBuilder::<RestartNode>::new(1, chain)
            .with_node(move |_| RestartNode { checkpoints: path.clone(), ..Default::default() })
            .with_node_config_modifier(|mut config| {
                config.rpc.rpc_compute_state_root_for_eth_simulate = true;
                config
            })
            .build_single()
            .await;
        let error = match result {
            Ok(_) => eyre::bail!("PBT accepted MPT simulation roots"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "--rpc.compute-state-root-for-eth-simulate is unsupported with PBT"
        );
        Ok(())
    }

    #[tokio::test]
    async fn finalized_cleanup_keeps_history_and_retirement_checkpoint() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("pbt");
        let store = Arc::new(SnapshotStore::open(&path, DecodeLimits::default())?);
        let chain =
            prepare_chain((*test_chain_spec(EthereumHardfork::Amsterdam)).clone(), 0, &store)?;
        let backfill_path = path.clone();
        let (mut node, _) = E2ETestSetupBuilder::<RestartNode>::new(1, chain)
            .with_node(move |_| RestartNode {
                checkpoints: path.clone(),
                startup: Startup { keep_mpt: true, ..Default::default() },
                ..Default::default()
            })
            .with_tree_config_modifier(|config| {
                config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
            })
            .with_backfill(move |_| {
                PbtBackfill::new(
                    Arc::new(PbtStateRootStrategy::new(
                        Arc::new(
                            SnapshotStore::open(&backfill_path, DecodeLimits::default()).unwrap(),
                        ),
                        0,
                    )),
                    CatchupLimits::default(),
                )
            })
            .build_single()
            .await?;
        node.set_finality(Finality::Keep);
        let genesis = node.block_hash(0);
        let canonical = node.advance_blocks(4).await?;
        let fork = node.advance_fork(canonical[0].block().hash(), 2).await?;
        node.reorg_to(canonical[3].block().hash()).await?;
        let unknown = B256::repeat_byte(0xab);
        let state = store.load_block(canonical[0].block().hash())?;
        store.save_block_with_number(unknown, 1, &state)?;
        let mut orphan = state.clone();
        orphan.set_account(
            Address::repeat_byte(0xac),
            AccountData { nonce: 7, ..Default::default() },
        )?;
        store.save(&orphan)?;
        node.set_finality(Finality::Head);
        let finalized = node.advance_block().await?;
        node.wait_for_persisted_state(finalized.block().number()).await?;
        let strategy = PbtStateRootStrategy::new(store.clone(), 0);
        assert_eq!(
            retirement::prune_finalized(&strategy, &node.inner.provider, None, 1)?,
            Some(finalized.block().hash())
        );
        assert!(!store.contains_block(canonical[2].block().hash())?);
        assert!(!store.contains_block(fork[1].block().hash())?);
        assert!(!store.contains_block(unknown)?);
        assert!(store.contains_block(canonical[3].block().hash())?);
        assert!(store.contains_block(genesis)?);
        assert!(matches!(store.load(orphan.root()), Err(StoreError::Io(_))));
        let verifying_store = Arc::new(SnapshotStore::<RetirementHasher>::open(
            store.path(),
            DecodeLimits::default(),
        )?);
        let verifying_strategy = PbtStateRootStrategy::new(verifying_store, 0);
        let provider = node.inner.provider.clone();
        let (started_sender, started_receiver) = mpsc::channel();
        let (resume_sender, resume_receiver) = mpsc::channel();
        let verifier = tokio::task::spawn_blocking(move || {
            RETIREMENT_PAUSE.with(|pause| {
                *pause.borrow_mut() = Some(Box::new(move || {
                    started_sender.send(()).unwrap();
                    resume_receiver.recv_timeout(Duration::from_secs(20)).unwrap();
                }))
            });
            retirement::try_retire(&verifying_strategy, &provider)
        });
        started_receiver.recv_timeout(Duration::from_secs(10))?;
        let advanced = tokio::time::timeout(Duration::from_secs(10), async {
            let block = node.advance_block().await?;
            node.wait_for_persisted_state(block.block().number()).await?;
            Ok::<_, eyre::Report>(block)
        })
        .await;
        resume_sender.send(())?;
        let retired = verifier.await??;
        let advanced = advanced??;
        assert!(advanced.block().number() > finalized.block().number());
        assert!(retired);
        assert_eq!(
            node.inner.provider.database_provider_ro()?.mpt_retired_at()?,
            Some(finalized.block().number())
        );
        let tip = node.advance_blocks(2).await?;
        let tip = tip.last().unwrap().block();
        node.wait_for_persisted_state(tip.number()).await?;
        retirement::prune_finalized(&strategy, &node.inner.provider, None, 0)?;
        assert!(store.contains_block(finalized.block().hash())?);
        assert!(store.contains_block(genesis)?);
        retirement::validate_startup(&strategy, &node.inner.provider)?;
        Ok(())
    }

    #[tokio::test]
    async fn mpt_retirement_preserves_finality_reorgs_and_restart() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();
        for keep_mpt in [true, false] {
            let directory = tempfile::tempdir()?;
            let checkpoints = directory.path().join("pbt");
            let store = Arc::new(SnapshotStore::open(&checkpoints, DecodeLimits::default())?);
            let activation = 1_710_338_138;
            let chain = prepare_chain(
                (*test_chain_spec(EthereumHardfork::Amsterdam)).clone(),
                activation,
                &store,
            )?;
            let node_checkpoints = checkpoints.clone();
            let backfill_checkpoints = checkpoints.clone();
            let (mut node, _) = E2ETestSetupBuilder::<RestartNode>::new(1, chain)
                .with_node(move |_| RestartNode {
                    checkpoints: node_checkpoints.clone(),
                    activation,
                    startup: Startup { keep_mpt, ..Default::default() },
                })
                .with_restartable_nodes()
                .with_tree_config_modifier(|config| {
                    config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
                })
                .with_backfill(move |_| {
                    PbtBackfill::new(
                        Arc::new(PbtStateRootStrategy::new(
                            Arc::new(
                                SnapshotStore::open(&backfill_checkpoints, DecodeLimits::default())
                                    .unwrap(),
                            ),
                            activation,
                        )),
                        CatchupLimits::default(),
                    )
                })
                .build_single()
                .await?;
            node.set_finality(Finality::Keep);
            node.set_next_payload_timestamp(activation - 2)?;
            let mut sender = Wallet::new(1).account(0);
            let recipient = Address::repeat_byte(0x59);
            let transaction = sender.transfer(recipient, U256::from(9)).await;
            let before = node.mine([transaction]).await?.ensure_success()?;
            assert!(before.block().timestamp < activation);
            let blocks = node.advance_blocks(3).await?;
            let boundary = blocks.last().unwrap().block();
            let finalized = blocks[1].block();
            assert!(boundary.timestamp >= activation);
            node.wait_for_persisted_block(boundary.number).await?;
            let strategy = PbtStateRootStrategy::new(store.clone(), activation);
            assert!(!retirement::try_retire(&strategy, &node.inner.provider)?);
            assert_eq!(node.inner.provider.database_provider_ro()?.mpt_retired_at()?, None);
            assert!(
                node.inner
                    .provider
                    .database_provider_ro()?
                    .tx_ref()
                    .entries::<tables::AccountsTrie>()? >
                    0
            );
            let status = node
                .engine
                .forkchoice_updated(ForkchoiceState {
                    head_block_hash: boundary.hash(),
                    safe_block_hash: boundary.hash(),
                    finalized_block_hash: finalized.hash(),
                })
                .await?;
            assert!(status.payload_status.is_valid());
            if keep_mpt {
                reth_e2e_test_utils::wait::assert_holds_for(
                    Duration::from_millis(1200),
                    "retained MPT for archive use",
                    || async {
                        Ok(node.inner.provider.database_provider_ro()?.mpt_retired_at()?.is_none())
                    },
                )
                .await?;
                let marker = checkpoints.join("states").join(boundary.state_root.to_string());
                let original = reth_fs_util::read(&marker)?;
                let mut corrupt = original.clone();
                corrupt[11] = u8::MAX;
                reth_fs_util::write(&marker, corrupt)?;
                let error = retirement::try_retire(&strategy, &node.inner.provider).unwrap_err();
                assert!(matches!(
                    error.downcast_ref::<StoreError>(),
                    Some(StoreError::Artifact(ArtifactError::Invalid(
                        "invalid PBT checkpoint marker"
                    )))
                ));
                assert_eq!(node.inner.provider.database_provider_ro()?.mpt_retired_at()?, None);
                reth_fs_util::write(&marker, original)?;
                let provider = node.inner.provider.database_provider_rw()?;
                let original =
                    provider.get_stage_checkpoint(reth_stages::StageId::Finish)?.unwrap();
                provider.save_stage_checkpoint(
                    reth_stages::StageId::Finish,
                    reth_stages::StageCheckpoint::new(original.block_number)
                        .with_finish_stage_checkpoint(reth_stages::FinishCheckpoint {
                            partial_state_trie: Some(finalized.number - 1),
                        }),
                )?;
                provider.commit()?;
                assert!(!retirement::try_retire(&strategy, &node.inner.provider)?);
                let provider = node.inner.provider.database_provider_rw()?;
                provider.save_stage_checkpoint(reth_stages::StageId::Finish, original)?;
                provider.commit()?;
            } else {
                reth_e2e_test_utils::wait::poll_until(
                    "MPT retirement after finalized PBT activation",
                    || async { Ok(node.inner.provider.database_provider_ro()?.mpt_retired_at()?) },
                )
                .await?;
            }
            let provider = node.inner.provider.database_provider_ro()?;
            if keep_mpt {
                assert!(provider.tx_ref().entries::<tables::AccountsTrie>()? > 0);
            } else {
                assert_eq!(provider.mpt_retired_at()?, Some(finalized.number));
                assert_eq!(provider.tx_ref().entries::<tables::AccountsTrie>()?, 0);
                assert_eq!(provider.tx_ref().entries::<tables::StoragesTrie>()?, 0);
                assert!(provider.tx_ref().entries::<tables::HashedAccounts>()? > 0);
                assert_eq!(
                    strategy
                        .validate_retired_parent(before.block().header(), Some(finalized.number),)
                        .unwrap_err()
                        .to_string(),
                    "cannot unwind past MPT retirement; re-anchor or resync"
                );
            }
            drop(provider);
            let transaction = sender.transfer(recipient, U256::from(5)).await;
            let canonical = node.mine([transaction]).await?.ensure_success()?;
            assert_eq!(
                store.load_block(canonical.block().hash())?.account(recipient).unwrap().balance,
                U256::from(14)
            );
            let fork = node.advance_fork(boundary.hash(), 2).await?;
            node.reorg_to(canonical.block().hash()).await?;
            node.reorg_to(fork[1].block().hash()).await?;
            let mut node = node.restart().await?;
            node.set_finality(Finality::Keep);
            assert_eq!(
                node.inner.provider.latest()?.basic_account(&recipient)?.unwrap().balance,
                store.load_block(fork[1].block().hash())?.account(recipient).unwrap().balance,
            );
            let next = node.advance_block().await?;
            assert_eq!(store.verify_block(next.block().hash())?, next.block().state_root);
            let provider = node.inner.provider.database_provider_ro()?;
            assert_eq!(provider.mpt_retired_at()?.is_some(), !keep_mpt);
            if !keep_mpt {
                assert_eq!(provider.tx_ref().entries::<tables::AccountsTrie>()?, 0);
                assert_eq!(provider.tx_ref().entries::<tables::StoragesTrie>()?, 0);
            }
        }
        Ok(())
    }
}
