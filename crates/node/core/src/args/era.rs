use clap::Args;
use eyre::eyre;
use reth_chainspec::{ChainKind, NamedChain};
use std::path::PathBuf;
use url::Url;

/// Arguments for the block history import based on ERA encoded files.
#[derive(Clone, Debug, Default, Args)]
#[group(required = false, multiple = false)]
pub struct EraArgs {
    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the local directory parsing headers and bodies.
    #[arg(long = "era.path", value_name = "ERA_PATH", verbatim_doc_comment)]
    pub path: Option<PathBuf>,

    /// The URL to a remote host where the ERA1 files are hosted.
    ///
    /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
    /// and bodies.
    #[arg(long = "era.url", value_name = "ERA_URL", verbatim_doc_comment)]
    pub url: Option<Url>,
}

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
                Url::parse("https://era.ithaca.xyz/era1/").expect("URL should be valid")
            }
            Self::Named(NamedChain::Sepolia) => {
                Url::parse("https://era.ithaca.xyz/sepolia-era1/").expect("URL should be valid")
            }
            chain => return Err(eyre!("No known host for ERA files on chain {chain:?}")),
        })
    }
}
