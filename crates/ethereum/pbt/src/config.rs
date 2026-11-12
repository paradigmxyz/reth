//! Explicit configuration for PBT execution and onboarding.

use crate::{CatchupLimits, DecodeLimits, MigrationFiles, DEFAULT_CHECKPOINT_HISTORY};

/// Node-owned PBT settings. Construct and validate CLI or service configuration before launch.
#[derive(Clone, Debug)]
pub struct PbtConfig {
    /// Optional finalized migration anchor and its canonical artifact inputs.
    pub migration: Option<MigrationFiles>,
    /// Onboarding range limits and per-chunk staged replay budgets.
    pub catchup: CatchupLimits,
    /// Bounds for untrusted migration artifacts; live native state uses independent limits.
    pub import_limits: DecodeLimits,
    /// Estimated full-state cache byte budget. `None` uses the store's 1 GiB default.
    pub cache_max_bytes: Option<usize>,
    /// Number of blocks retained below finality, in addition to genesis and retirement.
    pub checkpoint_history: u64,
    /// Whether to irreversibly retire MPT storage after post-activation finality.
    /// Defaults to false; enabling PBT commitments alone retains MPT storage.
    pub retire_mpt: bool,
}

impl Default for PbtConfig {
    fn default() -> Self {
        Self {
            migration: None,
            catchup: CatchupLimits::default(),
            import_limits: DecodeLimits::default(),
            cache_max_bytes: None,
            checkpoint_history: DEFAULT_CHECKPOINT_HISTORY,
            retire_mpt: false,
        }
    }
}
