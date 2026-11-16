/// Verbosity of a metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
pub struct Level(LevelInner);

impl Level {
    /// The "trace" level.
    pub const TRACE: Self = Self(LevelInner::Trace);

    /// The "debug" level.
    pub const DEBUG: Self = Self(LevelInner::Debug);

    /// The "info" level.
    pub const INFO: Self = Self(LevelInner::Info);

    /// The "warn" level.
    pub const WARN: Self = Self(LevelInner::Warn);

    /// The "error" level.
    pub const ERROR: Self = Self(LevelInner::Error);
}

impl std::convert::TryFrom<&str> for Level {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim() {
            "trace" | "TRACE" => Ok(Level::TRACE),
            "debug" | "DEBUG" => Ok(Level::DEBUG),
            "info" | "INFO" => Ok(Level::INFO),
            "warn" | "WARN" => Ok(Level::WARN),
            "error" | "ERROR" => Ok(Level::ERROR),
            unknown => Err(format!("unknown log level: {} (expected one of 'trace', 'debug', 'info', 'warn', or 'error')", unknown)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LevelInner {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

/// Metadata describing a metric.
///
/// All metrics have the following metadata:
///
/// - A [`target`](Metadata::target), a string that categorizes part of the system where metric originates from. The
///   `metrics`` macros default to using the module path where the metric originate as the target, but it may be
///   overridden.
/// - A [`level`](Metadata::level), specifying the verbosity the metric is emitted at.
///
/// In addition, the following optional metadata describing the source code location where the metric originated from
/// may be provided:
///
/// - The [module path](Metadata::module_path) of the source code location where the metric event originated.
///
/// Metadata usage is exporter-specific, and may be ignored entirely. See the documentation of the specific exporter
/// being used for more information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata<'a> {
    target: &'a str,
    level: Level,
    module_path: Option<&'a str>,
}

impl<'a> Metadata<'a> {
    /// Constructs a new [`Metadata`].
    pub const fn new(target: &'a str, level: Level, module_path: Option<&'a str>) -> Self {
        Self { target, level, module_path }
    }

    /// Returns the verbosity level of the metric.
    pub fn level(&self) -> &Level {
        &self.level
    }

    /// Returns the target of the metric.
    ///
    /// This specifies the part of the system where the metric originates from. Typically, this is the module path where
    /// the metric originated from, but can be overridden when registering a metric.
    pub fn target(&self) -> &'a str {
        self.target
    }

    /// Returns the module path of the metric.
    ///
    /// This specifies the module where the metric originates from, or `None` if the module path is unknown.
    pub fn module_path(&self) -> Option<&'a str> {
        self.module_path
    }
}

#[cfg(test)]
mod tests {
    use std::convert::TryFrom as _;

    use super::*;

    #[test]
    fn level_try_from_valid() {
        let cases = &[
            ("trace", Level::TRACE),
            ("TRACE", Level::TRACE),
            ("debug", Level::DEBUG),
            ("DEBUG", Level::DEBUG),
            ("info", Level::INFO),
            ("INFO", Level::INFO),
            ("warn", Level::WARN),
            ("WARN", Level::WARN),
            ("error", Level::ERROR),
            ("ERROR", Level::ERROR),
        ];

        for (input, expected) in cases {
            assert_eq!(Level::try_from(*input).unwrap(), *expected);

            // Now try with some whitespace on either end.
            let input_whitespace = format!("  {}  ", input);
            assert_eq!(Level::try_from(&*input_whitespace).unwrap(), *expected);
        }
    }

    #[test]
    fn level_try_from_invalid() {
        let cases = &["", "foo", "bar", "baz", "qux", "quux"];

        for input in cases {
            assert!(Level::try_from(*input).is_err());
        }
    }

    #[test]
    fn level_ordering() {
        // A few manual comparisons because it makes me feel better:
        assert!(Level::TRACE < Level::DEBUG);
        assert!(Level::DEBUG < Level::INFO);
        assert!(Level::ERROR > Level::DEBUG);
        assert!(Level::WARN == Level::WARN);

        // Now check each level programmatically.
        let levels = &[Level::TRACE, Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR];

        for i in 0..levels.len() {
            let current_level = levels[i];
            let lower_levels = &levels[..i];
            let higher_levels = &levels[i + 1..];

            for lower_level in lower_levels {
                assert!(current_level > *lower_level);
                assert!(*lower_level < current_level);
            }

            for higher_level in higher_levels {
                assert!(current_level < *higher_level);
                assert!(*higher_level > current_level);
            }
        }
    }
}
