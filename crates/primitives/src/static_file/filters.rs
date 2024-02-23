use strum::AsRefStr;

#[derive(Debug, Copy, Clone)]
/// Snapshot filters.
pub enum Filters {
    /// Snapshot uses filters with [InclusionFilter] and [PerfectHashingFunction].
    WithFilters(InclusionFilter, PerfectHashingFunction),
    /// Snapshot doesn't use any filters.
    WithoutFilters,
}

impl Filters {
    /// Returns `true` if snapshot uses filters.
    pub const fn has_filters(&self) -> bool {
        matches!(self, Self::WithFilters(_, _))
    }
}

#[derive(Debug, Copy, Clone, AsRefStr)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Snapshot inclusion filter. Also see [Filters].
pub enum InclusionFilter {
    #[strum(serialize = "cuckoo")]
    /// Cuckoo filter
    Cuckoo,
}

#[derive(Debug, Copy, Clone, AsRefStr)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Snapshot perfect hashing  function. Also see [Filters].
pub enum PerfectHashingFunction {
    #[strum(serialize = "fmph")]
    /// Fingerprint-Based Minimal Perfect Hash Function
    Fmph,
    #[strum(serialize = "gofmph")]
    /// Fingerprint-Based Minimal Perfect Hash Function with Group Optimization
    GoFmph,
}
