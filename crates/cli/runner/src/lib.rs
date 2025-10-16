//! A tokio based CLI runner.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Entrypoint for running commands.

use reth_tasks::{TaskExecutor, TaskManager};
use std::{future::Future, pin::pin, sync::mpsc, time::Duration};
use tokio::runtime::{Handle, Runtime};
use tracing::{debug, error, trace};

/// Executes CLI commands.
///
/// Provides utilities for running a cli command to completion.
#[derive(Debug)]
#[non_exhaustive]
pub struct CliRunner {
    executor: RuntimeOrHandle,
}

impl CliRunner {
    /// Attempts to create a new [`CliRunner`] using the default tokio
    /// [`Runtime`].
    ///
    /// The default tokio runtime is multi-threaded, with both I/O and time drivers enabled.
    pub fn try_default_runtime() -> Result<Self, std::io::Error> {
        Ok(Self { executor: RuntimeOrHandle::Runtime(tokio_runtime()?) })
    }

    /// Create a new [`CliRunner`] from a provided tokio [`Runtime`].
    pub const fn from_runtime(tokio_runtime: Runtime) -> Self {
        Self { executor: RuntimeOrHandle::Runtime(tokio_runtime) }
    }

    /// Create a new [`CliRunner`] from the current tokio runtime handle.
    /// Panics if not called from a tokio runtime context.
    pub fn current() -> Self {
        Self { executor: RuntimeOrHandle::Handle(Handle::current()) }
    }

    /// Try to create a new [`CliRunner`] from the current tokio runtime handle.
    /// It does not panic if not called from a tokio runtime context.
    pub fn try_current() -> Option<Self> {
        Handle::try_current().ok().map(|handle| Self { executor: RuntimeOrHandle::Handle(handle) })
    }

    /// Create a new [`CliRunner`] from a tokio [`Handle`].
    ///
    /// # Warning
    ///
    /// When using a [`Handle`], some operations may panic if called from within
    /// the same runtime context.
    ///
    /// Prefer using [`Self::from_runtime`] when possible.
    ///
    /// # Example
    /// ```ignore
    /// // Use from a separate thread to avoid async context issues
    /// std::thread::spawn(move || {
    ///     let runner = CliRunner::from_handle(handle);
    ///     runner.run_until_ctrl_c_with_handle(fut);
    /// });
    /// ```
    pub const fn from_handle(handle: Handle) -> Self {
        Self { executor: RuntimeOrHandle::Handle(handle) }
    }

    /// Returns the handle reference, regardless of whether this contains a runtime or handle
    pub fn handle(&self) -> &Handle {
        self.executor.handle()
    }

    /// Executes a regular future until completion or until external signal received
    pub fn run_until_ctrl_c_with_handle<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        self.executor.block_on(run_until_ctrl_c(fut)).map_err(E::from)??;
        Ok(())
    }
}

// === impl CliRunner ===

impl CliRunner {
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
        let tokio_runtime = self.executor.into_runtime("async commands")?;
        let AsyncCliRunner { context, mut task_manager, tokio_runtime } =
            AsyncCliRunner::new(tokio_runtime);

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
            task_manager.graceful_shutdown_with_timeout(Duration::from_secs(5));
        }

        // `drop(tokio_runtime)` would block the current thread until its pools
        // (including blocking pool) are shutdown. Since we want to exit as soon as possible, drop
        // it on a separate thread and wait for up to 5 seconds for this operation to
        // complete.
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tokio-runtime-shutdown".to_string())
            .spawn(move || {
                drop(tokio_runtime);
                let _ = tx.send(());
            })
            .unwrap();

        let _ = rx.recv_timeout(Duration::from_secs(5)).inspect_err(|err| {
            debug!(target: "reth::cli", %err, "tokio runtime shutdown timed out");
        });

        command_res
    }

    /// Executes a regular future until completion or until external signal received.
    pub fn run_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>>,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        let tokio_runtime = self.executor.into_runtime("async commands")?;
        tokio_runtime.block_on(run_until_ctrl_c(fut))?;
        Ok(())
    }

    /// Executes a regular future as a spawned blocking task until completion or until external
    /// signal received.
    ///
    /// See [`Runtime::spawn_blocking`](tokio::runtime::Runtime::spawn_blocking) .
    pub fn run_blocking_until_ctrl_c<F, E>(self, fut: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
        E: Send + Sync + From<std::io::Error> + 'static,
    {
        let fut = self.executor.spawn_blocking_task(fut);

        self.executor
            .block_on(run_until_ctrl_c(async move { fut.await.expect("Failed to join task") }))
            .map_err(E::from)??;

        // drop the tokio runtime on a separate thread because drop blocks until its pools
        // (including blocking pool) are shutdown. In other words `drop(tokio_runtime)` would block
        // the current thread but we want to exit right away.
        if let RuntimeOrHandle::Runtime(tokio_runtime) = self.executor {
            std::thread::Builder::new()
                .name("tokio-runtime-shutdown".to_string())
                .spawn(move || drop(tokio_runtime))
                .unwrap();
        }

        Ok(())
    }
}

/// [`CliRunner`] configuration when executing commands asynchronously
struct AsyncCliRunner {
    context: CliContext,
    task_manager: TaskManager,
    tokio_runtime: tokio::runtime::Runtime,
}

// === impl AsyncCliRunner ===

impl AsyncCliRunner {
    /// Given a tokio [`Runtime`], creates additional context required to
    /// execute commands asynchronously.
    fn new(tokio_runtime: tokio::runtime::Runtime) -> Self {
        let task_manager = TaskManager::new(tokio_runtime.handle().clone());
        let task_executor = task_manager.executor();
        Self { context: CliContext { task_executor }, task_manager, tokio_runtime }
    }
}

/// Additional context provided by the [`CliRunner`] when executing commands
#[derive(Debug)]
pub struct CliContext {
    /// Used to execute/spawn tasks
    pub task_executor: TaskExecutor,
}

/// Creates a new default tokio multi-thread [Runtime] with all features
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
        let fut = pin!(fut);
        tokio::select! {
            task_manager_result = tasks => {
                if let Err(panicked_error) = task_manager_result {
                    return Err(panicked_error.into());
                }
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
        let sigterm = pin!(sigterm);
        let ctrl_c = pin!(ctrl_c);
        let fut = pin!(fut);

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
        let ctrl_c = pin!(ctrl_c);
        let fut = pin!(fut);

        tokio::select! {
            _ = ctrl_c => {
                trace!(target: "reth::cli", "Received ctrl-c");
            },
            res = fut => res?,
        }
    }

    Ok(())
}

/// A tokio runtime or handle.
#[derive(Debug)]
enum RuntimeOrHandle {
    /// Owned runtime that can be used for blocking operations
    Runtime(Runtime),
    /// Handle to an existing runtime
    Handle(Handle),
}

impl RuntimeOrHandle {
    /// Returns a reference to the inner tokio runtime handle.
    fn handle(&self) -> &Handle {
        match self {
            Self::Runtime(rt) => rt.handle(),
            Self::Handle(handle) => handle,
        }
    }

    /// Attempts to extract the runtime, returning an error if only a handle is available.
    fn into_runtime(self, operation: &str) -> Result<Runtime, std::io::Error> {
        let (rt, _handle) = self.into_runtime_or_handle();
        rt.ok_or_else(|| {
            std::io::Error::other(
                format!("A tokio runtime is required to run {}. Please create a CliRunner with an owned runtime.", operation)
            )
        })
    }

    /// Chooses to return the owned runtime if it exists, otherwise returns `None` and the handle.
    fn into_runtime_or_handle(self) -> (Option<Runtime>, Handle) {
        match self {
            Self::Runtime(runtime) => {
                let handle = runtime.handle().clone();
                (Some(runtime), handle)
            }
            Self::Handle(handle) => (None, handle),
        }
    }

    /// Block on a future, handling both `Runtime` and `Handle` cases.
    ///
    /// # Example
    /// ```ignore
    /// // Safe: Called from outside async context
    /// std::thread::spawn(move || {
    ///     let result = handle.block_on(async { "ok" });
    /// });
    ///
    /// // Unsafe: Would panic if called directly in async context
    /// // async fn bad() {
    /// //     handle.block_on(async { "panic!" }); // Don't do this!
    /// // }
    /// ```
    fn block_on<F>(&self, fut: F) -> Result<F::Output, std::io::Error>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match self {
            Self::Runtime(rt) => Ok(rt.block_on(fut)),
            Self::Handle(handle) => {
                // Check if we're in an async context to spawn a thread to avoid panic
                if Handle::try_current().is_ok() {
                    let handle = handle.clone();
                    std::thread::spawn(move || handle.block_on(fut))
                        .join()
                        .map_err(|_| std::io::Error::other("Failed to join blocking thread"))
                } else {
                    Ok(handle.block_on(fut))
                }
            }
        }
    }

    /// Spawn a blocking task that runs a future
    fn spawn_blocking_task<F>(&self, fut: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handle = self.handle().clone();
        self.handle().spawn_blocking(move || handle.block_on(fut))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[test]
    fn test_runtime_with_run_until_ctrl_c() {
        let runner = CliRunner::try_default_runtime().unwrap();
        let result = runner.run_until_ctrl_c(async {
            sleep(Duration::from_millis(5)).await;
            Ok::<(), std::io::Error>(())
        });
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_with_run_until_ctrl_c_with_handle() {
        let rt = tokio_runtime().unwrap();
        let handle = rt.handle().clone();

        // Separate thread is used to avoid async context
        let result = std::thread::spawn(move || {
            let runner = CliRunner::from_handle(handle);
            runner.run_until_ctrl_c_with_handle(async { Ok::<(), std::io::Error>(()) })
        })
        .join()
        .unwrap();

        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_with_run_blocking_until_ctrl_c() {
        let rt = tokio_runtime().unwrap();
        let handle = rt.handle().clone();

        let result = std::thread::spawn(move || {
            let runner = CliRunner::from_handle(handle);
            runner.run_blocking_until_ctrl_c(async { Ok::<(), std::io::Error>(()) })
        })
        .join()
        .unwrap();

        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_fails_with_run_until_ctrl_c() {
        let rt = tokio_runtime().unwrap();
        let runner = CliRunner::from_handle(rt.handle().clone());

        // This should fail because `run_until_ctrl_c` needs an owned runtime
        let result = runner.run_until_ctrl_c(async { Ok::<(), std::io::Error>(()) });

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("tokio runtime is required"));
    }
}
