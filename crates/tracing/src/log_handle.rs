//! Global log handle for runtime filter changes.
//!
//! Provides a single global [`LogFilterHandle`] that collects reload handles from all
//! reloadable layers (stdout, file, OTLP traces, and OTLP logs). `set_log_verbosity` and
//! `set_log_vmodule` update every registered layer in one shot.

use tracing::level_filters::LevelFilter;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// Type alias for a single layer's reload handle.
pub type LogFilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Reloadable tracing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogFilterTarget {
    /// Standard output logs.
    Stdout,
    /// File logs.
    File,
    /// OTLP traces.
    OtlpTraces,
    /// OTLP logs.
    OtlpLogs,
}

#[derive(Debug)]
struct ReloadableFilter {
    target: LogFilterTarget,
    handle: LogFilterReloadHandle,
    startup_filter: EnvFilter,
}

/// Collects reload handles so all layers can be updated together.
#[derive(Debug)]
pub struct LogFilterHandle {
    filters: Vec<ReloadableFilter>,
}

impl LogFilterHandle {
    /// Creates a new, empty handle collection.
    const fn new() -> Self {
        Self { filters: Vec::new() }
    }

    /// Adds a reload handle for a layer.
    fn push(
        &mut self,
        target: LogFilterTarget,
        handle: LogFilterReloadHandle,
        startup_filter: EnvFilter,
    ) {
        self.filters.push(ReloadableFilter { target, handle, startup_filter });
    }

    /// Returns `true` if at least one handle is registered.
    const fn is_available(&self) -> bool {
        !self.filters.is_empty()
    }

    /// Returns the unique targets with registered reload handles.
    fn available_targets(&self) -> Vec<LogFilterTarget> {
        let mut targets = Vec::new();
        for filter in &self.filters {
            if !targets.contains(&filter.target) {
                targets.push(filter.target);
            }
        }
        targets
    }

    /// Reloads every registered layer with a fresh filter built by `make_filter`.
    fn reload_all(
        &self,
        make_filter: impl Fn() -> Result<EnvFilter, String>,
    ) -> Result<(), String> {
        for reloadable in &self.filters {
            let filter = make_filter()?;
            reloadable.handle.reload(filter).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Reloads layers matching one of the requested targets.
    fn reload_targets(
        &self,
        targets: &[LogFilterTarget],
        make_filter: impl Fn() -> Result<EnvFilter, String>,
    ) -> Result<(), String> {
        for reloadable in self.filters.iter().filter(|filter| targets.contains(&filter.target)) {
            let filter = make_filter()?;
            reloadable.handle.reload(filter).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Restores every registered layer to its own startup directives.
    fn reset_all(&self) -> Result<(), String> {
        for reloadable in &self.filters {
            reloadable
                .handle
                .reload(reloadable.startup_filter.clone())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Restores matching layers to their own startup directives.
    fn reset_targets(&self, targets: &[LogFilterTarget]) -> Result<(), String> {
        for reloadable in self.filters.iter().filter(|filter| targets.contains(&filter.target)) {
            reloadable
                .handle
                .reload(reloadable.startup_filter.clone())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// Single global log handle shared by all reloadable layers.
static LOG_HANDLE: std::sync::Mutex<LogFilterHandle> =
    std::sync::Mutex::new(LogFilterHandle::new());

/// Registers a reload handle for a layer with an INFO reset baseline.
///
/// Can be called multiple times — each handle is appended.
pub fn install_log_handle(handle: LogFilterReloadHandle) {
    install_log_handle_with_target(
        LogFilterTarget::Stdout,
        handle,
        EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).parse_lossy(""),
    );
}

/// Registers a reload handle and the filter that should be restored on reset.
pub fn install_log_handle_with_baseline(handle: LogFilterReloadHandle, startup_filter: EnvFilter) {
    install_log_handle_with_target(LogFilterTarget::Stdout, handle, startup_filter);
}

/// Registers a targeted reload handle and the filter that should be restored on reset.
pub fn install_log_handle_with_target(
    target: LogFilterTarget,
    handle: LogFilterReloadHandle,
    startup_filter: EnvFilter,
) {
    LOG_HANDLE.lock().expect("log handle poisoned").push(target, handle, startup_filter);
}

/// Returns `true` if at least one global log handle is available.
pub fn log_handle_available() -> bool {
    LOG_HANDLE.lock().expect("log handle poisoned").is_available()
}

/// Returns the tracing targets with registered reload handles.
pub fn available_log_filter_targets() -> Vec<LogFilterTarget> {
    LOG_HANDLE.lock().expect("log handle poisoned").available_targets()
}

/// Restores every reloadable layer to the tracing directives it started with.
pub fn reset_log_filters() -> Result<(), String> {
    let guard = LOG_HANDLE.lock().expect("log handle poisoned");
    if !guard.is_available() {
        return Err("Log filter reload not available".to_string());
    }
    guard.reset_all()
}

/// Restores the requested reloadable layers to their startup filters.
pub fn reset_log_filters_for_targets(targets: &[LogFilterTarget]) -> Result<(), String> {
    let guard = LOG_HANDLE.lock().expect("log handle poisoned");
    if !guard.is_available() {
        return Err("Log filter reload not available".to_string());
    }
    guard.reset_targets(targets)
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
/// Updates all reloadable tracing layers.
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
/// Updates all reloadable tracing layers.
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

/// Sets module-specific log levels for the requested reloadable layers.
pub fn set_log_vmodule_for_targets(
    pattern: &str,
    targets: &[LogFilterTarget],
) -> Result<(), String> {
    let guard = LOG_HANDLE.lock().expect("log handle poisoned");
    if !guard.is_available() {
        return Err("Log filter reload not available".to_string());
    }

    EnvFilter::try_new(pattern).map_err(|e| format!("Invalid filter pattern: {e}"))?;
    guard.reload_targets(targets, || {
        EnvFilter::try_new(pattern).map_err(|e| format!("Invalid filter pattern: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_reload_and_reset_only_affect_targeted_layers() {
        let (_stdout_layer, stdout_handle): (_, LogFilterReloadHandle) =
            reload::Layer::new(EnvFilter::try_new("info,reth=debug").unwrap());
        let (_otlp_layer, otlp_handle): (_, LogFilterReloadHandle) =
            reload::Layer::new(EnvFilter::try_new("warn,reth=trace").unwrap());
        let stdout_baseline = stdout_handle.with_current(Clone::clone).unwrap();
        let otlp_baseline = otlp_handle.with_current(Clone::clone).unwrap();

        let mut filters = LogFilterHandle::new();
        filters.push(LogFilterTarget::Stdout, stdout_handle.clone(), stdout_baseline.clone());
        filters.push(LogFilterTarget::OtlpTraces, otlp_handle.clone(), otlp_baseline.clone());
        filters
            .reload_targets(&[LogFilterTarget::Stdout], || {
                EnvFilter::try_new("trace").map_err(|err| err.to_string())
            })
            .unwrap();
        assert_eq!(stdout_handle.with_current(ToString::to_string).unwrap(), "trace");
        assert_eq!(
            otlp_handle.with_current(ToString::to_string).unwrap(),
            otlp_baseline.to_string()
        );

        filters.reset_targets(&[LogFilterTarget::Stdout]).unwrap();

        assert_eq!(
            stdout_handle.with_current(ToString::to_string).unwrap(),
            stdout_baseline.to_string()
        );
        assert_eq!(
            otlp_handle.with_current(ToString::to_string).unwrap(),
            otlp_baseline.to_string()
        );
    }
}
