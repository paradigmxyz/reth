//! Ethereum EVM and executor builder with bytecode compiler support.

use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, FullNodeTypes};
use reth_node_builder::{components::ExecutorBuilder, BuilderContext};
use reth_node_ethereum::{EthEvmConfig, EthExecutorProvider};
use reth_primitives::B256;
use reth_revm::{
    handler::register::EvmHandler,
    interpreter::{InterpreterAction, SharedMemory},
    primitives::SpecId,
    Context, Database, Evm, Frame,
};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

pub use reth_evm_compiler::*;

/// About 10MiB of cached `bytecode_hash -> function_pointer` (40 bytes).
const MAX_CACHE_SIZE: usize = 250_000;

/// Ethereum EVM and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CompilerExecutorBuilder;

impl<Node: FullNodeTypes> ExecutorBuilder<Node> for CompilerExecutorBuilder {
    type EVM = CompilerEvmConfig;
    type Executor = EthExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let mk_return = |config: Self::EVM| {
            (config.clone(), EthExecutorProvider::new(ctx.chain_spec(), config))
        };

        let compiler_config = &ctx.config().experimental.compiler;
        if !compiler_config.compiler {
            tracing::debug!("EVM bytecode compiler is disabled");
            return Ok(mk_return(CompilerEvmConfig::disabled()));
        }
        tracing::info!("EVM bytecode compiler initialized");

        let out_dir = compiler_config
            .out_dir
            .clone()
            .unwrap_or_else(|| ctx.data_dir().compiler().join("artifacts"));
        let mut compiler = EvmParCompiler::new(out_dir.clone())?;

        let contracts_path = compiler_config
            .contracts_file
            .clone()
            .unwrap_or_else(|| ctx.data_dir().compiler().join("contracts.toml"));
        let contracts_config = ContractsConfig::load(&contracts_path)?;

        let done = Arc::new(AtomicBool::new(false));
        let done2 = done.clone();
        let handle = ctx.task_executor().spawn_blocking(async move {
            if let Err(err) = compiler.run_to_end(&contracts_config) {
                tracing::error!(%err, "failed to run compiler");
            }
            done2.store(true, Ordering::Relaxed);
        });
        if compiler_config.block_on_compiler {
            tracing::info!("Blocking on EVM bytecode compiler");
            handle.await?;
            tracing::info!("Done blocking on EVM bytecode compiler");
        }
        Ok(mk_return(CompilerEvmConfig::new(done, out_dir)))
    }
}

/// Ethereum EVM configuration with bytecode compiler support.
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct CompilerEvmConfig(Option<Arc<Mutex<CompilerEvmConfigInner>>>);

struct CompilerEvmConfigInner {
    compiler_is_done: Arc<AtomicBool>,
    out_dir: PathBuf,
    evm_version: Option<SpecId>,
    dll: Option<EvmCompilerDll>,
}

impl CompilerEvmConfig {
    fn new(compiler_is_done: Arc<AtomicBool>, out_dir: PathBuf) -> Self {
        Self(Some(Arc::new(Mutex::new(CompilerEvmConfigInner {
            compiler_is_done,
            out_dir,
            evm_version: None,
            dll: None,
        }))))
    }

    fn disabled() -> Self {
        Self(None)
    }

    fn make_context(&self) -> CompilerEvmContext<'_> {
        CompilerEvmContext::new(
            self.0.as_ref().map(|c| c.lock().unwrap_or_else(PoisonError::into_inner)),
        )
    }
}

impl CompilerEvmConfigInner {
    /// Clears the cache if it's too big.
    fn gc(&mut self) {
        if let Some(dll) = &mut self.dll {
            if dll.cache.len() > MAX_CACHE_SIZE {
                dll.cache = Default::default();
            }
        }
    }

    fn get_or_load_library(&mut self, spec_id: SpecId) -> Option<&mut EvmCompilerDll> {
        self.maybe_load_library(spec_id);
        self.dll.as_mut()
    }

    fn maybe_load_library(&mut self, spec_id: SpecId) {
        if !self.compiler_is_done.load(Ordering::Relaxed) {
            return;
        }

        let evm_version = spec_id_to_evm_version(spec_id);
        if let Some(v) = self.evm_version {
            if v == evm_version {
                return;
            }
        }

        self.load_library(evm_version);
    }

    #[cold]
    #[inline(never)]
    fn load_library(&mut self, evm_version: SpecId) {
        if let Some(dll) = self.dll.take() {
            if let Err(err) = dll.close() {
                tracing::error!(%err, ?self.evm_version, "failed to close shared library");
            }
        }
        self.evm_version = Some(evm_version);
        self.dll = match unsafe { EvmCompilerDll::open_in(&self.out_dir, evm_version) } {
            Ok(library) => Some(library),
            // TODO: This can happen if the library is not found, but we should handle it better.
            Err(err) => {
                tracing::warn!(%err, ?evm_version, "failed to load shared library");
                None
            }
        };
    }
}

impl ConfigureEvmEnv for CompilerEvmConfig {
    fn fill_tx_env(
        tx_env: &mut reth_revm::primitives::TxEnv,
        transaction: &reth_primitives::TransactionSigned,
        sender: reth_primitives::Address,
    ) {
        <EthEvmConfig as ConfigureEvmEnv>::fill_tx_env(tx_env, transaction, sender)
    }

    fn fill_cfg_env(
        cfg_env: &mut reth_revm::primitives::CfgEnvWithHandlerCfg,
        chain_spec: &reth_primitives::ChainSpec,
        header: &reth_primitives::Header,
        total_difficulty: reth_primitives::U256,
    ) {
        <EthEvmConfig as ConfigureEvmEnv>::fill_cfg_env(
            cfg_env,
            chain_spec,
            header,
            total_difficulty,
        )
    }
}

impl ConfigureEvm for CompilerEvmConfig {
    type DefaultExternalContext<'a> = CompilerEvmContext<'a>;

    fn evm<'a, DB: reth_revm::Database + 'a>(
        &'a self,
        db: DB,
    ) -> Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        let builder = Evm::builder().with_db(db).with_external_context(self.make_context());
        if self.0.is_some() {
            builder.append_handler_register(register_compiler_handler).build()
        } else {
            builder.build()
        }
    }
}

/// [`CompilerEvmConfig`] EVM context.
#[allow(missing_debug_implementations)]
pub struct CompilerEvmContext<'a> {
    config: Option<MutexGuard<'a, CompilerEvmConfigInner>>,
}

impl<'a> CompilerEvmContext<'a> {
    fn new(mut config: Option<MutexGuard<'a, CompilerEvmConfigInner>>) -> Self {
        if let Some(config) = &mut config {
            config.gc();
        }
        Self { config }
    }

    #[inline]
    fn get_or_load_library(&mut self, spec_id: SpecId) -> Option<&mut EvmCompilerDll> {
        match self.config.as_mut() {
            Some(config) => config.get_or_load_library(spec_id),
            None => unreachable_misconfigured(),
        }
    }
}

fn register_compiler_handler<DB: Database>(
    handler: &mut EvmHandler<'_, CompilerEvmContext<'_>, DB>,
) {
    let previous = handler.execution.execute_frame.clone();
    handler.execution.execute_frame = Arc::new(move |frame, memory, table, context| {
        if let Some(action) = execute_frame(frame, memory, context) {
            Ok(action)
        } else {
            previous(frame, memory, table, context)
        }
    });
}

fn execute_frame<'cx, DB: Database>(
    frame: &mut Frame,
    memory: &mut SharedMemory,
    context: &mut Context<CompilerEvmContext<'cx>, DB>,
) -> Option<InterpreterAction> {
    let library = context.external.get_or_load_library(context.evm.spec_id())?;
    let interpreter = frame.interpreter_mut();
    let hash = match interpreter.contract.hash {
        Some(hash) => hash,
        None => unreachable_no_hash(),
    };
    let f = match library.get_function(hash) {
        Ok(Some(f)) => f,
        Ok(None) => return None,
        // Shouldn't happen.
        Err(err) => {
            unlikely_log_get_function_error(err, &hash);
            return None;
        }
    };

    interpreter.shared_memory =
        std::mem::replace(memory, reth_revm::interpreter::EMPTY_SHARED_MEMORY);
    let result = unsafe { f.call_with_interpreter(interpreter, context) };
    *memory = interpreter.take_memory();
    Some(result)
}

#[cold]
#[inline(never)]
const fn unreachable_no_hash() -> ! {
    panic!("unreachable: bytecode hash is not set in the interpreter")
}

#[cold]
#[inline(never)]
const fn unreachable_misconfigured() -> ! {
    panic!("unreachable: AOT EVM is misconfigured")
}

#[cold]
#[inline(never)]
fn unlikely_log_get_function_error(err: impl std::error::Error, hash: &B256) {
    tracing::error!(%err, %hash, "failed getting function from shared library");
}
