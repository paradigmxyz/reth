/// Log level.
///
/// Levels greater than [LogLevel::Notice] require libmdbx built with `MDBX_DEBUG` option.
pub enum LogLevel {
    /// Critical conditions, i.e. assertion failures.
    Fatal,
    /// Enables logging for error conditions.
    Error,
    /// Enables logging for warning conditions.
    Warn,
    /// Enables logging for normal but significant condition.
    Notice,
    /// Enables logging for verbose informational.
    Verbose,
    /// Enables logging for debug-level messages.
    Debug,
    /// Enables logging for trace debug-level messages.
    Trace,
    /// Enables extra debug-level messages (dump pgno lists).
    Extra,
}

impl From<LogLevel> for reth_libmdbx::ffi::LogLevel {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Fatal => 0,
            LogLevel::Error => 1,
            LogLevel::Warn => 2,
            LogLevel::Notice => 3,
            LogLevel::Verbose => 4,
            LogLevel::Debug => 5,
            LogLevel::Trace => 6,
            LogLevel::Extra => 7,
        }
    }
}

impl From<reth_interfaces::db::LogLevel> for LogLevel {
    fn from(value: reth_interfaces::db::LogLevel) -> Self {
        match value {
            reth_interfaces::db::LogLevel::Fatal => LogLevel::Fatal,
            reth_interfaces::db::LogLevel::Error => LogLevel::Error,
            reth_interfaces::db::LogLevel::Warn => LogLevel::Warn,
            reth_interfaces::db::LogLevel::Notice => LogLevel::Notice,
            reth_interfaces::db::LogLevel::Verbose => LogLevel::Verbose,
            reth_interfaces::db::LogLevel::Debug => LogLevel::Debug,
            reth_interfaces::db::LogLevel::Trace => LogLevel::Trace,
            reth_interfaces::db::LogLevel::Extra => LogLevel::Extra,
        }
    }
}
