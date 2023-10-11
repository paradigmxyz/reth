#[derive(Debug, Copy, Clone)]
pub enum Filters {
    WithFilters(PerfectHashingFunction),
    WithoutFilters,
}

impl Filters {
    pub const fn has_filters(&self) -> bool {
        matches!(self, Self::WithFilters(_))
    }
}

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum PerfectHashingFunction {
    Mphf,
    GoMphf,
}
