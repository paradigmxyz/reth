//! A tokio based CLI runner.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Entrypoint for running commands.

use reth_tasks::{PanickedTaskError, TaskExecutor};
use std::{future::Future, pin::pin, sync::mpsc, time::Duration};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

/// Executes CLI commands.
///
/// Provides utilities for running a cli command to completion.
#[derive(Debug)]
pub struct CliRunner {
    config: CliRunnerConfig,
    runtime: reth_tasks::Runtime,
}

impl CliRunner {
    /// Attempts to create a new [`CliRunner`] using the default
    /// [`Runtime`](reth_tasks::Runtime).
    ///
    /// The default runtime is multi-threaded, with both I/O and time drivers enabled.
    pub fn try_default_runtime() -> Result<Self, reth_tasks::RuntimeBuildError> {
        Self::try_with_runtime_config(reth_tasks::RuntimeConfig::default())
    }

    /// Creates a new [`CliRunner`] with the given [`RuntimeConfig`](reth_tasks::RuntimeConfig).
    pub fn try_with_runtime_config(
        config: reth_tasks::RuntimeConfig,
    ) -> Result<Self, reth_tasks::RuntimeBuildError> {
        let runtime = reth_tasks::RuntimeBuilder::new(config).build()?;
        Ok(Self { config: CliRunnerConfig::default(), runtime })
    }

    /// Sets the [`CliRunnerConfig`] for this runner.
    pub const fn with_config(mut self, config: CliRunnerConfig) -> Self {
        self.config = config;
        self
    }

    /// Returns a clone of the underlying [`Runtime`](reth_tasks::Runtime).
    pub fn runtime(&self) -> reth_tasks::Runtime {
        self.runtime.clone()
    }

    /// Executes an async block on the runtime and blocks until completion.
    pub fn block_on<F, T>(&self, fut: F) -> T
    where
        F: Future<Output = T>,
    {
        self.runtime.handle().block_on(fut)
    }

    /// Executes the given _async_ command on the tokio runtime until the command future resolves or
    /// until the process receives a `SIGINT` or `SIGTERM` signal.
    ///
    /// Tasks spawned by the command via the [`TaskExecutor`] are shut down and an attempt is made
    /// to drive their shutdown to completion after the command has finished.
    pub fn run_command_until_exit<F, E>(
        self,
        command: impl FnOnce(CliContext) -> F,
    ) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>>,
        E: Send + Sync + From<std::io::Error> + From<reth_tasks::PanickedTaskError> + 'static,
    {
        let (context, task_manager_handle) = cli_context(&self.runtime);

        // Executes the command until it finished or ctrl-c was fired
        let command_res = self.runtime.handle().block_on(run_to_completion_or_panic(
            task_manager_handle,
            run_until_ctrl_c(command(context)),
        ));

        if command_res.is_err() {
            error!(target: "reth::cli", "shutting down due to error");
        } else {
            debug!(target: "reth::cli", "shutting down gracefully");
            // after the command has finished or exit signal was received we shutdown the
            // runtime which fires the shutdown signal to all tasks spawned via the task
            // executor and awaiting on tasks spawned with graceful shutdown
            self.runtime.graceful_shutdown_with_timeout(self.config.graceful_shutdown_timeout);
        }

        runtime_shutdown(self.runtime, true);

        command_res
    }

    /// Executes a command in a blocking context with access to `CliContext`.
    ///
    /// See [`Runtime::spawn_blocking`](tokio::runtime::Runtime::spawn_blocking).
    pub fn run_blocking_command_until_exit<F, E>(
        self,
        command: impl FnOnce(CliContext) -> F + Send + 'static,
    ) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Send + Sync + From<std::io::Error> + From<reth_tasks::PanickedTaskError> + 'static,
    {
        let (context, task_manager_handle) = cli_context(&self.runtime);

        // Spawn the command on the blocking thread pool
        let handle = self.runtime.handle().clone();
        let handle2 = handle.clone();
        let command_handle = handle.spawn_blocking(move || handle2.block_on(command(context)));

        // Wait for the command to complete or ctrl-c
        let command_res = self.runtime.handle().block_on(run_to_completion_or_panic(
            task_manager_handle,
            run_until_ctrl_c(
                async move { command_handle.await.expect("Failed to join blocking task") },
            ),
        ));

        if command_res.is_err() {
            error!(target: "reth::cli", "shutting down due to error");
        } else {
            debug!(target: "reth::cli", "shutting down gracefully");
            self.runtime.graceful_shutdown_with_timeout(self.config.graceful_shutdown_timeout);
        }

        runtime_shutdown(self.runtime, true);

        command_res
    }

    /// Executes a regular future until completion or until external signal received.
    pub fn run_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>>,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        self.runtime.handle().block_on(run_until_ctrl_c(fut))?;
        Ok(())
    }

    /// Executes a regular future as a spawned blocking task until completion or until external
    /// signal received.
    ///
    /// See [`Runtime::spawn_blocking`](tokio::runtime::Runtime::spawn_blocking).
    pub fn run_blocking_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        let handle = self.runtime.handle().clone();
        let handle2 = handle.clone();
        let fut = handle.spawn_blocking(move || handle2.block_on(fut));
        self.runtime
            .handle()
            .block_on(run_until_ctrl_c(async move { fut.await.expect("Failed to join task") }))?;

        runtime_shutdown(self.runtime, false);

        Ok(())
    }
}

/// Extracts the task manager handle from the runtime and creates the [`CliContext`].
fn cli_context(
    runtime: &reth_tasks::Runtime,
) -> (CliContext, JoinHandle<Result<(), PanickedTaskError>>) {
    let handle =
        runtime.take_task_manager_handle().expect("Runtime must contain a TaskManager handle");
    let context = CliContext { task_executor: runtime.clone() };
    (context, handle)
}

/// Additional context provided by the [`CliRunner`] when executing commands
#[derive(Debug)]
pub struct CliContext {
    /// Used to execute/spawn tasks
    pub task_executor: TaskExecutor,
}

/// Default timeout for graceful shutdown of tasks.
const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for [`CliRunner`].
#[derive(Debug, Clone)]
pub struct CliRunnerConfig {
    /// Timeout for graceful shutdown of tasks.
    ///
    /// After the command completes, this is the maximum time to wait for spawned tasks
    /// to finish before forcefully terminating them.
    pub graceful_shutdown_timeout: Duration,
}

impl Default for CliRunnerConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl CliRunnerConfig {
    /// Creates a new config with default values.
    pub const fn new() -> Self {
        Self { graceful_shutdown_timeout: DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT }
    }

    /// Sets the graceful shutdown timeout.
    pub const fn with_graceful_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.graceful_shutdown_timeout = timeout;
        self
    }
}

/// Runs the given future to completion or until a critical task panicked.
///
/// Returns the error if a task panicked, or the given future returned an error.
async fn run_to_completion_or_panic<F, E>(
    task_manager_handle: JoinHandle<Result<(), PanickedTaskError>>,
    fut: F,
) -> Result<(), E>
where
    F: Future<Output = Result<(), E>>,
    E: Send + Sync + From<reth_tasks::PanickedTaskError> + 'static,
{
    let fut = pin!(fut);
    tokio::select! {
        task_manager_result = task_manager_handle => {
            if let Ok(Err(panicked_error)) = task_manager_result {
                return Err(panicked_error.into());
            }
        },
        res = fut => res?,
    }
    Ok(())
}

/// Runs the future to completion or until:
/// - `ctrl-c` is received.
/// - `SIGTERM` is received (unix only).
async fn run_until_ctrl_c<F, E>(fut: F) -> Result<(), E>
where
    F: Future<Output = Result<(), E>>,
    E: Send + Sync + 'static + From<std::io::Error>,
{
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut stream = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let sigterm = stream.recv();
        let sigterm = pin!(sigterm);
        let ctrl_c = pin!(ctrl_c);
        let fut = pin!(fut);

        tokio::select! {
            _ = ctrl_c => {
                info!(target: "reth::cli", "Received ctrl-c");
            },
            _ = sigterm => {
                info!(target: "reth::cli", "Received SIGTERM");
            },
            res = fut => res?,
        }
    }

    #[cfg(not(unix))]
    {
        let ctrl_c = pin!(ctrl_c);
        let fut = pin!(fut);

        tokio::select! {
            _ = ctrl_c => {
                info!(target: "reth::cli", "Received ctrl-c");
            },
            res = fut => res?,
        }
    }

    Ok(())
}

/// Default timeout for waiting on the tokio runtime to shut down.
const DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Shut down the given [`Runtime`](reth_tasks::Runtime), and wait for it if `wait` is set.
///
/// Dropping the runtime on the current thread could block due to tokio pool teardown.
/// Instead, we drop it on a separate thread and optionally wait for completion.
fn runtime_shutdown(rt: reth_tasks::Runtime, wait: bool) {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rt-shutdown".to_string())
        .spawn(move || {
            drop(rt);
            let _ = tx.send(());
        })
        .unwrap();

    if wait {
        let _ = rx.recv_timeout(DEFAULT_RUNTIME_SHUTDOWN_TIMEOUT).inspect_err(|err| {
            tracing::warn!(target: "reth::cli", %err, "runtime shutdown timed out");
        });
    }
}
