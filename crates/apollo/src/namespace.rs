//! Apollo namespace and component name constants for configuration management.
/// Component name prefixes for validation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// OpReth component
    OpReth,
}

impl Component {
    /// Get the string prefix for the component
    pub const fn as_str(&self) -> &str {
        match self {
            Component::OpReth => "opreth",
        }
    }

    /// Get the string prefix for the component with the namespace
    pub fn with_namespace(self, namespace: Namespace) -> String {
        format!("{}_{}", self.as_str(), namespace.as_str())
    }
}
/// Namespace identifiers with component prefixes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    /// JsonRPC configuration namespace
    JsonRpc,
    /// Sequencer configuration for op-reth
    Sequencer,
    /// Halt control namespace
    Halt,
}

impl Namespace {
    /// Get the full namespace string
    pub const fn as_str(&self) -> &str {
        match self {
            Namespace::JsonRpc => "jsonrpc",
            Namespace::Sequencer => "sequencer",
            Namespace::Halt => "halt",
        }
    }
}
