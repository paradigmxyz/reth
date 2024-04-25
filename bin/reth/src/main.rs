#![allow(missing_docs)]

// We use jemalloc for performance reasons.
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "optimism", not(test)))]
compile_error!("Cannot build the `reth` binary with the `optimism` feature flag enabled. Did you mean to build `op-reth`?");

#[cfg(not(feature = "optimism"))]
fn main() {
    use reth::cli::Cli;
    use reth_node_ethereum::EthereumNode;

    reth::sigsegv_handler::install();

    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = Cli::parse_args().run(|builder, _| async {
        #[cfg(feature = "compiler")]
        let node = EthereumNode::default();
        #[cfg(not(feature = "compiler"))]
        let node = EthereumNode::default();
        let handle = builder.launch_node(node).await?;
        handle.node_exit_future.await
    }) {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

mod compiler {
    use reth_config::{config::CompilerConfig, Config};
    use reth_node_api::{ConfigureEvm, ConfigureEvmEnv, NodeTypes};
    use reth_node_ethereum::{EthEngineTypes, EthEvmConfig};
    use reth_revm::Evm;
    use reth_rpc_builder::EthConfig;
    use reth_tasks::TaskExecutor;

    struct CompilerNode {}

    impl CompilerNode {
        fn new(config: &CompilerConfig) -> Self {
            Self {}
        }
    }

    impl NodeTypes for CompilerNode {
        type Primitives = ();
        type Engine = EthEngineTypes;
        type Evm = EthEvmConfig;

        fn evm_config(&self) -> Self::Evm {
            EthEvmConfig::default()
        }
    }

    #[derive(Clone)]
    struct CompilerEvmConfig {}

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
        fn evm<'a, DB: reth_revm::Database + 'a>(&self, db: DB) -> Evm<'a, (), DB> {
            Evm::builder()
                .with_db(db)
                .with_external_context(CompilerContext::new())
                .append_handler_register(register_compiler_handler)
                .build()
        }
    }

    struct CompilerContext {}

    impl CompilerContext {
        fn new() -> Self {
            Self {}
        }
    }

    fn register_compiler_handler<'a, 'ctx, DB: reth_revm::Database>(
        handler: &mut reth_revm::handler::register::EvmHandler<'a, CompilerContext, DB>,
    ) {
        handler.execution.execute_frame = Some(Arc::new(|frame, memory, evm| {
            let interpreter = frame.interpreter_mut();

            let address = interpreter.contract.target_address;
            let bytecode = interpreter.contract.bytecode.original_byte_slice();
            let spec_id = evm.handler.cfg().spec_id;
            let f = evm.context.external.get_or_compile(address, bytecode, spec_id);

            interpreter.shared_memory = std::mem::take(memory);
            let r = unsafe { f.call_with_interpreter(interpreter, evm) };
            *memory = interpreter.take_memory();
            Some(r)
        }));
    }
}
