//! clap [Args](clap::Args) for database configuration

use crate::version::default_client_version;
use clap::{
    builder::{PossibleValue, TypedValueParser},
    error::ErrorKind,
    Arg, Args, Command, Error,
};
use reth_storage_errors::db::LogLevel;

/// Parameters for database configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "Database")]
pub struct DatabaseArgs {
    /// Database logging level. Levels higher than "notice" require a debug build.
    #[arg(long = "db.log-level", value_parser = LogLevelValueParser::default())]
    pub log_level: Option<LogLevel>,
    /// Open environment in exclusive/monopolistic mode. Makes it possible to open a database on an
    /// NFS volume.
    #[arg(long = "db.exclusive")]
    pub exclusive: Option<bool>,
}

impl DatabaseArgs {
    /// Returns default database arguments with configured log level and client version.
    pub fn database_args(&self) -> reth_db::mdbx::DatabaseArguments {
        reth_db::mdbx::DatabaseArguments::new(default_client_version())
            .with_log_level(self.log_level)
            .with_exclusive(self.exclusive)
    }
}

/// clap value parser for [`LogLevel`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct LogLevelValueParser;

impl TypedValueParser for LogLevelValueParser {
    type Value = LogLevel;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, Error> {
        let val =
            value.to_str().ok_or_else(|| Error::raw(ErrorKind::InvalidUtf8, "Invalid UTF-8"))?;

        val.parse::<LogLevel>().map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = LogLevel::value_variants()
                .iter()
                .map(|v| format!("- {:?}: {}", v, v.help_message()))
                .collect::<Vec<_>>()
                .join("\n");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    Possible values:\n{possible_values}"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = LogLevel::value_variants()
            .iter()
            .map(|v| PossibleValue::new(v.variant_name()).help(v.help_message()));
        Some(Box::new(values))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_default_database_args() {
        let default_args = DatabaseArgs::default();
        let args = CommandParser::<DatabaseArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_possible_values() {
        // Initialize the LogLevelValueParser
        let parser = LogLevelValueParser;

        // Call the possible_values method
        let possible_values: Vec<PossibleValue> = parser.possible_values().unwrap().collect();

        // Expected possible values
        let expected_values = vec![
            PossibleValue::new("fatal")
                .help("Enables logging for critical conditions, i.e. assertion failures"),
            PossibleValue::new("error").help("Enables logging for error conditions"),
            PossibleValue::new("warn").help("Enables logging for warning conditions"),
            PossibleValue::new("notice")
                .help("Enables logging for normal but significant condition"),
            PossibleValue::new("verbose").help("Enables logging for verbose informational"),
            PossibleValue::new("debug").help("Enables logging for debug-level messages"),
            PossibleValue::new("trace").help("Enables logging for trace debug-level messages"),
            PossibleValue::new("extra").help("Enables logging for extra debug-level messages"),
        ];

        // Check that the possible values match the expected values
        assert_eq!(possible_values.len(), expected_values.len());
        for (actual, expected) in possible_values.iter().zip(expected_values.iter()) {
            assert_eq!(actual.get_name(), expected.get_name());
            assert_eq!(actual.get_help(), expected.get_help());
        }
    }

    #[test]
    fn test_command_parser_with_valid_log_level() {
        let cmd =
            CommandParser::<DatabaseArgs>::try_parse_from(["reth", "--db.log-level", "Debug"])
                .unwrap();
        assert_eq!(cmd.args.log_level, Some(LogLevel::Debug));
    }

    #[test]
    fn test_command_parser_with_invalid_log_level() {
        let result =
            CommandParser::<DatabaseArgs>::try_parse_from(["reth", "--db.log-level", "invalid"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_parser_without_log_level() {
        let cmd = CommandParser::<DatabaseArgs>::try_parse_from(["reth"]).unwrap();
        assert_eq!(cmd.args.log_level, None);
    }
}
