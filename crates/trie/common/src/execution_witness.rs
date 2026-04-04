/// Controls how execution witnesses are generated.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum ExecutionWitnessMode {
    /// Preserve the current witness shape for compatibility with existing consumers.
    #[default]
    Legacy,
    /// Produce the minimized, draft-spec witness form.
    Canonical,
}

impl ExecutionWitnessMode {
    /// Returns `true` if the mode is [`Self::Canonical`].
    pub const fn is_canonical(self) -> bool {
        matches!(self, Self::Canonical)
    }
}
