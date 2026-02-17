use crate::{cli::config::PayloadBuilderConfig, version::default_extra_data};
use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use clap::{
    builder::{RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use reth_cli_util::{
    parse_duration_from_secs, parse_duration_from_secs_or_ms,
    parsers::format_duration_as_secs_or_ms,
};
use std::{borrow::Cow, ffi::OsStr, sync::OnceLock, time::Duration};

/// Global static payload builder defaults
static PAYLOAD_BUILDER_DEFAULTS: OnceLock<DefaultPayloadBuilderValues> = OnceLock::new();

/// Default values for payload builder that can be customized
///
/// Global defaults can be set via [`DefaultPayloadBuilderValues::try_init`].
#[derive(Debug, Clone)]
pub struct DefaultPayloadBuilderValues {
    /// Default extra data for blocks
    extra_data: String,
    /// Default interval between payload builds in seconds
    interval: String,
    /// Default deadline for payload builds in seconds
    deadline: String,
    /// Default maximum number of concurrent payload building tasks
    max_payload_tasks: usize,
}

impl DefaultPayloadBuilderValues {
    /// Initialize the global payload builder defaults with this configuration
    pub fn try_init(self) -> Result<(), Self> {
        PAYLOAD_BUILDER_DEFAULTS.set(self)
    }

    /// Get a reference to the global payload builder defaults
    pub fn get_global() -> &'static Self {
        PAYLOAD_BUILDER_DEFAULTS.get_or_init(Self::default)
    }

    /// Set the default extra data
    pub fn with_extra_data(mut self, v: impl Into<String>) -> Self {
        self.extra_data = v.into();
        self
    }

    /// Set the default interval in seconds
    pub fn with_interval(mut self, v: Duration) -> Self {
        self.interval = format_duration_as_secs_or_ms(v);
        self
    }

    /// Set the default deadline in seconds
    pub fn with_deadline(mut self, v: u64) -> Self {
        self.deadline = format!("{}", v);
        self
    }

    /// Set the default maximum payload tasks
    pub const fn with_max_payload_tasks(mut self, v: usize) -> Self {
        self.max_payload_tasks = v;
        self
    }
}

impl Default for DefaultPayloadBuilderValues {
    fn default() -> Self {
        Self {
            extra_data: default_extra_data(),
            interval: "1".to_string(),
            deadline: "12".to_string(),
            max_payload_tasks: 3,
        }
    }
}

/// Parameters for configuring the Payload Builder
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Builder")]
pub struct PayloadBuilderArgs {
    /// Block extra data set by the payload builder.
    #[arg(
        long = "builder.extradata",
        value_parser = ExtraDataValueParser::default(),
        default_value_t = DefaultPayloadBuilderValues::get_global().extra_data.clone()
    )]
    pub extra_data: String,

    /// Target gas limit for built blocks.
    #[arg(long = "builder.gaslimit", alias = "miner.gaslimit", value_name = "GAS_LIMIT")]
    pub gas_limit: Option<u64>,

    /// The interval at which the job should build a new payload after the last.
    ///
    /// Interval is specified in seconds or in milliseconds if the value ends with `ms`:
    ///   * `50ms` -> 50 milliseconds
    ///   * `1` -> 1 second
    #[arg(
        long = "builder.interval",
        value_parser = parse_duration_from_secs_or_ms,
        default_value = DefaultPayloadBuilderValues::get_global().interval.as_str(),
        value_name = "DURATION"
    )]
    pub interval: Duration,

    /// The deadline for when the payload builder job should resolve.
    #[arg(
        long = "builder.deadline",
        value_parser = parse_duration_from_secs,
        default_value = DefaultPayloadBuilderValues::get_global().deadline.as_str(),
        value_name = "SECONDS"
    )]
    pub deadline: Duration,

    /// Maximum number of tasks to spawn for building a payload.
    #[arg(
        long = "builder.max-tasks",
        value_parser = RangedU64ValueParser::<usize>::new().range(1..),
        default_value_t = DefaultPayloadBuilderValues::get_global().max_payload_tasks
    )]
    pub max_payload_tasks: usize,

    /// Maximum number of blobs to include per block.
    #[arg(long = "builder.max-blobs", value_name = "COUNT")]
    pub max_blobs_per_block: Option<u64>,
}

impl Default for PayloadBuilderArgs {
    fn default() -> Self {
        let defaults = DefaultPayloadBuilderValues::get_global();
        Self {
            extra_data: defaults.extra_data.clone(),
            interval: parse_duration_from_secs_or_ms(defaults.interval.as_str()).unwrap(),
            gas_limit: None,
            deadline: Duration::from_secs(defaults.deadline.parse().unwrap()),
            max_payload_tasks: defaults.max_payload_tasks,
            max_blobs_per_block: None,
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

    fn gas_limit(&self) -> Option<u64> {
        self.gas_limit
    }

    fn max_payload_tasks(&self) -> usize {
        self.max_payload_tasks
    }

    fn max_blobs_per_block(&self) -> Option<u64> {
        self.max_blobs_per_block
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

        // Determine the byte length: if the value is a 0x-prefixed hex string, validate the
        // decoded byte length; otherwise, validate the raw UTF-8 byte length.
        let byte_len = if let Some(hex_str) = val.strip_prefix("0x") {
            if hex_str.len() % 2 != 0 {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::InvalidValue,
                    "Hex extradata must have an even number of digits",
                ))
            }
            // Reject before decoding to avoid unnecessary allocation
            let decoded_len = hex_str.len() / 2;
            if decoded_len > MAXIMUM_EXTRA_DATA_SIZE {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::InvalidValue,
                    format!(
                        "Payload builder extradata size exceeds {MAXIMUM_EXTRA_DATA_SIZE}-byte limit"
                    ),
                ))
            }
            // Validate that it's actually valid hex
            alloy_primitives::hex::decode(hex_str).map_err(|e| {
                clap::Error::raw(
                    clap::error::ErrorKind::InvalidValue,
                    format!("Invalid hex in extradata: {e}"),
                )
            })?;
            decoded_len
        } else {
            val.len()
        };

        if byte_len > MAXIMUM_EXTRA_DATA_SIZE {
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
    fn test_valid_hex_extra_data() {
        // 32 bytes encoded as hex (64 hex chars + 0x prefix) should be accepted
        let hex = format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE));
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.extradata",
            hex.as_str(),
        ])
        .args;
        assert_eq!(args.extra_data, hex);
    }

    #[test]
    fn test_oversized_hex_extra_data() {
        // 33 bytes encoded as hex should be rejected
        let hex = format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1));
        let args = CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            hex.as_str(),
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn test_invalid_hex_extra_data() {
        let args = CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            "0xZZZZ",
        ]);
        assert!(args.is_err());
    }

    #[test]
    fn test_odd_length_hex_extra_data() {
        let args = CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            "0xabc",
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
