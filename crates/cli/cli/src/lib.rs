//! Cli abstraction for reth based nodes.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use clap::{Error, Parser};
use reth_cli_runner::CliRunner;
use std::{borrow::Cow, ffi::OsString};

/// The chainspec module defines the different chainspecs that can be used by the node.
pub mod chainspec;

/// Reth based node cli.
///
/// This trait is supposed to be implemented by the main struct of the CLI.
///
/// It provides commonly used functionality for running commands and information about the CL, such
/// as the name and version.
pub trait RethCli: Sized {
    /// The name of the implementation, eg. `reth`, `op-reth`, etc.
    fn name(&self) -> Cow<'static, str>;

    /// The version of the node, such as `reth/v1.0.0`
    fn version(&self) -> Cow<'static, str>;

    /// Parse args from iterator from [`std::env::args_os()`].
    fn parse_args() -> Result<Self, Error>
    where
        Self: Parser,
    {
        <Self as RethCli>::try_parse_from(std::env::args_os())
    }

    /// Parse args from the given iterator.
    fn try_parse_from<I, T>(itr: I) -> Result<Self, Error>
    where
        Self: Parser,
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        <Self as Parser>::try_parse_from(itr)
    }

    /// Executes a command.
    fn with_runner<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self, CliRunner) -> R,
    {
        let runner = CliRunner::default();

        f(self, runner)
    }

    /// Parses and executes a command.
    fn execute<F, R>(f: F) -> Result<R, Error>
    where
        Self: Parser,
        F: FnOnce(Self, CliRunner) -> R,
    {
        let cli = Self::parse_args()?;

        Ok(cli.with_runner(f))
    }
}
