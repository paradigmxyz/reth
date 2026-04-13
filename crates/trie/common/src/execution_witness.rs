/// Controls how execution witnesses are generated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ExecutionWitnessMode {
    /// Produces the legacy execution witness format.
    #[default]
    Legacy,
    /// Produces the canonical spec currently implemented in
    /// ethereum/execution-specs@projects/zkevm. The main differences with the
    /// legacy format are:
    /// - For the `bytecode` field:
    ///     - It contains only bytecodes required for execution, compared to Legacy which also
    ///       contains created bytecode.
    ///     - Compared to Legacy, it does not include empty bytecodes (i.e. 0x80).
    ///     - Values are sorted lexicographically ascending.
    /// - For the `state` field:
    ///     - Avoids including empty nodes (i.e. 0x80).
    ///     - Compared to legacy, it does not include storage trie root nodes if no storage is
    ///       accessed.
    ///     - It contains the minimum amount of siblings for post-state root calculation, since it
    ///       does updates/insertions first and then deletions. Compared to legacy which does the
    ///       post-state calculation with removals and then insertions/updates, which results in
    ///       more siblings.
    ///     - Values are sorted lexicographically ascending.
    Canonical,
}

impl ExecutionWitnessMode {
    /// Returns `true` if the mode is [`Self::Canonical`].
    pub const fn is_canonical(self) -> bool {
        matches!(self, Self::Canonical)
    }
}
