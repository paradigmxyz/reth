//! Entrypoint for running commands.

use futures::pin_mut;
use reth_tasks::{TaskExecutor, TaskManager};
use std::future::Future;
use tracing::{debug, error, trace};

/// Executes CLI commands.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct CliRunner;

// === impl CliRunner ===

impl CliRunner {
    /// Executes the given _async_ command on the tokio runtime until the command future resolves or
    /// until the process receives a `SIGINT` or `SIGTERM` signal.
    ///
    /// Tasks spawned by the command via the [TaskExecutor] are shut down and an attempt is made to
    /// drive their shutdown to completion after the command has finished.
    pub fn run_command_until_exit<F, E>(
        self,
        command: impl FnOnce(CliContext) -> F,
    ) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>>,
        E: Send + Sync + From<std::io::Error> + From<reth_tasks::PanickedTaskError> + 'static,
    {
        let AsyncCliRunner { context, mut task_manager, tokio_runtime } = AsyncCliRunner::new()?;

        // Executes the command until it finished or ctrl-c was fired
        let command_res = tokio_runtime.block_on(run_to_completion_or_panic(
            &mut task_manager,
            run_until_ctrl_c(command(context)),
        ));

        if command_res.is_err() {
            error!(target: "reth::cli", "shutting down due to error");
        } else {
            debug!(target: "reth::cli", "shutting down gracefully");
            // after the command has finished or exit signal was received we shutdown the task
            // manager which fires the shutdown signal to all tasks spawned via the task
            // executor and awaiting on tasks spawned with graceful shutdown
            task_manager.graceful_shutdown_with_timeout(std::time::Duration::from_secs(10));
        }

        // drop the tokio runtime on a separate thread because drop blocks until its pools
        // (including blocking pool) are shutdown. In other words `drop(tokio_runtime)` would block
        // the current thread but we want to exit right away.
        std::thread::spawn(move || drop(tokio_runtime));

        command_res
    }

    /// Executes a regular future until completion or until external signal received.
    pub fn run_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>>,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        let tokio_runtime = tokio_runtime()?;
        tokio_runtime.block_on(run_until_ctrl_c(fut))?;
        Ok(())
    }

    /// Executes a regular future as a spawned blocking task until completion or until external
    /// signal received.
    ///
    /// See [Runtime::spawn_blocking](tokio::runtime::Runtime::spawn_blocking) .
    pub fn run_blocking_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        let tokio_runtime = tokio_runtime()?;
        let handle = tokio_runtime.handle().clone();
        let fut = tokio_runtime.handle().spawn_blocking(move || handle.block_on(fut));
        tokio_runtime
            .block_on(run_until_ctrl_c(async move { fut.await.expect("Failed to join task") }))?;

        // drop the tokio runtime on a separate thread because drop blocks until its pools
        // (including blocking pool) are shutdown. In other words `drop(tokio_runtime)` would block
        // the current thread but we want to exit right away.
        std::thread::spawn(move || drop(tokio_runtime));

        Ok(())
    }
}

/// [CliRunner] configuration when executing commands asynchronously
struct AsyncCliRunner {
    context: CliContext,
    task_manager: TaskManager,
    tokio_runtime: tokio::runtime::Runtime,
}

// === impl AsyncCliRunner ===

impl AsyncCliRunner {
    /// Attempts to create a tokio Runtime and additional context required to execute commands
    /// asynchronously.
    fn new() -> Result<Self, std::io::Error> {
        let tokio_runtime = tokio_runtime()?;
        let task_manager = TaskManager::new(tokio_runtime.handle().clone());
        let task_executor = task_manager.executor();
        Ok(Self { context: CliContext { task_executor }, task_manager, tokio_runtime })
    }
}

/// Additional context provided by the [CliRunner] when executing commands
#[derive(Debug)]
pub struct CliContext {
    /// Used to execute/spawn tasks
    pub task_executor: TaskExecutor,
}

/// Creates a new default tokio multi-thread [Runtime](tokio::runtime::Runtime) with all features
/// enabled
pub fn tokio_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build()
}

/// Runs the given future to completion or until a critical task panicked.
///
/// Returns the error if a task panicked, or the given future returned an error.
async fn run_to_completion_or_panic<F, E>(tasks: &mut TaskManager, fut: F) -> Result<(), E>
where
    F: Future<Output = Result<(), E>>,
    E: Send + Sync + From<reth_tasks::PanickedTaskError> + 'static,
{
    {
        pin_mut!(fut);
        tokio::select! {
            err = tasks => {
                return Err(err.into())
            },
            res = fut => res?,
        }
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
        pin_mut!(sigterm, ctrl_c, fut);

        tokio::select! {
            _ = ctrl_c => {
                trace!(target: "reth::cli", "Received ctrl-c");
            },
            _ = sigterm => {
                trace!(target: "reth::cli", "Received SIGTERM");
            },
            res = fut => res?,
        }
    }

    #[cfg(not(unix))]
    {
        pin_mut!(ctrl_c, fut);

        tokio::select! {
            _ = ctrl_c => {
                trace!(target: "reth::cli", "Received ctrl-c");
            },
            res = fut => res?,
        }
    }

    Ok(())
}
