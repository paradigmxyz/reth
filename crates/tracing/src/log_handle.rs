//! Global log handle for runtime filter changes.
//!
//! Provides a single global [`LogFilterHandle`] that collects reload handles from all
//! reloadable layers (stdout, file, etc.). `set_log_verbosity` and `set_log_vmodule`
//! update every registered layer in one shot.

use crate::layers::DEFAULT_ENV_FILTER_DIRECTIVES;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{reload, EnvFilter, Registry};

/// Type alias for a single layer's reload handle.
pub type LogFilterReloadHandle = reload::Handle<EnvFilter, Registry>;

/// Collects reload handles so all layers can be updated together.
#[derive(Debug)]
pub struct LogFilterHandle {
    /// Each layer's handle and the filter it started with, so an empty pattern restores it.
    handles: Vec<(LogFilterReloadHandle, String)>,
}

impl LogFilterHandle {
    /// Creates a new, empty handle collection.
    const fn new() -> Self {
        Self { handles: Vec::new() }
    }

    /// Adds a reload handle for a layer.
    fn push(&mut self, handle: LogFilterReloadHandle) {
        let initial = handle.with_current(|filter| filter.to_string()).unwrap_or_default();
        self.handles.push((handle, initial));
    }

    /// Returns `true` if at least one handle is registered.
    const fn is_available(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Reloads every registered layer with a fresh filter built by `make_filter`, which receives
    /// that layer's initial filter.
    fn reload_all(
        &self,
        make_filter: impl Fn(&str) -> Result<EnvFilter, String>,
    ) -> Result<(), String> {
        for (handle, initial) in &self.handles {
            let filter = make_filter(initial)?;
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
/// Updates all reloadable layers (stdout, file, etc.). The default directives that suppress noisy
/// dependencies are kept, as they are at startup.
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

    guard.reload_all(|_| Ok(verbosity_filter(level_filter)))
}

/// Prepends the default directives to `pattern`, as startup does, so they still apply unless
/// `pattern` names the same target.
fn with_default_directives(pattern: &str) -> String {
    DEFAULT_ENV_FILTER_DIRECTIVES
        .iter()
        .copied()
        .chain(pattern.split(',').map(str::trim).filter(|directive| !directive.is_empty()))
        .collect::<Vec<_>>()
        .join(",")
}

/// The filter `set_log_verbosity` installs: `level` over the default directives. A builder's
/// default directive applies only to an empty directive string, so the defaults are added after.
fn verbosity_filter(level: LevelFilter) -> EnvFilter {
    let filter = EnvFilter::builder().with_default_directive(level.into()).parse_lossy("");
    DEFAULT_ENV_FILTER_DIRECTIVES
        .iter()
        .filter_map(|directive| directive.parse().ok())
        .fold(filter, EnvFilter::add_directive)
}

/// The filter `set_log_vmodule` installs for a non-empty `pattern`: the default directives, then
/// `pattern`.
fn vmodule_filter(pattern: &str) -> Result<EnvFilter, String> {
    EnvFilter::try_new(with_default_directives(pattern))
        .map_err(|e| format!("Invalid filter pattern: {e}"))
}

/// Sets module-specific log levels using a pattern string.
///
/// Pattern format follows the `RUST_LOG` environment variable syntax:
/// - `module1=level1,module2=level2`
/// - Example: `reth::sync=debug,reth::net=trace`
/// - Example: `info,reth::stages=debug`
///
/// The default directives that suppress noisy dependencies are kept, as they are at startup. An
/// empty string restores each layer's initial filter.
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
        // The initial filter is an `EnvFilter` rendered back to its directive list.
        guard.reload_all(|initial| EnvFilter::try_new(initial).map_err(|e| e.to_string()))
    } else {
        // Validate the pattern once before reloading all handles
        vmodule_filter(pattern)?;
        guard.reload_all(|_| vmodule_filter(pattern))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{
        layer::{Context, SubscriberExt},
        Layer,
    };

    /// Records the target of every event it sees.
    struct Targets(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Targets {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            self.0.lock().unwrap().push(event.metadata().target().to_owned());
        }
    }

    /// Emits a `discv5` debug event (off by default) and two events that a debug filter keeps.
    fn emit() {
        tracing::debug!(target: "discv5::unit", "suppressed by default");
        tracing::warn!(target: "discv5::unit", "suppressed by default");
        tracing::debug!(target: "reload_test", "kept");
    }

    fn seen_through(filter: EnvFilter) -> Vec<String> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Targets(Arc::clone(&seen)).with_filter(filter));
        tracing::subscriber::with_default(subscriber, emit);
        Arc::into_inner(seen).expect("the subscriber is dropped").into_inner().unwrap()
    }

    #[test]
    fn a_reloaded_filter_keeps_the_default_directives() {
        assert_eq!(seen_through(verbosity_filter(LevelFilter::DEBUG)), ["reload_test"]);
        assert_eq!(seen_through(vmodule_filter("debug").unwrap()), ["reload_test"]);
        assert_eq!(
            seen_through(vmodule_filter("debug,discv5=debug").unwrap()),
            ["discv5::unit", "discv5::unit", "reload_test"]
        );
        assert!(vmodule_filter("reload_test=nonsense").is_err());
    }

    #[test]
    fn an_empty_pattern_restores_the_initial_filter() {
        let initial = EnvFilter::try_new("warn,reload_test=debug").unwrap();
        let (filter, handle) = reload::Layer::new(initial);
        install_log_handle(handle);

        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Targets(Arc::clone(&seen)).with_filter(filter));
        tracing::subscriber::with_default(subscriber, || {
            set_log_vmodule("error").unwrap();
            emit();
            set_log_vmodule("").unwrap();
            emit();
        });

        // Under "error" nothing passes; the restored filter lets `reload_test` debug and the
        // `discv5` warning through, exactly as it did before the first reload.
        assert_eq!(*seen.lock().unwrap(), ["discv5::unit", "reload_test"]);
    }
}
