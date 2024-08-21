//! clap [Args](clap::Args) for debugging purposes

use clap::{
    builder::{PossibleValue, TypedValueParser},
    Arg, Args, Command,
};
use reth_primitives::B256;
use std::{collections::HashSet, ffi::OsStr, fmt, path::PathBuf, str::FromStr};
use strum::{AsRefStr, EnumIter, IntoStaticStr, ParseError, VariantArray, VariantNames};

/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Debug")]
pub struct DebugArgs {
    /// Flag indicating whether the node should be terminated after the pipeline sync.
    #[arg(long = "debug.terminate", help_heading = "Debug")]
    pub terminate: bool,

    /// Set the chain tip manually for testing purposes.
    ///
    /// NOTE: This is a temporary flag
    #[arg(long = "debug.tip", help_heading = "Debug")]
    pub tip: Option<B256>,

    /// Runs the sync only up to the specified block.
    #[arg(long = "debug.max-block", help_heading = "Debug")]
    pub max_block: Option<u64>,

    /// Runs a fake consensus client that advances the chain using recent block hashes
    /// on Etherscan. If specified, requires an `ETHERSCAN_API_KEY` environment variable.
    #[arg(
        long = "debug.etherscan",
        help_heading = "Debug",
        conflicts_with = "tip",
        conflicts_with = "rpc_consensus_ws",
        value_name = "ETHERSCAN_API_URL"
    )]
    pub etherscan: Option<Option<String>>,

    /// Runs a fake consensus client using blocks fetched from an RPC `WebSocket` endpoint.
    #[arg(
        long = "debug.rpc-consensus-ws",
        help_heading = "Debug",
        conflicts_with = "tip",
        conflicts_with = "etherscan"
    )]
    pub rpc_consensus_ws: Option<String>,

    /// If provided, the engine will skip `n` consecutive FCUs.
    #[arg(long = "debug.skip-fcu", help_heading = "Debug")]
    pub skip_fcu: Option<usize>,

    /// If provided, the engine will skip `n` consecutive new payloads.
    #[arg(long = "debug.skip-new-payload", help_heading = "Debug")]
    pub skip_new_payload: Option<usize>,

    /// If provided, the chain will be reorged at specified frequency.
    #[arg(long = "debug.reorg-frequency", help_heading = "Debug")]
    pub reorg_frequency: Option<usize>,

    /// The path to store engine API messages at.
    /// If specified, all of the intercepted engine API messages
    /// will be written to specified location.
    #[arg(long = "debug.engine-api-store", help_heading = "Debug", value_name = "PATH")]
    pub engine_api_store: Option<PathBuf>,

    /// Determines which type of bad block hook to install
    ///
    /// Example: `witness,prestate`
    #[arg(long = "debug.bad-block-hook", help_heading = "Debug", value_parser = BadBlockSelectionValueParser::default())]
    pub bad_block_hook: Option<BadBlockSelection>,
}

/// Describes the invalid block hooks that should be installed.
///
/// # Example
///
/// Create a [`BadBlockSelection`] from a selection.
///
/// ```
/// use reth_node_core::args::{BadBlockHook, BadBlockSelection};
/// let config: BadBlockSelection = vec![BadBlockHook::Witness].into();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadBlockSelection(HashSet<BadBlockHook>);

impl BadBlockSelection {
    /// Creates a new _unique_ [`BadBlockSelection`] from the given items.
    ///
    /// # Note
    ///
    /// This will dedupe the selection and remove duplicates while preserving the order.
    ///
    /// # Example
    ///
    /// Create a selection from the [`BadBlockHook`] string identifiers
    ///
    /// ```
    /// use reth_node_core::args::{BadBlockHook, BadBlockSelection};
    /// let selection = vec!["witness", "prestate", "opcode"];
    /// let config = BadBlockSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(
    ///     config,
    ///     BadBlockSelection::from([
    ///         BadBlockHook::Witness,
    ///         BadBlockHook::PreState,
    ///         BadBlockHook::Opcode
    ///     ])
    /// );
    /// ```
    ///
    /// Create a unique selection from the [`BadBlockHook`] string identifiers
    ///
    /// ```
    /// use reth_node_core::args::{BadBlockHook, BadBlockSelection};
    /// let selection = vec!["witness", "prestate", "opcode", "witness", "prestate"];
    /// let config = BadBlockSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(
    ///     config,
    ///     BadBlockSelection::from([
    ///         BadBlockHook::Witness,
    ///         BadBlockHook::PreState,
    ///         BadBlockHook::Opcode
    ///     ])
    /// );
    /// ```
    pub fn try_from_selection<I, T>(selection: I) -> Result<Self, T::Error>
    where
        I: IntoIterator<Item = T>,
        T: TryInto<BadBlockHook>,
    {
        selection.into_iter().map(TryInto::try_into).collect()
    }
}

impl From<&[BadBlockHook]> for BadBlockSelection {
    fn from(s: &[BadBlockHook]) -> Self {
        Self(s.iter().copied().collect())
    }
}

impl From<Vec<BadBlockHook>> for BadBlockSelection {
    fn from(s: Vec<BadBlockHook>) -> Self {
        Self(s.into_iter().collect())
    }
}

impl<const N: usize> From<[BadBlockHook; N]> for BadBlockSelection {
    fn from(s: [BadBlockHook; N]) -> Self {
        Self(s.iter().copied().collect())
    }
}

impl FromIterator<BadBlockHook> for BadBlockSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = BadBlockHook>,
    {
        Self(iter.into_iter().collect())
    }
}

impl FromStr for BadBlockSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self(Default::default()))
        }
        let hooks = s.split(',').map(str::trim).peekable();
        Self::try_from_selection(hooks)
    }
}

impl fmt::Display for BadBlockSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "))
    }
}

/// clap value parser for [`BadBlockSelection`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct BadBlockSelectionValueParser;

impl TypedValueParser for BadBlockSelectionValueParser {
    type Value = BadBlockSelection;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        val.parse::<BadBlockSelection>().map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = BadBlockHook::all_variant_names().to_vec().join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = BadBlockHook::all_variant_names().iter().map(PossibleValue::new);
        Some(Box::new(values))
    }
}

/// The type of bad block hook to install
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    AsRefStr,
    IntoStaticStr,
    VariantNames,
    VariantArray,
    EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum BadBlockHook {
    /// A witness value enum
    Witness,
    /// A prestate trace value enum
    PreState,
    /// An opcode trace value enum
    Opcode,
}

impl FromStr for BadBlockHook {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "witness" => Self::Witness,
            "prestate" => Self::PreState,
            "opcode" => Self::Opcode,
            _ => return Err(ParseError::VariantNotFound),
        })
    }
}

impl TryFrom<&str> for BadBlockHook {
    type Error = ParseError;
    fn try_from(s: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {
        FromStr::from_str(s)
    }
}

impl fmt::Display for BadBlockHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_ref())
    }
}

impl BadBlockHook {
    /// Returns all variant names of the enum
    pub const fn all_variant_names() -> &'static [&'static str] {
        <Self as VariantNames>::VARIANTS
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
    fn test_parse_default_debug_args() {
        let default_args = DebugArgs::default();
        let args = CommandParser::<DebugArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }

    #[test]
    fn test_parse_bad_block_args() {
        let expected_args = DebugArgs {
            bad_block_hook: Some(BadBlockSelection::from([BadBlockHook::Witness])),
            ..Default::default()
        };
        let args =
            CommandParser::<DebugArgs>::parse_from(["reth", "--debug.bad-block-hook", "witness"])
                .args;
        assert_eq!(args, expected_args);

        let expected_args = DebugArgs {
            bad_block_hook: Some(BadBlockSelection::from([
                BadBlockHook::Witness,
                BadBlockHook::PreState,
            ])),
            ..Default::default()
        };
        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.bad-block-hook",
            "witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.bad-block-hook",
            "witness,prestate,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.bad-block-hook",
            "witness,witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.bad-block-hook",
            "prestate,witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);
    }
}
