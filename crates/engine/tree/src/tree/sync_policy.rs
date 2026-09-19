//! Selects the catch-up path without bypassing a custom flat-state commitment.

pub(crate) const fn pipeline_gap_exceeded(
    local_tip: u64,
    block: u64,
    threshold: u64,
    flat_root_mode: bool,
) -> bool {
    !flat_root_mode && block > local_tip && block - local_tip > threshold
}

#[cfg(test)]
mod tests {
    use super::pipeline_gap_exceeded;

    #[test]
    fn flat_root_mode_keeps_large_gaps_on_engine_path() {
        assert!(!pipeline_gap_exceeded(100, 133, 32, true));
        assert!(!pipeline_gap_exceeded(0, u64::MAX, 32, true));
    }

    #[test]
    fn ordinary_mode_preserves_pipeline_threshold() {
        assert!(!pipeline_gap_exceeded(100, 99, 32, false));
        assert!(!pipeline_gap_exceeded(100, 100, 32, false));
        assert!(!pipeline_gap_exceeded(100, 132, 32, false));
        assert!(pipeline_gap_exceeded(100, 133, 32, false));
    }
}
