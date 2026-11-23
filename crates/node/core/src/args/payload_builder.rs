use crate::{cli::config::PayloadBuilderConfig, version::default_extra_data};
use alloy_consensus::constants::MAXIMUM_EXTRA_DATA_SIZE;
use alloy_primitives::{hex, Bytes};
use clap::{
    builder::{RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use reth_cli_util::{
    parse_duration_from_secs, parse_duration_from_secs_or_ms,
    parsers::format_duration_as_secs_or_ms,
};
use std::{ffi::OsStr, sync::OnceLock, time::Duration};

/// Registered defaults and the decoded extra data used to construct node configs.
static PAYLOAD_BUILDER_DEFAULTS: OnceLock<(DefaultPayloadBuilderValues, Bytes)> = OnceLock::new();

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
    /// Initialize the global payload builder defaults with this configuration.
    ///
    /// Returns the supplied configuration if defaults are already installed or the extra data
    /// is invalid. Use [`Self::try_init_detailed`] to distinguish the two cases.
    pub fn try_init(self) -> Result<(), Self> {
        self.try_init_detailed().map_err(|err| match err {
            PayloadBuilderDefaultsError::AlreadyInitialized(defaults) |
            PayloadBuilderDefaultsError::InvalidExtraData { defaults, .. } => defaults,
        })
    }

    /// Initialize the global payload builder defaults, returning the reason for failure.
    ///
    /// Returns [`PayloadBuilderDefaultsError::AlreadyInitialized`] if defaults are already
    /// installed. [`PayloadBuilderDefaultsError::InvalidExtraData`] returns the configuration
    /// for correction without initializing the defaults.
    pub fn try_init_detailed(self) -> Result<(), PayloadBuilderDefaultsError> {
        if PAYLOAD_BUILDER_DEFAULTS.get().is_some() {
            return Err(PayloadBuilderDefaultsError::AlreadyInitialized(self));
        }
        let extra_data = match decode_extra_data(&self.extra_data) {
            Ok(bytes) => bytes,
            Err(source) => {
                return Err(PayloadBuilderDefaultsError::InvalidExtraData { defaults: self, source })
            }
        };
        PAYLOAD_BUILDER_DEFAULTS
            .set((self, extra_data))
            .map_err(|(defaults, _)| PayloadBuilderDefaultsError::AlreadyInitialized(defaults))
    }

    /// Get a reference to the global payload builder defaults
    pub fn get_global() -> &'static Self {
        &Self::global().0
    }

    /// Set the default extra data as `0x`-prefixed hex or plain UTF-8 text.
    ///
    /// [`Self::try_init`] checks the hex encoding. The 32-byte limit is enforced when the CLI
    /// parses the default, since programmatic configs may target chains with a higher limit.
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

    fn global() -> &'static (Self, Bytes) {
        PAYLOAD_BUILDER_DEFAULTS.get_or_init(|| {
            let defaults = Self::default();
            let extra_data = Bytes::copy_from_slice(defaults.extra_data.as_bytes());
            (defaults, extra_data)
        })
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
    ///
    /// If the value is a `0x`-prefixed hex string, it is decoded into raw bytes. Otherwise, the
    /// raw UTF-8 bytes of the string are used.
    #[arg(
        long = "builder.extradata",
        value_parser = ExtraDataValueParser::default(),
        default_value = DefaultPayloadBuilderValues::get_global().extra_data.as_str()
    )]
    pub extra_data: Bytes,

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
        let (defaults, extra_data) = DefaultPayloadBuilderValues::global();
        Self {
            extra_data: extra_data.clone(),
            interval: parse_duration_from_secs_or_ms(defaults.interval.as_str()).unwrap(),
            gas_limit: None,
            deadline: Duration::from_secs(defaults.deadline.parse().unwrap()),
            max_payload_tasks: defaults.max_payload_tasks,
            max_blobs_per_block: None,
        }
    }
}

impl PayloadBuilderConfig for PayloadBuilderArgs {
    fn extra_data(&self) -> Bytes {
        self.extra_data.clone()
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

/// Failure to register global payload builder defaults. Both variants return the supplied values.
#[derive(Debug, thiserror::Error)]
pub enum PayloadBuilderDefaultsError {
    /// Another configuration has already been installed.
    #[error("payload builder defaults already initialized")]
    AlreadyInitialized(DefaultPayloadBuilderValues),
    /// The supplied `0x`-prefixed extra data is not valid hex and was not installed.
    #[error("invalid hex in payload builder default extra data")]
    InvalidExtraData {
        /// The supplied defaults, which can be corrected and submitted again.
        defaults: DefaultPayloadBuilderValues,
        /// The reason the extra data was rejected.
        source: hex::FromHexError,
    },
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct ExtraDataValueParser;

impl TypedValueParser for ExtraDataValueParser {
    type Value = Bytes;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;

        let max_len = if val.starts_with("0x") {
            "0x".len() + 2 * MAXIMUM_EXTRA_DATA_SIZE
        } else {
            MAXIMUM_EXTRA_DATA_SIZE
        };
        if val.len() > max_len {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!(
                    "Payload builder extradata size exceeds {MAXIMUM_EXTRA_DATA_SIZE}-byte limit"
                ),
            ))
        }

        decode_extra_data(val).map_err(|e| {
            clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!("Invalid hex in extradata: {e}"),
            )
        })
    }
}

/// Decodes `0x`-prefixed hex and keeps any other value as UTF-8 bytes.
fn decode_extra_data(val: &str) -> Result<Bytes, hex::FromHexError> {
    if val.starts_with("0x") {
        hex::decode(val).map(Bytes::from)
    } else {
        Ok(Bytes::copy_from_slice(val.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Debug, Parser)]
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
        assert_eq!(args.extra_data.as_ref(), extra_data.as_bytes());
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
        let hex = format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE));
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.extradata",
            hex.as_str(),
        ])
        .args;
        assert_eq!(args.extra_data.as_ref(), vec![0xab; MAXIMUM_EXTRA_DATA_SIZE].as_slice());
    }

    #[test]
    fn test_oversized_hex_extra_data() {
        let hex = format!("0x{}", "ab".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1));
        assert!(CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            hex.as_str(),
        ])
        .is_err());
    }

    #[test]
    fn extra_data_size_is_checked_before_hex_decoding() {
        let oversized = format!("0x{}zz", "ff".repeat(MAXIMUM_EXTRA_DATA_SIZE));
        let err = ExtraDataValueParser
            .parse_ref(&Command::new("reth"), None, OsStr::new(&oversized))
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        assert_eq!(
            err.to_string(),
            format!("error: Payload builder extradata size exceeds {MAXIMUM_EXTRA_DATA_SIZE}-byte limit")
        );
    }

    #[test]
    fn test_invalid_hex_extra_data() {
        for invalid in ["0xZZZZ", "0x0x", "0x0xab", "0x0Xab"] {
            let err = CommandParser::<PayloadBuilderArgs>::try_parse_from([
                "reth",
                "--builder.extradata",
                invalid,
            ])
            .unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        }
    }

    #[test]
    fn test_odd_length_hex_extra_data() {
        assert!(CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            "0xabc",
        ])
        .is_err());
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
