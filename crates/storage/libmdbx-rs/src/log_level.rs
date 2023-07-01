pub enum LogLevel {
    Fatal,
    Error,
    Warn,
    Notice,
    Verbose,
    Debug,
    Trace,
    Extra,
}

impl From<LogLevel> for ffi::MDBX_log_level_t {
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
