use strum::AsRefStr;

#[derive(Debug, Copy, Clone)]
/// Static File filters.
pub enum Filters {
    /// Static File doesn't use any filters.
    WithoutFilters,
}

#[derive(Debug, Copy, Clone, AsRefStr)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
/// Static File inclusion filter. Also see [Filters].
pub enum InclusionFilter {
    #[strum(serialize = "cuckoo")]
    /// Cuckoo filter
    Cuckoo,
}
