//! Global log handle for runtime filter changes.
//!
//! Provides a single global [`LogFilterHandle`] that collects reload handles from all
//! reloadable layers (stdout, file, etc.). `set_log_verbosity` and `set_log_vmodule`
//! update every registered layer in one shot.

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// Type alias for a single layer's reload handle.
pub type LogFilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Collects reload handles so all layers can be updated together.
#[derive(Debug)]
pub struct LogFilterHandle {
    handles: Vec<LogFilterReloadHandle>,
}

impl LogFilterHandle {
    /// Creates a new, empty handle collection.
    const fn new() -> Self {
        Self { handles: Vec::new() }
    }

    /// Adds a reload handle for a layer.
    fn push(&mut self, handle: LogFilterReloadHandle) {
        self.handles.push(handle);
    }

    /// Returns `true` if at least one handle is registered.
    const fn is_available(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Reloads every registered layer with a fresh filter built by `make_filter`.
    fn reload_all(
        &self,
        make_filter: impl Fn() -> Result<EnvFilter, String>,
    ) -> Result<(), String> {
        for handle in &self.handles {
            let filter = make_filter()?;
            handle.reload(filter).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Single global log handle shared by all reloadable layers.
static LOG_HANDLE: std::sync::Mutex<LogFilterHandle> =
    std::sync::Mutex::new(LogFilterHandle::new());

/// Registers a reload handle for a layer (stdout, file, etc.).
///
/// Can be called multiple times — each handle is appended.
pub fn install_log_handle(handle: LogFilterReloadHandle) {
    LOG_HANDLE.lock().expect("log handle poisoned").push(handle);
}

/// Returns `true` if at least one global log handle is available.
pub fn log_handle_available() -> bool {
    LOG_HANDLE.lock().expect("log handle poisoned").is_available()
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
/// Updates all reloadable layers (stdout, file, etc.).
///
/// Returns an error if no log handle is installed or if the reload fails.
pub fn set_log_verbosity(level: usize) -> Result<(), String> {
    let guard = LOG_HANDLE.lock().expect("log handle poisoned");

    if !guard.is_available() {
        return Err("Log filter reload not available".to_string());
    }

    let level_filter = match level {
        0 => LevelFilter::OFF,
        1 => LevelFilter::ERROR,
        2 => LevelFilter::WARN,
        3 => LevelFilter::INFO,
        4 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    guard.reload_all(|| {
        Ok(EnvFilter::builder().with_default_directive(level_filter.into()).parse_lossy(""))
    })
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
/// Updates all reloadable layers (stdout, file, etc.).
///
/// Returns an error if no log handle is installed or if parsing fails.
pub fn set_log_vmodule(pattern: &str) -> Result<(), String> {
    let guard = LOG_HANDLE.lock().expect("log handle poisoned");

    if !guard.is_available() {
        return Err("Log filter reload not available".to_string());
    }

    if pattern.trim().is_empty() {
        guard.reload_all(|| {
            Ok(EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .parse_lossy(""))
        })
    } else {
        // Validate the pattern once before reloading all handles
        EnvFilter::try_new(pattern).map_err(|e| format!("Invalid filter pattern: {e}"))?;
        guard.reload_all(|| {
            EnvFilter::try_new(pattern).map_err(|e| format!("Invalid filter pattern: {e}"))
        })
    }
}
