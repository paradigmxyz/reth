//! clap [Args](clap::Args) for database configuration

use clap::Args;
use reth_interfaces::db::LogLevel;

use crate::version::default_client_version;

/// Parameters for database configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone, Copy)]
#[command(next_help_heading = "Database")]
pub struct DatabaseArgs {
    /// Database logging level. Levels higher than "notice" require a debug build.
    #[arg(skip)]
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

    /// Parses command-line arguments into a DatabaseArgs struct.
    pub fn parse() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::parse_from_args(&args)
    }

    /// Parses a slice of arguments into a DatabaseArgs struct.
    pub fn parse_from_args(args: &[String]) -> Self {
        let mut log_level = None;
        let mut exclusive = None;

        for arg in args.iter() {
            if arg.starts_with("--db.log-level=") {
                if let Some(level_str) = arg.strip_prefix("--db.log-level=") {
                    log_level = LogLevel::from_str(level_str);
                }
            } else if arg == "--db.exclusive" {
                exclusive = Some(true);
            }
        }

        DatabaseArgs { log_level, exclusive }
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
    fn test_parse_log_level_fatal() {
        let args = DatabaseArgs::parse_from_args(&[
            "reth".to_string(),
            "--db.log-level=fatal".to_string(),
        ]);
        assert_eq!(args.log_level, Some(LogLevel::Fatal));
    }

    #[test]
    fn test_parse_log_level_debug() {
        let args = DatabaseArgs::parse_from_args(&[
            "reth".to_string(),
            "--db.log-level=debug".to_string(),
        ]);
        assert_eq!(args.log_level, Some(LogLevel::Debug));
    }

    #[test]
    fn test_parse_exclusive() {
        let args =
            DatabaseArgs::parse_from_args(&["reth".to_string(), "--db.exclusive".to_string()]);
        assert_eq!(args.exclusive, Some(true));
    }

    #[test]
    fn test_parse_combined_args() {
        let args = DatabaseArgs::parse_from_args(&[
            "reth".to_string(),
            "--db.log-level=warn".to_string(),
            "--db.exclusive".to_string(),
        ]);
        assert_eq!(args.log_level, Some(LogLevel::Warn));
        assert_eq!(args.exclusive, Some(true));
    }

    #[test]
    fn test_invalid_log_level() {
        let args = DatabaseArgs::parse_from_args(&[
            "reth".to_string(),
            "--db.log-level=invalid".to_string(),
        ]);
        assert_eq!(args.log_level, None);
    }
}
