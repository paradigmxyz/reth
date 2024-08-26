use std::{collections::HashSet, fmt, str::FromStr};

use serde::{Deserialize, Serialize, Serializer};
use strum::{AsRefStr, EnumIter, IntoStaticStr, ParseError, VariantArray, VariantNames};

/// Describes the modules that should be installed.
///
/// # Example
///
/// Create a [`RpcModuleSelection`] from a selection.
///
/// ```
/// use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
/// let config: RpcModuleSelection = vec![RethRpcModule::Eth].into();
/// ```
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub enum RpcModuleSelection {
    /// Use _all_ available modules.
    All,
    /// The default modules `eth`, `net`, `web3`
    #[default]
    Standard,
    /// Only use the configured modules.
    Selection(HashSet<RethRpcModule>),
}

// === impl RpcModuleSelection ===

impl RpcModuleSelection {
    /// The standard modules to instantiate by default `eth`, `net`, `web3`
    pub const STANDARD_MODULES: [RethRpcModule; 3] =
        [RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3];

    /// Returns a selection of [`RethRpcModule`] with all [`RethRpcModule::all_variants`].
    pub fn all_modules() -> HashSet<RethRpcModule> {
        RethRpcModule::modules().into_iter().collect()
    }

    /// Returns the [`RpcModuleSelection::STANDARD_MODULES`] as a selection.
    pub fn standard_modules() -> HashSet<RethRpcModule> {
        HashSet::from(Self::STANDARD_MODULES)
    }

    /// All modules that are available by default on IPC.
    ///
    /// By default all modules are available on IPC.
    pub fn default_ipc_modules() -> HashSet<RethRpcModule> {
        Self::all_modules()
    }

    /// Creates a new _unique_ [`RpcModuleSelection::Selection`] from the given items.
    ///
    /// # Note
    ///
    /// This will dedupe the selection and remove duplicates while preserving the order.
    ///
    /// # Example
    ///
    /// Create a selection from the [`RethRpcModule`] string identifiers
    ///
    /// ```
    /// use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
    /// let selection = vec!["eth", "admin"];
    /// let config = RpcModuleSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(config, RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]));
    /// ```
    ///
    /// Create a unique selection from the [`RethRpcModule`] string identifiers
    ///
    /// ```
    /// use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};
    /// let selection = vec!["eth", "admin", "eth", "admin"];
    /// let config = RpcModuleSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(config, RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]));
    /// ```
    pub fn try_from_selection<I, T>(selection: I) -> Result<Self, T::Error>
    where
        I: IntoIterator<Item = T>,
        T: TryInto<RethRpcModule>,
    {
        selection.into_iter().map(TryInto::try_into).collect()
    }

    /// Returns the number of modules in the selection
    pub fn len(&self) -> usize {
        match self {
            Self::All => RethRpcModule::variant_count(),
            Self::Standard => Self::STANDARD_MODULES.len(),
            Self::Selection(s) => s.len(),
        }
    }

    /// Returns true if no selection is configured
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Selection(sel) => sel.is_empty(),
            _ => false,
        }
    }

    /// Returns an iterator over all configured [`RethRpcModule`]
    pub fn iter_selection(&self) -> Box<dyn Iterator<Item = RethRpcModule> + '_> {
        match self {
            Self::All => Box::new(RethRpcModule::modules().into_iter()),
            Self::Standard => Box::new(Self::STANDARD_MODULES.iter().copied()),
            Self::Selection(s) => Box::new(s.iter().copied()),
        }
    }

    /// Clones the set of configured [`RethRpcModule`].
    pub fn to_selection(&self) -> HashSet<RethRpcModule> {
        match self {
            Self::All => Self::all_modules(),
            Self::Standard => Self::standard_modules(),
            Self::Selection(s) => s.clone(),
        }
    }

    /// Converts the selection into a [`HashSet`].
    pub fn into_selection(self) -> HashSet<RethRpcModule> {
        match self {
            Self::All => Self::all_modules(),
            Self::Standard => Self::standard_modules(),
            Self::Selection(s) => s,
        }
    }

    /// Returns true if both selections are identical.
    pub fn are_identical(http: Option<&Self>, ws: Option<&Self>) -> bool {
        match (http, ws) {
            // Shortcut for common case to avoid iterating later
            (Some(Self::All), Some(other)) | (Some(other), Some(Self::All)) => {
                other.len() == RethRpcModule::variant_count()
            }

            // If either side is disabled, then the other must be empty
            (Some(some), None) | (None, Some(some)) => some.is_empty(),

            (Some(http), Some(ws)) => http.to_selection() == ws.to_selection(),
            (None, None) => true,
        }
    }
}

impl From<&HashSet<RethRpcModule>> for RpcModuleSelection {
    fn from(s: &HashSet<RethRpcModule>) -> Self {
        Self::from(s.clone())
    }
}

impl From<HashSet<RethRpcModule>> for RpcModuleSelection {
    fn from(s: HashSet<RethRpcModule>) -> Self {
        Self::Selection(s)
    }
}

impl From<&[RethRpcModule]> for RpcModuleSelection {
    fn from(s: &[RethRpcModule]) -> Self {
        Self::Selection(s.iter().copied().collect())
    }
}

impl From<Vec<RethRpcModule>> for RpcModuleSelection {
    fn from(s: Vec<RethRpcModule>) -> Self {
        Self::Selection(s.into_iter().collect())
    }
}

impl<const N: usize> From<[RethRpcModule; N]> for RpcModuleSelection {
    fn from(s: [RethRpcModule; N]) -> Self {
        Self::Selection(s.iter().copied().collect())
    }
}

impl<'a> FromIterator<&'a RethRpcModule> for RpcModuleSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a RethRpcModule>,
    {
        iter.into_iter().copied().collect()
    }
}

impl FromIterator<RethRpcModule> for RpcModuleSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = RethRpcModule>,
    {
        Self::Selection(iter.into_iter().collect())
    }
}

impl FromStr for RpcModuleSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Self::Selection(Default::default()))
        }
        let mut modules = s.split(',').map(str::trim).peekable();
        let first = modules.peek().copied().ok_or(ParseError::VariantNotFound)?;
        match first {
            "all" | "All" => Ok(Self::All),
            "none" | "None" => Ok(Self::Selection(Default::default())),
            _ => Self::try_from_selection(modules),
        }
    }
}

impl fmt::Display for RpcModuleSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}]",
            self.iter_selection().map(|s| s.to_string()).collect::<Vec<_>>().join(", ")
        )
    }
}

/// Represents RPC modules that are supported by reth
#[derive(
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
    AsRefStr,
    IntoStaticStr,
    VariantNames,
    VariantArray,
    EnumIter,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
pub enum RethRpcModule {
    /// `admin_` module
    Admin,
    /// `debug_` module
    Debug,
    /// `eth_` module
    Eth,
    /// `net_` module
    Net,
    /// `trace_` module
    Trace,
    /// `txpool_` module
    Txpool,
    /// `web3_` module
    Web3,
    /// `rpc_` module
    Rpc,
    /// `reth_` module
    Reth,
    /// `ots_` module
    Ots,
}

// === impl RethRpcModule ===

impl RethRpcModule {
    /// Returns the number of variants in the enum
    pub const fn variant_count() -> usize {
        <Self as VariantArray>::VARIANTS.len()
    }

    /// Returns all variant names of the enum
    pub const fn all_variant_names() -> &'static [&'static str] {
        <Self as VariantNames>::VARIANTS
    }

    /// Returns all variants of the enum
    pub const fn all_variants() -> &'static [Self] {
        <Self as VariantArray>::VARIANTS
    }

    /// Returns all variants of the enum
    pub fn modules() -> impl IntoIterator<Item = Self> {
        use strum::IntoEnumIterator;
        Self::iter()
    }

    /// Returns the string representation of the module.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

impl FromStr for RethRpcModule {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "admin" => Self::Admin,
            "debug" => Self::Debug,
            "eth" => Self::Eth,
            "net" => Self::Net,
            "trace" => Self::Trace,
            "txpool" => Self::Txpool,
            "web3" => Self::Web3,
            "rpc" => Self::Rpc,
            "reth" => Self::Reth,
            "ots" => Self::Ots,
            _ => return Err(ParseError::VariantNotFound),
        })
    }
}

impl TryFrom<&str> for RethRpcModule {
    type Error = ParseError;
    fn try_from(s: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {
        FromStr::from_str(s)
    }
}

impl fmt::Display for RethRpcModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_ref())
    }
}

impl Serialize for RethRpcModule {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(self.as_ref())
    }
}
