use eyre::eyre;
use reth_chainspec::{ChainKind, NamedChain};
use url::Url;

/// Conversion to [`Url`] from a reference.
pub trait TryToUrl {
    /// Converts `self` into [`Url`].
    ///
    /// Returns `Err` if the conversion is not possible.
    fn try_to_url(&self) -> eyre::Result<Url>;
}

impl TryToUrl for ChainKind {
    fn try_to_url(&self) -> eyre::Result<Url> {
        Ok(match self {
            Self::Named(NamedChain::Mainnet) => {
                Url::parse("https://era.ithaca.xyz/era1/index.html").expect("URL should be valid")
            }
            Self::Named(NamedChain::Sepolia) => {
                Url::parse("https://era.ithaca.xyz/sepolia-era1/index.html")
                    .expect("URL should be valid")
            }
            chain => return Err(eyre!("No known host for ERA files on chain {chain:?}")),
        })
    }
}
