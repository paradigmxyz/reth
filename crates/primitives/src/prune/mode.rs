use crate::{BlockNumber, PrunePart, PrunePartError};
use reth_codecs::{main_codec, Compact};

/// Prune mode.
#[main_codec]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum PruneMode {
    /// Prune all blocks.
    Full,
    /// Prune blocks before the `head-N` block number. In other words, keep last N + 1 blocks.
    Distance(u64),
    /// Prune blocks before the specified block number. The specified block number is not pruned.
    Before(BlockNumber),
}

impl PruneMode {
    /// Returns block up to which variant pruning needs to be done, inclusive, according to the
    /// provided tip.
    pub fn prune_target_block(
        &self,
        tip: BlockNumber,
        min_blocks: u64,
        prune_part: PrunePart,
    ) -> Result<Option<(BlockNumber, PruneMode)>, PrunePartError> {
        let result = match self {
            PruneMode::Full if min_blocks == 0 => Some((tip, *self)),
            PruneMode::Distance(distance) if *distance > tip => None, // Nothing to prune yet
            PruneMode::Distance(distance) if *distance >= min_blocks => {
                Some((tip - distance, *self))
            }
            PruneMode::Before(n) if *n > tip => None, // Nothing to prune yet
            PruneMode::Before(n) if tip - n >= min_blocks => Some((n - 1, *self)),
            _ => return Err(PrunePartError::Configuration(prune_part)),
        };
        Ok(result)
    }

    /// Check if target block should be pruned according to the provided prune mode and tip.
    pub fn should_prune(&self, block: BlockNumber, tip: BlockNumber) -> bool {
        match self {
            PruneMode::Full => true,
            PruneMode::Distance(distance) => {
                if *distance > tip {
                    return false
                }
                block < tip - *distance
            }
            PruneMode::Before(n) => *n > block,
        }
    }
}

#[cfg(test)]
impl Default for PruneMode {
    fn default() -> Self {
        Self::Full
    }
}

#[cfg(test)]
mod tests {
    use crate::{prune::PruneMode, PrunePart, PrunePartError, MINIMUM_PRUNING_DISTANCE};
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[test]
    fn test_prune_target_block() {
        let tip = 1000;
        let min_blocks = MINIMUM_PRUNING_DISTANCE;
        let prune_part = PrunePart::Receipts;

        let tests = vec![
            // MINIMUM_PRUNING_DISTANCE makes this impossible
            (PruneMode::Full, Err(PrunePartError::Configuration(prune_part))),
            // Nothing to prune
            (PruneMode::Distance(tip + 1), Ok(None)),
            (PruneMode::Distance(min_blocks + 1), Ok(Some(tip - (min_blocks + 1)))),
            // Nothing to prune
            (PruneMode::Before(tip + 1), Ok(None)),
            (
                PruneMode::Before(tip - MINIMUM_PRUNING_DISTANCE),
                Ok(Some(tip - MINIMUM_PRUNING_DISTANCE - 1)),
            ),
            (
                PruneMode::Before(tip - MINIMUM_PRUNING_DISTANCE - 1),
                Ok(Some(tip - MINIMUM_PRUNING_DISTANCE - 2)),
            ),
            // MINIMUM_PRUNING_DISTANCE is 128
            (PruneMode::Before(tip - 1), Err(PrunePartError::Configuration(prune_part))),
        ];

        for (index, (mode, expected_result)) in tests.into_iter().enumerate() {
            assert_eq!(
                mode.prune_target_block(tip, min_blocks, prune_part),
                expected_result.map(|r| r.map(|b| (b, mode))),
                "Test {} failed",
                index + 1,
            );
        }

        // Test for a scenario where there are no minimum blocks and Full can be used
        assert_eq!(
            PruneMode::Full.prune_target_block(tip, 0, prune_part),
            Ok(Some((tip, PruneMode::Full))),
        );
    }

    #[test]
    fn test_should_prune() {
        let tip = 1000;
        let should_prune = true;

        let tests = vec![
            (PruneMode::Distance(tip + 1), 1, !should_prune),
            (
                PruneMode::Distance(MINIMUM_PRUNING_DISTANCE + 1),
                tip - MINIMUM_PRUNING_DISTANCE - 1,
                !should_prune,
            ),
            (
                PruneMode::Distance(MINIMUM_PRUNING_DISTANCE + 1),
                tip - MINIMUM_PRUNING_DISTANCE - 2,
                should_prune,
            ),
            (PruneMode::Before(tip + 1), 1, should_prune),
            (PruneMode::Before(tip + 1), tip + 1, !should_prune),
        ];

        for (index, (mode, block, expected_result)) in tests.into_iter().enumerate() {
            assert_eq!(mode.should_prune(block, tip), expected_result, "Test {} failed", index + 1,);
        }
    }

    #[test]
    fn prune_mode_deserialize() {
        #[derive(Debug, Deserialize)]
        struct Config {
            a: Option<PruneMode>,
            b: Option<PruneMode>,
            c: Option<PruneMode>,
            d: Option<PruneMode>,
        }

        let toml_str = r#"
        a = "full"
        b = { distance = 10 }
        c = { before = 20 }
    "#;

        assert_matches!(
            toml::from_str(toml_str),
            Ok(Config {
                a: Some(PruneMode::Full),
                b: Some(PruneMode::Distance(10)),
                c: Some(PruneMode::Before(20)),
                d: None
            })
        );
    }
}
