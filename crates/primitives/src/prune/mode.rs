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
        Ok(match self {
            PruneMode::Full if min_blocks == 0 => Some((tip, *self)),
            PruneMode::Distance(distance) if *distance > tip => None, // Nothing to prune yet
            PruneMode::Distance(distance) if *distance >= min_blocks => {
                Some((tip - distance, *self))
            }
            PruneMode::Before(n) if *n > tip => None, // Nothing to prune yet
            PruneMode::Before(n) if tip - n >= min_blocks => Some((n - 1, *self)),
            _ => return Err(PrunePartError::Configuration(prune_part)),
        })
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
    use crate::prune::PruneMode;
    use assert_matches::assert_matches;
    use serde::Deserialize;

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
