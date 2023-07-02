pub(crate) struct LogLevel(pub(crate) reth_interfaces::db::LogLevel);

impl From<LogLevel> for reth_libmdbx::ffi::LogLevel {
    fn from(value: LogLevel) -> Self {
        match value.0 {
            reth_interfaces::db::LogLevel::Fatal => 0,
            reth_interfaces::db::LogLevel::Error => 1,
            reth_interfaces::db::LogLevel::Warn => 2,
            reth_interfaces::db::LogLevel::Notice => 3,
            reth_interfaces::db::LogLevel::Verbose => 4,
            reth_interfaces::db::LogLevel::Debug => 5,
            reth_interfaces::db::LogLevel::Trace => 6,
            reth_interfaces::db::LogLevel::Extra => 7,
        }
    }
}
