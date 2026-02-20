//! Global log level handle for runtime filter changes.
//!
//! Provides functions to install and use a global [`LogFilterReloadHandle`] that allows
//! changing log filters at runtime, used by RPC methods like `debug_verbosity` and
//! `debug_vmodule`.

use std::sync::OnceLock;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// Type alias for the reload handle used to dynamically update log filters.
pub type LogFilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Global log level handle for runtime filter changes.
static GLOBAL_LOG_HANDLE: OnceLock<LogFilterReloadHandle> = OnceLock::new();

/// Installs the global log level handle.
/// Returns `true` if the handle was installed, `false` if one was already installed.
pub fn install_log_handle(handle: LogFilterReloadHandle) -> bool {
    GLOBAL_LOG_HANDLE.set(handle).is_ok()
}

/// Returns `true` if a global log handle is available.
pub fn log_handle_available() -> bool {
    GLOBAL_LOG_HANDLE.get().is_some()
}

/// Sets the global log verbosity level.
///
/// - 0: OFF
/// - 1: ERROR
/// - 2: WARN
/// - 3: INFO
/// - 4: DEBUG
/// - 5+: TRACE
///
/// Returns an error if no log handle is installed or if the reload fails.
pub fn set_log_verbosity(level: usize) -> Result<(), String> {
    let Some(handle) = GLOBAL_LOG_HANDLE.get() else {
        return Err("Log filter reload not available".to_string());
    };

    let level_filter = match level {
        0 => LevelFilter::OFF,
        1 => LevelFilter::ERROR,
        2 => LevelFilter::WARN,
        3 => LevelFilter::INFO,
        4 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Use parse_lossy to avoid reading from RUST_LOG env var
    let filter = EnvFilter::builder().with_default_directive(level_filter.into()).parse_lossy("");

    handle.reload(filter).map_err(|e| e.to_string())
}

/// Sets module-specific log levels using a pattern string.
///
/// Pattern format follows the `RUST_LOG` environment variable syntax:
/// - `module1=level1,module2=level2`
/// - Example: `reth::sync=debug,reth::net=trace`
/// - Example: `info,reth::stages=debug`
///
/// An empty string resets the filter to the default level (INFO).
///
/// Returns an error if no log handle is installed or if parsing fails.
pub fn set_log_vmodule(pattern: &str) -> Result<(), String> {
    let Some(handle) = GLOBAL_LOG_HANDLE.get() else {
        return Err("Log filter reload not available".to_string());
    };

    let filter = if pattern.trim().is_empty() {
        // Reset to default INFO level when pattern is empty
        EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).parse_lossy("")
    } else {
        EnvFilter::try_new(pattern).map_err(|e| format!("Invalid filter pattern: {e}"))?
    };

    handle.reload(filter).map_err(|e| e.to_string())
}
