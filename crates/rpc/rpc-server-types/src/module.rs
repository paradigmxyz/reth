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
        // We convert to lowercase to make the comparison case-insensitive
        //
        // This is a way to allow typing "all" and "ALL" and "All" and "aLl" etc.
        match first.to_lowercase().as_str() {
            "all" => Ok(Self::All),
            "none" => Ok(Self::Selection(Default::default())),
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_all_modules() {
        let all_modules = RpcModuleSelection::all_modules();
        assert_eq!(all_modules.len(), RethRpcModule::variant_count());
    }

    #[test]
    fn test_standard_modules() {
        let standard_modules = RpcModuleSelection::standard_modules();
        let expected_modules: HashSet<RethRpcModule> =
            HashSet::from([RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3]);
        assert_eq!(standard_modules, expected_modules);
    }

    #[test]
    fn test_default_ipc_modules() {
        let default_ipc_modules = RpcModuleSelection::default_ipc_modules();
        assert_eq!(default_ipc_modules, RpcModuleSelection::all_modules());
    }

    #[test]
    fn test_try_from_selection_success() {
        let selection = vec!["eth", "admin"];
        let config = RpcModuleSelection::try_from_selection(selection).unwrap();
        assert_eq!(config, RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]));
    }

    #[test]
    fn test_rpc_module_selection_len() {
        let all_modules = RpcModuleSelection::All;
        let standard = RpcModuleSelection::Standard;
        let selection = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);

        assert_eq!(all_modules.len(), RethRpcModule::variant_count());
        assert_eq!(standard.len(), 3);
        assert_eq!(selection.len(), 2);
    }

    #[test]
    fn test_rpc_module_selection_is_empty() {
        let empty_selection = RpcModuleSelection::from(HashSet::new());
        assert!(empty_selection.is_empty());

        let non_empty_selection = RpcModuleSelection::from([RethRpcModule::Eth]);
        assert!(!non_empty_selection.is_empty());
    }

    #[test]
    fn test_rpc_module_selection_iter_selection() {
        let all_modules = RpcModuleSelection::All;
        let standard = RpcModuleSelection::Standard;
        let selection = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);

        assert_eq!(all_modules.iter_selection().count(), RethRpcModule::variant_count());
        assert_eq!(standard.iter_selection().count(), 3);
        assert_eq!(selection.iter_selection().count(), 2);
    }

    #[test]
    fn test_rpc_module_selection_to_selection() {
        let all_modules = RpcModuleSelection::All;
        let standard = RpcModuleSelection::Standard;
        let selection = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);

        assert_eq!(all_modules.to_selection(), RpcModuleSelection::all_modules());
        assert_eq!(standard.to_selection(), RpcModuleSelection::standard_modules());
        assert_eq!(
            selection.to_selection(),
            HashSet::from([RethRpcModule::Eth, RethRpcModule::Admin])
        );
    }

    #[test]
    fn test_rpc_module_selection_are_identical() {
        // Test scenario: both selections are `All`
        //
        // Since both selections include all possible RPC modules, they should be considered
        // identical.
        let all_modules = RpcModuleSelection::All;
        assert!(RpcModuleSelection::are_identical(Some(&all_modules), Some(&all_modules)));

        // Test scenario: both `http` and `ws` are `None`
        //
        // When both arguments are `None`, the function should return `true` because no modules are
        // selected.
        assert!(RpcModuleSelection::are_identical(None, None));

        // Test scenario: both selections contain identical sets of specific modules
        //
        // In this case, both selections contain the same modules (`Eth` and `Admin`),
        // so they should be considered identical.
        let selection1 = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);
        let selection2 = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);
        assert!(RpcModuleSelection::are_identical(Some(&selection1), Some(&selection2)));

        // Test scenario: one selection is `All`, the other is `Standard`
        //
        // `All` includes all possible modules, while `Standard` includes a specific set of modules.
        // Since `Standard` does not cover all modules, these two selections should not be
        // considered identical.
        let standard = RpcModuleSelection::Standard;
        assert!(!RpcModuleSelection::are_identical(Some(&all_modules), Some(&standard)));

        // Test scenario: one is `None`, the other is an empty selection
        //
        // When one selection is `None` and the other is an empty selection (no modules),
        // they should be considered identical because neither selects any modules.
        let empty_selection = RpcModuleSelection::Selection(HashSet::new());
        assert!(RpcModuleSelection::are_identical(None, Some(&empty_selection)));
        assert!(RpcModuleSelection::are_identical(Some(&empty_selection), None));

        // Test scenario: one is `None`, the other is a non-empty selection
        //
        // If one selection is `None` and the other contains modules, they should not be considered
        // identical because `None` represents no selection, while the other explicitly
        // selects modules.
        let non_empty_selection = RpcModuleSelection::from([RethRpcModule::Eth]);
        assert!(!RpcModuleSelection::are_identical(None, Some(&non_empty_selection)));
        assert!(!RpcModuleSelection::are_identical(Some(&non_empty_selection), None));

        // Test scenario: `All` vs. non-full selection
        //
        // If one selection is `All` (which includes all modules) and the other contains only a
        // subset of modules, they should not be considered identical.
        let partial_selection = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Net]);
        assert!(!RpcModuleSelection::are_identical(Some(&all_modules), Some(&partial_selection)));

        // Test scenario: full selection vs `All`
        //
        // If the other selection explicitly selects all available modules, it should be identical
        // to `All`.
        let full_selection =
            RpcModuleSelection::from(RethRpcModule::modules().into_iter().collect::<HashSet<_>>());
        assert!(RpcModuleSelection::are_identical(Some(&all_modules), Some(&full_selection)));

        // Test scenario: different non-empty selections
        //
        // If the two selections contain different sets of modules, they should not be considered
        // identical.
        let selection3 = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Net]);
        let selection4 = RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Web3]);
        assert!(!RpcModuleSelection::are_identical(Some(&selection3), Some(&selection4)));

        // Test scenario: `Standard` vs an equivalent selection
        // The `Standard` selection includes a predefined set of modules. If we explicitly create
        // a selection with the same set of modules, they should be considered identical.
        let matching_standard =
            RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3]);
        assert!(RpcModuleSelection::are_identical(Some(&standard), Some(&matching_standard)));

        // Test scenario: `Standard` vs non-matching selection
        //
        // If the selection does not match the modules included in `Standard`, they should not be
        // considered identical.
        let non_matching_standard =
            RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Net]);
        assert!(!RpcModuleSelection::are_identical(Some(&standard), Some(&non_matching_standard)));
    }

    #[test]
    fn test_rpc_module_selection_from_str() {
        // Test empty string returns default selection
        let result = RpcModuleSelection::from_str("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::Selection(Default::default()));

        // Test "all" (case insensitive) returns All variant
        let result = RpcModuleSelection::from_str("all");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::All);

        let result = RpcModuleSelection::from_str("All");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::All);

        let result = RpcModuleSelection::from_str("ALL");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::All);

        // Test "none" (case insensitive) returns empty selection
        let result = RpcModuleSelection::from_str("none");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::Selection(Default::default()));

        let result = RpcModuleSelection::from_str("None");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::Selection(Default::default()));

        let result = RpcModuleSelection::from_str("NONE");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::Selection(Default::default()));

        // Test valid selections: "eth,admin"
        let result = RpcModuleSelection::from_str("eth,admin");
        assert!(result.is_ok());
        let expected_selection =
            RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]);
        assert_eq!(result.unwrap(), expected_selection);

        // Test valid selection with extra spaces: " eth , admin "
        let result = RpcModuleSelection::from_str(" eth , admin ");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_selection);

        // Test invalid selection should return error
        let result = RpcModuleSelection::from_str("invalid,unknown");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ParseError::VariantNotFound);

        // Test single valid selection: "eth"
        let result = RpcModuleSelection::from_str("eth");
        assert!(result.is_ok());
        let expected_selection = RpcModuleSelection::from([RethRpcModule::Eth]);
        assert_eq!(result.unwrap(), expected_selection);

        // Test single invalid selection: "unknown"
        let result = RpcModuleSelection::from_str("unknown");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ParseError::VariantNotFound);
    }
}
