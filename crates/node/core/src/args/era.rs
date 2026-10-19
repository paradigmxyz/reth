use clap::Args;
use reth_chainspec::{ChainKind, NamedChain};
use std::path::Path;
use url::Url;

/// Syncs ERA1 encoded blocks from a local or remote source.
#[derive(Clone, Debug, Default, Args)]
pub struct EraArgs {
    /// Enable import from ERA1 files.
    #[arg(
        id = "era.enable",
        long = "era.enable",
        value_name = "ERA_ENABLE",
        default_value_t = false
    )]
    pub enabled: bool,

    /// Describes where to get the ERA files to import from.
    #[clap(flatten)]
    pub source: EraSourceArgs,
}

/// Arguments for the block history import based on ERA1 encoded files.
#[derive(Clone, Debug, Default, Args)]
#[group(required = false, multiple = false)]
pub struct EraSourceArgs {
    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the local directory parsing headers and bodies.
    #[arg(long = "era.path", value_name = "ERA_PATH", verbatim_doc_comment)]
    pub path: Option<Box<Path>>,

    /// The URL to a remote host where the ERA1 files are hosted.
    ///
    /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
    /// and bodies.
    #[arg(long = "era.url", value_name = "ERA_URL", verbatim_doc_comment)]
    pub url: Option<Url>,
}

/// The `ExtractEraHost` trait allows to derive a default URL host for ERA files.
pub trait DefaultEraHost {
    /// Converts `self` into [`Url`] index page of the ERA host.
    ///
    /// Returns `Err` if the conversion is not possible.
    fn default_era_host(&self) -> Option<Url>;
}

impl DefaultEraHost for ChainKind {
    fn default_era_host(&self) -> Option<Url> {
        Some(match self {
            Self::Named(NamedChain::Mainnet) => {
                Url::parse("https://era.ithaca.xyz/era1/index.html").expect("URL should be valid")
            }
            Self::Named(NamedChain::Sepolia) => {
                Url::parse("https://era.ithaca.xyz/sepolia-era1/index.html")
                    .expect("URL should be valid")
            }
            _ => return None,
        })
    }
}
