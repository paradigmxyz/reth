//! Ethereum EVM and executor builder with bytecode compiler support.

use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, FullNodeTypes};
use reth_node_builder::{components::ExecutorBuilder, BuilderContext};
use reth_node_ethereum::EthEvmConfig;
use reth_revm::{handler::register::EvmHandler, primitives::SpecId, Database, Evm};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

pub use reth_evm_compiler::*;

/// Ethereum EVM and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CompilerExecutorBuilder;

impl<Node: FullNodeTypes> ExecutorBuilder<Node> for CompilerExecutorBuilder {
    type EVM = CompilerEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let compiler_config = &ctx.config().experimental.compiler;
        if !compiler_config.compiler {
            tracing::debug!("EVM bytecode compiler is disabled");
            return Ok(CompilerEvmConfig::disabled());
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
        ctx.task_executor().spawn_blocking(async move {
            if let Err(err) = compiler.run_to_end(&contracts_config) {
                tracing::error!(%err, "failed to run compiler");
            }
            done2.store(true, Ordering::Relaxed);
        });
        Ok(CompilerEvmConfig::new(done, out_dir))
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
        CompilerEvmContext {
            config: self.0.as_ref().map(|c| c.lock().unwrap_or_else(PoisonError::into_inner)),
        }
    }
}

impl CompilerEvmConfigInner {
    fn get_or_load_library(&mut self, spec_id: SpecId) -> Option<&mut EvmCompilerDll> {
        if !self.compiler_is_done.load(Ordering::Relaxed) {
            return None;
        }

        let evm_version = spec_id_to_evm_version(spec_id);
        if let Some(v) = self.evm_version {
            if v == evm_version {
                return self.dll.as_mut();
            }
        }

        self.load_library(evm_version)
    }

    #[cold]
    fn load_library(&mut self, evm_version: SpecId) -> Option<&mut EvmCompilerDll> {
        if let Some(dll) = self.dll.take() {
            if let Err(err) = dll.close() {
                tracing::error!(%err, ?self.evm_version, "failed to close shared library");
            }
        }
        self.evm_version = Some(evm_version);
        self.dll = match unsafe { EvmCompilerDll::open_in(&self.out_dir, evm_version) } {
            Ok(library) => Some(library),
            Err(err) => {
                tracing::error!(%err, ?evm_version, "failed to load shared library");
                None
            }
        };
        self.dll.as_mut()
    }
}

impl ConfigureEvmEnv for CompilerEvmConfig {
    type TxMeta = <EthEvmConfig as ConfigureEvmEnv>::TxMeta;

    fn fill_tx_env<T>(
        tx_env: &mut reth_revm::primitives::TxEnv,
        transaction: T,
        sender: reth_primitives::Address,
        meta: Self::TxMeta,
    ) where
        T: AsRef<reth_primitives::Transaction>,
    {
        <EthEvmConfig as ConfigureEvmEnv>::fill_tx_env(tx_env, transaction, sender, meta)
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

impl CompilerEvmContext<'_> {
    #[inline]
    fn get_or_load_library(&mut self, spec_id: SpecId) -> Option<&mut EvmCompilerDll> {
        self.config.as_mut()?.get_or_load_library(spec_id)
    }
}

fn register_compiler_handler<DB: Database>(
    handler: &mut EvmHandler<'_, CompilerEvmContext<'_>, DB>,
) {
    handler.execution.execute_frame = Some(Arc::new(move |frame, memory, evm| {
        let interpreter = frame.interpreter_mut();
        let hash = interpreter.contract.hash?;
        let library = evm.context.external.get_or_load_library(evm.spec_id())?;
        let f = match library.get_function(hash) {
            Ok(f) => f?,
            // Shouldn't happen.
            Err(err) => {
                tracing::error!(%err, %hash, "failed getting function from shared library");
                return None;
            }
        };

        interpreter.shared_memory =
            std::mem::replace(memory, reth_revm::interpreter::EMPTY_SHARED_MEMORY);
        let r = unsafe { f.call_with_interpreter(interpreter, evm) };
        *memory = interpreter.take_memory();
        Some(r)
    }));
}
