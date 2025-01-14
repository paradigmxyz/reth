//! clap [Args](clap::Args) for debugging purposes

use alloy_primitives::B256;
use clap::{
    builder::{PossibleValue, TypedValueParser},
    Arg, Args, Command,
};
use std::{collections::HashSet, ffi::OsStr, fmt, path::PathBuf, str::FromStr};
use strum::{AsRefStr, EnumIter, IntoStaticStr, ParseError, VariantArray, VariantNames};

/// Parameters for debugging purposes
#[derive(Debug, Clone, Args, PartialEq, Eq)]
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

    /// The reorg depth for chain reorgs.
    #[arg(long = "debug.reorg-depth", requires = "reorg_frequency", help_heading = "Debug")]
    pub reorg_depth: Option<usize>,

    /// The path to store engine API messages at.
    /// If specified, all of the intercepted engine API messages
    /// will be written to specified location.
    #[arg(long = "debug.engine-api-store", help_heading = "Debug", value_name = "PATH")]
    pub engine_api_store: Option<PathBuf>,

    /// Determines which type of invalid block hook to install
    ///
    /// Example: `witness,prestate`
    #[arg(
        long = "debug.invalid-block-hook",
        help_heading = "Debug",
        value_parser = InvalidBlockSelectionValueParser::default(),
        default_value = "witness"
    )]
    pub invalid_block_hook: Option<InvalidBlockSelection>,

    /// The RPC URL of a healthy node to use for comparing invalid block hook results against.
    #[arg(
        long = "debug.healthy-node-rpc-url",
        help_heading = "Debug",
        value_name = "URL",
        verbatim_doc_comment
    )]
    pub healthy_node_rpc_url: Option<String>,
}

impl Default for DebugArgs {
    fn default() -> Self {
        Self {
            terminate: false,
            tip: None,
            max_block: None,
            etherscan: None,
            rpc_consensus_ws: None,
            skip_fcu: None,
            skip_new_payload: None,
            reorg_frequency: None,
            reorg_depth: None,
            engine_api_store: None,
            invalid_block_hook: Some(InvalidBlockSelection::default()),
            healthy_node_rpc_url: None,
        }
    }
}

/// Describes the invalid block hooks that should be installed.
///
/// # Example
///
/// Create a [`InvalidBlockSelection`] from a selection.
///
/// ```
/// use reth_node_core::args::{InvalidBlockHookType, InvalidBlockSelection};
/// let config: InvalidBlockSelection = vec![InvalidBlockHookType::Witness].into();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Deref)]
pub struct InvalidBlockSelection(HashSet<InvalidBlockHookType>);

impl Default for InvalidBlockSelection {
    fn default() -> Self {
        Self([InvalidBlockHookType::Witness].into())
    }
}

impl InvalidBlockSelection {
    /// Creates a new _unique_ [`InvalidBlockSelection`] from the given items.
    ///
    /// # Note
    ///
    /// This will dedupe the selection and remove duplicates while preserving the order.
    ///
    /// # Example
    ///
    /// Create a selection from the [`InvalidBlockHookType`] string identifiers
    ///
    /// ```
    /// use reth_node_core::args::{InvalidBlockHookType, InvalidBlockSelection};
    /// let selection = vec!["witness", "prestate", "opcode"];
    /// let config = InvalidBlockSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(
    ///     config,
    ///     InvalidBlockSelection::from([
    ///         InvalidBlockHookType::Witness,
    ///         InvalidBlockHookType::PreState,
    ///         InvalidBlockHookType::Opcode
    ///     ])
    /// );
    /// ```
    ///
    /// Create a unique selection from the [`InvalidBlockHookType`] string identifiers
    ///
    /// ```
    /// use reth_node_core::args::{InvalidBlockHookType, InvalidBlockSelection};
    /// let selection = vec!["witness", "prestate", "opcode", "witness", "prestate"];
    /// let config = InvalidBlockSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(
    ///     config,
    ///     InvalidBlockSelection::from([
    ///         InvalidBlockHookType::Witness,
    ///         InvalidBlockHookType::PreState,
    ///         InvalidBlockHookType::Opcode
    ///     ])
    /// );
    /// ```
    pub fn try_from_selection<I, T>(selection: I) -> Result<Self, T::Error>
    where
        I: IntoIterator<Item = T>,
        T: TryInto<InvalidBlockHookType>,
    {
        selection.into_iter().map(TryInto::try_into).collect()
    }

    /// Clones the set of configured [`InvalidBlockHookType`].
    pub fn to_selection(&self) -> HashSet<InvalidBlockHookType> {
        self.0.clone()
    }
}

impl From<&[InvalidBlockHookType]> for InvalidBlockSelection {
    fn from(s: &[InvalidBlockHookType]) -> Self {
        Self(s.iter().copied().collect())
    }
}

impl From<Vec<InvalidBlockHookType>> for InvalidBlockSelection {
    fn from(s: Vec<InvalidBlockHookType>) -> Self {
        Self(s.into_iter().collect())
    }
}

impl<const N: usize> From<[InvalidBlockHookType; N]> for InvalidBlockSelection {
    fn from(s: [InvalidBlockHookType; N]) -> Self {
        Self(s.iter().copied().collect())
    }
}

impl FromIterator<InvalidBlockHookType> for InvalidBlockSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = InvalidBlockHookType>,
    {
        Self(iter.into_iter().collect())
    }
}

impl FromStr for InvalidBlockSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self(Default::default()))
        }
        let hooks = s.split(',').map(str::trim).peekable();
        Self::try_from_selection(hooks)
    }
}

impl fmt::Display for InvalidBlockSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "))
    }
}

/// clap value parser for [`InvalidBlockSelection`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
struct InvalidBlockSelectionValueParser;

impl TypedValueParser for InvalidBlockSelectionValueParser {
    type Value = InvalidBlockSelection;

    fn parse_ref(
        &self,
        _cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        val.parse::<InvalidBlockSelection>().map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = InvalidBlockHookType::all_variant_names().to_vec().join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }

    fn possible_values(&self) -> Option<Box<dyn Iterator<Item = PossibleValue> + '_>> {
        let values = InvalidBlockHookType::all_variant_names().iter().map(PossibleValue::new);
        Some(Box::new(values))
    }
}

/// The type of invalid block hook to install
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
pub enum InvalidBlockHookType {
    /// A witness value enum
    Witness,
    /// A prestate trace value enum
    PreState,
    /// An opcode trace value enum
    Opcode,
}

impl FromStr for InvalidBlockHookType {
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

impl TryFrom<&str> for InvalidBlockHookType {
    type Error = ParseError;
    fn try_from(s: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {
        FromStr::from_str(s)
    }
}

impl fmt::Display for InvalidBlockHookType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_ref())
    }
}

impl InvalidBlockHookType {
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
    fn test_parse_invalid_block_args() {
        let expected_args = DebugArgs {
            invalid_block_hook: Some(InvalidBlockSelection::from([InvalidBlockHookType::Witness])),
            ..Default::default()
        };
        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.invalid-block-hook",
            "witness",
        ])
        .args;
        assert_eq!(args, expected_args);

        let expected_args = DebugArgs {
            invalid_block_hook: Some(InvalidBlockSelection::from([
                InvalidBlockHookType::Witness,
                InvalidBlockHookType::PreState,
            ])),
            ..Default::default()
        };
        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.invalid-block-hook",
            "witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.invalid-block-hook",
            "witness,prestate,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.invalid-block-hook",
            "witness,witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);

        let args = CommandParser::<DebugArgs>::parse_from([
            "reth",
            "--debug.invalid-block-hook",
            "prestate,witness,prestate",
        ])
        .args;
        assert_eq!(args, expected_args);
    }
}
