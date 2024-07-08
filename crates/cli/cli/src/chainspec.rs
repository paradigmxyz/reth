use clap::builder::TypedValueParser;
use reth_chainspec::ChainSpec;
use std::sync::Arc;

/// Trait for parsing chain specifications.
///
/// This trait extends [`clap::builder::TypedValueParser`] to provide a parser for chain
/// specifications. Implementers of this trait must provide a list of supported chains and a
/// function to parse a given string into a [`ChainSpec`].
pub trait ChainSpecParser: TypedValueParser<Value = Arc<ChainSpec>> + Default {
    /// List of supported chains.
    const SUPPORTED_CHAINS: &'static [&'static str];

    /// Parses the given string into a [`ChainSpec`].
    ///
    /// # Arguments
    ///
    /// * `s` - A string slice that holds the chain spec to be parsed.
    ///
    /// # Errors
    ///
    /// This function will return an error if the input string cannot be parsed into a valid
    /// [`ChainSpec`].
    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>>;
}
