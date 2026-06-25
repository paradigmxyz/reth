//! Launch-time task executors for main and latency-sensitive work.

use reth_tasks::{RuntimeBuildError, TaskExecutor, TokioConfig};
use tracing::info;

/// Task executors used during node launch.
///
/// When latency isolation is disabled (`latency_isolated == false`), `main` and `rpc` are the same
/// executor. When enabled, `rpc` is an attached tokio runtime that shares shutdown and panic
/// reporting with `main`.
#[derive(Debug, Clone)]
pub struct LaunchExecutors {
    main: TaskExecutor,
    rpc: TaskExecutor,
    latency_isolated: bool,
}

impl LaunchExecutors {
    /// Creates a launch context with a single executor for both main and RPC work.
    pub fn single(executor: TaskExecutor) -> Self {
        Self { main: executor.clone(), rpc: executor, latency_isolated: false }
    }

    /// Creates a launch context with a dedicated latency-sensitive tokio runtime.
    pub fn with_latency(
        main: TaskExecutor,
        worker_threads: usize,
    ) -> Result<Self, RuntimeBuildError> {
        let rpc = main.build_attached_tokio(TokioConfig::latency_runtime(worker_threads))?;
        info!(
            target: "reth::cli",
            threads = worker_threads,
            "Latency RPC tokio runtime enabled"
        );
        Ok(Self { main, rpc, latency_isolated: true })
    }

    /// Returns the main task executor.
    pub const fn main(&self) -> &TaskExecutor {
        &self.main
    }

    /// Returns the RPC/latency task executor.
    pub const fn rpc(&self) -> &TaskExecutor {
        &self.rpc
    }

    /// Returns `true` if a dedicated latency runtime is configured.
    pub const fn latency_isolated(&self) -> bool {
        self.latency_isolated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LaunchContext;
    use reth_chainspec::MAINNET;
    use reth_node_core::{args::DatadirArgs, dirs::MaybePlatformPath};
    use reth_tasks::{RuntimeBuilder, RuntimeConfig};
    use std::str::FromStr;
    use tempfile::tempdir;

    #[test]
    fn with_latency_creates_isolated_rpc_executor() {
        let main = RuntimeBuilder::new(RuntimeConfig::default()).build().unwrap();
        let executors = LaunchExecutors::with_latency(main.clone(), 2).unwrap();

        assert!(executors.latency_isolated());
        assert_ne!(main.handle().id(), executors.rpc().handle().id());
        assert_eq!(main.handle().id(), executors.main().handle().id());
    }

    #[test]
    fn single_executor_is_not_latency_isolated() {
        let main = RuntimeBuilder::new(RuntimeConfig::default()).build().unwrap();
        let executors = LaunchExecutors::single(main.clone());

        assert!(!executors.latency_isolated());
        assert_eq!(main.handle().id(), executors.rpc().handle().id());
    }

    #[test]
    fn launch_context_wires_latency_executors() {
        let main = RuntimeBuilder::new(RuntimeConfig::default()).build().unwrap();
        let executors = LaunchExecutors::with_latency(main.clone(), 2).unwrap();
        let tempdir = tempdir().unwrap();
        let datadir_args = DatadirArgs {
            datadir: MaybePlatformPath::from_str(tempdir.path().to_str().unwrap()).unwrap(),
            ..Default::default()
        };
        let data_dir =
            datadir_args.datadir.clone().unwrap_or_chain_default(MAINNET.chain, datadir_args);

        let ctx = LaunchContext::new(executors, data_dir);
        assert!(ctx.latency_isolated());
        assert_ne!(ctx.task_executor().handle().id(), ctx.rpc_task_executor().handle().id());
        assert_eq!(ctx.task_executor().handle().id(), main.handle().id());
    }
}
