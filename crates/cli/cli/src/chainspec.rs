use std::{fs, path::PathBuf, sync::Arc};

use clap::builder::TypedValueParser;

#[derive(Debug, Clone)]
struct Parser<C>(std::marker::PhantomData<C>);

impl<C: ChainSpecParser> TypedValueParser for Parser<C> {
    type Value = Arc<C::ChainSpec>;

    fn parse_ref(
        &self,
        _cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let val =
            value.to_str().ok_or_else(|| clap::Error::new(clap::error::ErrorKind::InvalidUtf8))?;
        C::parse(val).map_err(|err| {
            let arg = arg.map(|a| a.to_string()).unwrap_or_else(|| "...".to_owned());
            let possible_values = C::SUPPORTED_CHAINS.join(",");
            let msg = format!(
                "Invalid value '{val}' for {arg}: {err}.\n    [possible values: {possible_values}]"
            );
            clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg)
        })
    }
}

/// Trait for parsing chain specifications.
///
/// This trait extends [`clap::builder::TypedValueParser`] to provide a parser for chain
/// specifications. Implementers of this trait must provide a list of supported chains and a
/// function to parse a given string into a chain spec.
pub trait ChainSpecParser: Clone + Send + Sync + 'static {
    /// The chain specification type.
    type ChainSpec: std::fmt::Debug + Send + Sync;

    /// List of supported chains.
    const SUPPORTED_CHAINS: &'static [&'static str];

    /// Parses the given string into a chain spec.
    ///
    /// # Arguments
    ///
    /// * `s` - A string slice that holds the chain spec to be parsed.
    ///
    /// # Errors
    ///
    /// This function will return an error if the input string cannot be parsed into a valid
    /// chain spec.
    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>>;

    /// Produces a [`TypedValueParser`] for this chain spec parser.
    fn parser() -> impl TypedValueParser<Value = Arc<Self::ChainSpec>> {
        Parser(std::marker::PhantomData::<Self>)
    }

    /// Produces a help message for the chain spec argument.
    fn help_message() -> String {
        format!("The chain this node is running.\nPossible values are either a built-in chain or the path to a chain specification file.\n\nBuilt-in chains:\n    {}", Self::SUPPORTED_CHAINS.join(", "))
    }
}

/// A helper to parse a [`Genesis`](alloy_genesis::Genesis) as argument or from disk.
pub fn parse_genesis(s: &str) -> eyre::Result<alloy_genesis::Genesis> {
    // try to read json from path first
    let raw = match fs::read_to_string(PathBuf::from(shellexpand::full(s)?.into_owned())) {
        Ok(raw) => raw,
        Err(io_err) => {
            // valid json may start with "\n", but must contain "{"
            if s.contains('{') {
                s.to_string()
            } else {
                return Err(io_err.into()) // assume invalid path
            }
        }
    };

    Ok(serde_json::from_str(&raw)?)
}
