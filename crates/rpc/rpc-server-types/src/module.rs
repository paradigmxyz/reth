use std::{collections::HashSet, fmt, str::FromStr};

use serde::{Deserialize, Serialize, Serializer};
use strum::{ParseError, VariantNames};

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

    /// Returns true if all modules are selected
    pub const fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Returns an iterator over all configured [`RethRpcModule`]
    pub fn iter_selection(&self) -> Box<dyn Iterator<Item = RethRpcModule> + '_> {
        match self {
            Self::All => Box::new(RethRpcModule::modules().into_iter()),
            Self::Standard => Box::new(Self::STANDARD_MODULES.iter().cloned()),
            Self::Selection(s) => Box::new(s.iter().cloned()),
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

    /// Returns true if the selection contains the given module.
    pub fn contains(&self, module: &RethRpcModule) -> bool {
        match self {
            Self::All => true,
            Self::Standard => Self::STANDARD_MODULES.contains(module),
            Self::Selection(s) => s.contains(module),
        }
    }

    /// Adds a module to the selection.
    ///
    /// If the selection is `All`, this is a no-op.
    /// Otherwise, converts to a `Selection` and adds the module.
    pub fn push(&mut self, module: RethRpcModule) {
        if !self.is_all() {
            let mut modules = self.to_selection();
            modules.insert(module);
            *self = Self::Selection(modules);
        }
    }

    /// Returns a new selection with the given module added.
    ///
    /// If the selection is `All`, returns `All`.
    /// Otherwise, converts to a `Selection` and adds the module.
    pub fn append(self, module: RethRpcModule) -> Self {
        if self.is_all() {
            Self::All
        } else {
            let mut modules = self.into_selection();
            modules.insert(module);
            Self::Selection(modules)
        }
    }

    /// Extends the selection with modules from an iterator.
    ///
    /// If the selection is `All`, this is a no-op.
    /// Otherwise, converts to a `Selection` and adds the modules.
    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = RethRpcModule>,
    {
        if !self.is_all() {
            let mut modules = self.to_selection();
            modules.extend(iter);
            *self = Self::Selection(modules);
        }
    }

    /// Returns a new selection with modules from an iterator added.
    ///
    /// If the selection is `All`, returns `All`.
    /// Otherwise, converts to a `Selection` and adds the modules.
    pub fn extended<I>(self, iter: I) -> Self
    where
        I: IntoIterator<Item = RethRpcModule>,
    {
        if self.is_all() {
            Self::All
        } else {
            let mut modules = self.into_selection();
            modules.extend(iter);
            Self::Selection(modules)
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
        Self::Selection(s.iter().cloned().collect())
    }
}

impl From<Vec<RethRpcModule>> for RpcModuleSelection {
    fn from(s: Vec<RethRpcModule>) -> Self {
        Self::Selection(s.into_iter().collect())
    }
}

impl<const N: usize> From<[RethRpcModule; N]> for RpcModuleSelection {
    fn from(s: [RethRpcModule; N]) -> Self {
        Self::Selection(s.iter().cloned().collect())
    }
}

impl<'a> FromIterator<&'a RethRpcModule> for RpcModuleSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a RethRpcModule>,
    {
        iter.into_iter().cloned().collect()
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
#[derive(Debug, Clone, Eq, PartialEq, Hash, VariantNames, Deserialize)]
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
    /// `flashbots_` module
    Flashbots,
    /// `miner_` module
    Miner,
    /// `mev_` module
    Mev,
    /// Custom RPC module not part of the standard set
    #[strum(default)]
    #[serde(untagged)]
    Other(String),
}

// === impl RethRpcModule ===

impl RethRpcModule {
    /// All standard variants (excludes Other)
    const STANDARD_VARIANTS: &'static [Self] = &[
        Self::Admin,
        Self::Debug,
        Self::Eth,
        Self::Net,
        Self::Trace,
        Self::Txpool,
        Self::Web3,
        Self::Rpc,
        Self::Reth,
        Self::Ots,
        Self::Flashbots,
        Self::Miner,
        Self::Mev,
    ];

    /// Returns the number of standard variants (excludes Other)
    pub const fn variant_count() -> usize {
        Self::STANDARD_VARIANTS.len()
    }

    /// Returns all variant names including Other (for parsing)
    pub const fn all_variant_names() -> &'static [&'static str] {
        <Self as VariantNames>::VARIANTS
    }

    /// Returns standard variant names (excludes "other") for CLI display
    pub fn standard_variant_names() -> impl Iterator<Item = &'static str> {
        <Self as VariantNames>::VARIANTS.iter().copied().filter(|&name| name != "other")
    }

    /// Returns all standard variants (excludes Other)
    pub const fn all_variants() -> &'static [Self] {
        Self::STANDARD_VARIANTS
    }

    /// Returns iterator over standard modules only
    pub fn modules() -> impl IntoIterator<Item = Self> + Clone {
        Self::STANDARD_VARIANTS.iter().cloned()
    }

    /// Returns the string representation of the module.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Other(s) => s.as_str(),
            _ => self.as_ref(), // Uses AsRefStr trait
        }
    }

    /// Returns true if this is an `Other` variant.
    pub const fn is_other(&self) -> bool {
        matches!(self, Self::Other(_))
    }
}

impl AsRef<str> for RethRpcModule {
    fn as_ref(&self) -> &str {
        match self {
            Self::Other(s) => s.as_str(),
            // For standard variants, use the derive-generated static strings
            Self::Admin => "admin",
            Self::Debug => "debug",
            Self::Eth => "eth",
            Self::Net => "net",
            Self::Trace => "trace",
            Self::Txpool => "txpool",
            Self::Web3 => "web3",
            Self::Rpc => "rpc",
            Self::Reth => "reth",
            Self::Ots => "ots",
            Self::Flashbots => "flashbots",
            Self::Miner => "miner",
            Self::Mev => "mev",
        }
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
            "flashbots" => Self::Flashbots,
            "miner" => Self::Miner,
            "mev" => Self::Mev,
            // Any unknown module becomes Other
            other => Self::Other(other.to_string()),
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
        s.serialize_str(self.as_str())
    }
}

/// Trait for validating RPC module selections.
///
/// This allows customizing how RPC module names are validated when parsing
/// CLI arguments or configuration.
pub trait RpcModuleValidator: Clone + Send + Sync + 'static {
    /// Parse and validate an RPC module selection string.
    fn parse_selection(s: &str) -> Result<RpcModuleSelection, String>;

    /// Validates RPC module selection that was already parsed.
    ///
    /// This is used to validate modules that were parsed as `Other` variants
    /// to ensure they meet the validation rules of the specific implementation.
    fn validate_selection(modules: &RpcModuleSelection, arg_name: &str) -> Result<(), String> {
        // Re-validate the modules using the parser's validator
        // This is necessary because the clap value parser accepts any input
        // and we need to validate according to the specific parser's rules
        let RpcModuleSelection::Selection(module_set) = modules else {
            // All or Standard variants are always valid
            return Ok(());
        };

        for module in module_set {
            let RethRpcModule::Other(name) = module else {
                // Standard modules are always valid
                continue;
            };

            // Try to parse and validate using the configured validator
            // This will check for typos and other validation rules
            Self::parse_selection(name)
                .map_err(|e| format!("Invalid RPC module '{name}' in {arg_name}: {e}"))?;
        }

        Ok(())
    }
}

/// Default validator that rejects unknown module names.
///
/// This validator only accepts known RPC module names.
#[derive(Debug, Clone, Copy)]
pub struct DefaultRpcModuleValidator;

impl RpcModuleValidator for DefaultRpcModuleValidator {
    fn parse_selection(s: &str) -> Result<RpcModuleSelection, String> {
        // First try standard parsing
        let selection = RpcModuleSelection::from_str(s)
            .map_err(|e| format!("Failed to parse RPC modules: {}", e))?;

        // Validate each module in the selection
        if let RpcModuleSelection::Selection(modules) = &selection {
            for module in modules {
                if let RethRpcModule::Other(name) = module {
                    return Err(format!("Unknown RPC module: '{}'", name));
                }
            }
        }

        Ok(selection)
    }
}

/// Lenient validator that accepts any module name without validation.
///
/// This validator accepts any module name, including unknown ones.
#[derive(Debug, Clone, Copy)]
pub struct LenientRpcModuleValidator;

impl RpcModuleValidator for LenientRpcModuleValidator {
    fn parse_selection(s: &str) -> Result<RpcModuleSelection, String> {
        RpcModuleSelection::from_str(s).map_err(|e| format!("Failed to parse RPC modules: {}", e))
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
    fn test_rpc_module_selection_append() {
        // Test append on Standard selection
        let selection = RpcModuleSelection::Standard;
        let new_selection = selection.append(RethRpcModule::Admin);
        assert!(new_selection.contains(&RethRpcModule::Eth));
        assert!(new_selection.contains(&RethRpcModule::Net));
        assert!(new_selection.contains(&RethRpcModule::Web3));
        assert!(new_selection.contains(&RethRpcModule::Admin));

        // Test append on empty Selection
        let selection = RpcModuleSelection::Selection(HashSet::new());
        let new_selection = selection.append(RethRpcModule::Eth);
        assert!(new_selection.contains(&RethRpcModule::Eth));
        assert_eq!(new_selection.len(), 1);

        // Test append on All (should return All)
        let selection = RpcModuleSelection::All;
        let new_selection = selection.append(RethRpcModule::Eth);
        assert_eq!(new_selection, RpcModuleSelection::All);
    }

    #[test]
    fn test_rpc_module_selection_extend() {
        // Test extend on Standard selection
        let mut selection = RpcModuleSelection::Standard;
        selection.extend(vec![RethRpcModule::Admin, RethRpcModule::Debug]);
        assert!(selection.contains(&RethRpcModule::Eth));
        assert!(selection.contains(&RethRpcModule::Net));
        assert!(selection.contains(&RethRpcModule::Web3));
        assert!(selection.contains(&RethRpcModule::Admin));
        assert!(selection.contains(&RethRpcModule::Debug));

        // Test extend on empty Selection
        let mut selection = RpcModuleSelection::Selection(HashSet::new());
        selection.extend(vec![RethRpcModule::Eth, RethRpcModule::Admin]);
        assert!(selection.contains(&RethRpcModule::Eth));
        assert!(selection.contains(&RethRpcModule::Admin));
        assert_eq!(selection.len(), 2);

        // Test extend on All (should be no-op)
        let mut selection = RpcModuleSelection::All;
        selection.extend(vec![RethRpcModule::Eth, RethRpcModule::Admin]);
        assert_eq!(selection, RpcModuleSelection::All);
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

        // Test custom module selections now work (no longer return errors)
        let result = RpcModuleSelection::from_str("invalid,unknown");
        assert!(result.is_ok());
        let selection = result.unwrap();
        assert!(selection.contains(&RethRpcModule::Other("invalid".to_string())));
        assert!(selection.contains(&RethRpcModule::Other("unknown".to_string())));

        // Test single valid selection: "eth"
        let result = RpcModuleSelection::from_str("eth");
        assert!(result.is_ok());
        let expected_selection = RpcModuleSelection::from([RethRpcModule::Eth]);
        assert_eq!(result.unwrap(), expected_selection);

        // Test single custom module selection: "unknown" now becomes Other
        let result = RpcModuleSelection::from_str("unknown");
        assert!(result.is_ok());
        let expected_selection =
            RpcModuleSelection::from([RethRpcModule::Other("unknown".to_string())]);
        assert_eq!(result.unwrap(), expected_selection);
    }

    #[test]
    fn test_rpc_module_other_variant() {
        // Test parsing custom module
        let custom_module = RethRpcModule::from_str("myCustomModule").unwrap();
        assert_eq!(custom_module, RethRpcModule::Other("myCustomModule".to_string()));

        // Test as_str for Other variant
        assert_eq!(custom_module.as_str(), "myCustomModule");

        // Test as_ref for Other variant
        assert_eq!(custom_module.as_ref(), "myCustomModule");

        // Test Display impl
        assert_eq!(custom_module.to_string(), "myCustomModule");
    }

    #[test]
    fn test_rpc_module_selection_with_mixed_modules() {
        // Test selection with both standard and custom modules
        let result = RpcModuleSelection::from_str("eth,admin,myCustomModule,anotherCustom");
        assert!(result.is_ok());

        let selection = result.unwrap();
        assert!(selection.contains(&RethRpcModule::Eth));
        assert!(selection.contains(&RethRpcModule::Admin));
        assert!(selection.contains(&RethRpcModule::Other("myCustomModule".to_string())));
        assert!(selection.contains(&RethRpcModule::Other("anotherCustom".to_string())));
    }

    #[test]
    fn test_rpc_module_all_excludes_custom() {
        // Test that All selection doesn't include custom modules
        let all_selection = RpcModuleSelection::All;

        // All should contain standard modules
        assert!(all_selection.contains(&RethRpcModule::Eth));
        assert!(all_selection.contains(&RethRpcModule::Admin));

        // But All doesn't explicitly contain custom modules
        // (though contains() returns true for all modules when selection is All)
        assert_eq!(all_selection.len(), RethRpcModule::variant_count());
    }

    #[test]
    fn test_rpc_module_equality_with_other() {
        let other1 = RethRpcModule::Other("custom".to_string());
        let other2 = RethRpcModule::Other("custom".to_string());
        let other3 = RethRpcModule::Other("different".to_string());

        assert_eq!(other1, other2);
        assert_ne!(other1, other3);
        assert_ne!(other1, RethRpcModule::Eth);
    }

    #[test]
    fn test_rpc_module_is_other() {
        // Standard modules should return false
        assert!(!RethRpcModule::Eth.is_other());
        assert!(!RethRpcModule::Admin.is_other());
        assert!(!RethRpcModule::Debug.is_other());

        // Other variants should return true
        assert!(RethRpcModule::Other("custom".to_string()).is_other());
        assert!(RethRpcModule::Other("mycustomrpc".to_string()).is_other());
    }

    #[test]
    fn test_standard_variant_names_excludes_other() {
        let standard_names: Vec<_> = RethRpcModule::standard_variant_names().collect();

        // Verify "other" is not in the list
        assert!(!standard_names.contains(&"other"));

        // Should have exactly as many names as STANDARD_VARIANTS
        assert_eq!(standard_names.len(), RethRpcModule::STANDARD_VARIANTS.len());

        // Verify all standard variants have their names in the list
        for variant in RethRpcModule::STANDARD_VARIANTS {
            assert!(standard_names.contains(&variant.as_ref()));
        }
    }

    #[test]
    fn test_default_validator_accepts_standard_modules() {
        // Should accept standard modules
        let result = DefaultRpcModuleValidator::parse_selection("eth,admin,debug");
        assert!(result.is_ok());

        let selection = result.unwrap();
        assert!(matches!(selection, RpcModuleSelection::Selection(_)));
    }

    #[test]
    fn test_default_validator_rejects_unknown_modules() {
        // Should reject unknown module names
        let result = DefaultRpcModuleValidator::parse_selection("eth,mycustom");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown RPC module: 'mycustom'"));

        let result = DefaultRpcModuleValidator::parse_selection("unknownmodule");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown RPC module: 'unknownmodule'"));

        let result = DefaultRpcModuleValidator::parse_selection("eth,admin,xyz123");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown RPC module: 'xyz123'"));
    }

    #[test]
    fn test_default_validator_all_selection() {
        // Should accept "all" selection
        let result = DefaultRpcModuleValidator::parse_selection("all");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::All);
    }

    #[test]
    fn test_default_validator_none_selection() {
        // Should accept "none" selection
        let result = DefaultRpcModuleValidator::parse_selection("none");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), RpcModuleSelection::Selection(Default::default()));
    }

    #[test]
    fn test_lenient_validator_accepts_unknown_modules() {
        // Lenient validator should accept any module name without validation
        let result = LenientRpcModuleValidator::parse_selection("eht,adimn,xyz123,customrpc");
        assert!(result.is_ok());

        let selection = result.unwrap();
        if let RpcModuleSelection::Selection(modules) = selection {
            assert!(modules.contains(&RethRpcModule::Other("eht".to_string())));
            assert!(modules.contains(&RethRpcModule::Other("adimn".to_string())));
            assert!(modules.contains(&RethRpcModule::Other("xyz123".to_string())));
            assert!(modules.contains(&RethRpcModule::Other("customrpc".to_string())));
        } else {
            panic!("Expected Selection variant");
        }
    }

    #[test]
    fn test_default_validator_mixed_standard_and_custom() {
        // Should reject mix of standard and custom modules
        let result = DefaultRpcModuleValidator::parse_selection("eth,admin,mycustom,debug");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown RPC module: 'mycustom'"));
    }
}
