use crate::{args::utils::parse_duration_from_secs, version::default_extradata};
use clap::{
    builder::{RangedU64ValueParser, TypedValueParser},
    Arg, Args, Command,
};
use reth_primitives::{bytes::BytesMut, constants::MAXIMUM_EXTRA_DATA_SIZE};
use reth_rlp::Encodable;
use std::{ffi::OsStr, time::Duration};

/// Parameters for configuring the Payload Builder
#[derive(Debug, Args, PartialEq, Default)]
pub struct PayloadBuilderArgs {
    /// Block extra data set by the payload builder.
    #[arg(long = "builder.extradata", help_heading = "Builder", value_parser=ExtradataValueParser::default(),  default_value_t = default_extradata())]
    pub extradata: String,

    /// Target gas ceiling for built blocks.
    #[arg(
        long = "builder.gaslimit",
        help_heading = "Builder",
        default_value = "30000000",
        value_name = "GAS_LIMIT"
    )]
    pub max_gas_limit: u64,

    /// The interval at which the job should build a new payload after the last (in seconds).
    #[arg(long = "builder.interval", help_heading = "Builder", value_parser = parse_duration_from_secs, default_value = "1", value_name = "SECONDS")]
    pub interval: Duration,

    /// The deadline for when the payload builder job should resolve.
    #[arg(long = "builder.deadline", help_heading = "Builder", value_parser = parse_duration_from_secs, default_value = "12", value_name = "SECONDS")]
    pub deadline: Duration,

    /// Maximum number of tasks to spawn for building a payload.
    #[arg(long = "builder.max-tasks", help_heading = "Builder", default_value = "3", value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
    pub max_payload_tasks: usize,
}

impl PayloadBuilderArgs {
    /// Returns the rlp-encoded extradata bytes.
    pub fn extradata_bytes(&self) -> reth_primitives::bytes::Bytes {
        let mut extradata = BytesMut::new();
        self.extradata.as_bytes().encode(&mut extradata);
        extradata.freeze()
    }
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct ExtradataValueParser;

impl TypedValueParser for ExtradataValueParser {
    type Value = String;

    fn parse_ref(
        &self,
        _cmd: &Command,
        _arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        if val.as_bytes().len() > MAXIMUM_EXTRA_DATA_SIZE {
            return Err(clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!(
                    "Payload builder extradata size exceeds {MAXIMUM_EXTRA_DATA_SIZE}bytes limit"
                ),
            ))
        }
        Ok(val.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
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
    fn test_default_extradata() {
        let extradata = default_extradata();
        let args = CommandParser::<PayloadBuilderArgs>::parse_from([
            "reth",
            "--builder.extradata",
            extradata.as_str(),
        ])
        .args;
        assert_eq!(args.extradata, extradata);
    }

    #[test]
    fn test_invalid_extradata() {
        let extradata = "x".repeat(MAXIMUM_EXTRA_DATA_SIZE + 1);
        let args = CommandParser::<PayloadBuilderArgs>::try_parse_from([
            "reth",
            "--builder.extradata",
            extradata.as_str(),
        ]);
        assert!(args.is_err());
    }
}
