/// The version of Engine API message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineApiMessageVersion {
    /// Version 1
    V1,
    /// Version 2
    ///
    /// Added for shanghai hardfork.
    V2,
    /// Version 3
    ///
    /// Added for cancun hardfork.
    V3,
}
