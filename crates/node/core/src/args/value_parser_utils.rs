//! Utility functions for clap TypedValueParser implementations

use clap::{error::ErrorKind, Arg, Error};
use std::ffi::OsStr;

/// Converts an `OsStr` to UTF-8, returning `InvalidUtf8` error on failure.
pub fn parse_osstr_to_str(value: &OsStr) -> Result<&str, Error> {
    value.to_str().ok_or_else(|| Error::new(ErrorKind::InvalidUtf8))
}

/// Formats argument name for error messages, returning "..." if None.
pub fn format_arg_name(arg: Option<&Arg>) -> String {
    arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned())
}

/// Builds a standardized `InvalidValue` error with possible values listed.
pub fn build_invalid_value_error<E: std::fmt::Display>(
    val: &str,
    arg: Option<&Arg>,
    err: E,
    possible_values: &str,
) -> Error {
    let arg_name = format_arg_name(arg);
    let msg = format!(
        "Invalid value '{val}' for {arg_name}: {err}.\n    [possible values: {possible_values}]"
    );
    Error::raw(ErrorKind::InvalidValue, msg)
}
