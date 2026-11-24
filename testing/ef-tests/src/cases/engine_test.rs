//! Test runner for blockchain tests in the engine format.
//!
//! Each fixture gets a fresh temporary database with its genesis, and the engine stack a node
//! builds on it: the engine tree with [`BasicEngineValidator`] and its persistence service, and
//! the [`EngineApi`] handler. The fixture's parameters go, unchanged, through the handler's
//! JSON-RPC method table for `engine_newPayloadV<n>` and `engine_forkchoiceUpdatedV<n>` at the
//! versions the fixture names, so the handler's own parameter decoding and version checks run.
//! No node, RPC server or network is started.

use crate::{
    case::{load_json, run_workers},
    models::{EngineNewPayload, EngineTest},
    result::{FixtureResult, Rejection},
    Error,
};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ClientCode, ClientVersionV1};
use futures::StreamExt;
use jsonrpsee::{
    types::{Response, ResponsePayload},
    RpcModule,
};
use reth_chain_state::{CanonicalInMemoryStateWriter, InMemoryState};
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db::{
    init_db,
    mdbx::{DatabaseArguments, SyncMode},
    DatabaseEnv,
};
use reth_db_common::init::init_genesis_with_settings;
use reth_engine_primitives::{ConsensusEngineHandle, NoopInvalidBlockHook};
use reth_engine_tree::{
    chain::{ChainEvent, FromOrchestrator},
    engine::{EngineApiKind, EngineRequestHandler},
    launch::EngineOrchestratorBuilder,
    tree::{BasicEngineValidator, TreeConfig},
};
use reth_eth_wire_types::EthNetworkPrimitives;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_evm_ethereum::EthEvmConfig;
use reth_network_api::noop::NoopNetwork;
use reth_network_p2p::full_block::NoopFullBlockClient;
use reth_node_core::args::EngineArgs;
use reth_node_ethereum::{EthEngineTypes, EthereumEngineValidator, EthereumNode};
use reth_node_types::NodeTypesWithDBAdapter;
use reth_payload_builder::{PayloadBuilderHandle, PayloadStore};
use reth_provider::{
    providers::{BlockchainProvider, RocksDBBuilder, RocksDBProvider, StaticFileProvider},
    BlockNumReader, ProviderFactory, StorageSettings,
};
use reth_prune::{PruneModes, PrunerBuilder};
use reth_rpc_api::EngineApiServer;
use reth_rpc_engine_api::{EngineApi, EngineCapabilities};
use reth_stages_api::Pipeline;
use reth_static_file::StaticFileProducer;
use reth_storage_overlay::OverlayManager;
use reth_tasks::Runtime;
use reth_transaction_pool::noop::NoopTransactionPool;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The cross-block cache size of each fixture's engine, in MB (`--engine.cross-block-cache-size`).
const CROSS_BLOCK_CACHE_SIZE_MB: usize = 1;

/// BAL prewarm workers per fixture engine; production nodes use their normal pool size.
const BAL_PREWARM_THREADS: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// The engine tree's persistence threshold (`--engine.persistence-threshold`): more blocks than a
/// fixture has, so the tree persists them only when it terminates.
const PERSISTENCE_THRESHOLD: u64 = 1_000_000;

/// How long a fixture's engine tree gets to persist its blocks and stop.
const ENGINE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum number of fixture runtimes waiting for background database handles per worker.
const MAX_PENDING_ENGINES: usize = 8;

/// Bound requests so a stopped or stuck engine cannot hang the whole fixture set.
const ENGINE_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs every fixture in the JSON `files` on `workers` threads and passes one result per
/// fixture to `on_result` as it completes. A file that fails to load is reported as an error,
/// without creating a fixture result. Each datadir is created in the temporary directory
/// (`TMPDIR`), where a tmpfs avoids disk syncs, and its genesis is written with `storage_settings`.
/// Returns an error if the fixture engines cannot be cleaned up completely.
pub fn run_fixtures(
    files: Vec<PathBuf>,
    bal_sequential: bool,
    storage_settings: StorageSettings,
    workers: usize,
    on_result: &(dyn Fn(Result<FixtureResult, Error>) + Sync),
) -> Result<(), Error> {
    let tree_config = tree_config(bal_sequential);
    let datadir_root = std::env::temp_dir();
    let cleanup_error = Mutex::new(None);
    let record_error = |err| {
        let mut first = cleanup_error.lock().unwrap();
        if first.is_none() {
            *first = Some(err);
        } else {
            eprintln!("error: {err}");
        }
    };
    run_workers(files, workers, |files| {
        let mut cleanup = Cleanup::default();
        for path in files {
            let tests = match load_json::<BTreeMap<String, EngineTest>>(&path) {
                Ok(tests) => tests,
                Err(err) => {
                    on_result(Err(err));
                    continue
                }
            };
            for (name, test) in tests {
                let (result, remains) =
                    run_fixture(name, &test, &tree_config, storage_settings, &datadir_root);
                on_result(Ok(result));
                if let Err(err) = cleanup.push(remains) {
                    record_error(err);
                    return
                }
            }
        }
        if let Err(err) = cleanup.finish() {
            record_error(err);
        }
    });
    cleanup_error.into_inner().unwrap().map_or(Ok(()), Err)
}

/// Returns the engine tree configuration of the node with the runner's engine arguments.
fn tree_config(bal_sequential: bool) -> TreeConfig {
    let engine_args = EngineArgs {
        bal_parallel_execution_disabled: bal_sequential,
        // The default 4 GB cache would dominate the memory of concurrent workers.
        cross_block_cache_size: CROSS_BLOCK_CACHE_SIZE_MB,
        // Keep a fixture's blocks in memory instead of persisting them while it runs.
        persistence_threshold: PERSISTENCE_THRESHOLD,
        ..Default::default()
    };
    engine_args.validate().expect("the runner's engine arguments are valid");
    // Fixtures have their own engine; avoid creating the production pool's 128 threads per case.
    engine_args.tree_config().with_bal_prewarm_threads(BAL_PREWARM_THREADS)
}

/// What the engine ended with, reported beside the verdict.
#[derive(Debug, Default)]
struct Outcome {
    last_block_hash: Option<B256>,
    last_payload_status: Option<String>,
    rejections: Vec<Rejection>,
}

/// What a fixture leaves behind: its runtime, on which the engine's background tasks can run
/// for seconds after the tree stopped, its databases and its datadir.
struct Remains {
    runtime: Runtime,
    databases: Option<Databases>,
    datadir: Option<PathBuf>,
}

impl Remains {
    /// Whether the background tasks released the fixture's databases.
    fn is_released(&self) -> bool {
        self.databases.as_ref().is_none_or(Databases::is_released)
    }

    /// Closes the databases, drops the runtime, joining its threads, and tries to remove the
    /// datadir. Returns the datadir if a task still writing to it made the removal fail.
    fn clear(self) -> Option<PathBuf> {
        let Self { runtime, databases, datadir } = self;
        drop(databases);
        drop(runtime);
        datadir.filter(|datadir| fs::remove_dir_all(datadir).is_err())
    }
}

/// A fixture's databases, held by the runner so that they are closed on a worker thread once the
/// engine's background tasks release them.
///
/// A task holding the last provider factory releases the MDBX handle before the `RocksDB` one, and
/// closing `RocksDB` flushes it. Were it closed on that task's thread, it could still be closing
/// while the runner moves on or the process exits, which crashes the runner.
struct Databases {
    mdbx: Arc<DatabaseEnv>,
    rocksdb: RocksDBProvider,
}

impl Databases {
    /// Whether the runner holds the last handle to each database.
    fn is_released(&self) -> bool {
        Arc::strong_count(&self.mdbx) == 1 && self.rocksdb.is_last_handle()
    }
}

/// What a worker still has to clean up.
#[derive(Default)]
struct Cleanup {
    remains: Vec<Remains>,
    datadirs: Vec<PathBuf>,
}

impl Cleanup {
    /// Applies backpressure before pending runtimes or directories can grow beyond the limit.
    fn push(&mut self, remains: Remains) -> Result<(), Error> {
        self.remains.push(remains);
        self.sweep();
        if self.remains.len() >= MAX_PENDING_ENGINES || self.datadirs.len() >= MAX_PENDING_ENGINES {
            std::mem::take(self).finish()?;
        }
        Ok(())
    }

    /// Clears the remains whose databases were released and retries the datadirs that could not be
    /// removed yet.
    fn sweep(&mut self) {
        let (released, pending): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.remains).into_iter().partition(Remains::is_released);
        self.remains = pending;
        self.datadirs.retain(|datadir| fs::remove_dir_all(datadir).is_err());
        self.datadirs.extend(released.into_iter().filter_map(Remains::clear));
    }

    /// Sweeps until everything is cleaned up or [`ENGINE_SHUTDOWN_TIMEOUT`] passes, then clears
    /// what is left and reports the datadirs that could not be removed.
    fn finish(mut self) -> Result<(), Error> {
        let deadline = std::time::Instant::now() + ENGINE_SHUTDOWN_TIMEOUT;
        self.sweep();
        while !(self.remains.is_empty() && self.datadirs.is_empty()) &&
            std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
            self.sweep();
        }
        let unreleased = self.remains.len();
        self.datadirs.extend(self.remains.into_iter().filter_map(Remains::clear));
        for path in self.datadirs {
            fs::remove_dir_all(&path).map_err(|error| Error::Io { path, error })?;
        }
        if unreleased > 0 {
            return Err(Error::Assertion(format!(
                "{unreleased} fixture engines retained database handles after shutdown"
            )))
        }
        Ok(())
    }
}

/// Runs one fixture against a fresh engine on a runtime of its own, and returns what it leaves
/// behind.
///
/// The runtime is per fixture because the engine's background tasks can outlive a fixture's
/// tree by seconds, and on a runtime shared with the next fixtures they slowed those by seconds.
/// The runtime's async tasks stop with the fixture; it is dropped, and the datadir removed, once
/// the remaining tasks released the databases, from the worker thread rather than from one of
/// the runtime's own threads, which could not join itself.
fn run_fixture(
    name: String,
    test: &EngineTest,
    tree_config: &TreeConfig,
    storage_settings: StorageSettings,
    datadir_root: &Path,
) -> (FixtureResult, Remains) {
    let fork = format!("{:?}", test.network);
    let mut outcome = Outcome::default();
    let runtime = Runtime::test();
    let mut remains = Remains { runtime: runtime.clone(), databases: None, datadir: None };
    let result = runtime.handle().block_on(run_case(
        test,
        tree_config,
        storage_settings,
        datadir_root,
        &runtime,
        &mut outcome,
        &mut remains,
    ));
    // Join Tokio workers before the next fixture starts, even while `remains` retains a clone.
    runtime.shutdown_timeout(ENGINE_SHUTDOWN_TIMEOUT);
    let mut result = FixtureResult::new(name, fork, result).with_rejections(outcome.rejections);
    result.last_block_hash = outcome.last_block_hash;
    result.last_payload_status = outcome.last_payload_status;
    (result, remains)
}

type FixtureNode = NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>;

async fn run_case(
    test: &EngineTest,
    tree_config: &TreeConfig,
    storage_settings: StorageSettings,
    datadir_root: &Path,
    runtime: &Runtime,
    outcome: &mut Outcome,
    remains: &mut Remains,
) -> Result<(), Error> {
    let chain_spec = Arc::new(test.chain_spec()?);
    let genesis_hash = chain_spec.genesis_hash();
    if genesis_hash != test.genesis_block_header.hash {
        return Err(Error::Assertion(format!(
            "genesis hash mismatch: the chain spec has {genesis_hash}, the fixture {}",
            test.genesis_block_header.hash
        )))
    }

    let datadir = tempfile::Builder::new()
        .prefix("reth-enginetest-")
        .tempdir_in(datadir_root)
        .map_err(|err| Error::Io { path: datadir_root.to_path_buf(), error: err })?
        .keep();
    remains.datadir = Some(datadir.clone());
    let engine = Engine::start(chain_spec, &datadir, tree_config, storage_settings, runtime)?;
    let result = drive(&engine.rpc, test, outcome).await;
    let head = engine.provider.chain_info().map(|info| info.best_hash);
    let (databases, shutdown) = engine.stop().await;
    remains.databases = Some(databases);
    outcome.last_block_hash = Some(head?);
    result?;
    shutdown?;

    match outcome.last_block_hash {
        Some(hash) if hash == test.lastblockhash => Ok(()),
        hash => Err(Error::Assertion(format!(
            "last block hash mismatch: got {hash:?}, expected {}",
            test.lastblockhash
        ))),
    }
}

/// The engine stack of one fixture, wired as the node's engine launcher wires it,
/// without the network, the RPC servers, the transaction pool or the payload builder, which the
/// fixtures do not reach.
struct Engine {
    /// The `engine_` JSON-RPC methods of the [`EngineApi`] handler.
    rpc: RpcModule<()>,
    provider: BlockchainProvider<FixtureNode>,
    databases: Databases,
    handler: tokio::task::JoinHandle<()>,
    to_handler: tokio::sync::mpsc::UnboundedSender<FromOrchestrator>,
}

impl Engine {
    fn start(
        chain_spec: Arc<ChainSpec>,
        datadir: &Path,
        tree_config: &TreeConfig,
        storage_settings: StorageSettings,
        runtime: &Runtime,
    ) -> Result<Self, Error> {
        let db = Arc::new(
            init_db(
                datadir.join("db"),
                // The database is thrown away after the fixture, so skip MDBX's durable syncs
                // (`--db.sync-mode safe-no-sync`).
                DatabaseArguments::test().with_sync_mode(Some(SyncMode::SafeNoSync)),
            )
            .map_err(|err| Error::engine("failed to set up the engine", err))?,
        );
        let rocksdb = RocksDBBuilder::new(datadir.join("rocksdb"))
            .with_default_tables()
            .build()
            .map_err(|err| Error::engine("failed to set up the engine", err))?;
        let (in_memory_state, in_memory_state_writer) = InMemoryState::new();
        let overlay_manager = OverlayManager::with_worker_pool(
            in_memory_state,
            runtime.state_trie_overlay_worker_pool(),
        );
        let factory = ProviderFactory::<FixtureNode>::new(
            db.clone(),
            chain_spec.clone(),
            StaticFileProvider::read_write(datadir.join("static_files"))
                .map_err(|err| Error::engine("failed to set up the engine", err))?,
            rocksdb.clone(),
            overlay_manager.clone(),
            runtime.clone(),
        )
        .map_err(|err| Error::engine("failed to set up the engine", err))?;
        init_genesis_with_settings(&factory, storage_settings)
            .map_err(|err| Error::engine("failed to set up the engine", err))?;
        let canonical_in_memory_state_writer = CanonicalInMemoryStateWriter::new(
            factory
                .chain_info_headers()
                .map_err(|err| Error::engine("failed to set up the engine", err))?,
            in_memory_state_writer,
        );
        let provider = BlockchainProvider::with_canonical_in_memory_state(
            factory.clone(),
            canonical_in_memory_state_writer.canonical_in_memory_state().clone(),
        );

        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));
        let evm_config = EthEvmConfig::new(chain_spec.clone());
        let validator = BasicEngineValidator::new(
            provider.clone(),
            consensus.clone(),
            evm_config.clone(),
            EthereumEngineValidator::new(chain_spec.clone()),
            tree_config.clone(),
            Box::new(NoopInvalidBlockHook::default()),
            overlay_manager.clone(),
            runtime.clone(),
        );
        let payload_builder = PayloadBuilderHandle::<EthEngineTypes>::noop();
        let (to_engine, from_api) = tokio::sync::mpsc::unbounded_channel();
        let (sync_metrics_tx, _) = tokio::sync::mpsc::unbounded_channel();
        // Built as the node's engine launcher builds it. A fixture's payloads extend blocks the
        // engine already has, so nothing is downloaded or backfilled: the block client is a no-op
        // and the backfill pipeline has no stages.
        let mut orchestrator = EngineOrchestratorBuilder {
            engine_kind: EngineApiKind::Ethereum,
            consensus,
            client: NoopFullBlockClient::<EthNetworkPrimitives>::default(),
            incoming_requests: UnboundedReceiverStream::new(from_api),
            pipeline: Pipeline::<FixtureNode>::builder().build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            ),
            pipeline_task_spawner: runtime.clone(),
            provider: factory.clone(),
            blockchain_db: provider.clone(),
            canonical_in_memory_state_writer,
            pruner: PrunerBuilder::default().build_with_provider_factory(factory),
            payload_builder: payload_builder.clone(),
            payload_validator: validator,
            overlay_manager,
            tree_config: tree_config.clone(),
            sync_metrics_tx,
            evm_config,
            runtime: runtime.clone(),
        }
        .build();

        // Polls the orchestrator as the node's consensus engine task does, without the network
        // and event bookkeeping, and passes on the runner's terminate request.
        let (to_handler, mut from_runner) =
            tokio::sync::mpsc::unbounded_channel::<FromOrchestrator>();
        let handler = runtime.handle().spawn(async move {
            loop {
                tokio::select! {
                    event = orchestrator.next() => {
                        if matches!(event, None | Some(ChainEvent::FatalError)) {
                            break
                        }
                    }
                    Some(event) = from_runner.recv() => {
                        orchestrator.handler_mut().handler_mut().on_event(event.into());
                    }
                }
            }
        });

        let api = EngineApi::new(
            provider.clone(),
            chain_spec.clone(),
            ConsensusEngineHandle::new(to_engine),
            PayloadStore::new(payload_builder),
            NoopTransactionPool::default(),
            runtime.clone(),
            ClientVersionV1 {
                code: ClientCode::RH,
                name: "reth".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                commit: String::new(),
            },
            EngineCapabilities::default(),
            EthereumEngineValidator::new(chain_spec),
            false,
            NoopNetwork::default(),
        );

        let databases = Databases { mdbx: db, rocksdb };
        Ok(Self { rpc: api.into_rpc().remove_context(), provider, databases, handler, to_handler })
    }

    /// Asks the engine tree to persist its blocks and stop, as a node does on shutdown, and
    /// returns the databases, which background tasks may still hold.
    async fn stop(self) -> (Databases, Result<(), Error>) {
        let Self { rpc, provider, databases, handler, to_handler } = self;
        drop(rpc);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let shutdown = if to_handler.send(FromOrchestrator::Terminate { tx }).is_ok() {
            match tokio::time::timeout(ENGINE_SHUTDOWN_TIMEOUT, rx).await {
                Ok(result) => {
                    result.map_err(|err| Error::engine("engine stopped before persisting", err))
                }
                Err(err) => Err(Error::engine("timed out stopping the engine", err)),
            }
        } else {
            Err(Error::Assertion("engine handler stopped before termination".to_string()))
        };
        handler.abort();
        let joined = match tokio::time::timeout(ENGINE_SHUTDOWN_TIMEOUT, handler).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) if err.is_cancelled() => Ok(()),
            Ok(Err(err)) => Err(Error::engine("engine handler panicked", err)),
            Err(err) => Err(Error::engine("timed out joining the engine handler", err)),
        };
        drop(provider);
        (databases, shutdown.and(joined))
    }
}

/// Sends the fixture's payloads in order, each valid one followed by a forkchoice update that
/// makes it the head, as a consensus client would.
async fn drive(rpc: &RpcModule<()>, test: &EngineTest, outcome: &mut Outcome) -> Result<(), Error> {
    let Some(first) = test.engine_new_payloads.first() else { return Ok(()) };
    forkchoice_updated(rpc, &first.forkchoice_updated_version, test.genesis_block_header.hash)
        .await?;

    for (idx, payload) in test.engine_new_payloads.iter().enumerate() {
        new_payload(rpc, idx, payload, outcome).await?;
        if payload.validation_error.is_none() && payload.error_code.is_none() {
            let head = payload.params[0]["blockHash"].as_str().unwrap_or_default();
            let head = head.parse().map_err(|err| {
                Error::Assertion(format!("payload {idx}: invalid blockHash {head:?}: {err}"))
            })?;
            forkchoice_updated(rpc, &payload.forkchoice_updated_version, head).await?;
        }
    }
    Ok(())
}

/// A JSON-RPC error returned by the handler.
#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl std::error::Error for RpcError {}

impl std::fmt::Display for RpcError {
    /// Formats the error as `<code>: <message>`, followed by `: <data>` when it carries data: a
    /// string as is, anything else as compact JSON.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        match &self.data {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(data)) => write!(f, ": {data}"),
            Some(data) => write!(f, ": {data}"),
        }
    }
}

/// Separates dispatch failures from client JSON-RPC responses, so a timeout is never reported as
/// a rejected payload or matched against a fixture's `errorCode`.
async fn call(
    rpc: &RpcModule<()>,
    method: &str,
    params: Value,
) -> Result<Result<Value, RpcError>, Error> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let (response, _) =
        tokio::time::timeout(ENGINE_REQUEST_TIMEOUT, rpc.raw_json_request(&request, 1))
            .await
            .map_err(|err| Error::engine(format!("{method} timed out"), err))?
            .map_err(|err| Error::engine(method, err))?;
    let response: Response<'_, Value> = serde_json::from_str(&response)
        .map_err(|err| Error::engine(format!("cannot decode {method} response"), err))?;
    Ok(match response.payload {
        ResponsePayload::Success(result) => Ok(result.into_owned()),
        ResponsePayload::Error(error) => Err(RpcError {
            code: i64::from(error.code()),
            message: error.message().to_string(),
            data: error
                .data()
                .map(|data| serde_json::from_str(data.get()))
                .transpose()
                .map_err(|err| Error::engine(format!("cannot decode {method} error data"), err))?,
        }),
    })
}

/// Calls `engine_newPayloadV<n>` with the fixture's payload at `index`, records a rejection
/// with reth's error, and checks the response against the fixture's expectation.
async fn new_payload(
    rpc: &RpcModule<()>,
    index: usize,
    payload: &EngineNewPayload,
    outcome: &mut Outcome,
) -> Result<(), Error> {
    let method = format!("engine_newPayloadV{}", payload.new_payload_version);
    let expected_code = payload
        .error_code
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
        .map_err(|err| Error::engine("invalid fixture errorCode", err))?;

    match call(rpc, &method, Value::from(payload.params.clone())).await? {
        Ok(response) => {
            let status = response["status"].as_str().unwrap_or_default().to_string();
            outcome.last_payload_status = Some(status.clone());
            if matches!(status.as_str(), "INVALID" | "INVALID_BLOCK_HASH") {
                let error = response["validationError"].as_str().unwrap_or_default().to_string();
                outcome.rejections.push(Rejection { index, hash: None, error });
            }
            let expected = if payload.validation_error.is_some() { "INVALID" } else { "VALID" };
            if status != expected {
                return Err(Error::Assertion(format!(
                    "{method} returned {response}, expected status {expected}"
                )))
            }
            if let Some(code) = expected_code {
                return Err(Error::Assertion(format!(
                    "{method} returned {response}, expected error code {code}"
                )))
            }
            Ok(())
        }
        Err(err) => {
            let code = err.code;
            outcome.last_payload_status = Some(format!("error {code}"));
            outcome.rejections.push(Rejection { index, hash: None, error: err.to_string() });
            if expected_code == Some(code) {
                Ok(())
            } else {
                Err(Error::engine(method, err))
            }
        }
    }
}

/// Calls `engine_forkchoiceUpdatedV<n>` without payload attributes and requires a valid head.
async fn forkchoice_updated(rpc: &RpcModule<()>, version: &str, head: B256) -> Result<(), Error> {
    let method = format!("engine_forkchoiceUpdatedV{version}");
    let state = json!({
        "headBlockHash": head,
        "safeBlockHash": B256::ZERO,
        "finalizedBlockHash": B256::ZERO,
    });
    let response = call(rpc, &method, json!([state, null]))
        .await?
        .map_err(|err| Error::engine(&method, err))?;
    match response["payloadStatus"]["status"].as_str() {
        Some("VALID") => Ok(()),
        _ => Err(Error::Assertion(format!("{method} returned {response}, expected status VALID"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ForkSpec, Header, State};
    use alloy_consensus::{
        proofs::calculate_transaction_root, SignableTransaction, TxLegacy, EMPTY_OMMER_ROOT_HASH,
        EMPTY_ROOT_HASH,
    };
    use alloy_eips::{eip1559::BaseFeeParams, eip2718::Encodable2718};
    use alloy_primitives::{Address, Bloom, Bytes, Signature, TxKind, U256, U64};
    use alloy_rpc_types_engine::ExecutionPayloadV1;
    use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
    use reth_evm::{database::StateProviderDatabase, ConfigureEvm, Evm};
    use reth_primitives_traits::{SignedTransaction, SignerRecoverable};
    use reth_provider::{
        test_utils::{ExtendedAccount, MockEthProvider},
        DatabaseProviderFactory, StateProvider, StorageSettingsCache,
    };
    use std::process::Command;

    /// A Paris fixture whose one payload is an empty block on genesis with the given state root,
    /// and that payload's block hash.
    fn empty_block_fixture(state_root: Option<B256>) -> (EngineTest, B256) {
        block_fixture(State::default(), Vec::new(), state_root)
    }

    fn block_fixture(
        pre: State,
        transactions: Vec<TransactionSigned>,
        state_root: Option<B256>,
    ) -> (EngineTest, B256) {
        let mut test = EngineTest {
            genesis_block_header: Header {
                gas_limit: U256::from(30_000_000),
                base_fee_per_gas: Some(U256::from(1_000_000_000)),
                ..Default::default()
            },
            pre,
            lastblockhash: B256::ZERO,
            network: ForkSpec::Merge,
            config: Default::default(),
            engine_new_payloads: Vec::new(),
        };
        let chain_spec = test.chain_spec().unwrap();
        let genesis = chain_spec.genesis_header();
        test.genesis_block_header.hash = chain_spec.genesis_hash();

        let header = alloy_consensus::Header {
            parent_hash: chain_spec.genesis_hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            state_root: state_root.unwrap_or(genesis.state_root),
            transactions_root: calculate_transaction_root(&transactions),
            receipts_root: EMPTY_ROOT_HASH,
            number: 1,
            gas_limit: genesis.gas_limit,
            timestamp: 12,
            base_fee_per_gas: genesis.next_block_base_fee(BaseFeeParams::ethereum()),
            ..Default::default()
        };
        let block_hash = header.hash_slow();
        let payload = ExecutionPayloadV1 {
            parent_hash: header.parent_hash,
            fee_recipient: Address::ZERO,
            state_root: header.state_root,
            receipts_root: header.receipts_root,
            logs_bloom: Bloom::ZERO,
            prev_randao: B256::ZERO,
            block_number: header.number,
            gas_limit: header.gas_limit,
            gas_used: 0,
            timestamp: header.timestamp,
            extra_data: Default::default(),
            base_fee_per_gas: U256::from(header.base_fee_per_gas.unwrap()),
            block_hash,
            transactions: transactions.into_iter().map(|tx| tx.encoded_2718().into()).collect(),
        };
        test.engine_new_payloads.push(EngineNewPayload {
            params: vec![serde_json::to_value(payload).unwrap()],
            new_payload_version: "1".to_string(),
            forkchoice_updated_version: "1".to_string(),
            validation_error: None,
            error_code: None,
        });
        (test, block_hash)
    }

    /// Runs a fixture as a worker does and checks that its datadir is removed.
    fn run(test: &EngineTest) -> FixtureResult {
        let datadir_root = tempfile::tempdir().unwrap();
        let (result, remains) = run_fixture(
            "test".to_string(),
            test,
            &tree_config(false),
            StorageSettings::base(),
            datadir_root.path(),
        );
        let mut cleanup = Cleanup::default();
        cleanup.remains.push(remains);
        cleanup.finish().unwrap();
        assert_eq!(fs::read_dir(datadir_root.path()).unwrap().count(), 0, "datadir left behind");
        result
    }

    #[test]
    fn engine_runs_on_the_given_storage_layout() {
        for storage_settings in [StorageSettings::v2(), StorageSettings::v1()] {
            let (test, _) = empty_block_fixture(None);
            let datadir_root = tempfile::tempdir().unwrap();
            let datadir = datadir_root.path().join("datadir");
            fs::create_dir(&datadir).unwrap();
            let runtime = Runtime::test();
            let databases = runtime.handle().block_on(async {
                let engine = Engine::start(
                    Arc::new(test.chain_spec().unwrap()),
                    &datadir,
                    &tree_config(false),
                    storage_settings,
                    &runtime,
                )
                .unwrap();
                let stored =
                    engine.provider.database_provider_ro().unwrap().cached_storage_settings();
                assert_eq!(stored, storage_settings);
                let (databases, shutdown) = engine.stop().await;
                shutdown.unwrap();
                databases
            });
            runtime.clone().shutdown_timeout(ENGINE_SHUTDOWN_TIMEOUT);
            let mut cleanup = Cleanup::default();
            cleanup.remains.push(Remains {
                runtime,
                databases: Some(databases),
                datadir: Some(datadir),
            });
            cleanup.finish().unwrap();
            let left = fs::read_dir(datadir_root.path()).unwrap().count();
            assert_eq!(left, 0, "datadir left behind");
        }
    }

    #[test]
    fn valid_payload_becomes_head() {
        let (mut test, block_hash) = empty_block_fixture(None);
        test.lastblockhash = block_hash;

        let result = run(&test);
        assert!(result.pass, "{}", result.error);
        assert_eq!(result.last_block_hash, Some(block_hash));
        assert_eq!(result.last_payload_status.as_deref(), Some("VALID"));
        assert_eq!(result.rejections, []);
    }

    #[test]
    fn invalid_payload_is_rejected() {
        let (mut test, _) = empty_block_fixture(Some(B256::repeat_byte(1)));
        test.lastblockhash = test.genesis_block_header.hash;
        test.engine_new_payloads[0].validation_error = Some("state root mismatch".to_string());

        let result = run(&test);
        assert!(result.pass, "{}", result.error);
        assert_eq!(result.last_block_hash, Some(test.genesis_block_header.hash));
        assert_eq!(result.last_payload_status.as_deref(), Some("INVALID"));
        let [rejection] = result.rejections.as_slice() else { panic!("{:?}", result.rejections) };
        assert_eq!(rejection.index, 0);
        assert!(rejection.error.contains("mismatched block state root"), "{}", rejection.error);
    }

    #[test]
    fn payload_rejected_with_an_rpc_error() {
        // Paris has no `engine_newPayloadV3`, and V3 takes three parameters, not one.
        let (mut test, _) = empty_block_fixture(None);
        test.lastblockhash = test.genesis_block_header.hash;
        test.engine_new_payloads[0].new_payload_version = "3".to_string();
        test.engine_new_payloads[0].error_code = Some("-32602".to_string());

        let result = run(&test);
        assert!(result.pass, "{}", result.error);
        let [rejection] = result.rejections.as_slice() else { panic!("{:?}", result.rejections) };
        assert_eq!(rejection.index, 0);
        assert!(rejection.error.starts_with("-32602: "), "{}", rejection.error);
    }

    #[test]
    fn unexpected_verdict_fails() {
        let (mut test, block_hash) = empty_block_fixture(Some(B256::repeat_byte(1)));
        test.lastblockhash = block_hash;

        let result = run(&test);
        assert!(!result.pass);
        assert_eq!(result.last_payload_status.as_deref(), Some("INVALID"));
    }
    /// A protected transaction calling a contract on a chain chosen by the fixture, with a
    /// deliberately wrong gas-used header so completed execution returns INVALID.
    fn transaction_fixture(chain_id: u64, code: Bytes) -> EngineTest {
        let contract = Address::with_last_byte(0x42);
        let tx = TransactionSigned::Legacy(
            TxLegacy {
                chain_id: Some(1),
                nonce: 0,
                gas_limit: 30_000_000,
                gas_price: 1_000_000_000,
                to: TxKind::Call(contract),
                ..Default::default()
            }
            .into_signed(Signature::test_signature()),
        );
        let sender = tx.recover_signer().unwrap();
        let pre = serde_json::from_value(json!({
            sender.to_string(): {"balance": "0xffffffffffffffffffffffff", "nonce": "0x0", "code": "0x", "storage": {}},
            contract.to_string(): {"balance": "0x0", "nonce": "0x1", "code": code, "storage": {}}
        })).unwrap();
        let (mut test, _) = block_fixture(pre, vec![tx], Some(B256::repeat_byte(1)));
        test.config.chain_id = Some(U64::from(chain_id));
        test.lastblockhash = test.genesis_block_header.hash;
        test.engine_new_payloads[0].validation_error = Some("invalid block".to_string());
        test
    }

    #[test]
    fn protected_transaction_uses_fixture_chain_id() {
        let result = run(&transaction_fixture(7, Bytes::from_static(&[0x00])));
        assert!(result.pass, "{}", result.error);
        assert_eq!(result.last_payload_status.as_deref(), Some("INVALID"));
        assert_eq!(result.rejections.len(), 1);
        assert_eq!(
            result.rejections[0].error,
            "EVM reported invalid transaction (0xb74c71ef0df77555a822630371d46d129d235b18edb45f1ad1f646ef7beb4f11): invalid chain id: expected 7, got 1"
        );
    }

    #[test]
    fn deeply_nested_calls_fit_the_engine_thread_stack() {
        with_runner_stack("deeply_nested_calls_fit_the_engine_thread_stack", || {
            // Repeated CALL to ADDRESS with all remaining gas recurses until the EIP-150 gas bound.
            let code = Bytes::from_static(&[
                0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x30, 0x5a, 0xf1, 0x00,
            ]);
            let result = run(&transaction_fixture(1, code));
            assert!(result.pass, "{}", result.error);
            assert_eq!(result.last_payload_status.as_deref(), Some("INVALID"));
            assert_eq!(result.rejections.len(), 1);
            assert!(
                result.rejections[0].error.starts_with("block gas used mismatch:"),
                "{}",
                result.rejections[0].error
            );
        });
    }

    #[test]
    fn deeply_nested_calls_fit_speculative_worker_stacks() {
        with_runner_stack("deeply_nested_calls_fit_speculative_worker_stacks", || {
            let runtime = Runtime::test();
            for pool in [runtime.prewarming_pool(), runtime.bal_streaming_pool()] {
                pool.install_fn(|| {
                    let sender = Address::with_last_byte(0x41);
                    let contract = Address::with_last_byte(0x42);
                    let provider = MockEthProvider::<EthPrimitives>::default();
                    provider.add_account(sender, ExtendedAccount::new(0, U256::MAX));
                    provider.add_account(
                        contract,
                        ExtendedAccount::new(1, U256::ZERO).with_bytecode(Bytes::from_static(&[
                            0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0, 0x30, 0x5a, 0xf1, 0x00,
                        ])),
                    );
                    let config = EthEvmConfig::new(ForkSpec::Merge.to_chain_spec());
                    let env = config
                        .evm_env(&alloy_consensus::Header {
                            gas_limit: 30_000_000,
                            base_fee_per_gas: Some(0),
                            ..Default::default()
                        })
                        .unwrap();
                    let tx = TransactionSigned::Legacy(
                        TxLegacy {
                            chain_id: Some(1),
                            gas_limit: 30_000_000,
                            to: TxKind::Call(contract),
                            ..Default::default()
                        }
                        .into_signed(Signature::test_signature()),
                    );
                    let tx = config.tx_env(tx.with_signer(sender));
                    let mut evm = config.evm_with_env(
                        StateProviderDatabase(provider.into_evm_state_provider()),
                        env,
                    );
                    evm.transact_and_discard(&tx, |_| {}).unwrap();
                });
            }
        });
    }

    #[test]
    fn fixture_configuration_errors_are_reported_without_starting_an_engine() {
        let (mut test, _) = empty_block_fixture(None);
        test.network = ForkSpec::MergePush0;
        let result = run(&test);
        assert!(!result.pass);
        assert_eq!(result.error, "unsupported fixture fork: MergePush0");
        test.network = ForkSpec::Merge;
        test.genesis_block_header.gas_limit = U256::from(u64::MAX) + U256::ONE;
        let result = run(&test);
        assert!(!result.pass);
        assert_eq!(result.last_payload_status, None);
        assert!(result.rejections.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn request_timeout_is_not_a_payload_rejection() {
        let mut rpc = RpcModule::new(());
        rpc.register_async_method("engine_newPayloadV1", |_, _| async {
            std::future::pending::<Result<Value, jsonrpsee::types::ErrorObjectOwned>>().await
        })
        .unwrap();
        let (test, _) = empty_block_fixture(None);
        let mut outcome = Outcome::default();
        let error =
            new_payload(&rpc, 0, &test.engine_new_payloads[0], &mut outcome).await.unwrap_err();
        assert_eq!(error.to_string(), "engine_newPayloadV1 timed out: deadline has elapsed");
        assert_eq!(outcome.rejections, []);
        assert_eq!(outcome.last_payload_status, None);
    }

    #[test]
    fn cleanup_keeps_pending_runtimes_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut cleanup = Cleanup::default();
        let (mut test, hash) = empty_block_fixture(None);
        test.lastblockhash = hash;
        for _ in 0..MAX_PENDING_ENGINES * 2 {
            let (result, remains) = run_fixture(
                "test".to_string(),
                &test,
                &tree_config(false),
                StorageSettings::base(),
                dir.path(),
            );
            assert!(result.pass, "{}", result.error);
            cleanup.push(remains).unwrap();
            assert!(cleanup.remains.len() < MAX_PENDING_ENGINES);
            assert!(cleanup.datadirs.len() < MAX_PENDING_ENGINES);
        }
        cleanup.finish().unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn fixture_engines_bound_bal_prewarm_workers_without_disabling_parallel_execution() {
        let config = tree_config(false);
        assert_eq!(config.bal_prewarm_threads(), Some(BAL_PREWARM_THREADS));
        assert!(!config.disable_bal_parallel_execution());
        assert_eq!(TreeConfig::default().bal_prewarm_threads(), None);
    }

    /// Exercises deep calls in a process started with the runner's stack policy, without
    /// changing environment variables while the test harness has other threads running.
    fn with_runner_stack(test: &str, run: impl FnOnce()) {
        let minimum_stack_size = 8 * 1024 * 1024;
        if matches!(std::env::var("RUST_MIN_STACK"), Ok(value) if value.parse::<usize>().is_ok_and(|size| size >= minimum_stack_size))
        {
            run();
            return
        }
        let module = module_path!().split_once("::").unwrap().1;
        let test = format!("{module}::{test}");
        let output = Command::new(std::env::current_exe().unwrap())
            .env("RUST_MIN_STACK", minimum_stack_size.to_string())
            .args(["--exact", &test, "--color", "never"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let expected = format!("test {test} ... ok");
        assert!(stdout.lines().any(|line| line == expected), "{stdout}");
    }
}
