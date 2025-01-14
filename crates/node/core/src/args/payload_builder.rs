use crate::{cli::config::PayloadBuilderConfig, version::default_extra_data};
use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use alloy_eips::{eip1559::ETHEREUM_BLOCK_GAS_LIMIT, merge::SLOT_DURATION};
use clap::{
    builder::{RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use reth_cli_util::{parse_duration_from_secs, parse_duration_from_secs_or_ms};
use std::{borrow::Cow, ffi::OsStr, time::Duration};

/// Parameters for configuring the Payload Builder
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Builder")]
pub struct PayloadBuilderArgs {
    /// Block extra data set by the payload builder.
    #[arg(long = "builder.extradata", value_parser = ExtraDataValueParser::default(), default_value_t = default_extra_data())]
    pub extra_data: String,

    /// Target gas limit for built blocks.
    #[arg(long = "builder.gaslimit", default_value_t = ETHEREUM_BLOCK_GAS_LIMIT, value_name = "GAS_LIMIT")]
    pub gas_limit: u64,

    /// The interval at which the job should build a new payload after the last.
    ///
    /// Interval is specified in seconds or in milliseconds if the value ends with `ms`:
    ///   * `50ms` -> 50 milliseconds
    ///   * `1` -> 1 second
    #[arg(long = "builder.interval", value_parser = parse_duration_from_secs_or_ms, default_value = "1", value_name = "DURATION")]
    pub interval: Duration,

    /// The deadline for when the payload builder job should resolve.
    #[arg(long = "builder.deadline", value_parser = parse_duration_from_secs, default_value = "12", value_name = "SECONDS")]
    pub deadline: Duration,

    /// Maximum number of tasks to spawn for building a payload.
    #[arg(long = "builder.max-tasks", default_value = "3", value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
    pub max_payload_tasks: usize,
}

impl Default for PayloadBuilderArgs {
    fn default() -> Self {
        Self {
            extra_data: default_extra_data(),
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            interval: Duration::from_secs(1),
            deadline: SLOT_DURATION,
            max_payload_tasks: 3,
        }
    }
}

impl PayloadBuilderConfig for PayloadBuilderArgs {
    fn extra_data(&self) -> Cow<'_, str> {
        self.extra_data.as_str().into()
    }

    fn interval(&self) -> Duration {
        self.interval
    }

    fn deadline(&self) -> Duration {
        self.deadline
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn max_payload_tasks(&self) -> usize {
        self.max_payload_tasks
    }
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct ExtraDataValueParser;

impl TypedValueParser for ExtraDataValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        if val.len() > MAXIMUM_EXTRA_DATA_SIZE {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!(
                    "Payload builder extradata size exceeds {MAXIMUM_EXTRA_DATA_SIZE}-byte limit"
                ),
            ))
        }
        Ok(val.to_string())
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
    fn test_args_with_valid_max_tasks() {
        let args =
            CommandParser::<PayloadBuilderArgs>::parse_from(["reth", "--builder.max-tasks", "1"])
                .args;
        assert_eq!(args.max_payload_tasks, 1)
    }

    #[test]
    fn test_args_with_invalid_max_tasks() {
        assert!(CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.max-tasks",
            "0"
        ])
        .is_err());
    }

    #[test]
    fn test_default_extra_data() {
        let extra_data = default_extra_data();
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.extradata",
            extra_data.as_str(),
        ])
        .args;
        assert_eq!(args.extra_data, extra_data);
    }

    #[test]
    fn test_invalid_extra_data() {
        let extra_data = "x".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1);
        let args = CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            extra_data.as_str(),
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn payload_builder_args_default_sanity_check() {
        let default_args = PayloadBuilderArgs::default();
        let args = CommandParser::<PayloadBuilderArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_args_with_s_interval() {
        let args =
            CommandParser::<PayloadBuilderArgs>::parse_from(["reth", "--builder.interval", "50"])
                .args;
        assert_eq!(args.interval, Duration::from_secs(50));
    }

    #[test]
    fn test_args_with_ms_interval() {
        let args =
            CommandParser::<PayloadBuilderArgs>::parse_from(["reth", "--builder.interval", "50ms"])
                .args;
        assert_eq!(args.interval, Duration::from_millis(50));
    }
}
