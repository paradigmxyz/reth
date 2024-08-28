use std::sync::Arc;

use clap::builder::TypedValueParser;

/// Trait for parsing chain specifications.
///
/// This trait extends [`clap::builder::TypedValueParser`] to provide a parser for chain
/// specifications. Implementers of this trait must provide a list of supported chains and a
/// function to parse a given string into a chain spec.
pub trait ChainSpecParser<ChainSpec>: TypedValueParser<Value = Arc<ChainSpec>> + Default {
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
    fn parse(s: &str) -> eyre::Result<Arc<ChainSpec>>;
}
